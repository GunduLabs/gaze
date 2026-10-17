// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gaze_vision::face::Spectrum;
use ndarray::Array1;

use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::crypto::{self, EmbeddingCipher};

mod recode;

type FaceMap = HashMap<String, HashMap<String, (Array1<f32>, Spectrum)>>;
pub type FaceScore = (String, f32, f32, bool, u32);

const RECOGNIZER_TAG: &str = "recognizer";
const PRIVATE_PREFIX: &str = ".gaze-";

fn sync_dir(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

/// A private directory which is invisible to store walkers, including after a crash.
struct StagingDir(PathBuf);

impl StagingDir {
    fn new(parent: &Path, prefix: &str) -> std::io::Result<Self> {
        let path = parent.join(format!("{prefix}{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.0.display(), %error, "could not clean up private staging directory");
        }
    }
}

/// Migrations replace the complete store in one namespace operation. Never fall back to
/// a sequence of file renames: a crash would otherwise publish a mixed-format store.
fn exchange_dirs(left: &Path, right: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let left = std::ffi::CString::new(left.as_os_str().as_bytes())?;
    let right = std::ffi::CString::new(right.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(left.as_ptr(), right.as_ptr(), libc::RENAME_SWAP) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = {
        let _ = (left, right);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic directory exchange is unavailable",
        ));
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

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
            UserDbError::UserNotFound(username) => write!(f, "User '{username}' not found"),
            UserDbError::FaceNotFound(face_name) => write!(f, "Face '{face_name}' not found"),
            UserDbError::FaceExists(face_name) => write!(f, "Face '{face_name}' already exists"),
            UserDbError::InvalidName(msg) => write!(f, "{msg}"),
            UserDbError::Io(err) => write!(f, "{err}"),
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
    recognizer: Option<String>,
    unavailable: Cell<bool>,
    #[cfg(test)]
    fail_reload: bool,
    #[cfg(test)]
    fail_rollback: bool,
}

impl UserDatabase {
    fn is_abandoned_name(name: &str) -> bool {
        [".gaze-stage-", ".gaze-deleted-"].iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok())
        })
    }

    fn validate_component(kind: &str, value: &str) -> Result<(), UserDbError> {
        if value.is_empty() || value.trim() != value {
            return Err(UserDbError::InvalidName(format!(
                "{kind} cannot be empty or contain leading/trailing whitespace"
            )));
        }
        if value == "."
            || value == ".."
            || value.starts_with(PRIVATE_PREFIX)
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

    fn retire_dir(path: &Path) -> std::io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(std::io::Error::other("refusing to retire a non-directory"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("directory has no parent"))?;
        let retired = parent.join(format!(".gaze-deleted-{}", uuid::Uuid::new_v4()));
        fs::rename(path, &retired)?;
        sync_dir(parent)?;
        // The durable rename revoked it. Cleanup is best effort and can resume at startup.
        if let Err(error) = Self::remove_private_dir_all(&retired) {
            tracing::warn!(path = %retired.display(), %error, "retired templates await cleanup");
        }
        sync_dir(parent)
    }

    /// Held for the process lifetime by gazed, including maintenance invocations.
    pub fn lock_store(base_dir: &Path) -> anyhow::Result<File> {
        use std::os::fd::AsRawFd;
        let parent = base_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("store has no parent"))?;
        Self::ensure_private_dir(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join(".gaze-store.lock"))?;
        anyhow::ensure!(
            file.metadata()?.is_file(),
            "store lock is not a regular file"
        );
        anyhow::ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "template store is already in use; stop gazed before maintenance"
        );
        Ok(file)
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
            base_dir: if Path::new(base_dir).is_absolute() {
                PathBuf::from(base_dir)
            } else {
                std::env::current_dir()?.join(base_dir)
            },
            max_templates,
            users: HashMap::new(),
            cipher,
            recognizer: None,
            unavailable: Cell::new(false),
            #[cfg(test)]
            fail_reload: false,
            #[cfg(test)]
            fail_rollback: false,
        };
        db.init_dirs()?;
        db.cleanup_abandoned_staging()?;
        db.load_all()?;
        Ok(db)
    }

    pub fn is_encrypted(&self) -> bool {
        self.cipher.is_some()
    }

    pub fn set_cipher(&mut self, cipher: Option<EmbeddingCipher>) {
        self.cipher = cipher;
    }

    pub fn use_recognizer(&mut self, recognizer: &str) -> anyhow::Result<()> {
        let previous = self.recognizer.replace(recognizer.to_string());
        if let Err(error) = self.load_all() {
            self.recognizer = previous;
            return Err(error);
        }
        Ok(())
    }

    fn recognizer_tag(dir: &Path) -> Option<String> {
        let tag = fs::read_to_string(dir.join(RECOGNIZER_TAG)).ok()?;
        let tag = tag.trim();
        (!tag.is_empty()).then(|| tag.to_string())
    }

    fn write_recognizer_tag(dir: &Path, recognizer: &str) -> std::io::Result<()> {
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join(RECOGNIZER_TAG))
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(e) => return Err(e),
        };
        file.write_all(recognizer.as_bytes())?;
        file.sync_all()?;
        sync_dir(dir)
    }

    // Untagged templates predate tagging, so they can only be attributed to the recognizer
    // the daemon is starting with.
    pub fn adopt_untagged_templates(&self, recognizer: &str) -> anyhow::Result<usize> {
        let mut dirs: Vec<PathBuf> = self
            .collect_bin_files()?
            .into_iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect();
        dirs.sort();
        dirs.dedup();
        let mut adopted = 0;
        for dir in dirs {
            if Self::recognizer_tag(&dir).is_none() {
                Self::write_recognizer_tag(&dir, recognizer)?;
                adopted += 1;
            }
        }
        Ok(adopted)
    }

    fn ensure_available(&self) -> std::io::Result<()> {
        if self.unavailable.get() {
            return Err(std::io::Error::other(
                "template storage needs recovery; restart gazed before matching or modifying templates",
            ));
        }
        Ok(())
    }

    fn init_dirs(&self) -> std::io::Result<()> {
        self.ensure_available()?;
        Self::ensure_private_dir(&self.base_dir)?;
        sync_dir(&self.base_dir)?;
        if let Some(parent) = self.base_dir.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    }

    fn user_dir(&self, username: &str) -> PathBuf {
        self.base_dir.join(username)
    }

    fn face_dir(&self, username: &str, face_name: &str) -> PathBuf {
        self.user_dir(username).join(face_name)
    }

    // Face labels are deliberately excluded so a rename needs no re-encryption. The
    // filename binds UUID and spectrum; the account name prevents cross-user substitution.
    fn embedding_aad(&self, path: &Path) -> anyhow::Result<Vec<u8>> {
        let relative = path.strip_prefix(&self.base_dir)?;
        let username = relative
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .ok_or_else(|| anyhow::anyhow!("embedding has no account"))?;
        Self::validate_username(username)?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("embedding has no filename"))?;
        let mut aad = b"gaze-template-v2\0".to_vec();
        aad.extend_from_slice(username.as_bytes());
        aad.push(0);
        aad.extend_from_slice(filename.as_bytes());
        Ok(aad)
    }

    fn read_embedding(&self, path: &Path) -> anyhow::Result<Array1<f32>> {
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_file() {
            anyhow::bail!("embedding path is not a regular file: {}", path.display());
        }
        let raw = fs::read(path)?;
        let bytes = if crypto::is_encrypted(&raw) {
            let cipher = self.cipher.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "{} is encrypted but template encryption is disabled",
                    path.display()
                )
            })?;
            cipher.decrypt(&raw, &self.embedding_aad(path)?)?
        } else if self.cipher.is_some() {
            // Plaintext here would bypass AES-GCM authentication and allow an unauthenticated
            // template into the encrypted store; it must go through explicit migration instead.
            anyhow::bail!(
                "{} is not encrypted but template encryption is enabled",
                path.display()
            );
        } else {
            raw
        };
        if bytes.is_empty() || bytes.len() % std::mem::size_of::<f32>() != 0 {
            anyhow::bail!("invalid embedding length in {}", path.display());
        }
        let embed_vec = bytes
            .as_chunks::<{ std::mem::size_of::<f32>() }>()
            .0
            .iter()
            .map(|chunk| f32::from_ne_bytes(*chunk))
            .collect();
        Ok(Array1::from_vec(embed_vec))
    }

    fn encode_embedding(&self, path: &Path, embed: &Array1<f32>) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            !embed.is_empty() && embed.iter().all(|v| v.is_finite()),
            "invalid embedding values"
        );
        // Templates are not portable across architectures with different endianness.
        let plain: Vec<u8> = embed.iter().flat_map(|value| value.to_ne_bytes()).collect();
        match &self.cipher {
            Some(cipher) => cipher.encrypt(&plain, &self.embedding_aad(path)?),
            None => Ok(plain),
        }
    }

    fn write_embedding(&self, path: &Path, embed: &Array1<f32>) -> anyhow::Result<()> {
        let bytes = self.encode_embedding(path, embed)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }

    pub fn load_all(&mut self) -> anyhow::Result<()> {
        self.init_dirs()?;
        let mut users = HashMap::new();
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
                faces.insert(face_name.clone(), self.load_face(&username, &face_name)?);
            }
            users.insert(username, faces);
            #[cfg(test)]
            if self.fail_reload {
                anyhow::bail!("injected directory traversal failure");
            }
        }
        self.users = users;
        Ok(())
    }

    fn load_face(
        &self,
        username: &str,
        face_name: &str,
    ) -> anyhow::Result<HashMap<String, (Array1<f32>, Spectrum)>> {
        let mut embeddings = HashMap::new();
        let mut stale_count = 0;
        let mut stack = vec![self.face_dir(username, face_name)];
        while let Some(dir) = stack.pop() {
            let stale = matches!((&self.recognizer, Self::recognizer_tag(&dir)), (Some(current), Some(tag)) if *current != tag);
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(PRIVATE_PREFIX)
                {
                    continue;
                }
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    stack.push(path);
                } else if kind.is_file() && path.extension().and_then(|e| e.to_str()) == Some("bin")
                {
                    if stale {
                        stale_count += 1;
                        continue;
                    }
                    let embed = match self.read_embedding(&path) {
                        Ok(embed) => embed,
                        Err(error) => {
                            tracing::warn!(path = %path.display(), %error, "ignoring an unreadable template; it will not be matched");
                            continue;
                        }
                    };
                    let stem = path.file_stem().unwrap().to_string_lossy();
                    let (id, spectrum) = if let Some(id) = stem.strip_suffix("_ir") {
                        (id.to_string(), Spectrum::Ir)
                    } else {
                        (
                            stem.strip_suffix("_rgb").unwrap_or(&stem).to_string(),
                            Spectrum::Rgb,
                        )
                    };
                    embeddings.insert(id, (embed, spectrum));
                }
            }
        }
        if stale_count > 0 {
            tracing::warn!(
                username,
                face = face_name,
                stale = stale_count,
                recognizer = self.recognizer.as_deref().unwrap_or_default(),
                "ignoring templates enrolled under a different recognizer; re-enroll this face"
            );
        }
        Ok(embeddings)
    }

    fn refresh_face(&mut self, username: &str, face_name: &str) -> Result<(), UserDbError> {
        match self.load_face(username, face_name) {
            Ok(embeddings) => {
                self.users
                    .entry(username.to_string())
                    .or_default()
                    .insert(face_name.to_string(), embeddings);
                Ok(())
            }
            Err(error) => {
                // Never retain revoked/stale embeddings if reconciliation fails.
                if let Some(faces) = self.users.get_mut(username) {
                    faces.remove(face_name);
                }
                Err(UserDbError::Io(std::io::Error::other(error.to_string())))
            }
        }
    }

    fn collect_bin_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.ensure_available()?;
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
                if file_type.is_dir()
                    && !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(PRIVATE_PREFIX)
                {
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

    /// Startup must never turn unauthenticated input into trusted ciphertext. Only
    /// privileged live config changes or the explicit maintenance command may migrate it.
    pub fn require_bound_templates(&self) -> anyhow::Result<()> {
        for path in self.collect_bin_files()? {
            let bytes = fs::read(&path)?;
            anyhow::ensure!(
                crypto::is_encrypted(&bytes) && !crypto::is_legacy(&bytes),
                "unbound or plaintext templates require explicit migration: stop gazed, verify template ownership, then run sudo gazed --migrate-legacy-templates, or re-enroll"
            );
        }
        Ok(())
    }

    pub fn has_encrypted_templates(&self) -> anyhow::Result<bool> {
        for path in self.collect_bin_files()? {
            if crypto::is_encrypted(&fs::read(&path)?) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn add_template(
        &mut self,
        username: &str,
        face_name: &str,
        template_id: &str,
        embeddings: Vec<(Array1<f32>, Spectrum)>,
    ) -> Result<(), UserDbError> {
        Self::validate_username(username)?;
        Self::validate_face_name(face_name)?;
        Self::validate_template_id(template_id)?;
        if embeddings.is_empty() {
            return Err(UserDbError::InvalidName(
                "an enrollment must contain embeddings".into(),
            ));
        }
        self.init_dirs()?;
        let user_dir = self.user_dir(username);
        let face_dir = self.face_dir(username, face_name);
        Self::ensure_private_dir(&user_dir)?;
        Self::ensure_private_dir(&face_dir)?;
        sync_dir(&self.base_dir)?;
        sync_dir(&user_dir)?;
        let template_dir = face_dir.join(template_id);
        let existing = match fs::symlink_metadata(&template_dir) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => true,
            Ok(_) => {
                return Err(UserDbError::Io(std::io::Error::other(
                    "template path is not a private directory",
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let templates = self.list_template_ids(username, face_name)?;
        let staged = StagingDir::new(&face_dir, ".gaze-stage-")?;
        if let Some(recognizer) = &self.recognizer {
            Self::write_recognizer_tag(&staged.0, recognizer)?;
        }
        for (embed, spectrum) in embeddings {
            let suffix = match spectrum {
                Spectrum::Rgb => "rgb",
                Spectrum::Ir => "ir",
            };
            let file_path = staged
                .0
                .join(format!("{}_{suffix}.bin", uuid::Uuid::new_v4()));
            self.write_embedding(&file_path, &embed)
                .map_err(|err| UserDbError::Io(std::io::Error::other(err.to_string())))?;
        }
        sync_dir(&staged.0)?;
        // An existing ID appends an atomic batch, never replaces a nonempty directory.
        let destination = if existing {
            template_dir.join(format!("batch-{}", uuid::Uuid::new_v4()))
        } else {
            template_dir
        };
        fs::rename(&staged.0, &destination)?;
        let committed = (|| -> Result<(), UserDbError> {
            sync_dir(destination.parent().unwrap())?;
            sync_dir(&face_dir)?;
            if !existing && self.max_templates > 0 {
                let excess = (templates.len() + 1).saturating_sub(self.max_templates);
                for oldest_id in templates.iter().take(excess) {
                    self.remove_template_dir(username, face_name, oldest_id)
                        .map_err(|e| UserDbError::Io(std::io::Error::other(e.to_string())))?;
                }
            }
            Ok(())
        })();
        // Only the affected face is read/decrypted, including after a post-commit error.
        let refresh = self.refresh_face(username, face_name);
        committed.and(refresh)
    }

    pub fn list_template_ids(
        &self,
        username: &str,
        face_name: &str,
    ) -> Result<Vec<String>, UserDbError> {
        self.ensure_available()?;
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

    fn remove_template_dir(
        &self,
        username: &str,
        face_name: &str,
        template_id: &str,
    ) -> anyhow::Result<bool> {
        Self::validate_username(username).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Self::validate_face_name(face_name).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Self::validate_template_id(template_id).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let template_dir = self.face_dir(username, face_name).join(template_id);
        if !template_dir.exists() {
            return Ok(false);
        }
        Self::retire_dir(&template_dir)?;
        Ok(true)
    }

    pub fn remove_face(&mut self, username: &str, face_name: &str) -> Result<(), UserDbError> {
        self.ensure_available()?;
        Self::validate_username(username)?;
        Self::validate_face_name(face_name)?;
        let Some(faces) = self.users.get(username) else {
            return Err(UserDbError::UserNotFound(username.to_string()));
        };
        if !faces.contains_key(face_name) {
            return Err(UserDbError::FaceNotFound(face_name.to_string()));
        }

        let face_dir = self.face_dir(username, face_name);
        let result = if face_dir.exists() {
            Self::retire_dir(&face_dir)
        } else {
            Ok(())
        };
        // A failed sync may follow a successful rename. Invalidate conservatively even
        // on failure; no partial recursive delete can leave cached matches behind.
        if let Some(faces) = self.users.get_mut(username) {
            faces.remove(face_name);
        }
        result.map_err(Into::into)
    }

    pub fn rename_face(
        &mut self,
        username: &str,
        old_face_name: &str,
        new_face_name: &str,
    ) -> Result<(), UserDbError> {
        Self::validate_username(username)?;
        self.ensure_available()?;
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

        if let Err(e) = fs::rename(&old_face_dir, &new_face_dir) {
            faces.insert(old_face_name.to_string(), embeddings);
            return Err(e.into());
        }

        faces.insert(new_face_name.to_string(), embeddings);
        sync_dir(&self.user_dir(username))?;
        Ok(())
    }

    pub fn clear_user(&mut self, username: &str) -> Result<(), UserDbError> {
        self.ensure_available()?;
        Self::validate_username(username)?;
        let user_dir = self.user_dir(username);
        let result = if user_dir.exists() {
            Self::retire_dir(&user_dir)
        } else {
            Ok(())
        };
        self.users.remove(username);
        result.map_err(Into::into)
    }

    pub fn match_faces(
        &self,
        username: &str,
        embed: &ndarray::Array1<f32>,
        threshold: f32,
        spectrum: Spectrum,
    ) -> Result<Vec<FaceScore>, UserDbError> {
        self.ensure_available()?;
        Self::validate_username(username)?;
        let faces = self
            .users
            .get(username)
            .ok_or_else(|| UserDbError::UserNotFound(username.to_string()))?;

        let embed_len = embed.len();
        let mut results: Vec<FaceScore> = Vec::with_capacity(faces.len());
        for (name, uuid_map) in faces {
            let mut best: Option<f32> = None;
            for (ref_embed, spec) in uuid_map.values() {
                if *spec != spectrum || ref_embed.len() != embed_len {
                    continue;
                }

                // Unit-norm embeddings make this dot product cosine similarity, which decides
                // the match. The later percentage is for display, not an identity probability.
                let score = embed.dot(ref_embed);
                best = Some(match best {
                    Some(current)
                        if current
                            .partial_cmp(&score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            == std::cmp::Ordering::Less =>
                    {
                        score
                    }
                    Some(current) => current,
                    None => score,
                });
            }

            let best = best.unwrap_or(0.0);
            // Center at 0.4 with slope 15 so values near the threshold spread out nicely.
            let pct = 100.0 / (1.0 + (-15.0_f32 * (best - 0.4)).exp());
            results.push((
                name.clone(),
                best,
                pct,
                best > threshold,
                uuid_map.len() as u32,
            ));
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }

    pub fn list_faces(
        &self,
        username: &str,
    ) -> Result<Vec<(String, u32, bool, bool)>, UserDbError> {
        self.ensure_available()?;
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

    pub fn has_enrolled_faces(&self, username: &str) -> Result<bool, UserDbError> {
        self.ensure_available()?;
        Self::validate_username(username)?;
        Ok(self
            .users
            .get(username)
            .is_some_and(|faces| faces.values().any(|embeds| !embeds.is_empty())))
    }

    pub fn get_user_embeddings(&self, username: &str) -> Option<Vec<&Array1<f32>>> {
        if self.ensure_available().is_err() || Self::validate_username(username).is_err() {
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
mod tests;
