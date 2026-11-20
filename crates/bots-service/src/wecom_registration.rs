//! 企业微信智能机器人扫码注册（`wecomRegistration.ts` 的 Rust 移植）。
//!
//! 使用企业微信公开的智能机器人 QR 接口：
//! `GET /ai/qc/generate?source=mode&plat=<1|2|3>` 拿 `scode` + `auth_url`；
//! UI 把 `auth_url` 渲染成二维码，然后 `GET /ai/qc/query_result?scode=...` 轮询。
//! 成功时返回 `bot_info.{botid, secret}`（secret 由上层存入凭据库）。
//!
//! 与 TS 的差异只有传输层：请求经注入的 [`HttpJsonPort`] 发出（生产 reqwest，测试脚本化）；
//! 状态映射、scode 字符集与错误分层保持与 `wecomRegistration.ts` 一致。

use std::sync::Arc;
use std::time::Duration;

use crate::http::HttpJsonPort;

pub const WECOM_QR_BASE_URL: &str = "https://work.weixin.qq.com/ai/qc";
pub const WECOM_QR_POLL_INTERVAL_MS: u64 = 2_000;
pub const WECOM_QR_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// 企业微信未返回过期时间，按 10 分钟兜底（与扫码授权体验匹配）。
pub const WECOM_QR_TTL_MS: i64 = 10 * 60 * 1000;
/// source 用于企业微信后台统计来源，固定为本产品标识。
const WECOM_QR_SOURCE: &str = "mode";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WecomRegistrationBegin {
    pub scode: String,
    pub auth_url: String,
    pub interval_ms: u64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WecomRegistrationPoll {
    Pending { interval_ms: u64 },
    Scanned { interval_ms: u64 },
    Success { bot_id: String, secret: String },
    Expired,
    Cancelled,
    Denied,
    Error { message: String },
}

fn read_string(record: Option<&serde_json::Value>, key: &str) -> String {
    record
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn platform_code() -> u8 {
    if cfg!(target_os = "macos") {
        1
    } else if cfg!(target_os = "windows") {
        2
    } else {
        3
    }
}

/// `scode` 只用于 query 参数；保留 URL-safe 字母表（含 `-` / `_`，2026-10-20 真机修复）。
pub fn sanitize_scode(scode: &str) -> String {
    scode
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .collect()
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn unwrap_data(payload: &serde_json::Value) -> serde_json::Value {
    match payload.get("data") {
        Some(serde_json::Value::Object(_)) => payload["data"].clone(),
        _ => payload.clone(),
    }
}

async fn fetch_json(http: Arc<dyn HttpJsonPort>, url: &str) -> Result<serde_json::Value, String> {
    let response = http.get_bytes(url, &[], WECOM_QR_REQUEST_TIMEOUT).await?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "WeCom registration request failed: HTTP {}",
            response.status
        ));
    }
    let text = String::from_utf8_lossy(&response.body).to_string();
    let payload: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("WeCom registration returned invalid JSON: {error}"))?;
    if !payload.is_object() {
        return Err("WeCom registration returned a non-object payload.".to_string());
    }
    let errcode = payload.get("errcode").and_then(serde_json::Value::as_i64);
    if errcode.is_some_and(|code| code != 0) {
        let errmsg = read_string(Some(&payload), "errmsg");
        return Err(format!(
            "WeCom registration API error {}: {}",
            errcode.unwrap_or(0),
            if errmsg.is_empty() {
                "unknown error"
            } else {
                &errmsg
            }
        ));
    }
    Ok(unwrap_data(&payload))
}

/// `beginWecomRegistration`：要 scode 与授权 URL。
pub async fn begin_wecom_registration(
    http: Arc<dyn HttpJsonPort>,
    base_url: &str,
) -> Result<WecomRegistrationBegin, String> {
    let url = format!(
        "{}/generate?source={}&plat={}",
        base_url.trim_end_matches('/'),
        percent_encode(WECOM_QR_SOURCE),
        platform_code()
    );
    let payload = fetch_json(http, &url).await?;
    let scode = read_string(Some(&payload), "scode");
    let auth_url = read_string(Some(&payload), "auth_url");
    if scode.is_empty() || auth_url.is_empty() {
        return Err("WeCom registration did not return scode / auth_url.".to_string());
    }
    Ok(WecomRegistrationBegin {
        scode,
        auth_url,
        interval_ms: WECOM_QR_POLL_INTERVAL_MS,
        expires_at: now_millis() + WECOM_QR_TTL_MS,
    })
}

