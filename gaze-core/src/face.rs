use crate::camera::frame_to_bytes;
use crate::config::{Config, MODELS_DIR};
use crate::dbus::CaptureStatus;
use crate::detect::{DetectError, FaceDetector};
use opencv::core::Mat;
use opencv::prelude::*;
use std::path::Path;

const MIN_FACE_SIZE_RATIO: f32 = 0.35;
const MAX_FACE_SIZE_RATIO: f32 = 0.78;

/// On an IR camera the emitter-lit face sits on a near-black background, which
/// drags the frame mean down. Cap the darkness cutoff here so an IR frame is not
/// wrongly rejected, while still catching a covered or unlit sensor (mean ~0).
const IR_DARK_LUMA_CEILING: u8 = 25;

/// The dark-frame cutoff to use, relaxed for IR cameras (never raised).
pub fn effective_dark_luma_threshold(config: &Config) -> u8 {
    if config.cameras.ir.trim().is_empty() {
        config.cameras.dark_luma_threshold
    } else {
        config.cameras.dark_luma_threshold.min(IR_DARK_LUMA_CEILING)
    }
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
            dark_luma_threshold: effective_dark_luma_threshold(config),
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

        let frame_w = frame.cols() as f32;
        let frame_h = frame.rows() as f32;
        let max_dim = frame_w.max(frame_h);
        let min_dim = frame_w.min(frame_h);
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

        let status = if bbox_is_clipped((x1, y1, x2, y2), frame_w, frame_h) {
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

/// True when the detected face comes within 5% of the original frame's edges.
///
/// Detection runs on the square-padded frame (`pad_to_square`), so the bbox is
/// offset by the padding. The check must measure against the original frame's
/// content rect inside that square: against the square's own edges, a face cut
/// off at the top or bottom of a landscape frame would never read as clipped
/// because the content boundary sits well inside the padded margin.
fn bbox_is_clipped(bbox: (f32, f32, f32, f32), frame_w: f32, frame_h: f32) -> bool {
    const EDGE_MARGIN: f32 = 0.05;
    let max_dim = frame_w.max(frame_h);
    // Same integer-truncation split as pad_to_square.
    let pad_x = ((max_dim - frame_w) / 2.0).floor();
    let pad_y = ((max_dim - frame_h) / 2.0).floor();
    let (x1, y1, x2, y2) = bbox;

    x1 - pad_x < EDGE_MARGIN * frame_w
        || y1 - pad_y < EDGE_MARGIN * frame_h
        || x2 - pad_x > (1.0 - EDGE_MARGIN) * frame_w
        || y2 - pad_y > (1.0 - EDGE_MARGIN) * frame_h
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

    #[test]
    fn clipping_measures_against_content_bounds_not_padded_square() {
        // 640x480 landscape frame padded to 640x640: content spans y = 80..560
        // in padded coordinates, so the top margin sits at y = 80 + 24.

        // Face touching the top of the visible frame is clipped even though it
        // is far from the padded square's edge (the old check missed this).
        assert!(bbox_is_clipped((300.0, 85.0, 380.0, 200.0), 640.0, 480.0));
        // Face touching the bottom of the visible frame.
        assert!(bbox_is_clipped((300.0, 400.0, 380.0, 555.0), 640.0, 480.0));
        // Well inside the content rect on every side.
        assert!(!bbox_is_clipped((300.0, 250.0, 380.0, 380.0), 640.0, 480.0));
        // Left/right margins are unchanged from the old behaviour.
        assert!(bbox_is_clipped((20.0, 250.0, 380.0, 380.0), 640.0, 480.0));
        assert!(bbox_is_clipped((300.0, 250.0, 620.0, 380.0), 640.0, 480.0));

        // Square frames have no padding; both axes measure against the edges.
        assert!(bbox_is_clipped((10.0, 200.0, 300.0, 400.0), 480.0, 480.0));
        assert!(!bbox_is_clipped((100.0, 200.0, 300.0, 400.0), 480.0, 480.0));

        // Portrait frame: padding is on the x axis instead.
        assert!(bbox_is_clipped((85.0, 300.0, 200.0, 380.0), 480.0, 640.0));
        assert!(!bbox_is_clipped((250.0, 300.0, 380.0, 380.0), 480.0, 640.0));
    }

    #[test]
    fn ir_camera_relaxes_dark_threshold_without_raising_it() {
        let mut config = crate::config::Config::default();
        config.cameras.dark_luma_threshold = 70;

        assert_eq!(effective_dark_luma_threshold(&config), 70);

        config.cameras.ir = "/dev/video2".to_string();
        assert_eq!(effective_dark_luma_threshold(&config), 25);

        // a user-chosen lower value is never raised
        config.cameras.dark_luma_threshold = 15;
        assert_eq!(effective_dark_luma_threshold(&config), 15);
    }
}
