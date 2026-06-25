#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub jwt_secret: String,
    pub encryption_key: Option<String>,
    pub environment: String,
    pub frontend_url: Option<String>,
    pub backend_url: Option<String>,
    pub public_storage_url: Option<String>,
    pub extra_origins: Vec<String>,
    /// Reverse proxy IPs whose forwarded client-IP headers may be trusted.
    pub trusted_proxies: Vec<std::net::IpAddr>,
    pub port: u16,
    /// Local directory for message-attachment uploads (created at runtime).
    pub upload_dir: String,
    /// LINE webhook signature secret (CRD 2735: a configured platform channel
    /// secret must be present in the environment).
    pub line_channel_secret: Option<String>,
    /// Facebook/Instagram webhook app secret (CRD 2788: two environment names
    /// accepted, in priority order: FACEBOOK_APP_SECRET then FB_APP_SECRET).
    pub facebook_app_secret: Option<String>,
    /// Facebook subscription-handshake verification token (CRD 2787).
    pub facebook_verify_token: Option<String>,
    /// Facebook Page access token for outbound Send API (FACEBOOK_PAGE_ACCESS_TOKEN).
    pub facebook_page_access_token: Option<String>,
    /// Instagram messaging access token (INSTAGRAM_ACCESS_TOKEN); falls back to
    /// the Facebook page token when unset (IG messaging uses the linked page).
    pub instagram_access_token: Option<String>,
    /// LINE front-end (LIFF) application identifier.
    pub liff_id: Option<String>,
    /// LINE Login channel id used to verify LIFF ID tokens.
    pub line_login_channel_id: Option<String>,
    /// LINE ID-token verification endpoint.
    pub line_id_token_verify_url: String,
    /// Messaging-account handle, e.g. "@support".
    pub line_bot_id: Option<String>,
    /// LINE push credential (required by the LIFF welcome flow).
    pub line_channel_access_token: Option<String>,
    /// LINE push endpoint; override only in tests/dev harnesses.
    pub line_push_url: String,
    /// Separate HMAC secret for signing/verifying file download URLs (review #8).
    /// Falls back to `jwt_secret` when unset so existing deployments keep working.
    pub file_signing_secret: Option<String>,
    /// Shopee Open Platform partner id (SHOPEE_PARTNER_ID).
    pub shopee_partner_id: Option<i64>,
    /// Shopee Open Platform partner key / app secret (SHOPEE_PARTNER_KEY).
    pub shopee_partner_key: Option<String>,
    /// Shopee API host (SHOPEE_HOST), e.g. https://partner.shopeemobile.com.
    pub shopee_host: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();
        Self {
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://localhost/mcss".into()),
            jwt_secret: std::env::var("JWT_SECRET")
                .unwrap_or_else(|_| "dev-only-insecure-secret".into()),
            encryption_key: std::env::var("ENCRYPTION_KEY").ok().filter(|s| !s.is_empty()),
            environment: std::env::var("ENVIRONMENT").unwrap_or_else(|_| "production".into()),
            frontend_url: std::env::var("FRONTEND_URL").ok().filter(|s| !s.is_empty()),
            backend_url: std::env::var("BACKEND_URL")
                .ok()
                .map(|s| s.trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty()),
            public_storage_url: std::env::var("PUBLIC_STORAGE_URL").ok().filter(|s| !s.is_empty()),
            extra_origins: std::env::var("EXTRA_ORIGINS")
                .map(|s| {
                    s.split(',')
                        .map(|o| o.trim().to_string())
                        .filter(|o| !o.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            trusted_proxies: std::env::var("TRUSTED_PROXIES")
                .map(|s| parse_trusted_proxies(&s))
                .unwrap_or_else(|_| default_trusted_proxies()),
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            upload_dir: std::env::var("UPLOAD_DIR")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "data/uploads".into()),
            line_channel_secret: std::env::var("LINE_CHANNEL_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            facebook_app_secret: std::env::var("FACEBOOK_APP_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var("FB_APP_SECRET").ok().filter(|s| !s.is_empty())),
            liff_id: std::env::var("LIFF_ID").ok().filter(|s| !s.is_empty()),
            line_login_channel_id: std::env::var("LINE_LOGIN_CHANNEL_ID")
                .ok()
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var("LIFF_CHANNEL_ID").ok().filter(|s| !s.is_empty())),
            line_id_token_verify_url: std::env::var("LINE_ID_TOKEN_VERIFY_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "https://api.line.me/oauth2/v2.1/verify".into()),
            line_bot_id: std::env::var("LINE_BOT_ID").ok().filter(|s| !s.is_empty()),
            line_channel_access_token: std::env::var("LINE_CHANNEL_ACCESS_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            line_push_url: std::env::var("LINE_PUSH_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "https://api.line.me/v2/bot/message/push".into()),
            facebook_verify_token: std::env::var("FACEBOOK_VERIFY_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            facebook_page_access_token: std::env::var("FACEBOOK_PAGE_ACCESS_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            instagram_access_token: std::env::var("INSTAGRAM_ACCESS_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            file_signing_secret: std::env::var("FILE_SIGNING_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            shopee_partner_id: std::env::var("SHOPEE_PARTNER_ID").ok().and_then(|s| s.parse().ok()),
            shopee_partner_key: std::env::var("SHOPEE_PARTNER_KEY").ok().filter(|s| !s.is_empty()),
            shopee_host: std::env::var("SHOPEE_HOST").ok().filter(|s| !s.is_empty()),
        }
    }

    pub fn is_production(&self) -> bool {
        !matches!(
            self.environment.trim().to_ascii_lowercase().as_str(),
            "development" | "dev" | "test" | "local"
        )
    }

    /// Key used to sign/verify file download URLs (review #8). Falls back to the
    /// JWT secret for backward compatibility when FILE_SIGNING_SECRET is unset.
    pub fn file_signing_key(&self) -> &str {
        self.file_signing_secret.as_deref().unwrap_or(&self.jwt_secret)
    }

    /// Reject insecure production-like configuration. Unknown/missing environments
    /// fail closed as production; only explicit dev/test/local names are relaxed.
    pub fn validate_for_production(&self) -> Result<(), String> {
        if !self.is_production() {
            return Ok(());
        }
        if self.jwt_secret == "dev-only-insecure-secret" {
            return Err("JWT_SECRET must be set in production (refusing the insecure default)".into());
        }
        if self.jwt_secret.len() < 32 {
            return Err("JWT_SECRET is too short for production (require >= 32 chars)".into());
        }
        if self.encryption_key.is_none() {
            return Err("ENCRYPTION_KEY must be set in production to protect integration credentials".into());
        }
        Ok(())
    }

    /// Allowed browser origins per CRD §7.1 "Allowed-origins policy" (line 5656).
    pub fn allowed_origins(&self) -> Vec<String> {
        let mut origins: Vec<String> = Vec::new();
        if !self.is_production() {
            for port in [3000, 3001, 4173, 5173, 5174, 8080, 8787] {
                for scheme in ["http", "https"] {
                    origins.push(format!("{scheme}://localhost:{port}"));
                    origins.push(format!("{scheme}://127.0.0.1:{port}"));
                }
            }
        }
        for o in [&self.frontend_url, &self.backend_url, &self.public_storage_url]
            .into_iter()
            .flatten()
        {
            origins.push(o.trim_end_matches('/').to_string());
        }
        origins.extend(self.extra_origins.iter().cloned());
        origins.sort();
        origins.dedup();
        origins
    }
}

#[cfg(test)]
pub fn test_config() -> Config {
    Config {
        database_url: "postgres://localhost/mcss_test".into(),
        jwt_secret: "test-secret".into(),
        encryption_key: None,
        environment: "development".into(),
        frontend_url: None,
        backend_url: None,
        public_storage_url: None,
        extra_origins: vec![],
        trusted_proxies: default_trusted_proxies(),
        port: 0,
        upload_dir: "data/uploads".into(),
        line_channel_secret: None,
        facebook_app_secret: None,
        facebook_verify_token: None,
        facebook_page_access_token: None,
        instagram_access_token: None,
        liff_id: None,
        line_login_channel_id: None,
        line_id_token_verify_url: "https://api.line.me/oauth2/v2.1/verify".into(),
        line_bot_id: None,
        line_channel_access_token: None,
        line_push_url: "https://api.line.me/v2/bot/message/push".into(),
        file_signing_secret: None,
        shopee_partner_id: None,
        shopee_partner_key: None,
        shopee_host: None,
    }
}

fn default_trusted_proxies() -> Vec<std::net::IpAddr> {
    ["127.0.0.1", "::1"]
        .into_iter()
        .filter_map(|ip| ip.parse().ok())
        .collect()
}

fn parse_trusted_proxies(raw: &str) -> Vec<std::net::IpAddr> {
    raw.split(',')
        .filter_map(|ip| ip.trim().parse().ok())
        .collect()
}

#[cfg(test)]
mod validate_production_tests {
    use super::*;

    /// Development config (test_config) must always pass because its environment
    /// explicitly opts into the dev/test relaxation.
    #[test]
    fn dev_config_passes() {
        let cfg = test_config();
        assert_eq!(cfg.environment, "development");
        assert!(cfg.validate_for_production().is_ok());
    }

    #[test]
    fn unknown_environment_is_production_like() {
        let cfg = Config {
            environment: "prodution".into(),
            jwt_secret: "dev-only-insecure-secret".into(),
            encryption_key: Some("some-key".into()),
            ..test_config()
        };
        assert!(cfg.is_production());
        let err = cfg.validate_for_production().unwrap_err();
        assert!(err.contains("JWT_SECRET must be set"), "unexpected error: {err}");
    }

    /// Production config using the insecure default secret must be rejected.
    #[test]
    fn production_default_secret_rejected() {
        let cfg = Config {
            environment: "production".into(),
            jwt_secret: "dev-only-insecure-secret".into(),
            encryption_key: Some("some-key".into()),
            ..test_config()
        };
        let err = cfg.validate_for_production().unwrap_err();
        assert!(err.contains("JWT_SECRET must be set"), "unexpected error: {err}");
    }

    /// Production config with a short (< 32 char) secret must be rejected.
    #[test]
    fn production_short_secret_rejected() {
        let cfg = Config {
            environment: "production".into(),
            jwt_secret: "tooshort".into(),
            encryption_key: Some("some-key".into()),
            ..test_config()
        };
        let err = cfg.validate_for_production().unwrap_err();
        assert!(err.contains("too short"), "unexpected error: {err}");
    }

    /// Production config missing ENCRYPTION_KEY must be rejected.
    #[test]
    fn production_missing_encryption_key_rejected() {
        let cfg = Config {
            environment: "production".into(),
            jwt_secret: "a-sufficiently-long-production-secret-key".into(),
            encryption_key: None,
            ..test_config()
        };
        let err = cfg.validate_for_production().unwrap_err();
        assert!(err.contains("ENCRYPTION_KEY"), "unexpected error: {err}");
    }

    /// Production config with a 32+ char secret AND an encryption key must pass.
    #[test]
    fn production_valid_config_passes() {
        let cfg = Config {
            environment: "production".into(),
            jwt_secret: "a-sufficiently-long-production-secret-key".into(),
            encryption_key: Some("my-encryption-key".into()),
            ..test_config()
        };
        assert!(cfg.validate_for_production().is_ok());
    }
}
