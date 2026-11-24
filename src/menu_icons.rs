//! 侧栏与设置菜单图标（Lucide，纯线框）。
//!
//! 只有一种线框风格：选中态靠调用方传的 `color` 区分（accent 色），
//! 不再像 Boxicons 那样切换线框/实心字形。
//! 资产在 `assets/icons/ui/`（与全局 `icons::Icon` 共用同一套）。
//!
//! 历史：手绘 24 网格 → Phosphor（实心、方）→ Remix（线框、圆）→
//! Boxicons（圆润、成对）→ **Lucide**（纯线框、最圆润）。
//! Lucide 圆润度实测 8%，复杂度 39 命令，ISC 许可。

use gpui::{Hsla, IntoElement, Rgba, Styled, px, svg};

/// 侧栏图标尺寸（左导轨）。
///
/// Boxicons 24 网格，线框笔画约 2px，21px 下与 Remix 时代观感相当。
pub const RAIL_SIZE: f32 = 22.;
/// 设置菜单图标尺寸。
pub const SETTINGS_SIZE: f32 = 16.;
/// 标题栏窗口控制按钮（最小化 / 最大化 / 关闭 / 置顶）的图标尺寸。
pub const TITLEBAR_SIZE: f32 = 14.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuIcon {
    Chat,
    Contacts,
    Channels,
    Gear,
    Sliders,
    Models,
    Plugins,
    Agents,
    Bot,
    /// 「Bot团队」页：人群轮廓（管 Bot + 群聊 + 分组）。
    Team,
    Memory,
    Im,
    Connectors,
    Schedules,
    Keys,
    Abilities,
    Usage,
    About,
    Feedback,
    Browser,
    Computer,
    More,
}

/// 每个菜单项一对 Remix 图标名：(线框, 实心)。
///
/// 成对是硬约束——取不到的实心会退回线框，选中态就没有视觉反馈了，
/// 所以这里只挑 Remix 确实同时提供 `-line` 与 `-fill` 的图标。
/// 每个菜单项对应的 Lucide 文件名（不含 `.svg`）。
///
/// 只有一个名字：Lucide 是纯线框库，没有实心变体。选中态由调用方
/// 用 `text_color` 表达（accent 色），见 `menu_icon` / `menu_icon_with_weight`。
const NAMES: [(MenuIcon, &str); 22] = [
    (MenuIcon::Chat, "message-circle"),
    (MenuIcon::Contacts, "users"),
    (MenuIcon::Channels, "hash"),
    (MenuIcon::Gear, "settings"),
    (MenuIcon::Sliders, "sliders-horizontal"),
    (MenuIcon::Models, "layers"),
    (MenuIcon::Plugins, "puzzle"),
    // 智能体用 CPU，「Bot」用机器人：两个都用 bot 会撞脸
    (MenuIcon::Agents, "cpu"),
    (MenuIcon::Bot, "bot"),
    (MenuIcon::Team, "users-round"),
    (MenuIcon::Memory, "brain"),
    (MenuIcon::Im, "smartphone"),
    (MenuIcon::Connectors, "plug"),
    (MenuIcon::Schedules, "calendar"),
    (MenuIcon::Keys, "key"),
    (MenuIcon::Abilities, "zap"),
    (MenuIcon::Usage, "chart-column"),
    (MenuIcon::About, "info"),
    (MenuIcon::Feedback, "megaphone"),
    (MenuIcon::Browser, "globe"),
    (MenuIcon::Computer, "monitor"),
    (MenuIcon::More, "ellipsis"),
];

