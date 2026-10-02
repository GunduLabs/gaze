// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashSet;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use image::RgbImage;
use nalgebra::Matrix3;
use ndarray::Array4;
use ort::{session::Session, value::TensorRef};

use gaze_core::config::InferenceConfig;
use gaze_vision::inference::create_session;

use crate::users::UserDatabase;

pub const DURESS_DIR: &str = "/var/lib/gaze/duress";

const INPUT_SIZE: u32 = 32;
const EYE_CROP_IPD_RATIO: f32 = 0.5;
const PIXEL_MEAN: f32 = 127.0;
const PIXEL_SCALE: f32 = 255.0;
const CLOSED_CLASS: usize = 0;
const CLASS_COUNT: usize = 2;

const MAX_CLOSED_GAP: Duration = Duration::from_millis(500);

pub struct EyeStateClassifier {
    session: Session,
}

impl EyeStateClassifier {
    pub fn new_with_inference(
        model_path: &str,
        inference: &InferenceConfig,
    ) -> anyhow::Result<Self> {
        let (session, _) = create_session(model_path, inference)?;
        Ok(Self { session })
    }

    fn eye_transform(eye: (f32, f32), ipd: f32) -> Option<Matrix3<f32>> {
        let side = ipd * EYE_CROP_IPD_RATIO;
        if !side.is_finite() || side <= f32::EPSILON || !eye.0.is_finite() || !eye.1.is_finite() {
            return None;
        }
        let scale = INPUT_SIZE as f32 / side;
        let half = INPUT_SIZE as f32 / 2.0;
        Some(Matrix3::new(
            scale,
            0.0,
            half - eye.0 * scale,
            0.0,
            scale,
            half - eye.1 * scale,
            0.0,
            0.0,
            1.0,
        ))
    }

    fn pre_process(crop: &RgbImage) -> Array4<f32> {
        let size = INPUT_SIZE as usize;
        let plane_len = size * size;
        let mut tensor = Array4::<f32>::zeros((1, 3, size, size));
        let data = tensor
            .as_slice_mut()
            .expect("preprocess tensor should be contiguous");

        for (x, y, p) in crop.enumerate_pixels() {
            let idx = y as usize * size + x as usize;
            data[idx] = (p[2] as f32 - PIXEL_MEAN) / PIXEL_SCALE;
            data[plane_len + idx] = (p[1] as f32 - PIXEL_MEAN) / PIXEL_SCALE;
            data[2 * plane_len + idx] = (p[0] as f32 - PIXEL_MEAN) / PIXEL_SCALE;
        }
        tensor
    }

    fn closed_probability_from_output(data: &[f32]) -> anyhow::Result<f32> {
        if data.len() != CLASS_COUNT {
            anyhow::bail!(
                "eye-state model produced {} scores, expected {CLASS_COUNT}",
                data.len()
            );
        }
        let closed = data[CLOSED_CLASS];
        if !closed.is_finite() {
            anyhow::bail!("eye-state model produced a non-finite score");
        }
        Ok(closed.clamp(0.0, 1.0))
    }

    pub fn closed_probabilities(
        &mut self,
        img: &RgbImage,
        eyes: [(f32, f32); 2],
    ) -> anyhow::Result<[f32; 2]> {
        let ipd = (eyes[0].0 - eyes[1].0).hypot(eyes[0].1 - eyes[1].1);
        let mut probabilities = [0.0; 2];
        for (eye, probability) in eyes.into_iter().zip(probabilities.iter_mut()) {
            let transform = Self::eye_transform(eye, ipd)
                .ok_or_else(|| anyhow::anyhow!("degenerate eye landmarks"))?;
            let crop = crate::align::warp_affine(img, &transform, INPUT_SIZE, INPUT_SIZE);
            let tensor = Self::pre_process(&crop);
            let inputs = ort::inputs![TensorRef::from_array_view(&tensor)?];
            let outputs = self.session.run(inputs)?;
            let (_shape, data) = outputs[0].try_extract_tensor::<f32>()?;
            *probability = Self::closed_probability_from_output(data)?;
        }
        Ok(probabilities)
    }
}

