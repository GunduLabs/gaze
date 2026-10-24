// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use futures::StreamExt;
use gaze_vision::orientation::{DeviceOrientation, OrientationHandle};
use std::time::Duration;
use tracing::{debug, info};

const SENSOR_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

#[zbus::proxy(
    interface = "net.hadess.SensorProxy",
    default_service = "net.hadess.SensorProxy",
    default_path = "/net/hadess/SensorProxy"
)]
trait Sensors {
    async fn claim_accelerometer(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn has_accelerometer(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn accelerometer_orientation(&self) -> zbus::Result<String>;
}

pub struct OrientationWatch {
    handle: OrientationHandle,
    task: tokio::task::JoinHandle<()>,
}

impl OrientationWatch {
    pub async fn start(enabled: bool) -> Option<Self> {
        if !enabled {
            return None;
        }
        let proxy = match tokio::time::timeout(SENSOR_CONNECT_TIMEOUT, claim()).await {
            Ok(Ok(Some(proxy))) => proxy,
            Ok(Ok(None)) => {
                debug!("No accelerometer; camera frames stay as captured");
                return None;
            }
            Ok(Err(err)) => {
                debug!("Orientation sensor unavailable: {err}");
                return None;
            }
            Err(_) => {
                debug!("Orientation sensor did not answer in time");
                return None;
            }
        };

        let handle = OrientationHandle::default();
        let watched = handle.clone();
        let task = tokio::spawn(async move {
            let mut changes = proxy.receive_accelerometer_orientation_changed().await;
            if let Ok(value) = proxy.accelerometer_orientation().await {
                let orientation = DeviceOrientation::from_sensor(&value);
                info!(?orientation, "Device orientation");
                watched.set(orientation);
            }
            while let Some(change) = changes.next().await {
                if let Ok(value) = change.get().await {
                    let orientation = DeviceOrientation::from_sensor(&value);
                    debug!(?orientation, "Device orientation changed");
                    watched.set(orientation);
                }
            }
        });
        Some(Self { handle, task })
    }

    pub fn handle(&self) -> OrientationHandle {
        self.handle.clone()
    }
}

impl Drop for OrientationWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn claim() -> zbus::Result<Option<SensorsProxy<'static>>> {
    let connection = zbus::Connection::system().await?;
    let proxy = SensorsProxy::new(&connection).await?;
    if !proxy.has_accelerometer().await? {
        return Ok(None);
    }
    proxy.claim_accelerometer().await?;
    Ok(Some(proxy))
}
