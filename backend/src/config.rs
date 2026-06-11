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
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();
        Self {
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://data/mcss.db?mode=rwc".into()),
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
        database_url: "sqlite::memory:".into(),
        jwt_secret: "test-secret".into(),
        encryption_key: None,
        environment: "development".into(),
        frontend_url: None,
        backend_url: None,
        public_storage_url: None,
        extra_origins: vec![],
        port: 0,
        upload_dir: "data/uploads".into(),
    }
}
