//! 微信 4.x 深色皮肤 token（取自 `2026-11-16` 现场截图：896×648 窗口像素采样）。
//!
//! 仅用于 `BOT_PILOT_WECHAT_UI` 三栏外壳；经典三栏仍走 `theme.rs`。

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{linear_color_stop, linear_gradient, Background, Rgba};

const fn rgb(value: u32) -> Rgba {
    rgba(value, 255)
}

const fn rgba(value: u32, a: u8) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

pub const FONT_FAMILY: &str = "Microsoft YaHei UI";

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// 启动时由 main 置位；设置页/对话框 helper 用它切到微信表面。
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Relaxed);
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

// 布局（微信 4.x 896×648 实测）
pub const TITLEBAR_HEIGHT: f32 = 28.0;
pub const NAV_WIDTH: f32 = 60.0;
pub const LIST_WIDTH: f32 = 246.0;
/// 右侧信息栏在本皮肤里默认不显示（微信没有常驻右栏）。
pub const PANEL_WIDTH: f32 = 166.0;
pub const NAV_ITEM_HEIGHT: f32 = 44.0;
pub const LIST_ROW_HEIGHT: f32 = 56.0;
pub const SEARCH_HEIGHT: f32 = 32.0;
pub const COMPOSER_HEIGHT: f32 = 176.0;

// 窗口 / 标题栏
pub fn window_bg() -> Rgba {
    rgb(0x1e1e1f)
}
pub fn titlebar_bg() -> Rgba {
    rgb(0x252526)
}

// 左导轨
pub fn nav_item_selected_bg() -> Rgba {
    rgb(0x2f2f30)
}
pub fn nav_item_hover_bg() -> Rgba {
    rgb(0x2f2f30)
}
pub fn nav_text() -> Rgba {
    rgb(0xc8c8c8)
}
pub fn nav_text_active() -> Rgba {
    rgb(0xededed)
}
pub fn nav_icon() -> Rgba {
    rgb(0x9a9a9c)
}
pub fn nav_icon_active() -> Rgba {
    rgb(0x00a361)
}
pub fn nav_badge_bg() -> Rgba {
    rgb(0xe24747)
}
pub fn nav_separator() -> Rgba {
    rgb(0x3a3a3b)
}

// 会话列表
pub fn list_hover() -> Rgba {
    rgb(0x3a3a3b)
}
pub fn list_title() -> Rgba {
    rgb(0xe8e8e8)
}
pub fn list_title_selected() -> Rgba {
    rgb(0xffffff)
}
pub fn list_preview() -> Rgba {
    rgb(0x8a8a8c)
}
pub fn list_preview_selected() -> Rgba {
    rgba(0xffffff, 0xd9)
}
pub fn list_time() -> Rgba {
    rgb(0x7a7a7c)
}
pub fn list_time_selected() -> Rgba {
    rgba(0xffffff, 0xa6)
}
pub fn search_bg() -> Rgba {
    rgb(0x454547)
}
pub fn star() -> Rgba {
    rgb(0xfbc224)
}

// 会话区
pub fn chat_bg() -> Rgba {
    rgb(0x1e1e1f)
}
pub fn chat_title() -> Rgba {
    rgb(0xededed)
}
pub fn chat_subtitle() -> Rgba {
    rgb(0x8a8a8c)
}
pub fn bubble_in() -> Rgba {
    rgb(0x2f2f30)
}
pub fn bubble_out() -> Rgba {
    rgb(0x35d28d)
}
pub fn bubble_text() -> Rgba {
    rgb(0xd8d8d8)
}
/// 微信发送气泡是绿底黑字。
pub fn bubble_out_text() -> Rgba {
    rgb(0x1a1a1a)
}
pub fn sender_label() -> Rgba {
    rgb(0x8a8a8c)
}
pub fn time_label() -> Rgba {
    rgb(0x7a7a7c)
}
pub fn chip_bg() -> Rgba {
    rgb(0x2f2f30)
}
pub fn chip_text() -> Rgba {
    rgb(0xa0a0a2)
}

// 输入区
pub fn composer_bg() -> Rgba {
    rgb(0x1e1e1f)
}
pub fn composer_border() -> Rgba {
    rgb(0x2f2f30)
}
pub fn composer_placeholder() -> Rgba {
    rgb(0x7a7a7c)
}
pub fn send_disabled_bg() -> Rgba {
    rgb(0x3a3a3b)
}
pub fn send_disabled_text() -> Rgba {
    rgb(0x7a7a7c)
}
pub fn send_enabled() -> Rgba {
    rgb(0x07c160)
}

// 通用
pub fn icon() -> Rgba {
    rgb(0xa8a8aa)
}
pub fn icon_strong() -> Rgba {
    rgb(0xd8d8da)
}
pub fn accent() -> Rgba {
    rgb(0x07c160)
}
pub fn divider() -> Rgba {
    rgb(0x2f2f30)
}

// 信息栏（本皮肤默认不显示；保留 token 供以后做聊天信息面板）
pub fn panel_bg() -> Rgba {
    rgb(0x252526)
}
pub fn panel_title() -> Rgba {
    rgb(0xd8d8da)
}
pub fn panel_text() -> Rgba {
    rgb(0xa9a9ab)
}
pub fn panel_card_bg() -> Rgba {
    rgb(0x2f2f30)
}
pub fn panel_divider() -> Rgba {
    rgb(0x3a3a3b)
}
pub fn online_dot() -> Rgba {
    rgb(0x07c160)
}
pub fn watermark() -> Rgba {
    rgba(0xffffff, 0)
}

/// 导轨/列表背景：微信是纯色，这里仍返回 Background 方便调用点统一。
pub fn nav_gradient() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgb(0x252526), 0.0),
        linear_color_stop(rgb(0x252526), 1.0),
    )
}

pub fn list_gradient() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgb(0x2f2f30), 0.0),
        linear_color_stop(rgb(0x2f2f30), 1.0),
    )
}

/// 选中会话行：微信是整行纯绿。
pub fn selected_gradient() -> Background {
    linear_gradient(
        90.0,
        linear_color_stop(rgb(0x0da869), 0.0),
        linear_color_stop(rgb(0x0da869), 1.0),
    )
}

/// 微信彩色首字头像色板（深色下低饱和）。
pub const AVATAR_COLORS: [u32; 8] = [
    0x8a5a5a, 0x4a7a5a, 0x4a6a8a, 0x6a5a8a, 0x8a7a4a, 0x5a6a6a, 0x7a4a6a, 0x4a5a7a,
];

pub fn avatar_color(id: &str) -> Rgba {
    let mut hash: u32 = 0;
    for unit in id.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(unit as u32);
    }
    rgb(AVATAR_COLORS[(hash as usize) % AVATAR_COLORS.len()])
}
