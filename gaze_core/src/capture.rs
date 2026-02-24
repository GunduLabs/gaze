use crate::camera::Camera;
use crate::centering::FaceChecker;
use opencv::prelude::*;
use std::thread;
use std::time::Duration;

pub struct CaptureResult {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn frame_to_bytes(frame: &opencv::core::Mat) -> anyhow::Result<CaptureResult> {
    let sz = frame.size()?;
    let total = (sz.width * sz.height * 3) as usize;
    let mut bytes = vec![0u8; total];
    unsafe {
        std::ptr::copy_nonoverlapping(frame.data(), bytes.as_mut_ptr(), total);
    }
    Ok(CaptureResult {
        bytes,
        width: sz.width as u32,
        height: sz.height as u32,
    })
}

pub enum CaptureStatus {
    NoFace,
    NotCentered,
    Ready(CaptureResult),
}

pub fn try_capture(cam: &mut Camera, checker: &mut FaceChecker) -> anyhow::Result<CaptureStatus> {
    let frame = cam.capture_frame()?;
    let status = checker.check(&frame)?;

    if !status.detected {
        return Ok(CaptureStatus::NoFace);
    }
    if !status.centered {
        return Ok(CaptureStatus::NotCentered);
    }

    Ok(CaptureStatus::Ready(frame_to_bytes(&frame)?))
}

pub fn wait_for_centered_capture(
    cam: &mut Camera,
    checker: &mut FaceChecker,
    mut on_status: impl FnMut(&CaptureStatus),
) -> anyhow::Result<CaptureResult> {
    loop {
        let status = try_capture(cam, checker)?;
        match status {
            CaptureStatus::Ready(result) => return Ok(result),
            ref s => {
                on_status(s);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
