use crate::camera::frame_to_bytes;
use crate::config::Config;
use crate::dbus::CaptureStatus;
use crate::detect::{DetectError, FaceDetector};
use opencv::core::Mat;
use opencv::prelude::*;

const MIN_FACE_SIZE_RATIO: f32 = 0.25;
const MAX_FACE_SIZE_RATIO: f32 = 0.78;

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
            let mut detector = self.detector.lock().unwrap_or_else(|e| e.into_inner());
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
        } else if self.check_centering_and_proximity
            && ((norm_cx - 0.5).abs() >= 0.2 || (norm_cy - 0.5).abs() >= 0.2)
        {
            CaptureStatus::NotCentered
        } else if self.check_centering_and_proximity && face_size_ratio < MIN_FACE_SIZE_RATIO {
            CaptureStatus::TooFar
        } else if self.check_centering_and_proximity && face_size_ratio > MAX_FACE_SIZE_RATIO {
            CaptureStatus::TooClose
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

                    tracing::info!("luma: {} avg_luma: {}", luma, avg_luma);

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
}
