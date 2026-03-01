use ndarray::Array1;
use opencv::core::{CV_8UC3, Mat};
use opencv::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use zbus::{fdo, interface};

use crate::align::align_face;
use crate::recognize::FaceRecognizer;
use crate::users::UserDatabase;
use gaze_core::detect::{DetectError, FaceDetector};

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
        .map_err(|e| fdo::Error::Failed(format!("Failed to reconstruct frame: {e}")))
    }

    fn process_frame(
        detector: &mut FaceDetector,
        recognizer: &mut FaceRecognizer,
        frame: &Mat,
    ) -> Result<Array1<f32>, fdo::Error> {
        let (bboxes, kpss, mat_rgb) = match detector.detect(frame) {
            Ok(result) => result,
            Err(DetectError::NoFacesDetected) => {
                return Err(fdo::Error::Failed("No faces detected".into()));
            }
            Err(err) => return Err(fdo::Error::Failed(format!("Detection failed: {err}"))),
        };

        if bboxes.nrows() == 0 {
            return Err(fdo::Error::Failed("No faces detected".into()));
        }

        let Some(kpss) = kpss else {
            return Err(fdo::Error::Failed("No keypoints detected".into()));
        };

        let aligned = align_face(&mat_rgb, &kpss)
            .map_err(|e| fdo::Error::Failed(format!("Alignment failed: {e}")))?;

        recognizer
            .get_embedding(&aligned)
            .map_err(|e| fdo::Error::Failed(format!("Recognition failed: {e}")))
    }
    async fn get_embedding_from_frame(
        &self,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> fdo::Result<Array1<f32>> {
        let frame = Self::bytes_to_mat(image_data, width, height)?;
        let mut detector = self.detector.lock().await;
        let mut rec = self.recognizer.lock().await;
        Self::process_frame(&mut detector, &mut rec, &frame)
    }
}

#[interface(name = "org.gaze.Auth")]
impl AuthDaemon {
    async fn verify(
        &self,
        username: String,
        image_data: Vec<u8>,
        width: u32,
        height: u32,
    ) -> fdo::Result<bool> {
        debug!(username = %username, width, height, "Verify request");
        let embed = self
            .get_embedding_from_frame(&image_data, width, height)
            .await?;

        let db = self.db.lock().await;
        let result = db.verify(&username, &embed, self.threshold);
        info!(username = %username, passed = result, "Verify result");
        Ok(result)
    }

    async fn match_faces(
        &self,
        username: String,
        image_data: Vec<u8>,
        width: u32,
        height: u32,
    ) -> fdo::Result<Vec<(String, f64, f64, bool, u32)>> {
        let embed = self
            .get_embedding_from_frame(&image_data, width, height)
            .await?;

        let db = self.db.lock().await;
        let results = db
            .score_all(&username, &embed, self.threshold)
            .into_iter()
            .map(|(name, score, pct, passed, count)| {
                (name, score as f64, pct as f64, passed, count)
            })
            .collect();
        Ok(results)
    }

    async fn add_face(
        &self,
        username: String,
        face_name: String,
        image_data: Vec<u8>,
        width: u32,
        height: u32,
    ) -> fdo::Result<String> {
        debug!(username = %username, face_name = %face_name, "Add face request");
        let embed = self
            .get_embedding_from_frame(&image_data, width, height)
            .await?;

        let mut db = self.db.lock().await;
        let result = db
            .add_face(&username, &face_name, &embed, self.max_captures)
            .map_err(|e| fdo::Error::Failed(format!("Failed to save face: {e}")))?;
        info!(username = %username, face_name = %face_name, "Face added");
        Ok(result)
    }

    async fn remove_face(&self, username: String, face_name: String) -> fdo::Result<bool> {
        info!(username = %username, face_name = %face_name, "Remove face request");
        let mut db = self.db.lock().await;
        db.remove_face(&username, &face_name)
            .map_err(|e| fdo::Error::Failed(format!("Failed to remove face: {e}")))
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
        warn!(username = %username, "Clear user request");
        let mut db = self.db.lock().await;
        db.clear_user(&username)
            .map_err(|e| fdo::Error::Failed(format!("Failed to clear user: {e}")))
    }
}
