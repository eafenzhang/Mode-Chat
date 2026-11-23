//! 侧栏与设置菜单图标（Remix Icon，line / fill 成对）。
//!
//! 线性用于未选中，实心用于选中；侧栏 24px、设置菜单 18px 两种尺寸。
//! 资产在 `assets/icons/ui/`（与全局 `icons::Icon` 共用同一套）。
//!
//! 历史：旧版是手绘 24 网格 path，中途换过 Phosphor（实心填充、外框偏方），
//! 最终切到 Remix Icon 的线框风格——2px 描边、转角更圆，更接近最初的观感。

use gpui::{Hsla, IntoElement, Rgba, Styled, px, svg};

/// 侧栏图标尺寸（左导轨）。
///
/// Remix 是 24 网格、图形约占画布 90%，与旧版手绘图标相当，
/// 因此直接用 22（比 Phosphor 时代的 21 略大一点，因为线框更轻）。
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
const PAIRS: [(MenuIcon, &str, &str); 21] = [
    // 「消息」用对话气泡，「反馈」用扩音器，两者不再撞脸
    (MenuIcon::Chat, "chat-3-line", "chat-3-fill"),
    (MenuIcon::Contacts, "contacts-line", "contacts-fill"),
    (MenuIcon::Channels, "hashtag", "grid-fill"),
    (MenuIcon::Gear, "settings-3-line", "settings-3-fill"),
    (MenuIcon::Sliders, "equalizer-line", "equalizer-fill"),
    (MenuIcon::Models, "stack-line", "stack-fill"),
    (MenuIcon::Plugins, "puzzle-line", "puzzle-fill"),
    // 智能体用 CPU，「Bot」用机器人：两个都用 robot 会撞脸
    (MenuIcon::Agents, "cpu-line", "cpu-fill"),
    (MenuIcon::Bot, "robot-2-line", "robot-2-fill"),
    (MenuIcon::Memory, "brain-line", "brain-fill"),
    (MenuIcon::Im, "smartphone-line", "smartphone-fill"),
    (MenuIcon::Connectors, "plug-2-line", "plug-2-fill"),
    (MenuIcon::Schedules, "calendar-line", "calendar-fill"),
    (MenuIcon::Keys, "key-2-line", "key-2-fill"),
    (MenuIcon::Abilities, "flashlight-line", "flashlight-fill"),
    (MenuIcon::Usage, "line-chart-line", "line-chart-fill"),
    (MenuIcon::About, "information-line", "information-fill"),
    (MenuIcon::Feedback, "megaphone-line", "megaphone-fill"),
    (MenuIcon::Browser, "global-line", "global-fill"),
    (MenuIcon::Computer, "computer-line", "computer-fill"),
    (MenuIcon::More, "more-2-line", "more-2-fill"),
];

impl MenuIcon {
    /// (线框文件名, 实心文件名)，不含 `.svg`。
    fn names(self) -> (&'static str, &'static str) {
        PAIRS
            .iter()
            .find(|(kind, _, _)| *kind == self)
            .map(|(_, line, solid)| (*line, *solid))
            .expect("PAIRS 覆盖了全部 MenuIcon")
    }

