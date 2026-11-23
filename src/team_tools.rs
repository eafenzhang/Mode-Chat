//! Chief（团队管理员）专属工具：名册、群目标、任务台账与派活。
//!
//! 工具只在群聊里由 `is_chief` 成员的会话装载；普通成员看不到，避免每个 Bot
//! 都能改目标/派活。派活沿用群任务台账 `create_handoff`，并通过
//! `BackendEvent::TeamDispatch` 让 UI 触发目标成员的回合（与 @ 交接同一条链路）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::Schema;
use rpi_ai::types::Tool;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::backend::BackendEvent;

/// 当前群回合的共享上下文：根任务与已经跑过的成员。
#[derive(Default)]
pub struct TeamTurnContext {
    pub root_task_id: Option<String>,
    pub root_message_id: Option<String>,
    /// 已在本回合派发/运行的成员 id（用于阻止重复派发）。
    pub active_members: HashSet<String>,
}

pub type TeamTurn = Arc<Mutex<TeamTurnContext>>;

pub fn turn_context() -> TeamTurn {
    Arc::new(Mutex::new(TeamTurnContext::default()))
}

/// 开始一轮：清空上一轮的根任务与活跃成员。
pub fn begin_turn(
    turn: &TeamTurn,
    root_task_id: &str,
    root_message_id: &str,
    active: impl IntoIterator<Item = String>,
) {
    if let Ok(mut context) = turn.lock() {
        if !root_task_id.trim().is_empty() {
            context.root_task_id = Some(root_task_id.to_string());
        }
        if !root_message_id.trim().is_empty() {
            context.root_message_id = Some(root_message_id.to_string());
        }
        context.active_members = active.into_iter().collect();
    }
}

/// 续跑/交接：补上根任务，并把目标记为活跃。
pub fn note_dispatch(
    turn: &TeamTurn,
    root_task_id: Option<&str>,
    root_message_id: Option<&str>,
    member_id: &str,
) {
    if let Ok(mut context) = turn.lock() {
        if let Some(root) = root_task_id.filter(|value| !value.trim().is_empty()) {
            context.root_task_id = Some(root.to_string());
        }
        if let Some(message) = root_message_id.filter(|value| !value.trim().is_empty()) {
            context.root_message_id = Some(message.to_string());
        }
        context.active_members.insert(member_id.to_string());
    }
}

#[derive(Clone)]
struct TeamMember {
    id: String,
    name: String,
    role: String,
    is_admin: bool,
}

#[derive(Clone, Copy)]
enum Action {
    Roster,
    GoalShow,
    GoalCreate,
    GoalStage,
    TaskList,
    Assign,
}

struct TeamTool {
    data_root: PathBuf,
    group_id: String,
    group_key: String,
    caller_id: String,
    caller_name: String,
    members: Vec<TeamMember>,
    turn: TeamTurn,
    events: UnboundedSender<BackendEvent>,
    tool: Tool,
    action: Action,
}

fn tool_def(name: &str, description: &str, parameters: Value) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters: Schema(parameters),
        constrained_sampling: None,
    }
}

