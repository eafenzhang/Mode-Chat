//! 定时任务工具：把 CronCreate/CronList/CronUpdate/CronDelete 的语义
//! 通过 pi-rust 工具暴露给内嵌引擎（与设置页共用 `tasks-index.sqlite`）。

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use automation_repo::AutomationScope;
use automation_repo::AutomationRepo;
use automation_service::{AutomationService, AutomationServiceOptions, CreateParams};
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::types::Tool;
use rpi_ai::Schema;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum Action {
    Create,
    List,
    SetEnabled,
    Delete,
    RunNow,
}

struct AutomationTool {
    service: Arc<AutomationService>,
    scope: AutomationScope,
    tool: Tool,
    action: Action,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn tool_def(name: &str, description: &str, parameters: Value) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters: Schema(parameters),
        constrained_sampling: None,
    }
}

/// 当前会话可用的定时任务工具。
pub fn tools(data_root: &Path, workspace: &str, identity: &str) -> Vec<HarnessTool> {
    let repo = AutomationRepo::with_busy_timeout(
        data_root.join("v2").join("tasks-index.sqlite"),
        5_000,
    );
    let service = Arc::new(AutomationService::new(
        repo,
        AutomationServiceOptions {
            offset_secs: crate::time::local_offset_secs() as i32,
        },
    ));
    let scope = AutomationScope {
        workspace_path: Some(workspace.to_string()),
        workspace_identity: Some(identity.to_string()),
    };
    let defs = vec![
        (
            Action::Create,
            tool_def(
                "automation_create",
                "创建一个定时任务：到点后按 prompt 运行一个回合。cron 为五段表达式（本地时区）。",
                json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "任务标题"},
                        "cron": {"type": "string", "description": "五段 cron，例如 */30 * * * *"},
                        "prompt": {"type": "string", "description": "到点后发给 Bot 的指令"},
                        "recurring": {"type": "boolean", "description": "是否循环（默认 true）"},
                        "maxRuns": {"type": "integer", "description": "最多运行次数（可选）"}
                    },
                    "required": ["title", "cron", "prompt"]
                }),
            ),
        ),
        (
            Action::List,
            tool_def(
                "automation_list",
                "列出当前 Bot 的定时任务（标题、cron、状态、下次运行）。",
                json!({"type": "object", "properties": {}}),
            ),
        ),
        (
            Action::SetEnabled,
            tool_def(
                "automation_set_enabled",
                "暂停或启用一个定时任务。",
                json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "description": "定时任务 id"},
                        "enabled": {"type": "boolean"}
                    },
                    "required": ["id", "enabled"]
                }),
            ),
        ),
        (
            Action::Delete,
            tool_def(
                "automation_delete",
                "删除一个定时任务（不动会话库）。",
                json!({
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
                }),
            ),
        ),
        (
            Action::RunNow,
            tool_def(
                "automation_run_now",
                "立即运行一次定时任务（不修改 cron 计划）。",
                json!({
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
                }),
            ),
        ),
    ];
    defs.into_iter()
        .map(|(action, tool)| {
            HarnessTool::new(Arc::new(AutomationTool {
                service: Arc::clone(&service),
                scope: scope.clone(),
                tool,
                action,
            }))
        })
        .collect()
}

#[async_trait]
impl AgentTool for AutomationTool {
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
        let text = self.run(params).map_err(rpi_agent::AgentError::Tool)?;
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

impl AutomationTool {
    fn run(&self, params: Value) -> Result<String, String> {
        match self.action {
            Action::Create => {
                let title = string_field(&params, "title")?;
                let cron_expr = string_field(&params, "cron")?;
                let prompt = string_field(&params, "prompt")?;
                let created = self.service.create(
                    &CreateParams {
                        title,
                        cron_expr,
                        relative_delay_minutes: None,
                        prompt,
                        model_selection: None,
                        mode: None,
                        target_task_id: None,
                        bot_delivery_target: None,
                        recurring: params.get("recurring").and_then(Value::as_bool),
                        max_runs: params.get("maxRuns").and_then(Value::as_i64),
                        end_at: None,
                        interval_unit: None,
                        interval: None,
                        schedule_rule: None,
                    },
                    &self.scope,
                    now_ms(),
                )?;
                Ok(format!(
                    "已创建定时任务「{}」（id {}，cron {}）",
                    created.title, created.automation_id, created.cron_expr
                ))
            }
            Action::List => {
                let rows = self.service.list(&self.scope)?;
                if rows.is_empty() {
                    return Ok("当前 Bot 没有定时任务".to_string());
                }
                let lines = rows
                    .into_iter()
                    .map(|row| {
                        format!(
                            "{} · {} · {} · {} · 下次 {}",
                            row.automation_id,
                            row.title,
                            row.cron_expr,
                            if row.enabled { "已启用" } else { "已暂停" },
                            row.next_run_at
                                .map(|at| {
                                    let at = at as u64;
                                    format!(
                                        "{} {}",
                                        crate::time::format_day(at),
                                        crate::time::format_hm(at)
                                    )
                                })
                                .unwrap_or_else(|| "-".to_string())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(lines)
            }
            Action::SetEnabled => {
                let id = string_field(&params, "id")?;
                let enabled = params
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| "enabled 必须是布尔值".to_string())?;
                self.service.set_enabled(&id, enabled, &self.scope)?;
                Ok(format!("任务 {id} 已{}", if enabled { "启用" } else { "暂停" }))
            }
            Action::Delete => {
                let id = string_field(&params, "id")?;
                let removed = self.service.delete(&id, &self.scope)?;
                Ok(if removed {
                    format!("已删除任务 {id}")
                } else {
                    format!("任务不存在：{id}")
                })
            }
            Action::RunNow => {
                let id = string_field(&params, "id")?;
                let claimed = self.service.run_now(&id, &self.scope, now_ms())?;
                Ok(if claimed.is_some() {
                    format!("任务 {id} 已排队，调度器会在 20 秒内认领")
                } else {
                    format!("任务 {id} 当前不可运行（可能已在运行）")
                })
            }
        }
    }
}

fn string_field(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少参数 {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn call(tools: &[HarnessTool], name: &str, params: Value) -> String {
        let tool = tools
            .iter()
            .find(|tool| tool.tool.schema().name == name)
            .expect("tool missing");
        let result = tool
            .tool
            .execute(
                "test",
                params,
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .expect("execute");
        match &result.content[0] {
            TextContentOrImage::Text(text) => text.text.clone(),
            TextContentOrImage::Image(_) => "[image]".to_string(),
        }
    }

    #[tokio::test]
    async fn create_list_and_delete() {
        let dir = std::env::temp_dir().join(format!("mode-chat-auto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tools = tools(&dir, "C:/mode-chat-test-ws", "mode-bot:teammate:test");
        let created = call(
            &tools,
            "automation_create",
            json!({"title":"每分钟检查","cron":"* * * * *","prompt":"ping","recurring":true}),
        )
        .await;
        assert!(created.contains("已创建"), "{created}");
        let listed = call(&tools, "automation_list", json!({})).await;
        assert!(listed.contains("每分钟检查"), "{listed}");
        let id = listed
            .split_whitespace()
            .next()
            .unwrap()
            .trim()
            .to_string();
        let deleted = call(&tools, "automation_delete", json!({"id": id})).await;
        assert!(deleted.contains("已删除"), "{deleted}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
