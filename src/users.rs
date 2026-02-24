use ndarray::Array1;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const BASE_DIR: &str = "/var/lib/gaze/users";

pub struct UserDatabase {
    pub users: HashMap<String, HashMap<String, Array1<f32>>>,
}

impl UserDatabase {
    pub fn new() -> anyhow::Result<Self> {
        let mut db = Self {
            users: HashMap::new(),
        };
        db.load_all()?;
        Ok(db)
    }

    fn init_dirs() -> anyhow::Result<()> {
        if !Path::new(BASE_DIR).exists() {
            fs::create_dir_all(BASE_DIR)?;
        }
        Ok(())
    }

    pub fn load_all(&mut self) -> anyhow::Result<()> {
        Self::init_dirs()?;
        self.users.clear();

        for entry in fs::read_dir(BASE_DIR)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let username = path.file_name().unwrap().to_string_lossy().into_owned();
                let mut embeddings = HashMap::new();

                for file_entry in fs::read_dir(&path)? {
                    let file_entry = file_entry?;
                    let file_path = file_entry.path();
                    if file_path.extension().and_then(|e| e.to_str()) == Some("bin") {
                        if let Ok(bytes) = fs::read(&file_path) {
                            let float_count = bytes.len() / std::mem::size_of::<f32>();
                            let mut embed_vec = vec![0.0f32; float_count];
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    bytes.as_ptr(),
                                    embed_vec.as_mut_ptr() as *mut u8,
                                    bytes.len(),
                                );
                            }
                            let embed = Array1::from_vec(embed_vec);
                            let uuid = file_path
                                .file_stem()
                                .unwrap()
                                .to_string_lossy()
                                .into_owned();
                            embeddings.insert(uuid, embed);
                        }
                    }
                }
                self.users.insert(username, embeddings);
            }
        }
        Ok(())
    }

    pub fn add_face(&mut self, username: &str, embed: &Array1<f32>) -> anyhow::Result<String> {
        Self::init_dirs()?;
        let user_dir = PathBuf::from(BASE_DIR).join(username);
        if !user_dir.exists() {
            fs::create_dir_all(&user_dir)?;
        }

        let uuid = uuid::Uuid::new_v4().to_string();
        let file_path = user_dir.join(format!("{}.bin", uuid));

        let embed_slice = embed.as_slice().expect("Failed to get embedding slice");
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                embed_slice.as_ptr() as *const u8,
                embed_slice.len() * std::mem::size_of::<f32>(),
            )
        };
        fs::write(&file_path, bytes)?;

        self.users
            .entry(username.to_string())
            .or_default()
            .insert(uuid.clone(), embed.clone());

        Ok(uuid)
    }

    pub fn remove_face(&mut self, username: &str, uuid: &str) -> anyhow::Result<bool> {
        let file_path = PathBuf::from(BASE_DIR)
            .join(username)
            .join(format!("{}.bin", uuid));

        let mut removed = false;
        if file_path.exists() {
            fs::remove_file(&file_path)?;
            removed = true;
        }

        if let Some(user_embeds) = self.users.get_mut(username) {
            removed |= user_embeds.remove(uuid).is_some();
        }

        Ok(removed)
    }

    pub fn clear_user(&mut self, username: &str) -> anyhow::Result<bool> {
        let user_dir = PathBuf::from(BASE_DIR).join(username);
        let mut cleared = false;

        if user_dir.exists() {
            fs::remove_dir_all(&user_dir)?;
            cleared = true;
        }

        cleared |= self.users.remove(username).is_some();
        Ok(cleared)
    }

    pub fn get_user_embeddings(&self, username: &str) -> Option<Vec<&Array1<f32>>> {
        self.users.get(username).map(|m| m.values().collect())
    }
}
