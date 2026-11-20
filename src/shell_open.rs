//! 用系统默认浏览器打开 URL，并探测常见浏览器安装情况。
//!
//! 不用 `cmd /C start`：URL 里可能带 `&`/引号，拼 shell 命令会有注入面。
//! Windows 走 `ShellExecuteW("open")`，其它平台退 `xdg-open`/`open`。

use std::path::PathBuf;

/// 只放行 http/https，且拒绝空白/引号/控制字符。
pub fn validate_url(value: &str) -> Result<&str, String> {
    let url = value.trim();
    if url.is_empty() {
        return Err("URL 不能为空".to_string());
    }
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("只支持 http/https URL".to_string());
    }
    if url.chars().any(|ch| {
        ch.is_whitespace() || ch.is_control() || matches!(ch, '"' | '\'' | '<' | '>')
    }) {
        return Err("URL 含非法字符".to_string());
    }
    Ok(url)
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn open(url: &str) -> Result<(), String> {
        let operation = wide("open");
        let file = wide(url);
        // SAFETY: 两个指针指向本栈上以 NUL 结尾的宽字符串；其余可空参数按文档传 null。
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as usize <= 32 {
            return Err(format!("系统未找到可打开该 URL 的程序（code={result}）"));
        }
        Ok(())
    }
}

/// 用系统默认程序打开本地文件/目录（设置页「打开配置文件」）。
pub fn open_path(value: &str) -> Result<(), String> {
    let path = std::path::Path::new(value.trim());
    if !path.exists() {
        return Err(format!("路径不存在：{}", path.display()));
    }
    #[cfg(windows)]
    {
        win::open(&path.to_string_lossy())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("调用 open 失败：{error}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("调用 xdg-open 失败：{error}"))
    }
}

pub fn open_url(value: &str) -> Result<(), String> {
    let url = validate_url(value)?;
    #[cfg(windows)]
    {
        win::open(url)
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("调用 open 失败：{error}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("调用 xdg-open 失败：{error}"))
    }
}

fn exists(path: PathBuf) -> bool {
    path.is_file()
}

/// 常见浏览器探测（只读文件系统，不启进程）。
pub fn installed_browsers() -> Vec<(&'static str, bool)> {
    let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| {
        r"C:\Program Files".to_string()
    });
    let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| {
        r"C:\Program Files (x86)".to_string()
    });
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let pf = PathBuf::from(&program_files);
    let pf86 = PathBuf::from(&program_files_x86);
    let local_app = PathBuf::from(&local);
    vec![
        (
            "Microsoft Edge",
            exists(pf86.join(r"Microsoft\Edge\Application\msedge.exe"))
                || exists(pf.join(r"Microsoft\Edge\Application\msedge.exe")),
        ),
        (
            "Google Chrome",
            exists(pf.join(r"Google\Chrome\Application\chrome.exe"))
                || exists(pf86.join(r"Google\Chrome\Application\chrome.exe"))
                || exists(local_app.join(r"Google\Chrome\Application\chrome.exe")),
        ),
        (
            "Firefox",
            exists(pf.join(r"Mozilla Firefox\firefox.exe"))
                || exists(pf86.join(r"Mozilla Firefox\firefox.exe")),
        ),
        (
            "Brave",
            exists(pf.join(r"BraveSoftware\Brave-Browser\Application\brave.exe"))
                || exists(
                    local_app
                        .join(r"BraveSoftware\Brave-Browser\Application\brave.exe"),
                ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_validation_allows_http_and_rejects_shellish_input() {
        assert!(validate_url("https://example.com/a?b=1&c=2").is_ok());
        assert!(validate_url("  http://127.0.0.1:8080  ").is_ok());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("https://a b.com").is_err());
        assert!(validate_url("https://a.com/\"x").is_err());
        assert!(validate_url("").is_err());
    }

    #[test]
    fn browser_probe_returns_expected_names() {
        let rows = installed_browsers();
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().any(|(name, _)| *name == "Microsoft Edge"));
        assert!(rows.iter().any(|(name, _)| *name == "Google Chrome"));
    }
}
