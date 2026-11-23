//! Windows 子进程窗口纪律。
//!
//! 发布包是 GUI 子系统（见 `main.rs` 顶部的 `windows_subsystem`），自身没有控制台；
//! 控制台程序（cmd / node / curl / tar / reg / powershell）按默认方式启动时，Windows
//! 会给它们各开一个黑框窗口。用户看到的效果是「启动程序弹出一个 cmd 窗口，关掉它
//! 程序就退出」（2026-11-23 反馈）。因此凡是不需要交互窗口的子进程，统一加
//! `CREATE_NO_WINDOW`：stdio 仍按调用方给的管道/空设备走，只是不分配控制台窗口。

/// `CREATE_NO_WINDOW`：子进程在没有控制台的环境里运行，不会新开窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 给 `std::process::Command` 加「不出窗口」标志。
#[cfg(windows)]
pub fn no_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// 给 `tokio::process::Command` 加「不出窗口」标志。
#[cfg(windows)]
pub fn no_console_async(command: &mut tokio::process::Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// 非 Windows 平台没有控制台窗口问题，保留空实现让调用点不分叉。
#[cfg(not(windows))]
pub fn no_console(_command: &mut std::process::Command) {}

/// 非 Windows 平台没有控制台窗口问题，保留空实现让调用点不分叉。
#[cfg(not(windows))]
pub fn no_console_async(_command: &mut tokio::process::Command) {}
