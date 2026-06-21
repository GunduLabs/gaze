use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::anyhow;

pub const KEY_LEN: usize = 32;
const MAGIC: &[u8; 4] = b"GZE1";
const NONCE_LEN: usize = 12;

pub fn is_encrypted(data: &[u8]) -> bool {
    data.len() >= MAGIC.len() + NONCE_LEN && &data[..MAGIC.len()] == MAGIC
}

pub struct EmbeddingCipher {
    cipher: Aes256Gcm,
}

impl EmbeddingCipher {
    pub fn new(key: &[u8; KEY_LEN]) -> Self {
        let key = Key::<Aes256Gcm>::from_slice(key);
        Self {
            cipher: Aes256Gcm::new(key),
        }
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|e| anyhow!("failed to draw a random nonce: {e}"))?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| anyhow!("embedding encryption failed: {e}"))?;

        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    pub fn decrypt(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if !is_encrypted(data) {
            return Err(anyhow!("not a Gaze-encrypted embedding"));
        }
        let (nonce_bytes, ciphertext) = data[MAGIC.len()..].split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        self.cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow!("embedding decryption failed (wrong TPM key or corrupt data)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; KEY_LEN] = [7u8; KEY_LEN];
    const KEY_B: [u8; KEY_LEN] = [9u8; KEY_LEN];

    #[test]
    fn round_trips_and_marks_ciphertext() {
        let cipher = EmbeddingCipher::new(&KEY_A);
        let plaintext = b"some 512-float embedding bytes".to_vec();

        let blob = cipher.encrypt(&plaintext).unwrap();
        assert!(is_encrypted(&blob));
        assert_ne!(&blob[..], &plaintext[..]);
        assert_eq!(cipher.decrypt(&blob).unwrap(), plaintext);
    }

    #[test]
    fn nonce_is_randomised_per_call() {
        let cipher = EmbeddingCipher::new(&KEY_A);
        let a = cipher.encrypt(b"same").unwrap();
        let b = cipher.encrypt(b"same").unwrap();
        assert_ne!(
            a, b,
            "identical plaintext must not yield identical ciphertext"
        );
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let blob = EmbeddingCipher::new(&KEY_A).encrypt(b"secret").unwrap();
        assert!(EmbeddingCipher::new(&KEY_B).decrypt(&blob).is_err());
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let cipher = EmbeddingCipher::new(&KEY_A);
        let mut blob = cipher.encrypt(b"secret").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(cipher.decrypt(&blob).is_err());
    }

    #[test]
    fn plaintext_is_not_detected_as_encrypted() {
        assert!(!is_encrypted(&[0u8; 8]));
        assert!(!is_encrypted(b"GZE1"));
        assert!(!is_encrypted(b""));
    }
}
