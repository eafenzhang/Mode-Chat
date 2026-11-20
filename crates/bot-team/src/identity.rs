//! 身份键与 AGENTS.md 受管块 marker。
//!
//! 这些字符串会进 workspaceIdentity / 会话持久化 / 记忆目录，是数据隔离的根；
//! 改动等同迁移，必须同时改数据迁移方案，不能只改常量。

pub const IDENTITY_START_MARKER: &str = "<!-- mode-bot:identity:start -->";
pub const IDENTITY_END_MARKER: &str = "<!-- mode-bot:identity:end -->";

/// teammate 的身份键：进 workspaceIdentity 后驱动 agent 进程、会话与事件路由隔离。
pub fn teammate_workspace_identity(teammate_id: &str) -> String {
    format!("mode-bot:teammate:{teammate_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teammate_identity_shape() {
        assert_eq!(teammate_workspace_identity("t1"), "mode-bot:teammate:t1");
    }
}
