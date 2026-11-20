//! Bot 壳能力声明（N6 W4，spec `docs/specs/mode-bot-rust-migration.md` §9.7）。
//!
//! 前端启动读一次能力集，按能力渲染；老壳没有新命令时自然隐藏新动作。能力只在
//! **壳侧真实可用**时广告——不要把未接线的功能提前放进来。

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BotCapability {
    /// W1：任务树命令 + `bot:group-task-event`。
    #[serde(rename = "bot-group-tasks-v1")]
    GroupTasksV1,
    /// W2：context packet 构建 + `context.json` + 熔断事件。
    #[serde(rename = "bot-context-packet-v1")]
    ContextPacketV1,
    /// W4：quiet run 投递语义（频道 quiet + 心跳抑制 + 跨会话合并通知）。
    #[serde(rename = "bot-quiet-run-v1")]
    QuietRunV1,
    /// W4：电脑面板流（尚未接线）。
    #[serde(rename = "bot-computer-v1")]
    ComputerV1,
}

impl BotCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            BotCapability::GroupTasksV1 => "bot-group-tasks-v1",
            BotCapability::ContextPacketV1 => "bot-context-packet-v1",
            BotCapability::QuietRunV1 => "bot-quiet-run-v1",
            BotCapability::ComputerV1 => "bot-computer-v1",
        }
    }
}

/// 当前壳真实可用的能力。电脑面板接线后再加入。
pub fn advertised() -> &'static [BotCapability] {
    &[
        BotCapability::GroupTasksV1,
        BotCapability::ContextPacketV1,
        BotCapability::QuietRunV1,
    ]
}

pub fn supports(capability: BotCapability) -> bool {
    advertised().contains(&capability)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_wire_names_are_stable() {
        assert_eq!(BotCapability::GroupTasksV1.as_str(), "bot-group-tasks-v1");
        assert_eq!(
            BotCapability::ContextPacketV1.as_str(),
            "bot-context-packet-v1"
        );
        assert_eq!(BotCapability::QuietRunV1.as_str(), "bot-quiet-run-v1");
        assert_eq!(BotCapability::ComputerV1.as_str(), "bot-computer-v1");
    }

    #[test]
    fn advertised_set_only_contains_wired_capabilities() {
        assert!(supports(BotCapability::GroupTasksV1));
        assert!(supports(BotCapability::ContextPacketV1));
        assert!(supports(BotCapability::QuietRunV1));
        assert!(!supports(BotCapability::ComputerV1));
    }

    #[test]
    fn serializes_as_kebab_case_strings() {
        let value = serde_json::to_value(BotCapability::GroupTasksV1).unwrap();
        assert_eq!(value, serde_json::json!("bot-group-tasks-v1"));
    }
}
