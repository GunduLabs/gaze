// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::anyhow;

pub use gaze_security::tpm::KEY_LEN;
const MAGIC: &[u8; 4] = b"GZE2";
const LEGACY_MAGIC: &[u8; 4] = b"GZE1";
const NONCE_LEN: usize = 12;

pub fn is_encrypted(data: &[u8]) -> bool {
    data.starts_with(MAGIC) || is_legacy(data)
}

pub fn is_legacy(data: &[u8]) -> bool {
    data.starts_with(LEGACY_MAGIC)
}

pub struct EmbeddingCipher {
    cipher: Aes256Gcm,
}

impl EmbeddingCipher {
    pub fn new(key: &[u8; KEY_LEN]) -> Self {
        let key = Key::<Aes256Gcm>::from(*key);
        Self {
            cipher: Aes256Gcm::new(&key),
        }
    }

    /// GZE2 authenticates both the embedding and its caller-supplied identity context.
    pub fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|e| anyhow!("failed to draw a random nonce: {e}"))?;
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|e| anyhow!("embedding encryption failed: {e}"))?;

        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    pub fn decrypt(&self, data: &[u8], aad: &[u8]) -> anyhow::Result<Vec<u8>> {
        if !data.starts_with(MAGIC) {
            return Err(anyhow!(
                "not an owner-bound Gaze embedding; legacy data requires migration"
            ));
        }
        self.decrypt_payload(data, aad)
    }

    /// Only explicit store migration may accept ciphertext without an owner binding.
    pub fn decrypt_legacy(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if !is_legacy(data) {
            return Err(anyhow!("not a legacy Gaze embedding"));
        }
        self.decrypt_payload(data, &[])
    }

    fn decrypt_payload(&self, data: &[u8], aad: &[u8]) -> anyhow::Result<Vec<u8>> {
        if data.len() < MAGIC.len() + NONCE_LEN + 16 {
            return Err(anyhow!("truncated encrypted embedding"));
        }
        let (nonce_bytes, ciphertext) = data[MAGIC.len()..].split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce_bytes).map_err(|e| anyhow!("invalid nonce: {e}"))?;
        self.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| {
                anyhow!(
                    "embedding authentication failed (wrong owner, metadata, key, or corrupt data)"
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; KEY_LEN] = [7u8; KEY_LEN];
    const KEY_B: [u8; KEY_LEN] = [9u8; KEY_LEN];
    const AAD: &[u8] = b"alice\0embedding_rgb.bin";

    #[test]
    fn round_trips_and_marks_ciphertext() {
        let cipher = EmbeddingCipher::new(&KEY_A);
        let plaintext = b"some 512-float embedding bytes".to_vec();
        let blob = cipher.encrypt(&plaintext, AAD).unwrap();
        assert!(is_encrypted(&blob));
        assert_ne!(&blob[..], &plaintext[..]);
        assert_eq!(cipher.decrypt(&blob, AAD).unwrap(), plaintext);
    }

    #[test]
    fn nonce_is_randomised_per_call() {
        let cipher = EmbeddingCipher::new(&KEY_A);
        assert_ne!(
            cipher.encrypt(b"same", AAD).unwrap(),
            cipher.encrypt(b"same", AAD).unwrap()
        );
    }

    #[test]
    fn wrong_key_or_identity_fails_to_decrypt() {
        let blob = EmbeddingCipher::new(&KEY_A)
            .encrypt(b"secret", AAD)
            .unwrap();
        assert!(EmbeddingCipher::new(&KEY_B).decrypt(&blob, AAD).is_err());
        for aad in [
            b"bob\0embedding_rgb.bin".as_slice(),
            b"alice\0embedding_ir.bin",
            b"",
        ] {
            assert!(EmbeddingCipher::new(&KEY_A).decrypt(&blob, aad).is_err());
        }
    }

    #[test]
    fn tampered_or_truncated_ciphertext_is_rejected() {
        let cipher = EmbeddingCipher::new(&KEY_A);
        let mut blob = cipher.encrypt(b"secret", AAD).unwrap();
        for len in 0..blob.len() {
            assert!(cipher.decrypt(&blob[..len], AAD).is_err());
        }
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(cipher.decrypt(&blob, AAD).is_err());
    }

    #[test]
    fn legacy_ciphertext_requires_explicit_migration() {
        let cipher = EmbeddingCipher::new(&KEY_A);
        let nonce = Nonce::from([0; NONCE_LEN]);
        let mut blob = LEGACY_MAGIC.to_vec();
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&cipher.cipher.encrypt(&nonce, b"secret".as_slice()).unwrap());
        assert!(is_encrypted(&blob));
        assert!(cipher.decrypt(&blob, AAD).is_err());
        assert_eq!(cipher.decrypt_legacy(&blob).unwrap(), b"secret");
    }

    #[test]
    fn plaintext_is_not_detected_as_encrypted() {
        assert!(!is_encrypted(&[0u8; 8]));
        assert!(!is_encrypted(b""));
        // Even truncated magic must not downgrade to plaintext.
        assert!(is_encrypted(b"GZE1"));
        assert!(is_encrypted(b"GZE2"));
    }
}
