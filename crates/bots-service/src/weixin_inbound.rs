//! 微信 iLink 入站消息解析（`weixinProvider.ts` 的 `getupdates` → `BotInboundMessage` 部分）。
//!
//! 本切片解析文本/群标识/上下文 token/messageId，以及**附件元数据**（kind/文件名/MIME/
//! 大小/下载地址/base64/AES key）。媒体下载与 AES-128-ECB 解密、typing、`/getconfig`
//! 续期属于 runtime 切片，本模块不发起网络。
//!
//! 结构对齐 `payload` 的宽松形状：消息容器可能在 `data` 下，字段可能在 `msg`/`message`
//! 内层（平台多版本共存），全部按「外层优先、内层兜底」读取。

use serde_json::Value;

fn is_record(value: &Value) -> bool {
    value.is_object()
}

fn read_string(record: Option<&Value>, key: &str) -> String {
    record
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn read_number(record: Option<&Value>, key: &str) -> Option<f64> {
    record.and_then(|value| value.get(key)).and_then(|value| match value {
        Value::Number(number) => number.as_f64(),
        _ => None,
    })
}

fn read_number_or_string(record: Option<&Value>, key: &str) -> String {
    match record.and_then(|value| value.get(key)) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeixinInboundAttachment {
    pub id: String,
    pub kind: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: Option<u64>,
    pub provider_file_id: Option<String>,
    pub download_url: Option<String>,
    pub data_base64: Option<String>,
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeixinInboundActor {
    pub provider_user_id: String,
    pub display_name: Option<String>,
    pub chat_type: String,
    pub chat_id: Option<String>,
    pub provider_message_id: Option<String>,
    pub provider_context_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeixinInboundMessage {
    pub bot_id: String,
    pub text: String,
    pub attachments: Vec<WeixinInboundAttachment>,
    pub actor: WeixinInboundActor,
}

fn inner_message(raw: &Value) -> Option<&Value> {
    for key in ["msg", "message"] {
        if let Some(inner) = raw.get(key).filter(|value| is_record(value)) {
            return Some(inner);
        }
    }
    None
}

fn item_list<'a>(raw: &'a Value, inner: Option<&'a Value>) -> &'a [Value] {
    for source in [Some(raw), inner] {
        if let Some(items) = source
            .and_then(|value| value.get("item_list"))
            .and_then(Value::as_array)
        {
            return items.as_slice();
        }
    }
    &[]
}

fn text_item(item: &Value) -> String {
    let text_item = item.get("text_item").filter(|value| is_record(value));
    let value = read_string(text_item, "text");
    if !value.is_empty() {
        return value;
    }
    let direct = read_string(Some(item), "text");
    if !direct.is_empty() {
        return direct;
    }
    read_string(Some(item), "content")
}

fn message_text(raw: &Value, inner: Option<&Value>) -> String {
    for key in ["text", "content", "message"] {
        let value = read_string(Some(raw), key);
        if !value.is_empty() {
            return value;
        }
    }
    let joined = item_list(raw, inner)
        .iter()
        .map(text_item)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !joined.is_empty() {
        return joined;
    }
    let fallback = read_string(inner, "text");
    if fallback.is_empty() {
        read_string(inner, "content")
    } else {
        fallback
    }
}

fn infer_kind(media: &Value, filename: &str) -> String {
    let has = |key: &str| media.get(key).is_some_and(is_record);
    if has("image_item") {
        return "image".to_string();
    }
    let explicit = read_string(Some(media), "kind")
        + &read_string(Some(media), "media_type")
        + &read_string(Some(media), "mediaType")
        + &read_string(Some(media), "type_name");
    let mime = read_string(Some(media), "mime_type") + &read_string(Some(media), "mimeType");
    let normalized = format!("{}{}{}", explicit, mime, filename).to_lowercase();
    if normalized.contains("image")
        || normalized.contains("photo")
        || normalized.contains("picture")
        || [".svg", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic", ".bmp"]
            .iter()
            .any(|ext| filename.to_lowercase().ends_with(ext))
    {
        return "image".to_string();
    }
    if normalized.contains("audio") || normalized.contains("voice") {
        return "audio".to_string();
    }
    if normalized.contains("video") {
        return "video".to_string();
    }
    "file".to_string()
}

fn infer_mime(filename: &str, kind: &str) -> String {
    let lower = filename.to_lowercase();
    let from_ext = match () {
        _ if lower.ends_with(".svg") => "image/svg+xml",
        _ if lower.ends_with(".png") => "image/png",
        _ if lower.ends_with(".jpg") || lower.ends_with(".jpeg") => "image/jpeg",
        _ if lower.ends_with(".gif") => "image/gif",
        _ if lower.ends_with(".webp") => "image/webp",
        _ => "",
    };
    if !from_ext.is_empty() {
        return from_ext.to_string();
    }
    match kind {
        "image" => "image/jpeg",
        "audio" => "audio/mpeg",
        "video" => "video/mp4",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn attachment_item(item: &Value, index: usize) -> Option<WeixinInboundAttachment> {
    if !is_record(item) {
        return None;
    }
    if !text_item(item).is_empty() {
        return None;
    }
    let media = ["image_item", "file_item", "video_item", "audio_item", "media_item"]
        .iter()
        .find_map(|key| item.get(*key).filter(|value| is_record(value)))
        .unwrap_or(item);
    let media_payload = media.get("media").filter(|value| is_record(value));
    // 内层 media 覆盖外层（TS `{ ...media, ...mediaPayload }`）。
    let source = media_payload.unwrap_or(media);
    let lookup = |key: &str| source.get(key);
    let provider_file_id = [
        "file_id",
        "fileId",
        "media_id",
        "mediaId",
        "id",
        "encrypt_query_param",
        "encryptQueryParam",
        "media",
        "md5",
    ]
    .iter()
    .map(|key| read_number_or_string(Some(source), key))
    .find(|value| !value.is_empty());
    let download_url = ["url", "download_url", "downloadUrl", "full_url", "fullUrl"]
        .iter()
        .map(|key| read_string(Some(source), key))
        .find(|value| !value.is_empty());
    let data_base64 = ["data_base64", "dataBase64", "base64"]
        .iter()
        .map(|key| read_string(Some(source), key))
        .find(|value| !value.is_empty());
    if provider_file_id.is_none() && download_url.is_none() && data_base64.is_none() {
        return None;
    }
    // 与 TS `inferWeixinAttachmentKind({ ...item, ...mediaSource })` 同义：item 外层键优先
    // （image_item 包裹时媒体对象自身没有 image_item 键）。
    let kind = if item.get("image_item").is_some_and(is_record) {
        "image".to_string()
    } else {
        infer_kind(media, "")
    };
    let filename = {
        let explicit = ["filename", "file_name", "name"]
            .iter()
            .map(|key| read_string(Some(source), key))
            .find(|value| !value.is_empty());
        explicit.unwrap_or_else(|| {
            if kind == "image" {
                format!("weixin-image-{}.jpg", index + 1)
            } else {
                format!("weixin-attachment-{}", index + 1)
            }
        })
    };
    let mime_type = {
        let explicit = read_string(Some(source), "mime_type");
        if explicit.is_empty() {
            read_string(Some(source), "mimeType")
        } else {
            explicit
        }
    };
    let mime_type = if mime_type.is_empty() {
        infer_mime(&filename, &kind)
    } else {
        mime_type
    };
    let size_bytes = ["size", "sizeBytes", "file_size", "len", "mid_size"]
        .iter()
        .find_map(|key| read_number(Some(source), key))
        .and_then(|value| (value >= 0.0).then_some(value as u64));
    let aes_key = ["aes_key", "aesKey", "aeskey"]
        .iter()
        .map(|key| read_string(Some(source), key))
        .find(|value| !value.is_empty());
    let _ = lookup;
    Some(WeixinInboundAttachment {
        id: provider_file_id
            .clone()
            .or_else(|| download_url.clone())
            .unwrap_or_else(|| format!("weixin-{}", index + 1)),
        kind,
        filename,
        mime_type,
        size_bytes,
        provider_file_id,
        download_url,
        data_base64,
        aes_key,
    })
}

fn attachments(raw: &Value, inner: Option<&Value>) -> Vec<WeixinInboundAttachment> {
    let mut result = Vec::new();
    for (index, item) in item_list(raw, inner).iter().enumerate() {
        if let Some(attachment) = attachment_item(item, index) {
            result.push(attachment);
        }
    }
    for source in [Some(raw), inner] {
        if let Some(items) = source
            .and_then(|value| value.get("attachments"))
            .and_then(Value::as_array)
        {
            for (index, item) in items.iter().enumerate() {
                if let Some(attachment) = direct_attachment(item, index) {
                    result.push(attachment);
                }
            }
        }
    }
    result
}

fn direct_attachment(item: &Value, index: usize) -> Option<WeixinInboundAttachment> {
    if !is_record(item) {
        return None;
    }
    let kind = read_string(Some(item), "kind");
    if !matches!(kind.as_str(), "image" | "audio" | "video" | "file") {
        return None;
    }
    let id = non_empty(read_string(Some(item), "id")).unwrap_or_else(|| format!("weixin-{}", index + 1));
    let filename = non_empty(read_string(Some(item), "filename")).unwrap_or_else(|| format!("{id}.{kind}"));
    let mime_type = non_empty(read_string(Some(item), "mimeType"))
        .or_else(|| non_empty(read_string(Some(item), "mime_type")))
        .unwrap_or_else(|| infer_mime(&filename, &kind));
    let size_bytes = read_number(Some(item), "sizeBytes")
        .or_else(|| read_number(Some(item), "size"))
        .and_then(|value| (value >= 0.0).then_some(value as u64));
    Some(WeixinInboundAttachment {
        id,
        kind,
        filename,
        mime_type,
        size_bytes,
        provider_file_id: non_empty(read_string(Some(item), "providerFileId"))
            .or_else(|| non_empty(read_string(Some(item), "file_id"))),
        download_url: non_empty(read_string(Some(item), "downloadUrl"))
            .or_else(|| non_empty(read_string(Some(item), "download_url"))),
        data_base64: non_empty(read_string(Some(item), "dataBase64"))
            .or_else(|| non_empty(read_string(Some(item), "data_base64"))),
        aes_key: None,
    })
}

pub struct WeixinUpdatesPage {
    pub messages: Vec<WeixinInboundMessage>,
    /// 下一页同步游标（无新值时保持调用方传入的 buf）。
    pub buf: Option<String>,
    /// 原始消息条数（诊断）。
    pub raw_message_count: usize,
}

/// 把 `/getupdates` 的 payload 解析为入站消息 + 下一游标。
pub fn parse_updates_payload(bot_id: &str, payload: &Value) -> WeixinUpdatesPage {
    let container = payload.get("data").filter(|value| is_record(value)).unwrap_or(payload);
    let raw_messages: Vec<&Value> = {
        let candidate = ["msgs", "messages", "updates", "items", "list"]
            .iter()
            .find_map(|key| container.get(*key));
        match candidate {
            Some(Value::Array(items)) => items.iter().filter(|item| is_record(item)).collect(),
            Some(value) if is_record(value) => vec![value],
            _ => Vec::new(),
        }
    };
    let messages: Vec<WeixinInboundMessage> = raw_messages
        .iter()
        .filter_map(|raw| build_inbound_message(bot_id, raw))
        .collect();
    let buf = ["get_updates_buf", "buf", "next_buf", "nextBuf", "getUpdatesBuf", "syncKey"]
        .iter()
        .map(|key| read_string(Some(container), key))
        .find(|value| !value.is_empty());
    WeixinUpdatesPage {
        raw_message_count: raw_messages.len(),
        messages,
        buf,
    }
}

fn build_inbound_message(bot_id: &str, raw: &Value) -> Option<WeixinInboundMessage> {
    // 机器人自己发出的消息回流：message_type=2。
    if read_number(Some(raw), "message_type") == Some(2.0) {
        return None;
    }
    let inner = inner_message(raw);
    let text = message_text(raw, inner).trim().to_string();
    let attachments = attachments(raw, inner);
    let provider_user_id = {
        let from = raw.get("from").filter(|value| is_record(value));
        let sender = raw.get("sender").filter(|value| is_record(value));
        let candidates = [
            read_string(Some(raw), "from_user_id"),
            read_string(Some(raw), "from"),
            read_string(Some(raw), "from_user"),
            read_string(Some(raw), "fromUser"),
            read_string(Some(raw), "user"),
            read_string(Some(raw), "user_id"),
            read_string(Some(raw), "userId"),
            read_string(from, "id"),
            read_string(from, "wxid"),
            read_string(sender, "id"),
            read_string(sender, "wxid"),
        ];
        candidates
            .into_iter()
            .find(|value| !value.trim().is_empty())?
            .trim()
            .to_string()
    };
    if text.is_empty() && attachments.is_empty() {
        return None;
    }
    let chat_id = ["room", "room_id", "roomId", "chat", "chat_id", "chatId"]
        .iter()
        .map(|key| read_string(Some(raw), key))
        .find(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string());
    let display_name = {
        let from = raw.get("from").filter(|value| is_record(value));
        let sender = raw.get("sender").filter(|value| is_record(value));
        [
            read_string(Some(raw), "name"),
            read_string(Some(raw), "displayName"),
            read_string(Some(raw), "nickname"),
            read_string(from, "name"),
            read_string(from, "nickname"),
            read_string(sender, "name"),
            read_string(sender, "nickname"),
            read_string(inner, "sender_name"),
        ]
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
    };
    let provider_message_id = {
        let candidates = [
            read_string(Some(raw), "id"),
            read_string(Some(raw), "msgid"),
            read_string(Some(raw), "msgId"),
            read_number_or_string(Some(raw), "message_id"),
            read_string(inner, "id"),
            read_string(inner, "msgid"),
            read_string(inner, "msgId"),
            read_number_or_string(inner, "message_id"),
        ];
        candidates
            .into_iter()
            .find(|value| !value.is_empty())
            .or_else(|| {
                read_number(Some(raw), "id")
                    .or_else(|| read_number(Some(raw), "msgid"))
                    .or_else(|| read_number(inner, "id"))
                    .or_else(|| read_number(inner, "msgid"))
                    .map(|value| value.to_string())
            })
    };
    let provider_context_token = [
        read_string(Some(raw), "context_token"),
        read_string(Some(raw), "contextToken"),
        read_string(Some(raw), "context"),
        read_string(inner, "context_token"),
    ]
    .into_iter()
    .find(|value| !value.is_empty());
    Some(WeixinInboundMessage {
        bot_id: bot_id.to_string(),
        text,
        attachments,
        actor: WeixinInboundActor {
            provider_user_id,
            display_name,
            chat_type: if chat_id.is_some() { "group" } else { "private" }.to_string(),
            chat_id,
            provider_message_id,
            provider_context_token,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_private_text_with_context_and_buf() {
        let payload = json!({
            "ret": 0,
            "data": {
                "messages": [{
                    "message_type": 1,
                    "from_user_id": "user-1",
                    "name": "小明",
                    "message_id": 42,
                    "context_token": "ctx-1",
                    "item_list": [{ "type": 1, "text_item": { "text": "  你好  " } }]
                }],
                "get_updates_buf": "buf-2"
            }
        });
        let page = parse_updates_payload("bot-1", &payload);
        assert_eq!(page.raw_message_count, 1);
        assert_eq!(page.buf.as_deref(), Some("buf-2"));
        let message = &page.messages[0];
        assert_eq!(message.text, "你好");
        assert_eq!(message.actor.provider_user_id, "user-1");
        assert_eq!(message.actor.display_name.as_deref(), Some("小明"));
        assert_eq!(message.actor.chat_type, "private");
        assert_eq!(message.actor.provider_message_id.as_deref(), Some("42"));
        assert_eq!(message.actor.provider_context_token.as_deref(), Some("ctx-1"));
    }

    #[test]
    fn filters_self_messages_and_parses_group_image_attachment() {
        let payload = json!({
            "messages": [
                { "message_type": 2, "from_user_id": "self", "text": "echo" },
                {
                    "from_user_id": "user-2",
                    "room_id": "room-1",
                    "item_list": [{
                        "type": 3,
                        "image_item": {
                            "media": {
                                "full_url": "https://cdn.example/img.jpg",
                                "mid_size": 1024,
                                "aes_key": "0123456789abcdef0123456789abcdef"
                            }
                        }
                    }]
                }
            ]
        });
        let page = parse_updates_payload("bot-1", &payload);
        assert_eq!(page.raw_message_count, 2);
        assert_eq!(page.messages.len(), 1);
        let message = &page.messages[0];
        assert_eq!(message.actor.chat_type, "group");
        assert_eq!(message.actor.chat_id.as_deref(), Some("room-1"));
        let attachment = &message.attachments[0];
        assert_eq!(attachment.kind, "image");
        assert_eq!(attachment.download_url.as_deref(), Some("https://cdn.example/img.jpg"));
        assert_eq!(attachment.size_bytes, Some(1024));
        assert_eq!(attachment.filename, "weixin-image-1.jpg");
        assert_eq!(
            attachment.aes_key.as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
    }
}
