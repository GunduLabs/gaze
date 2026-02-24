use ndarray::Array1;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

type FaceMap = HashMap<String, HashMap<String, Array1<f32>>>;

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

    fn init_dirs(&self) -> anyhow::Result<()> {
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
                embed_slice.len() * std::mem::size_of::<f32>(),
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
                    if bin_path.extension().and_then(|e| e.to_str()) == Some("bin") {
                        if let Ok(embed) = Self::read_embedding(&bin_path) {
                            let uuid = bin_path.file_stem().unwrap().to_string_lossy().into_owned();
                            embeddings.insert(uuid, embed);
                        }
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
    ) -> anyhow::Result<String> {
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
            if let Some(oldest_uuid) = Self::find_oldest_file(&face_dir)? {
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
        Self::write_embedding(&file_path, embed)?;
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

    pub fn remove_face(&mut self, username: &str, face_name: &str) -> anyhow::Result<bool> {
        let face_dir = self.base_dir.join(username).join(face_name);
        let mut cleared = false;

        if face_dir.exists() {
            fs::remove_dir_all(&face_dir)?;
            cleared = true;
        }

        if let Some(faces) = self.users.get_mut(username) {
            cleared |= faces.remove(face_name).is_some();
        }

        Ok(cleared)
    }

    pub fn clear_user(&mut self, username: &str) -> anyhow::Result<bool> {
        let user_dir = self.base_dir.join(username);
        let mut cleared = false;

        if user_dir.exists() {
            fs::remove_dir_all(&user_dir)?;
            cleared = true;
        }

        cleared |= self.users.remove(username).is_some();
        Ok(cleared)
    }

    pub fn find_match(
        &self,
        username: &str,
        embed: &ndarray::Array1<f32>,
        threshold: f32,
    ) -> Option<String> {
        let faces = self.users.get(username)?;
        for (face_name, uuid_map) in faces {
            for ref_embed in uuid_map.values() {
                if embed.dot(ref_embed) > threshold {
                    return Some(face_name.clone());
                }
            }
        }
        None
    }

    pub fn get_user_embeddings(&self, username: &str) -> Option<Vec<&Array1<f32>>> {
        self.users
            .get(username)
            .map(|faces| faces.values().flat_map(|embeds| embeds.values()).collect())
    }
}
