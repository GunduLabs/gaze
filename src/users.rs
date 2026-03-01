use ndarray::Array1;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

type FaceMap = HashMap<String, HashMap<String, Array1<f32>>>;

#[derive(Debug)]
pub enum UserDbError {
    UserNotFound(String),
    FaceNotFound(String),
    FaceExists(String),
    Io(std::io::Error),
}

impl std::fmt::Display for UserDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserDbError::UserNotFound(username) => write!(f, "User '{}' not found", username),
            UserDbError::FaceNotFound(face_name) => write!(f, "Face '{}' not found", face_name),
            UserDbError::FaceExists(face_name) => write!(f, "Face '{}' already exists", face_name),
            UserDbError::Io(err) => write!(f, "{}", err),
        }
    }
}

impl std::error::Error for UserDbError {}

impl From<std::io::Error> for UserDbError {
    fn from(value: std::io::Error) -> Self {
        UserDbError::Io(value)
    }
}

pub struct UserDatabase {
    base_dir: PathBuf,
    pub users: HashMap<String, FaceMap>,
}

impl UserDatabase {
    pub fn new(base_dir: &str) -> anyhow::Result<Self> {
        let mut db = Self {
            base_dir: PathBuf::from(base_dir),
            users: HashMap::new(),
        };
        db.load_all()?;
        Ok(db)
    }

    fn init_dirs(&self) -> std::io::Result<()> {
        if !self.base_dir.exists() {
            fs::create_dir_all(&self.base_dir)?;
        }
        Ok(())
    }

