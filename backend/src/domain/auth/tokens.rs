//! Credential minting/verification per CRD §1.1 Data Concepts (lines 275-282).

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Validation};
use serde::{Deserialize, Serialize};

pub const ACCESS_TTL_SECS: i64 = 7200; // 2 hours
pub const REFRESH_TTL_SECS: i64 = 604_800; // 7 days
pub const TEMP_CHANGE_TTL_SECS: i64 = 1800; // ~30 minutes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamClaim {
    #[serde(rename = "teamId")]
    pub team_id: i64,
    pub role: String,
    #[serde(rename = "isPrimary")]
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub role: String,
    #[serde(rename = "primaryTeamId", skip_serializing_if = "Option::is_none")]
    pub primary_team_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teams: Option<Vec<TeamClaim>>,
    /// access | refresh | temp_change | monitoring | user
    #[serde(rename = "type")]
    pub token_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitoring: Option<bool>,
    #[serde(rename = "serviceRootIat", skip_serializing_if = "Option::is_none")]
    pub service_root_iat: Option<i64>,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
}

impl Claims {
    pub fn new(
        sub: impl Into<String>,
        role: impl Into<String>,
        token_type: &str,
        ttl_secs: i64,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            sub: sub.into(),
            email: None,
            name: None,
            role: role.into(),
            primary_team_id: None,
            teams: None,
            token_type: token_type.to_string(),
            monitoring: None,
            service_root_iat: None,
            jti: uuid::Uuid::new_v4().to_string(),
            iat: now,
            exp: now + ttl_secs,
        }
    }
}

pub fn sign(claims: &Claims, secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    encode(
        &jsonwebtoken::Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

pub fn verify(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map(|d| d.claims)
}
