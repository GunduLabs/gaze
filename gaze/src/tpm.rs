// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

pub use gaze_security::tpm::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static TPM_LOCK: Mutex<()> = Mutex::new(());
    #[test]
    #[ignore = "requires a usable TPM"]
    fn full_daemon_workflow_encrypts_and_reloads() {
        use crate::crypto::EmbeddingCipher;
        use crate::users::UserDatabase;
        use gaze_core::face::Spectrum;
        use ndarray::Array1;

        let _guard = TPM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("gaze-tpm-wf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tpm_dir = root.join("tpm");
        let users_dir = root.join("users");
        let users = users_dir.to_str().unwrap();

        let dek = load_or_create_dek(&tpm_dir).expect("seal DEK");
        let mut db =
            UserDatabase::new_with_cipher(users, 4, Some(EmbeddingCipher::new(&dek))).unwrap();
        db.add_template(
            "alice",
            "work",
            "1",
            vec![(Array1::from_vec(vec![0.1, 0.2, 0.3]), Spectrum::Rgb)],
        )
        .unwrap();

        let plain = UserDatabase::new(users, 4).unwrap();
        assert_eq!(plain.get_user_embeddings("alice").map(|v| v.len()), Some(0));

        let dek2 = load_or_create_dek(&tpm_dir).expect("unseal DEK");
        assert_eq!(dek, dek2);
        let db2 =
            UserDatabase::new_with_cipher(users, 4, Some(EmbeddingCipher::new(&dek2))).unwrap();
        assert_eq!(db2.get_user_embeddings("alice").unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }
}