    fn read_embedding(path: &Path) -> anyhow::Result<Array1<f32>> {
        let bytes = fs::read(path)?;
        let float_count = bytes.len() / std::mem::size_of::<f32>();
        let mut embed_vec = vec![0.0f32; float_count];
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                embed_vec.as_mut_ptr() as *mut u8,
                bytes.len(),
            );
        }
        Ok(Array1::from_vec(embed_vec))
    }

    fn write_embedding(path: &Path, embed: &Array1<f32>) -> anyhow::Result<()> {
        let embed_slice = embed.as_slice().expect("Failed to get embedding slice");
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                embed_slice.as_ptr() as *const u8,
                std::mem::size_of_val(embed_slice),
            )
        };
        fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load_all(&mut self) -> anyhow::Result<()> {
        self.init_dirs()?;
        self.users.clear();

        for user_entry in fs::read_dir(&self.base_dir)? {
            let user_entry = user_entry?;
            let user_path = user_entry.path();
            if !user_path.is_dir() {
                continue;
            }
            let username = user_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let mut faces: FaceMap = HashMap::new();

            for face_entry in fs::read_dir(&user_path)? {
                let face_entry = face_entry?;
                let face_path = face_entry.path();
                if !face_path.is_dir() {
                    continue;
                }
                let face_name = face_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let mut embeddings = HashMap::new();

                for bin_entry in fs::read_dir(&face_path)? {
                    let bin_entry = bin_entry?;
                    let bin_path = bin_entry.path();
                    if bin_path.extension().and_then(|e| e.to_str()) == Some("bin")
                        && let Ok(embed) = Self::read_embedding(&bin_path)
                    {
                        let uuid = bin_path.file_stem().unwrap().to_string_lossy().into_owned();
                        embeddings.insert(uuid, embed);
                    }
                }
                faces.insert(face_name, embeddings);
            }
            self.users.insert(username, faces);
        }
        Ok(())
    }

    pub fn add_face(
        &mut self,
        username: &str,
        face_name: &str,
        embed: &Array1<f32>,
        max_captures: usize,
    ) -> Result<String, UserDbError> {
        self.init_dirs()?;
        let face_dir = self.base_dir.join(username).join(face_name);
        if !face_dir.exists() {
            fs::create_dir_all(&face_dir)?;
        }

        let face_map = self
            .users
            .entry(username.to_string())
            .or_default()
            .entry(face_name.to_string())
            .or_default();

        while face_map.len() >= max_captures {
            if let Some(oldest_uuid) = Self::find_oldest_file(&face_dir).map_err(|err| {
                UserDbError::Io(std::io::Error::other(err.to_string()))
            })? {
                let path = face_dir.join(format!("{}.bin", oldest_uuid));
                if path.exists() {
                    fs::remove_file(&path)?;
                }
                face_map.remove(&oldest_uuid);
            } else {
                break;
            }
        }

        let uuid = uuid::Uuid::new_v4().to_string();
        let file_path = face_dir.join(format!("{}.bin", uuid));
        Self::write_embedding(&file_path, embed)
            .map_err(|err| UserDbError::Io(std::io::Error::other(err.to_string())))?;
        face_map.insert(uuid.clone(), embed.clone());

        Ok(uuid)
    }

    fn find_oldest_file(dir: &std::path::Path) -> anyhow::Result<Option<String>> {
        let mut oldest: Option<(String, std::time::SystemTime)> = None;

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                continue;
            }
            let modified = entry.metadata()?.modified()?;
            let uuid = path.file_stem().unwrap().to_string_lossy().into_owned();

            match &oldest {
                Some((_, old_time)) if modified < *old_time => {
                    oldest = Some((uuid, modified));
                }
                None => {
                    oldest = Some((uuid, modified));
                }
                _ => {}
            }
        }

        Ok(oldest.map(|(uuid, _)| uuid))
    }

    fn clear_dir(path: std::path::PathBuf) -> anyhow::Result<bool> {
        let exists = path.exists();
        if exists {
            fs::remove_dir_all(&path)?;
        }
        Ok(exists)
    }

    pub fn remove_face(&mut self, username: &str, face_name: &str) -> Result<(), UserDbError> {
        let Some(faces) = self.users.get_mut(username) else {
            return Err(UserDbError::UserNotFound(username.to_string()));
        };

        if faces.remove(face_name).is_none() {
            return Err(UserDbError::FaceNotFound(face_name.to_string()));
        }

        let face_dir = self.base_dir.join(username).join(face_name);
        if face_dir.exists() {
            fs::remove_dir_all(face_dir)?;
        }

        Ok(())
    }

    pub fn rename_face(
        &mut self,
        username: &str,
        old_face_name: &str,
        new_face_name: &str,
    ) -> Result<(), UserDbError> {
        if old_face_name == new_face_name {
            return Ok(());
        }

        let Some(faces) = self.users.get_mut(username) else {
            return Err(UserDbError::UserNotFound(username.to_string()));
        };

        let Some(embeddings) = faces.remove(old_face_name) else {
            return Err(UserDbError::FaceNotFound(old_face_name.to_string()));
        };

        if faces.contains_key(new_face_name) {
            faces.insert(old_face_name.to_string(), embeddings);
            return Err(UserDbError::FaceExists(new_face_name.to_string()));
        }

        let old_face_dir = self.base_dir.join(username).join(old_face_name);
        let new_face_dir = self.base_dir.join(username).join(new_face_name);

        if new_face_dir.exists() {
            faces.insert(old_face_name.to_string(), embeddings);
            return Err(UserDbError::FaceExists(new_face_name.to_string()));
        }

        if !old_face_dir.exists() {
            faces.insert(old_face_name.to_string(), embeddings);
            return Err(UserDbError::FaceNotFound(old_face_name.to_string()));
        }

        fs::rename(&old_face_dir, &new_face_dir)?;

        faces.insert(new_face_name.to_string(), embeddings);

        Ok(())
    }

    pub fn clear_user(&mut self, username: &str) -> Result<(), UserDbError> {
        if self.users.remove(username).is_none() && !self.base_dir.join(username).exists() {
            return Err(UserDbError::UserNotFound(username.to_string()));
        }

        let user_dir = self.base_dir.join(username);
        if user_dir.exists() {
            fs::remove_dir_all(user_dir)?;
        }

        Ok(())
    }

    pub fn verify(&self, username: &str, embed: &ndarray::Array1<f32>, threshold: f32) -> bool {
        let Some(faces) = self.users.get(username) else {
            return false;
        };

        let candidates: Vec<&Array1<f32>> = faces
            .values()
            .flat_map(|uuid_map| uuid_map.values())
            .collect();

        candidates
            .into_par_iter()
            .any(|ref_embed| embed.dot(ref_embed) > threshold)
    }

    pub fn verify_user(
        &self,
        username: &str,
        embed: &ndarray::Array1<f32>,
        threshold: f32,
    ) -> Result<bool, UserDbError> {
        if !self.users.contains_key(username) {
            return Err(UserDbError::UserNotFound(username.to_string()));
        }
        Ok(self.verify(username, embed, threshold))
    }

    pub fn score_all(
        &self,
        username: &str,
        embed: &ndarray::Array1<f32>,
        threshold: f32,
    ) -> Vec<(String, f32, f32, bool, u32)> {
        let Some(faces) = self.users.get(username) else {
            return Vec::new();
        };

        let mut results: Vec<(String, f32, f32, bool, u32)> = faces
            .iter()
            .map(|(name, uuid_map)| {
                let best = uuid_map
                    .values()
                    .map(|ref_embed| embed.dot(ref_embed))
                    .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or(0.0);
                let pct = 100.0 / (1.0 + (-15.0_f32 * (best - 0.4)).exp());
                (
                    name.clone(),
                    best,
                    pct,
                    best > threshold,
                    uuid_map.len() as u32,
                )
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    pub fn match_faces(
        &self,
        username: &str,
        embed: &ndarray::Array1<f32>,
        threshold: f32,
    ) -> Result<Vec<(String, f32, f32, bool, u32)>, UserDbError> {
        if !self.users.contains_key(username) {
            return Err(UserDbError::UserNotFound(username.to_string()));
        }
        Ok(self.score_all(username, embed, threshold))
    }

    pub fn list_faces(&self, username: &str) -> Result<Vec<(String, u32)>, UserDbError> {
        let Some(face_map) = self.users.get(username) else {
            return Err(UserDbError::UserNotFound(username.to_string()));
        };

        let faces = face_map
            .iter()
            .map(|(name, embeds)| (name.clone(), embeds.len() as u32))
            .collect();
        Ok(faces)
    }

    pub fn get_user_embeddings(&self, username: &str) -> Option<Vec<&Array1<f32>>> {
        self.users
            .get(username)
            .map(|faces| faces.values().flat_map(|embeds| embeds.values()).collect())
    }
}
