use opencv::core::Mat;
use opencv::prelude::*;
use ort::{session::Session, session::builder::GraphOptimizationLevel};
use std::fmt;

#[derive(Debug)]
pub enum DetectError {
    InitFailed(String),
    ImageProcessing(opencv::Error),
    Io(std::io::Error),
    OrtSession(ort::Error),
    NoFacesDetected,
    InferenceFailed(String),
}

impl fmt::Display for DetectError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InitFailed(msg) => write!(fmt, "detector init failed: {msg}"),
            Self::ImageProcessing(err) => write!(fmt, "image processing: {err}"),
            Self::Io(err) => write!(fmt, "IO: {err}"),
            Self::OrtSession(err) => write!(fmt, "ORT session: {err}"),
            Self::NoFacesDetected => write!(fmt, "no faces detected"),
            Self::InferenceFailed(msg) => write!(fmt, "inference failed: {msg}"),
        }
    }
}

impl std::error::Error for DetectError {}

impl From<opencv::Error> for DetectError {
    fn from(err: opencv::Error) -> Self {
        Self::ImageProcessing(err)
    }
}

impl From<std::io::Error> for DetectError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<ort::Error> for DetectError {
    fn from(err: ort::Error) -> Self {
        Self::OrtSession(err)
    }
}

pub type DetectResult = (ndarray::Array2<f32>, Option<ndarray::Array3<f32>>, Mat);

pub struct FaceDetector {
    detector: rusty_scrfd::SCRFD,
}

impl FaceDetector {
    pub fn new(model_path: &str) -> Result<Self, DetectError> {
        let det_session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)?;

        let detector = rusty_scrfd::SCRFD::new(det_session, (320, 320), 0.1, 0.4, false)
            .map_err(|err| DetectError::InitFailed(err.to_string()))?;

        Ok(Self { detector })
    }

    pub fn pad_to_square(img: &Mat) -> Result<Mat, DetectError> {
        use opencv::core;
        let width = img.cols();
        let height = img.rows();
        let max_dim = width.max(height);
        let mut padded = Mat::default();

        let top = (max_dim - height) / 2;
        let bottom = max_dim - height - top;
        let left = (max_dim - width) / 2;
        let right = max_dim - width - left;

        opencv::core::copy_make_border(
            img,
            &mut padded,
            top,
            bottom,
            left,
            right,
            opencv::core::BORDER_CONSTANT,
            core::Scalar::all(0.0),
        )?;
        Ok(padded)
    }

    pub fn detect(&mut self, img: &Mat) -> Result<DetectResult, DetectError> {
        let mat_square = Self::pad_to_square(img)?;
        let mut mat_rgb = Mat::default();
        opencv::imgproc::cvt_color_def(&mat_square, &mut mat_rgb, opencv::imgproc::COLOR_BGR2RGB)?;

        let mut center_cache = std::collections::HashMap::new();

        // rusty_scrfd unconditionally prints raw tensor data to stdout
        use std::os::unix::io::AsRawFd;
        let devnull = std::fs::File::open("/dev/null")?;
        let stdout_fd = std::io::stdout().as_raw_fd();
        let saved_fd = unsafe { libc::dup(stdout_fd) };
        unsafe { libc::dup2(devnull.as_raw_fd(), stdout_fd) };

        let result = self
            .detector
            .detect(&mat_rgb, 1, "max", &mut center_cache)
            .map_err(|err| {
                let msg = err.to_string();
                if msg.contains("No faces detected") {
                    DetectError::NoFacesDetected
                } else {
                    DetectError::InferenceFailed(msg)
                }
            });

        unsafe { libc::dup2(saved_fd, stdout_fd) };
        unsafe { libc::close(saved_fd) };

        let (bboxes, kpss) = result?;
        Ok((bboxes, kpss, mat_rgb))
    }
}
