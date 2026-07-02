use crate::camera::frame_to_bytes;
use crate::config::Config;
use crate::dbus::{CaptureStatus, EnrollPrompt};
use crate::detect::{DetectError, FaceDetector};
use opencv::core::Mat;
use opencv::prelude::*;
use std::sync::{Mutex, MutexGuard};

const MIN_FACE_SIZE_RATIO: f32 = 0.25;
const MAX_FACE_SIZE_RATIO: f32 = 0.78;
const ENROLL_POSE_STABILITY_WINDOW: usize = 2;
const ENROLL_STABLE_YAW_RANGE: f32 = 0.08;
const ENROLL_STABLE_PITCH_RANGE: f32 = 0.06;
const ENROLL_HORIZONTAL_POSE_DELTA: f32 = 0.10;
const ENROLL_VERTICAL_POSE_DELTA: f32 = 0.04;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Spectrum {
    Rgb,
    Ir,
}

pub struct CaptureResult {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bbox: Option<(f32, f32, f32, f32)>,
    pub kpss: Option<ndarray::Array3<f32>>,
    pub mat_rgb: Option<opencv::core::Mat>,
    pub yaw: f32,
    pub pitch: f32,
}

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// True when a square-padded detector bbox is within 5% of the original image edge.
fn bbox_is_clipped(bbox: (f32, f32, f32, f32), frame_w: f32, frame_h: f32) -> bool {
    const EDGE_MARGIN: f32 = 0.05;
    let max_dim = frame_w.max(frame_h);
    let pad_x = ((max_dim - frame_w) / 2.0).floor();
    let pad_y = ((max_dim - frame_h) / 2.0).floor();
    let (x1, y1, x2, y2) = bbox;

    x1 - pad_x < EDGE_MARGIN * frame_w
        || y1 - pad_y < EDGE_MARGIN * frame_h
        || x2 - pad_x > (1.0 - EDGE_MARGIN) * frame_w
        || y2 - pad_y > (1.0 - EDGE_MARGIN) * frame_h
}

fn geometry_status(
    bbox: (f32, f32, f32, f32),
    frame_w: f32,
    frame_h: f32,
    check_centering_and_proximity: bool,
) -> Option<CaptureStatus> {
    if bbox_is_clipped(bbox, frame_w, frame_h) {
        return Some(CaptureStatus::Clipped);
    }
    if !check_centering_and_proximity {
        return None;
    }

    let (x1, y1, x2, y2) = bbox;
    let max_dim = frame_w.max(frame_h);
    let min_dim = frame_w.min(frame_h);
    let (width, height) = (x2 - x1, y2 - y1);
    let (cx, cy) = (x1 + width / 2.0, y1 + height / 2.0);
    let (norm_cx, norm_cy) = (cx / max_dim, cy / max_dim);
    let face_size_ratio = width.max(height) / min_dim;

    if (norm_cx - 0.5).abs() >= 0.2 || (norm_cy - 0.5).abs() >= 0.2 {
        Some(CaptureStatus::NotCentered)
    } else if face_size_ratio < MIN_FACE_SIZE_RATIO {
        Some(CaptureStatus::TooFar)
    } else if face_size_ratio > MAX_FACE_SIZE_RATIO {
        Some(CaptureStatus::TooClose)
    } else {
        None
    }
}

