//! Time-limited URL signatures bound to the exact storage location
//! (CRD 3116, 3126, 3216). Invalid/expired signatures surface as 404 so
//! object existence is never disclosed.

use hmac::{Hmac, Mac};
use sha2::Sha256;

fn compute(secret: &str, key: &str, expires: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key size");
    mac.update(format!("{key}:{expires}").as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Returns (signature, absolute expiry as unix seconds).
pub fn sign(secret: &str, key: &str, ttl_secs: i64) -> (String, i64) {
    let expires = chrono::Utc::now().timestamp() + ttl_secs;
    (compute(secret, key, expires), expires)
}

pub fn verify(secret: &str, key: &str, sig: &str, expires: i64) -> bool {
    if expires < chrono::Utc::now().timestamp() {
        return false;
    }
    // Constant-time-ish comparison via the hmac verify path.
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key size");
    mac.update(format!("{key}:{expires}").as_bytes());
    let Ok(raw) = hex_decode(sig) else {
        return false;
    };
    mac.verify_slice(&raw).is_ok()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| ()))
        .collect()
}

/// Signed proxy URL for a storage location (default ~1h for downloads,
/// ~24h for public proxy links).
pub fn signed_path(secret: &str, route_prefix: &str, key: &str, ttl_secs: i64) -> String {
    let (sig, expires) = sign(secret, key, ttl_secs);
    format!("{route_prefix}/{key}?expires={expires}&sig={sig}")
}
