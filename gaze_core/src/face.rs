use crate::config::Config;
use crate::detect::{DetectError, FaceDetector};
use opencv::core::Mat;
use opencv::prelude::*;
use std::path::Path;

pub struct CaptureResult {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bbox: Option<(f32, f32, f32, f32)>,
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

pub enum CaptureStatus {
    NoFace,
    NotCentered(CaptureResult),
    Clipped(CaptureResult),
    Ready(CaptureResult),
}

impl std::fmt::Display for CaptureStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            CaptureStatus::NoFace => "No face detected. Look at the camera.",
            CaptureStatus::NotCentered(_) => "Face detected. Center your face.",
            CaptureStatus::Clipped(_) => "Face is clipped. Move fully into frame.",
            CaptureStatus::Ready(_) => "Face ready.",
        };
        write!(f, "{}", text)
    }
}

pub struct FaceChecker {
    detector: FaceDetector,
}

impl FaceChecker {
    pub fn new() -> anyhow::Result<Self> {
        let config = Config::load().unwrap_or_default();
        let model_path = Path::new(&config.storage.models_dir).join(config.security.detector());

        if !model_path.exists() {
            anyhow::bail!(
                "Model not found at {}. Please install the gaze daemon first.",
                model_path.display()
            );
        }

        let detector = FaceDetector::new(model_path.to_str().unwrap())?;
        Ok(Self { detector })
    }

    pub fn from_detector(detector: FaceDetector) -> Self {
        Self { detector }
    }

    fn build_capture_result(
        frame: &Mat,
        bbox: Option<(f32, f32, f32, f32)>,
    ) -> anyhow::Result<CaptureResult> {
        let sz = frame.size()?;
        Ok(CaptureResult {
            bytes: frame_to_bytes(frame)?,
            width: sz.width as u32,
            height: sz.height as u32,
            bbox,
        })
    }

    pub fn capture_status(&mut self, frame: &Mat) -> anyhow::Result<CaptureStatus> {
        let (bboxes, kpss, _) = match self.detector.detect(frame) {
            Ok(result) => result,
            Err(DetectError::NoFacesDetected) => return Ok(CaptureStatus::NoFace),
            Err(err) => return Err(err.into()),
        };

        if bboxes.nrows() == 0 {
            return Ok(CaptureStatus::NoFace);
        }

        let face = bboxes.row(0);
        let x1 = face[0];
        let y1 = face[1];
        let x2 = face[2];
        let y2 = face[3];

        let frame_w = frame.cols() as f32;
        let frame_h = frame.rows() as f32;
        let max_dim = frame_w.max(frame_h);
        let edge_margin = 0.05;

        if x1 / max_dim < edge_margin
            || y1 / max_dim < edge_margin
            || x2 / max_dim > (1.0 - edge_margin)
            || y2 / max_dim > (1.0 - edge_margin)
        {
            return Ok(CaptureStatus::Clipped(Self::build_capture_result(
                frame,
                Some((x1, y1, x2, y2)),
            )?));
        }

        if kpss.is_none() {
            return Ok(CaptureStatus::NoFace);
        }

        let width = x2 - x1;
        let height = y2 - y1;
        let cx = x1 + width / 2.0;
        let cy = y1 + height / 2.0;

        let norm_cx = cx / max_dim;
        let norm_cy = cy / max_dim;

        if (norm_cx - 0.5).abs() >= 0.2 || (norm_cy - 0.5).abs() >= 0.2 {
            return Ok(CaptureStatus::NotCentered(Self::build_capture_result(
                frame,
                Some((x1, y1, x2, y2)),
            )?));
        }

        Ok(CaptureStatus::Ready(Self::build_capture_result(
            frame,
            Some((x1, y1, x2, y2)),
        )?))
    }
}
