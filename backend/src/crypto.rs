//! Credential protection at rest (CRD lines 5716-5727).
//!
//! Implements the six observable guarantees with AES-256-GCM:
//!
//! 1. **Not readable at rest** — stored values are ciphertext under the
//!    configured key, never returned to clients.
//! 2. **Non-deterministic protection** — a fresh random 96-bit nonce per
//!    encryption, so equal inputs yield different stored values.
//! 3. **Tamper detection** — GCM authentication: any altered stored value
//!    fails the read instead of returning corrupted data.
//! 4. **Authorized read returns the original** — decryption with the correct
//!    key yields the exact original value.
//! 5. **Mixed-format tolerance** — protected values carry a recognizable
//!    prefix; values without it (historical plaintext) are returned as-is.
//!    When no key is configured, new values are stored unprotected with a
//!    warning.
//! 6. **Documented error behavior** — missing configuration or invalid key
//!    material reports a specific [`CryptoError`]; no partial result is ever
//!    produced.
//!
//! Key format: `ENCRYPTION_KEY` is the **standard-base64 encoding of exactly
//! 32 random bytes** (see `backend/.env.example`). [`generate_key`] produces a
//! fresh one.
//!
//! Stored format: `enc:v1:` + base64(nonce[12] || ciphertext+tag).

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;

/// Marker distinguishing protected blobs from legacy plaintext (guarantee 5).
pub const PROTECTED_PREFIX: &str = "enc:v1:";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    /// Protection is required but `ENCRYPTION_KEY` is not configured.
    #[error("Credential protection is not configured (ENCRYPTION_KEY is not set)")]
    NotConfigured,
    /// The configured key is not base64 of exactly 32 bytes.
    #[error("Invalid encryption key: {0}")]
    InvalidKey(String),
    /// The stored value is malformed, was altered, or was protected with a
    /// different key (guarantee 3).
    #[error("Credential decryption failed: value is corrupted or protected with a different key")]
    Tampered,
}

impl From<CryptoError> for crate::error::AppError {
    fn from(e: CryptoError) -> Self {
        crate::error::AppError::Internal(e.to_string())
    }
}

fn parse_key(key: &str) -> Result<[u8; 32], CryptoError> {
    let bytes = B64
        .decode(key.trim())
        .map_err(|_| CryptoError::InvalidKey("must be standard base64".into()))?;
    bytes
        .try_into()
        .map_err(|_| CryptoError::InvalidKey("must decode to exactly 32 bytes".into()))
}

/// Whether a stored value is in the protected format.
pub fn is_protected(stored: &str) -> bool {
    stored.starts_with(PROTECTED_PREFIX)
}

/// Encrypt one secret under the configured key (guarantees 1-3).
pub fn encrypt(key: &str, plaintext: &str) -> Result<String, CryptoError> {
    let key_bytes = parse_key(key)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
        .map_err(|_| CryptoError::Tampered)?;
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{PROTECTED_PREFIX}{}", B64.encode(blob)))
}

/// Decrypt one protected value (guarantees 3-4). The input must carry the
/// protected prefix; use [`reveal`] for mixed-format reads.
pub fn decrypt(key: &str, stored: &str) -> Result<String, CryptoError> {
    let key_bytes = parse_key(key)?;
    let encoded = stored
        .strip_prefix(PROTECTED_PREFIX)
        .ok_or(CryptoError::Tampered)?;
    let blob = B64.decode(encoded).map_err(|_| CryptoError::Tampered)?;
    if blob.len() < 12 {
        return Err(CryptoError::Tampered);
    }
    let (nonce, ciphertext) = blob.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| CryptoError::Tampered)?;
    String::from_utf8(plaintext).map_err(|_| CryptoError::Tampered)
}

/// Protect a secret for storage. With a configured key the value is encrypted;
/// without one it is stored unprotected, accompanied by a warning (guarantee 5).
/// An invalid configured key is the documented error (guarantee 6).
pub fn protect(key: Option<&str>, plaintext: &str) -> Result<String, CryptoError> {
    match key {
        Some(k) => encrypt(k, plaintext),
        None => {
            tracing::warn!(
                "ENCRYPTION_KEY is not configured; storing integration credential unprotected"
            );
            Ok(plaintext.to_string())
        }
    }
}

