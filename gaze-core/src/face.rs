use crate::config::{Config, MODELS_DIR};
use crate::dbus::CaptureStatus;
use crate::detect::{DetectError, FaceDetector};
use opencv::core::Mat;
use opencv::prelude::*;
use std::path::Path;

const MIN_FACE_SIZE_RATIO: f32 = 0.35;
const MAX_FACE_SIZE_RATIO: f32 = 0.78;

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

pub fn frame_to_bytes(frame: &Mat) -> anyhow::Result<Vec<u8>> {
    let sz = frame.size()?;
    let total = (sz.width * sz.height * 3) as usize;
    let mut bytes = vec![0u8; total];
    unsafe {
        std::ptr::copy_nonoverlapping(frame.data(), bytes.as_mut_ptr(), total);
    }
    Ok(bytes)
}

pub struct FaceChecker {
    detector: FaceDetector,
    dark_luma_threshold: u8,
}

impl FaceChecker {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let model_path = Path::new(MODELS_DIR).join(config.security.detector());

        if !model_path.exists() {
            anyhow::bail!(
                "Model not found at {}. Run 'gazed' once to download models, or install the gaze package.",
                model_path.display()
            );
        }

        let detector = FaceDetector::new(model_path.to_str().unwrap())?;
        Ok(Self::from_detector_with_config(detector, config))
    }

    pub fn from_detector(detector: FaceDetector) -> Self {
        Self {
            detector,
            dark_luma_threshold: 70,
        }
    }

    pub fn from_detector_with_config(detector: FaceDetector, config: &Config) -> Self {
        Self {
            detector,
            dark_luma_threshold: config.cameras.dark_luma_threshold,
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
        if is_dark_frame(frame, self.dark_luma_threshold)? {
            return Ok((CaptureStatus::TooDark, None));
        }

        let (bboxes, kps, mat_rgb) = match self.detector.detect(frame) {
            Ok(result) => result,
            Err(DetectError::NoFacesDetected) => return Ok((CaptureStatus::NoFace, None)),
            Err(err) => return Err(err.into()),
        };

        let face = bboxes.row(0);
        let x1 = face[0];
        let y1 = face[1];
        let x2 = face[2];
        let y2 = face[3];

        let max_dim = (frame.cols() as f32).max(frame.rows() as f32);
        let min_dim = (frame.cols() as f32).min(frame.rows() as f32);
        let edge_margin = 0.05;
        let (width, height) = (x2 - x1, y2 - y1);
        let (cx, cy) = (x1 + width / 2.0, y1 + height / 2.0);
        let (norm_cx, norm_cy) = (cx / max_dim, cy / max_dim);
        let face_size_ratio = width.max(height) / min_dim;

        let mut yaw = 0.0;
        let mut pitch = 0.0;

        if let Some(lm) = &kps {
            let lx = lm[[0, 0, 0]];
            let ly = lm[[0, 0, 1]];
            let rx = lm[[0, 1, 0]];
            let ry = lm[[0, 1, 1]];
            let nx = lm[[0, 2, 0]];
            let ny = lm[[0, 2, 1]];
            let mly = lm[[0, 3, 1]];
            let mry = lm[[0, 4, 1]];

            let eye_w = rx - lx;
            let eye_center_x = (lx + rx) / 2.0;
            yaw = (nx - eye_center_x) / eye_w;

            let eye_y = (ly + ry) / 2.0;
            let mouth_y = (mly + mry) / 2.0;
            let face_h = mouth_y - eye_y;
            pitch = (ny - eye_y) / face_h;
        }

        let status = if x1 / max_dim < edge_margin
            || y1 / max_dim < edge_margin
            || x2 / max_dim > (1.0 - edge_margin)
            || y2 / max_dim > (1.0 - edge_margin)
        {
            CaptureStatus::Clipped
        } else if (norm_cx - 0.5).abs() >= 0.2 || (norm_cy - 0.5).abs() >= 0.2 {
            CaptureStatus::NotCentered
        } else if face_size_ratio < MIN_FACE_SIZE_RATIO {
            CaptureStatus::TooFar
        } else if face_size_ratio > MAX_FACE_SIZE_RATIO {
            CaptureStatus::TooClose
        } else if kps.is_none() {
            return Ok((CaptureStatus::NoFace, None));
        } else {
            CaptureStatus::Ready
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

/// Returns true when the frame's mean luminance is below `dark_luma_threshold`,
/// i.e. the scene is too dark to reliably detect or recognize a face.
///
/// A per-pixel count of near-black pixels does not work on real webcams: their
/// auto-gain lifts a dark scene to a noisy mid-grey (mean luma ~50) that never
/// reaches true black, while a few light-leak pixels skew any ratio. The mean
/// is robust and separates "covered/dark" from "lit" cleanly.
pub fn is_dark_frame(frame: &Mat, dark_luma_threshold: u8) -> anyhow::Result<bool> {
    let size = frame.size()?;
    let pixel_count = (size.width.max(0) * size.height.max(0)) as usize;
    if pixel_count == 0 {
        return Ok(true);
    }

    let channels = frame.channels() as usize;
    if channels == 0 {
        return Ok(true);
    }

    let bytes = frame.data_bytes()?;
    let total: u64 = bytes
        .chunks_exact(channels)
        .take(pixel_count)
        .map(|pixel| {
            let luminance = if channels >= 3 {
                // OpenCV gives us BGR, not RGB. Weights are BT.601 (0.299/0.587/0.114) scaled
                // by 256 so the divide becomes a right shift.
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
    Ok(mean < dark_luma_threshold as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::{self, Scalar};

    #[test]
    fn dark_frame_detection_rejects_black_frames() {
        let frame =
            Mat::new_rows_cols_with_default(12, 12, core::CV_8UC3, Scalar::all(0.0)).unwrap();

        assert!(is_dark_frame(&frame, 70).unwrap());
    }

    #[test]
    fn dark_frame_detection_accepts_lit_frames() {
        let frame =
            Mat::new_rows_cols_with_default(12, 12, core::CV_8UC3, Scalar::all(120.0)).unwrap();

        assert!(!is_dark_frame(&frame, 70).unwrap());
    }

    #[test]
    fn mean_luminance_threshold_is_an_exclusive_lower_bound() {
        // A frame whose mean luma equals the threshold is *not* dark (strict `<`).
        let frame =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::all(50.0)).unwrap();

        assert!(is_dark_frame(&frame, 51).unwrap());
        assert!(!is_dark_frame(&frame, 50).unwrap());
    }

    #[test]
    fn mean_uses_bt601_weighting() {
        // Pure blue is visually dim: its BT.601 luminance is ~28 even though the raw
        // byte average is 85, so the frame reads as dark below ~30.
        // Scalar is ordered (B, G, R, A) to match OpenCV's BGR layout.
        let blue =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::new(255.0, 0.0, 0.0, 0.0))
                .unwrap();
        assert!(is_dark_frame(&blue, 30).unwrap());

        // Pure green carries most of the luminance weight (~149) and is not dark.
        let green =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::new(0.0, 255.0, 0.0, 0.0))
                .unwrap();
        assert!(!is_dark_frame(&green, 30).unwrap());
    }

    #[test]
    fn single_channel_frames_use_raw_luminance() {
        // Grayscale frames take the non-BGR averaging branch.
        let dark = Mat::new_rows_cols_with_default(8, 8, core::CV_8UC1, Scalar::all(5.0)).unwrap();
        assert!(is_dark_frame(&dark, 70).unwrap());

        let lit = Mat::new_rows_cols_with_default(8, 8, core::CV_8UC1, Scalar::all(120.0)).unwrap();
        assert!(!is_dark_frame(&lit, 70).unwrap());
    }

    #[test]
    fn mean_is_robust_to_a_few_bright_pixels() {
        // A mostly-black frame with one bright row stays well below the threshold,
        // unlike a pixel-count ratio which a bright spot could tip either way.
        let mut frame =
            Mat::new_rows_cols_with_default(8, 8, core::CV_8UC3, Scalar::all(0.0)).unwrap();
        {
            let mut top = Mat::roi_mut(&mut frame, core::Rect::new(0, 0, 8, 1)).unwrap();
            top.set_to_def(&Scalar::all(255.0)).unwrap();
        }
        // One of eight rows bright => mean luma ~32, still dark at threshold 70.
        assert!(is_dark_frame(&frame, 70).unwrap());
    }

    #[test]
    fn empty_frame_is_treated_as_dark() {
        let frame = Mat::default();
        assert!(is_dark_frame(&frame, 70).unwrap());
    }
}
