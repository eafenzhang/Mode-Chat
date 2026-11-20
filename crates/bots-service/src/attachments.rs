//! 入站附件物化（下载 → 解密 → 落盘 → 交给宿主）。
//!
//! 语义与 TS 一致：逐条 best-effort（单条失败记入 errors，不中断其它附件），文件名
//! 去路径分隔符与控制字符、同名自动加序号；总预算 20MiB（超出跳过并记录），落盘目录
//! 由宿主给出（通常是 workspace 的临时附件目录）。

use std::path::{Path, PathBuf};

use crate::http::HttpJsonPort;
use crate::weixin_media::download_and_decrypt;

pub const ATTACHMENT_BUDGET_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAttachment {
    pub id: String,
    /// image | file | video | audio
    pub kind: String,
    pub filename: String,
    pub mime_type: String,
    pub download_url: String,
    pub aes_key: Option<String>,
}

impl From<&crate::weixin_inbound::WeixinInboundAttachment> for Option<InboundAttachment> {
    fn from(attachment: &crate::weixin_inbound::WeixinInboundAttachment) -> Self {
        let url = attachment.download_url.clone()?;
        Some(InboundAttachment {
            id: attachment.id.clone(),
            kind: attachment.kind.clone(),
            filename: attachment.filename.clone(),
            mime_type: attachment.mime_type.clone(),
            download_url: url,
            aes_key: attachment.aes_key.clone(),
        })
    }
}

