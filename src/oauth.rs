//! 连接器 OAuth 授权（K04）：本地回调 + PKCE 换 token，token 落 secrets.json。
//!
//! 设计取舍：不做内置 client_id——各平台 OAuth 应用由用户自建（GitHub OAuth App /
//! Google Cloud OAuth 客户端 / Notion integration），回填 client_id 与 secret 后
//! 点「登录」：系统浏览器打开授权页 → 本地 `127.0.0.1:17890/callback` 收 code →
//! 换 access_token → 写入 `secrets.json` 的 `oauth:<connector>`，连接器按既有
//! `Authorization: Bearer <secret>` 机制直接可用（见 connectors-service 的 RemoteAuth）。

use base64::Engine;
use sha2::{Digest, Sha256};

/// 本地回调端口（各平台 OAuth 应用的回调地址统一填
/// `http://127.0.0.1:17890/callback`）。
pub const CALLBACK_PORT: u16 = 17890;
pub const CALLBACK_PATH: &str = "/callback";

pub struct OAuthProvider {
    pub id: &'static str,
    pub name: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub default_scopes: &'static str,
    /// 连接器页里的说明（scope 用途）。
    pub about: &'static str,
}

pub const PROVIDERS: &[OAuthProvider] = &[
    OAuthProvider {
        id: "github",
        name: "GitHub",
        authorize_url: "https://github.com/login/oauth/authorize",
        token_url: "https://github.com/login/oauth/access_token",
        default_scopes: "repo read:user",
        about: "仓库读写与用户信息（需自建 GitHub OAuth App）",
    },
    OAuthProvider {
        id: "google",
        name: "Google",
        authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        default_scopes: "openid email https://www.googleapis.com/auth/drive.readonly",
        about: "Google Drive 只读（需自建 OAuth 客户端，回调填本机地址）",
    },
    OAuthProvider {
        id: "notion",
        name: "Notion",
        authorize_url: "https://api.notion.com/v1/oauth/authorize",
        token_url: "https://api.notion.com/v1/oauth/token",
        default_scopes: "",
        about: "工作区页面读取（Notion integration 的 public 授权）",
    },
];

pub fn provider(id: &str) -> Option<&'static OAuthProvider> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

fn prefs_path() -> std::path::PathBuf {
    crate::instance::state_dir().join("prefs.json")
}

