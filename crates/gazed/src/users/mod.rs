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
const FORMAT_FILE: &str = ".gaze-store.format";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreFormat {
    Plain,
    Encrypted,
}

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

    fn format_path(base_dir: &Path) -> anyhow::Result<PathBuf> {
        let parent = base_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("store has no parent"))?;
        Ok(parent.join(FORMAT_FILE))
    }

    pub fn recorded_format(base_dir: &Path) -> anyhow::Result<Option<StoreFormat>> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(Self::format_path(base_dir)?)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut file, &mut contents)?;
        Ok(Some(match contents.trim() {
            "plain" => StoreFormat::Plain,
            _ => StoreFormat::Encrypted,
        }))
    }

    pub fn record_format(base_dir: &Path, format: StoreFormat) -> anyhow::Result<()> {
        let path = Self::format_path(base_dir)?;
        if Self::recorded_format(base_dir)? == Some(format) {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("store has no parent"))?;
        let tmp = parent.join(format!("{FORMAT_FILE}.{}.tmp", uuid::Uuid::new_v4()));
        let contents: &[u8] = match format {
            StoreFormat::Plain => b"plain\n",
            StoreFormat::Encrypted => b"encrypted\n",
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        if let Err(error) = file.write_all(contents).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
        if let Err(error) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
        sync_dir(parent)?;
        Ok(())
    }

    pub fn record_store_format(&self, format: StoreFormat) -> anyhow::Result<()> {
        Self::record_format(&self.base_dir, format)
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

    pub fn next_template_id(
        &self,
        username: &str,
        face_name: &str,
        now_secs: u64,
    ) -> Result<String, UserDbError> {
        let newest = self
            .list_template_ids(username, face_name)?
            .iter()
            .filter_map(|id| id.parse::<u64>().ok())
            .max();
        let next = newest.map_or(now_secs, |newest| now_secs.max(newest.saturating_add(1)));
        Ok(next.to_string())
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
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub(super) struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub(super) fn new(name: &str) -> Self {
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

        pub(super) fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    pub(super) fn embedding(values: &[f32]) -> Array1<f32> {
        Array1::from_vec(values.to_vec())
    }

    pub(super) fn rgb_embeds(embeds: Vec<Array1<f32>>) -> Vec<(Array1<f32>, Spectrum)> {
        embeds.into_iter().map(|e| (e, Spectrum::Rgb)).collect()
    }

    fn sorted_faces(db: &UserDatabase, username: &str) -> Vec<(String, u32, bool, bool)> {
        let mut faces = db.list_faces(username).unwrap();
        faces.sort_by(|a, b| a.0.cmp(&b.0));
        faces
    }

    #[test]
    fn daemon_and_maintenance_cannot_lock_the_same_store() {
        let temp = TempDir::new("store-lock");
        let base = temp.path().join("users");
        let lock = UserDatabase::lock_store(&base).unwrap();
        assert!(UserDatabase::lock_store(&base).is_err());
        drop(lock);
        assert!(UserDatabase::lock_store(&base).is_ok());
    }

    #[test]
    fn store_format_round_trips_and_unknown_records_read_as_encrypted() {
        let temp = TempDir::new("store-format");
        let base = temp.path().join("users");
        assert_eq!(UserDatabase::recorded_format(&base).unwrap(), None);
        UserDatabase::record_format(&base, StoreFormat::Plain).unwrap();
        assert_eq!(
            UserDatabase::recorded_format(&base).unwrap(),
            Some(StoreFormat::Plain)
        );
        UserDatabase::record_format(&base, StoreFormat::Encrypted).unwrap();
        assert_eq!(
            UserDatabase::recorded_format(&base).unwrap(),
            Some(StoreFormat::Encrypted)
        );
        fs::write(temp.path().join(FORMAT_FILE), "unknown\n").unwrap();
        assert_eq!(
            UserDatabase::recorded_format(&base).unwrap(),
            Some(StoreFormat::Encrypted)
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn encrypted_templates_reject_owner_and_spectrum_substitution() {
        let temp = TempDir::new("identity-binding");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();
        fs::rename(temp.path().join("alice"), temp.path().join("bob")).unwrap();
        db.load_all().unwrap();
        assert!(!db.has_enrolled_faces("bob").unwrap());
        fs::rename(temp.path().join("bob"), temp.path().join("alice")).unwrap();
        let file = db.collect_bin_files().unwrap().pop().unwrap();
        let renamed = file.with_file_name(
            file.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .replace("_rgb.bin", "_ir.bin"),
        );
        fs::rename(file, renamed).unwrap();
        db.load_all().unwrap();
        assert!(!db.has_enrolled_faces("alice").unwrap());
    }

    #[test]
    fn encrypted_face_rename_preserves_authenticated_templates() {
        let temp = TempDir::new("bound-rename");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        db.rename_face("alice", "work", "office").unwrap();
        db.load_all().unwrap();
        assert!(db.has_enrolled_faces("alice").unwrap());
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("office".into(), 1, true, false)]
        );
    }

    #[test]
    fn failed_enrollment_preserves_previous_disk_and_cached_templates() {
        let temp = TempDir::new("stage-failure");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 1).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        // The first staged embedding succeeds; the next one fails validation.
        assert!(
            db.add_template(
                "alice",
                "work",
                "2",
                rgb_embeds(vec![embedding(&[0.5]), embedding(&[])])
            )
            .is_err()
        );
        assert_eq!(db.list_template_ids("alice", "work").unwrap(), vec!["1"]);
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 1);
        db.load_all().unwrap();
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(temp.path().join("alice/work"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn traversal_failure_and_recognizer_failure_preserve_the_snapshot() {
        let temp = TempDir::new("reload-transaction");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.use_recognizer("old").unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        db.fail_reload = true;
        assert!(db.load_all().is_err());
        assert!(db.use_recognizer("new").is_err());
        assert_eq!(db.recognizer.as_deref(), Some("old"));
        assert!(db.has_enrolled_faces("alice").unwrap());
        // Enrollment must not invoke the failing full-store reload.
        db.add_template("alice", "work", "2", rgb_embeds(vec![embedding(&[0.5])]))
            .unwrap();
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 2);
    }

    #[test]
    fn abandoned_staging_and_retired_templates_never_load_or_count_toward_the_cap() {
        let temp = TempDir::new("abandoned");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new(base, 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        let file = db.collect_bin_files().unwrap().pop().unwrap();
        for prefix in [".gaze-stage-", ".gaze-deleted-"] {
            let dir = temp
                .path()
                .join("alice/work")
                .join(format!("{prefix}{}", uuid::Uuid::new_v4()));
            fs::create_dir(&dir).unwrap();
            fs::copy(&file, dir.join("stray_rgb.bin")).unwrap();
        }
        db.load_all().unwrap();
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 1);
        assert_eq!(db.list_template_ids("alice", "work").unwrap(), vec!["1"]);
        let db = UserDatabase::new(base, 4).unwrap();
        assert_eq!(db.collect_bin_files().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(temp.path().join("alice/work"))
                .unwrap()
                .count(),
            1
        );
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
    fn has_enrolled_faces_is_false_for_missing_or_cleared_users() {
        let temp = TempDir::new("has-enrolled");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();

        assert!(!db.has_enrolled_faces("alice").unwrap());
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        assert!(db.has_enrolled_faces("alice").unwrap());
        db.clear_user("alice").unwrap();
        assert!(!db.has_enrolled_faces("alice").unwrap());
        assert!(db.has_enrolled_faces("../alice").is_err());
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

    #[test]
    fn ir_probe_does_not_cross_match_an_rgb_only_face() {
        let temp = TempDir::new("no-cross-spectrum");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 3).unwrap();
        db.add_template(
            "alice",
            "rgb-only",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();

        let results = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 0.5, Spectrum::Ir)
            .unwrap();

        assert_eq!(
            results[0].1, 0.0,
            "IR probe must not score against RGB templates"
        );
        assert!(!results[0].3);
    }

    pub(super) fn test_cipher() -> EmbeddingCipher {
        EmbeddingCipher::new(&[42u8; crypto::KEY_LEN])
    }

    pub(super) fn all_bins_encrypted(db: &UserDatabase) -> bool {
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
    fn planted_plaintext_is_rejected_in_an_encrypted_store() {
        let temp = TempDir::new("enc-planted");
        let base = temp.path().to_str().unwrap();

        let mut db = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();
        assert!(all_bins_encrypted(&db));

        let planted = db.face_dir("alice", "work").join("planted_rgb.bin");
        let bytes: Vec<u8> = [9.0f32, 9.0f32]
            .iter()
            .flat_map(|f| f.to_ne_bytes())
            .collect();
        fs::write(&planted, &bytes).unwrap();

        db.load_all().unwrap();
        assert_eq!(
            db.get_user_embeddings("alice").unwrap().len(),
            1,
            "a planted plaintext template must not load once encryption is enabled"
        );
        assert!(db.has_encrypted_templates().unwrap());
    }

    fn mode_of(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn validate_rejects_names_that_would_escape_the_store() {
        for name in ["../etc", "..", ".", "/", "a\tb", "\u{7f}", " ", "\n"] {
            assert!(
                UserDatabase::validate_username(name).is_err(),
                "{name:?} should be rejected"
            );
        }
        // Dot-prefixed names are ordinary components, not traversal.
        UserDatabase::validate_username(".hidden").unwrap();
        UserDatabase::validate_face_name("..hidden").unwrap();
    }

    #[test]
    fn an_invalid_name_is_refused_before_anything_touches_the_disk() {
        let temp = TempDir::new("reject-before-write");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();

        assert!(matches!(
            db.add_template("../alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])])),
            Err(UserDbError::InvalidName(_))
        ));
        assert!(matches!(
            db.add_template("alice", "a/b", "1", rgb_embeds(vec![embedding(&[1.0])])),
            Err(UserDbError::InvalidName(_))
        ));
        assert!(matches!(
            db.add_template("alice", "work", "../1", rgb_embeds(vec![embedding(&[1.0])])),
            Err(UserDbError::InvalidName(_))
        ));
        assert!(!temp.path().join("alice").exists());
    }

    #[test]
    fn the_store_and_everything_in_it_is_owner_only() {
        let temp = TempDir::new("permissions");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new(base, 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        assert_eq!(mode_of(temp.path()), 0o700);
        assert_eq!(mode_of(&temp.path().join("alice")), 0o700);
        assert_eq!(mode_of(&temp.path().join("alice/work")), 0o700);
        assert_eq!(mode_of(&temp.path().join("alice/work/1")), 0o700);

        for bin in db.collect_bin_files().unwrap() {
            assert_eq!(
                mode_of(&bin),
                0o600,
                "{} is readable by others",
                bin.display()
            );
        }
    }

    #[test]
    fn a_clock_running_behind_still_evicts_the_oldest_enrolment() {
        let temp = TempDir::new("clock-skew");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        for (now_secs, value) in [(2_000, 1.0), (3_000, 2.0), (500, 3.0)] {
            let id = db.next_template_id("alice", "work", now_secs).unwrap();
            db.add_template("alice", "work", &id, rgb_embeds(vec![embedding(&[value])]))
                .unwrap();
        }

        assert_eq!(
            db.list_template_ids("alice", "work").unwrap(),
            vec!["3000", "3001"]
        );
    }

    #[test]
    fn next_template_id_follows_the_clock_when_it_is_ahead() {
        let temp = TempDir::new("clock-ahead");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 0).unwrap();
        assert_eq!(db.next_template_id("alice", "work", 0).unwrap(), "0");
        db.add_template("alice", "work", "10", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        assert_eq!(db.next_template_id("alice", "work", 10).unwrap(), "11");
        assert_eq!(db.next_template_id("alice", "work", 50).unwrap(), "50");
    }

    #[test]
    fn template_ids_are_ordered_numerically_not_lexically() {
        let temp = TempDir::new("numeric-order");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 0).unwrap();
        for id in ["10", "9", "2", "100"] {
            db.add_template("alice", "work", id, rgb_embeds(vec![embedding(&[1.0])]))
                .unwrap();
        }

        assert_eq!(
            db.list_template_ids("alice", "work").unwrap(),
            vec!["2", "9", "10", "100"]
        );
    }

    #[test]
    fn a_max_of_zero_never_evicts() {
        let temp = TempDir::new("unbounded");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 0).unwrap();
        for id in ["1", "2", "3", "4", "5"] {
            db.add_template("alice", "work", id, rgb_embeds(vec![embedding(&[1.0])]))
                .unwrap();
        }

        assert_eq!(db.list_template_ids("alice", "work").unwrap().len(), 5);
    }

    #[test]
    fn re_adding_an_existing_template_id_does_not_evict_its_siblings() {
        let temp = TempDir::new("no-evict-on-update");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        db.add_template("alice", "work", "2", rgb_embeds(vec![embedding(&[0.5])]))
            .unwrap();

        db.add_template("alice", "work", "2", rgb_embeds(vec![embedding(&[0.25])]))
            .unwrap();

        assert_eq!(
            db.list_template_ids("alice", "work").unwrap(),
            vec!["1", "2"],
            "an update must not push the oldest template out"
        );
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 3);
    }

    #[test]
    fn lowering_the_template_cap_takes_effect_on_the_next_enrolment() {
        let temp = TempDir::new("set-max");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        for id in ["1", "2", "3"] {
            db.add_template("alice", "work", id, rgb_embeds(vec![embedding(&[1.0])]))
                .unwrap();
        }

        db.set_max_templates(2);
        db.add_template("alice", "work", "4", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        assert_eq!(
            db.list_template_ids("alice", "work").unwrap(),
            vec!["3", "4"]
        );
    }

    #[test]
    fn listing_templates_for_an_unknown_face_is_empty_rather_than_an_error() {
        let temp = TempDir::new("list-missing");
        let db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();

        assert!(db.list_template_ids("alice", "work").unwrap().is_empty());
        assert!(matches!(
            db.list_template_ids("../alice", "work"),
            Err(UserDbError::InvalidName(_))
        ));
    }

    #[test]
    fn renaming_a_face_to_its_own_name_is_a_no_op() {
        let temp = TempDir::new("rename-self");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        db.rename_face("alice", "work", "work").unwrap();

        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 1, true, false)]
        );
        assert!(temp.path().join("alice/work").exists());
    }

    #[test]
    fn a_failed_rename_leaves_the_original_face_in_place() {
        let temp = TempDir::new("rename-rollback");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        db.add_template("alice", "home", "1", rgb_embeds(vec![embedding(&[0.5])]))
            .unwrap();

        assert!(matches!(
            db.rename_face("alice", "work", "home"),
            Err(UserDbError::FaceExists(_))
        ));

        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![
                ("home".to_string(), 1, true, false),
                ("work".to_string(), 1, true, false)
            ],
            "the rejected rename must not drop the face it was moving"
        );
        assert!(temp.path().join("alice/work").exists());
    }

    #[test]
    fn a_face_directory_left_on_disk_blocks_a_rename_onto_it() {
        let temp = TempDir::new("rename-onto-stray");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        // A directory with no templates never loads into memory, but it still owns the name.
        fs::create_dir(temp.path().join("alice/office")).unwrap();

        assert!(matches!(
            db.rename_face("alice", "work", "office"),
            Err(UserDbError::FaceExists(face)) if face == "office"
        ));
        assert!(temp.path().join("alice/work").exists());
    }

    #[test]
    fn renaming_reports_which_half_of_the_pair_was_missing() {
        let temp = TempDir::new("rename-missing");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        assert!(matches!(
            db.rename_face("bob", "work", "office"),
            Err(UserDbError::UserNotFound(user)) if user == "bob"
        ));
        assert!(matches!(
            db.rename_face("alice", "holiday", "office"),
            Err(UserDbError::FaceNotFound(face)) if face == "holiday"
        ));
        assert!(matches!(
            db.rename_face("alice", "work", "../escape"),
            Err(UserDbError::InvalidName(_))
        ));
    }

    #[test]
    fn removing_an_unknown_user_or_face_is_reported_not_ignored() {
        let temp = TempDir::new("remove-missing");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        assert!(matches!(
            db.remove_face("bob", "work"),
            Err(UserDbError::UserNotFound(_))
        ));
        assert!(matches!(
            db.remove_face("alice", "holiday"),
            Err(UserDbError::FaceNotFound(_))
        ));
    }

    #[test]
    fn clearing_a_user_who_was_never_enrolled_succeeds() {
        let temp = TempDir::new("clear-missing");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 2).unwrap();

        db.clear_user("bob").unwrap();

        assert!(matches!(
            db.clear_user("../bob"),
            Err(UserDbError::InvalidName(_))
        ));
    }

    #[test]
    fn a_reload_walks_nested_template_directories() {
        let temp = TempDir::new("nested");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();
        let nested = temp.path().join("alice/work/1/extra");
        fs::create_dir(&nested).unwrap();
        fs::copy(
            db.collect_bin_files().unwrap()[0].as_path(),
            nested.join("deep_rgb.bin"),
        )
        .unwrap();

        db.load_all().unwrap();

        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 2);
    }

    #[test]
    fn a_reload_reads_the_spectrum_out_of_the_file_name() {
        let temp = TempDir::new("spectrum-suffix");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            vec![
                (embedding(&[1.0, 0.0]), Spectrum::Rgb),
                (embedding(&[0.0, 1.0]), Spectrum::Ir),
            ],
        )
        .unwrap();

        let db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();

        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 2, true, true)]
        );
    }

    #[test]
    fn a_template_with_no_spectrum_suffix_is_treated_as_rgb() {
        let temp = TempDir::new("legacy-suffix");
        let base = temp.path();
        let template = base.join("alice/work/1");
        fs::create_dir_all(&template).unwrap();
        fs::write(template.join("legacy.bin"), 1.0f32.to_ne_bytes()).unwrap();

        let db = UserDatabase::new(base.to_str().unwrap(), 4).unwrap();

        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 1, true, false)],
            "templates enrolled before IR support must still load as RGB"
        );
    }

    #[test]
    fn a_reload_ignores_files_that_are_not_templates() {
        let temp = TempDir::new("non-templates");
        let base = temp.path();
        let template = base.join("alice/work/1");
        fs::create_dir_all(&template).unwrap();
        fs::write(template.join("good_rgb.bin"), 1.0f32.to_ne_bytes()).unwrap();
        fs::write(template.join("notes.txt"), b"ignore me").unwrap();
        fs::write(template.join("no-extension"), b"ignore me too").unwrap();

        let db = UserDatabase::new(base.to_str().unwrap(), 4).unwrap();

        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 1);
    }

    #[test]
    fn a_corrupt_template_is_skipped_instead_of_failing_the_whole_load() {
        let temp = TempDir::new("corrupt");
        let base = temp.path();
        let template = base.join("alice/work/1");
        fs::create_dir_all(&template).unwrap();
        fs::write(template.join("good_rgb.bin"), 1.0f32.to_ne_bytes()).unwrap();
        // The byte length is not a whole number of `f32` values.
        fs::write(template.join("truncated_rgb.bin"), [0u8, 1, 2]).unwrap();
        fs::write(template.join("empty_rgb.bin"), b"").unwrap();

        let db = UserDatabase::new(base.to_str().unwrap(), 4).unwrap();

        assert_eq!(
            db.get_user_embeddings("alice").unwrap().len(),
            1,
            "one unreadable file must not hide the templates beside it"
        );
    }

    #[test]
    fn a_reload_walks_past_entries_that_are_not_usable_user_directories() {
        let temp = TempDir::new("unsafe-dirs");
        let base = temp.path();
        let template = base.join(".hidden-user/ok-face/1");
        fs::create_dir_all(&template).unwrap();
        fs::write(template.join("a_rgb.bin"), 1.0f32.to_ne_bytes()).unwrap();
        // Include a directory with an invalid user name and a stray file at the top level.
        let unsafe_template = base.join(" spaced/face/1");
        fs::create_dir_all(&unsafe_template).unwrap();
        fs::write(unsafe_template.join("b_rgb.bin"), 1.0f32.to_ne_bytes()).unwrap();
        fs::write(base.join("loose.bin"), 1.0f32.to_ne_bytes()).unwrap();

        let db = UserDatabase::new(base.to_str().unwrap(), 4).unwrap();

        assert_eq!(
            db.get_user_embeddings(".hidden-user").unwrap().len(),
            1,
            "a leading dot is an ordinary name, not traversal"
        );
        assert!(
            !db.users.contains_key(" spaced"),
            "a directory whose name fails validation must not become a user"
        );
        assert!(
            db.get_user_embeddings("loose.bin").is_none(),
            "a stray file at the top level is not a user"
        );
    }

    #[test]
    fn embeddings_are_only_offered_for_users_that_exist() {
        let temp = TempDir::new("embeddings-lookup");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        assert!(db.get_user_embeddings("bob").is_none());
        assert!(db.get_user_embeddings("../alice").is_none());
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 1);
    }

    #[test]
    fn matching_against_an_unknown_user_is_an_error_not_an_empty_list() {
        let temp = TempDir::new("match-unknown");
        let db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();

        assert!(matches!(
            db.match_faces("bob", &embedding(&[1.0]), 0.5, Spectrum::Rgb),
            Err(UserDbError::UserNotFound(user)) if user == "bob"
        ));
        assert!(matches!(
            db.match_faces("../bob", &embedding(&[1.0]), 0.5, Spectrum::Rgb),
            Err(UserDbError::InvalidName(_))
        ));
    }

    #[test]
    fn the_reported_confidence_is_centred_on_the_default_threshold() {
        let temp = TempDir::new("confidence");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[0.4, 0.0])]),
        )
        .unwrap();

        let results = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 0.5, Spectrum::Rgb)
            .unwrap();

        assert!(
            (results[0].2 - 50.0).abs() < 0.01,
            "a score of 0.4 should read as 50%: {}",
            results[0].2
        );
    }

    #[test]
    fn a_face_with_no_usable_template_scores_zero_rather_than_matching() {
        let temp = TempDir::new("empty-face");
        let base = temp.path();
        fs::create_dir_all(base.join("alice/work")).unwrap();

        let db = UserDatabase::new(base.to_str().unwrap(), 4).unwrap();
        let results = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), -1.0, Spectrum::Rgb)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, 0.0);
        assert_eq!(results[0].4, 0);
        assert!(
            results[0].3,
            "the caller's threshold still decides, even for an empty face"
        );
    }

    #[test]
    fn matching_keeps_the_best_template_not_the_last_one_read() {
        let temp = TempDir::new("best-of");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![
                embedding(&[0.1, 0.0]),
                embedding(&[1.0, 0.0]),
                embedding(&[0.2, 0.0]),
            ]),
        )
        .unwrap();

        let results = db
            .match_faces("alice", &embedding(&[1.0, 0.0]), 0.5, Spectrum::Rgb)
            .unwrap();

        assert_eq!(results[0].1, 1.0);
        assert_eq!(results[0].4, 3, "the template count covers both spectra");
    }

    #[test]
    fn listing_faces_for_an_unknown_user_is_an_error() {
        let temp = TempDir::new("list-faces-unknown");
        let db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();

        assert!(matches!(
            db.list_faces("bob"),
            Err(UserDbError::UserNotFound(_))
        ));
        assert!(matches!(
            db.list_faces("a/b"),
            Err(UserDbError::InvalidName(_))
        ));
    }

    #[test]
    fn a_plaintext_store_reports_no_encrypted_templates() {
        let temp = TempDir::new("no-encrypted");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();

        assert!(!db.has_encrypted_templates().unwrap());
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        assert!(!db.has_encrypted_templates().unwrap());
        assert!(!db.is_encrypted());
    }

    #[test]
    fn a_single_encrypted_template_is_enough_to_report_an_encrypted_store() {
        let temp = TempDir::new("mixed-encrypted");
        let base = temp.path().to_str().unwrap();

        let mut plain = UserDatabase::new(base, 4).unwrap();
        plain
            .add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        let mut enc = UserDatabase::new(base, 4).unwrap();
        enc.set_cipher(Some(test_cipher()));
        assert!(enc.is_encrypted());
        enc.add_template("alice", "work", "2", rgb_embeds(vec![embedding(&[0.5])]))
            .unwrap();

        assert!(enc.has_encrypted_templates().unwrap());
    }

    #[test]
    fn templates_from_another_recognizer_are_not_matched() {
        let temp = TempDir::new("recognizer-switch");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new(base, 4).unwrap();
        db.use_recognizer("w600k_mbf.onnx").unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            rgb_embeds(vec![embedding(&[1.0, 0.0])]),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(temp.path().join("alice/work/1").join(RECOGNIZER_TAG)).unwrap(),
            "w600k_mbf.onnx"
        );

        db.use_recognizer("w600k_r50.onnx").unwrap();
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 0, false, false)]
        );
        assert!(!db.has_enrolled_faces("alice").unwrap());
        assert!(
            db.match_faces("alice", &embedding(&[1.0, 0.0]), 0.5, Spectrum::Rgb)
                .unwrap()
                .iter()
                .all(|score| !score.3)
        );

        db.add_template(
            "alice",
            "work",
            "2",
            rgb_embeds(vec![embedding(&[0.0, 1.0])]),
        )
        .unwrap();
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 1, true, false)]
        );

        db.use_recognizer("w600k_mbf.onnx").unwrap();
        assert_eq!(
            sorted_faces(&db, "alice"),
            vec![("work".to_string(), 1, true, false)]
        );
    }

    #[test]
    fn untagged_templates_are_adopted_by_the_starting_recognizer() {
        let temp = TempDir::new("recognizer-adopt");
        let base = temp.path().to_str().unwrap();
        let mut legacy = UserDatabase::new(base, 4).unwrap();
        legacy
            .add_template(
                "alice",
                "work",
                "1",
                rgb_embeds(vec![embedding(&[1.0, 0.0])]),
            )
            .unwrap();
        assert!(
            !temp
                .path()
                .join("alice/work/1")
                .join(RECOGNIZER_TAG)
                .exists()
        );

        let mut db = UserDatabase::new(base, 4).unwrap();
        assert_eq!(db.adopt_untagged_templates("w600k_mbf.onnx").unwrap(), 1);
        assert_eq!(db.adopt_untagged_templates("w600k_r50.onnx").unwrap(), 0);
        db.use_recognizer("w600k_mbf.onnx").unwrap();
        assert!(db.has_enrolled_faces("alice").unwrap());

        db.use_recognizer("w600k_r50.onnx").unwrap();
        assert!(!db.has_enrolled_faces("alice").unwrap());
    }
}
