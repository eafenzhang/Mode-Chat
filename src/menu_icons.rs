//! 侧栏与设置菜单图标（Phosphor，regular / fill 成对）。
//!
//! 线性用于未选中，实心用于选中；侧栏 24px、设置菜单 18px 两种尺寸。
//! 资产在 `assets/icons/phosphor/`（与全局 `icons::Icon` 共用同一套）。
//!
//! 旧版是手绘 24 网格 path（`assets/icons/menu/`），换 Phosphor 后线条与填色
//! 来自同族几何，选中态不会再“跳形”。

use gpui::{Hsla, IntoElement, Rgba, Styled, px, svg};

/// 侧栏图标尺寸（左导轨）。
///
/// 注意：Phosphor 的 256 viewBox 图形铺满整个画布（坐标跨 0..256），
/// 而换库前的手绘 24 网格图标只用 stroke 画在约 70% 的内框里。
/// 同样渲染 24px 时 Phosphor 的墨迹比旧图标大 20~25%，实测（1400x900
/// 窗口逐图标量墨迹包围盒）后把名义尺寸从 24 收到 21 才对得上。
pub const RAIL_SIZE: f32 = 21.;
/// 设置菜单图标尺寸。
pub const SETTINGS_SIZE: f32 = 16.;
/// 标题栏窗口控制按钮（最小化 / 最大化 / 关闭 / 置顶）的图标尺寸。
///
/// 同上：换库前名义 16px 的 stroke 图标，Phosphor 要 13px 才有相同观感。
pub const TITLEBAR_SIZE: f32 = 13.;

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

