#![allow(dead_code)]

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

use mcss_backend::config::Config;
use mcss_backend::state::AppState;
use mcss_backend::{app, db};

pub struct TestApp {
    pub router: Router,
    pub state: Arc<AppState>,
    _dir: tempfile::TempDir,
}

pub async fn spawn_app() -> TestApp {
    spawn_app_with_env("development").await
}

pub async fn spawn_app_with_env(environment: &str) -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = db::init_pool(&url).await.expect("db init");
    let config = Config {
        database_url: url,
        jwt_secret: "test-secret".into(),
        encryption_key: None,
        environment: environment.into(),
        frontend_url: None,
        backend_url: None,
        public_storage_url: None,
        extra_origins: vec![],
        port: 0,
    };
    let state = AppState::new(pool, config);
    TestApp { router: app::build_router(state.clone()), state, _dir: dir }
}

impl TestApp {
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value, HeaderMap) {
        self.request_with_headers(method, path, token, body, &[]).await
    }

    pub async fn request_with_headers(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
        extra_headers: &[(&str, &str)],
    ) -> (StatusCode, Value, HeaderMap) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }
        for (k, v) in extra_headers {
            builder = builder.header(*k, *v);
        }
        let request = if let Some(b) = body {
            builder
                .header("Content-Type", "application/json")
                .body(Body::from(b.to_string()))
                .unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = self.router.clone().oneshot(request).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json, headers)
    }

    /// Insert an agent directly and return its id.
    pub async fn seed_agent(&self, email: &str, password: &str, role: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let hash = mcss_backend::domain::auth::store::hash_password(password).unwrap();
        sqlx::query(
            "INSERT INTO agents (id, email, password_hash, display_name, role, is_active, created_at)
             VALUES (?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(&id)
        .bind(email)
        .bind(hash)
        .bind(format!("{role} user"))
        .bind(role)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.state.db)
        .await
        .unwrap();
        id
    }

    pub async fn seed_team(&self, name: &str) -> i64 {
        sqlx::query("INSERT INTO teams (name, created_at) VALUES (?, ?)")
            .bind(name)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&self.state.db)
            .await
            .unwrap()
            .last_insert_rowid()
    }

    pub async fn add_membership(&self, agent_id: &str, team_id: i64, role: &str, primary: bool) {
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(agent_id)
        .bind(team_id)
        .bind(role)
        .bind(primary as i64)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.state.db)
        .await
        .unwrap();
    }

    /// Login and return (accessToken, refreshToken, sessionId).
    pub async fn login(&self, email: &str, password: &str) -> (String, String, String) {
        let (status, body, _) = self
            .request(
                "POST",
                "/api/auth/login",
                None,
                Some(serde_json::json!({"email": email, "password": password})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "login failed: {body}");
        (
            body["data"]["token"].as_str().unwrap().to_string(),
            body["data"]["refreshToken"].as_str().unwrap().to_string(),
            body["data"]["sessionId"].as_str().unwrap().to_string(),
        )
    }
}
