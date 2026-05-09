use ndarray::Array1;

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

type FaceMap = HashMap<String, HashMap<String, Array1<f32>>>;
pub type FaceScore = (String, f32, f32, bool, u32);

#[derive(Debug)]
pub enum UserDbError {
    UserNotFound(String),
    FaceNotFound(String),
    FaceExists(String),
    InvalidName(String),
    Io(std::io::Error),
}

impl std::fmt::Display for UserDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserDbError::UserNotFound(username) => write!(f, "User '{}' not found", username),
            UserDbError::FaceNotFound(face_name) => write!(f, "Face '{}' not found", face_name),
            UserDbError::FaceExists(face_name) => write!(f, "Face '{}' already exists", face_name),
            UserDbError::InvalidName(msg) => write!(f, "{}", msg),
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
    max_templates: usize,
    users: HashMap<String, FaceMap>,
}

impl UserDatabase {
    fn validate_component(kind: &str, value: &str) -> Result<(), UserDbError> {
        if value.is_empty() || value.trim() != value {
            return Err(UserDbError::InvalidName(format!(
                "{kind} cannot be empty or contain leading/trailing whitespace"
            )));
        }
        if value == "."
            || value == ".."
            || value.contains('/')
            || value.contains('\\')
            || value.contains('\0')
            || value.chars().any(char::is_control)
        {
            return Err(UserDbError::InvalidName(format!(
                "{kind} must be a single safe path component"
            )));
        }
        Ok(())
    }

    pub fn validate_username(username: &str) -> Result<(), UserDbError> {
        Self::validate_component("username", username)
    }

    pub fn validate_face_name(face_name: &str) -> Result<(), UserDbError> {
        Self::validate_component("face name", face_name)
    }

    fn validate_template_id(template_id: &str) -> Result<(), UserDbError> {
        Self::validate_component("template id", template_id)
    }

    fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
        fs::create_dir_all(path)?;
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is not a private directory",
                path.display()
            )));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    fn remove_private_dir_all(path: &Path) -> std::io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is not a private directory",
                path.display()
            )));
        }
        fs::remove_dir_all(path)
    }

    pub fn new(base_dir: &str, max_templates: usize) -> anyhow::Result<Self> {
        let mut db = Self {
            base_dir: PathBuf::from(base_dir),
            max_templates,
            users: HashMap::new(),
        };
        db.load_all()?;
        Ok(db)
    }

    fn init_dirs(&self) -> std::io::Result<()> {
        Self::ensure_private_dir(&self.base_dir)
    }

    fn user_dir(&self, username: &str) -> PathBuf {
        self.base_dir.join(username)
    }

    fn face_dir(&self, username: &str, face_name: &str) -> PathBuf {
        self.user_dir(username).join(face_name)
    }

    fn read_embedding(path: &Path) -> anyhow::Result<Array1<f32>> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_file() {
            anyhow::bail!("embedding path is not a regular file: {}", path.display());
        }
        let bytes = fs::read(path)?;
        if bytes.is_empty() || bytes.len() % std::mem::size_of::<f32>() != 0 {
            anyhow::bail!("invalid embedding length in {}", path.display());
        }
        let embed_vec = bytes
            .chunks_exact(std::mem::size_of::<f32>())
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect();
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
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        file.flush()?;
        Ok(())
    }

    pub fn load_all(&mut self) -> anyhow::Result<()> {
        self.init_dirs()?;
        self.users.clear();

        for user_entry in fs::read_dir(&self.base_dir)? {
            let user_entry = user_entry?;
            if !user_entry.file_type()?.is_dir() {
                continue;
            }
            let user_path = user_entry.path();
            let username = user_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if Self::validate_username(&username).is_err() {
                continue;
            }
            let mut faces: FaceMap = HashMap::new();

            for face_entry in fs::read_dir(&user_path)? {
                let face_entry = face_entry?;
                if !face_entry.file_type()?.is_dir() {
                    continue;
                }
                let face_path = face_entry.path();
                let face_name = face_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if Self::validate_face_name(&face_name).is_err() {
                    continue;
                }
                let mut embeddings = HashMap::new();

                let mut walk_stack = vec![face_path.clone()];
                while let Some(current_path) = walk_stack.pop() {
                    for entry in fs::read_dir(current_path)? {
                        let entry = entry?;
                        let path = entry.path();
                        let file_type = entry.file_type()?;
                        if file_type.is_dir() {
                            walk_stack.push(path);
                        } else if file_type.is_file()
                            && path.extension().and_then(|e| e.to_str()) == Some("bin")
                            && let Ok(embed) = Self::read_embedding(&path)
                        {
                            let uuid = path.file_stem().unwrap().to_string_lossy().into_owned();
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

    pub fn add_template(
        &mut self,
        username: &str,
        face_name: &str,
        template_id: &str,
        embeddings: Vec<Array1<f32>>,
    ) -> Result<(), UserDbError> {
        self.init_dirs()?;
        Self::validate_username(username)?;
        Self::validate_face_name(face_name)?;
        Self::validate_template_id(template_id)?;

        let user_dir = self.user_dir(username);
        let face_dir = self.face_dir(username, face_name);
        Self::ensure_private_dir(&user_dir)?;
        Self::ensure_private_dir(&face_dir)?;
        let template_dir = self.face_dir(username, face_name).join(template_id);

        if !template_dir.exists() {
            let mut templates = self.list_template_ids(username, face_name)?;
            while templates.len() >= self.max_templates && self.max_templates > 0 {
                let oldest_id = templates.remove(0);
                self.remove_face_template(username, face_name, &oldest_id)
                    .map_err(|e| UserDbError::Io(std::io::Error::other(e.to_string())))?;
            }
        }

        Self::ensure_private_dir(&template_dir)?;

        for embed in embeddings {
            let uuid = uuid::Uuid::new_v4().to_string();
            let file_path = template_dir.join(format!("{}.bin", uuid));
            Self::write_embedding(&file_path, &embed)
                .map_err(|err| UserDbError::Io(std::io::Error::other(err.to_string())))?;
        }

        self.load_all()
            .map_err(|e| UserDbError::Io(std::io::Error::other(e.to_string())))?;

        Ok(())
    }

    pub fn list_template_ids(
        &self,
        username: &str,
        face_name: &str,
    ) -> Result<Vec<String>, UserDbError> {
        Self::validate_username(username)?;
        Self::validate_face_name(face_name)?;
        let face_dir = self.face_dir(username, face_name);
        if !face_dir.exists() {
            return Ok(vec![]);
        }
        let meta = fs::symlink_metadata(&face_dir)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(UserDbError::Io(std::io::Error::other(format!(
                "{} is not a face directory",
                face_dir.display()
            ))));
        }

        let mut templates = vec![];
        for entry in fs::read_dir(face_dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                && Self::validate_template_id(name).is_ok()
            {
                templates.push(name.to_string());
            }
        }

        templates.sort_by(|a, b| {
            let a_val = a.parse::<u64>().unwrap_or(0);
            let b_val = b.parse::<u64>().unwrap_or(0);
            a_val.cmp(&b_val)
        });

        Ok(templates)
    }

    pub fn remove_face_template(
        &mut self,
        username: &str,
        face_name: &str,
        template_id: &str,
    ) -> anyhow::Result<()> {
        Self::validate_username(username).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Self::validate_face_name(face_name).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Self::validate_template_id(template_id).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let template_dir = self.face_dir(username, face_name).join(template_id);
        if template_dir.exists() {
            Self::remove_private_dir_all(&template_dir)?;
            self.load_all()?;
        }
        Ok(())
    }

    pub fn remove_face(&mut self, username: &str, face_name: &str) -> Result<(), UserDbError> {
        Self::validate_username(username)?;
        Self::validate_face_name(face_name)?;
        let Some(faces) = self.users.get_mut(username) else {
            return Err(UserDbError::UserNotFound(username.to_string()));
        };

        if faces.remove(face_name).is_none() {
            return Err(UserDbError::FaceNotFound(face_name.to_string()));
        }

        let face_dir = self.base_dir.join(username).join(face_name);
        if face_dir.exists() {
            Self::remove_private_dir_all(&face_dir)?;
        }

        Ok(())
    }

    pub fn rename_face(
        &mut self,
        username: &str,
        old_face_name: &str,
        new_face_name: &str,
    ) -> Result<(), UserDbError> {
        Self::validate_username(username)?;
        Self::validate_face_name(old_face_name)?;
        Self::validate_face_name(new_face_name)?;
        if old_face_name == new_face_name {
            return Ok(());
        }

        let old_face_dir = self.face_dir(username, old_face_name);
        let new_face_dir = self.face_dir(username, new_face_name);

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

        if fs::symlink_metadata(&new_face_dir).is_ok() {
            faces.insert(old_face_name.to_string(), embeddings);
            return Err(UserDbError::FaceExists(new_face_name.to_string()));
        }

        let old_meta = match fs::symlink_metadata(&old_face_dir) {
            Ok(meta) => meta,
            Err(_) => {
                faces.insert(old_face_name.to_string(), embeddings);
                return Err(UserDbError::FaceNotFound(old_face_name.to_string()));
            }
        };
        if old_meta.file_type().is_symlink() || !old_meta.is_dir() {
            faces.insert(old_face_name.to_string(), embeddings);
            return Err(UserDbError::FaceNotFound(old_face_name.to_string()));
        }

        fs::rename(&old_face_dir, &new_face_dir)?;

        faces.insert(new_face_name.to_string(), embeddings);

        Ok(())
    }

    pub fn clear_user(&mut self, username: &str) -> Result<(), UserDbError> {
        Self::validate_username(username)?;
        self.users.remove(username);
        let user_dir = self.user_dir(username);
        if user_dir.exists() {
            Self::remove_private_dir_all(&user_dir)?;
        }
        Ok(())
    }

    pub fn match_faces(
        &self,
        username: &str,
        embed: &ndarray::Array1<f32>,
        threshold: f32,
    ) -> Result<Vec<FaceScore>, UserDbError> {
        Self::validate_username(username)?;
        let faces = self
            .users
            .get(username)
            .ok_or_else(|| UserDbError::UserNotFound(username.to_string()))?;

        let mut results: Vec<FaceScore> = faces
            .iter()
            .map(|(name, uuid_map)| {
                let best = uuid_map
                    .values()
                    .filter(|ref_embed| ref_embed.len() == embed.len())
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
        Ok(results)
    }

    pub fn list_faces(&self, username: &str) -> Result<Vec<(String, u32)>, UserDbError> {
        Self::validate_username(username)?;
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
        if Self::validate_username(username).is_err() {
            return None;
        }
        self.users
            .get(username)
            .map(|faces| faces.values().flat_map(|embeds| embeds.values()).collect())
    }

    pub fn set_max_templates(&mut self, max: usize) {
        self.max_templates = max;
    }
}
