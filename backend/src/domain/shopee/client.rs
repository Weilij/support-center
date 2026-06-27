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

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Shopee request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Shopee token call failed ({status}): {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("bad token JSON: {0}")]
    TokenJson(reqwest::Error),
    #[error("no access_token: {0}")]
    MissingAccessToken(serde_json::Value),
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
    pub fn signed_query(
        &self,
        path: &str,
        timestamp: i64,
        access_token: Option<&str>,
        shop_id: Option<i64>,
    ) -> String {
        let base = base_string(self.partner_id, path, timestamp, access_token, shop_id);
        let s = sign(&self.partner_key, &base);
        let mut q = format!(
            "partner_id={}&timestamp={}&sign={}",
            self.partner_id, timestamp, s
        );
        if let (Some(tok), Some(shop)) = (access_token, shop_id) {
            q.push_str(&format!("&access_token={tok}&shop_id={shop}"));
        }
        q
    }

    pub fn url(&self, path: &str, query: &str) -> String {
        format!("{}{}?{}", self.host, path, query)
    }

    pub fn authorization_url(&self, redirect_url: &str, timestamp: i64) -> String {
        let path = "/api/v2/shop/auth_partner";
        let base = base_string(self.partner_id, path, timestamp, None, None);
        let s = sign(&self.partner_key, &base);
        reqwest::Url::parse_with_params(
            &format!("{}{}", self.host, path),
            &[
                ("partner_id", self.partner_id.to_string()),
                ("timestamp", timestamp.to_string()),
                ("sign", s),
                ("redirect", redirect_url.to_string()),
            ],
        )
        .expect("Shopee authorization URL")
        .to_string()
    }

    async fn post_token(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<TokenResponse, ClientError> {
        let ts = chrono::Utc::now().timestamp();
        let url = self.url(path, &self.signed_query(path, ts, None, None));
        let resp = http_client().post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            return Err(ClientError::Http { status, body: txt });
        }
        let v: serde_json::Value = resp.json().await.map_err(ClientError::TokenJson)?;
        let access_token = v["access_token"]
            .as_str()
            .ok_or_else(|| ClientError::MissingAccessToken(v.clone()))?
            .to_string();
        let refresh_token = v["refresh_token"].as_str().unwrap_or_default().to_string();
        let expire_in = v["expire_in"].as_i64().unwrap_or(14400);
        Ok(TokenResponse {
            access_token,
            refresh_token,
            expire_in,
        })
    }

    /// Exchange an authorization code for tokens (`/api/v2/auth/token/get`).
    pub async fn fetch_token(
        &self,
        code: &str,
        shop_id: i64,
    ) -> Result<TokenResponse, ClientError> {
        self.post_token(
            "/api/v2/auth/token/get",
            json!({ "code": code, "shop_id": shop_id, "partner_id": self.partner_id }),
        )
        .await
    }

    /// Refresh an access token (`/api/v2/auth/access_token/get`).
    pub async fn refresh(
        &self,
        refresh_token: &str,
        shop_id: i64,
    ) -> Result<TokenResponse, ClientError> {
        self.post_token(
            "/api/v2/auth/access_token/get",
            json!({ "refresh_token": refresh_token, "shop_id": shop_id, "partner_id": self.partner_id }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> ShopeeClient {
        ShopeeClient {
            partner_id: 1,
            partner_key: "key".into(),
            host: "https://h".into(),
        }
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
    fn authorization_url_embeds_signed_redirect() {
        let url = client().authorization_url(
            "https://app.example/api/shopee/auth/callback?state=abc",
            1610000000,
        );
        assert!(url.starts_with("https://h/api/v2/shop/auth_partner?"));
        assert!(url.contains("partner_id=1"));
        assert!(url.contains("timestamp=1610000000"));
        assert!(url.contains("sign="));
        assert!(url.contains(
            "redirect=https%3A%2F%2Fapp.example%2Fapi%2Fshopee%2Fauth%2Fcallback%3Fstate%3Dabc"
        ));
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