pub fn any_eye_closed(probabilities: [f32; 2], threshold: f32) -> bool {
    probabilities.iter().any(|p| *p >= threshold)
}

pub struct DuressTracker {
    hold: Duration,
    closed_since: Option<Instant>,
    last_closed: Option<Instant>,
}

impl DuressTracker {
    pub fn new(hold: Duration) -> Self {
        Self {
            hold,
            closed_since: None,
            last_closed: None,
        }
    }

    pub fn observe(&mut self, closed: bool, now: Instant) -> bool {
        if !closed {
            self.closed_since = None;
            self.last_closed = None;
            return false;
        }
        let continues = self
            .last_closed
            .is_some_and(|last| now.saturating_duration_since(last) <= MAX_CLOSED_GAP);
        if !continues {
            self.closed_since = Some(now);
        }
        self.last_closed = Some(now);
        self.closed_since
            .is_some_and(|since| now.saturating_duration_since(since) >= self.hold)
    }
}

pub struct DuressLockout {
    dir: PathBuf,
    locked: Mutex<HashSet<String>>,
}

impl DuressLockout {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            locked: Mutex::new(HashSet::new()),
        }
    }

    fn marker(&self, username: &str) -> anyhow::Result<PathBuf> {
        UserDatabase::validate_username(username).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(self.dir.join(username))
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.locked.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_locked(&self, username: &str) -> bool {
        if self.memory().contains(username) {
            return true;
        }
        let Ok(marker) = self.marker(username) else {
            return true;
        };
        match std::fs::symlink_metadata(&marker) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    }

    pub fn lock(&self, username: &str) -> anyhow::Result<()> {
        self.memory().insert(username.to_string());
        let marker = self.marker(username)?;
        ensure_private_dir(&self.dir)?;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&marker)?
            .sync_all()?;
        Ok(())
    }

    pub fn clear(&self, username: &str) -> anyhow::Result<bool> {
        let was_in_memory = self.memory().remove(username);
        let marker = self.marker(username)?;
        match std::fs::remove_file(&marker) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(was_in_memory),
            Err(e) => {
                self.memory().insert(username.to_string());
                Err(e.into())
            }
        }
    }
}

