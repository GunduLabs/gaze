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
            if error
                .downcast_ref::<gaze_core::config::ConfigCommitUncertain>()
                .is_some()
            {
                self.unavailable.set(true);
                tracing::error!(backup = %staged.0.display(), %error,
                    "config durability is uncertain; preserving both stores and disabling authentication until restart");
                std::mem::forget(staged);
                return Err(error);
            }
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
    /// Pre-commit errors restore the old store; uncertain commits disable use until restart.
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
        if result.as_ref().is_err_and(|error| {
            error
                .downcast_ref::<gaze_core::config::ConfigCommitUncertain>()
                .is_some()
        }) {
            self.unavailable.set(true);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::tests::{TempDir, all_bins_encrypted, embedding, rgb_embeds, test_cipher};

    #[test]
    fn uncertain_config_commit_preserves_both_stores_and_disables_matching() {
        let temp = TempDir::new("config-uncertain");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        let file = db.collect_bin_files().unwrap().pop().unwrap();
        let relative = file.strip_prefix(temp.path()).unwrap().to_path_buf();
        let result = db.change_encryption(test_cipher(), true, || {
            Err(
                gaze_core::config::ConfigCommitUncertain(std::io::Error::other(
                    "injected directory sync failure",
                ))
                .into(),
            )
        });
        assert!(result.is_err());
        assert!(db.has_enrolled_faces("alice").is_err());
        assert!(
            db.match_faces("alice", &embedding(&[1.0]), 0.5, Spectrum::Rgb)
                .is_err()
        );
        assert!(crypto::is_encrypted(&fs::read(&file).unwrap()));
        let prefix = db.recode_prefix().unwrap();
        let backup = fs::read_dir(temp.path().parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .expect("the old store must survive an uncertain commit");
        assert!(!crypto::is_encrypted(
            &fs::read(backup.path().join(relative)).unwrap()
        ));
        fs::remove_dir_all(backup.path()).unwrap();
    }

    #[test]
    fn uncertain_plain_config_commit_also_disables_matching() {
        let temp = TempDir::new("plain-config-uncertain");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        let result = db.apply_settings(None, false, None, 4, || {
            Err(
                gaze_core::config::ConfigCommitUncertain(std::io::Error::other(
                    "injected directory sync failure",
                ))
                .into(),
            )
        });
        assert!(result.is_err());
        assert!(db.has_enrolled_faces("alice").is_err());
    }

    #[test]
    fn failed_config_save_restores_the_exact_store_and_live_cipher() {
        let temp = TempDir::new("config-rollback");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        let file = db.collect_bin_files().unwrap().pop().unwrap();
        let plaintext = fs::read(&file).unwrap();
        assert!(
            db.change_encryption(test_cipher(), true, || anyhow::bail!("config read-only"))
                .is_err()
        );
        assert_eq!(fs::read(&file).unwrap(), plaintext);
        assert!(!db.is_encrypted());
        assert!(db.has_enrolled_faces("alice").unwrap());
        db.change_encryption(test_cipher(), true, || Ok(()))
            .unwrap();
        let ciphertext = fs::read(&file).unwrap();
        assert!(crypto::is_encrypted(&ciphertext));
        assert!(
            db.change_encryption(test_cipher(), false, || anyhow::bail!("config read-only"))
                .is_err()
        );
        assert_eq!(fs::read(&file).unwrap(), ciphertext);
        assert!(db.is_encrypted());
        db.load_all().unwrap();
        assert!(db.has_enrolled_faces("alice").unwrap());
    }

    #[test]
    fn failed_rollback_preserves_backup_and_disables_matching_and_writes() {
        let temp = TempDir::new("rollback-failure");
        let base = temp.path().join("users");
        let mut db = UserDatabase::new(base.to_str().unwrap(), 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        db.fail_rollback = true;
        assert!(
            db.change_encryption(test_cipher(), true, || anyhow::bail!("config read-only"))
                .is_err()
        );
        assert!(
            db.match_faces("alice", &embedding(&[1.0]), 0.5, Spectrum::Rgb)
                .is_err()
        );
        assert!(
            db.add_template("alice", "work", "2", rgb_embeds(vec![embedding(&[1.0])]))
                .is_err()
        );
        assert!(db.load_all().is_err());
        assert!(db.get_user_embeddings("alice").is_none());
        assert_eq!(
            fs::read_dir(temp.path()).unwrap().count(),
            2,
            "both complete stores must survive"
        );
        drop(db);
        let db =
            UserDatabase::new_with_cipher(base.to_str().unwrap(), 4, Some(test_cipher())).unwrap();
        assert!(db.has_enrolled_faces("alice").unwrap());
        assert_eq!(
            fs::read_dir(temp.path()).unwrap().count(),
            1,
            "restart cleans the abandoned backup"
        );
    }

    #[test]
    fn rejected_settings_restore_recognizer_snapshot_and_template_cap() {
        let temp = TempDir::new("settings-rollback");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.use_recognizer("old").unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        assert!(
            db.apply_settings(Some(test_cipher()), true, Some("new"), 1, || anyhow::bail!(
                "config read-only"
            ))
            .is_err()
        );
        assert_eq!(db.recognizer.as_deref(), Some("old"));
        assert_eq!(db.max_templates, 4);
        assert!(!db.is_encrypted());
        assert!(db.has_enrolled_faces("alice").unwrap());
        db.fail_reload = true;
        let saved = Cell::new(false);
        assert!(
            db.apply_settings(None, false, Some("new"), 1, || {
                saved.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(
            !saved.get(),
            "a failed recognizer reload must happen before saving config"
        );
        assert!(db.has_enrolled_faces("alice").unwrap());
    }

    #[test]
    fn legacy_ciphertext_is_only_accepted_by_explicit_maintenance() {
        use aes_gcm::aead::{Aead, KeyInit};
        let temp = TempDir::new("legacy-migration");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new(base, 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        let file = db.collect_bin_files().unwrap().pop().unwrap();
        let plaintext = fs::read(&file).unwrap();
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&[42; crypto::KEY_LEN]).unwrap();
        let nonce = aes_gcm::Nonce::from([0; 12]);
        let mut legacy = b"GZE1".to_vec();
        legacy.extend_from_slice(&nonce);
        legacy.extend_from_slice(&cipher.encrypt(&nonce, plaintext.as_slice()).unwrap());
        fs::write(&file, &legacy).unwrap();
        db.set_cipher(Some(test_cipher()));
        db.load_all().unwrap();
        assert!(!db.has_enrolled_faces("alice").unwrap());
        assert!(db.migrate_plaintext_to_encrypted().is_err());
        assert!(db.decrypt_all_with(&test_cipher()).is_err());
        assert_eq!(fs::read(&file).unwrap(), legacy);
        assert_eq!(db.migrate_legacy_with(&test_cipher()).unwrap(), 1);
        assert!(fs::read(&file).unwrap().starts_with(b"GZE2"));
        db.load_all().unwrap();
        assert!(db.has_enrolled_faces("alice").unwrap());
        // Reintroducing a legacy record later must not silently rebind it.
        fs::write(&file, legacy).unwrap();
        assert!(db.migrate_plaintext_to_encrypted().is_err());
    }

    #[test]
    fn a_current_store_is_not_rewritten_on_startup() {
        use std::os::unix::fs::MetadataExt;
        let temp = TempDir::new("no-op-migration");
        let mut db =
            UserDatabase::new_with_cipher(temp.path().to_str().unwrap(), 4, Some(test_cipher()))
                .unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        let file = db.collect_bin_files().unwrap().pop().unwrap();
        let inode = fs::metadata(&file).unwrap().ino();
        assert_eq!(db.migrate_plaintext_to_encrypted().unwrap(), 0);
        assert_eq!(fs::metadata(&file).unwrap().ino(), inode);
    }

    #[test]
    fn cleanup_does_not_delete_arbitrary_reserved_labels() {
        let temp = TempDir::new("reserved-label");
        fs::create_dir_all(temp.path().join("alice/.gaze-existing-face")).unwrap();
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        assert!(
            db.change_encryption(test_cipher(), true, || Ok(()))
                .is_err()
        );
        assert!(temp.path().join("alice/.gaze-existing-face").is_dir());
    }

    #[test]
    fn migration_finishes_a_mixed_store_and_binds_every_template() {
        let temp = TempDir::new("mixed-migration");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new(base, 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        db.set_cipher(Some(test_cipher()));
        db.add_template("alice", "work", "2", rgb_embeds(vec![embedding(&[0.5])]))
            .unwrap();
        assert!(
            db.require_bound_templates().is_err(),
            "startup must not silently authenticate the plaintext half"
        );
        assert_eq!(db.migrate_legacy_with(&test_cipher()).unwrap(), 1);
        db.require_bound_templates().unwrap();
        db.load_all().unwrap();
        assert_eq!(db.get_user_embeddings("alice").unwrap().len(), 2);
        assert!(all_bins_encrypted(&db));
        assert_eq!(db.migrate_plaintext_to_encrypted().unwrap(), 0);
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

        let mut enc = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        assert_eq!(
            enc.get_user_embeddings("alice").map(|v| v.len()),
            Some(0),
            "plaintext templates are not trusted once encryption is enabled"
        );

        let expected = enc.collect_bin_files().unwrap().len();
        assert_eq!(enc.migrate_plaintext_to_encrypted().unwrap(), expected);
        assert!(all_bins_encrypted(&enc));
        assert_eq!(enc.migrate_plaintext_to_encrypted().unwrap(), 0);

        enc.load_all().unwrap();
        assert_eq!(enc.get_user_embeddings("alice").unwrap().len(), 1);

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

    #[test]
    fn migrating_an_already_encrypted_store_re_encrypts_nothing() {
        let temp = TempDir::new("migrate-idempotent");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        assert_eq!(db.migrate_plaintext_to_encrypted().unwrap(), 0);
        assert!(all_bins_encrypted(&db));
    }

    #[test]
    fn migrating_without_a_key_is_a_no_op() {
        let temp = TempDir::new("migrate-no-key");
        let mut db = UserDatabase::new(temp.path().to_str().unwrap(), 4).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();

        assert_eq!(db.migrate_plaintext_to_encrypted().unwrap(), 0);
        assert!(!all_bins_encrypted(&db));
    }

    #[test]
    fn decrypting_with_the_wrong_key_changes_nothing_on_disk() {
        let temp = TempDir::new("wrong-key");
        let base = temp.path().to_str().unwrap();
        let mut db = UserDatabase::new_with_cipher(base, 4, Some(test_cipher())).unwrap();
        db.add_template("alice", "work", "1", rgb_embeds(vec![embedding(&[1.0])]))
            .unwrap();
        let before: Vec<Vec<u8>> = db
            .collect_bin_files()
            .unwrap()
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect();

        assert!(
            db.decrypt_all_with(&EmbeddingCipher::new(&[7u8; crypto::KEY_LEN]))
                .is_err()
        );

        let after: Vec<Vec<u8>> = db
            .collect_bin_files()
            .unwrap()
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect();
        assert_eq!(
            before, after,
            "a failed pass must not half-rewrite the store"
        );
        let leftovers = fs::read_dir(temp.path().join("alice/work/1"))
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0, "staging files must be cleaned up on failure");
    }
}
