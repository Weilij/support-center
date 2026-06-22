# Shopee Auth + Signing Foundation (Track B4a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a tested `ShopeeClient` foundation — v2 request signing, OAuth token get/refresh, and a per-shop encrypted token store with auto-refresh — without the gated Chat API.

**Architecture:** A new `src/domain/shopee/` module (`sign`/`client`/`store`) using the existing `hmac`/`sha2` and `crypto` helpers and a module-local `reqwest::Client`; an OAuth callback route; and a recognized (`"shopee"` → not-supported) gateway arm.

**Tech Stack:** Rust, axum, sqlx, reqwest, hmac+sha2, serde_json, chrono.

**Spec:** `docs/superpowers/specs/2026-06-23-b4a-shopee-foundation-design.md`

---

## File Structure
- `backend/src/domain/shopee/{mod.rs,sign.rs,client.rs,store.rs}` — the foundation.
- `backend/src/domain/mod.rs` — `pub mod shopee;`.
- `backend/src/config.rs` — Shopee config fields.
- `backend/migrations/0014_shopee_shops.sql` — token table.
- `backend/src/domain/conversations/channels.rs` — `"shopee"` not-supported arm.
- `backend/src/app.rs` — mount `shopee::routes`.

---

## Task 1: Signing (`sign.rs`, pure TDD)

**Files:**
- Create: `backend/src/domain/shopee/mod.rs`, `backend/src/domain/shopee/sign.rs`
- Modify: `backend/src/domain/mod.rs`

- [ ] **Step 1: Register the module (so the crate compiles)**

In `backend/src/domain/mod.rs`, add (alphabetical-ish, near `sessions`/`system`): `pub mod shopee;`
Create `backend/src/domain/shopee/mod.rs`:
```rust
//! Shopee Open Platform v2 integration foundation (Track B4a): request signing,
//! OAuth token lifecycle, and per-shop encrypted token storage. The gated
//! SellerChat inbound/outbound land in B4b/B4c on top of this.
pub mod sign;
```

- [ ] **Step 2: Write the failing signing tests**

Create `backend/src/domain/shopee/sign.rs` with ONLY the test module first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_base_string_is_partner_path_timestamp() {
        assert_eq!(base_string(1, "/api/v2/auth/token/get", 1610000000, None, None),
            "1/api/v2/auth/token/get1610000000");
    }

    #[test]
    fn shop_base_string_appends_token_and_shop() {
        assert_eq!(base_string(1, "/api/v2/x", 1610000000, Some("ACCESS"), Some(42)),
            "1/api/v2/x1610000000ACCESS42");
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
```
Run `cd backend && cargo test --lib shopee::sign 2>&1 | tail -15` → FAIL (functions missing).

- [ ] **Step 3: Implement `base_string` + `sign`**

Prepend to `sign.rs` (above the test module):
```rust
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
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
```
Run `cd backend && cargo test --lib shopee::sign 2>&1 | tail -10` → PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add backend/src/domain/mod.rs backend/src/domain/shopee/mod.rs backend/src/domain/shopee/sign.rs
git commit -m "feat(shopee): v2 request signing (base string + HMAC-SHA256)"
```

---

## Task 2: Config + `ShopeeClient`

**Files:**
- Modify: `backend/src/config.rs`
- Create: `backend/src/domain/shopee/client.rs`
- Modify: `backend/src/domain/shopee/mod.rs`

- [ ] **Step 1: Add Shopee config**

In `backend/src/config.rs`, add fields (near the other platform fields):
```rust
    /// Shopee Open Platform partner id (SHOPEE_PARTNER_ID).
    pub shopee_partner_id: Option<i64>,
    /// Shopee Open Platform partner key / app secret (SHOPEE_PARTNER_KEY).
    pub shopee_partner_key: Option<String>,
    /// Shopee API host (SHOPEE_HOST), e.g. https://partner.shopeemobile.com.
    pub shopee_host: Option<String>,
```
In the constructor:
```rust
            shopee_partner_id: std::env::var("SHOPEE_PARTNER_ID").ok().and_then(|s| s.parse().ok()),
            shopee_partner_key: std::env::var("SHOPEE_PARTNER_KEY").ok().filter(|s| !s.is_empty()),
            shopee_host: std::env::var("SHOPEE_HOST").ok().filter(|s| !s.is_empty()),
```
In `test_config()` add: `shopee_partner_id: None,`, `shopee_partner_key: None,`, `shopee_host: None,`.

- [ ] **Step 2: Write the failing client builder tests**

Create `backend/src/domain/shopee/client.rs` with the test module first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> ShopeeClient {
        ShopeeClient { partner_id: 1, partner_key: "key".into(), host: "https://h".into() }
    }

    #[test]
    fn signed_query_public_has_no_token() {
        let q = client().signed_query("/api/v2/auth/token/get", 1610000000, None, None);
        assert!(q.starts_with("partner_id=1&timestamp=1610000000&sign="));
        assert!(!q.contains("access_token"));
    }

    #[test]
    fn signed_query_shop_has_token_and_shop() {
        let q = client().signed_query("/api/v2/x", 1610000000, Some("ACCESS"), Some(42));
        assert!(q.contains("access_token=ACCESS"));
        assert!(q.contains("shop_id=42"));
        assert!(q.contains("sign="));
    }

    #[test]
    fn url_joins_host_path_query() {
        assert_eq!(client().url("/api/v2/x", "a=1"), "https://h/api/v2/x?a=1");
    }

    #[test]
    fn from_config_requires_partner_id_and_key() {
        let mut c = crate::config::test_config();
        assert!(ShopeeClient::from_config(&c).is_none());
        c.shopee_partner_id = Some(1);
        c.shopee_partner_key = Some("k".into());
        let cl = ShopeeClient::from_config(&c).unwrap();
        assert_eq!(cl.host, "https://partner.shopeemobile.com"); // default applied
    }
}
```
Run `cd backend && cargo test --lib shopee::client 2>&1 | tail -15` → FAIL.

- [ ] **Step 3: Implement `client.rs`**

Prepend to `client.rs`:
```rust
//! Shopee Open Platform v2 client: signed URLs + OAuth token endpoints.