pub fn estimate_head_pose(kps: &ndarray::Array3<f32>) -> Option<(f32, f32)> {
    let shape = kps.shape();
    if shape[0] < 1 || shape[1] < 5 || shape[2] < 2 {
        return None;
    }

    let point = |index| (kps[[0, index, 0]], kps[[0, index, 1]]);
    let (lx, ly) = point(0);
    let (rx, ry) = point(1);
    let (nx, ny) = point(2);
    let (mlx, mly) = point(3);
    let (mrx, mry) = point(4);
    if [lx, ly, rx, ry, nx, ny, mlx, mly, mrx, mry]
        .iter()
        .any(|value| !value.is_finite())
    {
        return None;
    }

    // Project the nose into a coordinate system defined by the eye line. This
    // prevents head roll from being misread as yaw or pitch.
    let eye_dx = rx - lx;
    let eye_dy = ry - ly;
    let eye_distance = eye_dx.hypot(eye_dy);
    if eye_distance <= f32::EPSILON {
        return None;
    }
    let horizontal = (eye_dx / eye_distance, eye_dy / eye_distance);
    let vertical = (-horizontal.1, horizontal.0);

    let eye_center = ((lx + rx) / 2.0, (ly + ry) / 2.0);
    let mouth_center = ((mlx + mrx) / 2.0, (mly + mry) / 2.0);
    let nose_from_eyes = (nx - eye_center.0, ny - eye_center.1);
    let mouth_from_eyes = (mouth_center.0 - eye_center.0, mouth_center.1 - eye_center.1);
    let mouth_distance = mouth_from_eyes.0 * vertical.0 + mouth_from_eyes.1 * vertical.1;
    if mouth_distance <= f32::EPSILON {
        return None;
    }

    let yaw = (nose_from_eyes.0 * horizontal.0 + nose_from_eyes.1 * horizontal.1) / eye_distance;
    let pitch = (nose_from_eyes.0 * vertical.0 + nose_from_eyes.1 * vertical.1) / mouth_distance;
    (yaw.is_finite() && pitch.is_finite()).then_some((yaw, pitch))
}

#[derive(Default)]
pub struct EnrollmentPoseStability {
    samples: std::collections::VecDeque<(f32, f32)>,
}

impl EnrollmentPoseStability {
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    pub fn update(&mut self, prompt: EnrollPrompt, yaw: f32, pitch: f32) -> bool {
        if !yaw.is_finite() || !pitch.is_finite() {
            self.reset();
            return false;
        }

        self.samples.push_back((yaw, pitch));
        if self.samples.len() > ENROLL_POSE_STABILITY_WINDOW {
            self.samples.pop_front();
        }
        if self.samples.len() < ENROLL_POSE_STABILITY_WINDOW {
            return false;
        }

        let (mut min_yaw, mut max_yaw) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut min_pitch, mut max_pitch) = (f32::INFINITY, f32::NEG_INFINITY);
        for &(sample_yaw, sample_pitch) in &self.samples {
            min_yaw = min_yaw.min(sample_yaw);
            max_yaw = max_yaw.max(sample_yaw);
            min_pitch = min_pitch.min(sample_pitch);
            max_pitch = max_pitch.max(sample_pitch);
        }

        let stable_yaw = max_yaw - min_yaw < ENROLL_STABLE_YAW_RANGE;
        let stable_pitch = max_pitch - min_pitch < ENROLL_STABLE_PITCH_RANGE;
        match prompt {
            EnrollPrompt::LookStraight => stable_yaw && stable_pitch,
            EnrollPrompt::LookUp | EnrollPrompt::LookDown => stable_yaw,
            EnrollPrompt::LookLeft | EnrollPrompt::LookRight => stable_pitch,
            _ => false,
        }
    }
}

pub fn enrollment_pose_matches(
    prompt: EnrollPrompt,
    yaw: f32,
    pitch: f32,
    baseline: Option<(f32, f32)>,
) -> bool {
    if !yaw.is_finite() || !pitch.is_finite() {
        return false;
    }

    match prompt {
        EnrollPrompt::LookStraight => yaw.abs() < 0.18 && (0.2..0.8).contains(&pitch),
        EnrollPrompt::LookUp => {
            baseline.is_some_and(|(_, base_pitch)| pitch < base_pitch - ENROLL_VERTICAL_POSE_DELTA)
        }
        EnrollPrompt::LookDown => {
            baseline.is_some_and(|(_, base_pitch)| pitch > base_pitch + ENROLL_VERTICAL_POSE_DELTA)
        }
        EnrollPrompt::LookLeft => {
            baseline.is_some_and(|(base_yaw, _)| yaw < base_yaw - ENROLL_HORIZONTAL_POSE_DELTA)
        }
        EnrollPrompt::LookRight => {
            baseline.is_some_and(|(base_yaw, _)| yaw > base_yaw + ENROLL_HORIZONTAL_POSE_DELTA)
        }
        _ => false,
    }
}

