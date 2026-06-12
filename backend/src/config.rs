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
    /// LINE front-end (LIFF) application identifier.
    pub liff_id: Option<String>,
    /// Messaging-account handle, e.g. "@support".
    pub line_bot_id: Option<String>,
    /// LINE push credential (required by the LIFF welcome flow).
    pub line_channel_access_token: Option<String>,
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
            environment: std::env::var("ENVIRONMENT")
                .unwrap_or_else(|_| "development".into()),
            frontend_url: std::env::var("FRONTEND_URL").ok().filter(|s| !s.is_empty()),
            backend_url: std::env::var("BACKEND_URL").ok().filter(|s| !s.is_empty()),
            public_storage_url: std::env::var("PUBLIC_STORAGE_URL").ok().filter(|s| !s.is_empty()),
            extra_origins: std::env::var("EXTRA_ORIGINS")
                .map(|s| {
                    s.split(',')
                        .map(|o| o.trim().to_string())
                        .filter(|o| !o.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
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
            line_bot_id: std::env::var("LINE_BOT_ID").ok().filter(|s| !s.is_empty()),
            line_channel_access_token: std::env::var("LINE_CHANNEL_ACCESS_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            facebook_verify_token: std::env::var("FACEBOOK_VERIFY_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }

    pub fn is_production(&self) -> bool {
        self.environment == "production"
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
        port: 0,
        upload_dir: "data/uploads".into(),
        line_channel_secret: None,
        facebook_app_secret: None,
        facebook_verify_token: None,
        liff_id: None,
        line_bot_id: None,
        line_channel_access_token: None,
    }
}
