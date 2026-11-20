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
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

#[derive(Clone, Copy)]
enum Action {
    Screenshot,
    Move,
    Click,
    Type,
    Key,
    Scroll,
    ScreenSize,
}

struct ComputerTool {
    tool: Tool,
    action: Action,
}

fn define(name: &str, description: &str, parameters: Value, action: Action) -> HarnessTool {
    HarnessTool::new(Arc::new(ComputerTool {
        tool: Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Schema(parameters),
            constrained_sampling: None,
        },
        action,
    }))
}

/// 电脑控制工具集（截图 + 鼠标 + 键盘 + 滚轮）。
pub fn tools() -> Vec<HarnessTool> {
    vec![
        define(
            "computer_screenshot",
            "截取主屏幕（PNG，返回图片给模型）。",
            json!({"type": "object", "properties": {}}),
            Action::Screenshot,
        ),
        define(
            "computer_screen_size",
            "返回主屏幕宽高（像素）。",
            json!({"type": "object", "properties": {}}),
            Action::ScreenSize,
        ),
        define(
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
            "computer_scroll",
            "在当前位置滚动滚轮（正数向上，负数向下，单位 120）。",
            json!({
                "type": "object",
                "properties": {"delta": {"type": "integer"}},
                "required": ["delta"]
            }),
            Action::Scroll,
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

fn send_key_combo(keys: &str) -> Result<(), String> {
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
                let right = params.get("button").and_then(Value::as_str) == Some("right");
                // SAFETY: 坐标来自工具参数。
                unsafe {
                    SetCursorPos(x, y);
                }
                send_mouse(
                    if right { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_LEFTDOWN },
                    0,
                    0,
                    0,
                );
                send_mouse(
                    if right { MOUSEEVENTF_RIGHTUP } else { MOUSEEVENTF_LEFTUP },
                    0,
                    0,
                    0,
                );
                text_result(format!(
                    "已在 ({x}, {y}) {}点击",
                    if right { "右键" } else { "左键" }
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
}
