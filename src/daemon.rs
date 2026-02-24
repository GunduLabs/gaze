use ndarray::Array1;
use opencv::core::{CV_8UC3, Mat};
use opencv::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::{fdo, interface};

use crate::align::align_face;
use crate::recognize::FaceRecognizer;
use crate::users::UserDatabase;
use gaze_common::detect::FaceDetector;

pub struct AuthDaemon {
    pub detector: Arc<Mutex<FaceDetector>>,
    pub recognizer: Arc<Mutex<FaceRecognizer>>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub threshold: f32,
    pub max_captures: usize,
}

impl AuthDaemon {
    fn bytes_to_mat(data: &[u8], width: u32, height: u32) -> Result<Mat, fdo::Error> {
        let expected = (width * height * 3) as usize;
        if data.len() != expected {
            return Err(fdo::Error::Failed(format!(
                "Expected {} bytes ({}x{}x3), got {}",
                expected,
                width,
                height,
                data.len()
            )));
        }
        unsafe {
            Mat::new_rows_cols_with_data_unsafe_def(
                height as i32,
                width as i32,
                CV_8UC3,
                data.as_ptr() as *mut std::ffi::c_void,
            )
        }
        .map_err(|e| fdo::Error::Failed(format!("Failed to reconstruct frame: {}", e)))
    }

    fn process_frame(
        detector: &mut FaceDetector,
        recognizer: &mut FaceRecognizer,
        frame: &Mat,
    ) -> Result<Array1<f32>, fdo::Error> {
        let (_bboxes, kpss, mat_rgb) = detector
            .detect(frame)
            .map_err(|e| fdo::Error::Failed(format!("Detection failed: {}", e)))?;

        let kps = kpss.ok_or_else(|| fdo::Error::Failed("No face found".to_string()))?;
        let aligned = align_face(&mat_rgb, &kps)
            .map_err(|e| fdo::Error::Failed(format!("Alignment failed: {}", e)))?;

        recognizer
            .get_embedding(&aligned)
            .map_err(|e| fdo::Error::Failed(format!("Recognition failed: {}", e)))
    }
}

#[interface(name = "org.gaze.Auth")]
impl AuthDaemon {
    async fn authenticate(
        &self,
        username: String,
        image_data: Vec<u8>,
        width: u32,
        height: u32,
    ) -> fdo::Result<String> {
        let frame = Self::bytes_to_mat(&image_data, width, height)?;

        let embed = {
            let mut det = self.detector.lock().await;
            let mut rec = self.recognizer.lock().await;
            Self::process_frame(&mut det, &mut rec, &frame)?
        };

        let db = self.db.lock().await;
        Ok(db
            .find_match(&username, &embed, self.threshold)
            .unwrap_or_default())
    }

    async fn add_face(
        &self,
        username: String,
        face_name: String,
        image_data: Vec<u8>,
        width: u32,
        height: u32,
    ) -> fdo::Result<String> {
        let frame = Self::bytes_to_mat(&image_data, width, height)?;

        let embed = {
            let mut det = self.detector.lock().await;
            let mut rec = self.recognizer.lock().await;
            Self::process_frame(&mut det, &mut rec, &frame)?
        };

        let mut db = self.db.lock().await;
        db.add_face(&username, &face_name, &embed, self.max_captures)
            .map_err(|e| fdo::Error::Failed(format!("Failed to save face: {}", e)))
    }

    async fn remove_face(&self, username: String, face_name: String) -> fdo::Result<bool> {
        let mut db = self.db.lock().await;
        db.remove_face(&username, &face_name)
            .map_err(|e| fdo::Error::Failed(format!("Failed to remove face: {}", e)))
    }

    async fn list_faces(&self, username: String) -> fdo::Result<Vec<(String, u32)>> {
        let db = self.db.lock().await;
        let faces = db
            .users
            .get(&username)
            .map(|face_map| {
                face_map
                    .iter()
                    .map(|(name, embeds)| (name.clone(), embeds.len() as u32))
                    .collect()
            })
            .unwrap_or_default();
        Ok(faces)
    }

    async fn clear_user(&self, username: String) -> fdo::Result<bool> {
        let mut db = self.db.lock().await;
        db.clear_user(&username)
            .map_err(|e| fdo::Error::Failed(format!("Failed to clear user: {}", e)))
    }
}