fn ensure_private_dir(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        anyhow::bail!("{} is not a private directory", path.display());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("gaze-duress-{name}-{}-{nanos}", std::process::id()));
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn pre_process_outputs_nchw_bgr_scaled_around_127() {
        let crop = RgbImage::from_pixel(INPUT_SIZE, INPUT_SIZE, Rgb([255, 127, 0]));
        let tensor = EyeStateClassifier::pre_process(&crop);
        assert_eq!(tensor.shape(), &[1, 3, 32, 32]);
        assert!((tensor[[0, 0, 0, 0]] - (-127.0 / 255.0)).abs() < 1e-6);
        assert!(tensor[[0, 1, 0, 0]].abs() < 1e-6);
        assert!((tensor[[0, 2, 0, 0]] - (128.0 / 255.0)).abs() < 1e-6);
    }

    #[test]
    fn eye_transform_centres_a_half_ipd_square_on_the_eye() {
        let transform = EyeStateClassifier::eye_transform((100.0, 50.0), 64.0).unwrap();
        let centre = transform * nalgebra::Vector3::new(100.0, 50.0, 1.0);
        assert!((centre.x - 16.0).abs() < 1e-4 && (centre.y - 16.0).abs() < 1e-4);
        let corner = transform * nalgebra::Vector3::new(116.0, 66.0, 1.0);
        assert!((corner.x - 32.0).abs() < 1e-4 && (corner.y - 32.0).abs() < 1e-4);
    }

    #[test]
    fn eye_transform_rejects_degenerate_landmarks() {
        assert!(EyeStateClassifier::eye_transform((10.0, 10.0), 0.0).is_none());
        assert!(EyeStateClassifier::eye_transform((f32::NAN, 10.0), 40.0).is_none());
        assert!(EyeStateClassifier::eye_transform((10.0, 10.0), f32::INFINITY).is_none());
    }

    #[test]
    fn closed_probability_reads_the_first_class() {
        let p = EyeStateClassifier::closed_probability_from_output(&[0.93, 0.07]).unwrap();
        assert!((p - 0.93).abs() < 1e-6);
        assert!(EyeStateClassifier::closed_probability_from_output(&[0.5]).is_err());
        assert!(EyeStateClassifier::closed_probability_from_output(&[f32::NAN, 0.5]).is_err());
    }

    #[test]
    fn either_closed_eye_counts() {
        assert!(any_eye_closed([0.95, 0.01], 0.9));
        assert!(any_eye_closed([0.01, 0.92], 0.9));
        assert!(!any_eye_closed([0.4, 0.89], 0.9));
    }

    #[test]
    fn a_blink_never_reaches_the_hold() {
        let start = Instant::now();
        let mut tracker = DuressTracker::new(Duration::from_millis(600));
        assert!(!tracker.observe(true, start));
        assert!(!tracker.observe(true, start + Duration::from_millis(200)));
        assert!(!tracker.observe(false, start + Duration::from_millis(250)));
        assert!(!tracker.observe(true, start + Duration::from_millis(300)));
        assert!(!tracker.observe(true, start + Duration::from_millis(800)));
    }

    #[test]
    fn a_held_closure_trips_after_the_hold() {
        let start = Instant::now();
        let mut tracker = DuressTracker::new(Duration::from_millis(600));
        for ms in (0..600).step_by(100) {
            assert!(!tracker.observe(true, start + Duration::from_millis(ms)));
        }
        assert!(tracker.observe(true, start + Duration::from_millis(600)));
    }

    #[test]
    fn a_long_gap_restarts_the_hold() {
        let start = Instant::now();
        let mut tracker = DuressTracker::new(Duration::from_millis(600));
        assert!(!tracker.observe(true, start));
        assert!(!tracker.observe(true, start + Duration::from_millis(2000)));
        assert!(!tracker.observe(true, start + Duration::from_millis(2300)));
        assert!(tracker.observe(true, start + Duration::from_millis(2600)));
    }

    #[test]
    fn a_zero_hold_trips_on_the_first_closed_frame() {
        let mut tracker = DuressTracker::new(Duration::ZERO);
        assert!(tracker.observe(true, Instant::now()));
    }

    #[test]
    fn lockout_persists_across_instances_until_cleared() {
        let temp = TempDir::new("persist");
        let lockout = DuressLockout::new(&temp.path);
        assert!(!lockout.is_locked("alice"));

        lockout.lock("alice").unwrap();
        assert!(lockout.is_locked("alice"));
        assert!(!lockout.is_locked("bob"));
        let mode = std::fs::metadata(&temp.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let reloaded = DuressLockout::new(&temp.path);
        assert!(reloaded.is_locked("alice"));
        assert!(reloaded.clear("alice").unwrap());
        assert!(!reloaded.is_locked("alice"));
        assert!(!reloaded.clear("alice").unwrap());
    }

    #[test]
    fn lockout_rejects_path_tricks_and_fails_closed_on_them() {
        let temp = TempDir::new("names");
        let lockout = DuressLockout::new(&temp.path);
        assert!(lockout.lock("../etc").is_err());
        assert!(lockout.is_locked("../etc"));
        assert!(lockout.clear("a/b").is_err());
    }

    #[test]
    fn lockout_holds_in_memory_when_the_directory_is_unwritable() {
        let temp = TempDir::new("unwritable");
        std::fs::write(&temp.path, b"not a directory").unwrap();
        let lockout = DuressLockout::new(temp.path.join("duress"));
        assert!(lockout.lock("alice").is_err());
        assert!(lockout.is_locked("alice"));
        std::fs::remove_file(&temp.path).unwrap();
    }
}
