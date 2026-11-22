//! 窗口原生外观（框架级）：DWM 阴影 + 任务栏图标。
//!
//! gpui 自绘无边框窗口默认没有系统阴影，视觉上像一块"贴"在桌面上的矩形（产品安装版
//! 用 DWM 给的就是带阴影的窗口）。这里在窗口起来后补两件事：
//!
//! 1. `WM_SETICON`：把随包嵌入的 `.ico` 设成任务栏/Alt-Tab 图标。
//! 2. DWM 投影：**未做**。本窗口样式已含 WS_CAPTION|WS_THICKFRAME（gpui 自绘标题栏），
//!    单纯 `DwmExtendFrameIntoClientArea` 不产生可见投影；要投影得在 wndproc 里处理
//!    `WM_NCCALCSIZE` 把非客户区让给自绘——wndproc 在 gpui 内部，本试点不改。
//!    宁可没有投影，也不引入"系统边框吃掉内容"的风险。
//!
//! 两件事都走 `FindWindowW("Zed::Window", "Mode Chat")` 定位自己的 HWND（gpui 这版没有
//! 暴露 raw window handle 的公开路径）。

/// 随包嵌入的应用图标（与产品安装包同一个 .ico）。
const ICON_BYTES: &[u8] = include_bytes!("../assets/icons/app.ico");

/// 随包嵌入的应用图标字节（托盘复用同一份）。
pub fn icon_bytes() -> &'static [u8] {
    ICON_BYTES
}

#[cfg(windows)]
pub fn apply() {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
        fn LoadImageW(
            instance: *mut c_void,
            name: *const u16,
            kind: u32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> *mut c_void;
        fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let class = wide("Zed::Window");
    let title = wide("Mode Chat");
    // SAFETY: 两个指针都指向以 NUL 结尾的宽字符串。
    let hwnd = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
    if hwnd.is_null() {
        return;
    }

    // 2) 任务栏图标：把嵌入的 ico 落到试点目录再从文件加载（避免解析 ICO 结构）。
    // 每次都覆盖：旧版本缓存会让任务栏一直显示上一代图标。
    let icon_path = crate::instance::state_dir().join("app.ico");
    if let Err(error) = std::fs::write(&icon_path, ICON_BYTES) {
        eprintln!("[window] 图标写入失败：{error}");
        return;
    }
    let icon_wide = wide(&icon_path.to_string_lossy());
    const IMAGE_ICON: u32 = 1;
    const LR_LOADFROMFILE: u32 = 0x0000_0010;
    // SAFETY: 路径是以 NUL 结尾的宽字符串；失败返回空指针，下面判空。
    let hicon = unsafe {
        LoadImageW(
            std::ptr::null_mut(),
            icon_wide.as_ptr(),
            IMAGE_ICON,
            0,
            0,
            LR_LOADFROMFILE,
        )
    };
    if hicon.is_null() {
        eprintln!("[window] 图标加载失败：{}", icon_path.display());
        return;
    }
    const WM_SETICON: u32 = 0x0080;
    const ICON_BIG: usize = 1;
    const ICON_SMALL: usize = 0;
    // SAFETY: hicon 来自 LoadImageW 且已判空；WM_SETICON 的语义是接管该句柄。
    unsafe {
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL, hicon as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG, hicon as isize);
    }
}

#[cfg(not(windows))]
pub fn apply() {}

#[cfg(windows)]
fn window_handle() -> Option<*mut std::ffi::c_void> {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    }
    let wide = |value: &str| -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    };
    let class = wide("Zed::Window");
    let title = wide("Mode Chat");
    // SAFETY: 两个指针都指向以 NUL 结尾的宽字符串。
    let hwnd = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
    (!hwnd.is_null()).then_some(hwnd)
}

/// 最小化窗口（gpui 的 `minimize_window` 走同一 API，但这里保留显式兜底）。
#[cfg(windows)]
pub fn minimize() {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindowAsync(hwnd: Hwnd, cmd: i32) -> i32;
    }
    if let Some(hwnd) = window_handle() {
        const SW_MINIMIZE: i32 = 6;
        // SAFETY: hwnd 来自 FindWindowW 且已判空；ShowWindowAsync 不接管句柄。
        unsafe {
            ShowWindowAsync(hwnd, SW_MINIMIZE);
        }
    }
}

/// 最大化/还原切换（gpui 的 `zoom_window` 只会最大化，不能还原）。
#[cfg(windows)]
pub fn toggle_maximize() {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindowAsync(hwnd: Hwnd, cmd: i32) -> i32;
        fn IsZoomed(hwnd: Hwnd) -> i32;
    }
    let Some(hwnd) = window_handle() else {
        if std::env::var("BOT_PILOT_DEBUG").ok().as_deref() == Some("1") {
            eprintln!("[win] toggle_maximize: hwnd not found");
        }
        return;
    };
    // SAFETY: hwnd 已判空；两个调用都只读/设置窗口状态。
    let zoomed = unsafe { IsZoomed(hwnd) } != 0;
    if std::env::var("BOT_PILOT_DEBUG").ok().as_deref() == Some("1") {
        eprintln!("[win] toggle_maximize: hwnd={hwnd:?} zoomed={zoomed}");
    }
    const SW_MAXIMIZE: i32 = 3;
    const SW_RESTORE: i32 = 9;
    let cmd = if zoomed { SW_RESTORE } else { SW_MAXIMIZE };
    unsafe {
        ShowWindowAsync(hwnd, cmd);
    }
}

#[cfg(not(windows))]
pub fn minimize() {}

#[cfg(not(windows))]
pub fn toggle_maximize() {}

/// 置顶/取消置顶窗口（微信标题栏的图钉）。
#[cfg(windows)]
pub fn set_topmost(on: bool) {
    use std::ffi::c_void;
    type Hwnd = *mut c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    }
    let Some(hwnd) = window_handle() else {
        return;
    };
    // HWND_TOPMOST = -1, HWND_NOTOPMOST = -2；SWP_NOSIZE|SWP_NOMOVE|SWP_NOACTIVATE。
    let after = if on { -1isize } else { -2isize } as Hwnd;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    // SAFETY: hwnd 来自 FindWindowW 且已判空；只改 Z 序，不改尺寸/位置。
    unsafe {
        SetWindowPos(hwnd, after, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE);
    }
}

#[cfg(not(windows))]
pub fn set_topmost(_on: bool) {}
