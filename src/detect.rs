use opencv::core::Mat;
use opencv::prelude::*;
use ort::{session::Session, session::builder::GraphOptimizationLevel};

pub struct FaceDetector {
    detector: rusty_scrfd::SCRFD,
}

impl FaceDetector {
    pub fn new(model_path: &str) -> anyhow::Result<Self> {
        let det_session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)?;

        let detector = rusty_scrfd::SCRFD::new(det_session, (320, 320), 0.1, 0.4, false)
            .expect("Failed to init detector");

        Ok(Self { detector })
    }

    // Fixes stupid bug in rusty_scrfd
    pub fn pad_to_square(img: &Mat) -> Mat {
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
        )
        .expect("Failed to pad image");
        padded
    }

    pub fn detect(
        &mut self,
        img: &Mat,
    ) -> anyhow::Result<(ndarray::Array2<f32>, Option<ndarray::Array3<f32>>, Mat)> {
        let mat_square = Self::pad_to_square(img);
        let mut mat_rgb = Mat::default();
        opencv::imgproc::cvt_color(
            &mat_square,
            &mut mat_rgb,
            opencv::imgproc::COLOR_BGR2RGB,
            0,
            opencv::core::AlgorithmHint::ALGO_HINT_DEFAULT,
        )
        .expect("Failed color conversion");

        let mut center_cache = std::collections::HashMap::new();
        let (bboxes, kpss) = self
            .detector
            .detect(&mat_rgb, 1, "max", &mut center_cache)
            .expect("Detect failed");

        Ok((bboxes, kpss, mat_rgb))
    }
}
