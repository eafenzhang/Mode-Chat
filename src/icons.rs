//! 图标（Remix Icon，Apache-2.0）。
//!
//! 为什么用 SVG 原文而不是手绘 path：GPUI 的 `svg().data()` 直接渲染同源 SVG，
//! 几何/线宽/圆角与设计稿一致，不需要近似重画。
//!
//! 资产在 `assets/icons/ui/`，由 `scripts/gen-icons-lobe.py` 一次性生成
//! （不进构建链）。画布是 24×24。
//!
//! 颜色走 `currentColor`：调用方用 `text_color` 指定，悬停换色不重新栅格化。
//!
//! 为什么从 Phosphor 换成 Remix：Phosphor 是实心填充、外框偏方，在 21px 下
//! 显得厚重。Remix 的 `-line` 变体是 2px 线框、转角更圆，与旧版手绘风格更接近，
//! 且同样提供 `-fill` 成对变体，菜单选中态可以照旧切实心。

use gpui::{px, svg, Hsla, IntoElement, Rgba, Styled};

/// 图标种类（只收当前界面用到的；新增图标先加资产再加枚举）。
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
    /// 展开右侧栏（侧栏轮廓）
    SidebarExpand,
    /// 折叠右侧栏（双箭头向右收拢）
    PaneCollapse,
    /// 展开（双箭头向左）
    PaneExpand,
}

/// 图标字重。Remix 只提供 `-line` 与 `-fill` 两种；light 与 regular 同源。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Weight {
    /// 常规（默认）
    #[default]
    Regular,
    /// 细线：比 regular 更轻，用于标题栏/输入框这类需要退后的位置
    Light,
    /// 实心：`MessageCircleFilled` 这类本来就只有 fill 的图标同义
    #[allow(dead_code)] // 菜单图标用各自的 fill 资产，这里只为公共 API 完整
    Fill,
}

impl Icon {
    /// 全部图标（测试与清单用；顺序与 `svg()` 无关）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [Icon; 55] = [
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
        Icon::SidebarExpand,
        Icon::PaneCollapse,
        Icon::PaneExpand,
    ];

    /// 图标对应的 SVG 原文（regular 字重）。
    pub fn svg(self) -> &'static [u8] {
        self.svg_at(Weight::Regular)
    }

    /// 按字重取 SVG。缺少 light 变体的图标（目前只有 `MessageCircleFilled`）
    /// 回落到 regular，保证调用方不用逐图标判断。
    pub fn svg_at(self, weight: Weight) -> &'static [u8] {
        match weight {
            Weight::Regular | Weight::Fill => self.regular_svg(),
            Weight::Light => self.light_svg(),
        }
    }

    fn regular_svg(self) -> &'static [u8] {
        match self {
            Icon::Search => include_bytes!("../assets/icons/ui/search-line.svg"),
            Icon::Plus => include_bytes!("../assets/icons/ui/add-line.svg"),
            Icon::Ellipsis => include_bytes!("../assets/icons/ui/more-line.svg"),
            Icon::Settings => include_bytes!("../assets/icons/ui/settings-3-line.svg"),
            Icon::X => include_bytes!("../assets/icons/ui/close-line.svg"),
            Icon::Check => include_bytes!("../assets/icons/ui/check-line.svg"),
            Icon::ArrowUp => include_bytes!("../assets/icons/ui/arrow-up-line.svg"),
            Icon::Stop => include_bytes!("../assets/icons/ui/stop-line.svg"),
            Icon::ChevronLeft => include_bytes!("../assets/icons/ui/arrow-left-s-line.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/ui/arrow-right-s-line.svg"),
            Icon::ChevronDown => include_bytes!("../assets/icons/ui/arrow-down-s-line.svg"),
            Icon::Paperclip => include_bytes!("../assets/icons/ui/attachment-line.svg"),
            Icon::Minus => include_bytes!("../assets/icons/ui/subtract-line.svg"),
            Icon::WindowMaximize => include_bytes!("../assets/icons/ui/checkbox-blank-line.svg"),
            Icon::WindowRestore => include_bytes!("../assets/icons/ui/file-copy-line.svg"),
            Icon::PanelRight => include_bytes!("../assets/icons/ui/layout-right-line.svg"),
            Icon::Hash => include_bytes!("../assets/icons/ui/hashtag.svg"),
            Icon::Pin => include_bytes!("../assets/icons/ui/pushpin-line.svg"),
            Icon::Star => include_bytes!("../assets/icons/ui/star-line.svg"),
            Icon::Crown => include_bytes!("../assets/icons/ui/vip-crown-line.svg"),
            Icon::Pencil => include_bytes!("../assets/icons/ui/edit-line.svg"),
            Icon::Trash => include_bytes!("../assets/icons/ui/delete-bin-line.svg"),
            Icon::EyeOff => include_bytes!("../assets/icons/ui/eye-off-line.svg"),
            Icon::Archive => include_bytes!("../assets/icons/ui/archive-line.svg"),
            Icon::Chart => include_bytes!("../assets/icons/ui/line-chart-line.svg"),
            Icon::Cpu => include_bytes!("../assets/icons/ui/cpu-line.svg"),
            Icon::Plug => include_bytes!("../assets/icons/ui/plug-line.svg"),
            Icon::Calendar => include_bytes!("../assets/icons/ui/calendar-line.svg"),
            Icon::Refresh => include_bytes!("../assets/icons/ui/refresh-line.svg"),
            Icon::Download => include_bytes!("../assets/icons/ui/download-line.svg"),
            Icon::MessageCircle => include_bytes!("../assets/icons/ui/chat-3-line.svg"),
            Icon::Bot => include_bytes!("../assets/icons/ui/robot-line.svg"),
            Icon::ListChecks => include_bytes!("../assets/icons/ui/list-check-2.svg"),
            Icon::Building => include_bytes!("../assets/icons/ui/building-line.svg"),
            Icon::Smile => include_bytes!("../assets/icons/ui/emotion-happy-line.svg"),
            Icon::Shield => include_bytes!("../assets/icons/ui/shield-check-line.svg"),
            Icon::Terminal => include_bytes!("../assets/icons/ui/terminal-box-line.svg"),
            Icon::Folder => include_bytes!("../assets/icons/ui/folder-line.svg"),
            Icon::Scissors => include_bytes!("../assets/icons/ui/scissors-line.svg"),
            Icon::Mic => include_bytes!("../assets/icons/ui/mic-line.svg"),
            Icon::BellOff => include_bytes!("../assets/icons/ui/notification-off-line.svg"),
            Icon::Key => include_bytes!("../assets/icons/ui/key-2-line.svg"),
            Icon::Info => include_bytes!("../assets/icons/ui/information-line.svg"),
            Icon::MessageSquare => include_bytes!("../assets/icons/ui/message-2-line.svg"),
            Icon::Sparkles => include_bytes!("../assets/icons/ui/sparkling-line.svg"),
            Icon::Contacts => include_bytes!("../assets/icons/ui/contacts-line.svg"),
            Icon::MessageCircleFilled => include_bytes!("../assets/icons/ui/chat-3-fill.svg"),
            Icon::Brain => include_bytes!("../assets/icons/ui/brain-line.svg"),
            Icon::Pushpin => include_bytes!("../assets/icons/ui/pushpin-line.svg"),
            Icon::Sun => include_bytes!("../assets/icons/ui/sun-line.svg"),
            Icon::Moon => include_bytes!("../assets/icons/ui/moon-line.svg"),
            Icon::Monitor => include_bytes!("../assets/icons/ui/computer-line.svg"),
            Icon::SidebarExpand => include_bytes!("../assets/icons/ui/layout-left-line.svg"),
            Icon::PaneCollapse => include_bytes!("../assets/icons/ui/arrow-right-double-line.svg"),
            Icon::PaneExpand => include_bytes!("../assets/icons/ui/arrow-left-double-line.svg"),
        }
    }

    /// 细线变体。Remix 没有 light 字重，`-line` 本身已是线框，
    /// 因此这里与 regular 相同；保留接口是为了不动调用方。
    fn light_svg(self) -> &'static [u8] {
        self.regular_svg()
    }
}


