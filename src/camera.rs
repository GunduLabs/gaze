use opencv::core::Mat;
use opencv::prelude::*;
use opencv::videoio::{CAP_ANY, VideoCapture};

pub struct Camera {
    cap: VideoCapture,
}

impl Camera {
    pub fn new(device_idx: i32) -> anyhow::Result<Self> {
        let cap = VideoCapture::new(device_idx, CAP_ANY)?;
        if !cap.is_opened()? {
            anyhow::bail!("Failed to open camera device {}", device_idx);
        }
        Ok(Self { cap })
    }

    pub fn capture_frame(&mut self) -> anyhow::Result<Mat> {
        let mut frame = Mat::default();
        self.cap.read(&mut frame)?;
        if frame.empty() {
            anyhow::bail!("Captured an empty frame from camera");
        }
        Ok(frame)
    }
}
