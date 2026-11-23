//! 统一 UI 组件层。
//!
//! 规范见 `docs/ui-design-spec.md`。页面只允许调用本模块，不允许自己写
//! `text_size` / `rounded` / `gap` 这种裸样式——审查时发现设置页有 141 处
//! 裸 `rounded`、93 处裸 `cursor_pointer`，同样的按钮在不同页面长得不一样。
//!
//! 双主题（Bot 深色 / 企业微信）在这里统一分支，调用方不判断皮肤。

// 规范完整、接入分批：尚未被页面使用的条目先压掉 dead_code 噪音。
#![allow(dead_code)]

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, Div, Hsla, IntoElement, ParentElement, Rgba, SharedString, Stateful,
};

use gpui::{App, Context, Entity, Window};

use crate::i18n;
use crate::simple_input::SimpleInput;
use crate::theme;
use crate::wecom_theme;

// ---- 间距（4px 基准，见规范 §2）----
pub const S1: f32 = 4.;
pub const S2: f32 = 8.;
pub const S3: f32 = 12.;
pub const S4: f32 = 16.;
pub const S5: f32 = 24.;
pub const S6: f32 = 32.;

// ---- 字号（四级，见规范 §3）----
pub const T_LABEL: f32 = 11.;
pub const T_LABEL_LH: f32 = 16.;
pub const T_BODY: f32 = 12.;
pub const T_BODY_LH: f32 = 18.;
pub const T_TITLE: f32 = 13.;
pub const T_TITLE_LH: f32 = 18.;
pub const T_PAGE: f32 = 15.;
pub const T_PAGE_LH: f32 = 24.;
pub const T_METRIC: f32 = 22.;
pub const T_METRIC_LH: f32 = 28.;

/// 卡片圆角（规范 §4：`R_Card`）。
pub const R_CARD: f32 = 12.;

fn wechat() -> bool {
    wecom_theme::active()
}

fn font() -> &'static str {
    if wechat() {
        wecom_theme::FONT_FAMILY
    } else {
        theme::FONT_FAMILY
    }
}

// ---- 语义色（规范 §5）----
pub fn ink() -> Rgba {
    if wechat() {
        wecom_theme::chat_title()
    } else {
        theme::ink()
    }
}

pub fn ink_secondary() -> Rgba {
    if wechat() {
        wecom_theme::list_preview()
    } else {
        theme::ink_secondary()
    }
}

pub fn ink_tertiary() -> Rgba {
    if wechat() {
        wecom_theme::time_label()
    } else {
        theme::ink_tertiary()
    }
}

pub fn edge() -> Rgba {
    if wechat() {
        wecom_theme::panel_divider()
    } else {
        theme::edge()
    }
}

pub fn describe_edge() -> Rgba {
    if wechat() {
        wecom_theme::composer_border()
    } else {
        theme::edge()
    }
}

pub fn card_bg() -> Rgba {
    if wechat() {
        wecom_theme::panel_bg()
    } else {
        theme::card()
    }
}

pub fn accent() -> Rgba {
    if wechat() {
        wecom_theme::send_enabled()
    } else {
        theme::accent()
    }
}

// ---- 文字 ----

/// 辅助文字：11/16，次级色。
pub fn label(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(font())
        .text_size(px(T_LABEL))
        .line_height(px(T_LABEL_LH))
        .text_color(ink_secondary())
        .child(text.into())
}

/// 正文：12/18，主色。
pub fn body(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(font())
        .text_size(px(T_BODY))
        .line_height(px(T_BODY_LH))
        .text_color(ink())
        .child(text.into())
}

/// 弱化正文：12/18，次级色。
pub fn body_muted(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(font())
        .text_size(px(T_BODY))
        .line_height(px(T_BODY_LH))
        .text_color(ink_secondary())
        .child(text.into())
}

/// 卡片/分区标题：13/18，600。
pub fn title(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(font())
        .text_size(px(T_TITLE))
        .line_height(px(T_TITLE_LH))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(ink())
        .child(text.into())
}

