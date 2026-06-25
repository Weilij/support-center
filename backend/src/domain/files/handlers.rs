//! File & Attachment Management handlers (CRD §4.4, lines 3005-3198).

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::limiter::{ADMIN_UPLOADS, STANDARD_UPLOADS};
use super::store::{self, FileRow, NewFile};
use super::{sign, validate};

type Result<T = Response> = std::result::Result<T, AppError>;

const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const DOWNLOAD_URL_TTL: i64 = 3600;
const PROXY_URL_TTL: i64 = 86_400;
const PRESIGNED_TTL: i64 = 900; // ~15 minutes

fn valid_file_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn require_file_id(id: &str) -> Result<()> {
    if valid_file_id(id) {
        Ok(())
    } else {
        Err(AppError::BadRequest("Invalid file identifier".into()))
    }
}

/// Marks an attachment record as failed (shared by the direct-upload and
/// confirm rejection branches).
async fn mark_upload_failed(state: &AppState, file_id: &str) -> Result<()> {
    sqlx::query("UPDATE attachments SET upload_status = 'failed', updated_at = $1 WHERE id = $2")
        .bind(now_iso())
        .bind(file_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Single-resource ownership rule: admins access any file; everyone else only
/// their own uploads. Denials surface as 404 (never 403) to avoid id enumeration.
fn user_can_access_file(user: &AuthUser, row: &FileRow) -> bool {
    user.is_admin() || row.uploaded_by.as_deref() == Some(user.id.as_str())
}

fn signed_download_url(state: &AppState, file_id: &str, key: &str, ttl: i64) -> (String, i64) {
    let (sig, expires) = sign::sign(state.config.file_signing_key(), key, ttl);
    let base = state.config.backend_url.clone().unwrap_or_default();
    (format!("{base}/api/files/download/{file_id}?expires={expires}&sig={sig}"), expires)
}

pub(crate) fn signed_public_url(state: &AppState, key: &str, ttl: i64) -> String {
    let base = state.config.backend_url.clone().unwrap_or_default();
    let (sig, expires) = sign::sign(state.config.file_signing_key(), key, ttl);
    format!("{base}/api/files/public/{key}?expires={expires}&sig={sig}")
}

// ================================================================ public ops

pub async fn health(State(state): State<Arc<AppState>>) -> Response {
    let db_ok = sqlx::query_scalar::<_, i64>("SELECT 1::bigint").fetch_one(&state.db).await.is_ok();
    let store_ok = tokio::fs::create_dir_all(&state.config.upload_dir).await.is_ok();
    envelope::ok(json!({
        "status": if db_ok && store_ok { "healthy" } else { "degraded" },
        "module": "files",
        "timestamp": now_iso(),
        "storageAvailable": store_ok,
        "databaseAvailable": db_ok,
    }))
}

#[derive(Deserialize)]
pub struct SignedQuery {
    pub sig: Option<String>,
    pub expires: Option<i64>,
}

fn verify_signature(state: &AppState, key: &str, q: &SignedQuery) -> bool {
    match (&q.sig, q.expires) {
        (Some(sig), Some(expires)) => {
            sign::verify(state.config.file_signing_key(), key, sig, expires)
        }
        _ => false,
    }
}

fn stream_bytes(
    bytes: Vec<u8>,
    content_type: &str,
    disposition: Option<&str>,
    cache: &'static str,
    cors_origin: Option<&str>,
) -> Response {
    let mut resp = (StatusCode::OK, bytes).into_response();
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Some(d) = disposition {
        if let Ok(v) = HeaderValue::from_str(d) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(origin) = cors_origin {
        if let Ok(v) = HeaderValue::from_str(origin) {
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
        }
    }
    resp
}

fn frontend_origin(state: &AppState) -> Option<String> {
    state
        .config
        .frontend_url
        .as_deref()
        .map(|s| s.trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

fn public_disposition_for(content_type: &str) -> Option<&'static str> {
    let lower = content_type.to_ascii_lowercase();
    if lower.starts_with("image/") || lower.starts_with("video/") {
        None
    } else {
        Some("attachment")
    }
}

/// GET /api/files/public/{*path} — signature-gated public proxy (CRD 3113-3121).
pub async fn public_proxy(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    Query(q): Query<SignedQuery>,
) -> Result {
    if path.is_empty() {
        return Err(AppError::BadRequest("Storage path is required".into()));
    }
    // Invalid/expired/missing signature -> 404, never 401 (CRD 3119).
    if !verify_signature(&state, &path, &q) {
        return Err(AppError::NotFound("File not found".into()));
    }
    let Some(bytes) = store::get_object(&state.config.upload_dir, &path).await else {
        return Err(AppError::NotFound("File not found".into()));
    };
    let content_type = content_type_for_key(&state, &path).await;
    let cors = frontend_origin(&state);
    Ok(stream_bytes(
        bytes,
        &content_type,
        public_disposition_for(&content_type),
        "public, max-age=86400",
        cors.as_deref(),
    ))
}

/// GET /api/assets/video-placeholder.png — a static thumbnail used as the
/// `previewImageUrl` for outbound LINE video messages (public, no auth).
pub async fn video_placeholder() -> Response {
    const PNG: &[u8] = include_bytes!("../../../assets/video-placeholder.png");
    stream_bytes(PNG.to_vec(), "image/png", None, "public, max-age=604800", None)
}

async fn content_type_for_key(state: &AppState, key: &str) -> String {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT content_type FROM attachments WHERE storage_key = $1 LIMIT 1",
    )
    .bind(key)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .flatten()
    .unwrap_or_else(|| "application/octet-stream".into())
}

/// GET /api/files/download/{attachmentId} — canonical force-download link
/// (CRD 3123-3130). The signature is bound to the storage location.
pub async fn public_download(
    State(state): State<Arc<AppState>>,
    Path(attachment_id): Path<String>,
    Query(q): Query<SignedQuery>,
) -> Result {
    if attachment_id.is_empty() {
        return Err(AppError::BadRequest("Attachment identifier is required".into()));
    }
    let row = store::find(&state.db, &attachment_id)
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    let key = row
        .storage_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    if !verify_signature(&state, &key, &q) {
        return Err(AppError::NotFound("File not found".into()));
    }
    let Some(bytes) = store::get_object(&state.config.upload_dir, &key).await else {
        return Err(AppError::NotFound("File not found".into()));
    };
    let content_type =
        row.content_type.clone().unwrap_or_else(|| "application/octet-stream".into());
    // Extension-less stored names still download as openable files (CRD 3127).
    let mut filename = row.file_name.clone().unwrap_or_else(|| attachment_id.clone());
    if validate::extension_of(&filename).is_none() {
        if let Some(ext) = validate::extension_for_type(&content_type) {
            filename = format!("{filename}.{ext}");
        }
    }
    Ok(stream_bytes(
        bytes,
        &content_type,
        Some(&format!("attachment; filename=\"{filename}\"")),
        "public, max-age=86400",
        frontend_origin(&state).as_deref(),
    ))
}

/// GET /api/files/line-proxy/{lineMessageId} — fast path from the store,
/// live-fetch fallback with background self-heal (CRD 3132-3140).
pub async fn line_proxy(
    State(state): State<Arc<AppState>>,
    Path(line_message_id): Path<String>,
) -> Result {
    if line_message_id.is_empty() || !line_message_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::BadRequest("LINE message identifier must be numeric".into()));
    }
    let key = format!("line/media/{line_message_id}");
    if let Some(bytes) = store::get_object(&state.config.upload_dir, &key).await {
        let content_type = content_type_for_key(&state, &key).await;
        let cors = frontend_origin(&state);
        return Ok(stream_bytes(
            bytes,
            &content_type,
            public_disposition_for(&content_type),
            "public, max-age=86400",
            cors.as_deref(),
        ));
    }
    if state.config.line_channel_access_token.is_none() {
        return Err(AppError::Internal("LINE channel token is not configured".into()));
    }
    // TODO(channels): live fetch from the LINE content API, stream the bytes,
    // then self-heal in the background (persist object + attachment record).
    // Without a live upstream, the fallback reports bad-gateway (CRD 3138).
    Ok((
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "success": false,
            "error": "LINE content is unavailable upstream",
            "timestamp": now_iso(),
        })),
    )
        .into_response())
}

