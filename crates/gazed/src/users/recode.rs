// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

impl UserDatabase {
    pub(super) fn recode_prefix(&self) -> anyhow::Result<String> {
        let name = self
            .base_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("store must have a directory name"))?;
        Ok(format!(".gaze-recode-{name}-"))
    }

    pub(super) fn cleanup_abandoned_staging(&self) -> anyhow::Result<()> {
        // A directory exchange leaves either the unpublished new store or the old store
        // at this sibling path. Neither is authoritative; remove it after a restart.
        let parent = self
            .base_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("store has no parent"))?;
        let prefix = self.recode_prefix()?;
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(suffix) = name.strip_prefix(&prefix)
                && uuid::Uuid::parse_str(suffix).is_ok()
                && entry.file_type()?.is_dir()
            {
                Self::remove_private_dir_all(&entry.path())?;
                sync_dir(parent)?;
            }
        }
        let mut stack = vec![self.base_dir.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if Self::is_abandoned_name(&name) {
                    Self::remove_private_dir_all(&entry.path())?;
                    sync_dir(&dir)?;
                } else if !name.starts_with(PRIVATE_PREFIX) {
                    stack.push(entry.path());
                }
            }
        }
        Ok(())
    }

    pub(super) fn recode_all_templates(
        &self,
        recode: impl Fn(&Path, &[u8]) -> anyhow::Result<Option<Vec<u8>>>,
    ) -> anyhow::Result<usize> {
        self.recode_all_templates_and_then(recode, || Ok(()))
    }

    pub(super) fn recode_all_templates_and_then(
        &self,
        recode: impl Fn(&Path, &[u8]) -> anyhow::Result<Option<Vec<u8>>>,
        persist: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<usize> {
        self.ensure_available()?;
        // Validate before staging. An already-current store needs no writes or directory swap.
        let mut conversions = HashMap::new();
        for path in self.collect_bin_files()? {
            if let Some(bytes) = recode(&path, &fs::read(&path)?)? {
                conversions.insert(path, bytes);
            }
        }
        let changed = conversions.len();
        if changed == 0 {
            persist()?;
            return Ok(0);
        }
        let parent = self
            .base_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("store has no parent"))?;
        let staged = StagingDir::new(parent, &self.recode_prefix()?)?;
        let mut stack = vec![(self.base_dir.clone(), staged.0.clone())];
        let mut directories = Vec::new();
        while let Some((source, target)) = stack.pop() {
            directories.push(target.clone());
            for entry in fs::read_dir(&source)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if Self::is_abandoned_name(&name) && entry.file_type()?.is_dir() {
                    continue;
                }
                anyhow::ensure!(
                    !name.starts_with(PRIVATE_PREFIX),
                    "reserved entry in template store: {}; rename or remove it before migration",
                    entry.path().display()
                );
                let path = entry.path();
                let destination = target.join(entry.file_name());
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    fs::DirBuilder::new().mode(0o700).create(&destination)?;
                    stack.push((path, destination));
                } else if kind.is_file() {
                    let bytes = match conversions.remove(&path) {
                        Some(bytes) => bytes,
                        None => fs::read(&path)?,
                    };
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(destination)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                } else {
                    anyhow::bail!("non-regular entry in template store: {}", path.display());
                }
            }
        }
        anyhow::ensure!(
            conversions.is_empty(),
            "template store changed during migration"
        );
        for dir in directories.iter().rev() {
            sync_dir(dir)?;
        }
        sync_dir(parent)?;
        exchange_dirs(&self.base_dir, &staged.0)?;
        // Keep the old store until both the new store and its config have committed.
        let commit = sync_dir(parent)
            .map_err(anyhow::Error::from)
            .and_then(|_| persist());
        if let Err(error) = commit {
            let rollback = self
                .rollback_recode(&staged.0)
                .and_then(|_| sync_dir(parent));
            if let Err(rollback_error) = rollback {
                self.unavailable.set(true);
                tracing::error!(backup = %staged.0.display(), %error, %rollback_error,
                    "storage rollback failed; matching and mutations disabled until restart");
                // Preserve both complete copies. Never delete the backup on rollback failure.
                std::mem::forget(staged);
                return Err(error.context(format!(
                    "storage rollback failed: {rollback_error}; restart gazed"
                )));
            }
            return Err(error);
        }
        drop(staged);
        // Cleanup cannot turn a durable successful commit into a reported failed conversion.
        if let Err(error) = sync_dir(parent) {
            tracing::warn!(%error, "could not sync retired-store cleanup; startup will retry");
        }
        Ok(changed)
    }

    pub(super) fn rollback_recode(&self, staged: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_rollback {
            return Err(std::io::Error::other("injected rollback failure"));
        }
        exchange_dirs(&self.base_dir, staged)
    }

    /// Change disk format, saved configuration and the live cipher as one operation.
    /// The caller holds the DB lock; a failed config write restores the exact old store.
    pub fn change_encryption(
        &mut self,
        cipher: EmbeddingCipher,
        enabled: bool,
        persist: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<usize> {
        let count = self.recode_all_templates_and_then(
            |path, raw| self.recode_embedding(&cipher, enabled, false, path, raw),
            persist,
        )?;
        self.cipher = enabled.then_some(cipher);
        Ok(count)
    }

    /// Prepare recognizer filtering before saving settings. A rejected update restores
    /// the previous snapshot without another fallible disk read.
    pub fn apply_settings(
        &mut self,
        pending_cipher: Option<EmbeddingCipher>,
        encrypt: bool,
        recognizer: Option<&str>,
        max_templates: usize,
        persist: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<usize> {
        self.ensure_available()?;
        anyhow::ensure!(
            pending_cipher.is_some() || encrypt == self.is_encrypted(),
            "encryption change requires a TPM cipher"
        );
        let previous_recognizer = self.recognizer.clone();
        let previous_users = if let Some(recognizer) = recognizer {
            let previous = std::mem::take(&mut self.users);
            if let Err(error) = self.use_recognizer(recognizer) {
                self.users = previous;
                return Err(error);
            }
            Some(previous)
        } else {
            None
        };
        let result = match pending_cipher {
            Some(cipher) => self.change_encryption(cipher, encrypt, persist),
            None => persist().map(|_| 0),
        };
        if result.is_err() {
            self.recognizer = previous_recognizer;
            if let Some(users) = previous_users {
                self.users = users;
            }
        } else {
            self.max_templates = max_templates;
        }
        result
    }

    pub(super) fn recode_embedding(
        &self,
        cipher: &EmbeddingCipher,
        encrypt: bool,
        allow_legacy: bool,
        path: &Path,
        raw: &[u8],
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let aad = self.embedding_aad(path)?;
        if crypto::is_legacy(raw) {
            anyhow::ensure!(
                allow_legacy,
                "legacy unbound templates require explicit migration: stop gazed, then run sudo gazed --migrate-legacy-templates, or re-enroll"
            );
            let plain = cipher.decrypt_legacy(raw)?;
            return if encrypt {
                cipher.encrypt(&plain, &aad).map(Some)
            } else {
                Ok(Some(plain))
            };
        }
        if crypto::is_encrypted(raw) {
            let plain = cipher.decrypt(raw, &aad)?;
            return if encrypt { Ok(None) } else { Ok(Some(plain)) };
        }
        if encrypt {
            cipher.encrypt(raw, &aad).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn migrate_plaintext_to_encrypted(&self) -> anyhow::Result<usize> {
        let Some(cipher) = self.cipher.as_ref() else {
            return Ok(0);
        };
        self.encrypt_all_with(cipher)
    }

    pub fn encrypt_all_with(&self, cipher: &EmbeddingCipher) -> anyhow::Result<usize> {
        self.encrypt_all_with_legacy(cipher, false)
    }

    /// Maintenance-only opt-in: the operator must trust the legacy directory ownership.
    pub fn migrate_legacy_with(&self, cipher: &EmbeddingCipher) -> anyhow::Result<usize> {
        self.encrypt_all_with_legacy(cipher, true)
    }

    pub(super) fn encrypt_all_with_legacy(
        &self,
        cipher: &EmbeddingCipher,
        allow_legacy: bool,
    ) -> anyhow::Result<usize> {
        self.recode_all_templates(|path, raw| {
            self.recode_embedding(cipher, true, allow_legacy, path, raw)
        })
    }

    pub fn decrypt_all_with(&self, cipher: &EmbeddingCipher) -> anyhow::Result<usize> {
        self.recode_all_templates(|path, raw| {
            self.recode_embedding(cipher, false, false, path, raw)
        })
    }
}