/// 画一个图标（regular 字重）。`size` 是边长，颜色由调用方给（`currentColor`）。
pub fn icon(kind: Icon, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg())
        .size(px(size))
        .text_color(Hsla::from(color))
}

/// 细线版。
///
/// 旧接口 `icon_with_stroke(.., stroke_width)` 靠替换 SVG 里的 `stroke-width` 字符串；
/// Remix 的 `-line` 是填充路径、同样没有这个属性，那种做法会静默失效。
/// 现在该函数与 [`icon`] 等价（Remix 没有 light 字重），保留只为不动调用方。
pub fn icon_light(kind: Icon, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg_at(Weight::Light))
        .size(px(size))
        .text_color(Hsla::from(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_well_formed_svg() {
        for icon in Icon::ALL {
            for weight in [Weight::Regular, Weight::Light] {
                let bytes = icon.svg_at(weight);
                assert!(bytes.starts_with(b"<svg"), "{icon:?}/{weight:?} 不是 svg");
                let text = std::str::from_utf8(bytes).unwrap();
                assert!(
                    text.contains("viewBox=\"0 0 24 24\""),
                    "{icon:?}/{weight:?} 缺 24 画布"
                );
                assert!(
                    text.contains("currentColor"),
                    "{icon:?}/{weight:?} 缺 currentColor"
                );
                assert!(text.trim_end().ends_with("</svg>"), "{icon:?}/{weight:?} 未闭合");
            }
        }
    }

    #[test]
    fn icon_set_matches_design_inventory() {
        assert_eq!(Icon::ALL.len(), 55);
    }

    #[test]
    fn light_weight_falls_back_to_regular() {
        // Remix 没有 light 字重：`-line` 本身就是线框，light 与 regular 同源。
        for icon in Icon::ALL {
            assert_eq!(
                icon.svg_at(Weight::Regular),
                icon.svg_at(Weight::Light),
                "{icon:?} 的 light 应与 regular 同源"
            );
        }
    }
}
