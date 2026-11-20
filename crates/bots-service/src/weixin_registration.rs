//! 微信 iLink 扫码注册（`weixinRegistration.ts` 的完整移植）。
//!
//! 流程：`GET /ilink/bot/get_bot_qrcode?bot_type=3` 拿二维码 → UI 展示 →
//! `GET /ilink/bot/get_qrcode_status?qrcode=...` 轮询（3s 间隔，120s 过期）→
//! 成功返回 `bot_token`（可带 `ilink_bot_id`）。
//!
//! 轮询超时**不**代表失败：状态接口会长时间挂起等手机确认，超时按 pending 继续。
//! 请求带 `iLink-App-ClientVersion: 1` 头；`ret/errcode != 0` 显式报错。

use std::sync::Arc;
use std::time::Duration;

use crate::http::HttpJsonPort;

pub const WEIXIN_LOGIN_INTERVAL: Duration = Duration::from_secs(3);
pub const WEIXIN_LOGIN_EXPIRE: Duration = Duration::from_secs(120);
pub const WEIXIN_LOGIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const WEIXIN_BOT_API_PREFIX: &str = "/ilink/bot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationBegin {
    pub qr_code: String,
    pub qr_url: String,
    pub interval_ms: u64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationPoll {
    Pending { interval_ms: u64 },
    Scanned { interval_ms: u64 },
    Success { bot_token: String, bot_id: Option<String> },
    Expired,
    Error { message: String },
}

fn read_string(record: Option<&serde_json::Value>, key: &str) -> String {
    record
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn read_number(record: Option<&serde_json::Value>, key: &str) -> Option<f64> {
    record
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_f64)
}