/// 数字看板：22/28，600。
pub fn metric(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(font())
        .text_size(px(T_METRIC))
        .line_height(px(T_METRIC_LH))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(ink())
        .child(text.into())
}

// ---- 容器 ----

/// 页面骨架：纵向排布，区块间距 `S4`。
pub fn page() -> Div {
    div().w_full().flex().flex_col().gap(px(S4))
}

/// 分区标题（带上方呼吸位）。
pub fn section(title_text: impl Into<SharedString>) -> Div {
    div().w_full().pt(px(S3)).child(title(title_text))
}

/// 分区标题 + 右侧动作。
pub fn section_action(title_text: impl Into<SharedString>, action: impl IntoElement) -> Div {
    div()
        .w_full()
        .pt(px(S3))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(S2))
        .child(title(title_text))
        .child(action.into_any_element())
}

/// 卡片：`R_Card` + `edge` 边框 + `card` 底色。
pub fn card(children: impl IntoIterator<Item = AnyElement>) -> Div {
    let mut el = div()
        .w_full()
        .rounded(px(R_CARD))
        .border_1()
        .border_color(edge())
        .bg(card_bg())
        .overflow_hidden()
        .flex()
        .flex_col();
    for child in children {
        el = el.child(child);
    }
    el
}

/// 卡片内的一行（行间 0.5px 分隔）。`last` 为真时不画底线。
pub fn card_row(content: impl IntoElement, last: bool) -> Div {
    let mut row = div().w_full();
    if !last {
        row = row
            .border_b(px(0.5))
            .border_color(if wechat() {
                wecom_theme::divider()
            } else {
                theme::edge()
            });
    }
    row.child(content)
}

/// 标准列表行：左标题+副标题，右侧挂动作。
pub fn list_row(row_title: impl Into<SharedString>, detail: Option<SharedString>) -> Div {
    let mut text = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_BODY))
                .line_height(px(T_BODY_LH))
                .text_color(ink())
                .truncate()
                .child(row_title.into()),
        );
    if let Some(detail) = detail {
        text = text.child(
            div()
                .font_family(font())
                .text_size(px(T_LABEL))
                .line_height(px(T_LABEL_LH))
                .text_color(ink_tertiary())
                .truncate()
                .child(detail),
        );
    }
    div()
        .w_full()
        .min_h(px(44.))
        .px(px(S3))
        .py(px(S2))
        .flex()
        .items_center()
        .gap(px(S3))
        .child(text)
}

/// 带前置图标/头像的列表行。
pub fn list_row_leading(
    leading: impl IntoElement,
    row_title: impl Into<SharedString>,
    detail: Option<SharedString>,
) -> Div {
    list_row(row_title, detail).child(leading.into_any_element())
}

/// 键值行：左标签 11px，右值 12px。
pub fn stat_row(label_text: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .min_h(px(36.))
        .px(px(S3))
        .py(px(S2))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(S3))
        .child(label(label_text))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_BODY))
                .line_height(px(T_BODY_LH))
                .text_color(ink())
                .child(value.into()),
        )
}

// ---- 卡片样式（规范 §6.3）----

/// 图标徽标：32 方，圆角 `R_Field`，浅底。
pub fn icon_tile(icon: impl IntoElement) -> Div {
    div()
        .size(px(32.))
        .flex_shrink_0()
        .rounded(px(theme::RADIUS_FIELD))
        .bg(if wechat() {
            wecom_theme::chip_bg()
        } else {
            theme::hover()
        })
        .flex()
        .items_center()
        .justify_center()
        .child(icon.into_element())
}

/// 卡片式条目：图标 + 标题 + 说明 +（可选）右上角徽标。
///
/// 模型 / 插件 / 智能体 / 连接器四页共用这一种形态。
pub fn icon_card(
    icon: impl IntoElement,
    card_title: impl Into<SharedString>,
    detail: Option<SharedString>,
    badge: Option<AnyElement>,
) -> Div {
    let mut head = div()
        .flex()
        .items_center()
        .gap(px(S2))
        .child(icon_tile(icon))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(title(card_title)),
        );
    if let Some(badge) = badge {
        head = head.child(badge);
    }
    let mut el = div()
        .w_full()
        .rounded(px(R_CARD))
        .border_1()
        .border_color(describe_edge())
        .bg(card_bg())
        .p(px(S3))
        .flex()
        .flex_col()
        .gap(px(S2))
        .child(head);
    if let Some(detail) = detail {
        el = el.child(
            div()
                .font_family(font())
                .text_size(px(T_LABEL))
                .line_height(px(T_LABEL_LH))
                .text_color(ink_tertiary())
                .child(detail),
        );
    }
    el
}

