//! File metadata persistence + local object store (CRD §4.4 Data Concepts).

use serde_json::{json, Value};
use sqlx::PgPool;
use std::path::{Component, Path, PathBuf};

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

pub async fn find(pool: &PgPool, id: &str) -> sqlx::Result<Option<FileRow>> {
    sqlx::query_as(&crate::db::pg_params(&format!(
        "SELECT {COLUMNS} FROM attachments WHERE id = $1"
    )))
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

pub async fn insert(pool: &PgPool, f: &NewFile<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO attachments
            (id, message_id, conversation_id, file_name, original_name, content_type, file_size,
             file_url, public_url, storage_key, upload_status, uploaded_by, platform, file_type,
             created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
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

pub fn object_path(upload_dir: &str, key: &str) -> std::io::Result<PathBuf> {
    let key_path = Path::new(key);
    if key_path.is_absolute()
        || key_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid storage key",
        ));
    }
    Ok(Path::new(upload_dir).join(key_path))
}

async fn canonical_upload_dir(upload_dir: &str) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(upload_dir).await?;
    tokio::fs::canonicalize(upload_dir).await
}

async fn ensure_parent_under_upload_dir(upload_dir: &str, path: &Path) -> std::io::Result<()> {
    let base = canonical_upload_dir(upload_dir).await?;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "storage key has no parent",
        )
    })?;
    tokio::fs::create_dir_all(parent).await?;
    let parent = tokio::fs::canonicalize(parent).await?;
    if parent.starts_with(base) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "storage key escapes upload directory",
        ))
    }
}

async fn canonical_object_under_upload_dir(
    upload_dir: &str,
    path: &Path,
) -> std::io::Result<PathBuf> {
    let base = canonical_upload_dir(upload_dir).await?;
    let object = tokio::fs::canonicalize(path).await?;
    if object.starts_with(base) {
        Ok(object)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "storage key escapes upload directory",
        ))
    }
}

pub async fn put_object(upload_dir: &str, key: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = object_path(upload_dir, key)?;
    ensure_parent_under_upload_dir(upload_dir, &path).await?;
    tokio::fs::write(path, bytes).await
}

pub async fn get_object(upload_dir: &str, key: &str) -> Option<Vec<u8>> {
    let path = object_path(upload_dir, key).ok()?;
    let path = canonical_object_under_upload_dir(upload_dir, &path)
        .await
        .ok()?;
    tokio::fs::read(path).await.ok()
}

/// Idempotent delete: an absent object is treated as already deleted (CRD 3060).
pub async fn delete_object(upload_dir: &str, key: &str) {
    let Ok(path) = object_path(upload_dir, key) else {
        return;
    };
    let Ok(path) = canonical_object_under_upload_dir(upload_dir, &path).await else {
        return;
    };
    let _ = tokio::fs::remove_file(path).await;
}

#[cfg(test)]
mod tests {
    use super::{get_object, object_path, put_object};

    #[test]
    fn object_path_rejects_absolute_and_parent_components() {
        assert!(object_path("/tmp/uploads", "../secret.txt").is_err());
        assert!(object_path("/tmp/uploads", "/etc/passwd").is_err());
        assert!(object_path("/tmp/uploads", "uploads/../secret.txt").is_err());
        assert!(object_path("/tmp/uploads", "uploads/a.txt").is_ok());
    }

    #[tokio::test]
    async fn object_store_rejects_traversal_keys() {
        let dir = tempfile::tempdir().unwrap();
        let upload_dir = dir.path().join("uploads");
        let outside = dir.path().join("outside.txt");
        let key = "../outside.txt";

        let err = put_object(upload_dir.to_str().unwrap(), key, b"secret")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!outside.exists());
        assert!(get_object(upload_dir.to_str().unwrap(), key)
            .await
            .is_none());
    }
}