/// GET /api/r2-public/{folder}/{filename} (CRD 3142-3149).
pub async fn r2_public(
    State(state): State<Arc<AppState>>,
    Path((folder, filename)): Path<(String, String)>,
    Query(q): Query<SignedQuery>,
) -> Result {
    let key = format!("{folder}/{filename}");
    if !verify_signature(&state, &key, &q) {
        return Err(AppError::NotFound("File not found".into()));
    }
    let Some(bytes) = store::get_object(&state.config.upload_dir, &key).await else {
        return Err(AppError::NotFound("File not found".into()));
    };
    let content_type = content_type_for_key(&state, &key).await;
    let etag = format!("\"{}\"", bytes.len());
    let cors = frontend_origin(&state);
    let mut resp = stream_bytes(
        bytes,
        &content_type,
        public_disposition_for(&content_type),
        "public, max-age=31536000",
        cors.as_deref(),
    );
    if let Ok(v) = HeaderValue::from_str(&etag) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    Ok(resp)
}

/// PUT /api/files/direct/{fileId} — the signed direct-upload target minted by
/// the presigned-url operation (signature bound to the record's storage key).
pub async fn direct_upload(
    State(state): State<Arc<AppState>>,
    Path(file_id): Path<String>,
    Query(q): Query<SignedQuery>,
    body: axum::body::Bytes,
) -> Result {
    let row = store::find(&state.db, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    let key = row.storage_key.clone().unwrap_or_default();
    if !verify_signature(&state, &key, &q) {
        return Err(AppError::NotFound("File not found".into()));
    }
    let content_type = row.content_type.as_deref().unwrap_or("application/octet-stream");
    let platform = row.platform.as_deref().unwrap_or("system");
    let reject = if !validate::allowed_types(platform).contains(&content_type) {
        Some(format!("Content type '{content_type}' is not allowed"))
    } else if body.len() > validate::size_cap(content_type, platform) {
        Some("File exceeds the maximum allowed size".to_string())
    } else {
        validate::check_signature(content_type, &body).err()
    };
    if let Some(message) = reject {
        mark_upload_failed(&state, &file_id).await?;
        return Err(AppError::BadRequest(message));
    }
    store::put_object(&state.config.upload_dir, &key, &body)
        .await
        .map_err(|e| AppError::Internal(format!("Object write failed: {e}")))?;
    Ok(envelope::ok(json!({"uploaded": true, "size": body.len()})))
}

// ================================================================ authed ops

pub async fn info(Extension(_user): Extension<AuthUser>) -> Result {
    Ok(envelope::ok_msg(
        json!({
            "features": [
                "upload", "multi-upload", "download", "signed-urls", "direct-upload",
                "chunked-upload", "search", "batch-operations", "statistics",
            ],
            "limits": {
                "maxFileSize": "10MB",
                "allowedTypes": ["image", "video", "audio", "document", "archive"],
                "platforms": ["line", "facebook", "system"],
            },
        }),
        "File management module",
    ))
}

struct UploadedPart {
    filename: String,
    content_type: String,
    bytes: Vec<u8>,
}

struct UploadForm {
    files: Vec<UploadedPart>,
    platform: String,
    conversation_id: Option<String>,
    message_id: Option<String>,
}

async fn read_multipart(mut multipart: Multipart) -> Result<UploadForm> {
    let mut form = UploadForm {
        files: Vec::new(),
        platform: "system".into(),
        conversation_id: None,
        message_id: None,
    };
    let read = async {
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| AppError::BadRequest(format!("Invalid multipart body: {e}")))?
        {
            let name = field.name().unwrap_or("").to_string();
            match name.as_str() {
                "file" | "files" => {
                    let filename = field.file_name().unwrap_or("").to_string();
                    let content_type = field
                        .content_type()
                        .unwrap_or("application/octet-stream")
                        .to_string();
                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Upload read failed: {e}")))?;
                    if filename.is_empty() {
                        return Err(AppError::BadRequest("File is required".into()));
                    }
                    form.files.push(UploadedPart { filename, content_type, bytes: bytes.to_vec() });
                }
                "platform" => {
                    let v = field.text().await.unwrap_or_default();
                    if ["line", "facebook", "system", "admin"].contains(&v.as_str()) {
                        form.platform = v;
                    }
                }
                "conversationId" => {
                    form.conversation_id = Some(field.text().await.unwrap_or_default())
                        .filter(|s| !s.is_empty());
                }
                "messageId" => {
                    form.message_id =
                        Some(field.text().await.unwrap_or_default()).filter(|s| !s.is_empty());
                }
                _ => {}
            }
        }
        Ok(form)
    };
    match tokio::time::timeout(UPLOAD_TIMEOUT, read).await {
        Ok(r) => r,
        Err(_) => Err(AppError::BadRequest("Upload timed out".into())), // surfaced as 408 below
    }
}