pub struct FaceChecker {
    pub detector: std::sync::Arc<std::sync::Mutex<FaceDetector>>,
    pub dark_luma_threshold: u8,
    pub rgb_luma_history: std::collections::VecDeque<u8>,
    pub spectrum: Spectrum,
    pub check_centering_and_proximity: bool,
}

impl FaceChecker {
    pub fn new(
        detector: std::sync::Arc<std::sync::Mutex<FaceDetector>>,
        config: &Config,
        spectrum: Spectrum,
        check_centering_and_proximity: bool,
    ) -> Self {
        Self {
            detector,
            dark_luma_threshold: config.cameras.dark_luma_threshold,
            rgb_luma_history: std::collections::VecDeque::new(),
            spectrum,
            check_centering_and_proximity,
        }
    }

    fn build_capture_result(
        frame: &Mat,
        bbox: Option<(f32, f32, f32, f32)>,
        kpss: Option<ndarray::Array3<f32>>,
        mat_rgb: Option<opencv::core::Mat>,
        yaw: f32,
        pitch: f32,
    ) -> anyhow::Result<CaptureResult> {
        let sz = frame.size()?;
        Ok(CaptureResult {
            bytes: frame_to_bytes(frame)?,
            width: sz.width as u32,
            height: sz.height as u32,
            bbox,
            kpss,
            mat_rgb,
            yaw,
            pitch,
        })
    }

    pub fn capture_status(
        &mut self,
        frame: &Mat,
    ) -> anyhow::Result<(CaptureStatus, Option<CaptureResult>)> {
        let detection = {
            let mut detector = lock_recover(&self.detector);
            detector.detect(frame)
        };
        let (bboxes, kps, mat_rgb) = match detection {
            Ok(result) => result,
            Err(DetectError::NoFacesDetected) => return Ok((CaptureStatus::NoFace, None)),
            Err(err) => return Err(err.into()),
        };

        let face = bboxes.row(0);
        let x1 = face[0];
        let y1 = face[1];
        let x2 = face[2];
        let y2 = face[3];

        let (yaw, pitch) = kps
            .as_ref()
            .and_then(estimate_head_pose)
            .unwrap_or((f32::NAN, f32::NAN));

        let status = if let Some(status) = geometry_status(
            (x1, y1, x2, y2),
            frame.cols() as f32,
            frame.rows() as f32,
            self.check_centering_and_proximity,
        ) {
            status
        } else if kps.is_none() {
            return Ok((CaptureStatus::NoFace, None));
        } else {
            if let Spectrum::Rgb = self.spectrum {
                let w = frame.cols() as f32;
                let h = frame.rows() as f32;
                let max_dim = w.max(h);
                let top = (max_dim - h) / 2.0;
                let left = (max_dim - w) / 2.0;

                let x1_unpadded = x1 - left;
                let y1_unpadded = y1 - top;
                let x2_unpadded = x2 - left;
                let y2_unpadded = y2 - top;

                let face_rect =
                    clamp_bbox(frame, (x1_unpadded, y1_unpadded, x2_unpadded, y2_unpadded));
                if let Ok(face_roi) = Mat::roi(frame, face_rect).and_then(|r| r.try_clone()) {
                    let luma = frame_mean_luma(&face_roi).unwrap_or(0);

                    let history = &mut self.rgb_luma_history;
                    history.push_back(luma);
                    if history.len() > 5 {
                        history.pop_front();
                    }
                    let sum_luma: u32 = history.iter().map(|&v| v as u32).sum();
                    let avg_luma = sum_luma as f64 / history.len() as f64;
                    let threshold = self.dark_luma_threshold as f64;

                    let is_current_frame_dark = (luma as f64) < threshold;
                    let is_avg_dark = avg_luma < threshold;

                    tracing::debug!("luma: {} avg_luma: {}", luma, avg_luma);

                    if !is_current_frame_dark {
                        CaptureStatus::Usable
                    } else if is_avg_dark {
                        CaptureStatus::TooDark
                    } else {
                        CaptureStatus::Ready
                    }
                } else {
                    CaptureStatus::TooDark
                }
            } else {
                CaptureStatus::Usable
            }
        };

        Ok((
            status,
            Some(Self::build_capture_result(
                frame,
                Some((x1, y1, x2, y2)),
                kps,
                Some(mat_rgb),
                yaw,
                pitch,
            )?),
        ))
    }
}

