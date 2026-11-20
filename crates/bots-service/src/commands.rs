//! 命令执行面（步骤 3f：status / new / workspace）。
//!
//! 只做**状态与选择**，文案由宿主 i18n 渲染（`CommandReply` 返回结构化结果）：
//! - `new`：进入新草稿（可同时切换工作区），清旧任务/审批/队列；
//! - `workspace list`：返回允许列表与当前工作区；
//! - `workspace set`：按序号/id/label/path/key 解析，失败返回可选列表供提示；
//! - `status`：返回当前上下文快照（mode/工作区/taskId/队列长度），不读运行态任务。
//!
//! 绑定 workspace 到会话是纯状态操作；发送回复、typing、审批等副作用由宿主执行。

use serde_json::Value;

use crate::conversation::{switch_workspace_context, write_draft_context};
use crate::workspace::{
    filter_allowed_workspaces, resolve_workspace_by_value, workspace_key, WorkspaceRef,
};
use crate::BotsRepo;

#[derive(Debug, Clone, PartialEq)]
pub enum CommandReply {
    /// `/new`：已进入草稿（附带工作区 id 与是否发生切换）。
    NewDraft {
        workspace: WorkspaceRef,
        switched: bool,
    },
    /// `/workspace`（无参）：允许列表 + 当前工作区 id。
    WorkspaceList {
        workspaces: Vec<WorkspaceRef>,
        current: Option<String>,
    },
    /// `/workspace <值>`：解析成功。
    WorkspaceSelected { workspace: WorkspaceRef },
    /// `/workspace <值>`：解析失败（附带可选列表）。
    WorkspaceNotFound {
        value: String,
        workspaces: Vec<WorkspaceRef>,
    },
    /// `/status`：上下文快照。
    Status {
        workspace_path: String,
        workspace_identity: Option<String>,
        mode: String,
        active_task_id: Option<String>,
        queued_messages: usize,
        pending_permission: bool,
        pending_elicitation: bool,
    },
}

fn current_workspace_id(context: &Value) -> Option<String> {
    let workspace_path = context.get("workspacePath").and_then(Value::as_str)?;
    let identity = context.get("workspaceIdentity").and_then(Value::as_str);
    Some(workspace_key(workspace_path, identity))
}

/// `/new [工作区]`：切换（可选）并进入新草稿；`draft_options` 由宿主按新工作区初始化。
pub fn handle_new(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    workspaces: &[WorkspaceRef],
    allowed_ids: &[String],
    requested_workspace: Option<&str>,
    draft_options: Option<Value>,
) -> Result<CommandReply, String> {
    let current = repo.read_state()?;
    let current_id = current
        .get("bots")
        .and_then(|bots| bots.get(bot_id))
        .and_then(|bot| bot.get("conversations"))
        .and_then(|conversations| conversations.get(conversation_key))
        .and_then(current_workspace_id);
    let allowed = filter_allowed_workspaces(workspaces, allowed_ids);
    let target = match requested_workspace {
        Some(value) => resolve_workspace_by_value(workspaces, value, allowed_ids).cloned(),
        None => allowed.first().map(|workspace| (*workspace).clone()),
    };
    let Some(target) = target else {
        if let Some(value) = requested_workspace {
            return Ok(CommandReply::WorkspaceNotFound {
                value: value.to_string(),
                workspaces: allowed.into_iter().cloned().collect(),
            });
        }
        return Err("Bot 没有可用工作区（allowedWorkspaces 为空或未绑定）".to_string());
    };
    let switched = current_id.as_deref() != Some(target.id.as_str());
    switch_workspace_context(repo, bot_id, conversation_key, &target, draft_options)?;
    Ok(CommandReply::NewDraft {
        workspace: target,
        switched,
    })
}

/// `/workspace`（无参）：允许列表 + 当前选中值。
pub fn handle_workspace_list(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    workspaces: &[WorkspaceRef],
    allowed_ids: &[String],
) -> Result<CommandReply, String> {
    let current = read_context(repo, bot_id, conversation_key)?;
    Ok(CommandReply::WorkspaceList {
        workspaces: filter_allowed_workspaces(workspaces, allowed_ids)
            .into_iter()
            .cloned()
            .collect(),
        current: current.as_ref().and_then(current_workspace_id),
    })
}