/// 数字看板卡：大数字 + 小标签。
pub fn stat_card(value: impl Into<SharedString>, label_text: impl Into<SharedString>) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .rounded(px(R_CARD))
        .border_1()
        .border_color(edge())
        .bg(card_bg())
        .px(px(S3))
        .py(px(S3))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(metric(value))
        .child(label(label_text))
}

// ---- 控件 ----

/// 文本按钮：30 高、胶囊、`hover` 底。
pub fn button(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.))
        .rounded_full()
        .bg(if wechat() {
            wecom_theme::chip_bg()
        } else {
            theme::hover()
        })
        .px(px(S3))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(theme::raised_hover()))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_BODY))
                .line_height(px(T_BODY_LH))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink())
                .child(text.into()),
        )
}

/// 轻量文本按钮：无底色，只换文字色（卡片内的次级动作）。
pub fn button_ghost(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(22.))
        .px(px(S1))
        .flex()
        .items_center()
        .cursor_pointer()
        .font_family(font())
        .text_size(px(T_LABEL))
        .line_height(px(T_LABEL_LH))
        .text_color(ink_secondary())
        .hover(|style| style.text_color(accent()))
        .child(text.into())
}

/// 筛选 chip：胶囊，选中用强调色底。
pub fn chip(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .rounded_full()
        .border_1()
        .border_color(if active { accent() } else { edge() })
        .bg(if active {
            if wechat() {
                wecom_theme::chip_bg()
            } else {
                theme::accent_soft()
            }
        } else {
            Rgba {
                r: 0.,
                g: 0.,
                b: 0.,
                a: 0.,
            }
        })
        .px(px(S3))
        .flex()
        .items_center()
        .cursor_pointer()
        .when(!active, |el| el.hover(|style| style.bg(theme::hover())))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_LABEL))
                .line_height(px(T_LABEL_LH))
                .text_color(if active { accent() } else { ink_secondary() })
                .child(text.into()),
        )
}

/// 徽标色调。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Success,
    Danger,
    Accent,
}

fn tone_colors(tone: Tone) -> (Rgba, Rgba) {
    match tone {
        Tone::Neutral => (ink_tertiary(), theme::hover()),
        Tone::Success => (theme::success(), theme::success_soft()),
        Tone::Danger => (theme::danger(), theme::danger_soft()),
        Tone::Accent => (accent(), theme::accent_soft()),
    }
}

/// 徽标：`R_Tag`，2px 8px 内边距。
pub fn badge(text: impl Into<SharedString>, tone: Tone) -> Div {
    let (fg, bg) = tone_colors(tone);
    div()
        .rounded(px(theme::RADIUS_TAG))
        .bg(bg)
        .px(px(S2))
        .py(px(1.))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_LABEL))
                .line_height(px(T_LABEL_LH))
                .text_color(fg)
                .child(text.into()),
        )
}

// ---- 状态 ----

/// 空态：居中一行提示。
pub fn empty(text: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .rounded(px(R_CARD))
        .border_1()
        .border_color(edge())
        .bg(card_bg())
        .px(px(S3))
        .py(px(S4))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .font_family(font())
                .text_size(px(T_LABEL))
                .line_height(px(T_LABEL_LH))
                .text_color(ink_tertiary())
                .child(text.into()),
        )
}

/// 提示条。
pub fn note(text: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .font_family(font())
        .text_size(px(T_BODY))
        .line_height(px(T_BODY_LH))
        .text_color(ink_secondary())
        .child(text.into())
}

