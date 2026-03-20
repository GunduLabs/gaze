use crate::camera::Camera;
use crate::face::FaceChecker;
use std::thread;
use std::time::Duration;

pub use crate::face::{CaptureResult, CaptureStatus, frame_to_bytes};

pub fn init_camera_and_checker(device: &str) -> anyhow::Result<(Camera, FaceChecker)> {
    let checker_thread = thread::spawn(FaceChecker::new);
    let cam = Camera::open(device);

    let checker = checker_thread
        .join()
        .map_err(|_| anyhow::anyhow!("FaceChecker init thread panicked"))??;
    let cam = cam?;

    Ok((cam, checker))
}

pub fn try_capture(cam: &mut Camera, checker: &mut FaceChecker) -> anyhow::Result<CaptureStatus> {
    let frame = cam.capture_frame()?;
    checker.capture_status(&frame)
}

pub fn wait_for_capture(
    cam: &mut Camera,
    checker: &mut FaceChecker,
    centering_required: bool,
    mut on_status: impl FnMut(&CaptureStatus),
) -> anyhow::Result<CaptureResult> {
    let result = wait_for_capture_until(
        cam,
        checker,
        centering_required,
        |status| on_status(status),
        || false,
    )?;

    result.ok_or_else(|| anyhow::anyhow!("Capture interrupted"))
}

pub fn wait_for_capture_until(
    cam: &mut Camera,
    checker: &mut FaceChecker,
    centering_required: bool,
    mut on_status: impl FnMut(&CaptureStatus),
    mut should_abort: impl FnMut() -> bool,
) -> anyhow::Result<Option<CaptureResult>> {
    loop {
        if should_abort() {
            return Ok(None);
        }

        let frame = cam.capture_frame()?;
        let status = checker.capture_status(&frame)?;

        match status {
            CaptureStatus::Ready(result) => return Ok(Some(result)),
            CaptureStatus::NotCentered(result) if !centering_required => return Ok(Some(result)),
            other => on_status(&other),
        }
        thread::sleep(Duration::from_millis(100));
    }
}
