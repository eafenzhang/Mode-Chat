//! 电脑控制（纯 Rust）：截屏、鼠标、键盘与滚轮。
//!
//! 这是 Mode CUA 的第一层（屏幕 + 输入注入）；无障碍元素树（UIA）后续补齐。
//! 全部走 Win32 GDI / SendInput，不依赖 Node 或外部 helper。

use std::sync::Arc;

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::types::{ImageContent, Tool};
use rpi_ai::Schema;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use windows_sys::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, SendInput,
};
use std::time::Duration;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{HDC, GetWindowDC};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SM_CXSCREEN, SM_CYSCREEN, SW_RESTORE, SetCursorPos, SetForegroundWindow, ShowWindow,
};

const CF_UNICODETEXT: u32 = 13;

#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdc: HDC, flags: u32) -> i32;
    // windows-sys 0.61 未导出这两个前台锁相关 API，按 Win32 签名手工声明。
    fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
    fn BringWindowToTop(hwnd: HWND) -> i32;
}

#[derive(Clone, Copy)]
enum Action {
    Screenshot,
    Move,
    Click,
    Type,
    Key,
    Scroll,
    ScreenSize,
    Windows,
    FocusWindow,
    WindowScreenshot,
    Drag,
    ClipboardGet,
    ClipboardSet,
    CursorPos,
}

struct ComputerTool {
    tool: Tool,
    action: Action,
    /// 会话身份；执行时按设置页「使用范围」再查一次。
    scope: String,
}

fn define(
    scope: &str,
    name: &str,
    description: &str,
    parameters: Value,
    action: Action,
) -> HarnessTool {
    HarnessTool::new(Arc::new(ComputerTool {
        tool: Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Schema(parameters),
            constrained_sampling: None,
        },
        action,
        scope: scope.to_string(),
    }))
}

/// 电脑控制工具集（截图 + 鼠标 + 键盘 + 滚轮 + 窗口/剪贴板）。
pub fn tools(identity: &str) -> Vec<HarnessTool> {
    let scope = identity;
    vec![
        define(
            scope,
            "computer_screenshot",
            "截取主屏幕（PNG，返回图片给模型）。",
            json!({"type": "object", "properties": {}}),
            Action::Screenshot,
        ),
        define(
            scope,
            "computer_screen_size",
            "返回主屏幕宽高（像素）。",
            json!({"type": "object", "properties": {}}),
            Action::ScreenSize,
        ),
        define(
            scope,
            "computer_move",
            "把鼠标移动到屏幕坐标 (x, y)。",
            json!({
                "type": "object",
                "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}},
                "required": ["x", "y"]
            }),
            Action::Move,
        ),
        define(
            scope,
            "computer_click",
            "在屏幕坐标 (x, y) 点击（默认左键，可传 button=right）。",
            json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "button": {"type": "string", "enum": ["left", "right"]}
                },
                "required": ["x", "y"]
            }),
            Action::Click,
        ),
        define(
            scope,
            "computer_type",
            "向当前前台窗口输入一段文本（Unicode）。",
            json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            }),
            Action::Type,
        ),
        define(
            scope,
            "computer_key",
            "按下组合键，例如 ctrl+c、alt+tab、enter、esc、win+r。",
            json!({
                "type": "object",
                "properties": {"keys": {"type": "string"}},
                "required": ["keys"]
            }),
            Action::Key,
        ),
        define(
            scope,
            "computer_scroll",
            "在当前位置滚动滚轮（正数向上，负数向下，单位 120）。",
            json!({
                "type": "object",
                "properties": {"delta": {"type": "integer"}},
                "required": ["delta"]
            }),
            Action::Scroll,
        ),
        define(
            scope,
            "computer_windows",
            "列出当前可见的桌面窗口（hwnd、pid、标题、位置、是否前台）。",
            json!({"type": "object", "properties": {}}),
            Action::Windows,
        ),
        define(
            scope,
            "computer_focus_window",
            "按 hwnd 或标题包含匹配聚焦一个窗口（最小化会先还原）。",
            json!({
                "type": "object",
                "properties": {
                    "hwnd": {"type": "string", "description": "窗口句柄（十六进制或十进制，可省略）"},
                    "title": {"type": "string", "description": "标题包含匹配（可省略）"}
                }
            }),
            Action::FocusWindow,
        ),
        define(
            scope,
            "computer_window_screenshot",
            "截取指定窗口（hwnd/标题）；不传则截当前前台窗口。",
            json!({
                "type": "object",
                "properties": {
                    "hwnd": {"type": "string"},
                    "title": {"type": "string"}
                }
            }),
            Action::WindowScreenshot,
        ),
        define(
            scope,
            "computer_drag",
            "从 (from_x, from_y) 拖拽到 (to_x, to_y)，可指定 button=left/right。",
            json!({
                "type": "object",
                "properties": {
                    "from_x": {"type": "integer"},
                    "from_y": {"type": "integer"},
                    "to_x": {"type": "integer"},
                    "to_y": {"type": "integer"},
                    "button": {"type": "string", "enum": ["left", "right"]}
                },
                "required": ["from_x", "from_y", "to_x", "to_y"]
            }),
            Action::Drag,
        ),
        define(
            scope,
            "computer_clipboard_get",
            "读取剪贴板纯文本。",
            json!({"type": "object", "properties": {}}),
            Action::ClipboardGet,
        ),
        define(
            scope,
            "computer_clipboard_set",
            "把一段文本写入剪贴板。",
            json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"]
            }),
            Action::ClipboardSet,
        ),
        define(
            scope,
            "computer_cursor_pos",
            "返回当前鼠标光标坐标。",
            json!({"type": "object", "properties": {}}),
            Action::CursorPos,
        ),
    ]
}

