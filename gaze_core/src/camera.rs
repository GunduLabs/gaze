use opencv::core::Mat;
use opencv::prelude::*;
use opencv::videoio::{CAP_V4L2, VideoCapture};

pub struct Camera {
    cap: VideoCapture,
}

impl Camera {
    pub fn open(device_path: &str) -> anyhow::Result<Self> {
        let saved_stderr = unsafe { libc::dup(2) };
        let devnull = unsafe { libc::open(c"/dev/null".as_ptr() as _, libc::O_WRONLY) };
        if saved_stderr >= 0 && devnull >= 0 {
            unsafe { libc::dup2(devnull, 2) };
        }

        let cap = VideoCapture::from_file(device_path, CAP_V4L2);

        if saved_stderr >= 0 {
            unsafe { libc::dup2(saved_stderr, 2) };
            unsafe { libc::close(saved_stderr) };
        }
        if devnull >= 0 {
            unsafe { libc::close(devnull) };
        }

        let cap = cap?;
        if !cap.is_opened()? {
            anyhow::bail!("Failed to open camera at {}", device_path);
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
