# Shopee Auth + Signing Foundation (Track B4a) — Design Spec

**Date:** 2026-06-23
**Track:** B4a (backend; first sub-project of Shopee / Track B4)
**Status:** design approved, pending written-spec review

---

## 0. Context

Shopee is **unlike** LINE/FB/IG: it's the Shopee Open Platform v2, with OAuth (per-shop `access_token` + `refresh_token`), a custom HMAC-SHA256 request signature on every call, and its own region-specific Chat API. None of the Meta/LINE outbound/webhook code reuses. The Chat (SellerChat) API itself is **partner-gated** — its webhook-push and send-message payload shapes are not in public docs and `open.shopee.com` is not fetchable from this environment.

**Decision:** build the **auth + signing + token foundation now** (all publicly confirmed), as a clean `ShopeeClient` that B4b (inbound webhook) and B4c (outbound send) plug into once the gated Chat docs are available. We deliberately do **not** ship a speculative webhook endpoint or guessed Chat payload mapping in B4a.

### Publicly confirmed (sources)
- **Signature (v2):** base string = `partner_id + path + timestamp` for public/auth calls, and `partner_id + path + timestamp + access_token + shop_id` for shop-scoped calls; `sign = hex(HMAC_SHA256(partner_key, base_string))`; request carries `partner_id`, `timestamp`, `sign`, and (for shop calls) `access_token`, `shop_id` in the query string. (Shopee TW Open API authorization PDF; community SDKs.)
- **OAuth token endpoints:** `POST /api/v2/auth/token/get` (`code` → `access_token`+`refresh_token`); `POST /api/v2/auth/access_token/get` (`refresh_token` → new `access_token`). `access_token` valid **4h** (`expire_in`=14400s); `refresh_token` valid **30 days**. Response includes `access_token`, `refresh_token`, `expire_in`, `shop_id`.
- **Host:** region-specific, e.g. `https://partner.shopeemobile.com`.

---

## 1. Goal & non-goals

**Goal:** a tested `ShopeeClient` that signs v2 requests correctly, performs the OAuth code→token and refresh flows, and stores per-shop tokens (encrypted) with automatic refresh before expiry — the reusable base for B4b/B4c.

**Non-goals (deferred to B4b/B4c, gated):** the SellerChat inbound webhook (route + push-signature verification + payload→`Normalized` mapping) and the outbound send-message call (endpoint path + body). No live OAuth UI flow polish. The `OutboundGateway` gains a `"shopee"` arm that returns "not supported" until B4c wires `shopee_send`.

---

## 2. New module: `src/domain/shopee/`

Register `pub mod shopee;` in `src/domain/mod.rs`.

### 2a. `sign.rs` (pure — unit-tested)
```rust
pub fn base_string(partner_id: i64, path: &str, timestamp: i64, access_token: Option<&str>, shop_id: Option<i64>) -> String
// public/auth: format!("{partner_id}{path}{timestamp}")
// shop call:   format!("{partner_id}{path}{timestamp}{access_token}{shop_id}")
pub fn sign(partner_key: &str, base: &str) -> String  // hex HMAC-SHA256 (reuse the hmac/sha2 deps already in the crate)
```
Tests assert the **exact base-string composition** for both public and shop forms (the highest-risk bug surface — field order/content), plus `sign` determinism + 64-char hex. (When an official test vector is available, add an equality assertion.)

### 2b. `client.rs`
```rust
pub struct ShopeeClient { partner_id: i64, partner_key: String, host: String }
impl ShopeeClient {
    pub fn from_config(config: &Config) -> Option<Self>;   // Some only when partner_id + partner_key set
    pub fn signed_query(&self, path: &str, timestamp: i64, access_token: Option<&str>, shop_id: Option<i64>) -> String; // partner_id&timestamp&sign[&access_token&shop_id]
    pub fn url(&self, path: &str, query: &str) -> String;  // host + path + "?" + query
    pub async fn fetch_token(&self, code: &str, shop_id: i64) -> Result<TokenResponse, String>;     // POST /api/v2/auth/token/get
    pub async fn refresh(&self, refresh_token: &str, shop_id: i64) -> Result<TokenResponse, String>;// POST /api/v2/auth/access_token/get
}
pub struct TokenResponse { access_token: String, refresh_token: String, expire_in: i64 } // parsed from JSON
```
The URL/query builders are pure (unit-tested); the two HTTP calls use a shared `reqwest::Client` (a `OnceLock` in this module, mirroring `channels::http_client`) with a 10s timeout. JSON parsing is defensive (missing fields → `Err`).