fn capture_screen() -> Result<(u32, u32, Vec<u8>), String> {
    // SAFETY: GDI 句柄在同一个函数内成对释放；像素缓冲按 32bpp 屏幕尺寸分配。
    unsafe {
        let width = GetSystemMetrics(SM_CXSCREEN) as u32;
        let height = GetSystemMetrics(SM_CYSCREEN) as u32;
        if width == 0 || height == 0 {
            return Err("读取屏幕尺寸失败".to_string());
        }
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("获取屏幕 DC 失败".to_string());
        }
        let mem = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width as i32, height as i32);
        let previous = SelectObject(mem, bitmap);
        let copied = BitBlt(
            mem,
            0,
            0,
            width as i32,
            height as i32,
            screen,
            0,
            0,
            SRCCOPY,
        );
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            mem,
            bitmap,
            0,
            height,
            pixels.as_mut_ptr() as *mut _,
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, previous);
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if copied == 0 || lines == 0 {
            return Err("截屏失败（BitBlt/GetDIBits 返回 0）".to_string());
        }
        for pixel in pixels.chunks_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Ok((width, height, pixels))
    }
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<String, String> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "截屏缓冲尺寸不匹配".to_string())?;
    let mut cursor = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|error| format!("PNG 编码失败：{error}"))?;
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(cursor.into_inner()))
}

/// 左键点击屏幕坐标（供 UI Automation 回退使用）。
pub fn click_at(x: i32, y: i32) -> Result<(), String> {
    // SAFETY: 坐标来自调用方；SendInput 复制数据。
    unsafe {
        SetCursorPos(x, y);
    }
    send_mouse(MOUSEEVENTF_LEFTDOWN, 0, 0, 0);
    send_mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0);
    Ok(())
}

/// 设置页「屏幕截图」：返回 PNG 字节。
pub fn screenshot_png() -> Result<Vec<u8>, String> {
    let (width, height, rgba) = capture_screen()?;
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "截屏缓冲尺寸不匹配".to_string())?;
    let mut cursor = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|error| format!("PNG 编码失败：{error}"))?;
    Ok(cursor.into_inner())
}