/// 错误条：`danger_soft` 底 + `R_Field`。
pub fn error(text: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .rounded(px(theme::RADIUS_FIELD))
        .bg(theme::danger_soft())
        .px(px(S3))
        .py(px(S2))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_BODY))
                .line_height(px(T_BODY_LH))
                .text_color(theme::danger())
                .child(text.into()),
        )
}

/// 只读值块（路径、诊断信息）。
pub fn value_box(value: impl Into<SharedString>, label_text: Option<SharedString>) -> Div {
    let mut el = div()
        .w_full()
        .rounded(px(theme::RADIUS_FIELD))
        .border_1()
        .border_color(describe_edge())
        .bg(if wechat() {
            wecom_theme::chip_bg()
        } else {
            theme::card()
        })
        .px(px(S3))
        .py(px(S2))
        .flex()
        .flex_col()
        .gap(px(2.));
    if let Some(label_text) = label_text {
        el = el.child(label(label_text));
    }
    el.child(
        div()
            .font_family(font())
            .text_size(px(T_BODY))
            .line_height(px(T_BODY_LH))
            .text_color(ink_secondary())
            .child(value.into()),
    )
}

/// 开关：36×20。
pub fn switch(id: impl Into<gpui::ElementId>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(36.))
        .h(px(20.))
        .rounded_full()
        .bg(if on {
            accent()
        } else if wechat() {
            wecom_theme::panel_divider()
        } else {
            theme::hover()
        })
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            div()
                .size(px(16.))
                .rounded_full()
                .bg(gpui::white())
                .ml(px(if on { 18. } else { 2. })),
        )
}

/// 分段控件（外观 / 布局这类二选一）。
pub fn segment(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.))
        .rounded(px(theme::RADIUS_SEGMENT))
        .bg(if active {
            if wechat() {
                wecom_theme::send_enabled()
            } else {
                theme::accent()
            }
        } else {
            Rgba {
                r: 0.,
                g: 0.,
                b: 0.,
                a: 0.,
            }
        })
        .px(px(S3))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(!active, |el| el.hover(|style| style.bg(theme::hover())))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_BODY))
                .line_height(px(T_BODY_LH))
                .text_color(if active {
                    if wechat() {
                        wecom_theme::nav_text_active()
                    } else {
                        theme::accent_ink()
                    }
                } else {
                    ink_secondary()
                })
                .child(text.into()),
        )
}

/// 把 `Rgba` 转成 `Hsla`（给需要直接设文字色的地方用）。
pub fn hsla(color: Rgba) -> Hsla {
    Hsla::from(color)
}

// ---- 侧栏详情面板（规范 §7.2 / §8）------------------------------------------

/// 详情栏宽度（设置页内部与会话右栏共用同一档）。
pub const PANEL_W: f32 = 340.;

/// 侧栏详情面板的骨架：标题行（返回 + 标题 + 关闭）+ 可滚动内容 + 底部动作。
///
/// 多级页面用它替代弹窗：一级页在左侧保持不动，二级内容从右侧滑入。
/// 破坏性确认（删除）仍然用 `open_alert_dialog`，规则见规范 §8。
pub struct DetailPanel {
    title: SharedString,
    subtitle: Option<SharedString>,
    body: Vec<AnyElement>,
    actions: Vec<AnyElement>,
}


/// 详情面板右上角的关闭按钮（点击交给面板自己处理）。
fn ui_close_button() -> Stateful<Div> {
    div()
        .id("detail-close-x")
        .size(px(22.))
        .flex_shrink_0()
        .rounded(px(theme::RADIUS_CHIP))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(theme::hover()))
        .child(crate::icons::icon(
            crate::icons::Icon::X,
            14.,
            ink_tertiary(),
        ))
}

impl DetailPanel {
    pub fn new(caption: impl Into<SharedString>) -> Self {
        Self {
            title: caption.into(),
            subtitle: None,
            body: Vec::new(),
            actions: Vec::new(),
        }
    }

    pub fn subtitle(mut self, text: impl Into<SharedString>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    pub fn body(mut self, element: impl IntoElement) -> Self {
        self.body.push(element.into_any_element());
        self
    }

    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }

