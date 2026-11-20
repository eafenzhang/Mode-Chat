//! 企微消息载荷（步骤 2：入站解析 + 回复 body 构造）。
//!
//! 入站（`aibot_msg_callback.body`，SDK `BaseMessage`）：
//! - `text.content`；`mixed.msg_item[]` 中 text 取正文、image 记 `[图片]`；
//! - `voice.content` 是长连接模式下的转写文本；`image/file/video` 只有类型占位；
//! - `quote` 是用户引用内容，只作为上下文，不进入命令解析；
//! - 媒体附件：`url` + `aeskey`（下载后按需解密）。
//!
//! 出站（SDK `replyStream`/`sendMessage`/媒体上传）：
//! - 流式回复 `{msgtype:"stream", stream:{id, finish, content, msg_item?}}`；
//! - 主动消息 `{chatid, msgtype:"markdown", markdown:{content}}`；
//! - 媒体三段：init（type/filename/total_size/total_chunks/md5）→ chunk
//!   （upload_id/chunk_index/base64_data）→ finish（upload_id）。

use serde::Serialize;
use serde_json::{json, Value};

pub const WECOM_TYPING_PLACEHOLDER: &str = "<think></think>";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeComInboundAttachment {
    pub id: String,
    /// image | file | video
    pub kind: String,
    pub filename: String,
    pub mime_type: String,
    pub download_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeComInboundMessage {
    pub bot_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quoted_text: Option<String>,
    pub attachments: Vec<WeComInboundAttachment>,
    pub provider_user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    /// private | group
    pub chat_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

fn read_str(record: Option<&Value>, key: &str) -> String {
    record
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn media_attachment(kind: &str, media: Option<&Value>, filename: &str, mime: &str, index: usize) -> Option<WeComInboundAttachment> {
    let url = read_str(media, "url");
    if url.is_empty() {
        return None;
    }
    let aes_key = read_str(media, "aeskey");
    let tail: String = url.chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
    Some(WeComInboundAttachment {
        id: format!("{kind}-{tail}"),
        kind: kind.to_string(),
        filename: if index == 0 {
            filename.to_string()
        } else {
            format!("{kind}-{}.{}", index + 1, filename.rsplit('.').next().unwrap_or("bin"))
        },
        mime_type: mime.to_string(),
        download_url: url,
        aes_key: if aes_key.is_empty() { None } else { Some(aes_key) },
    })
}

fn message_text(msgtype: &str, body: &Value) -> String {
    match msgtype {
        "text" => read_str(body.get("text"), "content"),
        "mixed" => body
            .get("mixed")
            .and_then(|mixed| mixed.get("msg_item"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        if read_str(Some(item), "msgtype") == "text" {
                            read_str(item.get("text"), "content")
                        } else {
                            "[图片]".to_string()
                        }
                    })
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "voice" => read_str(body.get("voice"), "content"),
        "image" => "[图片]".to_string(),
        "file" => "[文件]".to_string(),
        "video" => "[视频]".to_string(),
        _ => String::new(),
    }
}

fn quoted_text(body: &Value) -> Option<String> {
    let quote = body.get("quote")?;
    let msgtype = read_str(Some(quote), "msgtype");
    let text = match msgtype.as_str() {
        "text" => read_str(quote.get("text"), "content").trim().to_string(),
        "voice" => read_str(quote.get("voice"), "content").trim().to_string(),
        "mixed" => quote
            .get("mixed")
            .and_then(|mixed| mixed.get("msg_item"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        if read_str(Some(item), "msgtype") == "text" {
                            read_str(item.get("text"), "content")
                        } else {
                            "[图片]".to_string()
                        }
                    })
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "image" => "[图片]".to_string(),
        "file" => "[文件]".to_string(),
        _ => String::new(),
    };
    (!text.is_empty()).then_some(text)
}

fn attachments(body: &Value) -> Vec<WeComInboundAttachment> {
    let msgtype = read_str(Some(body), "msgtype");
    let mut result = Vec::new();
    match msgtype.as_str() {
        "image" => {
            if let Some(item) = media_attachment("image", body.get("image"), "wecom-image.jpg", "image/jpeg", 0) {
                result.push(item);
            }
        }
        "file" => {
            if let Some(item) = media_attachment("file", body.get("file"), "wecom-file", "application/octet-stream", 0) {
                result.push(item);
            }
        }
        "video" => {
            if let Some(item) = media_attachment("video", body.get("video"), "wecom-video.mp4", "video/mp4", 0) {
                result.push(item);
            }
        }
        "mixed" => {
            if let Some(items) = body
                .get("mixed")
                .and_then(|mixed| mixed.get("msg_item"))
                .and_then(Value::as_array)
            {
                for (index, item) in items.iter().enumerate() {
                    if read_str(Some(item), "msgtype") == "image" {
                        if let Some(attachment) =
                            media_attachment("image", item.get("image"), "wecom-image.jpg", "image/jpeg", index)
                        {
                            result.push(attachment);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    result
}

/// 解析 `aibot_msg_callback` 的 body。
pub fn parse_inbound_message(bot_id: &str, body: &Value) -> Option<WeComInboundMessage> {
    let msgtype = read_str(Some(body), "msgtype");
    let text_source = message_text(&msgtype, body);
    let attachments = attachments(body);
    if text_source.trim().is_empty() && attachments.is_empty() {
        return None;
    }
    let provider_user_id = {
        let from = body.get("from");
        let value = read_str(from, "userid");
        if value.is_empty() {
            read_str(from, "id")
        } else {
            value
        }
    };
    if provider_user_id.trim().is_empty() {
        return None;
    }
    let chat_id = {
        let value = read_str(Some(body), "chatid");
        (!value.trim().is_empty()).then_some(value.trim().to_string())
    };
    let chat_type = {
        let explicit = read_str(Some(body), "chattype");
        if !explicit.is_empty() {
            explicit
        } else if chat_id.is_some() {
            "group".to_string()
        } else {
            "private".to_string()
        }
    };
    let display_name = {
        let value = read_str(body.get("from"), "name");
        (!value.trim().is_empty()).then_some(value.trim().to_string())
    };
    let message_id = {
        let value = read_str(Some(body), "msgid");
        if value.is_empty() {
            read_str(Some(body), "msg_id")
        } else {
            value
        }
    };
    Some(WeComInboundMessage {
        bot_id: bot_id.to_string(),
        text: text_source,
        quoted_text: quoted_text(body),
        attachments,
        provider_user_id: provider_user_id.trim().to_string(),
        chat_id,
        chat_type,
        message_id: (!message_id.trim().is_empty()).then_some(message_id),
        display_name,
    })
}

/// 会话键：群聊 chatid，单聊 from.userid。
pub fn chat_key(message: &WeComInboundMessage) -> String {
    message
        .chat_id
        .clone()
        .unwrap_or_else(|| message.provider_user_id.clone())
}

/// 流式回复 body（`replyStream`）。
pub fn stream_reply_body(stream_id: &str, content: &str, finish: bool) -> Value {
    json!({
        "msgtype": "stream",
        "stream": { "id": stream_id, "finish": finish, "content": content }
    })
}

/// 主动 markdown 消息 body（`sendMessage`）。
pub fn markdown_send_body(chat_id: &str, content: &str) -> Value {
    json!({
        "chatid": chat_id,
        "msgtype": "markdown",
        "markdown": { "content": content }
    })
}

/// 被动回复媒体 body（`replyMedia`）。
pub fn media_reply_body(media_type: &str, media_id: &str) -> Value {
    json!({
        "msgtype": media_type,
        media_type: { "media_id": media_id }
    })
}

/// 主动推送媒体 body（`sendMediaMessage`）。
pub fn media_send_body(chat_id: &str, media_type: &str, media_id: &str) -> Value {
    json!({
        "chatid": chat_id,
        "msgtype": media_type,
        media_type: { "media_id": media_id }
    })
}

/// 媒体上传 init body。
pub fn media_upload_init_body(
    media_type: &str,
    filename: &str,
    total_size: u64,
    total_chunks: u64,
    md5: &str,
) -> Value {
    json!({
        "type": media_type,
        "filename": filename,
        "total_size": total_size,
        "total_chunks": total_chunks,
        "md5": md5,
    })
}

/// 媒体上传分片 body。
pub fn media_upload_chunk_body(upload_id: &str, chunk_index: u64, base64_data: &str) -> Value {
    json!({
        "upload_id": upload_id,
        "chunk_index": chunk_index,
        "base64_data": base64_data,
    })
}

/// 媒体上传 finish body。
pub fn media_upload_finish_body(upload_id: &str) -> Value {
    json!({ "upload_id": upload_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_with_quote() {
        let body = json!({
            "msgid": "m1",
            "msgtype": "text",
            "from": { "userid": "u1", "name": "小明" },
            "text": { "content": " 你好 " },
            "quote": { "msgtype": "text", "text": { "content": "上一条" } }
        });
        let message = parse_inbound_message("b1", &body).unwrap();
        assert_eq!(message.text, " 你好 ");
        assert_eq!(message.quoted_text.as_deref(), Some("上一条"));
        assert_eq!(message.provider_user_id, "u1");
        assert_eq!(message.chat_type, "private");
        assert_eq!(message.display_name.as_deref(), Some("小明"));
        assert_eq!(chat_key(&message), "u1");
    }

    #[test]
    fn parses_mixed_with_image_attachment_and_group_chat() {
        let body = json!({
            "msgid": "m2",
            "msgtype": "mixed",
            "from": { "userid": "u2" },
            "chatid": "room-1",
            "mixed": { "msg_item": [
                { "msgtype": "text", "text": { "content": "看图" } },
                { "msgtype": "image", "image": { "url": "https://cdn.example/p.jpg", "aeskey": "k" } }
            ]}
        });
        let message = parse_inbound_message("b1", &body).unwrap();
        assert_eq!(message.text, "看图\n[图片]");
        assert_eq!(message.chat_type, "group");
        assert_eq!(chat_key(&message), "room-1");
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(message.attachments[0].kind, "image");
        assert_eq!(message.attachments[0].aes_key.as_deref(), Some("k"));
    }

    #[test]
    fn empty_payload_is_ignored() {
        assert!(parse_inbound_message("b1", &json!({ "msgtype": "event" })).is_none());
    }

    #[test]
    fn reply_bodies_match_sdk_shapes() {
        assert_eq!(
            stream_reply_body("s1", "内容", false),
            json!({ "msgtype": "stream", "stream": { "id": "s1", "finish": false, "content": "内容" } })
        );
        assert_eq!(
            markdown_send_body("room-1", "**hi**"),
            json!({ "chatid": "room-1", "msgtype": "markdown", "markdown": { "content": "**hi**" } })
        );
        assert_eq!(
            media_upload_init_body("image", "a.jpg", 1024, 1, "md5"),
            json!({ "type": "image", "filename": "a.jpg", "total_size": 1024, "total_chunks": 1, "md5": "md5" })
        );
        assert_eq!(
            media_upload_chunk_body("up-1", 0, "AA=="),
            json!({ "upload_id": "up-1", "chunk_index": 0, "base64_data": "AA==" })
        );
        assert_eq!(media_upload_finish_body("up-1"), json!({ "upload_id": "up-1" }));
        assert_eq!(
            media_reply_body("file", "mid-1"),
            json!({ "msgtype": "file", "file": { "media_id": "mid-1" } })
        );
        assert_eq!(
            media_send_body("room-1", "image", "mid-2"),
            json!({ "chatid": "room-1", "msgtype": "image", "image": { "media_id": "mid-2" } })
        );
    }
}