pub fn frame_mean_luma(frame: &Mat) -> anyhow::Result<u8> {
    let size = frame.size()?;
    let pixel_count = (size.width.max(0) * size.height.max(0)) as usize;
    if pixel_count == 0 {
        return Ok(0);
    }

    let channels = frame.channels() as usize;
    if channels == 0 {
        return Ok(0);
    }

    let bytes = frame.data_bytes()?;
    let total: u64 = bytes
        .chunks_exact(channels)
        .take(pixel_count)
        .map(|pixel| {
            let luminance = if channels >= 3 {
                let b = pixel[0] as u32;
                let g = pixel[1] as u32;
                let r = pixel[2] as u32;
                (77 * r + 150 * g + 29 * b) >> 8
            } else {
                pixel.iter().map(|&v| v as u32).sum::<u32>() / channels as u32
            };
            luminance as u64
        })
        .sum();

    let mean = total / pixel_count as u64;
    Ok(mean as u8)
}

fn clamp_bbox(frame: &Mat, bbox: (f32, f32, f32, f32)) -> opencv::core::Rect {
    let (x1, y1, x2, y2) = bbox;
    let w = frame.cols();
    let h = frame.rows();
    let xi1 = (x1.max(0.0) as i32).min(w.saturating_sub(1));
    let yi1 = (y1.max(0.0) as i32).min(h.saturating_sub(1));
    let xi2 = (x2.max(0.0) as i32).min(w);
    let yi2 = (y2.max(0.0) as i32).min(h);
    opencv::core::Rect::new(xi1, yi1, (xi2 - xi1).max(0), (yi2 - yi1).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::{self, Scalar};

    #[test]
    fn dark_frame_detection_rejects_black_frames() {
        let frame =
            Mat::new_rows_cols_with_default(12, 12, core::CV_8UC3, Scalar::all(0.0)).unwrap();

        assert!(frame_mean_luma(&frame).unwrap() < 30);
    }

    #[test]
    fn dark_frame_detection_accepts_lit_frames() {
        let frame =
            Mat::new_rows_cols_with_default(12, 12, core::CV_8UC3, Scalar::all(120.0)).unwrap();

        assert!(frame_mean_luma(&frame).unwrap() >= 30);
    }

    #[test]
    fn mean_luminance_threshold_is_an_exclusive_lower_bound() {
        let frame =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::all(50.0)).unwrap();

        assert!(frame_mean_luma(&frame).unwrap() < 51);
        assert!(frame_mean_luma(&frame).unwrap() >= 50);
    }

    #[test]
    fn mean_uses_bt601_weighting() {
        let blue =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::new(255.0, 0.0, 0.0, 0.0))
                .unwrap();
        assert!(frame_mean_luma(&blue).unwrap() < 30);

        let green =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::new(0.0, 255.0, 0.0, 0.0))
                .unwrap();
        assert!(frame_mean_luma(&green).unwrap() >= 30);
    }

    #[test]
    fn single_channel_frames_use_raw_luminance() {
        let dark = Mat::new_rows_cols_with_default(8, 8, core::CV_8UC1, Scalar::all(5.0)).unwrap();
        assert!(frame_mean_luma(&dark).unwrap() < 30);

        let lit = Mat::new_rows_cols_with_default(8, 8, core::CV_8UC1, Scalar::all(120.0)).unwrap();
        assert!(frame_mean_luma(&lit).unwrap() >= 30);
    }

    #[test]
    fn mean_is_robust_to_a_few_bright_pixels() {
        let mut frame =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::all(0.0)).unwrap();
        {
            let mut top = Mat::roi_mut(&mut frame, core::Rect::new(0, 0, 4, 1)).unwrap();
            top.set_to_def(&Scalar::all(255.0)).unwrap();
        }
        assert!(frame_mean_luma(&frame).unwrap() < 30);
    }

    #[test]
    fn empty_frame_is_treated_as_dark() {
        let frame = Mat::default();
        assert!(frame_mean_luma(&frame).unwrap_or(0) < 30);
    }

    #[test]
    fn clipping_uses_landscape_content_bounds_inside_square_padding() {
        assert!(bbox_is_clipped((300.0, 85.0, 380.0, 200.0), 640.0, 480.0));
        assert!(bbox_is_clipped((300.0, 400.0, 380.0, 555.0), 640.0, 480.0));
        assert!(!bbox_is_clipped((300.0, 250.0, 380.0, 380.0), 640.0, 480.0));
    }

    #[test]
    fn clipping_uses_portrait_content_bounds_inside_square_padding() {
        assert!(bbox_is_clipped((85.0, 300.0, 200.0, 380.0), 480.0, 640.0));
        assert!(!bbox_is_clipped((250.0, 300.0, 380.0, 380.0), 480.0, 640.0));
    }

    #[test]
    fn clipping_checks_square_frame_edges_without_padding() {
        assert!(bbox_is_clipped((10.0, 200.0, 300.0, 400.0), 480.0, 480.0));
        assert!(!bbox_is_clipped((100.0, 200.0, 300.0, 400.0), 480.0, 480.0));
    }

    #[test]
    fn geometry_uses_square_detector_axes_for_centering() {
        assert_eq!(
            geometry_status((240.0, 240.0, 400.0, 400.0), 640.0, 480.0, true),
            None
        );
        assert_eq!(
            geometry_status((240.0, 240.0, 400.0, 400.0), 480.0, 640.0, true),
            None
        );
        assert_eq!(
            geometry_status((400.0, 240.0, 560.0, 400.0), 640.0, 480.0, true),
            Some(CaptureStatus::NotCentered)
        );
    }

    #[test]
    fn geometry_keeps_the_lower_face_size_threshold() {
        assert_eq!(
            geometry_status((375.0, 375.0, 625.0, 625.0), 1000.0, 1000.0, true),
            None
        );
        assert_eq!(
            geometry_status((380.0, 380.0, 620.0, 620.0), 1000.0, 1000.0, true),
            Some(CaptureStatus::TooFar)
        );
        assert_eq!(
            geometry_status((105.0, 105.0, 895.0, 895.0), 1000.0, 1000.0, true),
            Some(CaptureStatus::TooClose)
        );
    }

    #[test]
    fn authentication_skips_centering_and_proximity_but_still_rejects_clipping() {
        assert_eq!(
            geometry_status((400.0, 240.0, 560.0, 400.0), 640.0, 480.0, false),
            None
        );
        assert_eq!(
            geometry_status((300.0, 85.0, 380.0, 200.0), 640.0, 480.0, false),
            Some(CaptureStatus::Clipped)
        );
    }

    #[test]
    fn poisoned_detector_style_lock_recovers_the_inner_value() {
        let value = std::sync::Arc::new(Mutex::new(1_u8));
        let poison = value.clone();
        let _ = std::thread::spawn(move || {
            let mut guard = poison.lock().unwrap();
            *guard = 2;
            panic!("poison lock");
        })
        .join();

        assert_eq!(*lock_recover(&value), 2);
    }

    fn landmarks(points: [(f32, f32); 5]) -> ndarray::Array3<f32> {
        ndarray::Array3::from_shape_fn((1, 5, 2), |(_, point, axis)| {
            if axis == 0 {
                points[point].0
            } else {
                points[point].1
            }
        })
    }

    #[test]
    fn head_pose_is_invariant_to_roll() {
        let level_points = [
            (10.0, 10.0),
            (30.0, 10.0),
            (22.0, 20.0),
            (14.0, 30.0),
            (26.0, 30.0),
        ];
        let level = landmarks(level_points);
        let angle = std::f32::consts::FRAC_PI_4;
        let (sin, cos) = angle.sin_cos();
        let rolled = landmarks(level_points.map(|(x, y)| {
            let (dx, dy) = (x - 20.0, y - 10.0);
            (20.0 + dx * cos - dy * sin, 10.0 + dx * sin + dy * cos)
        }));

        let level_pose = estimate_head_pose(&level).unwrap();
        let rolled_pose = estimate_head_pose(&rolled).unwrap();
        assert!((level_pose.0 - rolled_pose.0).abs() < 0.001);
        assert!((level_pose.1 - rolled_pose.1).abs() < 0.001);
    }

    #[test]
    fn head_pose_rejects_degenerate_landmarks() {
        let coincident_eyes = landmarks([
            (10.0, 10.0),
            (10.0, 10.0),
            (10.0, 20.0),
            (5.0, 30.0),
            (15.0, 30.0),
        ]);
        assert!(estimate_head_pose(&coincident_eyes).is_none());

        let malformed = ndarray::Array3::zeros((1, 4, 2));
        assert!(estimate_head_pose(&malformed).is_none());
    }

    #[test]
    fn enrollment_pose_stability_accepts_a_held_pose() {
        let mut stability = EnrollmentPoseStability::default();
        assert!(!stability.update(EnrollPrompt::LookStraight, -0.50, 0.48));
        assert!(stability.update(EnrollPrompt::LookStraight, -0.54, 0.49));
    }

    #[test]
    fn enrollment_pose_stability_rejects_motion_and_resets() {
        let mut stability = EnrollmentPoseStability::default();
        for (yaw, pitch) in [(-0.10, 0.51), (-0.20, 0.50), (-0.30, 0.49)] {
            assert!(!stability.update(EnrollPrompt::LookStraight, yaw, pitch));
        }
        assert!(!stability.update(EnrollPrompt::LookStraight, f32::NAN, 0.49));
        assert!(!stability.update(EnrollPrompt::LookStraight, -0.50, 0.48));
        assert!(stability.update(EnrollPrompt::LookStraight, -0.52, 0.49));
    }

    #[test]
    fn enrollment_pose_stability_ignores_motion_on_the_prompted_axis() {
        let mut stability = EnrollmentPoseStability::default();
        assert!(!stability.update(EnrollPrompt::LookRight, 0.10, 0.50));
        assert!(stability.update(EnrollPrompt::LookRight, 0.30, 0.52));

        stability.reset();
        assert!(!stability.update(EnrollPrompt::LookUp, 0.02, 0.50));
        assert!(stability.update(EnrollPrompt::LookUp, 0.04, 0.30));

        stability.reset();
        assert!(!stability.update(EnrollPrompt::LookRight, 0.10, 0.40));
        assert!(!stability.update(EnrollPrompt::LookRight, 0.30, 0.50));

        stability.reset();
        assert!(!stability.update(EnrollPrompt::LookUp, -0.10, 0.50));
        assert!(!stability.update(EnrollPrompt::LookUp, 0.10, 0.30));
    }

    #[test]
    fn enrollment_pose_directions_are_relative_to_straight() {
        let baseline = Some((0.08, 0.62));
        assert!(enrollment_pose_matches(
            EnrollPrompt::LookLeft,
            -0.03,
            0.62,
            baseline
        ));
        assert!(enrollment_pose_matches(
            EnrollPrompt::LookRight,
            0.19,
            0.62,
            baseline
        ));
        assert!(enrollment_pose_matches(
            EnrollPrompt::LookUp,
            0.08,
            0.57,
            baseline
        ));
        assert!(enrollment_pose_matches(
            EnrollPrompt::LookDown,
            0.08,
            0.67,
            baseline
        ));
        assert!(!enrollment_pose_matches(
            EnrollPrompt::LookLeft,
            0.01,
            0.62,
            baseline
        ));
        assert!(!enrollment_pose_matches(
            EnrollPrompt::LookRight,
            0.15,
            0.62,
            baseline
        ));
    }

    #[test]
    fn enrollment_straight_pose_rejects_invalid_geometry() {
        assert!(enrollment_pose_matches(
            EnrollPrompt::LookStraight,
            0.1,
            0.3,
            None
        ));
        assert!(!enrollment_pose_matches(
            EnrollPrompt::LookStraight,
            f32::NAN,
            0.5,
            None
        ));
        assert!(!enrollment_pose_matches(
            EnrollPrompt::LookStraight,
            0.0,
            0.9,
            None
        ));
    }
}