fn validate_part(part: &UploadedPart, platform: &str) -> std::result::Result<(), String> {
    validate::validate_filename(&part.filename)?;
    if !validate::allowed_types(platform).contains(&part.content_type.as_str()) {
        return Err(format!(
            "Content type '{}' is not allowed for platform '{platform}'",
            part.content_type
        ));
    }
    let cap = validate::size_cap(&part.content_type, platform);
    if part.bytes.is_empty() {
        return Err("File is empty".into());
    }
    if part.bytes.len() > cap {
        return Err(format!("File too large (max {} bytes)", cap));
    }
    validate::check_signature(&part.content_type, &part.bytes)?;
    Ok(())
}

async fn persist_part(
    state: &AppState,
    user: &AuthUser,
    part: &UploadedPart,
    platform: &str,
    conversation_id: Option<&str>,
    message_id: Option<&str>,
) -> Result<Value> {
    let file_id = uuid::Uuid::new_v4().to_string();
    let file_type = validate::file_category(&part.content_type);
    let ext = validate::extension_of(&part.filename);
    let key = store::storage_key(platform, file_type, ext.as_deref());
    store::put_object(&state.config.upload_dir, &key, &part.bytes)
        .await
        .map_err(|e| AppError::BadRequest(format!("Upload failed: {e}")))?;
    let (url, _) = signed_download_url(state, &file_id, &key, DOWNLOAD_URL_TTL);
    let public_url = signed_public_url(state, &key, PROXY_URL_TTL);
    let sanitized = validate::sanitize_filename(&part.filename);
    store::insert(
        &state.db,
        &NewFile {
            id: &file_id,
            filename: &sanitized,
            original_name: &part.filename,
            content_type: &part.content_type,
            size: part.bytes.len() as i64,
            storage_key: &key,
            file_url: &url,
            public_url: Some(&public_url),
            platform,
            file_type,
            conversation_id,
            message_id,
            uploaded_by: &user.id,
            status: "completed",
        },
    )
    .await
    .map_err(|e| AppError::BadRequest(format!("Upload failed: {e}")))?;

    let thumbnail = (file_type == "image").then(|| public_url.clone());
    Ok(json!({
        "id": file_id,
        "filename": sanitized,
        "size": part.bytes.len(),
        "contentType": part.content_type,
        "url": url,
        "publicUrl": public_url,
        "thumbnailUrl": thumbnail,
        "fileType": file_type,
        "createdAt": now_iso(),
    }))
}

