use ndarray::Array1;
use opencv::core::{CV_8UC3, Mat};
use opencv::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;
use zbus::{fdo, interface};

use crate::align::align_face;
use crate::recognize::FaceRecognizer;
use crate::users::{UserDatabase, UserDbError};
use gaze_core::detect::{DetectError, FaceDetector};

pub struct AuthDaemon {
    pub detector: Arc<Mutex<FaceDetector>>,
    pub recognizer: Arc<Mutex<FaceRecognizer>>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub threshold: f32,
    pub max_captures: usize,
}

impl AuthDaemon {
    fn map_user_db_error(err: UserDbError) -> fdo::Error {
        match err {
            UserDbError::UserNotFound(msg) => fdo::Error::FileNotFound(msg),
            UserDbError::FaceNotFound(msg) => fdo::Error::FileNotFound(msg),
            UserDbError::FaceExists(msg) => fdo::Error::FileExists(msg),
            UserDbError::Io(io_err) => fdo::Error::Failed(io_err.to_string()),
        }
    }

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
    ) -> Result<Vec<Array1<f32>>, fdo::Error> {
        let (bboxes, kpss, mat_rgb) = match detector.detect(frame) {
            Ok(result) => result,
            Err(DetectError::NoFacesDetected) => {
                return Err(fdo::Error::Failed("No faces detected".into()));
            }
            Err(err) => return Err(fdo::Error::Failed(format!("Detection failed: {err}"))),
        };

        let Some(kpss) = kpss else {
            return Err(fdo::Error::Failed("No keypoints detected".into()));
        };

        let face_count = bboxes.nrows().min(kpss.shape()[0]);
        let mut embeddings = Vec::with_capacity(face_count);

        for face_index in 0..face_count {
            let aligned = align_face(&mat_rgb, &kpss, face_index)
                .map_err(|e| fdo::Error::Failed(format!("Alignment failed: {e}")))?;

            let embedding = recognizer
                .get_embedding(&aligned)
                .map_err(|e| fdo::Error::Failed(format!("Recognition failed: {e}")))?;
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    async fn get_embeddings_from_frame(
        &self,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> fdo::Result<Vec<Array1<f32>>> {
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
        info!(username = %username, width, height, "Verify request");
        let embeds = self
            .get_embeddings_from_frame(&image_data, width, height)
            .await?;

        let db = self.db.lock().await;
        let mut result = false;
        for embed in &embeds {
            if db
                .verify_user(&username, embed, self.threshold)
                .map_err(Self::map_user_db_error)?
            {
                result = true;
                break;
            }
        }
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
        info!(username = %username, width, height, "Match faces request");
        let embeds = self
            .get_embeddings_from_frame(&image_data, width, height)
            .await?;

        let db = self.db.lock().await;
        let mut combined: HashMap<String, (f32, f32, bool, u32)> = HashMap::new();

        for embed in &embeds {
            let per_face = db
                .match_faces(&username, embed, self.threshold)
                .map_err(Self::map_user_db_error)?;

            for (name, score, pct, passed, count) in per_face {
                let entry = combined.entry(name).or_insert((score, pct, passed, count));
                if score > entry.0 {
                    *entry = (score, pct, passed, count);
                }
            }
        }

        let mut results: Vec<(String, f64, f64, bool, u32)> = combined
            .into_iter()
            .map(|(name, (score, pct, passed, count))| {
                (name, score as f64, pct as f64, passed, count)
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
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
        info!(username = %username, face_name = %face_name, "Add face request");
        if face_name.trim().is_empty() {
            return Err(fdo::Error::InvalidArgs("Face name cannot be empty".into()));
        }
        let embeds = self
            .get_embeddings_from_frame(&image_data, width, height)
            .await?;
        let Some(embed) = embeds.first() else {
            return Err(fdo::Error::Failed("No faces detected".into()));
        };

        let mut db = self.db.lock().await;
        let result = db
            .add_face(&username, &face_name, embed, self.max_captures)
            .map_err(Self::map_user_db_error)?;
        info!(username = %username, face_name = %face_name, "Face added");
        Ok(result)
    }

    async fn remove_face(&self, username: String, face_name: String) -> fdo::Result<bool> {
        info!(username = %username, face_name = %face_name, "Remove face request");
        let mut db = self.db.lock().await;
        db.remove_face(&username, &face_name)
            .map_err(Self::map_user_db_error)?;
        Ok(true)
    }

    async fn rename_face(
        &self,
        username: String,
        old_face_name: String,
        new_face_name: String,
    ) -> fdo::Result<bool> {
        info!(
            username = %username,
            old_face_name = %old_face_name,
            new_face_name = %new_face_name,
            "Rename face request"
        );
        let mut db = self.db.lock().await;
        db.rename_face(&username, &old_face_name, &new_face_name)
            .map_err(Self::map_user_db_error)?;

        Ok(true)
    }

    async fn list_faces(&self, username: String) -> fdo::Result<Vec<(String, u32)>> {
        info!(username = %username, "List faces request");
        let db = self.db.lock().await;
        let faces = db.list_faces(&username).map_err(Self::map_user_db_error)?;
        Ok(faces)
    }

    async fn clear_user(&self, username: String) -> fdo::Result<bool> {
        info!(username = %username, "Clear user request");
        let mut db = self.db.lock().await;
        db.clear_user(&username).map_err(Self::map_user_db_error)?;
        Ok(true)
    }
}
