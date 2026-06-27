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
pub const AUDIO_TYPES: &[&str] = &[
    "audio/mpeg",
    "audio/mp4",
    "audio/ogg",
    "audio/wav",
    "audio/x-m4a",
];
pub const DOCUMENT_TYPES: &[&str] = &[
    "application/pdf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "text/plain",
    "text/csv",
];
pub const ARCHIVE_TYPES: &[&str] = &[
    "application/zip",
    "application/gzip",
    "application/x-rar-compressed",
];

const BLOCKED_EXTENSIONS: &[&str] = &[
    "exe", "bat", "cmd", "com", "sh", "ps1", "msi", "scr", "dll", "js", "vbs", "jar", "app",
];
const RESERVED_NAMES: &[&str] = &["con", "prn", "aux", "nul", "com1", "com2", "lpt1", "lpt2"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileValidationError {
    FilenameRequired,
    FilenameTooLong,
    FilenameInvalidCharacters,
    FilenameReservedDeviceName,
    BlockedExtension(String),
    ContentTypeNotAllowed {
        content_type: String,
        platform: String,
    },
    EmptyFile,
    FileTooLarge {
        max_bytes: usize,
    },
    SignatureMismatch,
}

impl std::fmt::Display for FileValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FilenameRequired => f.write_str("Filename is required"),
            Self::FilenameTooLong => f.write_str("Filename exceeds 255 characters"),
            Self::FilenameInvalidCharacters => f.write_str("Filename contains invalid characters"),
            Self::FilenameReservedDeviceName => f.write_str("Filename uses a reserved device name"),
            Self::BlockedExtension(ext) => write!(f, "File extension '.{ext}' is not allowed"),
            Self::ContentTypeNotAllowed {
                content_type,
                platform,
            } => write!(
                f,
                "Content type '{content_type}' is not allowed for platform '{platform}'"
            ),
            Self::EmptyFile => f.write_str("File is empty"),
            Self::FileTooLarge { max_bytes } => write!(f, "File too large (max {max_bytes} bytes)"),
            Self::SignatureMismatch => {
                f.write_str("File content does not match its declared type (corrupted file)")
            }
        }
    }
}

impl std::error::Error for FileValidationError {}

pub type FileValidationResult<T = ()> = std::result::Result<T, FileValidationError>;

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
    filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
}

/// Filename rules: required, <=255 chars, no path/control/reserved characters,
/// no reserved device names, no blocked extensions (CRD 3193-3194).
pub fn validate_filename(name: &str) -> FileValidationResult {
    if name.trim().is_empty() {
        return Err(FileValidationError::FilenameRequired);
    }
    if name.chars().count() > 255 {
        return Err(FileValidationError::FilenameTooLong);
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(|c| c.is_control())
        || name.contains("..")
    {
        return Err(FileValidationError::FilenameInvalidCharacters);
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(FileValidationError::FilenameReservedDeviceName);
    }
    if let Some(ext) = extension_of(name) {
        if BLOCKED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(FileValidationError::BlockedExtension(ext));
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
pub fn check_signature(content_type: &str, bytes: &[u8]) -> FileValidationResult {
    if bytes.is_empty() {
        return Err(FileValidationError::EmptyFile);
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
        "audio/mpeg" => {
            bytes.starts_with(b"ID3")
                || bytes.starts_with(&[0xFF, 0xFB])
                || bytes.starts_with(&[0xFF, 0xF3])
        }
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
        "text/plain" | "text/csv" => std::str::from_utf8(bytes).is_ok() && !bytes.contains(&0),
        _ => false, // unknown signatures fail closed
    };
    if ok {
        Ok(())
    } else {
        Err(FileValidationError::SignatureMismatch)
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
    use super::{check_signature, validate_filename, FileValidationError};

    #[test]
    fn validate_filename_rejects_paths_and_parent_segments() {
        assert!(validate_filename("../secret.txt").is_err());
        assert!(validate_filename("nested/file.txt").is_err());
        assert!(validate_filename("safe..txt").is_err());
    }

    #[test]
    fn validate_filename_rejects_blocked_extensions() {
        let err = validate_filename("run.sh").unwrap_err();

        assert_eq!(err, FileValidationError::BlockedExtension("sh".into()));
        assert_eq!(err.to_string(), "File extension '.sh' is not allowed");
        assert!(validate_filename("payload.js").is_err());
    }

    #[test]
    fn validate_filename_accepts_safe_names() {
        assert!(validate_filename("report.csv").is_ok());
        assert!(validate_filename("photo 1.png").is_ok());
    }

    #[test]
    fn check_signature_returns_typed_empty_file_error() {
        let err = check_signature("image/png", b"").unwrap_err();

        assert_eq!(err, FileValidationError::EmptyFile);
        assert_eq!(err.to_string(), "File is empty");
    }

    #[test]
    fn check_signature_returns_typed_mismatch_error() {
        let err = check_signature("image/png", b"not a png").unwrap_err();

        assert_eq!(err, FileValidationError::SignatureMismatch);
        assert_eq!(
            err.to_string(),
            "File content does not match its declared type (corrupted file)"
        );
    }
}
