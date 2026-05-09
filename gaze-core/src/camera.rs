use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use opencv::videoio::{CAP_GSTREAMER, VideoCapture};
use tracing::info;

use crate::config::DEFAULT_RGB_CAMERA;

const PRIMARY_CAMERA_DISPLAY_NAME: &str = "Primary Camera";

pub struct Camera {
    cap: VideoCapture,
}

impl Camera {
    pub fn open(camera_source: &str) -> anyhow::Result<Self> {
        let source = camera_source.trim();
        let p = if source.is_empty() {
            anyhow::bail!("camera source cannot be empty; use \"primary\" or a GStreamer source");
        } else if source == DEFAULT_RGB_CAMERA {
            "autovideosrc ! videoconvert ! appsink".to_string()
        } else if source.starts_with("/dev/video") {
            anyhow::bail!(
                "direct /dev/video* camera paths are no longer supported; use \"primary\" or a GStreamer source"
            );
        } else {
            format!("{} ! videoconvert ! appsink", source)
        };
        info!("Attempting to open GStreamer camera: {}", p);

        let cap = VideoCapture::from_file(&p, CAP_GSTREAMER)?;

        if !cap.is_opened()? {
            anyhow::bail!("Failed to open camera source {}", camera_source);
        }
        Ok(Self { cap })
    }

    pub fn capture_frame(&mut self) -> anyhow::Result<Mat> {
        let mut frame = Mat::default();
        self.cap.read(&mut frame)?;
        if frame.empty() {
            anyhow::bail!("Captured an empty frame from camera");
        }
        let mut mirrored = Mat::default();
        opencv::core::flip(&frame, &mut mirrored, 1)?;
        Ok(mirrored)
    }
}

pub fn enumerate_cameras() -> anyhow::Result<Vec<(String, String)>> {
    gstreamer::init()?;
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    monitor.start()?;
    let devices = monitor.devices();
    monitor.stop();

    let mut cameras = vec![(
        PRIMARY_CAMERA_DISPLAY_NAME.to_string(),
        DEFAULT_RGB_CAMERA.to_string(),
    )];
    for device in devices {
        let display_name = device.display_name().to_string();
        if let Some(props) = device.properties() {
            let target = if let Ok(name) = props.get::<String>("node.name") {
                name
            } else if let Ok(serial) = props.get::<u64>("object.serial") {
                serial.to_string()
            } else if let Ok(api) = props.get::<String>("object.path") {
                api
            } else {
                continue;
            };
            let target = format!("pipewiresrc target-object={}", target);
            if !cameras.iter().any(|(_, t)| t == &target) {
                cameras.push((display_name, target));
            }
        }
    }

    Ok(cameras)
}
