//! File metadata persistence + local object store (CRD §4.4 Data Concepts).

use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::db::now_iso;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FileRow {
    pub id: String,
    pub message_id: Option<String>,
    pub conversation_id: Option<String>,
    pub file_name: Option<String>,
    pub original_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
    pub file_url: Option<String>,
    pub public_url: Option<String>,
    pub storage_key: Option<String>,
    pub upload_status: String,
    pub uploaded_by: Option<String>,
    pub platform: Option<String>,
    pub file_type: Option<String>,
    pub created_at: String,
    pub updated_at: Option<String>,
}

const COLUMNS: &str = "id, message_id, conversation_id, file_name, original_name, content_type,
    file_size, file_url, public_url, storage_key, upload_status, uploaded_by, platform,
    file_type, created_at, updated_at";

pub fn file_view(row: &FileRow) -> Value {
    json!({
        "id": row.id,
        "filename": row.file_name,
        "originalName": row.original_name,
        "contentType": row.content_type,
        "size": row.file_size,
        "fileType": row.file_type,
        "url": row.file_url,
        "publicUrl": row.public_url,
        "platform": row.platform,
        "conversationId": row.conversation_id,
        "messageId": row.message_id,
        "uploadStatus": row.upload_status,
        "uploadedBy": row.uploaded_by,
        "createdAt": row.created_at,
        "updatedAt": row.updated_at,
    })
}

pub async fn find(pool: &SqlitePool, id: &str) -> sqlx::Result<Option<FileRow>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM attachments WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[allow(clippy::too_many_arguments)]
pub struct NewFile<'a> {
    pub id: &'a str,
    pub filename: &'a str,
    pub original_name: &'a str,
    pub content_type: &'a str,
    pub size: i64,
    pub storage_key: &'a str,
    pub file_url: &'a str,
    pub public_url: Option<&'a str>,
    pub platform: &'a str,
    pub file_type: &'a str,
    pub conversation_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub uploaded_by: &'a str,
    pub status: &'a str, // completed | pending
}

pub async fn insert(pool: &SqlitePool, f: &NewFile<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO attachments
            (id, message_id, conversation_id, file_name, original_name, content_type, file_size,
             file_url, public_url, storage_key, upload_status, uploaded_by, platform, file_type,
             created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(f.id)
    .bind(f.message_id)
    .bind(f.conversation_id)
    .bind(f.filename)
    .bind(f.original_name)
    .bind(f.content_type)
    .bind(f.size)
    .bind(f.file_url)
    .bind(f.public_url)
    .bind(f.storage_key)
    .bind(f.status)
    .bind(f.uploaded_by)
    .bind(f.platform)
    .bind(f.file_type)
    .bind(now_iso())
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(())
}

/// Derived storage location: prefix/platform/type/date/unique-leaf
/// (CRD 3203 — the original filename is not preserved in the leaf).
pub fn storage_key(platform: &str, file_type: &str, extension: Option<&str>) -> String {
    let date = chrono::Utc::now().format("%Y/%m/%d");
    let leaf = uuid::Uuid::new_v4().to_string();
    match extension {
        Some(ext) => format!("uploads/{platform}/{file_type}/{date}/{leaf}.{ext}"),
        None => format!("uploads/{platform}/{file_type}/{date}/{leaf}"),
    }
}

// ------------------------------------------------------------ local object store

pub fn object_path(upload_dir: &str, key: &str) -> std::path::PathBuf {
    std::path::Path::new(upload_dir).join(key)
}

pub async fn put_object(upload_dir: &str, key: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = object_path(upload_dir, key);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, bytes).await
}

pub async fn get_object(upload_dir: &str, key: &str) -> Option<Vec<u8>> {
    tokio::fs::read(object_path(upload_dir, key)).await.ok()
}

/// Idempotent delete: an absent object is treated as already deleted (CRD 3060).
pub async fn delete_object(upload_dir: &str, key: &str) {
    let _ = tokio::fs::remove_file(object_path(upload_dir, key)).await;
}
