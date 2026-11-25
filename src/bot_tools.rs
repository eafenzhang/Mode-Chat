//! Bot 之间的协作工具（v0.5.0 W2）：`list_bots` / `message_bot` / `post_to_group` / `whoami`。
//!
//! 与 DS Bot 的 `ds-bot/src/host/tools.js`（15 个团队工具）对齐的最小闭环：
//! 任何 Bot 都能看到名册、能给另一个 Bot 发一条消息并拿到回复、能往群里发言。
//!
//! 设计约束：
//! - **隐私优先**：`list_bots` 对隐私 Bot 只输出名字与 `private:true`，不给 id/目录；
//!   `message_bot` 目标为隐私 Bot 时直接拒绝，不建会话。
//! - **不死锁**：投递走 `Command::AskBot`（run loop 里开独立会话，不占用调用者 lane），
//!   120s 超时；目标正在跑也只影响它自己那一份，不会互相等。
//! - **群发言不阻塞**：`post_to_group` 只发一个 `BackendEvent::GroupPost`，
//!   由 UI 按既有的群发送路径派发回合，工具立即返回。

use std::sync::Arc;

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::Schema;
use rpi_ai::types::Tool;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const MESSAGE_MAX_CHARS: usize = 8_000;

#[derive(Clone, Copy)]
enum Action {
    ListBots,
    MessageBot,
    PostGroup,
    WhoAmI,
}

struct BotTool {
    tool: Tool,
    action: Action,
    identity: String,
}

fn tool_def(name: &str, description: &str, parameters: Value) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters: Schema(parameters),
        constrained_sampling: None,
    }
}

fn define(identity: &str, action: Action, tool: Tool) -> HarnessTool {
    HarnessTool::new(Arc::new(BotTool {
        tool,
        action,
        identity: identity.to_string(),
    }))
}

/// 构造协作工具集。`identity` 是调用者的 `mode-bot:teammate:<id>`。
pub fn tools(identity: &str) -> Vec<HarnessTool> {
    let identity = identity.trim();
    if identity.is_empty() {
        return Vec::new();
    }
    vec![
        define(
            identity,
            Action::WhoAmI,
            tool_def(
                "whoami",
                "返回你自己的名字、id 与角色。跨 Bot 沟通前先确认身份。",
                json!({"type": "object", "properties": {}}),
            ),
        ),
        define(
            identity,
            Action::ListBots,
            tool_def(
                "list_bots",
                "列出团队里的 Bot（名字、角色、是否隐私）。隐私 Bot 不给 id。",
                json!({"type": "object", "properties": {}}),
            ),
        ),
        define(
            identity,
            Action::MessageBot,
            tool_def(
                "message_bot",
                "给另一个 Bot 发一条消息并等待它的回复。\
                 对方在忙或设为隐私时会被拒绝，不要重试，直接告诉用户。",
                json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "string", "description": "目标 Bot 的名字或 id"},
                        "message": {"type": "string", "description": "要说的事（自我说明，对方看不到你的上下文）"}
                    },
                    "required": ["to", "message"]
                }),
            ),
        ),
        define(
            identity,
            Action::PostGroup,
            tool_def(
                "post_to_group",
                "往一个群聊里发一条消息（会按群的回应策略派发回合，不等待回复）。",
                json!({
                    "type": "object",
                    "properties": {
                        "group": {"type": "string", "description": "群名或群 id"},
                        "message": {"type": "string", "description": "要发的内容"}
                    },
                    "required": ["group", "message"]
                }),
            ),
        ),
    ]
}

#[async_trait]
impl AgentTool for BotTool {
    fn schema(&self) -> &Tool {
        &self.tool
    }

    fn label(&self) -> &str {
        &self.tool.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(rpi_agent::ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, rpi_agent::AgentError> {
        let text = self
            .run(params)
            .await
            .map_err(rpi_agent::AgentError::Tool)?;
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::Text(rpi_ai::TextContent {
                kind: rpi_ai::types::TextContentType,
                text,
                text_signature: None,
            })],
            details: Value::Null,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }
}

impl BotTool {
    async fn run(&self, params: Value) -> Result<String, String> {
        match self.action {
            Action::WhoAmI => self.who_am_i(),
            Action::ListBots => self.list_bots(),
            Action::MessageBot => self.message_bot(&params).await,
            Action::PostGroup => self.post_group(&params),
        }
    }

    fn roster(&self) -> crate::data::Roster {
        crate::data::Roster::load()
    }