async fn upload_with_platform(
    state: Arc<AppState>,
    user: AuthUser,
    multipart: Multipart,
    forced_platform: Option<&str>,
) -> Result {
    let mut form = read_multipart(multipart).await?;
    if let Some(p) = forced_platform {
        form.platform = p.to_string();
    }
    if form.files.is_empty() {
        return Err(AppError::BadRequest("File is required".into()));
    }
    let part = &form.files[0];
    let policy = if form.platform == "admin" { ADMIN_UPLOADS } else { STANDARD_UPLOADS };
    let _guard = state
        .files_limiter
        .admit(&user.id, part.bytes.len() as u64, &policy)
        .map_err(|m| AppError::TooManyRequests { message: m, retry_after: 60 })?;
    validate_part(part, &form.platform).map_err(AppError::BadRequest)?;
    let view = persist_part(
        &state,
        &user,
        part,
        &form.platform,
        form.conversation_id.as_deref(),
        form.message_id.as_deref(),
    )
    .await?;
    Ok(envelope::created(view))
}

/// POST /api/files (CRD 3021-3036).
pub async fn upload(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    multipart: Multipart,
) -> Result {
    upload_with_platform(state, user, multipart, None).await
}

/// POST /api/files/upload/{platform} (CRD 3161-3162).
pub async fn upload_platform(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(platform): Path<String>,
    multipart: Multipart,
) -> Result {
    if !["line", "facebook", "admin"].contains(&platform.as_str()) {
        return Err(AppError::NotFound("Unknown upload platform".into()));
    }
    upload_with_platform(state, user, multipart, Some(&platform)).await
}

/// POST /api/files/upload-multiple (CRD 3155-3159): cap 10, partial success.
pub async fn upload_multiple(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    multipart: Multipart,
) -> Result {
    let form = read_multipart(multipart).await?;
    if form.files.is_empty() {
        return Err(AppError::BadRequest("At least one file is required".into()));
    }
    if form.files.len() > 10 {
        return Err(AppError::BadRequest("At most 10 files per request".into()));
    }
    let mut successful = Vec::new();
    let mut failed = Vec::new();
    for part in &form.files {
        let admitted = state
            .files_limiter
            .admit(&user.id, part.bytes.len() as u64, &STANDARD_UPLOADS);
        let result = match admitted {
            Err(m) => Err(m),
            Ok(_guard) => match validate_part(part, &form.platform) {
                Err(m) => Err(m),
                Ok(()) => persist_part(
                    &state,
                    &user,
                    part,
                    &form.platform,
                    form.conversation_id.as_deref(),
                    form.message_id.as_deref(),
                )
                .await
                .map_err(|e| e.to_string()),
            },
        };
        match result {
            Ok(v) => successful.push(v),
            Err(e) => failed.push(json!({"filename": part.filename, "error": e})),
        }
    }
    Ok(envelope::ok(json!({
        "successful": successful,
        "failed": failed,
        "summary": {
            "total": form.files.len(),
            "successCount": successful.len(),
            "failedCount": failed.len(),
        },
    })))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
    pub platform: Option<String>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    pub q: Option<String>,
    pub mode: Option<String>,
    #[serde(rename = "urlOnly")]
    pub url_only: Option<bool>,
    #[serde(rename = "expiresIn")]
    pub expires_in: Option<i64>,
}

