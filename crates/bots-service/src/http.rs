//! HTTP JSON 端口（bot 通道运行时共用）：请求形状 + 注入式传输。
//!
//! 运行时（weixin/wecom）只构造请求与分类结果；真实 GET/POST 由宿主注入（bot-tauri 用
//! reqwest + rustls），测试注入脚本化响应——与 `provider-balance`/`coding-plan-http` 同模式。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub type HttpFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpJsonResponse {
    pub status: u16,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundHttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpBytesResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait HttpJsonPort: Send + Sync {
    fn post_json<'a>(
        &'a self,
        request: &'a OutboundHttpRequest,
    ) -> HttpFuture<'a, Result<HttpJsonResponse, String>>;

    /// 二进制 GET（媒体下载）。默认实现报错：只有需要媒体的宿主必须实现。
    fn get_bytes<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(String, String)],
        timeout: Duration,
    ) -> HttpFuture<'a, Result<HttpBytesResponse, String>> {
        let _ = (url, headers, timeout);
        Box::pin(async move { Err("binary GET is not supported by this HTTP port".to_string()) })
    }
}

pub const DEFAULT_PROVIDER_TIMEOUT: Duration = Duration::from_secs(15);
