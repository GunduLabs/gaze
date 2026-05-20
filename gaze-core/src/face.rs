use crate::config::{Config, MODELS_DIR};
use crate::dbus::CaptureStatus;
use crate::detect::{DetectError, FaceDetector};
use crate::ir_quality;
use opencv::core::Mat;
use opencv::prelude::*;
use std::path::Path;
use tracing::{debug, warn};

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
    ir_mode: bool,
}

impl FaceChecker {
    pub fn new() -> anyhow::Result<Self> {
        let config = Config::load().unwrap_or_default();
        let model_path = Path::new(MODELS_DIR).join(config.security.detector());

        if !model_path.exists() {
            anyhow::bail!(
                "Model not found at {}. Run 'gazed' once to download models, or install the gaze package.",
                model_path.display()
            );
        }

        let detector = FaceDetector::new(model_path.to_str().unwrap())?;
        Ok(Self {
            detector,
            ir_mode: false,
        })
    }

    pub fn from_detector(detector: FaceDetector) -> Self {
        Self {
            detector,
            ir_mode: false,
        }
    }

    pub fn set_ir_mode(&mut self, on: bool) {
        self.ir_mode = on;
    }

    pub fn ir_mode(&self) -> bool {
        self.ir_mode
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
        let ir_gray = if self.ir_mode {
            let gray = ir_quality::to_grayscale(frame)?;
            if ir_quality::is_dark_frame(&gray)? {
                debug!("dropping dark IR frame");
                return Ok((CaptureStatus::NoFace, None));
            }
            Some(gray)
        } else {
            None
        };

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
        } else if let Some(gray) = ir_gray.as_ref() {
            if !ir_quality::has_live_texture(gray, (x1, y1, x2, y2))? {
                warn!(bbox = ?(x1, y1, x2, y2), "IR face patch failed texture check");
                return Ok((CaptureStatus::NoFace, None));
            }
            CaptureStatus::Ready
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