/// 为群里的 Chief 会话构造工具集。
#[allow(clippy::too_many_arguments)]
pub fn tools(
    data_root: &Path,
    group_id: &str,
    group_key: &str,
    caller_id: &str,
    caller_name: &str,
    members: Vec<(String, String, String, bool)>,
    turn: TeamTurn,
    events: UnboundedSender<BackendEvent>,
) -> Vec<HarnessTool> {
    let members: Vec<TeamMember> = members
        .into_iter()
        .map(|(id, name, role, is_admin)| TeamMember {
            id,
            name,
            role,
            is_admin,
        })
        .collect();
    let defs = vec![
        (
            Action::Roster,
            tool_def(
                "team_roster",
                "查看当前群成员（id、名字、角色、是否管理员）。派活前先确认成员。",
                json!({"type": "object", "properties": {}}),
            ),
        ),
        (
            Action::GoalShow,
            tool_def(
                "team_goal_show",
                "读取当前群目标与阶段进度。",
                json!({"type": "object", "properties": {}}),
            ),
        ),
        (
            Action::GoalCreate,
            tool_def(
                "team_goal_create",
                "为当前群创建目标（已有目标时失败）。stages 至少一项，第一项自动变为进行中。",
                json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "目标标题"},
                        "description": {"type": "string", "description": "目标说明（可选）"},
                        "stages": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "阶段标题列表"
                        }
                    },
                    "required": ["title", "stages"]
                }),
            ),
        ),
        (
            Action::GoalStage,
            tool_def(
                "team_goal_stage",
                "更新群目标某个阶段的状态（pending / active / done）。stage 可以传阶段标题或 id。",
                json!({
                    "type": "object",
                    "properties": {
                        "stage": {"type": "string", "description": "阶段标题或 id"},
                        "status": {"type": "string", "enum": ["pending", "active", "done"]}
                    },
                    "required": ["stage", "status"]
                }),
            ),
        ),
        (
            Action::TaskList,
            tool_def(
                "team_task_list",
                "列出当前群的派活台账（任务 id、负责人、状态、期望结果）。",
                json!({"type": "object", "properties": {}}),
            ),
        ),
        (
            Action::Assign,
            tool_def(
                "team_assign",
                "把一项任务派给群成员：在当前根任务下建子任务并立即让该成员开工。不要派给正在跑的成员。",
                json!({
                    "type": "object",
                    "properties": {
                        "target": {"type": "string", "description": "成员名字或 id"},
                        "task": {"type": "string", "description": "要对方完成的具体任务"}
                    },
                    "required": ["target", "task"]
                }),
            ),
        ),
    ];
    defs.into_iter()
        .map(|(action, tool)| {
            HarnessTool::new(Arc::new(TeamTool {
                data_root: data_root.to_path_buf(),
                group_id: group_id.to_string(),
                group_key: group_key.to_string(),
                caller_id: caller_id.to_string(),
                caller_name: caller_name.to_string(),
                members: members.clone(),
                turn: Arc::clone(&turn),
                events: events.clone(),
                tool,
                action,
            }))
        })
        .collect()
}

#[async_trait]
impl AgentTool for TeamTool {
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

impl TeamTool {
    fn run(&self, params: Value) -> Result<String, String> {
        match self.action {
            Action::Roster => Ok(self.roster()),
            Action::GoalShow => self.goal_show(),
            Action::GoalCreate => self.goal_create(&params),
            Action::GoalStage => self.goal_stage(&params),
            Action::TaskList => self.task_list(),
            Action::Assign => self.assign(&params),
        }
    }

    fn roster(&self) -> String {
        let mut lines = vec![format!(
            "当前群成员（调用者：{}，{}）：",
            self.caller_name, self.caller_id
        )];
        for member in &self.members {
            lines.push(format!(
                "- {}（id {}，角色 {}）{}",
                member.name,
                member.id,
                if member.role.trim().is_empty() {
                    "成员"
                } else {
                    member.role.as_str()
                },
                if member.is_admin { " · 管理员" } else { "" }
            ));
        }
        lines.join("\n")
    }

    fn goal_show(&self) -> Result<String, String> {
        let Some(goal) = crate::group_goal::read(&self.data_root, &self.group_id) else {
            return Ok("当前群还没有目标".to_string());
        };
        let mut lines = vec![format!(
            "目标：{}（{:?}）",
            goal.title, goal.status
        )];
        if !goal.description.trim().is_empty() {
            lines.push(goal.description.clone());
        }
        for stage in &goal.stages {
            lines.push(format!(
                "- [{}] {}（id {}）",
                status_label(stage.status),
                stage.title,
                stage.id
            ));
        }
        Ok(lines.join("\n"))
    }

    fn goal_create(&self, params: &Value) -> Result<String, String> {
        let title = string_field(params, "title")?;
        let description = params
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let stages: Vec<String> = params
            .get("stages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if stages.is_empty() {
            return Err("stages 至少需要一个阶段".to_string());
        }
        let goal = crate::group_goal::apply(
            &self.data_root,
            &self.group_id,
            bot_team::goal::GoalAction::Create {
                title,
                description,
                stages,
            },
        )?
        .ok_or_else(|| "创建目标失败".to_string())?;
        Ok(format!(
            "已创建目标「{}」，共 {} 个阶段",
            goal.title,
            goal.stages.len()
        ))
    }