/// GET /api/files (CRD 3038-3044): non-admins see only their own uploads.
pub async fn list(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    let (page, size) = envelope::clamp_page(q.page, q.page_size.or(q.limit));
    let scope_user = (!user.is_admin()).then_some(user.id.clone());
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM attachments
         WHERE ($1 IS NULL OR uploaded_by = $2)
           AND ($3 IS NULL OR platform = $4)
           AND ($5 IS NULL OR conversation_id = $6)",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .bind(&q.platform)
    .bind(&q.platform)
    .bind(&q.conversation_id)
    .bind(&q.conversation_id)
    .fetch_one(&state.db)
    .await?;
    let rows: Vec<FileRow> = sqlx::query_as(
        "SELECT id, message_id, conversation_id, file_name, original_name, content_type,
                file_size, file_url, public_url, storage_key, upload_status, uploaded_by,
                platform, file_type, created_at, updated_at
         FROM attachments
         WHERE ($1 IS NULL OR uploaded_by = $2)
           AND ($3 IS NULL OR platform = $4)
           AND ($5 IS NULL OR conversation_id = $6)
         ORDER BY created_at DESC, id DESC LIMIT $7 OFFSET $8",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .bind(&q.platform)
    .bind(&q.platform)
    .bind(&q.conversation_id)
    .bind(&q.conversation_id)
    .bind(size)
    .bind((page - 1) * size)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows.iter().map(store::file_view).collect();
    Ok(envelope::paginated(&items, page, size, total))
}

/// GET /api/files/stats/summary (CRD 3065-3071).
pub async fn stats_summary(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    let scope_user = (!user.is_admin()).then_some(user.id.clone());
    let (count, bytes): (i64, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), SUM(file_size)::bigint FROM attachments WHERE ($1 IS NULL OR uploaded_by = $2)",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, None));
    let by_type: Vec<(Option<String>, i64, Option<i64>)> = sqlx::query_as(
        "SELECT file_type, COUNT(*), SUM(file_size)::bigint FROM attachments
         WHERE ($1 IS NULL OR uploaded_by = $2) GROUP BY file_type",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let by_platform: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT platform, COUNT(*) FROM attachments
         WHERE ($1 IS NULL OR uploaded_by = $2) GROUP BY platform",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let window_start = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
    let recent: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM attachments WHERE ($1 IS NULL OR uploaded_by = $2) AND created_at >= $3",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .bind(&window_start)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let total_bytes = bytes.unwrap_or(0);
    Ok(envelope::ok(json!({
        "totalFiles": count,
        "totalBytes": total_bytes,
        "averageBytes": if count > 0 { total_bytes / count } else { 0 },
        "byType": by_type.iter().map(|(t, c, b)| json!({
            "type": t.clone().unwrap_or_else(|| "other".into()),
            "count": c, "bytes": b.unwrap_or(0),
        })).collect::<Vec<_>>(),
        "byPlatform": by_platform.iter().map(|(p, c)| json!({
            "platform": p.clone().unwrap_or_else(|| "system".into()), "count": c,
        })).collect::<Vec<_>>(),
        "storage": { "usedBytes": total_bytes, "availableBytes": -1, "usedPercentage": -1 },
        "recentActivity": { "periodDays": 30, "uploads": recent },
    })))
}

/// GET /api/files/{fileId} (CRD 3046-3054): url mode or raw stream.
pub async fn get_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(file_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result {
    require_file_id(&file_id)?;
    let row = store::find(&state.db, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    if !user_can_access_file(&user, &row) {
        return Err(AppError::NotFound("File not found".into()));
    }
    let url_mode = q.url_only.unwrap_or(false) || q.mode.as_deref() == Some("url");
    if url_mode {
        let url = match row.storage_key.as_deref().filter(|k| !k.is_empty()) {
            Some(key) => signed_download_url(&state, &row.id, key, DOWNLOAD_URL_TTL).0,
            None => row.file_url.clone().unwrap_or_default(),
        };
        return Ok(envelope::ok(json!({ "url": url, "file": store::file_view(&row) })));
    }
    let key = row
        .storage_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| AppError::BadRequest("No file data available".into()))?;
    let Some(bytes) = store::get_object(&state.config.upload_dir, &key).await else {
        return Err(AppError::NotFound("File data not found".into()));
    };
    let content_type =
        row.content_type.clone().unwrap_or_else(|| "application/octet-stream".into());
    let filename = row.file_name.clone().unwrap_or_else(|| file_id.clone());
    Ok(stream_bytes(
        bytes,
        &content_type,
        Some(&format!("attachment; filename=\"{filename}\"")),
        "private, max-age=3600",
        None,
    ))
}

/// DELETE /api/files/{fileId} (CRD 3056-3063): hard delete, idempotent object removal.
pub async fn delete_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(file_id): Path<String>,
) -> Result {
    require_file_id(&file_id)?;
    let row = store::find(&state.db, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    if !user_can_access_file(&user, &row) {
        return Err(AppError::NotFound("File not found".into()));
    }
    if let Some(key) = row.storage_key.as_deref().filter(|k| !k.is_empty()) {
        store::delete_object(&state.config.upload_dir, key).await;
    }
    sqlx::query("DELETE FROM attachments WHERE id = $1")
        .bind(&file_id)
        .execute(&state.db)
        .await?;
    Ok(envelope::ok_msg(json!({"id": file_id}), "File deleted"))
}

/// GET /api/files/{fileId}/download-url (CRD 3178-3180).
pub async fn download_url(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(file_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result {
    require_file_id(&file_id)?;
    let row = store::find(&state.db, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    if !user_can_access_file(&user, &row) {
        return Err(AppError::NotFound("File not found".into()));
    }
    let key = row
        .storage_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    let ttl = q.expires_in.unwrap_or(DOWNLOAD_URL_TTL).clamp(60, 7 * 86_400);
    let (url, expires) = signed_download_url(&state, &row.id, &key, ttl);
    Ok(envelope::ok(json!({ "url": url, "expiresAt": expires })))
}

// ------------------------------------------------ scoped listings & search

pub async fn conversation_files(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(conversation_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result {
    let q2 = ListQuery { conversation_id: Some(conversation_id), ..q };
    list(State(state), Extension(user), Query(q2)).await
}

pub async fn message_files(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(message_id): Path<String>,
) -> Result {
    let scope_user = (!user.is_admin()).then_some(user.id.clone());
    let rows: Vec<FileRow> = sqlx::query_as(
        "SELECT id, message_id, conversation_id, file_name, original_name, content_type,
                file_size, file_url, public_url, storage_key, upload_status, uploaded_by,
                platform, file_type, created_at, updated_at
         FROM attachments
         WHERE message_id = $1 AND ($2 IS NULL OR uploaded_by = $3)
         ORDER BY created_at DESC LIMIT 50",
    )
    .bind(&message_id)
    .bind(&scope_user)
    .bind(&scope_user)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope::ok(rows.iter().map(store::file_view).collect::<Vec<_>>()))
}

/// GET /api/files/search (CRD 3169-3171): case-insensitive filename contains.
pub async fn search(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    let query = q
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("Search query is required".into()))?;
    let (page, size) = envelope::clamp_page(q.page, q.page_size.or(q.limit));
    let scope_user = (!user.is_admin()).then_some(user.id.clone());
    let pattern = format!("%{}%", query.to_lowercase());
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM attachments
         WHERE ($1 IS NULL OR uploaded_by = $2)
           AND (LOWER(file_name) LIKE $3 OR LOWER(original_name) LIKE $4)
           AND ($5 IS NULL OR platform = $6)
           AND ($7 IS NULL OR file_type = $8)",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .bind(&pattern)
    .bind(&pattern)
    .bind(&q.platform)
    .bind(&q.platform)
    .bind(&q.file_type)
    .bind(&q.file_type)
    .fetch_one(&state.db)
    .await?;
    let rows: Vec<FileRow> = sqlx::query_as(
        "SELECT id, message_id, conversation_id, file_name, original_name, content_type,
                file_size, file_url, public_url, storage_key, upload_status, uploaded_by,
                platform, file_type, created_at, updated_at
         FROM attachments
         WHERE ($1 IS NULL OR uploaded_by = $2)
           AND (LOWER(file_name) LIKE $3 OR LOWER(original_name) LIKE $4)
           AND ($5 IS NULL OR platform = $6)
           AND ($7 IS NULL OR file_type = $8)
         ORDER BY created_at DESC LIMIT $9 OFFSET $10",
    )
    .bind(&scope_user)
    .bind(&scope_user)
    .bind(&pattern)
    .bind(&pattern)
    .bind(&q.platform)
    .bind(&q.platform)
    .bind(&q.file_type)
    .bind(&q.file_type)
    .bind(size)
    .bind((page - 1) * size)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows.iter().map(store::file_view).collect();
    Ok(envelope::paginated(&items, page, size, total))
}

#[derive(Deserialize)]
pub struct BatchBody {
    pub operation: Option<String>,
    #[serde(rename = "fileIds")]
    pub file_ids: Option<Vec<String>>,
}

/// POST /api/files/batch (CRD 3173-3176): delete is the supported operation.
pub async fn batch(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<BatchBody>,
) -> Result {
    let started = std::time::Instant::now();
    let operation = body
        .operation
        .as_deref()
        .filter(|o| !o.is_empty())
        .ok_or_else(|| AppError::BadRequest("operation is required".into()))?;
    let ids = body
        .file_ids
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AppError::BadRequest("fileIds must be a non-empty array".into()))?;

    let mut successful = Vec::new();
    let mut failed = Vec::new();
    for id in &ids {
        if operation != "delete" {
            failed.push(json!({"id": id, "error": format!("Unsupported operation '{operation}'")}));
            continue;
        }
        match store::find(&state.db, id).await {
            Ok(Some(row)) => {
                // Non-owners can't delete (and we don't reveal the file exists).
                if !user_can_access_file(&user, &row) {
                    failed.push(json!({"id": id, "error": "File not found"}));
                    continue;
                }
                if let Some(key) = row.storage_key.as_deref().filter(|k| !k.is_empty()) {
                    store::delete_object(&state.config.upload_dir, key).await;
                }
                let _ = sqlx::query("DELETE FROM attachments WHERE id = $1")
                    .bind(id)
                    .execute(&state.db)
                    .await;
                successful.push(json!({"id": id}));
            }
            Ok(None) => failed.push(json!({"id": id, "error": "File not found"})),
            Err(e) => failed.push(json!({"id": id, "error": e.to_string()})),
        }
    }
    Ok(envelope::ok(json!({
        "successful": successful,
        "failed": failed,
        "summary": {
            "total": ids.len(),
            "successCount": successful.len(),
            "failedCount": failed.len(),
            "processingTimeMs": started.elapsed().as_millis() as i64,
        },
    })))
}

// ------------------------------------------------ direct-upload flow

#[derive(Deserialize)]
pub struct PresignedBody {
    pub filename: Option<String>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub size: Option<i64>,
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "messageId")]
    pub message_id: Option<String>,
}

/// POST /api/files/presigned-url (CRD 3075-3085).
pub async fn presigned_url(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<PresignedBody>,
) -> Result {
    let filename = body.filename.as_deref().unwrap_or("");
    if filename.is_empty() {
        return Err(AppError::Validation(
            "Validation failed".into(),
            vec![crate::error::FieldProblem {
                field: "filename".into(),
                message: "filename is required".into(),
                value: None,
            }],
        ));
    }
    if filename.chars().count() > 255 {
        return Err(AppError::BadRequest("filename exceeds 255 characters".into()));
    }
    validate::validate_filename(filename).map_err(AppError::BadRequest)?;
    let content_type = body.content_type.as_deref().unwrap_or("");
    if content_type.is_empty() || !validate::allowed_types("admin").contains(&content_type) {
        return Err(AppError::BadRequest(format!(
            "contentType '{content_type}' is missing or unsupported"
        )));
    }
    let size = body.size.unwrap_or(0);
    if size <= 0 || size as usize > validate::GLOBAL_MAX {
        return Err(AppError::BadRequest("size must be a positive number up to 10MB".into()));
    }

    let file_id = uuid::Uuid::new_v4().to_string();
    let sanitized = validate::sanitize_filename(filename);
    let file_type = validate::file_category(content_type);
    let key = store::storage_key("system", file_type, validate::extension_of(&sanitized).as_deref());
    let (sig, expires) = sign::sign(state.config.file_signing_key(), &key, PRESIGNED_TTL);
    let base = state.config.backend_url.clone().unwrap_or_default();
    let upload_url = format!("{base}/api/files/direct/{file_id}?expires={expires}&sig={sig}");
    let public_url = signed_public_url(&state, &key, PROXY_URL_TTL);

    store::insert(
        &state.db,
        &NewFile {
            id: &file_id,
            filename: &sanitized,
            original_name: filename,
            content_type,
            size,
            storage_key: &key,
            file_url: &upload_url,
            public_url: Some(&public_url),
            platform: "system",
            file_type,
            conversation_id: body.conversation_id.as_deref(),
            message_id: body.message_id.as_deref(),
            uploaded_by: &user.id,
            status: "pending",
        },
    )
    .await?;

    Ok(envelope::ok(json!({
        "uploadUrl": upload_url,
        "fileId": file_id,
        "publicUrl": public_url,
        "expiresAt": expires,
        "instructions": {
            "method": "PUT",
            "headers": { "Content-Type": content_type },
            "thenCall": format!("POST /api/files/{file_id}/confirm"),
        },
    })))
}

/// GET /api/files/presigned-url/status (CRD 3087-3091).
pub async fn presigned_status(Extension(_user): Extension<AuthUser>) -> Result {
    Ok(envelope::ok(json!({
        "configured": true,
        "maxBytes": validate::GLOBAL_MAX,
        "maxMB": 10,
        "allowedTypes": validate::allowed_types("admin"),
        "uploadUrlValiditySeconds": PRESIGNED_TTL,
        "message": "Direct upload is available",
    })))
}

#[derive(Deserialize)]
pub struct ConfirmBody {
    pub size: Option<i64>,
    pub checksum: Option<String>,
}

/// POST /api/files/{fileId}/confirm (CRD 3093-3101).
pub async fn confirm_upload(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(file_id): Path<String>,
    Json(body): Json<ConfirmBody>,
) -> Result {
    require_file_id(&file_id)?;
    let size = body.size.unwrap_or(0);
    if size <= 0 {
        return Err(AppError::BadRequest("size must be a positive number".into()));
    }
    let row = store::find(&state.db, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    if !user_can_access_file(&user, &row) {
        return Err(AppError::NotFound("File not found".into()));
    }
    if row.upload_status == "completed" {
        // Confirming an already-completed record is idempotent (CRD 3101).
        let mut view = store::file_view(&row);
        view["confirmed"] = json!(true);
        return Ok(envelope::ok(view));
    }
    let key = row.storage_key.clone().unwrap_or_default();
    let object = store::get_object(&state.config.upload_dir, &key).await;
    let Some(bytes) = object else {
        mark_upload_failed(&state, &file_id).await?;
        return Err(AppError::BadRequest("Uploaded object not found in store".into()));
    };
    if bytes.len() as i64 != size {
        mark_upload_failed(&state, &file_id).await?;
        return Err(AppError::BadRequest("Uploaded size does not match the confirmed size".into()));
    }
    if let Some(expected) = body.checksum.as_deref().filter(|c| !c.is_empty()) {
        let digest = Sha256::digest(&bytes);
        let actual = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
        if !actual.eq_ignore_ascii_case(expected) {
            mark_upload_failed(&state, &file_id).await?;
            return Err(AppError::BadRequest("Uploaded checksum does not match".into()));
        }
    }
    sqlx::query(
        "UPDATE attachments SET upload_status = 'completed', file_size = $1, updated_at = $2 WHERE id = $3",
    )
    .bind(bytes.len() as i64)
    .bind(now_iso())
    .bind(&file_id)
    .execute(&state.db)
    .await?;
    let row = store::find(&state.db, &file_id).await?.unwrap_or(row);
    let mut view = store::file_view(&row);
    view["confirmed"] = json!(true);
    Ok(envelope::ok(view))
}

/// GET /api/files/{fileId}/status (CRD 3103-3109).
pub async fn upload_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(file_id): Path<String>,
) -> Result {
    require_file_id(&file_id)?;
    let row = store::find(&state.db, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    if !user_can_access_file(&user, &row) {
        return Err(AppError::NotFound("File not found".into()));
    }
    Ok(envelope::ok(store::file_view(&row)))
}

// ------------------------------------------------ chunked upload (boundary)

#[derive(Deserialize)]
pub struct ChunkInitBody {
    pub filename: Option<String>,
    pub size: Option<i64>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
}

/// Chunked lifecycle (CRD 3182-3183): acknowledged but chunks are not durably
/// persisted within the current behavioral boundary.
pub async fn chunked_init(
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<ChunkInitBody>,
) -> Result {
    let filename = body.filename.as_deref().unwrap_or("");
    validate::validate_filename(filename).map_err(AppError::BadRequest)?;
    let size = body.size.unwrap_or(0);
    if size <= 0 {
        return Err(AppError::BadRequest("size must be a positive number".into()));
    }
    if body.content_type.as_deref().unwrap_or("").is_empty() {
        return Err(AppError::BadRequest("contentType is required".into()));
    }
    const CHUNK: i64 = 1024 * 1024;
    Ok(envelope::ok(json!({
        "uploadId": uuid::Uuid::new_v4().to_string(),
        "chunkSize": CHUNK,
        "totalChunks": (size + CHUNK - 1) / CHUNK,
        "expiresInSeconds": 86_400,
    })))
}

pub async fn chunked_chunk(
    Extension(_user): Extension<AuthUser>,
    Path(session_id): Path<String>,
) -> Result {
    Ok(envelope::ok_msg(json!({"uploadId": session_id}), "Chunk received"))
}

pub async fn chunked_complete(
    Extension(_user): Extension<AuthUser>,
    Path(session_id): Path<String>,
) -> Result {
    // Behavioral boundary: a synthesized record, not a stored object (CRD 3183).
    Ok(envelope::ok_msg(
        json!({
            "uploadId": session_id,
            "file": { "id": uuid::Uuid::new_v4().to_string(), "synthesized": true },
        }),
        "Upload completed",
    ))
}

pub async fn chunked_cancel(
    Extension(_user): Extension<AuthUser>,
    Path(session_id): Path<String>,
) -> Result {
    Ok(envelope::ok_msg(json!({"uploadId": session_id}), "Upload cancelled"))
}

#[cfg(test)]
mod tests {
    use super::{public_disposition_for, stream_bytes};
    use axum::http::header;

    #[test]
    fn stream_bytes_adds_nosniff_and_scoped_cors() {
        let resp = stream_bytes(
            b"hello".to_vec(),
            "text/plain",
            Some("attachment"),
            "public, max-age=60",
            Some("https://app.example"),
        );
        let headers = resp.headers();
        assert_eq!(headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "https://app.example"
        );
        assert_ne!(headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), "*");
        assert_eq!(headers.get(header::CONTENT_DISPOSITION).unwrap(), "attachment");
    }

    #[test]
    fn stream_bytes_omits_cors_when_no_frontend_origin_is_configured() {
        let resp = stream_bytes(
            b"hello".to_vec(),
            "text/plain",
            None,
            "private, max-age=60",
            None,
        );
        assert!(resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    }

    #[test]
    fn public_proxy_disposition_only_allows_images_and_videos_inline() {
        assert_eq!(public_disposition_for("image/png"), None);
        assert_eq!(public_disposition_for("video/mp4"), None);
        assert_eq!(public_disposition_for("text/html"), Some("attachment"));
        assert_eq!(public_disposition_for("application/pdf"), Some("attachment"));
    }
}
