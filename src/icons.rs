//! 图标（原版同源 lucide SVG）。
//!
//! 为什么用 SVG 而不是手绘 path：Web 版的图标就是 lucide-react（ISC），GPUI 这版
//! 支持 `svg().data(&bytes)`，直接渲染同源 SVG 原文即可得到完全一致的几何、线宽
//! 和圆头线端，不需要近似重画。
//!
//! 资产在 `assets/icons/`，由 `scripts/gen-icons.mjs` 一次性生成（不进构建链）。
//! 颜色走 `currentColor`：调用方用 `text_color` 指定，悬停换色不重新栅格化。

use gpui::{px, svg, Hsla, IntoElement, Rgba, Styled};

/// 图标种类（只收当前界面用到的；新增图标先加资产再加枚举）。
// TODO(visual): Task 3/4 接入头像徽标与标题栏后删掉这个 allow。
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Search,
    Plus,
    Ellipsis,
    Settings,
    X,
    Check,
    ArrowUp,
    Stop,
    ChevronLeft,
    ChevronRight,
    Paperclip,
    Minus,
    WindowMaximize,
    WindowRestore,
    PanelRight,
    Hash,
    Pin,
    Star,
    Crown,
    Pencil,
    Trash,
    EyeOff,
    Archive,
    Chart,
    Cpu,
    Plug,
    Calendar,
    Refresh,
    Download,
    MessageCircle,
    Bot,
    ListChecks,
    Building,
    Smile,
    Shield,
    Terminal,
    Folder,
    Scissors,
    Mic,
    BellOff,
    Key,
    Info,
    MessageSquare,
    Sparkles,
    Contacts,
    MessageCircleFilled,
    Brain,
    Pushpin,
    Sun,
    Moon,
    Monitor,
    ChevronDown,
}