    fn who_am_i(&self) -> Result<String, String> {
        let roster = self.roster();
        let id = crate::rpi_engine::bot_id_from_identity(&self.identity);
        match id.and_then(|id| roster.bot_by_id(&id).map(|bot| (id, bot.clone()))) {
            Some((id, bot)) => Ok(format!(
                "我是 {}（id {}），角色：{}。{}",
                bot.name,
                id,
                if bot.role.trim().is_empty() {
                    "成员"
                } else {
                    bot.role.as_str()
                },
                if bot.is_chief { "我是团队管理员。" } else { "" }
            )),
            None => Ok(format!("我的身份：{}（名册里查不到记录）", self.identity)),
        }
    }

    fn list_bots(&self) -> Result<String, String> {
        let roster = self.roster();
        if roster.bots.is_empty() && roster.hidden_bots.is_empty() {
            return Ok("名册里还没有别的 Bot".to_string());
        }
        let mut lines = vec!["团队里的 Bot：".to_string()];
        for bot in roster.bots.iter().chain(roster.hidden_bots.iter()) {
            // 隐私 Bot 只暴露名字：id 与目录都不给（PRD AC1/AC4）。
            if bot.is_private {
                lines.push(format!(
                    "- {}（{}）[私有]",
                    bot.name,
                    if bot.role.trim().is_empty() {
                        "成员"
                    } else {
                        bot.role.as_str()
                    }
                ));
                continue;
            }
            lines.push(format!(
                "- {}（{}）id={}{}{}",
                bot.name,
                if bot.role.trim().is_empty() {
                    "成员"
                } else {
                    bot.role.as_str()
                },
                bot.id,
                if bot.is_chief { " · 管理员" } else { "" },
                if bot.is_main { " · 主 Bot" } else { "" }
            ));
        }
        lines.push("（隐私 Bot 不提供 id，也不能通过 message_bot 联系）".to_string());
        Ok(lines.join("\n"))
    }

    async fn message_bot(&self, params: &Value) -> Result<String, String> {
        let target = params
            .get("to")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "缺少参数：to".to_string())?;
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "缺少参数：message".to_string())?;
        if message.chars().count() > MESSAGE_MAX_CHARS {
            return Err(format!("message 过长（最多 {MESSAGE_MAX_CHARS} 字符）"));
        }
        let roster = self.roster();
        let found = roster
            .bots
            .iter()
            .chain(roster.hidden_bots.iter())
            .find(|bot| bot.id == target || bot.name.eq_ignore_ascii_case(target));
        let Some(bot) = found else {
            return Err(format!("找不到 Bot：{target}（先用 list_bots 看名册）"));
        };
        if bot.is_private {
            return Err("目标 Bot 已设为隐私，拒绝跨 Bot 读取".to_string());
        }
        let caller = crate::rpi_engine::bot_id_from_identity(&self.identity);
        if caller.as_deref() == Some(bot.id.as_str()) {
            return Err("不要给自己发消息".to_string());
        }
        let Some(bus) = crate::backend::bus() else {
            return Err("后台引擎未启动，无法跨 Bot 投递".to_string());
        };
        let reply = bus.ask_bot(bot.id.clone(), bot.name.clone(), message.to_string()).await?;
        if reply.trim().is_empty() {
            return Ok(format!("{} 的回复是空的。", bot.name));
        }
        Ok(format!("{} 的回复：\n{}", bot.name, reply.trim()))
    }

    fn post_group(&self, params: &Value) -> Result<String, String> {
        let target = params
            .get("group")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "缺少参数：group".to_string())?;
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "缺少参数：message".to_string())?;
        let roster = self.roster();
        let Some(group) = roster
            .groups
            .iter()
            .chain(roster.hidden_groups.iter())
            .find(|group| group.id == target || group.name.eq_ignore_ascii_case(target))
        else {
            return Err(format!("找不到群聊：{target}"));
        };
        let caller_id = crate::rpi_engine::bot_id_from_identity(&self.identity)
            .ok_or_else(|| "只有团队 Bot 能往群里发言".to_string())?;
        if !group.member_ids.iter().any(|id| id == &caller_id) {
            return Err(format!("你不在群「{}」里，不能发言", group.name));
        }
        let Some(bus) = crate::backend::bus() else {
            return Err("后台引擎未启动，无法往群里发言".to_string());
        };
        let caller_name = roster
            .bot_by_id(&caller_id)
            .map(|bot| bot.name.clone())
            .unwrap_or_else(|| caller_id.clone());
        bus.post_group(
            group.id.clone(),
            message.to_string(),
            caller_id,
            caller_name.clone(),
        )?;
        Ok(format!("已发到群「{}」", group.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tools_without_identity() {
        assert!(tools("").is_empty());
        assert_eq!(tools("mode-bot:teammate:b1").len(), 4);
    }
}