/// 主屏幕尺寸。
pub fn screen_size() -> (i32, i32) {
    // SAFETY: GetSystemMetrics 只读系统指标。
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

fn send_mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: INPUT 是 POD，长度与指针匹配；SendInput 会复制数据。
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_unicode(text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut inputs = Vec::with_capacity(units.len() * 2);
    for unit in units {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0,
                        wScan: unit,
                        dwFlags: flags as KEYBD_EVENT_FLAGS,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
        }
    }
    if inputs.is_empty() {
        return;
    }
    // SAFETY: 输入数组连续存放，长度与指针匹配。
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

fn key_vk(name: &str) -> Option<u16> {
    let upper = name.trim().to_ascii_uppercase();
    let vk = match upper.as_str() {
        "CTRL" | "CONTROL" => 0x11,
        "SHIFT" => 0x10,
        "ALT" => 0x12,
        "WIN" | "META" => 0x5B,
        "ENTER" | "RETURN" => 0x0D,
        "TAB" => 0x09,
        "ESC" | "ESCAPE" => 0x1B,
        "SPACE" => 0x20,
        "BACKSPACE" => 0x08,
        "DELETE" | "DEL" => 0x2E,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        other => {
            if let Some(number) = other.strip_prefix('F')
                && let Ok(index) = number.parse::<u16>()
                && (1..=24).contains(&index)
            {
                return Some(0x70 + index - 1);
            }
            let mut chars = other.chars();
            let ch = chars.next()?;
            if chars.next().is_none() {
                return Some(ch.to_ascii_uppercase() as u16);
            }
            return None;
        }
    };
    Some(vk)
}

pub fn send_key_combo(keys: &str) -> Result<(), String> {
    let parts: Vec<&str> = keys.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err("keys 为空".to_string());
    }
    let mut vks = Vec::new();
    for part in &parts {
        vks.push(key_vk(part).ok_or_else(|| format!("不支持的按键：{part}"))?);
    }
    for vk in &vks {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: *vk,
                    wScan: 0,
                    dwFlags: 0,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: 单条 INPUT。
        unsafe {
            SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
        }
    }
    for vk in vks.iter().rev() {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: *vk,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: 单条 INPUT。
        unsafe {
            SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
        }
    }
    Ok(())
}

struct WindowInfo {
    hwnd: isize,
    pid: u32,
    title: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    active: bool,
}

fn window_title(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; length as usize + 1];
    let written = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..written.max(0) as usize])
}

fn list_windows() -> Vec<WindowInfo> {
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return TRUE;
        }
        let title = window_title(hwnd);
        if title.trim().is_empty() {
            return TRUE;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            return TRUE;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let active = unsafe { GetForegroundWindow() } == hwnd;
        let out = unsafe { &mut *(lparam as *mut Vec<WindowInfo>) };
        out.push(WindowInfo {
            hwnd: hwnd as isize,
            pid,
            title,
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            active,
        });
        TRUE
    }
    let mut windows = Vec::new();
    unsafe {
        EnumWindows(Some(callback), &mut windows as *mut _ as LPARAM);
    }
    windows
}

fn parse_hwnd(value: &str) -> Option<HWND> {
    let text = value.trim();
    let number = if let Some(hex) = text.strip_prefix("0x") {
        isize::from_str_radix(hex, 16).ok()?
    } else {
        text.parse::<isize>().ok()?
    };
    Some(number as HWND)
}

fn find_window(params: &Value) -> Result<HWND, String> {
    if let Some(value) = params.get("hwnd").and_then(Value::as_str)
        && let Some(hwnd) = parse_hwnd(value)
    {
        return Ok(hwnd);
    }
    if let Some(value) = params.get("hwnd").and_then(Value::as_i64) {
        return Ok(value as HWND);
    }
    let needle = params
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if needle.is_empty() {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return Err("没有前台窗口".to_string());
        }
        return Ok(foreground);
    }
    list_windows()
        .into_iter()
        .find(|window| window.title.to_lowercase().contains(&needle))
        .map(|window| window.hwnd as HWND)
        .ok_or_else(|| format!("没有找到标题包含「{needle}」的窗口"))
}

