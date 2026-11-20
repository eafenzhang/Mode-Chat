//! `interaction/requestPermission`、`interaction/requestUserInput` 反向请求的**持有**应答。
//!
//! 为什么只持有不回答：CLI 的权限 broker 用「legacy 反向 RPC 与 v4 `resolveInteraction`
//! 竞速」（`interaction-response-race.ts`）——legacy 反向请求是主 await，v4 应答到达时
//! CLI 自己 abort 这条请求（-32021）并映射 v4 answer。宿主对这两类请求的正确姿势是
//! **返回 `Deferred` 且不回帧**：审批卡经 v4 会话流的 `permission.requested` 事件渲染，
//! 用户作答走 v4 命令面；一旦宿主回错误帧（无人处理时的默认行为）就会立刻杀死竞速，
//! CLI 侧表现为 `Permission request failed`（审批卡根本来不及出现）。
//!
//! 持有的请求在以下情况自然收口：v4 应答到达（CLI abort）、CLI 侧请求超时、或连接关闭。
//! 本处理器不维护 pending 表、不代答——代答与 UI 归宿主/renderer（见
//! `packages/services/src/mode-agent/modeAgentService.ts` 的 `interactionRequestPermission`
//! 分支与 `interaction-broker.ts`）。

use mode_protocol_client::{RequestHandler, RequestReply};
use serde_json::Value;

/// Agent → 宿主的权限确认反向请求（`modeProtocolMethods.interactionRequestPermission`）。
pub const INTERACTION_REQUEST_PERMISSION_METHOD: &str = "interaction/requestPermission";
/// Agent → 宿主的用户输入（AskUserQuestion）反向请求。
pub const INTERACTION_REQUEST_USER_INPUT_METHOD: &str = "interaction/requestUserInput";

fn is_hold_method(method: Option<&str>) -> bool {
    matches!(
        method,
        Some(INTERACTION_REQUEST_PERMISSION_METHOD) | Some(INTERACTION_REQUEST_USER_INPUT_METHOD)
    )
}

/// 纯函数（测试用）：该方法是否应被持有。
fn hold_decision(method: Option<&str>) -> bool {
    is_hold_method(method)
}

/// 持有型反向请求处理器：命中即 `Deferred`（不回帧），未命中交还链路。
pub fn interaction_hold_handler() -> RequestHandler {
    Arc::new(move |request: &Value, _reply: mode_protocol_client::ReplyHandle| {
        if hold_decision(request.get("method").and_then(Value::as_str)) {
            RequestReply::Deferred
        } else {
            RequestReply::NotHandled
        }
    })
}

use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::{hold_decision, INTERACTION_REQUEST_PERMISSION_METHOD};

    #[test]
    fn permission_request_is_held() {
        assert!(hold_decision(Some(INTERACTION_REQUEST_PERMISSION_METHOD)));
        assert!(hold_decision(Some("interaction/requestUserInput")));
    }

    #[test]
    fn other_methods_pass_through() {
        assert!(!hold_decision(Some("interaction/requestProviderRuntimeHeaders")));
        assert!(!hold_decision(Some("session/read")));
        assert!(!hold_decision(None));
    }
}
