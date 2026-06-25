//! Upload validation: size caps, type allow-lists per platform context,
//! filename rules, prohibited extensions, content-signature checks
//! (CRD 3187-3198).

pub const GLOBAL_MAX: usize = 10 * 1024 * 1024;
pub const IMAGE_MAX: usize = 5 * 1024 * 1024;
pub const VIDEO_MAX: usize = 20 * 1024 * 1024;
pub const AUDIO_MAX: usize = 10 * 1024 * 1024;
pub const DOCUMENT_MAX: usize = 10 * 1024 * 1024;
pub const ADMIN_MAX: usize = 50 * 1024 * 1024;

pub const IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];
pub const VIDEO_TYPES: &[&str] = &["video/mp4", "video/webm", "video/quicktime"];
pub const AUDIO_TYPES: &[&str] = &["audio/mpeg", "audio/mp4", "audio/ogg", "audio/wav", "audio/x-m4a"];
pub const DOCUMENT_TYPES: &[&str] = &[
    "application/pdf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "text/plain",
    "text/csv",
];
pub const ARCHIVE_TYPES: &[&str] = &["application/zip", "application/gzip", "application/x-rar-compressed"];

const BLOCKED_EXTENSIONS: &[&str] = &[
    "exe", "bat", "cmd", "com", "sh", "ps1", "msi", "scr", "dll", "js", "vbs", "jar", "app",
];
const RESERVED_NAMES: &[&str] = &["con", "prn", "aux", "nul", "com1", "com2", "lpt1", "lpt2"];

/// Allowed content types for one platform context (CRD 3192).
pub fn allowed_types(platform: &str) -> Vec<&'static str> {
    let mut types: Vec<&str> = Vec::new();
    types.extend(IMAGE_TYPES);
    types.extend(VIDEO_TYPES);
    types.extend(AUDIO_TYPES);
    match platform {
        "line" => {}
        "facebook" => types.extend(DOCUMENT_TYPES),
        "admin" => {
            types.extend(DOCUMENT_TYPES);
            types.extend(ARCHIVE_TYPES);
        }
        _ => {
            // system (default): documents + archives
            types.extend(DOCUMENT_TYPES);
            types.extend(ARCHIVE_TYPES);
        }
    }
    types
}

/// Derived type category from the content type (CRD 3204).
pub fn file_category(content_type: &str) -> &'static str {
    if content_type.starts_with("image/") {
        "image"
    } else if content_type.starts_with("video/") {
        "video"
    } else if content_type.starts_with("audio/") {
        "audio"
    } else if ARCHIVE_TYPES.contains(&content_type) {
        "archive"
    } else if DOCUMENT_TYPES.contains(&content_type) || content_type.starts_with("text/") {
        "document"
    } else {
        "other"
    }
}

pub fn size_cap(content_type: &str, platform: &str) -> usize {
    if platform == "admin" {
        return ADMIN_MAX;
    }
    match file_category(content_type) {
        "image" => IMAGE_MAX,
        "video" => VIDEO_MAX,
        "audio" => AUDIO_MAX,
        "document" | "archive" => DOCUMENT_MAX,
        _ => GLOBAL_MAX,
    }
}

pub fn extension_of(filename: &str) -> Option<String> {
    filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

/// Filename rules: required, <=255 chars, no path/control/reserved characters,
/// no reserved device names, no blocked extensions (CRD 3193-3194).
pub fn validate_filename(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Filename is required".into());
    }
    if name.chars().count() > 255 {
        return Err("Filename exceeds 255 characters".into());
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(|c| c.is_control())
        || name.contains("..")
    {
        return Err("Filename contains invalid characters".into());
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err("Filename uses a reserved device name".into());
    }
    if let Some(ext) = extension_of(name) {
        if BLOCKED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(format!("File extension '.{ext}' is not allowed"));
        }
    }
    Ok(())
}

/// Sanitized filename for storage (CRD 3079): strip path/dangerous chars,
/// collapse whitespace, cap length.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || ['/', '\\', ':', '*', '?', '"', '<', '>', '|'].contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(255).collect()
}

/// Leading-byte signature check against the declared content type; unknown
/// signatures fail closed and empty files are rejected (CRD 3195).
pub fn check_signature(content_type: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("File is empty".into());
    }
    let ok = match content_type {
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/png" => bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
        "image/gif" => bytes.starts_with(b"GIF8"),
        "image/webp" => bytes.len() > 11 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        "video/mp4" | "audio/mp4" | "audio/x-m4a" | "video/quicktime" => {
            bytes.len() > 11 && &bytes[4..8] == b"ftyp"
        }
        "video/webm" => bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]),
        "audio/mpeg" => bytes.starts_with(b"ID3") || bytes.starts_with(&[0xFF, 0xFB]) || bytes.starts_with(&[0xFF, 0xF3]),
        "audio/ogg" => bytes.starts_with(b"OggS"),
        "audio/wav" => bytes.len() > 11 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE",
        "application/pdf" => bytes.starts_with(b"%PDF"),
        "application/msword" | "application/vnd.ms-excel" => {
            bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
        }
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/zip" => bytes.starts_with(b"PK"),
        "application/gzip" => bytes.starts_with(&[0x1F, 0x8B]),
        "application/x-rar-compressed" => bytes.starts_with(b"Rar!"),
        "text/plain" | "text/csv" => {
            std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0)
        }
        _ => false, // unknown signatures fail closed
    };
    if ok {
        Ok(())
    } else {
        Err("File content does not match its declared type (corrupted file)".into())
    }
}

/// Extension appended to old extension-less filenames on download (CRD 3127).
pub fn extension_for_type(content_type: &str) -> Option<&'static str> {
    Some(match content_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/wav" => "wav",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "text/plain" => "txt",
        "text/csv" => "csv",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::validate_filename;

    #[test]
    fn validate_filename_rejects_paths_and_parent_segments() {
        assert!(validate_filename("../secret.txt").is_err());
        assert!(validate_filename("nested/file.txt").is_err());
        assert!(validate_filename("safe..txt").is_err());
    }

    #[test]
    fn validate_filename_rejects_blocked_extensions() {
        assert!(validate_filename("run.sh").is_err());
        assert!(validate_filename("payload.js").is_err());
    }

    #[test]
    fn validate_filename_accepts_safe_names() {
        assert!(validate_filename("report.csv").is_ok());
        assert!(validate_filename("photo 1.png").is_ok());
    }
}
