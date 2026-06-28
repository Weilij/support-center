//! Resolve the live credentials for a platform: the single active channel
//! integration (decrypted), falling back to `.env`/config. Never panics.

use serde_json::{Map, Value};

use super::store;
use crate::state::AppState;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedChannel {
    pub access_token: Option<String>,
    pub secret: Option<String>,
    pub config: Map<String, Value>, // plain fields: channelId / liffId / pageId / igId
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

pub async fn resolve_channel(state: &AppState, platform: &str) -> ResolvedChannel {
    let mut access_token = None;
    let mut secret = None;
    let mut config = Map::new();

    if let Ok(Some(row)) = store::find_active_by_platform(&state.db, platform).await {
        config = row
            .config
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        if let Ok(creds) =
            store::decrypt_credentials(state.config.encryption_key.as_deref(), &row.credentials)
        {
            let get = |k: &str| creds.get(k).and_then(Value::as_str).map(str::to_string);
            match platform {
                "line" => {
                    access_token = get("channelAccessToken");
                    secret = get("channelSecret");
                }
                "facebook" => {
                    access_token = get("accessToken");
                    secret = get("appSecret");
                }
                "instagram" => {
                    access_token = get("accessToken");
                }
                _ => {}
            }
        }
    }

    let cfg = &state.config;
    let access_token = non_empty(access_token).or_else(|| match platform {
        "line" => cfg.line_channel_access_token.clone(),
        "facebook" => cfg.facebook_page_access_token.clone(),
        "instagram" => cfg
            .instagram_access_token
            .clone()
            .or_else(|| cfg.facebook_page_access_token.clone()),
        _ => None,
    });
    let secret = non_empty(secret).or_else(|| match platform {
        "line" => cfg.line_channel_secret.clone(),
        _ => None,
    });

    ResolvedChannel {
        access_token: non_empty(access_token),
        secret: non_empty(secret),
        config,
    }
}