/// `pollWecomRegistration`：查询扫码状态；成功时带 `bot_info` 的 botid/secret。
pub async fn poll_wecom_registration(
    http: Arc<dyn HttpJsonPort>,
    base_url: &str,
    scode: &str,
) -> Result<WecomRegistrationPoll, String> {
    let safe_scode = sanitize_scode(scode);
    if safe_scode.is_empty() {
        return Ok(WecomRegistrationPoll::Error {
            message: "Invalid scode.".to_string(),
        });
    }
    let url = format!(
        "{}/query_result?scode={}",
        base_url.trim_end_matches('/'),
        percent_encode(&safe_scode)
    );
    let payload = fetch_json(http, &url).await?;
    let status = {
        let value = read_string(Some(&payload), "status");
        if value.is_empty() {
            "waiting".to_string()
        } else {
            value
        }
    };
    Ok(match status.to_ascii_lowercase().as_str() {
        "success" => {
            let bot_info = payload.get("bot_info");
            let bot_id = read_string(bot_info, "botid");
            let secret = read_string(bot_info, "secret");
            if bot_id.is_empty() || secret.is_empty() {
                WecomRegistrationPoll::Error {
                    message: "WeCom scan succeeded but bot_info is incomplete.".to_string(),
                }
            } else {
                WecomRegistrationPoll::Success { bot_id, secret }
            }
        }
        "expired" => WecomRegistrationPoll::Expired,
        "cancelled" => WecomRegistrationPoll::Cancelled,
        "denied" => WecomRegistrationPoll::Denied,
        "scaned" | "scanned" => WecomRegistrationPoll::Scanned {
            interval_ms: WECOM_QR_POLL_INTERVAL_MS,
        },
        _ => WecomRegistrationPoll::Pending {
            interval_ms: WECOM_QR_POLL_INTERVAL_MS,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpBytesResponse, HttpFuture, HttpJsonResponse, OutboundHttpRequest};
    use std::sync::Mutex;

    struct FakeHttp {
        responses: Mutex<Vec<Result<HttpBytesResponse, String>>>,
        urls: Mutex<Vec<String>>,
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
            url: &'a str,
            _headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> HttpFuture<'a, Result<HttpBytesResponse, String>> {
            self.urls.lock().unwrap().push(url.to_string());
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| Err("no scripted response".to_string()));
            Box::pin(async move { response })
        }
    }

    fn ok_json(text: &str) -> Result<HttpBytesResponse, String> {
        Ok(HttpBytesResponse {
            status: 200,
            body: text.as_bytes().to_vec(),
        })
    }

    fn fake(response: Result<HttpBytesResponse, String>) -> Arc<FakeHttp> {
        Arc::new(FakeHttp {
            responses: Mutex::new(vec![response]),
            urls: Mutex::new(Vec::new()),
        })
    }

    #[test]
    fn scode_keeps_url_safe_dash_and_underscore() {
        assert_eq!(sanitize_scode("dF3l53w8JGOL_tOf"), "dF3l53w8JGOL_tOf");
        assert_eq!(sanitize_scode("a-b_c!@ #"), "a-b_c");
        assert_eq!(sanitize_scode("!!!"), "");
    }

    #[tokio::test]
    async fn begin_parses_scode_and_auth_url() {
        let http = fake(ok_json(
            r#"{"errcode":0,"data":{"scode":"s-1","auth_url":"https://qr.example/1"}}"#,
        ));
        let begin = begin_wecom_registration(http.clone(), WECOM_QR_BASE_URL)
            .await
            .expect("begin");
        assert_eq!(begin.scode, "s-1");
        assert_eq!(begin.auth_url, "https://qr.example/1");
        assert_eq!(begin.interval_ms, 2_000);
        assert!(begin.expires_at > now_millis() - 1_000);
        let urls = http.urls.lock().unwrap();
        assert!(
            urls[0].contains("/ai/qc/generate?source=mode&plat="),
            "{urls:?}"
        );
    }

    #[tokio::test]
    async fn poll_maps_success_and_terminal_states() {
        let success = fake(ok_json(
            r#"{"errcode":0,"data":{"status":"success","bot_info":{"botid":"b1","secret":"sec"}}}"#,
        ));
        assert_eq!(
            poll_wecom_registration(success, WECOM_QR_BASE_URL, "s-1")
                .await
                .unwrap(),
            WecomRegistrationPoll::Success {
                bot_id: "b1".to_string(),
                secret: "sec".to_string()
            }
        );

        for (status, expected) in [
            (
                "scaned",
                WecomRegistrationPoll::Scanned { interval_ms: 2_000 },
            ),
            (
                "scanned",
                WecomRegistrationPoll::Scanned { interval_ms: 2_000 },
            ),
            (
                "waiting",
                WecomRegistrationPoll::Pending { interval_ms: 2_000 },
            ),
            ("expired", WecomRegistrationPoll::Expired),
            ("cancelled", WecomRegistrationPoll::Cancelled),
            ("denied", WecomRegistrationPoll::Denied),
        ] {
            let http = fake(ok_json(&format!(r#"{{"errcode":0,"status":"{status}"}}"#)));
            assert_eq!(
                poll_wecom_registration(http, WECOM_QR_BASE_URL, "s-1")
                    .await
                    .unwrap(),
                expected,
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn poll_rejects_invalid_scode_without_request() {
        let http = fake(ok_json("{}"));
        assert_eq!(
            poll_wecom_registration(http.clone(), WECOM_QR_BASE_URL, "!!!")
                .await
                .unwrap(),
            WecomRegistrationPoll::Error {
                message: "Invalid scode.".to_string()
            }
        );
        assert!(http.urls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn incomplete_bot_info_is_an_error_result() {
        let http = fake(ok_json(
            r#"{"errcode":0,"status":"success","bot_info":{"botid":"b1"}}"#,
        ));
        assert_eq!(
            poll_wecom_registration(http, WECOM_QR_BASE_URL, "s-1")
                .await
                .unwrap(),
            WecomRegistrationPoll::Error {
                message: "WeCom scan succeeded but bot_info is incomplete.".to_string()
            }
        );
    }

    #[tokio::test]
    async fn http_and_api_errors_are_layered() {
        let http = fake(Ok(HttpBytesResponse {
            status: 502,
            body: Vec::new(),
        }));
        let error = begin_wecom_registration(http, WECOM_QR_BASE_URL)
            .await
            .expect_err("http error");
        assert!(error.contains("HTTP 502"), "{error}");

        let http = fake(ok_json(r#"{"errcode":40001,"errmsg":"invalid scode"}"#));
        let error = begin_wecom_registration(http, WECOM_QR_BASE_URL)
            .await
            .expect_err("api error");
        assert!(error.contains("40001"), "{error}");
        assert!(error.contains("invalid scode"), "{error}");
    }
}