fn focus_window(hwnd: HWND) -> Result<(), String> {
    if unsafe { IsIconic(hwnd) } != 0 {
        unsafe {
            ShowWindow(hwnd, SW_RESTORE);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // 前台锁：把当前线程临时挂到前台线程上，再切目标窗口。
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground == hwnd {
            return Ok(());
        }
        let current_thread = GetCurrentThreadId();
        let foreground_thread = if foreground.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, std::ptr::null_mut())
        };
        let attached = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, TRUE) != 0;
        let switched = SetForegroundWindow(hwnd) != 0;
        let _ = BringWindowToTop(hwnd);
        if attached {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
        if !switched && GetForegroundWindow() != hwnd {
            return Err("系统拒绝把该窗口切到前台（可能被前台锁限制）".to_string());
        }
    }
    Ok(())
}

fn capture_window(hwnd: HWND) -> Result<(u32, u32, Vec<u8>), String> {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err("读取窗口位置失败".to_string());
    }
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    // SAFETY: GDI 句柄在本函数内成对释放。
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("获取屏幕 DC 失败".to_string());
        }
        let mem = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width as i32, height as i32);
        let previous = SelectObject(mem, bitmap);
        let mut copied = PrintWindow(hwnd, mem, 2);
        if copied == 0 {
            let window_dc = GetWindowDC(hwnd);
            copied = BitBlt(
                mem,
                0,
                0,
                width as i32,
                height as i32,
                window_dc,
                0,
                0,
                SRCCOPY,
            );
            ReleaseDC(hwnd, window_dc);
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            mem,
            bitmap,
            0,
            height,
            pixels.as_mut_ptr() as *mut _,
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, previous);
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if copied == 0 || lines == 0 {
            return Err("窗口截图失败（PrintWindow/BitBlt 返回 0）".to_string());
        }
        for pixel in pixels.chunks_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Ok((width, height, pixels))
    }
}

fn drag(from: (i32, i32), to: (i32, i32), button: &str) -> Result<(), String> {
    let (down, up) = if button == "right" {
        (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
    } else {
        (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
    };
    unsafe {
        SetCursorPos(from.0, from.1);
    }
    send_mouse(down, 0, 0, 0);
    let steps = 18;
    for step in 1..=steps {
        let x = from.0 + (to.0 - from.0) * step / steps;
        let y = from.1 + (to.1 - from.1) * step / steps;
        unsafe {
            SetCursorPos(x, y);
        }
        std::thread::sleep(Duration::from_millis(12));
    }
    send_mouse(up, 0, 0, 0);
    Ok(())
}

fn cursor_pos() -> Result<(i32, i32), String> {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return Err("读取光标位置失败".to_string());
    }
    Ok((point.x, point.y))
}

/// 别的程序可能短暂占用剪贴板：重试几次再报错。
fn open_clipboard() -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

fn clipboard_get() -> Result<String, String> {
    // SAFETY: 剪贴板在 Open/Close 之间成对操作；GlobalLock 指针在解锁前有效。
    unsafe {
        if !open_clipboard() {
            return Err("打开剪贴板失败（可能被其它程序占用）".to_string());
        }
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            let _ = CloseClipboard();
            return Ok(String::new());
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            let _ = CloseClipboard();
            return Err("读取剪贴板数据失败".to_string());
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            let _ = CloseClipboard();
            return Err("锁定剪贴板内存失败".to_string());
        }
        let mut units = Vec::new();
        let mut offset = 0usize;
        loop {
            let unit = *(pointer as *const u16).add(offset);
            if unit == 0 {
                break;
            }
            units.push(unit);
            offset += 1;
            if offset > 4_000_000 {
                break;
            }
        }
        let _ = GlobalUnlock(handle);
        let _ = CloseClipboard();
        Ok(String::from_utf16_lossy(&units))
    }
}

fn clipboard_set(text: &str) -> Result<(), String> {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * std::mem::size_of::<u16>();
    // SAFETY: 分配后写入 UTF-16，并把所有权交给系统剪贴板；失败路径释放内存。
    unsafe {
        if !open_clipboard() {
            return Err("打开剪贴板失败（可能被其它程序占用）".to_string());
        }
        let _ = EmptyClipboard();
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() {
            let _ = CloseClipboard();
            return Err("分配剪贴板内存失败".to_string());
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            let _ = windows_sys::Win32::Foundation::GlobalFree(handle);
            let _ = CloseClipboard();
            return Err("锁定剪贴板内存失败".to_string());
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), pointer as *mut u16, units.len());
        let _ = GlobalUnlock(handle);
        if SetClipboardData(CF_UNICODETEXT, handle).is_null() {
            let _ = windows_sys::Win32::Foundation::GlobalFree(handle);
            let _ = CloseClipboard();
            return Err("写入剪贴板失败".to_string());
        }
        let _ = CloseClipboard();
    }
    Ok(())
}

