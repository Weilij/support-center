//! Per-shop Shopee token storage (encrypted) + refresh-before-expiry logic.

use sqlx::PgPool;

use super::client::ShopeeClient;
use crate::crypto;
use crate::db::now_iso;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error("Shopee shop is not connected")]
    NotConnected,
    #[error(transparent)]
    Client(#[from] super::client::ClientError),
}

#[derive(Debug)]
pub struct ShopTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
}

/// Refresh when `now + buffer >= expires_at`. Unparseable timestamps → false.
pub fn needs_refresh(expires_at_iso: &str, now_iso: &str, buffer_secs: i64) -> bool {
    let (Ok(exp), Ok(now)) = (
        chrono::DateTime::parse_from_rfc3339(expires_at_iso),
        chrono::DateTime::parse_from_rfc3339(now_iso),
    ) else {
        return false;
    };
    now.timestamp() + buffer_secs >= exp.timestamp()
}

pub async fn save_tokens(
    db: &PgPool,
    enc_key: Option<&str>,
    shop_id: i64,
    access: &str,
    refresh: &str,
    expires_at: &str,
) -> Result<(), StoreError> {
    let access_enc = crypto::protect(enc_key, access)?;
    let refresh_enc = crypto::protect(enc_key, refresh)?;
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
    .map_err(StoreError::Database)?;
    Ok(())
}

pub async fn load(
    db: &PgPool,
    enc_key: Option<&str>,
    shop_id: i64,
) -> Result<Option<ShopTokens>, StoreError> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT access_token, refresh_token, expires_at FROM shopee_shops WHERE shop_id = $1",
    )
    .bind(shop_id)
    .fetch_optional(db)
    .await?;
    let Some((a, r, exp)) = row else {
        return Ok(None);
    };
    Ok(Some(ShopTokens {
        access_token: crypto::reveal(enc_key, &a)?,
        refresh_token: crypto::reveal(enc_key, &r)?,
        expires_at: exp,
    }))
}

/// Return a usable access token, refreshing (and persisting) when near expiry.
pub async fn valid_access_token(
    db: &PgPool,
    client: &ShopeeClient,
    enc_key: Option<&str>,
    shop_id: i64,
) -> Result<String, StoreError> {
    let tokens = load(db, enc_key, shop_id)
        .await?
        .ok_or(StoreError::NotConnected)?;
    if needs_refresh(&tokens.expires_at, &now_iso(), 300) {
        let fresh = client
            .refresh(&tokens.refresh_token, shop_id)
            .await
            .map_err(StoreError::Client)?;
        let new_refresh = if fresh.refresh_token.is_empty() {
            tokens.refresh_token
        } else {
            fresh.refresh_token
        };
        let expires_at =
            (chrono::Utc::now() + chrono::Duration::seconds(fresh.expire_in)).to_rfc3339();
        save_tokens(
            db,
            enc_key,
            shop_id,
            &fresh.access_token,
            &new_refresh,
            &expires_at,
        )
        .await?;
        Ok(fresh.access_token)
    } else {
        Ok(tokens.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_refresh_true_within_buffer() {
        assert!(needs_refresh(
            "2030-01-01T00:01:00Z",
            "2030-01-01T00:00:00Z",
            300
        ));
    }

    #[test]
    fn needs_refresh_false_when_fresh() {
        assert!(!needs_refresh(
            "2030-01-01T01:00:00Z",
            "2030-01-01T00:00:00Z",
            300
        ));
    }

    #[test]
    fn needs_refresh_false_on_unparseable() {
        assert!(!needs_refresh("not-a-date", "2030-01-01T00:00:00Z", 300));
    }
}
