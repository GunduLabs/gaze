use opencv::imgcodecs::{IMREAD_COLOR, imread};
use opencv::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::{fdo, interface};

use crate::THRESHOLD;
use crate::align::{align_face, mat_to_rgb};
use crate::detect::FaceDetector;
use crate::recognize::FaceRecognizer;
use crate::users::UserDatabase;

pub struct AuthDaemon {
    pub detector: Arc<Mutex<FaceDetector>>,
    pub recognizer: Arc<Mutex<FaceRecognizer>>,
    pub db: Arc<Mutex<UserDatabase>>,
}

#[interface(name = "org.gaze.Auth")]
impl AuthDaemon {
    async fn authenticate(&self, username: String, image_path: String) -> fdo::Result<bool> {
        let img_mat_bgr = imread(&image_path, IMREAD_COLOR)
            .map_err(|e| fdo::Error::Failed(format!("Failed to read image: {}", e)))?;

        if img_mat_bgr.empty() {
            return Err(fdo::Error::Failed(
                "OpenCV returned an empty matrix".to_string(),
            ));
        }

        let mut det = self.detector.lock().await;
        let (_bboxes, kpss, mat_rgb) = det
            .detect(&img_mat_bgr)
            .map_err(|e| fdo::Error::Failed(format!("Detection failed: {}", e)))?;

        let kps = kpss.ok_or_else(|| fdo::Error::Failed("No face found in image".to_string()))?;
        let aligned = align_face(&mat_rgb, &kps)
            .map_err(|e| fdo::Error::Failed(format!("Alignment failed: {}", e)))?;

        let mut rec = self.recognizer.lock().await;
        let embed = rec
            .get_embedding(&aligned)
            .map_err(|e| fdo::Error::Failed(format!("Recognition failed: {}", e)))?;

        let db = self.db.lock().await;
        let user_embeds_opt = db.get_user_embeddings(&username);

        if let Some(user_embeds) = user_embeds_opt {
            for ref_embed in user_embeds {
                let sim = embed.dot(ref_embed);
                if sim > THRESHOLD {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    async fn add_face(
        &self,
        username: String,
        face_name: String,
        image_path: String,
    ) -> fdo::Result<String> {
        let img_mat_bgr = imread(&image_path, IMREAD_COLOR)
            .map_err(|e| fdo::Error::Failed(format!("Failed to read image: {}", e)))?;

        if img_mat_bgr.empty() {
            return Err(fdo::Error::Failed(
                "OpenCV returned an empty matrix".to_string(),
            ));
        }

        let mut det = self.detector.lock().await;
        let (_bboxes, kpss, mat_rgb) = det
            .detect(&img_mat_bgr)
            .map_err(|e| fdo::Error::Failed(format!("Detection failed: {}", e)))?;

        let kps = kpss.ok_or_else(|| fdo::Error::Failed("No face found in image".to_string()))?;
        let aligned = align_face(&mat_rgb, &kps)
            .map_err(|e| fdo::Error::Failed(format!("Alignment failed: {}", e)))?;

        let mut rec = self.recognizer.lock().await;
        let embed = rec
            .get_embedding(&aligned)
            .map_err(|e| fdo::Error::Failed(format!("Recognition failed: {}", e)))?;

        let mut db = self.db.lock().await;
        let uuid = db
            .add_face(&username, &face_name, &embed)
            .map_err(|e| fdo::Error::Failed(format!("Failed to save face: {}", e)))?;

        Ok(uuid)
    }

    async fn remove_face(&self, username: String, face_name: String) -> fdo::Result<bool> {
        let mut db = self.db.lock().await;
        let removed = db
            .remove_face(&username, &face_name)
            .map_err(|e| fdo::Error::Failed(format!("Failed to remove face: {}", e)))?;
        Ok(removed)
    }

    async fn clear_user(&self, username: String) -> fdo::Result<bool> {
        let mut db = self.db.lock().await;
        let cleared = db
            .clear_user(&username)
            .map_err(|e| fdo::Error::Failed(format!("Failed to clear user: {}", e)))?;
        Ok(cleared)
    }
}
