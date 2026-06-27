//! Webhook URL helper behavior (CRD 2835-2839): derivation, validation,
//! platform detection, parameterized building, and equivalence comparison.

use serde_json::{json, Value};

use crate::config::Config;

/// Recognized platform path segments (CRD 2838): LINE-style, Facebook-style,
/// Instagram-style, plus reserved WhatsApp-style and Telegram-style paths.
pub const PLATFORM_PATHS: [(&str, &str); 5] = [
    ("line", "/api/webhook"),
    ("facebook", "/api/webhooks/facebook"),
    ("instagram", "/api/webhooks/instagram"),
    ("whatsapp", "/api/webhooks/whatsapp"),
    ("telegram", "/api/webhooks/telegram"),
];

pub fn webhook_path(platform: &str) -> Option<&'static str> {
    PLATFORM_PATHS
        .iter()
        .find(|(p, _)| *p == platform)
        .map(|(_, path)| *path)
}

fn base_url(config: &Config) -> String {
    config
        .backend_url
        .as_deref()
        .map(|b| b.trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("http://localhost:{}", config.port))
}

/// Derive the full externally-facing webhook URL for a platform from the
/// environment base URL.
pub fn webhook_url(config: &Config, platform: &str) -> Option<String> {
    webhook_path(platform).map(|path| format!("{}{path}", base_url(config)))
}

/// Config object: base URL, platform path, full URL, environment label.
pub fn webhook_config(config: &Config, platform: &str) -> Option<Value> {
    let path = webhook_path(platform)?;
    Some(json!({
        "baseUrl": base_url(config),
        "path": path,
        "url": format!("{}{path}", base_url(config)),
        "environment": if config.is_production() { "production" } else { "development" },
    }))
}

/// All platform webhook URLs.
pub fn all_webhook_urls(config: &Config) -> Value {
    let mut map = serde_json::Map::new();
    for (platform, path) in PLATFORM_PATHS {
        map.insert(
            platform.to_string(),
            json!(format!("{}{path}", base_url(config))),
        );
    }
    Value::Object(map)
}

/// Scheme/host/path split; only http(s) URLs with a host are accepted.
fn split_url(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme, host, path))
}

/// A well-formed http/https URL with a host and a non-root path (CRD 2837,
/// 2839): non-http(s) schemes, host-less URLs and root-only paths are rejected.
pub fn is_valid_webhook_url(url: &str) -> bool {
    matches!(split_url(url), Some((_, _, path)) if !path.is_empty() && path != "/")
}

/// Detect whether a URL is a webhook endpoint and which platform it targets by
/// inspecting its path segments; returns nothing for non-webhook paths.
pub fn extract_platform(url: &str) -> Option<&'static str> {
    let (_, _, path) = split_url(url)?;
    let path = path.trim_end_matches('/');
    PLATFORM_PATHS
        .iter()
        .find(|(_, p)| path == *p || (path.ends_with(p) && path.contains("/api/")))
        .map(|(platform, _)| *platform)
}

/// Build a webhook URL with appended query parameters.
pub fn build_webhook_url(
    config: &Config,
    platform: &str,
    params: &[(&str, &str)],
) -> Option<String> {
    let mut url = webhook_url(config, platform)?;
    for (i, (k, v)) in params.iter().enumerate() {
        url.push(if i == 0 { '?' } else { '&' });
        url.push_str(k);
        url.push('=');
        url.push_str(v);
    }
    Some(url)
}

/// Compare two webhook URLs ignoring query string and trailing slash.
pub fn urls_equivalent(a: &str, b: &str) -> bool {
    let norm = |u: &str| {
        u.split(['?', '#'])
            .next()
            .unwrap_or(u)
            .trim_end_matches('/')
            .to_string()
    };
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut c = crate::config::test_config();
        c.backend_url = Some("https://api.example.com/".into());
        c
    }

    #[test]
    fn derives_urls_and_config_objects() {
        let c = cfg();
        assert_eq!(
            webhook_url(&c, "line").unwrap(),
            "https://api.example.com/api/webhook"
        );
        assert_eq!(
            webhook_url(&c, "facebook").unwrap(),
            "https://api.example.com/api/webhooks/facebook"
        );
        assert!(webhook_url(&c, "smoke-signal").is_none());
        let obj = webhook_config(&c, "line").unwrap();
        assert_eq!(obj["baseUrl"], "https://api.example.com");
        assert_eq!(obj["environment"], "development");
        let all = all_webhook_urls(&c);
        assert_eq!(all.as_object().unwrap().len(), 5);
        assert!(all["telegram"]
            .as_str()
            .unwrap()
            .ends_with("/api/webhooks/telegram"));
    }

    #[test]
    fn validates_urls() {
        assert!(is_valid_webhook_url("https://x.example.com/api/webhook"));
        assert!(!is_valid_webhook_url("ftp://x.example.com/api/webhook")); // bad scheme
        assert!(!is_valid_webhook_url("https:///api/webhook")); // no host
        assert!(!is_valid_webhook_url("https://x.example.com")); // root-only
        assert!(!is_valid_webhook_url("https://x.example.com/")); // root-only
        assert!(!is_valid_webhook_url("not a url"));
    }

    #[test]
    fn extracts_platform_from_paths() {
        assert_eq!(extract_platform("https://h.io/api/webhook"), Some("line"));
        assert_eq!(
            extract_platform("https://h.io/api/webhooks/facebook/"),
            Some("facebook")
        );
        assert_eq!(
            extract_platform("https://h.io/api/webhooks/instagram?x=1"),
            Some("instagram")
        );
        assert_eq!(extract_platform("https://h.io/api/teams"), None);
        assert_eq!(extract_platform("nonsense"), None);
    }

    #[test]
    fn builds_and_compares_urls() {
        let c = cfg();
        let built = build_webhook_url(&c, "line", &[("team", "9"), ("token", "t1")]).unwrap();
        assert_eq!(built, "https://api.example.com/api/webhook?team=9&token=t1");
        assert!(urls_equivalent(
            "https://h.io/api/webhook/",
            "https://h.io/api/webhook?since=yesterday"
        ));
        assert!(!urls_equivalent(
            "https://h.io/api/webhook",
            "https://h.io/api/webhooks/facebook"
        ));
    }
}
