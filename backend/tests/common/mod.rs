#![allow(dead_code)]

pub mod ws;

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
    let env = environment.to_string();
    spawn_app_custom(move |c| c.environment = env).await
}

/// Spawn an app after applying a configuration customization (e.g. setting an
/// encryption key or clearing a webhook secret).
/// Admin connection for creating per-test databases. Override with
/// TEST_DATABASE_ADMIN_URL (CI uses a service container).
fn admin_url() -> String {
    std::env::var("TEST_DATABASE_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://localhost/postgres".into())
}

/// Drop leftover databases from previous runs, once per test binary.
/// In-use databases (current run) refuse the drop and are skipped.
async fn sweep_stale_test_dbs(admin: &sqlx::PgPool) {
    static ONCE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    ONCE.get_or_init(|| async {
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT datname FROM pg_database WHERE datname LIKE 'mcss_test_%'",
        )
        .fetch_all(admin)
        .await
        .unwrap_or_default();
        for name in names {
            let _ = sqlx::query(&format!("DROP DATABASE \"{name}\"")).execute(admin).await;
        }
    })
    .await;
}

pub async fn spawn_app_custom(customize: impl FnOnce(&mut Config)) -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let admin = sqlx::PgPool::connect(&admin_url()).await.expect("admin pg connect");
    sweep_stale_test_dbs(&admin).await;
    let db_name = format!("mcss_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin)
        .await
        .expect("create test db");
    let base = admin_url();
    let url = format!("{}/{db_name}", base.rsplit_once('/').map(|(b, _)| b).unwrap_or(&base));
    let pool = db::init_pool(&url).await.expect("db init");
    let mut config = Config {
        database_url: url,
        jwt_secret: "test-secret".into(),
        encryption_key: None,
        environment: "development".into(),
        frontend_url: None,
        backend_url: None,
        public_storage_url: None,
        extra_origins: vec![],
        trusted_proxies: vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()],
        port: 0,
        upload_dir: dir.path().join("uploads").display().to_string(),
        // Webhook signature secrets default to known test values so suites can
        // sign payloads; individual tests override via spawn_app_custom.
        line_channel_secret: Some("test-line-secret".into()),
        liff_id: Some("test-liff-id".into()),
        line_bot_id: Some("@testbot".into()),
        line_channel_access_token: None,
        facebook_app_secret: Some("test-fb-secret".into()),
        facebook_verify_token: Some("test-verify-token".into()),
        facebook_page_access_token: None,
        instagram_access_token: None,
        file_signing_secret: None,
        shopee_partner_id: None,
        shopee_partner_key: None,
        shopee_host: None,
    };
    customize(&mut config);
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

    /// Send a request whose body is a raw (possibly malformed) string with a JSON
    /// content type.
    pub async fn request_raw(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        raw_body: &str,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("Content-Type", "application/json")
            .header("Content-Length", raw_body.len().to_string());
        if let Some(t) = token {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }
        let request = builder.body(Body::from(raw_body.to_string())).unwrap();
        let resp = self.router.clone().oneshot(request).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }

    /// Insert an agent directly and return its id.
    pub async fn seed_agent(&self, email: &str, password: &str, role: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let hash = mcss_backend::domain::auth::store::hash_password(password).unwrap();
        sqlx::query(
            "INSERT INTO agents (id, email, password_hash, display_name, role, is_active, created_at)
             VALUES ($1, $2, $3, $4, $5, 1, $6)",
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
        sqlx::query_scalar::<_, i64>("INSERT INTO teams (name, created_at) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(chrono::Utc::now().to_rfc3339())
            .fetch_one(&self.state.db)
            .await
            .unwrap()
            
    }

    pub async fn add_membership(&self, agent_id: &str, team_id: i64, role: &str, primary: bool) {
        sqlx::query(
            "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at) VALUES ($1, $2, $3, $4, $5)",
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

    /// Insert a customer directly and return its numeric id.
    pub async fn seed_customer(
        &self,
        platform: &str,
        platform_user_id: &str,
        display_name: &str,
        team_id: Option<i64>,
    ) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO customers (platform, platform_user_id, display_name, source_team_id, created_at)
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(platform)
        .bind(platform_user_id)
        .bind(display_name)
        .bind(team_id)
        .bind(chrono::Utc::now().to_rfc3339())
        .fetch_one(&self.state.db)
        .await
        .unwrap()
        
    }

    /// Insert an active, global tag directly and return its id.
    pub async fn seed_tag(&self, name: &str, created_by: &str) -> i64 {
        self.seed_tag_full(name, created_by, None, true).await
    }

    /// Insert a tag with explicit team scope and active flag.
    pub async fn seed_tag_full(
        &self,
        name: &str,
        created_by: &str,
        team_id: Option<i64>,
        is_active: bool,
    ) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO tags (name, color, description, team_id, is_active, created_by, created_at)
             VALUES ($1, '#3B82F6', NULL, $2, $3, $4, $5) RETURNING id",
        )
        .bind(name)
        .bind(team_id)
        .bind(is_active as i64)
        .bind(created_by)
        .bind(chrono::Utc::now().to_rfc3339())
        .fetch_one(&self.state.db)
        .await
        .unwrap()
        
    }

    /// Insert a conversation directly and return its id.
    pub async fn seed_conversation(
        &self,
        customer_id: i64,
        team_id: Option<i64>,
        status: &str,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO conversations (id, customer_id, team_id, status, created_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&id)
        .bind(customer_id)
        .bind(team_id)
        .bind(status)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.state.db)
        .await
        .unwrap();
        id
    }

    /// Insert a message directly and return its id. `sender_type` is
    /// customer|agent|system; `created_at` defaults to now.
    pub async fn seed_message(
        &self,
        conversation_id: &str,
        sender_type: &str,
        content: &str,
        created_at: Option<&str>,
    ) -> String {
        self.seed_message_full(conversation_id, sender_type, content, created_at, None, None).await
    }

    /// Insert a message with explicit session linkage.
    pub async fn seed_message_full(
        &self,
        conversation_id: &str,
        sender_type: &str,
        content: &str,
        created_at: Option<&str>,
        session_id: Option<&str>,
        session_seq: Option<i64>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let customer_id: Option<i64> = if sender_type == "customer" {
            sqlx::query_scalar("SELECT customer_id FROM conversations WHERE id = $1")
                .bind(conversation_id)
                .fetch_optional(&self.state.db)
                .await
                .unwrap()
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, customer_id, content,
                                   content_type, session_id, session_seq, created_at)
             VALUES ($1, $2, $3, $4, $5, 'text', $6, $7, $8)",
        )
        .bind(&id)
        .bind(conversation_id)
        .bind(sender_type)
        .bind(customer_id)
        .bind(content)
        .bind(session_id)
        .bind(session_seq)
        .bind(
            created_at
                .map(str::to_string)
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        )
        .execute(&self.state.db)
        .await
        .unwrap();
        id
    }

    /// Insert a conversation session directly and return its id.
    #[allow(clippy::too_many_arguments)]
    pub async fn seed_session(
        &self,
        conversation_id: &str,
        is_active: bool,
        topic: Option<&str>,
        started_at: Option<&str>,
        last_activity_at: Option<&str>,
        message_count: i64,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        sqlx::query(
            "INSERT INTO conversation_sessions
                 (id, conversation_id, session_type, topic, started_at, ended_at,
                  last_activity_at, message_count, is_active, created_at)
             VALUES ($1, $2, 'continuous', $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&id)
        .bind(conversation_id)
        .bind(topic)
        .bind(started_at.map(str::to_string).unwrap_or_else(|| now.clone()))
        .bind(if is_active { None } else { Some(now.clone()) })
        .bind(last_activity_at.map(str::to_string).unwrap_or_else(|| now.clone()))
        .bind(message_count)
        .bind(is_active as i64)
        .bind(&now)
        .execute(&self.state.db)
        .await
        .unwrap();
        id
    }

    /// Attach a tag to a customer directly.
    pub async fn add_customer_tag(&self, customer_id: i64, tag_id: i64, assigned_by: &str) {
        sqlx::query(
            "INSERT INTO customer_tags (customer_id, tag_id, assigned_by, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(customer_id)
        .bind(tag_id)
        .bind(assigned_by)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.state.db)
        .await
        .unwrap();
    }

    /// Insert an audit-trail entry directly and return its numeric id. Actor name and
    /// role are derived from the agents table when present.
    pub async fn seed_activity(
        &self,
        agent_id: &str,
        action: &str,
        resource_type: &str,
        resource_id: Option<&str>,
        details: Option<Value>,
        created_at: Option<&str>,
    ) -> i64 {
        let actor: Option<(String, String)> =
            sqlx::query_as("SELECT display_name, role FROM agents WHERE id = $1")
                .bind(agent_id)
                .fetch_optional(&self.state.db)
                .await
                .unwrap();
        let (name, role) =
            actor.unwrap_or_else(|| ("seed user".to_string(), "agent".to_string()));
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO activity_logs (agent_id, agent_name, agent_role, action, resource_type, resource_id, details, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        )
        .bind(agent_id)
        .bind(name)
        .bind(role)
        .bind(action)
        .bind(resource_type)
        .bind(resource_id)
        .bind(details.map(|d| d.to_string()))
        .bind(created_at.map(str::to_string).unwrap_or_else(|| {
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        }))
        .fetch_one(&self.state.db)
        .await
        .unwrap()
        
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