/// `{[data]:{...}}` 摊平为顶层对象（外层优先，内层补齐）。
fn unwrap_data(payload: &serde_json::Value) -> serde_json::Value {
    let Some(object) = payload.as_object() else {
        return serde_json::json!({});
    };
    let Some(data) = payload.get("data").and_then(serde_json::Value::as_object) else {
        return payload.clone();
    };
    let mut merged = serde_json::Map::new();
    for (key, value) in object {
        if key != "data" {
            merged.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in data {
        merged.entry(key.clone()).or_insert_with(|| value.clone());
    }
    serde_json::Value::Object(merged)
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

/// `normalizeQrStatus`：数字与字符串两套状态码统一成五态。
pub fn normalize_qr_status(status: Option<&serde_json::Value>) -> &'static str {
    match status {
        Some(serde_json::Value::Number(number)) => match number.as_i64() {
            Some(0) => "pending",
            Some(1) => "scanned",
            Some(2) => "success",
            Some(3 | 4) => "expired",
            _ => "pending",
        },
        Some(serde_json::Value::String(text)) => match text.trim().to_lowercase().as_str() {
            "confirmed" | "confirm" | "authorized" | "success" | "ok" => "success",
            "scaned" | "scanned" | "scan" | "confirmed_wait" => "scanned",
            "expired" | "timeout" | "cancel" | "cancelled" | "canceled" => "expired",
            "error" | "failed" | "fail" => "error",
            _ => "pending",
        },
        _ => "pending",
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

async fn get_registration_json(
    http: Arc<dyn HttpJsonPort>,
    base_url: &str,
    path: &str,
) -> Result<serde_json::Value, String> {
    let url = format!("{}{}{}", base_url.trim_end_matches('/'), WEIXIN_BOT_API_PREFIX, path);
    let response = http
        .get_bytes(
            &url,
            &[("iLink-App-ClientVersion".to_string(), "1".to_string())],
            WEIXIN_LOGIN_REQUEST_TIMEOUT,
        )
        .await?;
    if !(200..300).contains(&response.status) {
        return Err(format!("Weixin login {path} failed: HTTP {}", response.status));
    }
    let text = String::from_utf8_lossy(&response.body).to_string();
    let payload: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("Weixin login 响应解析失败: {error}"))?;
    let payload = unwrap_data(&payload);
    let ret = read_number(Some(&payload), "ret");
    let errcode = read_number(Some(&payload), "errcode");
    if ret.is_some_and(|value| value != 0.0) || errcode.is_some_and(|value| value != 0.0) {
        let errmsg = read_string(Some(&payload), "errmsg");
        return Err(if errmsg.is_empty() {
            format!("ret={} errcode={}", ret.unwrap_or(0.0), errcode.unwrap_or(0.0))
        } else {
            errmsg
        });
    }
    Ok(payload)
}

/// `beginWeixinRegistration`：要二维码。
pub async fn begin_weixin_registration(
    http: Arc<dyn HttpJsonPort>,
    base_url: &str,
) -> Result<RegistrationBegin, String> {
    let payload = get_registration_json(http, base_url, "/get_bot_qrcode?bot_type=3").await?;
    let qr_code = {
        let value = read_string(Some(&payload), "qrcode");
        if value.is_empty() {
            read_string(Some(&payload), "qr_code")
        } else {
            value
        }
    };
    let qr_url = {
        let content = read_string(Some(&payload), "qrcode_img_content");
        if !content.is_empty() {
            content
        } else {
            let url = read_string(Some(&payload), "qrcode_url");
            if url.is_empty() { qr_code.clone() } else { url }
        }
    };
    if qr_code.is_empty() || qr_url.is_empty() {
        return Err("Weixin login did not return a QR code.".to_string());
    }
    let expire_seconds = read_number(Some(&payload), "expires_in")
        .filter(|value| *value > 0.0)
        .unwrap_or(WEIXIN_LOGIN_EXPIRE.as_secs() as f64);
    Ok(RegistrationBegin {
        qr_code,
        qr_url,
        interval_ms: WEIXIN_LOGIN_INTERVAL.as_millis() as u64,
        expires_at: now_millis() + (expire_seconds * 1000.0) as i64,
    })
}

/// `pollWeixinRegistration`：轮询扫码状态。超时按 pending（服务端会挂起等确认）。
pub async fn poll_weixin_registration(
    http: Arc<dyn HttpJsonPort>,
    base_url: &str,
    qr_code: &str,
) -> Result<RegistrationPoll, String> {
    let path = format!("/get_qrcode_status?qrcode={}", percent_encode(qr_code));
    let payload = match get_registration_json(http, base_url, &path).await {
        Ok(payload) => payload,
        Err(error) => {
            // 超时/网络抖动不升级成失败：交给 UI 串行继续轮询。
            if error.contains("timed out") || error.contains("timeout") {
                return Ok(RegistrationPoll::Pending {
                    interval_ms: WEIXIN_LOGIN_INTERVAL.as_millis() as u64,
                });
            }
            return Err(error);
        }
    };
    let status_value = payload
        .get("status")
        .or_else(|| payload.get("qrcode_status"))
        .or_else(|| payload.get("qr_status"));
    match normalize_qr_status(status_value) {
        "success" => {
            let bot_token = {
                let token = read_string(Some(&payload), "bot_token");
                if token.is_empty() {
                    read_string(Some(&payload), "token")
                } else {
                    token
                }
            };
            if bot_token.is_empty() {
                return Ok(RegistrationPoll::Error {
                    message: "Weixin login succeeded but did not return bot_token.".to_string(),
                });
            }
            let bot_id = {
                let id = read_string(Some(&payload), "ilink_bot_id");
                if id.is_empty() {
                    read_string(Some(&payload), "bot_id")
                } else {
                    id
                }
            };
            Ok(RegistrationPoll::Success {
                bot_token,
                bot_id: (!bot_id.is_empty()).then_some(bot_id),
            })
        }
        "expired" => Ok(RegistrationPoll::Expired),
        "error" => Ok(RegistrationPoll::Error {
            message: {
                let message = read_string(Some(&payload), "errmsg");
                if message.is_empty() {
                    "Weixin login failed.".to_string()
                } else {
                    message
                }
            },
        }),
        "scanned" => Ok(RegistrationPoll::Scanned {
            interval_ms: WEIXIN_LOGIN_INTERVAL.as_millis() as u64,
        }),
        _ => Ok(RegistrationPoll::Pending {
            interval_ms: WEIXIN_LOGIN_INTERVAL.as_millis() as u64,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpBytesResponse, HttpFuture, HttpJsonResponse, OutboundHttpRequest};
    use std::sync::Mutex;

    struct FakeHttp {
        responses: Mutex<Vec<Result<HttpBytesResponse, String>>>,
        urls: Mutex<Vec<String>>,
        headers: Mutex<Vec<Vec<(String, String)>>>,
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
            headers: &'a [(String, String)],
            _timeout: Duration,
        ) -> HttpFuture<'a, Result<HttpBytesResponse, String>> {
            self.urls.lock().unwrap().push(url.to_string());
            self.headers.lock().unwrap().push(headers.to_vec());
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

    #[tokio::test]
    async fn begin_parses_qr_and_defaults_expiry() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![ok_json(
                r#"{"ret":0,"data":{"qrcode":"qr-1","qrcode_img_content":"https://qr.example/1"}}"#,
            )]),
            urls: Mutex::new(Vec::new()),
            headers: Mutex::new(Vec::new()),
        });
        let begin = begin_weixin_registration(http.clone(), "https://ilink.example").await.unwrap();
        assert_eq!(begin.qr_code, "qr-1");
        assert_eq!(begin.qr_url, "https://qr.example/1");
        assert_eq!(begin.interval_ms, 3000);
        let urls = http.urls.lock().unwrap();
        assert!(urls[0].ends_with("/ilink/bot/get_bot_qrcode?bot_type=3"), "{urls:?}");
        let headers = http.headers.lock().unwrap();
        assert!(headers[0]
            .iter()
            .any(|(key, value)| key == "iLink-App-ClientVersion" && value == "1"));
    }

    #[tokio::test]
    async fn poll_maps_statuses_and_returns_token() {
        // 成功：number 2 + bot_token。
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![ok_json(
                r#"{"ret":0,"status":2,"bot_token":"token-1","ilink_bot_id":"bot-9"}"#,
            )]),
            urls: Mutex::new(Vec::new()),
            headers: Mutex::new(Vec::new()),
        });
        assert_eq!(
            poll_weixin_registration(http, "https://ilink.example", "qr 1").await.unwrap(),
            RegistrationPoll::Success {
                bot_token: "token-1".to_string(),
                bot_id: Some("bot-9".to_string())
            }
        );
        // 字符串 scanned / expired。
        assert_eq!(normalize_qr_status(Some(&serde_json::json!("scanned"))), "scanned");
        assert_eq!(normalize_qr_status(Some(&serde_json::json!(3))), "expired");
        assert_eq!(normalize_qr_status(Some(&serde_json::json!("confirmed"))), "success");
    }

    #[tokio::test]
    async fn poll_timeout_stays_pending() {
        let http = Arc::new(FakeHttp {
            responses: Mutex::new(vec![Err("request timed out".to_string())]),
            urls: Mutex::new(Vec::new()),
            headers: Mutex::new(Vec::new()),
        });
        assert_eq!(
            poll_weixin_registration(http, "https://ilink.example", "qr").await.unwrap(),
            RegistrationPoll::Pending { interval_ms: 3000 }
        );
    }
}
