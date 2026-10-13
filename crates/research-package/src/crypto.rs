//! Optional passphrase encryption of a whole package.
//!
//! Layout: magic, 16-byte salt, 12-byte nonce, ChaCha20-Poly1305 ciphertext.
//! The key is derived from the passphrase with Argon2id (crate defaults).

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use crate::error::{PackageError, Result};

const MAGIC: &[u8; 8] = b"RSPKGENC";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = MAGIC.len() + SALT_LEN + NONCE_LEN;

/// Encrypt package bytes under a passphrase.
pub fn encrypt(package: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut salt).map_err(|_| PackageError::Random)?;
    getrandom::fill(&mut nonce).map_err(|_| PackageError::Random)?;

    let key = derive_key(passphrase, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), package)
        .map_err(|_| PackageError::Decrypt)?;

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// True if `bytes` start with the encryption header.
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// Undo [`encrypt`]. Plain packages pass through unchanged and need no passphrase.
pub fn decrypt(bytes: &[u8], passphrase: Option<&str>) -> Result<Vec<u8>> {
    if !is_encrypted(bytes) {
        return Ok(bytes.to_vec());
    }
    let passphrase = passphrase.ok_or(PackageError::PassphraseRequired)?;
    if bytes.len() < HEADER_LEN {
        return Err(PackageError::Decrypt);
    }
    let salt = &bytes[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce = &bytes[MAGIC.len() + SALT_LEN..HEADER_LEN];
    let key = derive_key(passphrase, salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    cipher
        .decrypt(Nonce::from_slice(nonce), &bytes[HEADER_LEN..])
        .map_err(|_| PackageError::Decrypt)
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| PackageError::Kdf)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_bytes_decrypt_with_the_right_passphrase_only() {
        let plain = b"package bytes".to_vec();
        let sealed = encrypt(&plain, "correct horse").unwrap();
        assert!(is_encrypted(&sealed));
        assert_eq!(decrypt(&sealed, Some("correct horse")).unwrap(), plain);
        assert!(matches!(
            decrypt(&sealed, Some("wrong")),
            Err(PackageError::Decrypt)
        ));
        assert!(matches!(
            decrypt(&sealed, None),
            Err(PackageError::PassphraseRequired)
        ));
    }

    #[test]
    fn plain_bytes_pass_through() {
        let plain = b"PK\x03\x04 not encrypted".to_vec();
        assert!(!is_encrypted(&plain));
        assert_eq!(decrypt(&plain, None).unwrap(), plain);
    }

    #[test]
    fn tampering_is_detected() {
        let mut sealed = encrypt(b"data", "pw").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(matches!(
            decrypt(&sealed, Some("pw")),
            Err(PackageError::Decrypt)
        ));
    }
}
