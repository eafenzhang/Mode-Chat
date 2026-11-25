//! Composer 多行输入：gpui-kit `Textarea` 的适配层。
//!
//! 取代自绘 `text_input.rs`：Enter 提交（`submit_on_enter`），Shift+Enter 换行，
//! 通过 `InputEvent::PressEnter` 派发 `Send`，IME/选区/软折行由组件负责。

use gpui::component::Sizable;
use gpui::component::input::{InputEvent, Textarea, TextareaState};
use gpui::prelude::*;
use gpui::{
    App, Entity, FocusHandle, Focusable, IntoElement, MouseButton, Render, SharedString,
    Subscription, Window, div, px,
};

use crate::{theme, ui_kit, wechat_theme};

gpui::actions!(composer, [Send, SendSecondary]);

pub struct ComposerInput {
    state: Option<Entity<TextareaState>>,
    content: SharedString,
    pending: Option<SharedString>,
    placeholder: SharedString,
    local_focus: FocusHandle,
    /// 回车发送（false = Enter 换行、Ctrl+Enter 发送，prefs.json）。
    submit_on_enter: bool,
    /// 已同步到组件 state 的值（偏好切换后下一帧下发）。
    applied_submit_on_enter: Option<bool>,
    _subscription: Option<Subscription>,
}

impl ComposerInput {
    pub fn new(
        placeholder: impl Into<SharedString>,
        _placeholder_color: fn() -> gpui::Rgba,
        submit_on_enter: bool,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        Self {
            state: None,
            content: "".into(),
            pending: None,
            placeholder: placeholder.into(),
            local_focus: cx.focus_handle(),
            submit_on_enter,
            applied_submit_on_enter: None,
            _subscription: None,
        }
    }

    pub fn set_submit_on_enter(
        &mut self,
        submit: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.submit_on_enter == submit {
            return;
        }
        self.submit_on_enter = submit;
        cx.notify();
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn set_content(&mut self, text: impl Into<SharedString>, cx: &mut gpui::Context<Self>) {
        let text = text.into();
        self.content = text.clone();
        self.pending = Some(text);
        cx.notify();
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.content = "".into();
        self.pending = None;
        if let Some(state) = self.state.clone() {
            state.update(cx, |state, cx| state.set_value("", window, cx));
        }
        cx.notify();
    }

    /// 组合态由 gpui-kit 内部处理；保留旧 API 供 `on_send` 调用。
    pub fn is_composing(&self) -> bool {
        false
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state
            .as_ref()
            .map(|state| state.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.local_focus.clone())
    }
}

impl Focusable for ComposerInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        ComposerInput::focus_handle(self, cx)
    }
}

impl Render for ComposerInput {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if self.state.is_none() {
            let had_focus = self.local_focus.is_focused(window);
            let placeholder = self.placeholder.clone();
            let submit_on_enter = self.submit_on_enter;
            let state = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .submit_on_enter(submit_on_enter)
                    .placeholder(placeholder)
            });
            self.applied_submit_on_enter = Some(submit_on_enter);
            let subscription = cx.subscribe_in(
                &state,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.content = state.read(cx).value();
                        cx.notify();
                    }
                    InputEvent::PressEnter { shift, secondary } => {
                        // 回车发送：Enter/Ctrl+Enter 都发送，Shift+Enter 换行；
                        // 关闭后：Enter 换行，只有 Ctrl+Enter 发送。
                        let send = if this.submit_on_enter {
                            !shift
                        } else {
                            *secondary
                        };
                        if send {
                            // Ctrl+Enter 单独派发：回合进行中它是「立即插话」，
                            // Enter 仍按排队处理。
                            if *secondary {
                                window.dispatch_action(Box::new(SendSecondary), cx);
                            } else {
                                window.dispatch_action(Box::new(Send), cx);
                            }
                        }
                    }
                    _ => {}
                },
            );
            if had_focus {
                state.read(cx).focus_handle(cx).focus(window, cx);
            }
            self.state = Some(state);
            self._subscription = Some(subscription);
        }
        let state = self.state.clone().expect("composer state just initialized");
        if self.applied_submit_on_enter != Some(self.submit_on_enter) {
            let submit = self.submit_on_enter;
            state.update(cx, |state, cx| state.set_submit_on_enter(submit, cx));
            self.applied_submit_on_enter = Some(submit);
        }
        if self.local_focus.is_focused(window) {
            state.read(cx).focus_handle(cx).focus(window, cx);
        }
        if let Some(text) = self.pending.take() {
            state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
        let wechat = wechat_theme::active();
        div()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    let handle = this
                        .state
                        .as_ref()
                        .map(|state| state.read(cx).focus_handle(cx))
                        .unwrap_or_else(|| this.local_focus.clone());
                    handle.focus(window, cx);
                }),
            )
            .child(
                Textarea::new(&state)
                    .appearance(false)
                    .bordered(false)
                    // XSmall 去掉编辑器内部 10/8 的默认内边距，边距交给外壳统一控制。
                    .with_size(gpui::component::Size::XSmall)
                    .px(px(0.))
                    .py(px(0.))
                    .h_full()
                    .font_family(if wechat {
                        wechat_theme::FONT_FAMILY
                    } else {
                        theme::FONT_FAMILY
                    })
                    .text_size(px(ui_kit::T_PAGE))
                    .line_height(px(22.)),
            )
    }
}
