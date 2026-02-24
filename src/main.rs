#![allow(dead_code, unused_imports, unused_variables)]
use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use nalgebra::{ArrayStorage, Matrix, Matrix3, U2, U3};
use ndarray::azip;
use ndarray::{Array, Array1, Array3, Array4, Axis, s};
use opencv::core::Mat;
use opencv::imgcodecs::{IMREAD_COLOR, imread};
use opencv::prelude::*;
use ort::{session::Session, session::builder::GraphOptimizationLevel, value::TensorRef};
use std::io::{Read, Write};
use std::path::Path;

mod align;
mod daemon;
mod detect;
mod recognize;
mod users;

use daemon::AuthDaemon;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::ConnectionBuilder;

const THRESHOLD: f32 = 0.4;

fn pre_process(img: &RgbImage) -> Array4<f32> {
    let (width, height) = img.dimensions();
    let mut tensor = Array4::<f32>::zeros((1, 3, height as usize, width as usize));

    for (x, y, pixel) in img.enumerate_pixels() {
        let r = (pixel[0] as f32 - 127.5) / 127.5;
        let g = (pixel[1] as f32 - 127.5) / 127.5;
        let b = (pixel[2] as f32 - 127.5) / 127.5;

        tensor[[0, 0, y as usize, x as usize]] = b;
        tensor[[0, 1, y as usize, x as usize]] = g;
        tensor[[0, 2, y as usize, x as usize]] = r;
    }
    tensor
}

fn get_embedding(session: &mut Session, img: &RgbImage) -> Array1<f32> {
    let tensor = pre_process(img);
    let inputs = ort::inputs![TensorRef::from_array_view(&tensor).unwrap()];
    let outputs = session.run(inputs).unwrap();

    let (_shape, data) = outputs[0].try_extract_tensor::<f32>().unwrap();
    let row = Array1::from_vec(data.to_vec());

    let norm = row.dot(&row).sqrt();
    row / norm
}

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

    let daemon = AuthDaemon {
        detector: Arc::new(Mutex::new(detector)),
        recognizer: Arc::new(Mutex::new(recognizer)),
        db: Arc::new(Mutex::new(db)),
    };

    println!("Models & User DB loaded in: {:?}", t_load.elapsed());

    let _conn = ConnectionBuilder::system()?
        .name("org.gaze.Auth")?
        .serve_at("/org/gaze/Auth", daemon)?
        .build()
        .await?;

    println!("Gaze Daemon initialized and listening on System Bus...");

    std::future::pending::<()>().await;

    Ok(())
}
