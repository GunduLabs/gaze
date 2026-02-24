use crate::camera::Camera;
use crate::face::FaceChecker;
use std::thread;
use std::time::Duration;

pub use crate::face::{CaptureResult, CaptureStatus, ValidatedFace, frame_to_bytes};

pub fn try_capture(
    cam: &mut Camera,
    checker: &mut FaceChecker,
    require_centering: bool,
) -> anyhow::Result<CaptureStatus> {
    let frame = cam.capture_frame()?;
    checker.check(&frame, require_centering)
}

pub fn wait_for_capture(
    cam: &mut Camera,
    checker: &mut FaceChecker,
    require_centering: bool,
    mut on_status: impl FnMut(&CaptureStatus),
) -> anyhow::Result<CaptureResult> {
    loop {
        let frame = cam.capture_frame()?;
        let status = checker.check(&frame, require_centering)?;

        match status {
            CaptureStatus::Ready(result) => return Ok(result),
            other => on_status(&other),
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub fn wait_for_centered_capture(
    cam: &mut Camera,
    checker: &mut FaceChecker,
    on_status: impl FnMut(&CaptureStatus),
) -> anyhow::Result<CaptureResult> {
    wait_for_capture(cam, checker, true, on_status)
}
