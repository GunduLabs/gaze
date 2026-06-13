use crate::camera::Camera;
use crate::config::Config;
use crate::face::FaceChecker;
use std::thread;

use crate::dbus::CaptureStatus;
pub use crate::face::CaptureResult;

pub fn init_camera_and_checker(
    device: &str,
    config: &Config,
) -> anyhow::Result<(Camera, FaceChecker)> {
    let config = config.clone();
    let checker_thread = thread::spawn(move || FaceChecker::new(&config));
    let cam = Camera::open(device);

    let checker = checker_thread
        .join()
        .map_err(|_| anyhow::anyhow!("FaceChecker init thread panicked"))??;
    let cam = cam?;

    Ok((cam, checker))
}

pub fn try_capture(
    cam: &mut Camera,
    checker: &mut FaceChecker,
) -> anyhow::Result<(CaptureStatus, Option<CaptureResult>)> {
    let frame = cam
        .next()
        .ok_or_else(|| anyhow::anyhow!("Camera stopped"))?;
    checker.capture_status(&frame, true)
}

pub fn wait_for_capture(
    cam: &mut Camera,
    checker: &mut FaceChecker,
    centering_required: bool,
    mut on_status: impl FnMut(&(CaptureStatus, Option<CaptureResult>)),
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
    mut on_status: impl FnMut(&(CaptureStatus, Option<CaptureResult>)),
    mut should_abort: impl FnMut() -> bool,
) -> anyhow::Result<Option<CaptureResult>> {
    for frame in cam {
        if should_abort() {
            return Ok(None);
        }
        let (status, result) = checker.capture_status(&frame, centering_required)?;
        match (status, result) {
            (CaptureStatus::Usable, Some(result)) => return Ok(Some(result)),
            (CaptureStatus::NotCentered, Some(result)) if !centering_required => {
                return Ok(Some(result));
            }
            (s, r) => on_status(&(s, r)),
        }
    }
    anyhow::bail!("Camera stopped delivering frames")
}