use std::sync::OnceLock;

use serde_json::json;

use super::sign::{base_string, sign};
use crate::config::Config;

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client")
    })
}

pub struct ShopeeClient {
    pub partner_id: i64,
    pub partner_key: String,
    pub host: String,
}

/// Parsed token response (Shopee returns these at the top level).
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expire_in: i64,
}

impl ShopeeClient {
    pub fn from_config(config: &Config) -> Option<Self> {
        match (config.shopee_partner_id, config.shopee_partner_key.clone()) {
            (Some(partner_id), Some(partner_key)) if !partner_key.is_empty() => Some(Self {
                partner_id,
                partner_key,
                host: config
                    .shopee_host
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "https://partner.shopeemobile.com".into()),
            }),
            _ => None,
        }
    }

    /// Query string `partner_id&timestamp&sign[&access_token&shop_id]`.
    pub fn signed_query(&self, path: &str, timestamp: i64, access_token: Option<&str>, shop_id: Option<i64>) -> String {
        let base = base_string(self.partner_id, path, timestamp, access_token, shop_id);
        let s = sign(&self.partner_key, &base);
        let mut q = format!("partner_id={}&timestamp={}&sign={}", self.partner_id, timestamp, s);
        if let (Some(tok), Some(shop)) = (access_token, shop_id) {
            q.push_str(&format!("&access_token={tok}&shop_id={shop}"));
        }
        q
    }

    pub fn url(&self, path: &str, query: &str) -> String {
        format!("{}{}?{}", self.host, path, query)
    }

    async fn post_token(&self, path: &str, body: serde_json::Value) -> Result<TokenResponse, String> {
        let ts = chrono::Utc::now().timestamp();
        let url = self.url(path, &self.signed_query(path, ts, None, None));
        let resp = http_client()
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Shopee request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Err(format!("Shopee token call failed ({status}): {txt}"));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| format!("bad token JSON: {e}"))?;
        let access_token = v["access_token"].as_str().ok_or_else(|| format!("no access_token: {v}"))?.to_string();
        let refresh_token = v["refresh_token"].as_str().unwrap_or_default().to_string();
        let expire_in = v["expire_in"].as_i64().unwrap_or(14400);
        Ok(TokenResponse { access_token, refresh_token, expire_in })
    }

    /// Exchange an authorization code for tokens (`/api/v2/auth/token/get`).
    pub async fn fetch_token(&self, code: &str, shop_id: i64) -> Result<TokenResponse, String> {
        self.post_token(
            "/api/v2/auth/token/get",
            json!({ "code": code, "shop_id": shop_id, "partner_id": self.partner_id }),
        )
        .await
    }

    /// Refresh an access token (`/api/v2/auth/access_token/get`).
    pub async fn refresh(&self, refresh_token: &str, shop_id: i64) -> Result<TokenResponse, String> {
        self.post_token(
            "/api/v2/auth/access_token/get",
            json!({ "refresh_token": refresh_token, "shop_id": shop_id, "partner_id": self.partner_id }),
        )
        .await
    }
}
```
Add `pub mod client;` to `backend/src/domain/shopee/mod.rs`.

- [ ] **Step 4: Tests + build**

`cd backend && cargo test --lib shopee::client 2>&1 | tail -10` → 4 passing.
`cd backend && cargo build 2>&1 | tail -5` → success.

- [ ] **Step 5: Commit**

```bash
git add backend/src/config.rs backend/src/domain/shopee/client.rs backend/src/domain/shopee/mod.rs
git commit -m "feat(shopee): config + ShopeeClient (signed URLs + OAuth token endpoints)"
```

---

## Task 3: Token store + migration + OAuth callback + gateway arm

**Files:**
- Create: `backend/migrations/0014_shopee_shops.sql`, `backend/src/domain/shopee/store.rs`
- Modify: `backend/src/domain/shopee/mod.rs` (store + `routes`), `backend/src/app.rs`, `backend/src/domain/conversations/channels.rs`

- [ ] **Step 1: Migration for the token table**

Create `backend/migrations/0014_shopee_shops.sql`:
```sql
CREATE TABLE shopee_shops (
    shop_id       BIGINT PRIMARY KEY,
    access_token  TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    expires_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
```

- [ ] **Step 2: Write the failing store tests**

Create `backend/src/domain/shopee/store.rs` with the test module first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_refresh_true_within_buffer() {
        // expires in 60s, buffer 300s → refresh now
        assert!(needs_refresh("2030-01-01T00:01:00Z", "2030-01-01T00:00:00Z", 300));
    }

    #[test]
    fn needs_refresh_false_when_fresh() {
        // expires in 1h, buffer 300s → no refresh
        assert!(!needs_refresh("2030-01-01T01:00:00Z", "2030-01-01T00:00:00Z", 300));
    }

    #[test]
    fn needs_refresh_false_on_unparseable() {
        assert!(!needs_refresh("not-a-date", "2030-01-01T00:00:00Z", 300));
    }
}
```
Run `cd backend && cargo test --lib shopee::store 2>&1 | tail -15` → FAIL.

- [ ] **Step 3: Implement `store.rs`**

Prepend to `store.rs`:
```rust
//! Per-shop Shopee token storage (encrypted) + refresh-before-expiry logic.

use sqlx::PgPool;

use super::client::ShopeeClient;
use crate::crypto;
use crate::db::now_iso;

pub struct ShopTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
}

/// Refresh when `now + buffer >= expires_at`. Unparseable timestamps → false
/// (don't thrash on bad data; the live call will surface a real error).
pub fn needs_refresh(expires_at_iso: &str, now_iso: &str, buffer_secs: i64) -> bool {
    let (Ok(exp), Ok(now)) = (
        chrono::DateTime::parse_from_rfc3339(expires_at_iso),
        chrono::DateTime::parse_from_rfc3339(now_iso),
    ) else {
        return false;
    };
    now.timestamp() + buffer_secs >= exp.timestamp()
}

/// Upsert encrypted tokens for a shop.
pub async fn save_tokens(
    db: &PgPool,
    enc_key: Option<&str>,
    shop_id: i64,
    access: &str,
    refresh: &str,
    expires_at: &str,
) -> Result<(), String> {
    let access_enc = crypto::protect(enc_key, access).map_err(|e| e.to_string())?;
    let refresh_enc = crypto::protect(enc_key, refresh).map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO shopee_shops (shop_id, access_token, refresh_token, expires_at, updated_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (shop_id) DO UPDATE SET
            access_token = EXCLUDED.access_token,
            refresh_token = EXCLUDED.refresh_token,
            expires_at = EXCLUDED.expires_at,
            updated_at = EXCLUDED.updated_at",
    )
    .bind(shop_id)
    .bind(&access_enc)
    .bind(&refresh_enc)
    .bind(expires_at)
    .bind(now_iso())
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Load + decrypt a shop's tokens.
pub async fn load(db: &PgPool, enc_key: Option<&str>, shop_id: i64) -> Result<Option<ShopTokens>, String> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT access_token, refresh_token, expires_at FROM shopee_shops WHERE shop_id = $1",
    )
    .bind(shop_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())?;
    let Some((a, r, exp)) = row else { return Ok(None) };
    Ok(Some(ShopTokens {
        access_token: crypto::reveal(enc_key, &a).map_err(|e| e.to_string())?,
        refresh_token: crypto::reveal(enc_key, &r).map_err(|e| e.to_string())?,
        expires_at: exp,
    }))
}

/// Return a usable access token, refreshing (and persisting) when near expiry.
pub async fn valid_access_token(
    db: &PgPool,
    client: &ShopeeClient,
    enc_key: Option<&str>,
    shop_id: i64,
) -> Result<String, String> {
    let tokens = load(db, enc_key, shop_id).await?.ok_or("Shopee shop is not connected")?;
    if needs_refresh(&tokens.expires_at, &now_iso(), 300) {
        let fresh = client.refresh(&tokens.refresh_token, shop_id).await?;
        let new_refresh = if fresh.refresh_token.is_empty() { tokens.refresh_token } else { fresh.refresh_token };
        let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(fresh.expire_in)).to_rfc3339();
        save_tokens(db, enc_key, shop_id, &fresh.access_token, &new_refresh, &expires_at).await?;
        Ok(fresh.access_token)
    } else {
        Ok(tokens.access_token)
    }
}
```
Add `pub mod store;` to `backend/src/domain/shopee/mod.rs`.
Run `cd backend && cargo test --lib shopee::store 2>&1 | tail -10` → 3 passing. `cargo build` → success.

- [ ] **Step 4: OAuth callback route**

In `backend/src/domain/shopee/mod.rs`, add the route + handler (the file currently only declares submodules — add the imports and `routes`):
```rust
pub mod client;
pub mod sign;
pub mod store;

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde_json::json;

use crate::state::AppState;

/// GET /api/shopee/auth/callback?code=&shop_id= — exchange the OAuth code for
/// tokens and persist them (per-shop). The only B4a HTTP surface.
async fn auth_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let code = params.get("code").map(String::as_str).unwrap_or_default();
    let shop_id = params.get("shop_id").and_then(|s| s.parse::<i64>().ok());
    if code.is_empty() || shop_id.is_none() {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"success": false, "error": "code and shop_id are required"}))).into_response();
    }
    let shop_id = shop_id.unwrap();
    let Some(client) = client::ShopeeClient::from_config(&state.config) else {
        return (axum::http::StatusCode::NOT_IMPLEMENTED, Json(json!({"success": false, "error": "Shopee is not configured"}))).into_response();
    };
    match client.fetch_token(code, shop_id).await {
        Ok(t) => {
            let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(t.expire_in)).to_rfc3339();
            match store::save_tokens(&state.db, state.config.encryption_key.as_deref(), shop_id, &t.access_token, &t.refresh_token, &expires_at).await {
                Ok(()) => Json(json!({"success": true, "shopId": shop_id})).into_response(),
                Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"success": false, "error": e}))).into_response(),
            }
        }
        Err(e) => (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"success": false, "error": e}))).into_response(),
    }
}

pub fn routes(_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new().route("/api/shopee/auth/callback", get(auth_callback))
}
```
Mount it in `backend/src/app.rs` next to the other `.merge(crate::domain::…::routes(state.clone()))` lines:
```rust
        .merge(crate::domain::shopee::routes(state.clone()))
```

- [ ] **Step 5: Gateway recognition arm**

In `backend/src/domain/conversations/channels.rs::OutboundGateway::send_batch`, add before the `other =>` arm:
```rust
            "shopee" => Err("Outbound delivery is not supported for platform 'shopee'".into()),
```
(B4c will replace this with a real `shopee_send` once the gated send spec is available.)

- [ ] **Step 6: Callback validation test**

In `backend/tests/` add (or extend an existing system/integration test file) a test that the callback returns 400 when `code`/`shop_id` are missing:
```rust
// in a #[tokio::test] using spawn_app():
let (status, _, _) = app.request("GET", "/api/shopee/auth/callback", None, None).await;
assert_eq!(status, StatusCode::BAD_REQUEST);
```
(Place it in a small new `backend/tests/shopee.rs` with `mod common;` + `use common::spawn_app;`, mirroring other integration test files. The live token fetch is not exercised — no Shopee creds in tests.)

- [ ] **Step 7: Build + suites**

- `cd backend && cargo build 2>&1 | tail -5` → success.
- `cd backend && cargo build --tests 2>&1 | tail -5` → success.
- `cd backend && cargo test --lib shopee 2>&1 | grep -E "test result"` → green (sign+client+store units).
- `cd backend && cargo test --test shopee 2>&1 | grep -E "test result|error\[|FAILED"` → green (callback 400 test).
- `cd backend && cargo test 2>&1 | grep -E "test result|error\[" | tail -30` → all suites green (the migration applies; gateway arm doesn't change existing platforms).

- [ ] **Step 8: Commit**

```bash
git add backend/migrations/0014_shopee_shops.sql backend/src/domain/shopee/store.rs backend/src/domain/shopee/mod.rs backend/src/app.rs backend/src/domain/conversations/channels.rs backend/tests/shopee.rs
git commit -m "feat(shopee): encrypted per-shop token store + OAuth callback + gateway arm"
```

---

## Final verification (after all tasks)

- [ ] `cd backend && cargo build` + `cargo build --tests` — clean
- [ ] `cd backend && cargo test` — all suites green (shopee sign/client/store units + callback test + existing suites; migration 0014 applies)
- [ ] `detect_changes()` before the final commit — the only outbound change is a recognized `"shopee"` not-supported arm; the foundation is otherwise self-contained. Live OAuth/token refresh needs real partner credentials (deferred); SellerChat inbound/outbound are B4b/B4c.
```
