//! 设置 →「能力」：壳真实可用的 N6 能力集（`bot_team::capabilities::advertised`）。
//!
//! 只读状态，不做开关：未接线能力由壳决定不广告，UI 显示「未接线」；
//! 不要在试点里提前把未实现的能力标成可用。

/// 一行能力状态（与 WebView `CapabilitiesSection` 同文案）。
pub struct CapabilityRow {
    pub id: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
    pub wired: bool,
}

/// 固定四行（顺序与 WebView 一致）。
pub fn rows() -> Vec<CapabilityRow> {
    use bot_team::capabilities::{advertised, BotCapability};
    let wired = advertised();
    [
        (
            BotCapability::GroupTasksV1,
            "bot-group-tasks-v1",
            "群任务树",
            "任务 chip、Stop / Reassign / Cancel 与父子等待。",
        ),
        (
            BotCapability::ContextPacketV1,
            "bot-context-packet-v1",
            "上下文包",
            "32k 预算、确定性摘要与压缩熔断。",
        ),
        (
            BotCapability::QuietRunV1,
            "bot-quiet-run-v1",
            "静默运行",
            "群聊 quiet 与跨会话整轮合并通知。",
        ),
        (
            BotCapability::ComputerV1,
            "bot-computer-v1",
            "电脑面板",
            "复用远程桌面流；当前未接线。",
        ),
    ]
    .into_iter()
    .map(|(capability, id, title, hint)| CapabilityRow {
        id,
        title,
        hint,
        wired: wired.contains(&capability),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_shell_advertised_set() {
        let rows = rows();
        assert_eq!(rows.len(), 4);
        let by_id = |id: &str| rows.iter().find(|row| row.id == id).expect("capability row");
        assert!(by_id("bot-group-tasks-v1").wired);
        assert!(by_id("bot-context-packet-v1").wired);
        assert!(by_id("bot-quiet-run-v1").wired);
        assert!(!by_id("bot-computer-v1").wired);
        assert_eq!(by_id("bot-group-tasks-v1").title, "群任务树");
    }
}
