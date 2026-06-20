use gaze_core::face::Spectrum;
use ndarray::Array1;

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::crypto::{self, EmbeddingCipher};

type FaceMap = HashMap<String, HashMap<String, (Array1<f32>, Spectrum)>>;
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
    cipher: Option<EmbeddingCipher>,
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
        Self::new_with_cipher(base_dir, max_templates, None)
    }

    pub fn new_with_cipher(
        base_dir: &str,
        max_templates: usize,
        cipher: Option<EmbeddingCipher>,
    ) -> anyhow::Result<Self> {
        let mut db = Self {
            base_dir: PathBuf::from(base_dir),
            max_templates,
            users: HashMap::new(),
            cipher,
        };
        db.load_all()?;
        Ok(db)
    }

    pub fn is_encrypted(&self) -> bool {
        self.cipher.is_some()
    }

    pub fn set_cipher(&mut self, cipher: Option<EmbeddingCipher>) {
        self.cipher = cipher;
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

    fn read_embedding(&self, path: &Path) -> anyhow::Result<Array1<f32>> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_file() {
            anyhow::bail!("embedding path is not a regular file: {}", path.display());
        }
        let raw = fs::read(path)?;
        // Also read legacy plaintext files, so the store can be migrated in place.
        let bytes = if crypto::is_encrypted(&raw) {
            let cipher = self.cipher.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "{} is encrypted but template encryption is disabled",
                    path.display()
                )
            })?;
            cipher.decrypt(&raw)?
        } else {
            raw
        };
        if bytes.is_empty() || bytes.len() % std::mem::size_of::<f32>() != 0 {
            anyhow::bail!("invalid embedding length in {}", path.display());
        }
        let embed_vec = bytes
            .chunks_exact(std::mem::size_of::<f32>())
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect();
        Ok(Array1::from_vec(embed_vec))
    }

    fn encode_embedding(&self, embed: &Array1<f32>) -> anyhow::Result<Vec<u8>> {
        let embed_slice = embed.as_slice().expect("Failed to get embedding slice");
        // Templates are not portable across architectures with different endianness.
        let plain: &[u8] = unsafe {
            std::slice::from_raw_parts(
                embed_slice.as_ptr() as *const u8,
                std::mem::size_of_val(embed_slice),
            )
        };
        match &self.cipher {
            Some(cipher) => cipher.encrypt(plain),
            None => Ok(plain.to_vec()),
        }
    }

    fn write_embedding(&self, path: &Path, embed: &Array1<f32>) -> anyhow::Result<()> {
        let bytes = self.encode_embedding(embed)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(&bytes)?;
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
                            && let Ok(embed) = self.read_embedding(&path)
                        {
                            let stem = path.file_stem().unwrap().to_string_lossy();
                            let (uuid, spectrum) = if stem.ends_with("_ir") {
                                (stem.strip_suffix("_ir").unwrap().to_string(), Spectrum::Ir)
                            } else if stem.ends_with("_rgb") {
                                (
                                    stem.strip_suffix("_rgb").unwrap().to_string(),
                                    Spectrum::Rgb,
                                )
                            } else {
                                (stem.into_owned(), Spectrum::Rgb)
                            };
                            embeddings.insert(uuid, (embed, spectrum));
                        }
                    }
                }
                faces.insert(face_name, embeddings);
            }
            self.users.insert(username, faces);
        }
        Ok(())
    }

    fn collect_bin_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        if !self.base_dir.exists() {
            return Ok(files);
        }
        let mut stack = vec![self.base_dir.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file()
                    && path.extension().and_then(|e| e.to_str()) == Some("bin")
                {
                    files.push(path);
                }
            }
        }
        Ok(files)
    }

    fn replace_file_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("embedding path has no parent: {}", path.display()))?;
        let file_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
            anyhow::anyhow!("embedding path has no file name: {}", path.display())
        })?;
        let tmp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        if let Err(e) = file.write_all(bytes).and_then(|_| file.flush()) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn migrate_plaintext_to_encrypted(&self) -> anyhow::Result<usize> {
        let Some(cipher) = self.cipher.as_ref() else {
            return Ok(0);
        };
        let mut migrated = 0;
        for path in self.collect_bin_files()? {
            let raw = fs::read(&path)?;
            if crypto::is_encrypted(&raw) {
                continue;
            }
            let blob = cipher.encrypt(&raw)?;
            Self::replace_file_bytes(&path, &blob)?;
            migrated += 1;
        }
        Ok(migrated)
    }

    pub fn decrypt_all_with(&self, cipher: &EmbeddingCipher) -> anyhow::Result<usize> {
        let mut converted = 0;
        for path in self.collect_bin_files()? {
            let raw = fs::read(&path)?;
            if !crypto::is_encrypted(&raw) {
                continue;
            }
            let plain = cipher.decrypt(&raw)?;
            Self::replace_file_bytes(&path, &plain)?;
            converted += 1;
        }
        Ok(converted)
    }

    pub fn add_template(
        &mut self,
        username: &str,
        face_name: &str,
        template_id: &str,
        embeddings: Vec<(Array1<f32>, Spectrum)>,
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

        for (embed, spectrum) in embeddings {
            let uuid = uuid::Uuid::new_v4().to_string();
            let suffix = match spectrum {
                Spectrum::Rgb => "rgb",
                Spectrum::Ir => "ir",
            };
            let file_path = template_dir.join(format!("{}_{}.bin", uuid, suffix));
            self.write_embedding(&file_path, &embed)
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
        spectrum: Spectrum,
    ) -> Result<Vec<FaceScore>, UserDbError> {
        Self::validate_username(username)?;
        let faces = self
            .users
            .get(username)
            .ok_or_else(|| UserDbError::UserNotFound(username.to_string()))?;

        let mut results: Vec<FaceScore> = faces
            .iter()
            .map(|(name, uuid_map)| {
                let matching_embeds: Vec<&Array1<f32>> = uuid_map
                    .values()
                    .filter(|(ref_embed, spec)| *spec == spectrum && ref_embed.len() == embed.len())
                    .map(|(ref_embed, _)| ref_embed)
                    .collect();

                let ref_list = if matching_embeds.is_empty() {
                    uuid_map
                        .values()
                        .filter(|(ref_embed, spec)| {
                            *spec == Spectrum::Rgb && ref_embed.len() == embed.len()
                        })
                        .map(|(ref_embed, _)| ref_embed)
                        .collect()
                } else {
                    matching_embeds
                };

                let best = ref_list
                    .into_iter()
                    .map(|ref_embed| embed.dot(ref_embed))
                    .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or(0.0);
                // Center at 0.4 with slope 15 so values near the threshold spread out nicely.
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

    pub fn list_faces(
        &self,
        username: &str,
    ) -> Result<Vec<(String, u32, bool, bool)>, UserDbError> {
        Self::validate_username(username)?;
        let Some(face_map) = self.users.get(username) else {
            return Err(UserDbError::UserNotFound(username.to_string()));
        };

        let faces = face_map
            .iter()
            .map(|(name, embeds)| {
                let mut has_rgb = false;
                let mut has_ir = false;
                for (_, spectrum) in embeds.values() {
                    match spectrum {
                        Spectrum::Rgb => has_rgb = true,
                        Spectrum::Ir => has_ir = true,
                    }
                }
                (name.clone(), embeds.len() as u32, has_rgb, has_ir)
            })
            .collect();
        Ok(faces)
    }

    pub fn get_user_embeddings(&self, username: &str) -> Option<Vec<&Array1<f32>>> {
        if Self::validate_username(username).is_err() {
            return None;
        }
        self.users.get(username).map(|faces| {
            faces
                .values()
                .flat_map(|embeds| embeds.values().map(|(embed, _)| embed))
                .collect()
        })
    }

    pub fn set_max_templates(&mut self, max: usize) {
        self.max_templates = max;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "gaze-users-test-{}-{}-{name}",
                std::process::id(),
                unique
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn embedding(values: &[f32]) -> Array1<f32> {
        Array1::from_vec(values.to_vec())
    }

    fn rgb_embeds(embeds: Vec<Array1<f32>>) -> Vec<(Array1<f32>, Spectrum)> {
        embeds.into_iter().map(|e| (e, Spectrum::Rgb)).collect()
    }

    fn sorted_faces(db: &UserDatabase, username: &str) -> Vec<(String, u32, bool, bool)> {
        let mut faces = db.list_faces(username).unwrap();
        faces.sort_by(|a, b| a.0.cmp(&b.0));
        faces
    }

    #[test]
    fn validate_names_accept_safe_components_and_reject_path_tricks() {
        for name in ["alice", "Alice Smith", "face-1", "face_2"] {
            UserDatabase::validate_username(name).unwrap();
            UserDatabase::validate_face_name(name).unwrap();
        }

        for name in [
            "", " alice", "alice ", ".", "..", "a/b", "a\\b", "a\0b", "a\nb",
        ] {
            assert!(
                UserDatabase::validate_username(name).is_err(),
                "{name:?} should be invalid"
            );
            assert!(
                UserDatabase::validate_face_name(name).is_err(),
                "{name:?} should be invalid"
            );
        }
    }

    #[test]
    fn error_display_messages_are_stable() {
        assert_eq!(
            UserDbError::UserNotFound("alice".to_string()).to_string(),
            "User 'alice' not found"
        );
        assert_eq!(
            UserDbError::FaceNotFound("work".to_string()).to_string(),
            "Face 'work' not found"
        );
        assert_eq!(
            UserDbError::FaceExists("home".to_string()).to_string(),
            "Face 'home' already exists"
        );
        assert_eq!(
            UserDbError::InvalidName("bad name".to_string()).to_string(),
            "bad name"
        );
    }

    #[test]
    fn add_template_persists_embeddings_and_reload_reads_them() {
        let temp = TempDir::new("persist");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new(base, 4).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0]), embedding(&[0.0, 1.0])]),
        )
        .unwrap();

        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 2, true, false)]
        );
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 2);

        let db = UserDatabase::new(base, 4).unwrap();
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 2, true, false)]
        );
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 2);
    }

    #[test]
    fn max_templates_evicts_oldest_numeric_template_ids() {
        let temp = TempDir::new("evict");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        for id in ["1", "2", "3"] {
            db.add_template(
                "alice",
                "work",
                id,
                rgb_embeds(vec![embedding(&[id.parse::<f32>().unwrap()])]),
            )
            .unwrap();
        }

        assert_eq!(
            db.list_template_ids("alice", "work").unwrap(),
            vec!["2", "3"]
        );
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 2, true, false)]
        );
    }

    #[test]
    fn rename_remove_and_clear_update_memory_and_disk() {
        let temp = TempDir::new("rename-remove");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        db.add_template("alice", "home", "1", rgb_embeds(vec![embedding(&[0.5])]))
            .unwrap();

        assert!(matches!(
            db.rename_face("alice", "work", "home"),
            Err(UserDbError::FaceExists(face)) if face == "home"
        ));
        db.rename_face("alice", "work", "office").unwrap();
        assert!(!temp.path().join("alice/work").exists());
        assert!(temp.path().join("alice/office").exists());
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![
                ("home".to_string(), 1, true, false),
                ("office".to_string(), 1, true, false)
            ]
        );

        db.remove_face("alice", "home").unwrap();
        assert!(!temp.path().join("alice/home").exists());
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("office".to_string(), 1, true, false)]
        );

        db.clear_user("alice").unwrap();
        assert!(!temp.path().join("alice").exists());
        assert!(matches!(
            db.list_faces("alice"),
            Err(UserDbError::UserNotFound(user)) if user == "alice"
        ));
    }

    #[test]
    fn match_faces_sorts_scores_and_uses_strict_threshold() {
        let temp = TempDir::new("match");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 3).unwrap();
        db.add_template(
            "alice",
            "strong",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();
        db.add_template(
            "alice",
            "weak",
            "1",
            rgb_embeds(vec![embedding(&[0.25, 0.0])]),
        )
        .unwrap();

        let results = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 0.5, Spectrum::Rgb)
            .unwrap();
        assert_eq!(results[0].0, "strong");
        assert_eq!(results[0].1, 1.0);
        assert!(results[0].3);
        assert_eq!(results[1].0, "weak");
        assert_eq!(results[1].1, 0.25);
        assert!(!results[1].3);

        let results = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 1.0, Spectrum::Rgb)
            .unwrap();
        assert_eq!(results[0].0, "strong");
        assert!(!results[0].3, "threshold comparison should be strict");
    }

    #[test]
    fn match_faces_ignores_dimension_mismatches() {
        let temp = TempDir::new("dimension-mismatch");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 3).unwrap();
        db.add_template(
            "alice",
            "odd",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0, 0.0])]),
        )
        .unwrap();

        let results = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 0.0, Spectrum::Rgb)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "odd");
        assert_eq!(results[0].1, 0.0);
        assert!(!results[0].3);
        assert_eq!(results[0].4, 1);
    }

    #[test]
    fn match_faces_partitions_by_spectrum() {
        let temp = TempDir::new("spectrum-partition");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 3).unwrap();
        db.add_template(
            "alice",
            "face",
            "1",
            vec![
                (embedding(&[1.0, 0.0]), Spectrum::Rgb),
                (embedding(&[0.0, 1.0]), Spectrum::Ir),
            ],
        )
        .unwrap();

        let results_rgb = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 0.5, Spectrum::Rgb)
            .unwrap();
        assert!(results_rgb[0].3);

        let results_rgb_wrong = db
            .match_faces("alice", &embedding(&[0.0, 1.0]), 0.5, Spectrum::Rgb)
            .unwrap();
        assert!(!results_rgb_wrong[0].3);

        let results_ir = db
            .match_faces("alice", &embedding(&[0.0, 1.0]), 0.5, Spectrum::Ir)
            .unwrap();
        assert!(results_ir[0].3);

        let results_ir_wrong = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 0.5, Spectrum::Ir)
            .unwrap();
        assert!(!results_ir_wrong[0].3);
    }

    fn test_cipher() -> EmbeddingCipher {
        EmbeddingCipher::new(&[42u8; crypto::KEY_LEN])
    }

    fn all_bins_encrypted(db: &UserDatabase) -> bool {
        db.collect_bin_files()
            .unwrap()
            .iter()
            .all(|f| crypto::is_encrypted(&fs::read(f).unwrap()))
    }

    #[test]
    fn encrypted_round_trip_persists_and_reloads() {
        let temp = TempDir::new("enc-persist");
        let base = temp.path().to_str().unwrap();

        let mut db = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0]), embedding(&[0.0, 1.0])]),
        )
        .unwrap();
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 2);

        let files = db.collect_bin_files().unwrap();
        assert_eq!(files.len(), 2);
        assert!(all_bins_encrypted(&db));

        let reopened = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        assert_eq!(reopened.get_user_embeddings("alice").unwrap().len(), 2);
    }

    #[test]
    fn encrypted_templates_are_unreadable_without_the_key() {
        let temp = TempDir::new("enc-nokey");
        let base = temp.path().to_str().unwrap();

        let mut db = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();

        let plain = UserDatabase::new(base, 4).unwrap();
        assert_eq!(plain.get_user_embeddings("alice").map(|v| v.len()), Some(0));
    }

    #[test]
    fn migrate_plaintext_to_encrypted_converts_in_place() {
        let temp = TempDir::new("enc-migrate");
        let base = temp.path().to_str().unwrap();

        let mut plain = UserDatabase::new(base, 4).unwrap();
        plain
            .add_template(
                "alice",
                "work",
                "1",
                rgb_embeds(vec![embedding(&[1.0, 2.0, 3.0])]),
            )
            .unwrap();
        assert!(!all_bins_encrypted(&plain), "files start as plaintext");

        let enc = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        assert_eq!(enc.get_user_embeddings("alice").unwrap().len(), 1);

        let expected = enc.collect_bin_files().unwrap().len();
        assert_eq!(enc.migrate_plaintext_to_encrypted().unwrap(), expected);
        assert!(all_bins_encrypted(&enc));
        assert_eq!(enc.migrate_plaintext_to_encrypted().unwrap(), 0);

        let reopened = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        assert_eq!(reopened.get_user_embeddings("alice").unwrap().len(), 1);
    }

    #[test]
    fn decrypt_all_with_reverses_encryption() {
        let temp = TempDir::new("enc-decrypt");
        let base = temp.path().to_str().unwrap();

        let mut enc = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        enc.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();
        assert!(all_bins_encrypted(&enc));

        let expected = enc.collect_bin_files().unwrap().len();
        assert_eq!(enc.decrypt_all_with(&test_cipher()).unwrap(), expected);
        assert!(!all_bins_encrypted(&enc));

        let plain = UserDatabase::new(base, 4).unwrap();
        assert_eq!(plain.get_user_embeddings("alice").unwrap().len(), 1);
    }
}