impl From<&crate::wecom_payload::WeComInboundAttachment> for InboundAttachment {
    fn from(attachment: &crate::wecom_payload::WeComInboundAttachment) -> Self {
        Self {
            id: attachment.id.clone(),
            kind: attachment.kind.clone(),
            filename: attachment.filename.clone(),
            mime_type: attachment.mime_type.clone(),
            download_url: attachment.download_url.clone(),
            aes_key: attachment.aes_key.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedAttachment {
    pub id: String,
    pub kind: String,
    pub path: PathBuf,
    pub filename: String,
    pub mime_type: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterializationResult {
    pub files: Vec<MaterializedAttachment>,
    pub errors: Vec<String>,
}

/// 文件名净化：只保留最后一段、去掉控制字符与 Windows 非法字符。
pub fn sanitize_attachment_filename(filename: &str) -> String {
    let last = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .trim();
    let cleaned: String = last
        .chars()
        .filter(|ch| !ch.is_control() && !matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "attachment.bin".to_string()
    } else {
        cleaned
    }
}

fn unique_name(directory: &Path, filename: &str, used: &mut Vec<String>) -> String {
    let mut candidate = filename.to_string();
    if !used.iter().any(|name| name == &candidate) && !directory.join(&candidate).exists() {
        used.push(candidate.clone());
        return candidate;
    }
    let (stem, extension) = match filename.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (filename.to_string(), String::new()),
    };
    let mut index = 2usize;
    loop {
        candidate = format!("{stem}-{index}{extension}");
        if !used.iter().any(|name| name == &candidate) && !directory.join(&candidate).exists() {
            used.push(candidate.clone());
            return candidate;
        }
        index += 1;
    }
}

/// 逐条下载并落盘；`aes_key` 存在时走微信 CDN 解密（AES-128-ECB + PKCS7）。
pub async fn materialize_attachments(
    http: std::sync::Arc<dyn HttpJsonPort>,
    attachments: &[InboundAttachment],
    directory: &Path,
    budget_bytes: u64,
) -> MaterializationResult {
    let mut result = MaterializationResult::default();
    if attachments.is_empty() {
        return result;
    }
    if let Err(error) = std::fs::create_dir_all(directory) {
        result.errors.push(format!("创建附件目录失败: {error}"));
        return result;
    }
    let mut used: Vec<String> = Vec::new();
    let mut total: u64 = 0;
    for attachment in attachments {
        match download_and_decrypt(
            std::sync::Arc::clone(&http),
            &attachment.download_url,
            attachment.aes_key.as_deref(),
            std::time::Duration::from_secs(30),
        )
        .await
        {
            Ok(bytes) => {
                let bytes_len = bytes.len() as u64;
                if total + bytes_len > budget_bytes {
                    result.errors.push(format!(
                        "{}: 超出附件总预算 {} 字节，已跳过",
                        attachment.filename, budget_bytes
                    ));
                    continue;
                }
                let filename =
                    unique_name(directory, &sanitize_attachment_filename(&attachment.filename), &mut used);
                let path = directory.join(&filename);
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        total += bytes_len;
                        result.files.push(MaterializedAttachment {
                            id: attachment.id.clone(),
                            kind: attachment.kind.clone(),
                            path,
                            filename,
                            mime_type: attachment.mime_type.clone(),
                            bytes: bytes_len,
                        });
                    }
                    Err(error) => result
                        .errors
                        .push(format!("{}: 落盘失败: {error}", attachment.filename)),
                }
            }
            Err(error) => result
                .errors
                .push(format!("{}: 下载/解密失败: {error}", attachment.filename)),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpBytesResponse, HttpFuture, HttpJsonResponse, OutboundHttpRequest};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct FakeHttp {
        bytes: Mutex<Vec<Result<Vec<u8>, String>>>,
    }

    impl HttpJsonPort for FakeHttp {
        fn post_json<'a>(
            &'a self,
            _request: &'a OutboundHttpRequest,
        ) -> HttpFuture<'a, Result<HttpJsonResponse, String>> {
            Box::pin(async move { Err("not used".to_string()) })
        }

        fn get_bytes<'a>(
            &'a self,
            _url: &'a str,
            _headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> HttpFuture<'a, Result<HttpBytesResponse, String>> {
            let next = self.bytes.lock().unwrap().pop().unwrap_or_else(|| Err("none".to_string()));
            Box::pin(async move {
                next.map(|body| HttpBytesResponse { status: 200, body })
            })
        }
    }

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("attachments-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn attachment(id: &str, filename: &str) -> InboundAttachment {
        InboundAttachment {
            id: id.to_string(),
            kind: "image".to_string(),
            filename: filename.to_string(),
            mime_type: "image/png".to_string(),
            download_url: format!("https://cdn.example/{id}"),
            aes_key: None,
        }
    }

    #[test]
    fn filename_sanitization_strips_paths_and_controls() {
        assert_eq!(
            sanitize_attachment_filename("../../evil.exe"),
            "evil.exe"
        );
        assert_eq!(
            sanitize_attachment_filename(r"C:\temp\a:b*c?.png"),
            "abc.png"
        );
        assert_eq!(sanitize_attachment_filename("..."), "attachment.bin");
        assert_eq!(sanitize_attachment_filename("正常 文件名.png"), "正常 文件名.png");
    }

    #[tokio::test]
    async fn downloads_materializes_and_dedupes_names() {
        let base = sandbox("ok");
        let http = Arc::new(FakeHttp {
            bytes: Mutex::new(vec![Err("unused".into()), Ok(b"one".to_vec()), Ok(b"two".to_vec())]),
        });
        let attachments = vec![attachment("a1", "same.png"), attachment("a2", "same.png")];
        let result = materialize_attachments(http, &attachments, &base, ATTACHMENT_BUDGET_BYTES).await;
        assert_eq!(result.files.len(), 2);
        assert_eq!(result.files[0].filename, "same.png");
        assert_eq!(result.files[1].filename, "same-2.png");
        assert_eq!(result.errors.len(), 0);
        assert_eq!(std::fs::read(&result.files[0].path).unwrap(), b"two");
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn failures_are_collected_and_budget_is_enforced() {
        let base = sandbox("budget");
        let http = Arc::new(FakeHttp {
            bytes: Mutex::new(vec![Err("unused".into()), Ok(vec![0u8; 8]), Err("network".into())]),
        });
        let attachments = vec![attachment("bad", "bad.png"), attachment("big", "big.png")];
        let result = materialize_attachments(http, &attachments, &base, 4).await;
        assert!(result.files.is_empty());
        assert_eq!(result.errors.len(), 2, "{:?}", result.errors);
        assert!(result.errors[0].contains("下载/解密失败"), "{:?}", result.errors);
        assert!(result.errors[1].contains("超出附件总预算"), "{:?}", result.errors);
        std::fs::remove_dir_all(base).ok();
    }
}
