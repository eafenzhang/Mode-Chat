//! 后台任务工具（长任务 P1）：`task_start` / `task_send` / `task_stop` / `task_list`。
//!
//! 任务 = 独立 RpiSession（与主会话并行，记录在 `v2/tasks/index.json`）。
//! 工具走 `backend::bus()` 的 TaskStart/TaskSend/TaskStop 命令；`task_list`
//! 直接读记录。任务产物与主会话隔离（`.tasks/` 子目录），不阻塞调用者 lane。

use std::sync::Arc;

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::Schema;
use rpi_ai::types::Tool;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum Action {
    Start,
    Send,
    Stop,
    List,
}

struct TaskTool {
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
    HarnessTool::new(Arc::new(TaskTool {
        tool,
        action,
        identity: identity.to_string(),
    }))
}

/// 构造任务工具集。`identity` 是调用者的 `mode-bot:teammate:<id>`。
pub fn tools(identity: &str) -> Vec<HarnessTool> {
    let identity = identity.trim();
    if identity.is_empty() {
        return Vec::new();
    }
    vec![
        define(
            identity,
            Action::Start,
            tool_def(
                "task_start",
                "开一个后台任务：独立会话里执行一段较长的工作，不阻塞当前对话。\
                 返回任务 id；用 task_list 看进度，task_stop 停止。适合耗时数分钟以上的任务。",
                json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "任务标题（简短，会显示在任务列表）" },
                        "prompt": { "type": "string", "description": "任务的完整指令（自包含，任务会话看不到当前对话）" }
                    },
                    "required": ["prompt"]
                }),
            ),
        ),
        define(
            identity,
            Action::Send,
            tool_def(
                "task_send",
                "给运行中的后台任务追加一条消息（补充要求或回答它的问题）。",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "task_start 返回的任务 id" },
                        "text": { "type": "string", "description": "要追加的内容" }
                    },
                    "required": ["task_id", "text"]
                }),
            ),
        ),
        define(
            identity,
            Action::Stop,
            tool_def(
                "task_stop",
                "停止一个运行中的后台任务。",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "task_start 返回的任务 id" }
                    },
                    "required": ["task_id"]
                }),
            ),
        ),
        define(
            identity,
            Action::List,
            tool_def(
                "task_list",
                "列出当前 Bot 的后台任务（状态、标题、最近一行输出）。",
                json!({ "type": "object", "properties": {} }),
            ),
        ),
    ]
}

impl TaskTool {
    fn bot_id(&self) -> Option<String> {
        crate::rpi_engine::bot_id_from_identity(&self.identity)
    }

    fn ui_key(&self) -> Option<String> {
        self.bot_id().map(|id| format!("bot:{id}"))
    }

    async fn run(&self, params: Value) -> Result<String, String> {
        match self.action {
            Action::Start => self.start(&params).await,
            Action::Send => self.send(&params),
            Action::Stop => self.stop(&params),
            Action::List => self.list(),
        }
    }

    async fn start(&self, params: &Value) -> Result<String, String> {
        let prompt = params
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if prompt.is_empty() {
            return Err("prompt 不能为空".to_string());
        }
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let bot_id = self.bot_id().ok_or_else(|| "缺少 Bot 身份".to_string())?;
        let ui_key = self.ui_key().ok_or_else(|| "缺少 UI key".to_string())?;
        let handle = crate::backend::bus().ok_or_else(|| "后台引擎未就绪".to_string())?;
        let reply = handle.task_start(ui_key, bot_id, title, prompt)?;
        match tokio::time::timeout(std::time::Duration::from_secs(30), reply).await {
            Ok(Ok(Ok(record))) => Ok(format!(
                "任务已创建：{}（{}），运行中可用 task_list 查看进度。",
                record.id, record.title
            )),
            Ok(Ok(Err(message))) => Err(message),
            Ok(Err(_)) => Err("任务创建未返回（引擎可能已停止）".to_string()),
            Err(_) => Err("任务创建超时（30s）".to_string()),
        }
    }

    fn send(&self, params: &Value) -> Result<String, String> {
        let task_id = params
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if task_id.is_empty() || text.is_empty() {
            return Err("task_id 与 text 都不能为空".to_string());
        }
        let handle = crate::backend::bus().ok_or_else(|| "后台引擎未就绪".to_string())?;
        handle.task_send(task_id.clone(), text);
        Ok(format!("已追加到任务 {task_id}。"))
    }

    fn stop(&self, params: &Value) -> Result<String, String> {
        let task_id = params
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if task_id.is_empty() {
            return Err("task_id 不能为空".to_string());
        }
        let handle = crate::backend::bus().ok_or_else(|| "后台引擎未就绪".to_string())?;
        handle.task_stop(task_id.clone());
        Ok(format!("已请求停止任务 {task_id}。"))
    }

    fn list(&self) -> Result<String, String> {
        let ui_key = self.ui_key().ok_or_else(|| "缺少 UI key".to_string())?;
        let tasks = crate::task_store::list_for(&crate::data::data_root(), &ui_key);
        if tasks.is_empty() {
            return Ok("还没有后台任务。".to_string());
        }
        let mut lines = vec!["后台任务（新→旧）：".to_string()];
        for task in tasks.iter().take(20) {
            let line = format!(
                "- {} [{}] {}{}{}",
                task.id,
                task.status.as_str(),
                task.title,
                if task.last_line.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", task.last_line.chars().take(60).collect::<String>())
                },
                if task.error.is_empty() {
                    String::new()
                } else {
                    format!(" · 错误：{}", task.error.chars().take(80).collect::<String>())
                },
            );
            lines.push(line);
        }
        Ok(lines.join("\n"))
    }
}

#[async_trait]
impl AgentTool for TaskTool {
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
