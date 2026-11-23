//! 开机自启（Windows HKCU Run 键）。
//!
//! 不引 winreg：直接调用系统 `reg.exe`，避免为一个键增加依赖。
//! 非 Windows 平台是 no-op（GPUI 试点只发 Windows）。

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ModeBotGPUI";

/// 构造 `reg.exe` 调用：只需要 stdout，GUI 发布包没有控制台，不加标志会弹黑框。
#[cfg(windows)]
fn reg(args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("reg");
    command.args(args);
    crate::process_console::no_console(&mut command);
    command
}

/// 当前是否登记了开机自启（同名键存在即视为开启，不校验路径是否过期）。
pub fn is_enabled() -> bool {
    #[cfg(windows)]
    {
        reg(&["query", RUN_KEY, "/v", VALUE_NAME])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 开/关开机自启；开启时写入当前 exe 的绝对路径（带引号）。
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        if enabled {
            let exe = std::env::current_exe()
                .map_err(|error| format!("定位当前可执行文件失败：{error}"))?;
            let value = format!("\"{}\"", exe.display());
            let output = reg(&[
                "add",
                RUN_KEY,
                "/v",
                VALUE_NAME,
                "/t",
                "REG_SZ",
                "/d",
                value.as_str(),
                "/f",
            ])
            .output()
            .map_err(|error| format!("调用 reg.exe 失败：{error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "写入开机自启失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
        } else {
            let output = reg(&["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
                .output()
                .map_err(|error| format!("调用 reg.exe 失败：{error}"))?;
            // 键不存在也算关闭成功。
            if !output.status.success()
                && !String::from_utf8_lossy(&output.stderr).contains("unable to find")
                && !String::from_utf8_lossy(&output.stderr).contains("无法找到")
            {
                return Err(format!(
                    "移除开机自启失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("当前平台不支持开机自启".to_string())
    }
}
