//! Shopee v2 request signature (HMAC-SHA256 over a base string).

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Base string for the signature. Public/auth calls use
/// `partner_id + path + timestamp`; shop-scoped calls append
/// `access_token + shop_id`.
pub fn base_string(
    partner_id: i64,
    path: &str,
    timestamp: i64,
    access_token: Option<&str>,
    shop_id: Option<i64>,
) -> String {
    match (access_token, shop_id) {
        (Some(tok), Some(shop)) => format!("{partner_id}{path}{timestamp}{tok}{shop}"),
        _ => format!("{partner_id}{path}{timestamp}"),
    }
}

/// Lowercase hex HMAC-SHA256 of `base` keyed by `partner_key`.
pub fn sign(partner_key: &str, base: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(partner_key.as_bytes()).expect("any key size");
    mac.update(base.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_base_string_is_partner_path_timestamp() {
        assert_eq!(
            base_string(1, "/api/v2/auth/token/get", 1610000000, None, None),
            "1/api/v2/auth/token/get1610000000"
        );
    }

    #[test]
    fn shop_base_string_appends_token_and_shop() {
        assert_eq!(
            base_string(1, "/api/v2/x", 1610000000, Some("ACCESS"), Some(42)),
            "1/api/v2/x1610000000ACCESS42"
        );
    }

    #[test]
    fn sign_is_deterministic_64_char_hex() {
        let a = sign("partnerkey", "1/api/v2/auth/token/get1610000000");
        let b = sign("partnerkey", "1/api/v2/auth/token/get1610000000");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, sign("otherkey", "1/api/v2/auth/token/get1610000000"));
    }
}
