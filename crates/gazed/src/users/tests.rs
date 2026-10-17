// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

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
    let db = UserDatabase::new_with_cipher(base.to_str().unwrap(), 4, Some(test_cipher())).unwrap();
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
fn daemon_and_maintenance_cannot_lock_the_same_store() {
    let temp = TempDir::new("store-lock");
    let base = temp.path().join("users");
    let lock = UserDatabase::lock_store(&base).unwrap();
    assert!(UserDatabase::lock_store(&base).is_err());
    drop(lock);
    assert!(UserDatabase::lock_store(&base).is_ok());
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