fn text_result(text: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextContentOrImage::Text(rpi_ai::TextContent {
            kind: rpi_ai::types::TextContentType,
            text,
            text_signature: None,
        })],
        details: Value::Null,
        usage: None,
        added_tool_names: Vec::new(),
        terminate: false,
    }
}

fn image_result(data: String) -> Result<AgentToolResult, String> {
    let image: ImageContent = serde_json::from_value(json!({
        "type": "image",
        "data": data,
        "mimeType": "image/png"
    }))
    .map_err(|error| format!("截图编码失败：{error}"))?;
    Ok(AgentToolResult {
        content: vec![TextContentOrImage::Image(image)],
        details: Value::Null,
        usage: None,
        added_tool_names: Vec::new(),
        terminate: false,
    })
}

#[async_trait]
impl AgentTool for ComputerTool {
    fn schema(&self) -> &Tool {
        &self.tool
    }

    fn label(&self) -> &str {
        &self.tool.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(rpi_agent::ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, rpi_agent::AgentError> {
        if !crate::tool_scope::allowed(crate::tool_scope::Family::Computer, &self.scope) {
            return Err(rpi_agent::AgentError::Tool(crate::tool_scope::denial(
                crate::tool_scope::Family::Computer,
            )));
        }
        let result = match self.action {
            Action::Screenshot => {
                let (width, height, rgba) = capture_screen().map_err(rpi_agent::AgentError::Tool)?;
                let data = encode_png(width, height, &rgba).map_err(rpi_agent::AgentError::Tool)?;
                image_result(data).map_err(rpi_agent::AgentError::Tool)?
            }
            Action::ScreenSize => text_result(format!(
                "{}x{}",
                unsafe { GetSystemMetrics(SM_CXSCREEN) },
                unsafe { GetSystemMetrics(SM_CYSCREEN) }
            )),
            Action::Move => {
                let (x, y) = coords(&params)?;
                // SAFETY: 坐标来自工具参数。
                unsafe {
                    SetCursorPos(x, y);
                }
                text_result(format!("鼠标已移动到 ({x}, {y})"))
            }
            Action::Click => {
                let (x, y) = coords(&params)?;
                let button = params
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left");
                let count = params
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 3);
                let (down, up, label) = match button {
                    "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, "右键"),
                    "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, "中键"),
                    _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, "左键"),
                };
                // SAFETY: 坐标来自工具参数。
                unsafe {
                    SetCursorPos(x, y);
                }
                for index in 0..count {
                    if index > 0 {
                        std::thread::sleep(Duration::from_millis(80));
                    }
                    send_mouse(down, 0, 0, 0);
                    send_mouse(up, 0, 0, 0);
                }
                text_result(format!(
                    "已在 ({x}, {y}) {label}点击{}",
                    if count > 1 {
                        format!(" {count} 次")
                    } else {
                        String::new()
                    }
                ))
            }
            Action::Type => {
                let text = params
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 text".to_string()))?;
                send_unicode(text);
                text_result(format!("已输入 {} 个字符", text.chars().count()))
            }
            Action::Key => {
                let keys = params
                    .get("keys")
                    .and_then(Value::as_str)
                    .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 keys".to_string()))?;
                send_key_combo(keys).map_err(rpi_agent::AgentError::Tool)?;
                text_result(format!("已按键：{keys}"))
            }
            Action::Scroll => {
                let delta = params
                    .get("delta")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 delta".to_string()))?;
                send_mouse(MOUSEEVENTF_WHEEL, 0, 0, delta as i32);
                text_result(format!("已滚动 {delta}"))
            }
            Action::Windows => {
                let windows = list_windows();
                if windows.is_empty() {
                    text_result("没有可见窗口".to_string())
                } else {
                    text_result(
                        windows
                            .into_iter()
                            .map(|window| {
                                format!(
                                    "{}{} pid={} {}x{} at ({},{}) {}",
                                    window.hwnd,
                                    if window.active { " [前台]" } else { "" },
                                    window.pid,
                                    window.width,
                                    window.height,
                                    window.x,
                                    window.y,
                                    window.title
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                }
            }
            Action::FocusWindow => {
                let hwnd = find_window(&params).map_err(rpi_agent::AgentError::Tool)?;
                focus_window(hwnd).map_err(rpi_agent::AgentError::Tool)?;
                text_result(format!("已聚焦窗口 {}", window_title(hwnd)))
            }
            Action::WindowScreenshot => {
                // 指定了 hwnd/title 就严格按它；都没给才截前台窗口。
                let hwnd = if params.get("hwnd").is_some() || params.get("title").is_some() {
                    find_window(&params).map_err(rpi_agent::AgentError::Tool)?
                } else {
                    unsafe { GetForegroundWindow() }
                };
                if hwnd.is_null() {
                    return Err(rpi_agent::AgentError::Tool("没有前台窗口".to_string()));
                }
                let (width, height, rgba) =
                    capture_window(hwnd).map_err(rpi_agent::AgentError::Tool)?;
                let data = encode_png(width, height, &rgba).map_err(rpi_agent::AgentError::Tool)?;
                image_result(data).map_err(rpi_agent::AgentError::Tool)?
            }
            Action::Drag => {
                let number = |key: &str| -> Result<i32, rpi_agent::AgentError> {
                    params
                        .get(key)
                        .and_then(Value::as_i64)
                        .map(|value| value as i32)
                        .ok_or_else(|| {
                            rpi_agent::AgentError::Tool(format!("缺少 {key}"))
                        })
                };
                let from = (number("from_x")?, number("from_y")?);
                let to = (number("to_x")?, number("to_y")?);
                let button = params
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left");
                drag(from, to, button).map_err(rpi_agent::AgentError::Tool)?;
                text_result(format!(
                    "已从 ({}, {}) 拖到 ({}, {})",
                    from.0, from.1, to.0, to.1
                ))
            }
            Action::ClipboardGet => {
                text_result(clipboard_get().map_err(rpi_agent::AgentError::Tool)?)
            }
            Action::ClipboardSet => {
                let text = params
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 text".to_string()))?;
                clipboard_set(text).map_err(rpi_agent::AgentError::Tool)?;
                text_result(format!("已写入剪贴板（{} 字符）", text.chars().count()))
            }
            Action::CursorPos => {
                let (x, y) = cursor_pos().map_err(rpi_agent::AgentError::Tool)?;
                text_result(format!("{x},{y}"))
            }
        };
        Ok(result)
    }
}

fn coords(params: &Value) -> Result<(i32, i32), rpi_agent::AgentError> {
    let x = params
        .get("x")
        .and_then(Value::as_i64)
        .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 x".to_string()))?;
    let y = params
        .get("y")
        .and_then(Value::as_i64)
        .ok_or_else(|| rpi_agent::AgentError::Tool("缺少 y".to_string()))?;
    Ok((x as i32, y as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_screen_encodes_png() {
        let (width, height, rgba) = capture_screen().expect("screen capture");
        assert!(width > 0 && height > 0);
        let png = encode_png(width, height, &rgba).expect("png");
        assert!(png.len() > 100);
    }

    #[test]
    fn windows_and_cursor_are_readable() {
        assert!(!list_windows().is_empty(), "至少应有可见窗口");
        cursor_pos().expect("cursor position");
    }

    #[test]
    fn clipboard_roundtrip() {
        let previous = clipboard_get().ok();
        clipboard_set("mode-chat-uat").expect("set clipboard");
        assert_eq!(clipboard_get().expect("get clipboard"), "mode-chat-uat");
        if let Some(previous) = previous {
            let _ = clipboard_set(&previous);
        }
    }
}
