use axum::extract::{Multipart, Path, State};
use axum::Extension;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use crate::domain::messaging::store;

use super::{author_or_admin, message_not_found};

const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Allowed upload MIME types: common image, video, audio, PDF, plain text, and
/// Word documents (CRD 957).
const ALLOWED_MIME: &[&str] = &[
    "image/jpeg",
    "image/jpg",
    "image/png",
    "image/gif",
    "image/webp",
    "video/mp4",
    "video/quicktime",
    "video/webm",
    "audio/mpeg",
    "audio/mp3",
    "audio/wav",
    "audio/ogg",
    "application/pdf",
    "text/plain",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
];

pub async fn list_attachments(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;
    let attachments: Vec<Value> = store::attachments_for(&state.db, &id)
        .await?
        .iter()
        .map(store::attachment_view)
        .collect();
    Ok(envelope::ok(json!({
        "messageId": id,
        "conversationId": m.conversation_id,
        "attachments": attachments,
        "count": attachments.len(),
    })))
}

pub async fn upload_attachment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result {
    let m = store::find_message(&state.db, &id)
        .await?
        .ok_or_else(message_not_found)?;
    if m.sender_type == "agent" && !author_or_admin(&user, &m) {
        return Err(AppError::Forbidden(
            "Only the author or an administrator can add attachments".into(),
        ));
    }

    let mut file: Option<(String, String, Vec<u8>)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("upload.bin").to_string();
            let mime = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string();
            match field.bytes().await {
                Ok(bytes) => {
                    file = Some((filename, mime, bytes.to_vec()));
                    break;
                }
                Err(_) => return Err(AppError::BadRequest("No file provided".into())),
            }
        }
    }
    let Some((filename, mime, bytes)) = file.filter(|(_, _, b)| !b.is_empty()) else {
        return Err(AppError::BadRequest("No file provided".into()));
    };
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(AppError::BadRequest("File too large (max 10MB)".into()));
    }
    if !ALLOWED_MIME.contains(&mime.as_str()) {
        return Err(AppError::BadRequest(format!(
            "File type '{mime}' is not allowed"
        )));
    }

    let safe_name: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let attachment_id = uuid::Uuid::new_v4().to_string();
    let storage_key = format!("{attachment_id}_{safe_name}");
    let dir = std::path::Path::new(&state.config.upload_dir);
    let stored = async {
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(dir.join(&storage_key), &bytes).await
    }
    .await;
    if stored.is_err() {
        return Err(AppError::Internal(
            "Failed to upload file to storage".into(),
        ));
    }

    let file_url = format!("/uploads/{storage_key}");
    let now = crate::db::now_iso();
    sqlx::query(
        "INSERT INTO attachments (id, message_id, conversation_id, file_name, content_type,
                                  file_size, file_url, storage_key, upload_status, uploaded_by,
                                  created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'completed', $9, $10)",
    )
    .bind(&attachment_id)
    .bind(&id)
    .bind(&m.conversation_id)
    .bind(&filename)
    .bind(&mime)
    .bind(bytes.len() as i64)
    .bind(&file_url)
    .bind(&storage_key)
    .bind(&user.id)
    .bind(&now)
    .execute(&state.db)
    .await?;

    Ok(envelope::created(json!({
        "id": attachment_id,
        "messageId": id,
        "filename": filename,
        "mimeType": mime,
        "fileSize": bytes.len(),
        "url": file_url,
        "createdAt": now,
    })))
}
