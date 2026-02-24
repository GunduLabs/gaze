#![allow(dead_code, unused_imports)]

mod align;
mod camera;
mod daemon;
mod detect;
mod recognize;
mod users;

use daemon::AuthDaemon;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::ConnectionBuilder;

const THRESHOLD: f32 = 0.4;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Initializing Gaze Facial Authentication Daemon...");
    let t_load = std::time::Instant::now();

    let detector = detect::FaceDetector::new("/opt/gaze/models/det_500m.onnx")
        .or_else(|_| detect::FaceDetector::new("models/det_500m.onnx"))
        .expect("Failed to load detection model");

    let recognizer = recognize::FaceRecognizer::new("/opt/gaze/models/w600k_mbf.onnx")
        .or_else(|_| recognize::FaceRecognizer::new("models/w600k_mbf.onnx"))
        .expect("Failed to load recognition model");

    let db = users::UserDatabase::new()?;
    let cam = camera::Camera::new(0)?;

    let daemon = AuthDaemon {
        detector: Arc::new(Mutex::new(detector)),
        recognizer: Arc::new(Mutex::new(recognizer)),
        db: Arc::new(Mutex::new(db)),
        camera: Arc::new(Mutex::new(cam)),
    };

    println!("Models, Camera & User DB loaded in: {:?}", t_load.elapsed());

    let _conn = ConnectionBuilder::system()?
        .name("org.gaze.Auth")?
        .serve_at("/org/gaze/Auth", daemon)?
        .build()
        .await?;

    println!("Gaze Daemon initialized and listening on System Bus...");

    std::future::pending::<()>().await;

    Ok(())
}