fn read_pref(key: &str) -> Option<String> {
    let bytes = std::fs::read(prefs_path()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// client_id：密钥页（`oauth_client_id:<id>`）优先，回落 prefs 同名键。
pub fn client_id(provider_id: &str) -> Option<String> {
    crate::secrets::get(
        &crate::data::data_root(),
        &format!("oauth_client_id:{provider_id}"),
    )
    .or_else(|| read_pref(&format!("oauth_client_id:{provider_id}")))
    .filter(|value| !value.trim().is_empty())
}

/// 是否已配置客户端（client_id + secret 都在密钥库/偏好里）。
pub fn client_configured(provider_id: &str) -> bool {
    client_id(provider_id).is_some()
        && crate::secrets::get(
            &crate::data::data_root(),
            &format!("oauth_client_secret:{provider_id}"),
        )
        .is_some()
}

/// 是否已拿到 token（`secrets.json` 的 `oauth:<provider>`）。
pub fn authorized(provider_id: &str) -> bool {
    crate::secrets::list(&crate::data::data_root())
        .map(|names| names.contains(&format!("oauth:{provider_id}")))
        .unwrap_or(false)
}

fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 一次性授权会话（PKCE verifier + state）。
pub struct PendingAuth {
    pub provider_id: String,
    pub state: String,
    verifier: String,
}

/// 构造授权 URL（PKCE S256 + state），回调地址固定本机。
pub fn begin(provider_id: &str) -> Result<(PendingAuth, String), String> {
    let provider = provider(provider_id).ok_or_else(|| format!("未知 OAuth 平台：{provider_id}"))?;
    let client_id = client_id(provider_id)
        .ok_or_else(|| "还没有配置 Client ID：在「密钥」添加 oauth_client_id:<平台>".to_string())?;
    let verifier = uuid::Uuid::new_v4().simple().to_string()
        + &uuid::Uuid::new_v4().simple().to_string();
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    let state = uuid::Uuid::new_v4().simple().to_string();
    let redirect_uri = format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}");
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        provider.authorize_url,
        urlencode(&client_id),
        urlencode(&redirect_uri),
        state,
        challenge,
    );
    if !provider.default_scopes.is_empty() {
        url.push_str(&format!("&scope={}", urlencode(provider.default_scopes)));
    }
    // Google 强制 offline access 才能拿 refresh token。
    if provider_id == "google" {
        url.push_str("&access_type=offline&prompt=consent");
    }
    Ok((
        PendingAuth {
            provider_id: provider_id.to_string(),
            state,
            verifier,
        },
        url,
    ))
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// 回调 code 换 token，并写入 `secrets.json`（`oauth:<provider>`）。
pub async fn finish(pending: &PendingAuth, code: &str, returned_state: &str) -> Result<(), String> {
    if returned_state != pending.state {
        return Err("OAuth state 不匹配（可能是过期的授权回调）".to_string());
    }
    let provider = provider(&pending.provider_id)
        .ok_or_else(|| format!("未知 OAuth 平台：{}", pending.provider_id))?;
    let client_id = client_id(&pending.provider_id)
        .ok_or_else(|| "缺少 Client ID".to_string())?;
    let client_secret = crate::secrets::get(
        &crate::data::data_root(),
        &format!("oauth_client_secret:{}", pending.provider_id),
    )
    .ok_or_else(|| "缺少 Client Secret".to_string())?;
    let redirect_uri = format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}");

    let client = reqwest012::Client::new();
    let access_token = match pending.provider_id.as_str() {
        "github" => {
            let body = form_body(&[
                ("client_id", &client_id),
                ("client_secret", &client_secret),
                ("code", code),
                ("redirect_uri", &redirect_uri),
            ]);
            let response = client
                .post(provider.token_url)
                .header("Accept", "application/json")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body)
                .send()
                .await
                .map_err(|error| format!("GitHub token 交换失败：{error}"))?;
            let payload: serde_json::Value = serde_json::from_str(
                &response
                    .text()
                    .await
                    .map_err(|error| format!("GitHub token 响应读取失败：{error}"))?,
            )
            .map_err(|error| format!("GitHub token 响应解析失败：{error}"))?;
            payload
                .get("access_token")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }
        "google" => {
            let body = form_body(&[
                ("code", code),
                ("client_id", &client_id),
                ("client_secret", &client_secret),
                ("redirect_uri", &redirect_uri),
                ("grant_type", "authorization_code"),
                ("code_verifier", pending.verifier.as_str()),
            ]);
            let response = client
                .post(provider.token_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body)
                .send()
                .await
                .map_err(|error| format!("Google token 交换失败：{error}"))?;
            let payload: serde_json::Value = serde_json::from_str(
                &response
                    .text()
                    .await
                    .map_err(|error| format!("Google token 响应读取失败：{error}"))?,
            )
            .map_err(|error| format!("Google token 响应解析失败：{error}"))?;
            payload
                .get("access_token")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }
        "notion" => {
            let basic = base64::engine::general_purpose::STANDARD
                .encode(format!("{client_id}:{client_secret}"));
            let response = client
                .post(provider.token_url)
                .header("Authorization", format!("Basic {basic}"))
                .header("Content-Type", "application/json")
                .body(
                    serde_json::json!({
                        "grant_type": "authorization_code",
                        "code": code,
                        "redirect_uri": redirect_uri,
                    })
                    .to_string(),
                )
                .send()
                .await
                .map_err(|error| format!("Notion token 交换失败：{error}"))?;
            let payload: serde_json::Value = serde_json::from_str(
                &response
                    .text()
                    .await
                    .map_err(|error| format!("Notion token 响应读取失败：{error}"))?,
            )
            .map_err(|error| format!("Notion token 响应解析失败：{error}"))?;
            payload
                .get("access_token")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }
        other => return Err(format!("未实现的 OAuth 平台：{other}")),
    };
    let access_token = access_token.ok_or_else(|| "token 响应里没有 access_token".to_string())?;
    crate::secrets::set(
        &crate::data::data_root(),
        &format!("oauth:{}", pending.provider_id),
        &access_token,
    )?;
    Ok(())
}

/// 本地回调监听：等一次 `GET /callback?code=..&state=..`，回一段 HTML 后返回。
/// `timeout` 内没收到回调即失败（用户在浏览器里点了取消）。
pub async fn wait_callback(
    timeout: std::time::Duration,
) -> Result<(String, String), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .await
        .map_err(|error| format!("本地回调端口 {CALLBACK_PORT} 被占用：{error}"))?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let accept = tokio::time::timeout_at(deadline, listener.accept()).await;
        let Ok(Ok((mut stream, _))) = accept else {
            return Err("等待授权回调超时（可重试）".to_string());
        };
        let mut buffer = vec![0u8; 8192];
        let read = stream.read(&mut buffer).await.map_err(|error| error.to_string())?;
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let first_line = request.lines().next().unwrap_or_default().to_string();
        // 只处理 /callback?...
        let Some(target) = first_line.split_whitespace().nth(1) else {
            continue;
        };
        if !target.starts_with(CALLBACK_PATH) {
            // 浏览器偶尔会先请求 /favicon.ico，继续等真正的回调。
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await;
            continue;
        }
        let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
        let mut code = String::new();
        let mut state = String::new();
        let mut error = String::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = urldecode(value);
            match key {
                "code" => code = value,
                "state" => state = value,
                "error" => error = value,
                _ => {}
            }
        }
        let body = if error.is_empty() && !code.is_empty() {
            "<html><body style=\"font-family:system-ui;padding:40px;background:#111;color:#eee\"><h2>授权完成</h2><p>可以关闭这个页面，回到 Mode Chat 继续。</p></body></html>"
        } else {
            "<html><body style=\"font-family:system-ui;padding:40px;background:#111;color:#eee\"><h2>授权未完成</h2><p>请回到 Mode Chat 重试。</p></body></html>"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        if !error.is_empty() {
            return Err(format!("授权被拒绝：{error}"));
        }
        if code.is_empty() {
            return Err("回调里没有 code".to_string());
        }
        return Ok((code, state));
    }
}

fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        if bytes[index] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 撤销授权：删 token（保留 client 配置）。
pub fn revoke(provider_id: &str) -> Result<(), String> {
    crate::secrets::remove(
        &crate::data::data_root(),
        &format!("oauth:{provider_id}"),
    )?;
    Ok(())
}