/// `/workspace <值>`：解析成功切到该工作区并进入草稿；失败返回可选列表。
pub fn handle_workspace_set(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    workspaces: &[WorkspaceRef],
    allowed_ids: &[String],
    value: &str,
    draft_options: Option<Value>,
) -> Result<CommandReply, String> {
    match resolve_workspace_by_value(workspaces, value, allowed_ids) {
        Some(workspace) => {
            let workspace = workspace.clone();
            switch_workspace_context(repo, bot_id, conversation_key, &workspace, draft_options)?;
            Ok(CommandReply::WorkspaceSelected { workspace })
        }
        None => Ok(CommandReply::WorkspaceNotFound {
            value: value.to_string(),
            workspaces: filter_allowed_workspaces(workspaces, allowed_ids)
                .into_iter()
                .cloned()
                .collect(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeCommandReply {
    /// Bot 硬锁 yolo：任何 `/mode` 切换都拒绝（TS 同口径）。
    Locked,
    Set { mode: String },
    List {
        options: Vec<crate::reply::SelectOption>,
        current: Option<String>,
    },
}

/// `/mode` 与 `/mode <值>`：选项由宿主从 agent config options 注入。
pub fn handle_mode(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    options: &[crate::reply::SelectOption],
    current: Option<&str>,
    value: Option<&str>,
) -> Result<ModeCommandReply, String> {
    let _ = (repo, bot_id, conversation_key);
    if value.is_none() {
        return Ok(ModeCommandReply::Locked);
    }
    let _ = (options, current);
    Ok(ModeCommandReply::Locked)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCommandReply {
    Setted { task_id: String },
    List {
        tasks: Vec<crate::reply::SelectOption>,
        active_task_id: Option<String>,
    },
}

/// `/task` 与 `/task <值>`：候选由宿主注入（任务索引），命中后绑定任务上下文。
pub fn handle_task(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    tasks: &[crate::reply::SelectOption],
    value: Option<&str>,
) -> Result<TaskCommandReply, String> {
    let context = read_context(repo, bot_id, conversation_key)?;
    let active_task_id = context
        .as_ref()
        .and_then(|context| context.get("activeTaskId"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(value) = value else {
        return Ok(TaskCommandReply::List {
            tasks: tasks.to_vec(),
            active_task_id,
        });
    };
    let Some(option) = crate::reply::resolve_option_by_value(tasks, value) else {
        return Ok(TaskCommandReply::List {
            tasks: tasks.to_vec(),
            active_task_id,
        });
    };
    crate::conversation::write_task_context(repo, bot_id, conversation_key, &option.id)?;
    Ok(TaskCommandReply::Setted {
        task_id: option.id.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCommandReply {
    Set {
        provider_id: String,
        model_id: String,
        reasoning_level: String,
    },
    List {
        options: Vec<crate::reply::SelectOption>,
        current: Option<String>,
    },
}

/// `/model [provider/model]`：候选由宿主注入（provider 模型目录）；
/// `complete_reasoning(provider, model)` 由宿主按 ModelSelectionView 补最高档。
/// 命中后把完整 Selection（provider/model/最高档）写入 `draftOptions.modelSelection`。
pub fn handle_model(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    options: &[crate::reply::SelectOption],
    current: Option<&str>,
    value: Option<&str>,
    complete_reasoning: &dyn Fn(&str, &str) -> Option<String>,
) -> Result<ModelCommandReply, String> {
    let Some(value) = value else {
        return Ok(ModelCommandReply::List {
            options: options.to_vec(),
            current: current.map(str::to_string),
        });
    };
    let Some(option) = crate::reply::resolve_option_by_value(options, value) else {
        return Ok(ModelCommandReply::List {
            options: options.to_vec(),
            current: current.map(str::to_string),
        });
    };
    let Some((provider_id, model_id)) = option.id.split_once('/') else {
        return Ok(ModelCommandReply::List {
            options: options.to_vec(),
            current: current.map(str::to_string),
        });
    };
    // 旧菜单失效/目录读取失败不能清掉已保存选择：没有最高档就不落盘。
    let Some(reasoning_level) = complete_reasoning(provider_id, model_id) else {
        return Ok(ModelCommandReply::List {
            options: options.to_vec(),
            current: current.map(str::to_string),
        });
    };
    let selection = serde_json::json!({
        "providerId": provider_id,
        "modelId": model_id,
        "options": { "reasoningLevel": reasoning_level },
    });
    crate::conversation::set_draft_option(
        repo,
        bot_id,
        conversation_key,
        "modelSelection",
        selection,
    )?;
    Ok(ModelCommandReply::Set {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        reasoning_level,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingPermissionOption {
    pub request_id: String,
    pub option_id: String,
    pub command: String,
    pub label: String,
    pub response: Value,
    pub handled_at: Option<i64>,
}

fn parse_pending_permissions(context: &Value) -> Vec<PendingPermissionOption> {
    context
        .get("pendingPermissionOptions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(PendingPermissionOption {
                        request_id: item.get("requestId")?.as_str()?.to_string(),
                        option_id: item.get("optionId")?.as_str()?.to_string(),
                        command: item
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("approve")
                            .to_string(),
                        label: item
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        response: item.get("response").cloned().unwrap_or(Value::Null),
                        handled_at: item.get("handledAt").and_then(Value::as_i64),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionReply {
    /// 审批已提交（host 已收到 v4 ACK）。
    Submitted { request_id: String, denied: bool },
    /// 该条已处理过（重复点击/重投）。
    AlreadyHandled,
    /// 序号无效或上下文没有 pending 审批。
    Expired,
}

/// `/permission <序号>`：按 1 起的序号选 pending 审批选项；只有 host 的 v4 应答
/// 返回 true 才写入 handledAt（ACK 失败必须允许用户重试）。
pub async fn handle_permission(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    value: &str,
    respond: &(dyn Fn(&PendingPermissionOption) -> crate::weixin_runtime::RuntimeFuture<'_, Result<bool, String>>
             + Send
             + Sync),
) -> Result<PermissionReply, String> {
    let Some(context) = read_context(repo, bot_id, conversation_key)? else {
        return Ok(PermissionReply::Expired);
    };
    let options = parse_pending_permissions(&context);
    let Ok(index) = value.trim().parse::<usize>() else {
        return Ok(PermissionReply::Expired);
    };
    if index == 0 {
        return Ok(PermissionReply::Expired);
    }
    let Some(option) = options.get(index - 1) else {
        return Ok(PermissionReply::Expired);
    };
    if option.handled_at.is_some() {
        return Ok(PermissionReply::AlreadyHandled);
    }
    let submitted = respond(option).await?;
    if !submitted {
        return Ok(PermissionReply::AlreadyHandled);
    }
    crate::conversation::mark_permission_handled(
        repo,
        bot_id,
        conversation_key,
        &option.request_id,
        now_millis(),
    )?;
    Ok(PermissionReply::Submitted {
        request_id: option.request_id.clone(),
        denied: option.command == "deny",
    })
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// `/send <文本>`：把文本作为回合提示交给宿主执行（本层只做校验与意图）。
pub fn handle_send(value: &str) -> Result<String, String> {
    let text = value.trim();
    if text.is_empty() {
        return Err("send 需要非空文本".to_string());
    }
    Ok(text.to_string())
}

/// `/elicitation`（submit 已在命令解析分派）：回答值交给宿主提交。
pub fn handle_elicitation_respond(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("elicitation 回答不能为空".to_string());
    }
    Ok(value.to_string())
}

/// `/status`：读上下文快照（不触发运行态查询）。
pub fn handle_status(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
) -> Result<CommandReply, String> {
    let Some(context) = read_context(repo, bot_id, conversation_key)? else {
        return Err("会话上下文不存在（先发送一条消息建立会话）".to_string());
    };
    Ok(CommandReply::Status {
        workspace_path: context
            .get("workspacePath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        workspace_identity: context
            .get("workspaceIdentity")
            .and_then(Value::as_str)
            .map(str::to_string),
        mode: context
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("draft")
            .to_string(),
        active_task_id: context
            .get("activeTaskId")
            .and_then(Value::as_str)
            .map(str::to_string),
        queued_messages: context
            .get("queuedMessages")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        pending_permission: context
            .get("pendingPermissionOptions")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty()),
        pending_elicitation: context.get("pendingElicitation").is_some(),
    })
}

fn read_context(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
) -> Result<Option<Value>, String> {
    let state = repo.read_state()?;
    Ok(state
        .get("bots")
        .and_then(|bots| bots.get(bot_id))
        .and_then(|bot| bot.get("conversations"))
        .and_then(|conversations| conversations.get(conversation_key))
        .cloned())
}

/// `/new` 的等效状态变更（无工作区切换诉求时直接进草稿）。
pub fn handle_draft_reset(
    repo: &BotsRepo,
    bot_id: &str,
    conversation_key: &str,
    draft_options: Option<Value>,
) -> Result<Value, String> {
    write_draft_context(repo, bot_id, conversation_key, draft_options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::create_workspace_ref;
    use std::path::PathBuf;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("commands-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn workspaces() -> Vec<WorkspaceRef> {
        vec![
            create_workspace_ref("D:/ws-a", None),
            create_workspace_ref("D:/ws-b", None),
        ]
    }

    #[test]
    fn new_switches_workspace_and_resets_queue() {
        let base = sandbox("new");
        let repo = BotsRepo::new(&base);
        repo.mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["c1"] = serde_json::json!({
                "workspacePath": "D:/ws-a",
                "workspaceId": "D:/ws-a",
                "mode": "task",
                "activeTaskId": "task-1",
                "queuedMessages": [{ "text": "q" }]
            });
            Ok(())
        })
        .unwrap();
        let reply = handle_new(
            &repo,
            "b1",
            "c1",
            &workspaces(),
            &[],
            Some("ws-b"),
            Some(serde_json::json!({ "model": "m" })),
        )
        .unwrap();
        match reply {
            CommandReply::NewDraft { workspace, switched } => {
                assert_eq!(workspace.id, "D:/ws-b");
                assert!(switched);
            }
            other => panic!("unexpected {other:?}"),
        }
        let context = read_context(&repo, "b1", "c1").unwrap().unwrap();
        assert_eq!(context["mode"], "draft");
        assert_eq!(context["workspacePath"], "D:/ws-b");
        assert!(context["activeTaskId"].is_null());
        assert!(context.get("queuedMessages").is_none());
        assert_eq!(context["draftOptions"]["model"], "m");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn workspace_list_and_set_resolve_by_index() {
        let base = sandbox("workspace");
        let repo = BotsRepo::new(&base);
        let reply = handle_workspace_list(&repo, "b1", "c1", &workspaces(), &[]).unwrap();
        match reply {
            CommandReply::WorkspaceList { workspaces, current } => {
                assert_eq!(workspaces.len(), 2);
                assert!(current.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
        let reply = handle_workspace_set(&repo, "b1", "c1", &workspaces(), &[], "2", None).unwrap();
        match reply {
            CommandReply::WorkspaceSelected { workspace } => assert_eq!(workspace.id, "D:/ws-b"),
            other => panic!("unexpected {other:?}"),
        }
        // 不存在的值返回可选列表。
        let reply =
            handle_workspace_set(&repo, "b1", "c1", &workspaces(), &[], "nope", None).unwrap();
        assert!(matches!(
            reply,
            CommandReply::WorkspaceNotFound { workspaces, .. } if workspaces.len() == 2
        ));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn mode_and_task_commands_resolve_and_persist() {
        let base = sandbox("mode-task");
        let repo = BotsRepo::new(&base);
        let modes = vec![
            crate::reply::SelectOption {
                id: "code".to_string(),
                label: "Code".to_string(),
            },
            crate::reply::SelectOption {
                id: "chat".to_string(),
                label: "Chat".to_string(),
            },
        ];
        // Bot 硬锁 yolo：任何 /mode 切换/列表都返回 Locked，且不写状态。
        assert_eq!(
            handle_mode(&repo, "b1", "c1", &modes, None, Some("2")).unwrap(),
            ModeCommandReply::Locked
        );
        assert_eq!(
            handle_mode(&repo, "b1", "c1", &modes, Some("chat"), Some("nope")).unwrap(),
            ModeCommandReply::Locked
        );
        assert!(read_context(&repo, "b1", "c1").unwrap().is_none());
        let tasks = vec![crate::reply::SelectOption {
            id: "task-9".to_string(),
            label: "修复登录".to_string(),
        }];
        match handle_task(&repo, "b1", "c1", &tasks, Some("修复登录")).unwrap() {
            TaskCommandReply::Setted { task_id } => assert_eq!(task_id, "task-9"),
            other => panic!("unexpected {other:?}"),
        }
        let context = read_context(&repo, "b1", "c1").unwrap().unwrap();
        assert_eq!(context["mode"], "task");
        assert_eq!(context["activeTaskId"], "task-9");
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn model_command_writes_full_selection_with_highest_level() {
        let base = sandbox("model");
        let repo = BotsRepo::new(&base);
        let options = vec![
            crate::reply::SelectOption {
                id: "deepseek/deepseek-flash".to_string(),
                label: "DeepSeek · deepseek-flash".to_string(),
            },
            crate::reply::SelectOption {
                id: "tokendance/glm-5.3".to_string(),
                label: "TokenDance · glm-5.3".to_string(),
            },
        ];
        let complete = |provider: &str, model: &str| {
            (provider == "deepseek" && model == "deepseek-flash").then(|| "max".to_string())
        };
        match handle_model(&repo, "b1", "c1", &options, None, Some("1"), &complete).unwrap() {
            ModelCommandReply::Set {
                provider_id,
                model_id,
                reasoning_level,
            } => {
                assert_eq!(provider_id, "deepseek");
                assert_eq!(model_id, "deepseek-flash");
                assert_eq!(reasoning_level, "max");
            }
            other => panic!("unexpected {other:?}"),
        }
        let context = read_context(&repo, "b1", "c1").unwrap().unwrap();
        assert_eq!(
            context["draftOptions"]["modelSelection"]["options"]["reasoningLevel"],
            "max"
        );
        // 目录缺失最高档：不落盘，回列表。
        assert!(matches!(
            handle_model(&repo, "b1", "c1", &options, None, Some("glm-5.3"), &complete).unwrap(),
            ModelCommandReply::List { .. }
        ));
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn permission_respond_marks_only_after_ack() {
        let base = sandbox("permission");
        let repo = BotsRepo::new(&base);
        repo.mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["c1"] = serde_json::json!({
                "workspacePath": "D:/ws",
                "activeTaskId": "task-1",
                "pendingPermissionOptions": [
                    { "requestId": "r1", "optionId": "opt-deny", "command": "deny", "label": "拒绝", "response": { "type": "deny" } },
                    { "requestId": "r2", "optionId": "opt-ok", "command": "approve", "label": "允许", "response": { "type": "approve" } }
                ]
            });
            Ok(())
        })
        .unwrap();
        // 序号越界 / 非数字 → expired。
        assert_eq!(
            handle_permission(&repo, "b1", "c1", "9", &|_| Box::pin(async { Ok(true) })).await.unwrap(),
            PermissionReply::Expired
        );
        // ACK 失败：不写 handledAt，可重试。
        let failed = handle_permission(&repo, "b1", "c1", "2", &|_| Box::pin(async { Ok(false) })).await.unwrap();
        assert_eq!(failed, PermissionReply::AlreadyHandled);
        let context = read_context(&repo, "b1", "c1").unwrap().unwrap();
        assert!(context["pendingPermissionOptions"][1].get("handledAt").is_none());
        // ACK 成功：写 handledAt；重复点击 → already。
        let ok = handle_permission(&repo, "b1", "c1", "2", &|option| {
            let request_id = option.request_id.clone();
            Box::pin(async move {
                assert_eq!(request_id, "r2");
                Ok(true)
            })
        })
        .await
        .unwrap();
        assert_eq!(
            ok,
            PermissionReply::Submitted {
                request_id: "r2".to_string(),
                denied: false
            }
        );
        let context = read_context(&repo, "b1", "c1").unwrap().unwrap();
        assert!(context["pendingPermissionOptions"][1]["handledAt"].is_number());
        assert_eq!(
            handle_permission(&repo, "b1", "c1", "2", &|_| Box::pin(async { Ok(true) })).await.unwrap(),
            PermissionReply::AlreadyHandled
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn send_and_elicitation_validate_values() {
        assert_eq!(handle_send(" hello ").unwrap(), "hello");
        assert!(handle_send("  ").is_err());
        assert_eq!(handle_elicitation_respond("42").unwrap(), "42");
        assert!(handle_elicitation_respond("").is_err());
    }

    #[test]
    fn status_reports_context_snapshot() {
        let base = sandbox("status");
        let repo = BotsRepo::new(&base);
        assert!(handle_status(&repo, "b1", "c1").is_err());
        repo.mutate_state(|state| {
            state["bots"]["b1"]["conversations"]["c1"] = serde_json::json!({
                "workspacePath": "D:/ws-a",
                "mode": "task",
                "activeTaskId": "task-1",
                "queuedMessages": [{ "text": "q" }],
                "pendingPermissionOptions": [{ "requestId": "r1" }]
            });
            Ok(())
        })
        .unwrap();
        match handle_status(&repo, "b1", "c1").unwrap() {
            CommandReply::Status {
                mode,
                active_task_id,
                queued_messages,
                pending_permission,
                ..
            } => {
                assert_eq!(mode, "task");
                assert_eq!(active_task_id.as_deref(), Some("task-1"));
                assert_eq!(queued_messages, 1);
                assert!(pending_permission);
            }
            other => panic!("unexpected {other:?}"),
        }
        std::fs::remove_dir_all(base).ok();
    }
}