impl Icon {
    /// 全部图标（测试与清单用；顺序与 `svg()` 无关）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [Icon; 52] = [
        Icon::Search,
        Icon::Plus,
        Icon::Ellipsis,
        Icon::Settings,
        Icon::X,
        Icon::Check,
        Icon::ArrowUp,
        Icon::Stop,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::Paperclip,
        Icon::Minus,
        Icon::WindowMaximize,
        Icon::WindowRestore,
        Icon::PanelRight,
        Icon::Hash,
        Icon::Pin,
        Icon::Star,
        Icon::Crown,
        Icon::Pencil,
        Icon::Trash,
        Icon::EyeOff,
        Icon::Archive,
        Icon::Chart,
        Icon::Cpu,
        Icon::Plug,
        Icon::Calendar,
        Icon::Refresh,
        Icon::Download,
        Icon::MessageCircle,
        Icon::Bot,
        Icon::ListChecks,
        Icon::Building,
        Icon::Smile,
        Icon::Shield,
        Icon::Terminal,
        Icon::Folder,
        Icon::Scissors,
        Icon::Mic,
        Icon::BellOff,
        Icon::Key,
        Icon::Info,
        Icon::MessageSquare,
        Icon::Sparkles,
        Icon::Contacts,
        Icon::MessageCircleFilled,
        Icon::Brain,
        Icon::Pushpin,
        Icon::Sun,
        Icon::Moon,
        Icon::Monitor,
        Icon::ChevronDown,
    ];

    /// 图标对应的 SVG 原文。
    pub fn svg(self) -> &'static [u8] {
        match self {
            Icon::Search => include_bytes!("../assets/icons/search.svg"),
            Icon::Plus => include_bytes!("../assets/icons/plus.svg"),
            Icon::Ellipsis => include_bytes!("../assets/icons/ellipsis.svg"),
            Icon::Settings => include_bytes!("../assets/icons/settings.svg"),
            Icon::X => include_bytes!("../assets/icons/x.svg"),
            Icon::Check => include_bytes!("../assets/icons/check.svg"),
            Icon::ArrowUp => include_bytes!("../assets/icons/arrow-up.svg"),
            Icon::Stop => include_bytes!("../assets/icons/stop.svg"),
            Icon::ChevronLeft => include_bytes!("../assets/icons/chevron-left.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/chevron-right.svg"),
            Icon::Paperclip => include_bytes!("../assets/icons/paperclip.svg"),
            Icon::Minus => include_bytes!("../assets/icons/minus.svg"),
            Icon::WindowMaximize => include_bytes!("../assets/icons/window-maximize.svg"),
            Icon::WindowRestore => include_bytes!("../assets/icons/window-restore.svg"),
            Icon::PanelRight => include_bytes!("../assets/icons/panel-right.svg"),
            Icon::Hash => include_bytes!("../assets/icons/hash.svg"),
            Icon::Pin => include_bytes!("../assets/icons/pin.svg"),
            Icon::Star => include_bytes!("../assets/icons/star.svg"),
            Icon::Crown => include_bytes!("../assets/icons/crown.svg"),
            Icon::Pencil => include_bytes!("../assets/icons/pencil.svg"),
            Icon::Trash => include_bytes!("../assets/icons/trash.svg"),
            Icon::EyeOff => include_bytes!("../assets/icons/eye-off.svg"),
            Icon::Archive => include_bytes!("../assets/icons/archive.svg"),
            Icon::Chart => include_bytes!("../assets/icons/chart.svg"),
            Icon::Cpu => include_bytes!("../assets/icons/cpu.svg"),
            Icon::Plug => include_bytes!("../assets/icons/plug.svg"),
            Icon::Calendar => include_bytes!("../assets/icons/calendar.svg"),
            Icon::Refresh => include_bytes!("../assets/icons/refresh.svg"),
            Icon::Download => include_bytes!("../assets/icons/download.svg"),
            Icon::MessageCircle => include_bytes!("../assets/icons/message-circle.svg"),
            Icon::Bot => include_bytes!("../assets/icons/bot.svg"),
            Icon::ListChecks => include_bytes!("../assets/icons/list-checks.svg"),
            Icon::Building => include_bytes!("../assets/icons/building.svg"),
            Icon::Smile => include_bytes!("../assets/icons/smile.svg"),
            Icon::Shield => include_bytes!("../assets/icons/shield.svg"),
            Icon::Terminal => include_bytes!("../assets/icons/square-terminal.svg"),
            Icon::Folder => include_bytes!("../assets/icons/folder.svg"),
            Icon::Scissors => include_bytes!("../assets/icons/scissors.svg"),
            Icon::Mic => include_bytes!("../assets/icons/mic.svg"),
            Icon::BellOff => include_bytes!("../assets/icons/bell-off.svg"),
            Icon::Key => include_bytes!("../assets/icons/key.svg"),
            Icon::Info => include_bytes!("../assets/icons/info.svg"),
            Icon::MessageSquare => include_bytes!("../assets/icons/message-square.svg"),
            Icon::Sparkles => include_bytes!("../assets/icons/sparkles.svg"),
            Icon::Contacts => include_bytes!("../assets/icons/contacts.svg"),
            Icon::MessageCircleFilled => {
                include_bytes!("../assets/icons/message-circle-filled.svg")
            }
            Icon::Brain => include_bytes!("../assets/icons/brain.svg"),
            Icon::Pushpin => include_bytes!("../assets/icons/pushpin.svg"),
            Icon::Sun => include_bytes!("../assets/icons/sun.svg"),
            Icon::Moon => include_bytes!("../assets/icons/moon.svg"),
            Icon::Monitor => include_bytes!("../assets/icons/monitor.svg"),
            Icon::ChevronDown => include_bytes!("../assets/icons/chevron-down.svg"),
        }
    }
}

/// 画一个图标。`size` 是边长，颜色由调用方给（`currentColor`）。
/// 指定描边粗细的图标（用于输入框/标题栏对齐微信的细线风格）。
pub fn icon_with_stroke(
    kind: Icon,
    size: f32,
    color: Rgba,
    stroke_width: f32,
) -> impl IntoElement {
    let source = String::from_utf8_lossy(kind.svg())
        .replace("stroke-width=\"1.7\"", &format!("stroke-width=\"{stroke_width}\""));
    svg()
        .data(source.as_bytes())
        .size(px(size))
        .text_color(Hsla::from(color))
}

pub fn icon(kind: Icon, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg())
        .size(px(size))
        .text_color(Hsla::from(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_well_formed_svg() {
        for icon in Icon::ALL {
            let bytes = icon.svg();
            assert!(bytes.starts_with(b"<svg"), "{icon:?} 不是 svg");
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(
                text.contains("viewBox=\"0 0 24 24\""),
                "{icon:?} 缺 24 画布"
            );
            assert!(text.contains("currentColor"), "{icon:?} 缺 currentColor");
            assert!(text.trim_end().ends_with("</svg>"), "{icon:?} 未闭合");
        }
    }

    #[test]
    fn icon_set_matches_design_inventory() {
        assert_eq!(Icon::ALL.len(), 52);
    }
}