    /// 渲染成右侧栏（宽度 `PANEL_W`，左侧 1px 分隔）。
    pub fn render(
        self,
        on_back: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<crate::PilotApp>,
    ) -> AnyElement {
        // render(self) 拿走所有权后，先把字段取成局部变量：
        // 否则 `title` 会解析到模块级 fn title()。
        let caption = self.title.clone();
        let subtitle = self.subtitle.clone();
        let close_button = ui_close_button();
        let head = div()
            .flex_shrink_0()
            .px(px(S3))
            .py(px(S2))
            .flex()
            .items_center()
            .gap(px(S2))
            .border_b_1()
            .border_color(edge())
            .child(
                button_ghost("detail-back", i18n::t("返回"))
                    .on_click(cx.listener(move |_this, _event, window, cx| {
                        on_back(window, cx);
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .font_family(font())
                    .text_size(px(T_TITLE))
                    .line_height(px(T_TITLE_LH))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(ink())
                    .truncate()
                    .child(caption),
            )
            .child(close_button);

        let mut body = div()
            .id("detail-panel-body")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .scrollbar_width(px(6.))
            .px(px(S3))
            .py(px(S3))
            .flex()
            .flex_col()
            .gap(px(S3));
        if let Some(subtitle) = subtitle {
            body = body.child(
                div()
                    .font_family(font())
                    .text_size(px(T_LABEL))
                    .line_height(px(T_LABEL_LH))
                    .text_color(ink_tertiary())
                    .child(subtitle),
            );
        }
        for element in self.body {
            body = body.child(element);
        }

        // 绝对定位贴右侧：设置卡宽度固定，塞第三列会把主内容压没。
        let mut panel = div()
            .id("detail-panel")
            .absolute()
            .top(px(0.))
            .right(px(0.))
            .bottom(px(0.))
            .w(px(PANEL_W))
            .border_l_1()
            .border_color(edge())
            .bg(card_bg())
            .shadow(theme::shadow_3())
            .flex()
            .flex_col()
            .child(head);
        panel = panel.child(body);
        if !self.actions.is_empty() {
            let mut actions = div()
                .flex_shrink_0()
                .px(px(S3))
                .py(px(S2))
                .flex()
                .items_center()
                .justify_end()
                .gap(px(S2))
                .border_t_1()
                .border_color(edge());
            for action in self.actions {
                actions = actions.child(action);
            }
            panel = panel.child(actions);
        }
        panel.into_any_element()
    }
}

/// 表单字段：标签 + 输入框（详情栏内用）。
pub fn form_field(id: &'static str, label_text: &'static str, input: &Entity<SimpleInput>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(S1))
        .child(
            div()
                .font_family(font())
                .text_size(px(T_LABEL))
                .line_height(px(T_LABEL_LH))
                .text_color(ink_secondary())
                .child(i18n::t(label_text)),
        )
        .child(div().id(id).h(px(34.)).child(input.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_is_multiples_of_four() {
        for value in [S1, S2, S3, S4, S5, S6] {
            assert_eq!(value % 4., 0., "{value} 不是 4 的倍数");
        }
    }

    #[test]
    fn type_scale_is_ordered_and_five_levels() {
        let sizes = [T_LABEL, T_BODY, T_TITLE, T_PAGE, T_METRIC];
        for pair in sizes.windows(2) {
            assert!(pair[0] < pair[1], "字号必须递增：{pair:?}");
        }
        assert_eq!(sizes.len(), 5);
    }

    #[test]
    fn line_height_exceeds_font_size() {
        // 行高必须比字号大，否则中文字会被裁
        for (size, lh) in [
            (T_LABEL, T_LABEL_LH),
            (T_BODY, T_BODY_LH),
            (T_TITLE, T_TITLE_LH),
            (T_PAGE, T_PAGE_LH),
            (T_METRIC, T_METRIC_LH),
        ] {
            assert!(lh > size, "{size}/{lh} 行高不足");
        }
    }

    #[test]
    fn card_radius_matches_spec() {
        assert_eq!(R_CARD, crate::theme::RADIUS_MENU);
        assert_eq!(T_TITLE, 13.);
    }
}