impl MenuIcon {
    /// Lucide 文件名，不含 `.svg`。
    fn name(self) -> &'static str {
        NAMES
            .iter()
            .find(|(kind, _)| *kind == self)
            .map(|(_, name)| *name)
            .expect("NAMES 覆盖了全部 MenuIcon")
    }

    /// 读资产。Lucide 只有一个风格，线框和「实心」返回同一份。
    fn svg_bytes(self) -> &'static [u8] {
        match self.name() {
            "bot" => include_bytes!("../assets/icons/ui/bot.svg"),
            "brain" => include_bytes!("../assets/icons/ui/brain.svg"),
            "calendar" => include_bytes!("../assets/icons/ui/calendar.svg"),
            "chart-column" => include_bytes!("../assets/icons/ui/chart-column.svg"),
            "cpu" => include_bytes!("../assets/icons/ui/cpu.svg"),
            "ellipsis" => include_bytes!("../assets/icons/ui/ellipsis.svg"),
            "globe" => include_bytes!("../assets/icons/ui/globe.svg"),
            "hash" => include_bytes!("../assets/icons/ui/hash.svg"),
            "info" => include_bytes!("../assets/icons/ui/info.svg"),
            "key" => include_bytes!("../assets/icons/ui/key.svg"),
            "layers" => include_bytes!("../assets/icons/ui/layers.svg"),
            "megaphone" => include_bytes!("../assets/icons/ui/megaphone.svg"),
            "message-circle" => include_bytes!("../assets/icons/ui/message-circle.svg"),
            "monitor" => include_bytes!("../assets/icons/ui/monitor.svg"),
            "plug" => include_bytes!("../assets/icons/ui/plug.svg"),
            "puzzle" => include_bytes!("../assets/icons/ui/puzzle.svg"),
            "settings" => include_bytes!("../assets/icons/ui/settings.svg"),
            "sliders-horizontal" => include_bytes!("../assets/icons/ui/sliders-horizontal.svg"),
            "smartphone" => include_bytes!("../assets/icons/ui/smartphone.svg"),
            "users" => include_bytes!("../assets/icons/ui/users.svg"),
            "users-round" => include_bytes!("../assets/icons/ui/users-round.svg"),
            "zap" => include_bytes!("../assets/icons/ui/zap.svg"),
            other => unreachable!("未登记的图标：{other}"),
        }
    }
}

/// 按状态渲染。
///
/// `solid` 参数保留是为了不动调用方签名；Lucide 没有实心变体，
/// 两种取值返回同一份资产——选中态由 `color` 决定。
pub fn menu_icon(kind: MenuIcon, _solid: bool, size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(kind.svg_bytes())
        .size(px(size))
        .text_color(Hsla::from(color))
}

/// 侧栏用：`light` 参数保留，Lucide 只有一档字重。
pub fn menu_icon_with_weight(
    kind: MenuIcon,
    _solid: bool,
    size: f32,
    color: Rgba,
    _light: bool,
) -> gpui::AnyElement {
    svg()
        .data(kind.svg_bytes())
        .size(px(size))
        .text_color(Hsla::from(color))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_asset() {
        for (kind, name) in NAMES {
            let bytes = kind.svg_bytes();
            assert!(bytes.starts_with(b"<svg"), "{kind:?}({name}) 不是 svg");
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.contains("viewBox=\"0 0 24 24\""), "{kind:?} 缺 24 画布");
        }
    }

    /// Lucide 是纯线框库：所有资产的路径都靠 `stroke` 描边，不填充。
    /// 这条断言防止以后误把面型图标混进来。
    #[test]
    fn every_icon_is_stroked_not_filled() {
        for (kind, name) in NAMES {
            let text = std::str::from_utf8(kind.svg_bytes()).unwrap();
            assert!(
                text.contains("stroke=\"currentColor\""),
                "{kind:?}({name}) 不是描边图标"
            );
            assert!(
                text.contains("fill=\"none\""),
                "{kind:?}({name}) 有填充，不符合纯线框"
            );
        }
    }

    #[test]
    fn feedback_is_not_a_chat_bubble() {
        // 「反馈」必须和「消息」用不同图标，否则侧栏里两个气泡分不清
        assert_ne!(MenuIcon::Feedback.name(), MenuIcon::Chat.name());
        assert_eq!(MenuIcon::Feedback.name(), "megaphone");
        assert_eq!(MenuIcon::Chat.name(), "message-circle");
    }

    #[test]
    fn agents_and_bot_do_not_collide() {
        // 「智能体」和「Bot」是两个菜单项，字形必须不同
        assert_ne!(MenuIcon::Agents.name(), MenuIcon::Bot.name());
        assert_eq!(MenuIcon::Agents.name(), "cpu");
        assert_eq!(MenuIcon::Bot.name(), "bot");
    }
}
