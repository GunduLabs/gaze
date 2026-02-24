use opencv::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::{fdo, interface};

use crate::THRESHOLD;
use crate::align::align_face;
use crate::camera::Camera;
use crate::detect::FaceDetector;
use crate::recognize::FaceRecognizer;
use crate::users::UserDatabase;

pub struct AuthDaemon {
    pub detector: Arc<Mutex<FaceDetector>>,
    pub recognizer: Arc<Mutex<FaceRecognizer>>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub camera: Arc<Mutex<Camera>>,
}

impl AuthDaemon {
    fn process_frame(
        detector: &mut FaceDetector,
        recognizer: &mut FaceRecognizer,
        frame: &opencv::core::Mat,
    ) -> Result<ndarray::Array1<f32>, fdo::Error> {
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
    async fn authenticate(&self, username: String) -> fdo::Result<bool> {
        let frame = {
            let mut cam = self.camera.lock().await;
            cam.capture_frame()
                .map_err(|e| fdo::Error::Failed(format!("Camera capture failed: {}", e)))?
        };

        let embed = {
            let mut det = self.detector.lock().await;
            let mut rec = self.recognizer.lock().await;
            Self::process_frame(&mut det, &mut rec, &frame)?
        };

        let db = self.db.lock().await;
        if let Some(user_embeds) = db.get_user_embeddings(&username) {
            for ref_embed in user_embeds {
                if embed.dot(ref_embed) > THRESHOLD {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    async fn add_face(&self, username: String, face_name: String) -> fdo::Result<String> {
        let frame = {
            let mut cam = self.camera.lock().await;
            cam.capture_frame()
                .map_err(|e| fdo::Error::Failed(format!("Camera capture failed: {}", e)))?
        };

        let embed = {
            let mut det = self.detector.lock().await;
            let mut rec = self.recognizer.lock().await;
            Self::process_frame(&mut det, &mut rec, &frame)?
        };

        let mut db = self.db.lock().await;
        let uuid = db
            .add_face(&username, &face_name, &embed)
            .map_err(|e| fdo::Error::Failed(format!("Failed to save face: {}", e)))?;

        Ok(uuid)
    }

    async fn remove_face(&self, username: String, face_name: String) -> fdo::Result<bool> {
        let mut db = self.db.lock().await;
        db.remove_face(&username, &face_name)
            .map_err(|e| fdo::Error::Failed(format!("Failed to remove face: {}", e)))
    }

    async fn clear_user(&self, username: String) -> fdo::Result<bool> {
        let mut db = self.db.lock().await;
        db.clear_user(&username)
            .map_err(|e| fdo::Error::Failed(format!("Failed to clear user: {}", e)))
    }
}
