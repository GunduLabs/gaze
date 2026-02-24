use crate::config::Config;
use crate::detect::FaceDetector;
use opencv::core::Mat;
use opencv::prelude::*;
use std::path::Path;

pub struct FaceChecker {
    detector: FaceDetector,
}

pub struct FaceStatus {
    pub detected: bool,
    pub centered: bool,
    pub bbox: Option<(i32, i32, i32, i32)>,
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

    pub fn check(&mut self, frame: &Mat) -> anyhow::Result<FaceStatus> {
        let (bboxes, _, _) = self.detector.detect(frame)?;

        if bboxes.nrows() == 0 {
            return Ok(FaceStatus {
                detected: false,
                centered: false,
                bbox: None,
            });
        }

        let face = bboxes.row(0);
        let x1 = face[0];
        let y1 = face[1];
        let x2 = face[2];
        let y2 = face[3];

        let width = x2 - x1;
        let height = y2 - y1;
        let cx = x1 + width / 2.0;
        let cy = y1 + height / 2.0;

        let frame_w = frame.cols() as f32;
        let frame_h = frame.rows() as f32;
        let max_dim = frame_w.max(frame_h);

        let norm_cx = cx / max_dim;
        let norm_cy = cy / max_dim;

        let centered = (norm_cx - 0.5).abs() < 0.2 && (norm_cy - 0.5).abs() < 0.2;

        Ok(FaceStatus {
            detected: true,
            centered,
            bbox: Some((x1 as i32, y1 as i32, width as i32, height as i32)),
        })
    }
}