/// Read a stored secret tolerating both formats (guarantee 5): legacy
/// plaintext passes through unchanged; a protected value requires the
/// configured key ([`CryptoError::NotConfigured`] otherwise, guarantee 6).
pub fn reveal(key: Option<&str>, stored: &str) -> Result<String, CryptoError> {
    if !is_protected(stored) {
        return Ok(stored.to_string());
    }
    let key = key.ok_or(CryptoError::NotConfigured)?;
    decrypt(key, stored)
}

/// Produce a fresh random protection key (CRD 5727: "a setup facility can also
/// produce a fresh random protection key").
pub fn generate_key() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    B64.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> String {
        generate_key()
    }

    // Guarantee 1: not readable at rest.
    #[test]
    fn ciphertext_does_not_reveal_plaintext() {
        let k = key();
        let stored = encrypt(&k, "super-secret-token").unwrap();
        assert!(is_protected(&stored));
        assert_ne!(stored, "super-secret-token");
        assert!(!stored.contains("super-secret-token"));
    }

    // Guarantee 2: non-deterministic protection.
    #[test]
    fn protecting_twice_yields_different_values() {
        let k = key();
        let a = encrypt(&k, "same input").unwrap();
        let b = encrypt(&k, "same input").unwrap();
        assert_ne!(a, b);
        // Both still decrypt to the original.
        assert_eq!(decrypt(&k, &a).unwrap(), "same input");
        assert_eq!(decrypt(&k, &b).unwrap(), "same input");
    }

    // Guarantee 3: tamper detection.
    #[test]
    fn altered_value_fails_to_decrypt() {
        let k = key();
        let stored = encrypt(&k, "payload").unwrap();
        // Flip one character of the base64 body.
        let mut chars: Vec<char> = stored.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert_eq!(decrypt(&k, &tampered).unwrap_err(), CryptoError::Tampered);
        // Truncated blob also fails cleanly.
        assert_eq!(
            decrypt(&k, "enc:v1:AAAA").unwrap_err(),
            CryptoError::Tampered
        );
    }

    // Guarantee 4: authorized read returns the original.
    #[test]
    fn round_trip_returns_exact_original() {
        let k = key();
        let original = "token-ключ-ありがとう \u{1F511} with spaces";
        let stored = encrypt(&k, original).unwrap();
        assert_eq!(decrypt(&k, &stored).unwrap(), original);
        assert_eq!(reveal(Some(&k), &stored).unwrap(), original);
    }

    // Guarantee 5: mixed plaintext/protected tolerance.
    #[test]
    fn legacy_plaintext_remains_readable() {
        let k = key();
        // Historical plaintext passes through with or without a key.
        assert_eq!(
            reveal(Some(&k), "legacy-plain-token").unwrap(),
            "legacy-plain-token"
        );
        assert_eq!(
            reveal(None, "legacy-plain-token").unwrap(),
            "legacy-plain-token"
        );
        // Without configured protection, protect() stores unprotected (warned).
        let stored = protect(None, "new-secret").unwrap();
        assert_eq!(stored, "new-secret");
        assert!(!is_protected(&stored));
        // With protection configured, protect() encrypts.
        let stored = protect(Some(&k), "new-secret").unwrap();
        assert!(is_protected(&stored));
        assert_eq!(reveal(Some(&k), &stored).unwrap(), "new-secret");
    }

    // Guarantee 6: documented error when unconfigured or key invalid.
    #[test]
    fn unconfigured_or_invalid_key_reports_documented_error() {
        let k = key();
        let stored = encrypt(&k, "secret").unwrap();
        // Protected value but no key configured.
        assert_eq!(
            reveal(None, &stored).unwrap_err(),
            CryptoError::NotConfigured
        );
        // Not valid base64.
        assert!(matches!(
            encrypt("not base64 !!!", "x").unwrap_err(),
            CryptoError::InvalidKey(_)
        ));
        // Valid base64 of the wrong length.
        assert!(matches!(
            encrypt(&B64.encode([1u8; 16]), "x").unwrap_err(),
            CryptoError::InvalidKey(_)
        ));
        assert!(matches!(
            decrypt("not base64 !!!", &stored).unwrap_err(),
            CryptoError::InvalidKey(_)
        ));
        // Correct format but a *different* key: tamper-evident failure, no partial result.
        let other = generate_key();
        assert_eq!(decrypt(&other, &stored).unwrap_err(), CryptoError::Tampered);
    }

    #[test]
    fn generated_keys_are_valid_and_distinct() {
        let a = generate_key();
        let b = generate_key();
        assert_ne!(a, b);
        assert!(parse_key(&a).is_ok());
        assert_eq!(B64.decode(&a).unwrap().len(), 32);
    }
}