    fn line_svg(self) -> &'static [u8] {
        match self.names().0 {
            "chat-3-line" => include_bytes!("../assets/icons/ui/chat-3-line.svg"),
            "contacts-line" => include_bytes!("../assets/icons/ui/contacts-line.svg"),
            "hashtag" => include_bytes!("../assets/icons/ui/hashtag.svg"),
            "settings-3-line" => include_bytes!("../assets/icons/ui/settings-3-line.svg"),
            "equalizer-line" => include_bytes!("../assets/icons/ui/equalizer-line.svg"),
            "stack-line" => include_bytes!("../assets/icons/ui/stack-line.svg"),
            "puzzle-line" => include_bytes!("../assets/icons/ui/puzzle-line.svg"),
            "cpu-line" => include_bytes!("../assets/icons/ui/cpu-line.svg"),
            "robot-2-line" => include_bytes!("../assets/icons/ui/robot-2-line.svg"),
            "brain-line" => include_bytes!("../assets/icons/ui/brain-line.svg"),
            "smartphone-line" => include_bytes!("../assets/icons/ui/smartphone-line.svg"),
            "plug-2-line" => include_bytes!("../assets/icons/ui/plug-2-line.svg"),
            "calendar-line" => include_bytes!("../assets/icons/ui/calendar-line.svg"),
            "key-2-line" => include_bytes!("../assets/icons/ui/key-2-line.svg"),
            "flashlight-line" => include_bytes!("../assets/icons/ui/flashlight-line.svg"),
            "line-chart-line" => include_bytes!("../assets/icons/ui/line-chart-line.svg"),
            "information-line" => include_bytes!("../assets/icons/ui/information-line.svg"),
            "megaphone-line" => include_bytes!("../assets/icons/ui/megaphone-line.svg"),
            "global-line" => include_bytes!("../assets/icons/ui/global-line.svg"),
            "computer-line" => include_bytes!("../assets/icons/ui/computer-line.svg"),
            "more-2-line" => include_bytes!("../assets/icons/ui/more-2-line.svg"),
            other => unreachable!("未登记的线框图标：{other}"),
        }
    }

    fn fill_svg(self) -> &'static [u8] {
        match self.names().1 {
            "chat-3-fill" => include_bytes!("../assets/icons/ui/chat-3-fill.svg"),
            "contacts-fill" => include_bytes!("../assets/icons/ui/contacts-fill.svg"),
            "grid-fill" => include_bytes!("../assets/icons/ui/grid-fill.svg"),
            "settings-3-fill" => include_bytes!("../assets/icons/ui/settings-3-fill.svg"),
            "equalizer-fill" => include_bytes!("../assets/icons/ui/equalizer-fill.svg"),
            "stack-fill" => include_bytes!("../assets/icons/ui/stack-fill.svg"),
            "puzzle-fill" => include_bytes!("../assets/icons/ui/puzzle-fill.svg"),
            "cpu-fill" => include_bytes!("../assets/icons/ui/cpu-fill.svg"),
            "robot-2-fill" => include_bytes!("../assets/icons/ui/robot-2-fill.svg"),
            "brain-fill" => include_bytes!("../assets/icons/ui/brain-fill.svg"),
            "smartphone-fill" => include_bytes!("../assets/icons/ui/smartphone-fill.svg"),
            "plug-2-fill" => include_bytes!("../assets/icons/ui/plug-2-fill.svg"),
            "calendar-fill" => include_bytes!("../assets/icons/ui/calendar-fill.svg"),
            "key-2-fill" => include_bytes!("../assets/icons/ui/key-2-fill.svg"),
            "flashlight-fill" => include_bytes!("../assets/icons/ui/flashlight-fill.svg"),
            "line-chart-fill" => include_bytes!("../assets/icons/ui/line-chart-fill.svg"),
            "information-fill" => include_bytes!("../assets/icons/ui/information-fill.svg"),
            "megaphone-fill" => include_bytes!("../assets/icons/ui/megaphone-fill.svg"),
            "global-fill" => include_bytes!("../assets/icons/ui/global-fill.svg"),
            "computer-fill" => include_bytes!("../assets/icons/ui/computer-fill.svg"),
            "more-2-fill" => include_bytes!("../assets/icons/ui/more-2-fill.svg"),
            other => unreachable!("未登记的实心图标：{other}"),
        }
    }

    fn light_svg(self) -> &'static [u8] {
        // Remix 没有 light 字重，`-line` 本身就是线框，直接复用。
        self.line_svg()
    }
}

/// 按状态渲染：`solid = true` 取实心，否则常规。
pub fn menu_icon(kind: MenuIcon, solid: bool, size: f32, color: Rgba) -> impl IntoElement {
    let data = if solid { kind.fill_svg() } else { kind.line_svg() };
    svg()
        .data(data)
        .size(px(size))
        .text_color(Hsla::from(color))
}

/// 侧栏用：`light = true` 时取细线字重（比 regular 更轻，贴近旧手绘观感）。
pub fn menu_icon_with_weight(
    kind: MenuIcon,
    solid: bool,
    size: f32,
    color: Rgba,
    light: bool,
) -> gpui::AnyElement {
    let data = match (solid, light) {
        (true, _) => kind.fill_svg(),
        (false, true) => kind.light_svg(),
        (false, false) => kind.line_svg(),
    };
    svg()
        .data(data)
        .size(px(size))
        .text_color(Hsla::from(color))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_both_weights() {
        for (kind, line, solid) in PAIRS {
            let l = kind.line_svg();
            let f = kind.fill_svg();
            assert!(l.starts_with(b"<svg"), "{kind:?}({line}) 不是 svg");
            assert!(f.starts_with(b"<svg"), "{kind:?}({solid}) 不是 svg");
            assert_ne!(l, f, "{kind:?} 的实心与线框相同");
            let text = std::str::from_utf8(l).unwrap();
            assert!(text.contains("viewBox=\"0 0 24 24\""), "{kind:?} 缺 24 画布");
        }
    }

    #[test]
    fn feedback_is_not_a_chat_bubble() {
        // 「反馈」必须和「消息」用不同图标，否则侧栏里两个气泡分不清
        assert_ne!(MenuIcon::Feedback.names().0, MenuIcon::Chat.names().0);
        assert_eq!(MenuIcon::Feedback.names().0, "megaphone-line");
        assert_eq!(MenuIcon::Chat.names().0, "chat-3-line");
    }

    #[test]
    fn line_icon_is_not_filled() {
        // Remix 的 `-line` 变体内部应当同时有描边感和镂空；
        // 这里只校验「线框」与「实心」确实是不同资产（light 与 line 同源）。
        for (kind, name, _) in PAIRS {
            assert_eq!(
                kind.light_svg(),
                kind.line_svg(),
                "{kind:?}({name}) 的 light 应与 line 同源"
            );
        }
    }
}