    fn goal_stage(&self, params: &Value) -> Result<String, String> {
        let target = string_field(params, "stage")?;
        let status = string_field(params, "status")?;
        let status = match status.to_ascii_lowercase().as_str() {
            "pending" => bot_team::goal::StageStatus::Pending,
            "active" => bot_team::goal::StageStatus::Active,
            "done" => bot_team::goal::StageStatus::Done,
            other => return Err(format!("未知阶段状态：{other}")),
        };
        let goal = crate::group_goal::read(&self.data_root, &self.group_id)
            .ok_or_else(|| "当前群还没有目标".to_string())?;
        let stage = goal
            .stages
            .iter()
            .find(|stage| {
                stage.id == target || stage.title.eq_ignore_ascii_case(target.trim())
            })
            .ok_or_else(|| format!("找不到阶段：{target}"))?;
        let stage_id = stage.id.clone();
        let stage_title = stage.title.clone();
        crate::group_goal::apply(
            &self.data_root,
            &self.group_id,
            bot_team::goal::GoalAction::SetStageStatus {
                stage_id,
                status,
            },
        )?;
        Ok(format!("阶段「{stage_title}」已更新为 {}", status_label(status)))
    }

    fn task_list(&self) -> Result<String, String> {
        let tasks = crate::group_tasks::list(&self.data_root, &self.group_id)?;
        if tasks.is_empty() {
            return Ok("当前群还没有派活记录".to_string());
        }
        let mut lines = vec![format!("共 {} 条任务：", tasks.len())];
        for task in tasks {
            let owner = self
                .members
                .iter()
                .find(|member| member.id == task.owner_member_id)
                .map(|member| member.name.clone())
                .unwrap_or_else(|| task.owner_member_id.clone());
            lines.push(format!(
                "- {} · {} · {:?} · {}",
                task.id,
                owner,
                task.state,
                truncate(&task.expected_result, 80)
            ));
        }
        Ok(lines.join("\n"))
    }

    fn assign(&self, params: &Value) -> Result<String, String> {
        let target = string_field(params, "target")?;
        let task = string_field(params, "task")?;
        let member = self
            .members
            .iter()
            .find(|member| {
                member.id == target || member.name.eq_ignore_ascii_case(target.trim())
            })
            .ok_or_else(|| format!("找不到群成员：{target}"))?;
        if member.id == self.caller_id {
            return Err("不要给自己派活".to_string());
        }
        let (root_task_id, root_message_id) = {
            let context = self
                .turn
                .lock()
                .map_err(|_| "团队回合上下文不可用".to_string())?;
            if context.active_members.contains(&member.id) {
                return Err(format!(
                    "{} 已经在本回合执行任务，请等它完成后再派",
                    member.name
                ));
            }
            (
                context
                    .root_task_id
                    .clone()
                    .ok_or_else(|| "当前没有群回合根任务，无法派活".to_string())?,
                context
                    .root_message_id
                    .clone()
                    .unwrap_or_else(|| root_placeholder(&self.group_id)),
            )
        };
        let created = crate::group_tasks::create_handoff(
            &self.data_root,
            &self.group_id,
            &root_message_id,
            &root_task_id,
            &member.id,
            &task,
        )?;
        if let Ok(mut context) = self.turn.lock() {
            context.active_members.insert(member.id.clone());
        }
        let _ = self.events.send(BackendEvent::TeamDispatch {
            key: self.group_key.clone(),
            member_id: member.id.clone(),
            member_name: member.name.clone(),
            text: task.clone(),
            task_id: created.id.clone(),
            root_task_id,
            root_message_id,
        });
        Ok(format!(
            "已把任务派给 {}（任务 id {}），它会在当前回合里开始执行。",
            member.name, created.id
        ))
    }
}

fn status_label(status: bot_team::goal::StageStatus) -> &'static str {
    match status {
        bot_team::goal::StageStatus::Pending => "待开始",
        bot_team::goal::StageStatus::Active => "进行中",
        bot_team::goal::StageStatus::Done => "已完成",
    }
}

fn string_field(params: &Value, field: &str) -> Result<String, String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少参数：{field}"))
}

fn truncate(value: &str, max: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= max {
        value.to_string()
    } else {
        format!("{}…", value.chars().take(max).collect::<String>())
    }
}

fn root_placeholder(group_id: &str) -> String {
    format!("group-turn:{group_id}")
}