/// 每个菜单项一对 Phosphor 图标名：(线框, 实心)。
///
/// 成对是硬约束——取不到的实心会退回线框，选中态就没有视觉反馈了，
/// 所以这里只挑 Phosphor 确实同时提供 regular 与 fill 的图标。
const PAIRS: [(MenuIcon, &str, &str); 21] = [
    // 「消息」沿用对话气泡+点（反馈原先的图标），「反馈」改成扩音器，两者不再撞脸
    (MenuIcon::Chat, "chat-dots", "chat-dots-fill"),
    (MenuIcon::Contacts, "users", "users-fill"),
    (MenuIcon::Channels, "hash", "hash-fill"),
    (MenuIcon::Gear, "gear", "gear-fill"),
    (MenuIcon::Sliders, "faders", "faders-fill"),
    (MenuIcon::Models, "stack", "stack-fill"),
    (MenuIcon::Plugins, "puzzle-piece", "puzzle-piece-fill"),
    (MenuIcon::Agents, "circuitry", "circuitry-fill"),
    (MenuIcon::Bot, "robot", "robot-fill"),
    (MenuIcon::Memory, "brain", "brain-fill"),
    (MenuIcon::Im, "device-mobile", "device-mobile-fill"),
    (MenuIcon::Connectors, "plug", "plug-fill"),
    (MenuIcon::Schedules, "calendar-blank", "calendar-blank-fill"),
    (MenuIcon::Keys, "key", "key-fill"),
    (MenuIcon::Abilities, "lightning", "lightning-fill"),
    (MenuIcon::Usage, "chart-line", "chart-line-fill"),
    (MenuIcon::About, "info", "info-fill"),
    (MenuIcon::Feedback, "megaphone", "megaphone-fill"),
    (MenuIcon::Browser, "globe", "globe-fill"),
    (MenuIcon::Computer, "desktop", "desktop-fill"),
    (MenuIcon::More, "dots-three-vertical", "dots-three-vertical-fill"),
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
            "chat-dots" => include_bytes!("../assets/icons/phosphor/chat-dots.svg"),
            "users" => include_bytes!("../assets/icons/phosphor/users.svg"),
            "hash" => include_bytes!("../assets/icons/phosphor/hash.svg"),
            "gear" => include_bytes!("../assets/icons/phosphor/gear.svg"),
            "faders" => include_bytes!("../assets/icons/phosphor/faders.svg"),
            "stack" => include_bytes!("../assets/icons/phosphor/stack.svg"),
            "puzzle-piece" => include_bytes!("../assets/icons/phosphor/puzzle-piece.svg"),
            "robot" => include_bytes!("../assets/icons/phosphor/robot.svg"),
            "circuitry" => include_bytes!("../assets/icons/phosphor/circuitry.svg"),
            "plug" => include_bytes!("../assets/icons/phosphor/plug.svg"),
            "brain" => include_bytes!("../assets/icons/phosphor/brain.svg"),
            "device-mobile" => include_bytes!("../assets/icons/phosphor/device-mobile.svg"),
            "calendar-blank" => include_bytes!("../assets/icons/phosphor/calendar-blank.svg"),
            "key" => include_bytes!("../assets/icons/phosphor/key.svg"),
            "lightning" => include_bytes!("../assets/icons/phosphor/lightning.svg"),
            "chart-line" => include_bytes!("../assets/icons/phosphor/chart-line.svg"),
            "info" => include_bytes!("../assets/icons/phosphor/info.svg"),
            "megaphone" => include_bytes!("../assets/icons/phosphor/megaphone.svg"),
            "globe" => include_bytes!("../assets/icons/phosphor/globe.svg"),
            "desktop" => include_bytes!("../assets/icons/phosphor/desktop.svg"),
            "dots-three-vertical" => {
                include_bytes!("../assets/icons/phosphor/dots-three-vertical.svg")
            }
            other => panic!("未知的线框图标：{other}"),
        }
    }

    fn fill_svg(self) -> &'static [u8] {
        match self.names().1 {
            "chat-dots-fill" => include_bytes!("../assets/icons/phosphor/chat-dots-fill.svg"),
            "users-fill" => include_bytes!("../assets/icons/phosphor/users-fill.svg"),
            "hash-fill" => include_bytes!("../assets/icons/phosphor/hash-fill.svg"),
            "gear-fill" => include_bytes!("../assets/icons/phosphor/gear-fill.svg"),
            "faders-fill" => include_bytes!("../assets/icons/phosphor/faders-fill.svg"),
            "stack-fill" => include_bytes!("../assets/icons/phosphor/stack-fill.svg"),
            "puzzle-piece-fill" => {
                include_bytes!("../assets/icons/phosphor/puzzle-piece-fill.svg")
            }
            "robot-fill" => include_bytes!("../assets/icons/phosphor/robot-fill.svg"),
            "circuitry-fill" => include_bytes!("../assets/icons/phosphor/circuitry-fill.svg"),
            "plug-fill" => include_bytes!("../assets/icons/phosphor/plug-fill.svg"),
            "brain-fill" => include_bytes!("../assets/icons/phosphor/brain-fill.svg"),
            "device-mobile-fill" => {
                include_bytes!("../assets/icons/phosphor/device-mobile-fill.svg")
            }
            "calendar-blank-fill" => {
                include_bytes!("../assets/icons/phosphor/calendar-blank-fill.svg")
            }
            "key-fill" => include_bytes!("../assets/icons/phosphor/key-fill.svg"),
            "lightning-fill" => include_bytes!("../assets/icons/phosphor/lightning-fill.svg"),
            "chart-line-fill" => include_bytes!("../assets/icons/phosphor/chart-line-fill.svg"),
            "info-fill" => include_bytes!("../assets/icons/phosphor/info-fill.svg"),
            "megaphone-fill" => include_bytes!("../assets/icons/phosphor/megaphone-fill.svg"),
            "globe-fill" => include_bytes!("../assets/icons/phosphor/globe-fill.svg"),
            "desktop-fill" => include_bytes!("../assets/icons/phosphor/desktop-fill.svg"),
            "dots-three-vertical-fill" => {
                include_bytes!("../assets/icons/phosphor/dots-three-vertical-fill.svg")
            }
            other => panic!("未知的实心图标：{other}"),
        }
    }

    fn light_svg(self) -> &'static [u8] {
        match self.names().0 {
            "chat-dots" => include_bytes!("../assets/icons/phosphor/chat-dots-light.svg"),
            "users" => include_bytes!("../assets/icons/phosphor/users-light.svg"),
            "hash" => include_bytes!("../assets/icons/phosphor/hash-light.svg"),
            "gear" => include_bytes!("../assets/icons/phosphor/gear-light.svg"),
            "faders" => include_bytes!("../assets/icons/phosphor/faders-light.svg"),
            "stack" => include_bytes!("../assets/icons/phosphor/stack-light.svg"),
            "puzzle-piece" => include_bytes!("../assets/icons/phosphor/puzzle-piece-light.svg"),
            "robot" => include_bytes!("../assets/icons/phosphor/robot-light.svg"),
            "circuitry" => include_bytes!("../assets/icons/phosphor/circuitry-light.svg"),
            "plug" => include_bytes!("../assets/icons/phosphor/plug-light.svg"),
            "brain" => include_bytes!("../assets/icons/phosphor/brain-light.svg"),
            "device-mobile" => include_bytes!("../assets/icons/phosphor/device-mobile-light.svg"),
            "calendar-blank" => include_bytes!("../assets/icons/phosphor/calendar-blank-light.svg"),
            "key" => include_bytes!("../assets/icons/phosphor/key-light.svg"),
            "lightning" => include_bytes!("../assets/icons/phosphor/lightning-light.svg"),
            "chart-line" => include_bytes!("../assets/icons/phosphor/chart-line-light.svg"),
            "info" => include_bytes!("../assets/icons/phosphor/info-light.svg"),
            "megaphone" => include_bytes!("../assets/icons/phosphor/megaphone-light.svg"),
            "globe" => include_bytes!("../assets/icons/phosphor/globe-light.svg"),
            "desktop" => include_bytes!("../assets/icons/phosphor/desktop-light.svg"),
            "dots-three-vertical" => {
                include_bytes!("../assets/icons/phosphor/dots-three-vertical-light.svg")
            }
            other => panic!("未知的线框图标：{other}"),
        }
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
            assert!(text.contains("viewBox=\"0 0 256 256\""), "{kind:?} 缺 256 画布");
        }
    }

    #[test]
    fn feedback_is_not_a_chat_bubble() {
        // 「反馈」必须和「消息」用不同图标，否则侧栏里两个气泡分不清
        assert_ne!(MenuIcon::Feedback.names().0, MenuIcon::Chat.names().0);
        assert_eq!(MenuIcon::Feedback.names().0, "megaphone");
        assert_eq!(MenuIcon::Chat.names().0, "chat-dots");
    }

    #[test]
    fn light_weight_differs_from_line() {
        for (kind, name, _) in PAIRS {
            assert_ne!(
                kind.line_svg(),
                kind.light_svg(),
                "{kind:?}({name}) 的 light 与 regular 相同"
            );
        }
    }
}
