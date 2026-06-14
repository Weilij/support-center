//! Cookie helpers for HttpOnly auth-cookie issuance (security review §3).

use axum::http::HeaderMap;

use crate::domain::auth::tokens;

/// Build a single `Set-Cookie` header value string.
fn build_cookie(
    name: &str,
    value: &str,
    max_age: i64,
    http_only: bool,
    path: &str,
    secure: bool,
) -> String {
    let mut s = format!(
        "{}={}; SameSite=Lax; Path={}; Max-Age={}",
        name, value, path, max_age
    );
    if http_only {
        s.push_str("; HttpOnly");
    }
    if secure {
        s.push_str("; Secure");
    }
    s
}

/// Return the three `Set-Cookie` strings for a fresh auth issuance.
///
/// - `mcss_access`  — HttpOnly, `Path=/`, TTL = ACCESS_TTL_SECS
/// - `mcss_refresh` — HttpOnly, `Path=/api/auth/refresh`, TTL = REFRESH_TTL_SECS
/// - `mcss_csrf`    — **NOT** HttpOnly (JS must read it), `Path=/`, TTL = REFRESH_TTL_SECS
pub fn auth_cookies(access: &str, refresh: &str, csrf: &str, secure: bool) -> Vec<String> {
    vec![
        build_cookie("mcss_access", access, tokens::ACCESS_TTL_SECS, true, "/", secure),
        build_cookie(
            "mcss_refresh",
            refresh,
            tokens::REFRESH_TTL_SECS,
            true,
            "/api/auth/refresh",
            secure,
        ),
        build_cookie("mcss_csrf", csrf, tokens::REFRESH_TTL_SECS, false, "/", secure),
    ]
}

/// Return three `Set-Cookie` strings with `Max-Age=0` to clear auth cookies.
pub fn clear_auth_cookies(secure: bool) -> Vec<String> {
    vec![
        build_cookie("mcss_access", "", 0, true, "/", secure),
        build_cookie("mcss_refresh", "", 0, true, "/api/auth/refresh", secure),
        build_cookie("mcss_csrf", "", 0, false, "/", secure),
    ]
}

/// Parse the `Cookie` request header and return the named cookie's value, if present.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get("cookie").and_then(|v| v.to_str().ok())?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn auth_cookies_dev_no_secure() {
        let cookies = auth_cookies("access_tok", "refresh_tok", "csrf_tok", false);
        assert_eq!(cookies.len(), 3);
        assert!(cookies[0].contains("mcss_access=access_tok"));
        assert!(cookies[0].contains("HttpOnly"));
        assert!(!cookies[0].contains("Secure"));
        assert!(cookies[0].contains("Path=/"));

        assert!(cookies[1].contains("mcss_refresh=refresh_tok"));
        assert!(cookies[1].contains("HttpOnly"));
        assert!(cookies[1].contains("Path=/api/auth/refresh"));

        assert!(cookies[2].contains("mcss_csrf=csrf_tok"));
        assert!(!cookies[2].contains("HttpOnly"));
        assert!(cookies[2].contains("Path=/"));
    }

    #[test]
    fn auth_cookies_prod_has_secure() {
        let cookies = auth_cookies("a", "r", "c", true);
        for c in &cookies {
            assert!(c.contains("Secure"), "expected Secure in: {c}");
        }
    }

    #[test]
    fn clear_auth_cookies_max_age_zero() {
        let cookies = clear_auth_cookies(false);
        for c in &cookies {
            assert!(c.contains("Max-Age=0"), "expected Max-Age=0 in: {c}");
        }
    }

    #[test]
    fn cookie_value_parses_named_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            HeaderValue::from_static("mcss_access=tok123; other=val"),
        );
        assert_eq!(cookie_value(&headers, "mcss_access"), Some("tok123".into()));
        assert_eq!(cookie_value(&headers, "other"), Some("val".into()));
        assert_eq!(cookie_value(&headers, "missing"), None);
    }

    #[test]
    fn cookie_value_absent_header() {
        let headers = HeaderMap::new();
        assert_eq!(cookie_value(&headers, "mcss_access"), None);
    }
}
