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

fn first_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| v.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn first_i64(v: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        v.get(*key).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
    })
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
    let raw_kind = message
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let media_id = s(message, "id").unwrap_or_default();
    let file_name = s(message, "fileName");

    // Type self-correction (CRD 2757): a non-file kind carrying a file name is
    // reclassified as a file.
    let kind = if raw_kind != "file" && file_name.is_some() {
        "file".to_string()
    } else {
        raw_kind.clone()
    };

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
    let attachments = message
        .get("attachments")
        .and_then(Value::as_array)
        .cloned();
    if let Some(atts) = &attachments {
        if !atts.is_empty() {
            // All raw attachments are retained in metadata (CRD 2800).
            metadata.insert("attachments".into(), json!(atts));
        }
    }

    let text = message.get("text").and_then(Value::as_str).unwrap_or("");
    if !text.is_empty() {
        return Normalized {
            content: text.into(),
            kind: "text".into(),
            media: None,
            metadata,
        };
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

/// Instagram inbound message. Delegates to `normalize_facebook` (same envelope)
/// and labels story mentions / replies, keeping the raw object in metadata.
pub fn normalize_instagram(message: &Value) -> Normalized {
    let is_story_mention = message
        .get("attachments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .any(|att| att.get("type").and_then(Value::as_str) == Some("story_mention"))
        })
        .unwrap_or(false);
    let story_reply = message
        .get("reply_to")
        .and_then(|r| r.get("story"))
        .cloned();

    let mut n = normalize_facebook(message);
    if is_story_mention {
        n.content = "[Story mention]".into();
        if let Some(atts) = message.get("attachments") {
            n.metadata.insert("storyMention".into(), atts.clone());
        }
    } else if let Some(story) = story_reply {
        if n.content.is_empty() || n.content == "[Unknown message]" {
            n.content = "[Story reply]".into();
        }
        n.metadata.insert("storyReply".into(), story);
    }
    n
}

/// Normalize one Shopee SellerChat/Webchat push message. Shopee chat push
/// payloads vary by event revision, so this accepts common text/media shapes
/// while preserving raw fields in metadata.
pub fn normalize_shopee(message: &Value) -> Normalized {
    let mut metadata = Map::new();
    metadata.insert("rawShopeeMessage".into(), message.clone());
    if let Some(conversation_id) = message
        .get("conversation_id")
        .or_else(|| message.get("conversationId"))
        .and_then(Value::as_str)
    {
        metadata.insert("shopeeConversationId".into(), json!(conversation_id));
    }

    let message_type = message
        .get("message_type")
        .or_else(|| message.get("messageType"))
        .or_else(|| message.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("text");
    let content = message.get("content").unwrap_or(&Value::Null);

    if message_type == "text" {
        let text = content
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| message.get("text").and_then(Value::as_str))
            .unwrap_or_default();
        return Normalized {
            content: text.to_string(),
            kind: "text".into(),
            media: None,
            metadata,
        };
    }

    if matches!(message_type, "image" | "video" | "audio" | "file") {
        let url = first_str(
            content,
            &["url", "file_url", "fileUrl", "image_url", "imageUrl"],
        )
        .or_else(|| {
            first_str(
                message,
                &["url", "file_url", "fileUrl", "image_url", "imageUrl"],
            )
        });
        let preview_url = first_str(
            content,
            &["thumbnail_url", "thumbnailUrl", "preview_url", "previewUrl"],
        )
        .or_else(|| {
            first_str(
                message,
                &["thumbnail_url", "thumbnailUrl", "preview_url", "previewUrl"],
            )
        });
        let file_name = first_str(content, &["file_name", "fileName", "name"])
            .or_else(|| first_str(message, &["file_name", "fileName", "name"]));
        let label = match message_type {
            "image" => "[Image]".to_string(),
            "video" => "[Video]".to_string(),
            "audio" => "[Voice message]".to_string(),
            _ => format!("[File] {}", file_name.as_deref().unwrap_or("Unknown file")),
        };
        let mut media = json!({ "type": message_type, "contentUrl": url });
        if let Some(url) = preview_url {
            media["previewUrl"] = json!(url);
        }
        if let Some(name) = file_name {
            media["fileName"] = json!(name);
        }
        return Normalized {
            content: label,
            kind: message_type.into(),
            media: Some(media),
            metadata,
        };
    }

    if message_type == "sticker" {
        let sticker_id = first_str(content, &["sticker_id", "stickerId", "id"])
            .or_else(|| first_str(message, &["sticker_id", "stickerId"]));
        let package_id = first_str(content, &["package_id", "packageId"])
            .or_else(|| first_str(message, &["package_id", "packageId"]));
        let url = first_str(content, &["url", "image_url", "imageUrl"])
            .or_else(|| first_str(message, &["url", "image_url", "imageUrl"]));
        return Normalized {
            content: "[Sticker]".into(),
            kind: "sticker".into(),
            media: Some(json!({
                "type": "sticker",
                "stickerId": sticker_id,
                "packageId": package_id,
                "contentUrl": url,
            })),
            metadata,
        };
    }

    if matches!(
        message_type,
        "product" | "item" | "order" | "invoice" | "voucher" | "bundle_message"
    ) {
        let title = first_str(
            content,
            &[
                "name",
                "title",
                "item_name",
                "itemName",
                "order_sn",
                "orderSn",
            ],
        )
        .or_else(|| {
            first_str(
                message,
                &[
                    "name",
                    "title",
                    "item_name",
                    "itemName",
                    "order_sn",
                    "orderSn",
                ],
            )
        });
        let url = first_str(content, &["url", "item_url", "itemUrl"])
            .or_else(|| first_str(message, &["url", "item_url", "itemUrl"]));
        let image_url = first_str(
            content,
            &["image_url", "imageUrl", "thumbnail_url", "thumbnailUrl"],
        )
        .or_else(|| {
            first_str(
                message,
                &["image_url", "imageUrl", "thumbnail_url", "thumbnailUrl"],
            )
        });
        let item_id = first_i64(content, &["item_id", "itemId"])
            .or_else(|| first_i64(message, &["item_id", "itemId"]));
        let mut rich = json!({
            "type": message_type,
            "title": title,
            "url": url,
            "imageUrl": image_url,
        });
        if let Some(id) = item_id {
            rich["itemId"] = json!(id);
        }
        metadata.insert("shopeeRichContent".into(), rich);
        let label = match message_type {
            "product" | "item" => "Product",
            "order" | "invoice" => "Order",
            "voucher" => "Voucher",
            "bundle_message" => "Bundle message",
            _ => "Shopee card",
        };
        let content = title
            .map(|title| format!("[{label}] {title}"))
            .unwrap_or_else(|| format!("[{label}]"));
        return Normalized {
            content,
            kind: "text".into(),
            media: None,
            metadata,
        };
    }

    Normalized {
        content: format!("[{message_type}]"),
        kind: "text".into(),
        media: None,
        metadata,
    }
}