### 2c. `store.rs` (per-shop encrypted token store + refresh logic)
- Migration `migrations/0014_shopee_shops.sql`:
  ```sql
  CREATE TABLE shopee_shops (
      shop_id       BIGINT PRIMARY KEY,
      access_token  TEXT NOT NULL,   -- encrypted (crypto::protect)
      refresh_token TEXT NOT NULL,   -- encrypted
      expires_at    TEXT NOT NULL,   -- ISO; when the access token expires
      updated_at    TEXT NOT NULL
  );
  ```
- `pub async fn save_tokens(db, enc_key: Option<&str>, shop_id, access, refresh, expires_at)` — encrypts via `crate::crypto::protect`, upserts.
- `pub async fn load(db, enc_key, shop_id) -> Option<ShopTokens>` — decrypts via `crate::crypto::reveal`.
- `pub fn needs_refresh(expires_at_iso: &str, now_iso: &str, buffer_secs: i64) -> bool` — **pure, unit-tested** (refresh when `now >= expires_at - buffer`).
- `pub async fn valid_access_token(db, client: &ShopeeClient, enc_key, shop_id) -> Result<String, String>` — load; if `needs_refresh` (5-min buffer) → `client.refresh(...)` → `save_tokens(...)` → return new; else return the stored token.

### 2d. OAuth callback (thin glue, real)
A route `GET /api/shopee/auth/callback?code=&shop_id=` (mounted in `shopee::routes`): calls `client.fetch_token`, computes `expires_at = now + expire_in`, `save_tokens(...)`, returns a small JSON ok. This is the only new HTTP surface in B4a and uses only the confirmed token endpoint. (No signature is required on Shopee's redirect back; we validate `shop_id`/`code` presence.)

---

## 3. Gateway recognition (minimal)

In `conversations/channels.rs::OutboundGateway::send_batch`, add a `"shopee"` arm returning `Err("Outbound delivery is not supported for platform 'shopee'".into())` (so the platform is recognized; the real send lands in B4c). No token field yet — B4c adds `shopee_send` using `ShopeeClient` + the gated send spec.

---

## 4. Config (`config.rs`)

Add: `shopee_partner_id: Option<i64>` (`SHOPEE_PARTNER_ID`), `shopee_partner_key: Option<String>` (`SHOPEE_PARTNER_KEY`), `shopee_host: Option<String>` (`SHOPEE_HOST`, default `https://partner.shopeemobile.com` applied in `ShopeeClient::from_config`). Add all to `test_config()` as `None`.

---

## 5. Files

- `backend/src/config.rs` — Shopee config fields.
- `backend/src/domain/mod.rs` — `pub mod shopee;`.
- `backend/src/domain/shopee/{mod.rs,sign.rs,client.rs,store.rs}` — the foundation.
- `backend/migrations/0014_shopee_shops.sql` — token table.
- `backend/src/domain/conversations/channels.rs` — `"shopee"` not-supported arm.
- `backend/src/app.rs` (or the router) — mount `shopee::routes`.
- Tests: unit (`base_string` both forms, `sign` format/determinism, `needs_refresh`, `signed_query`/`url` builders, encrypted store round-trip) + a route test for the callback's input validation.

---

## 6. Verification

- `cargo build` + `cargo build --tests` + `cargo test` green. The signing/refresh/builder/store units are network-free; the callback's live token fetch is exercised only with real credentials (deferred).
- The migration applies on the per-test DB.
- `detect_changes()` before commit.

---

## 7. Resolved decisions
- Build **B4a foundation only** (auth + signing + token store/refresh); the gated SellerChat inbound/outbound are **B4b/B4c**, completed once the docs are available (via a logged-in browser session or pasted content).
- Token storage: a dedicated **`shopee_shops`** table with `crypto::protect`-encrypted tokens (consistent with the channels credential encryption), keyed by `shop_id`.
- Signing tested by **base-string composition + format/determinism** now; add an official test vector when available.
- `OutboundGateway` gets a recognized `"shopee"` arm returning "not supported" until B4c.
