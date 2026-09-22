//! AES-256-GCM encryption for values kept at rest (megh-go's `Encryptor`). The caller supplies the key; megh
//! never reads it from the environment itself.

use aes_gcm::aead::{Aead, Generate, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EncryptError {
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
}

/// Encrypts and decrypts strings for at-rest storage. `key` must come from outside the database (an environment
/// variable or a secrets manager).
#[derive(Clone)]
pub struct Encryptor {
    cipher: Aes256Gcm,
}

impl Encryptor {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key)) }
    }

    /// A random nonce followed by the ciphertext, base64 (URL-safe, unpadded).
    pub fn encrypt(&self, plaintext: &str) -> Result<String, EncryptError> {
        let nonce = Nonce::generate();
        let ciphertext = self.cipher.encrypt(&nonce, plaintext.as_bytes()).map_err(|_| EncryptError::Encrypt)?;
        Ok(URL_SAFE_NO_PAD.encode([nonce.as_slice(), &ciphertext].concat()))
    }

    pub fn decrypt(&self, encoded: &str) -> Result<String, EncryptError> {
        let data = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| EncryptError::Decrypt)?;
        let (nonce, ciphertext) = data.split_at_checked(12).ok_or(EncryptError::Decrypt)?;
        let nonce = Nonce::try_from(nonce).map_err(|_| EncryptError::Decrypt)?;
        let plaintext = self.cipher.decrypt(&nonce, ciphertext).map_err(|_| EncryptError::Decrypt)?;
        String::from_utf8(plaintext).map_err(|_| EncryptError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encryptor() -> Encryptor {
        Encryptor::new(&[7u8; 32])
    }

    #[test]
    fn a_value_round_trips() {
        let encrypted = encryptor().encrypt("s3cret-access-token").unwrap();

        assert_ne!(encrypted, "s3cret-access-token");
        assert_eq!(encryptor().decrypt(&encrypted).unwrap(), "s3cret-access-token");
    }

    #[test]
    fn two_encryptions_of_the_same_value_differ() {
        let enc = encryptor();

        assert_ne!(enc.encrypt("token").unwrap(), enc.encrypt("token").unwrap());
    }

    #[test]
    fn the_wrong_key_cannot_decrypt() {
        let encrypted = encryptor().encrypt("token").unwrap();

        assert!(Encryptor::new(&[9u8; 32]).decrypt(&encrypted).is_err());
    }

    #[test]
    fn garbage_input_is_rejected_not_panicked() {
        assert!(encryptor().decrypt("not base64!!").is_err());
        assert!(encryptor().decrypt("dG9vc2hvcnQ").is_err());
    }
}
