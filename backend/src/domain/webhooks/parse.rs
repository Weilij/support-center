//! Platform event normalization (CRD §4.2): raw LINE / Facebook message
//! objects -> the platform-agnostic normalized inbound message (CRD 2842-2843).

use serde_json::{json, Map, Value};

/// Normalized inbound message (CRD 2842): display content, message kind,
/// optional media reference, and a free-form metadata bag.
#[derive(Debug, Clone)]
pub struct Normalized {
    pub content: String,
    /// text | image | video | audio | file | location | sticker
    pub kind: String,
    pub media: Option<Value>,
    pub metadata: Map<String, Value>,
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn f(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// LINE content-retrieval URLs for downloadable media (CRD 2750-2753).
fn line_content_url(message_id: &str) -> String {
    format!("https://api-data.line.me/v2/bot/message/{message_id}/content")
}

fn line_preview_url(message_id: &str) -> String {
    format!("https://api-data.line.me/v2/bot/message/{message_id}/content/preview")
}

/// Normalize one LINE message object per the documented rules (CRD 2748-2757).
pub fn normalize_line(message: &Value) -> Normalized {
    let raw_kind = message.get("type").and_then(Value::as_str).unwrap_or("unknown").to_string();
    let media_id = s(message, "id").unwrap_or_default();
    let file_name = s(message, "fileName");

    // Type self-correction (CRD 2757): a non-file kind carrying a file name is
    // reclassified as a file.
    let kind = if raw_kind != "file" && file_name.is_some() { "file".to_string() } else { raw_kind.clone() };

    match kind.as_str() {
        "text" => Normalized {
            content: s(message, "text").unwrap_or_default(),
            kind: "text".into(),
            media: None,
            metadata: Map::new(),
        },
        "image" => Normalized {
            content: "[Image]".into(),
            kind: "image".into(),
            media: Some(json!({
                "type": "image",
                "mediaId": media_id,
                "contentUrl": line_content_url(&media_id),
                "previewUrl": line_preview_url(&media_id),
            })),
            metadata: Map::new(),
        },
        "video" => Normalized {
            content: "[Video]".into(),
            kind: "video".into(),
            media: Some(json!({
                "type": "video",
                "mediaId": media_id,
                "contentUrl": line_content_url(&media_id),
                "previewUrl": line_preview_url(&media_id),
            })),
            metadata: Map::new(),
        },
        "audio" => Normalized {
            content: "[Voice message]".into(),
            kind: "audio".into(),
            media: Some(json!({
                "type": "audio",
                "mediaId": media_id,
                "contentUrl": line_content_url(&media_id),
                "duration": message.get("duration").and_then(Value::as_i64).unwrap_or(0),
            })),
            metadata: Map::new(),
        },
        "file" => {
            let name = file_name.unwrap_or_else(|| "Unknown file".into());
            Normalized {
                content: format!("[File] {name}"),
                kind: "file".into(),
                media: Some(json!({
                    "type": "file",
                    "mediaId": media_id,
                    "contentUrl": line_content_url(&media_id),
                    "fileName": name,
                    "fileSize": message.get("fileSize").and_then(Value::as_i64),
                })),
                metadata: Map::new(),
            }
        }
        "location" => {
            let title = s(message, "title").unwrap_or_else(|| "Location".into());
            let address = s(message, "address").unwrap_or_else(|| "Unknown address".into());
            Normalized {
                content: format!("[Location] {title} - {address}"),
                kind: "location".into(),
                // Locations are non-downloadable (CRD 2843).
                media: Some(json!({
                    "type": "location",
                    "title": title,
                    "address": address,
                    "latitude": f(message, "latitude"),
                    "longitude": f(message, "longitude"),
                })),
                metadata: Map::new(),
            }
        }
        "sticker" => Normalized {
            content: "[Sticker]".into(),
            kind: "sticker".into(),
            // Stickers are non-downloadable (CRD 2843).
            media: Some(json!({
                "type": "sticker",
                "packageId": s(message, "packageId"),
                "stickerId": s(message, "stickerId"),
            })),
            metadata: Map::new(),
        },
        other => {
            // Unknown kinds become a bracketed label, text-equivalent, with
            // the original kind retained in metadata (CRD 2756).
            let mut metadata = Map::new();
            metadata.insert("originalType".into(), json!(other));
            Normalized {
                content: format!("[{other}]"),
                kind: "text".into(),
                media: None,
                metadata,
            }
        }
    }
}

/// Normalize one Facebook/Instagram message object per the documented rules
/// (CRD 2798-2801).
pub fn normalize_facebook(message: &Value) -> Normalized {
    let mut metadata = Map::new();
    if let Some(qr) = message.get("quick_reply") {
        metadata.insert("quickReply".into(), qr.clone());
    }
    if let Some(rt) = message.get("reply_to") {
        metadata.insert("replyTo".into(), rt.clone());
    }
    let attachments = message.get("attachments").and_then(Value::as_array).cloned();
    if let Some(atts) = &attachments {
        if !atts.is_empty() {
            // All raw attachments are retained in metadata (CRD 2800).
            metadata.insert("attachments".into(), json!(atts));
        }
    }

    let text = message.get("text").and_then(Value::as_str).unwrap_or("");
    if !text.is_empty() {
        return Normalized { content: text.into(), kind: "text".into(), media: None, metadata };
    }

    // The first attachment determines the kind (CRD 2800).
    let Some(first) = attachments.as_ref().and_then(|a| a.first()) else {
        return Normalized {
            content: "[Unknown message]".into(),
            kind: "text".into(),
            media: None,
            metadata,
        };
    };

    let att_kind = first.get("type").and_then(Value::as_str).unwrap_or("other");
    let payload = first.get("payload").cloned().unwrap_or(Value::Null);
    let url = s(&payload, "url");
    match att_kind {
        "image" | "video" | "audio" => Normalized {
            content: match att_kind {
                "image" => "[Image]",
                "video" => "[Video]",
                _ => "[Voice message]",
            }
            .into(),
            kind: att_kind.into(),
            media: Some(json!({ "type": att_kind, "contentUrl": url })),
            metadata,
        },
        "file" => {
            let title = s(first, "title")
                .or_else(|| s(&payload, "title"))
                .unwrap_or_else(|| "Unknown file".into());
            Normalized {
                content: format!("[File] {title}"),
                kind: "file".into(),
                media: Some(json!({ "type": "file", "contentUrl": url, "fileName": title })),
                metadata,
            }
        }
        "location" => {
            let coords = payload.get("coordinates").cloned().unwrap_or(Value::Null);
            let title = s(first, "title")
                .or_else(|| s(&payload, "title"))
                .unwrap_or_else(|| "Location".into());
            Normalized {
                content: format!("[Location] {title}"),
                kind: "location".into(),
                media: Some(json!({
                    "type": "location",
                    "title": title,
                    "latitude": f(&coords, "lat"),
                    "longitude": f(&coords, "long"),
                })),
                metadata,
            }
        }
        // A fallback attachment maps to text using its title or a link
        // placeholder (CRD 2800).
        "fallback" => Normalized {
            content: s(first, "title")
                .or_else(|| s(&payload, "title"))
                .unwrap_or_else(|| "[Link]".into()),
            kind: "text".into(),
            media: None,
            metadata,
        },
        other => Normalized {
            content: format!("[{other}]"),
            kind: "text".into(),
            media: None,
            metadata,
        },
    }
}

/// A postback (button / quick-reply click) as a normalized text message.
pub fn normalize_facebook_postback(postback: &Value) -> Normalized {
    let title = postback.get("title").and_then(Value::as_str).unwrap_or("");
    let payload = postback.get("payload").and_then(Value::as_str).unwrap_or("");
    let content = if !title.is_empty() { title } else { payload };
    let mut metadata = Map::new();
    metadata.insert("postback".into(), postback.clone());
    Normalized {
        content: if content.is_empty() { "[Postback]".into() } else { content.into() },
        kind: "text".into(),
        media: None,
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_text_and_placeholders() {
        let n = normalize_line(&json!({ "type": "text", "id": "m1", "text": "hello" }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("text", "hello"));
        let n = normalize_line(&json!({ "type": "text", "id": "m1" }));
        assert_eq!(n.content, ""); // empty string when absent

        let n = normalize_line(&json!({ "type": "image", "id": "m2" }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("image", "[Image]"));
        let media = n.media.unwrap();
        assert!(media["contentUrl"].as_str().unwrap().contains("/m2/content"));
        assert!(media["previewUrl"].as_str().unwrap().ends_with("/preview"));

        let n = normalize_line(&json!({ "type": "audio", "id": "m3", "duration": 1200 }));
        assert_eq!(n.content, "[Voice message]");
        assert_eq!(n.media.unwrap()["duration"], 1200);

        let n = normalize_line(&json!({ "type": "file", "id": "m4" }));
        assert_eq!(n.content, "[File] Unknown file");

        let n = normalize_line(&json!({ "type": "location", "id": "m5" }));
        assert_eq!(n.content, "[Location] Location - Unknown address");
        assert_eq!(n.media.unwrap()["latitude"], 0.0);

        let n = normalize_line(&json!({
            "type": "sticker", "id": "m6", "packageId": "p1", "stickerId": "s1"
        }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("sticker", "[Sticker]"));
    }

    #[test]
    fn line_unknown_kind_and_file_self_correction() {
        let n = normalize_line(&json!({ "type": "imagemap", "id": "m7" }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("text", "[imagemap]"));
        assert_eq!(n.metadata["originalType"], "imagemap");

        // Non-file kind carrying a file name is reclassified as a file.
        let n = normalize_line(&json!({ "type": "video", "id": "m8", "fileName": "clip.mp4" }));
        assert_eq!(n.kind, "file");
        assert_eq!(n.content, "[File] clip.mp4");
    }

    #[test]
    fn facebook_text_attachments_and_fallback() {
        let n = normalize_facebook(&json!({ "mid": "f1", "text": "hi there" }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("text", "hi there"));

        let n = normalize_facebook(&json!({
            "mid": "f2",
            "attachments": [{ "type": "image", "payload": { "url": "https://cdn/x.jpg" } }]
        }));
        assert_eq!(n.kind, "image");
        assert_eq!(n.media.unwrap()["contentUrl"], "https://cdn/x.jpg");
        assert!(n.metadata.contains_key("attachments"));

        let n = normalize_facebook(&json!({
            "mid": "f3",
            "attachments": [{ "type": "fallback", "title": "A shared link" }]
        }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("text", "A shared link"));

        let n = normalize_facebook(&json!({ "mid": "f4" }));
        assert_eq!(n.content, "[Unknown message]");
    }

    #[test]
    fn facebook_postback_uses_title_then_payload() {
        let n = normalize_facebook_postback(&json!({ "title": "Get Started", "payload": "START" }));
        assert_eq!(n.content, "Get Started");
        assert_eq!(n.kind, "text");
        let n = normalize_facebook_postback(&json!({ "payload": "ONLY_PAYLOAD" }));
        assert_eq!(n.content, "ONLY_PAYLOAD");
    }
}
