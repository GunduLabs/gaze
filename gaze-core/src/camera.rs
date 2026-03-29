use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use opencv::videoio::{CAP_GSTREAMER, VideoCapture};
use tracing::info;

pub struct Camera {
    cap: VideoCapture,
}

impl Camera {
    pub fn open(device_target: &str) -> anyhow::Result<Self> {
        let p = if device_target.is_empty() {
            "pipewiresrc ! videoconvert ! appsink".to_string()
        } else {
            format!(
                "pipewiresrc target-object={} ! videoconvert ! appsink",
                device_target
            )
        };
        info!("Attempting to open pipewire GStreamer camera: {}", p);

        let cap = VideoCapture::from_file(&p, CAP_GSTREAMER)?;

        if !cap.is_opened()? {
            anyhow::bail!("Failed to open camera at {}", device_target);
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

    let mut cameras = Vec::new();
    for device in devices {
        let display_name = device.display_name().to_string();
        if let Some(props) = device.properties() {
            let target = if let Ok(name) = props.get::<String>("node.name") {
                name
            } else if let Ok(serial) = props.get::<u64>("object.serial") {
                serial.to_string()
            } else if let Ok(api) = props.get::<String>("api.v4l2.path") {
                api
            } else if let Ok(api) = props.get::<String>("object.path") {
                api
            } else {
                continue;
            };
            if !cameras.iter().any(|(_, t)| t == &target) {
                cameras.push((display_name, target));
            }
        }
    }

    Ok(cameras)
}