/// A postback (button / quick-reply click) as a normalized text message.
pub fn normalize_facebook_postback(postback: &Value) -> Normalized {
    let title = postback.get("title").and_then(Value::as_str).unwrap_or("");
    let payload = postback
        .get("payload")
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = if !title.is_empty() { title } else { payload };
    let mut metadata = Map::new();
    metadata.insert("postback".into(), postback.clone());
    Normalized {
        content: if content.is_empty() {
            "[Postback]".into()
        } else {
            content.into()
        },
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
        assert!(media["contentUrl"]
            .as_str()
            .unwrap()
            .contains("/m2/content"));
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
        assert_eq!(
            (n.kind.as_str(), n.content.as_str()),
            ("sticker", "[Sticker]")
        );
    }

    #[test]
    fn line_unknown_kind_and_file_self_correction() {
        let n = normalize_line(&json!({ "type": "imagemap", "id": "m7" }));
        assert_eq!(
            (n.kind.as_str(), n.content.as_str()),
            ("text", "[imagemap]")
        );
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
        assert_eq!(
            (n.kind.as_str(), n.content.as_str()),
            ("text", "A shared link")
        );

        let n = normalize_facebook(&json!({ "mid": "f4" }));
        assert_eq!(n.content, "[Unknown message]");
    }

    #[test]
    fn instagram_story_mention_is_labelled() {
        let n = normalize_instagram(&json!({
            "attachments": [{ "type": "story_mention", "payload": { "url": "https://x/s.jpg" } }]
        }));
        assert_eq!(n.content, "[Story mention]");
        assert!(n.metadata.contains_key("storyMention"));
    }

    #[test]
    fn instagram_story_reply_is_labelled() {
        let n = normalize_instagram(&json!({
            "reply_to": { "story": { "id": "STORY1", "url": "https://x/s.jpg" } }
        }));
        assert_eq!(n.content, "[Story reply]");
        assert!(n.metadata.contains_key("storyReply"));
    }

    #[test]
    fn instagram_plain_text_passes_through() {
        let n = normalize_instagram(&json!({ "mid": "m1", "text": "hi" }));
        assert_eq!(n.content, "hi");
        assert_eq!(n.kind, "text");
    }

    #[test]
    fn shopee_text_and_media_are_normalized() {
        let n = normalize_shopee(&json!({
            "message_type": "text",
            "content": { "text": "hello" },
            "conversation_id": "c1"
        }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("text", "hello"));
        assert_eq!(n.metadata["shopeeConversationId"], "c1");

        let n = normalize_shopee(&json!({
            "message_type": "image",
            "content": { "url": "https://cdn/i.jpg" }
        }));
        assert_eq!((n.kind.as_str(), n.content.as_str()), ("image", "[Image]"));
        assert_eq!(n.media.unwrap()["contentUrl"], "https://cdn/i.jpg");
    }

    #[test]
    fn shopee_rich_chat_cards_are_labelled_and_preserved() {
        let n = normalize_shopee(&json!({
            "message_type": "sticker",
            "content": { "sticker_id": "s1", "package_id": "p1", "url": "https://cdn/s.png" }
        }));
        assert_eq!(
            (n.kind.as_str(), n.content.as_str()),
            ("sticker", "[Sticker]")
        );
        assert_eq!(n.media.unwrap()["stickerId"], "s1");

        let n = normalize_shopee(&json!({
            "message_type": "product",
            "content": {
                "item_id": "123",
                "name": "Sneakers",
                "image_url": "https://cdn/p.jpg",
                "url": "https://shop/p/123"
            }
        }));
        assert_eq!(
            (n.kind.as_str(), n.content.as_str()),
            ("text", "[Product] Sneakers")
        );
        assert_eq!(n.metadata["shopeeRichContent"]["itemId"], 123);
        assert_eq!(n.metadata["shopeeRichContent"]["url"], "https://shop/p/123");
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
