//! automation 协议桥（GPUI 侧）：回答 agent 的 `automation/*` 请求。
//!
//! `CronCreate/CronList/CronUpdate/CronDelete` 由 `mode-agent` 直接发协议方法；
//! 宿主必须按 session 注入 workspace（协议参数只带 `targetTaskId`），这里用
//! `automation_scope` 找回 Bot 目录/身份，再复用 `automation-service` 的调度语义。

use automation_repo::{Automation, AutomationRepo, AutomationScope};
use automation_service::{AutomationService, CreateParams, UpdateParams};
use mode_protocol_client::RequestReply;
use serde_json::{Value, json};

pub fn handle_request(
    service: &AutomationService,
    repo: &AutomationRepo,
    request: &Value,
) -> Option<RequestReply> {
    let method = request.get("method").and_then(Value::as_str)?;
    if !method.starts_with("automation/") {
        return None;
    }
    let params = request
        .get("params")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Some(match method {
        "automation/checkTaskBinding" => check_binding(repo, &params),
        "automation/list" => list(service),
        "automation/create" => create(service, &params),
        "automation/update" => update(service, repo, &params),
        "automation/delete" => delete(service, repo, &params),
        "automation/listAutomationRuns" => list_runs(service, repo, &params),
        other => fail(-32601, format!("automation method not supported: {other}")),
    })
}

fn check_binding(repo: &AutomationRepo, params: &Value) -> RequestReply {
    let Some(target) = params.get("targetTaskId").and_then(Value::as_str) else {
        return fail(-32602, "targetTaskId is required");
    };
    match repo.list(&AutomationScope::default()) {
        Ok(rows) => result(json!({
            "bound": rows.iter().any(|row| row.target_task_id.as_deref() == Some(target))
        })),
        Err(error) => fail(-32603, error.to_string()),
    }
}

fn list(service: &AutomationService) -> RequestReply {
    match service.list_all() {
        Ok(automations) => result(json!({ "automations": automations })),
        Err(error) => fail(-32603, error),
    }
}

fn create(service: &AutomationService, params: &Value) -> RequestReply {
    let scope = match scope_for_target(params.get("targetTaskId").and_then(Value::as_str)) {
        Ok(scope) => scope,
        Err(message) => return fail(-32602, message),
    };
    let mut payload = params.clone();
    if let Some(object) = payload.as_object_mut() {
        object.remove("workspacePath");
        object.remove("workspaceIdentity");
    }
    let create: CreateParams = match serde_json::from_value(payload) {
        Ok(create) => create,
        Err(error) => return fail(-32602, format!("createAutomation 参数非法：{error}")),
    };
    match service.create(&create, &scope, now_ms()) {
        Ok(automation) => result(json!({ "automation": automation })),
        Err(error) => fail(-32603, error),
    }
}

fn update(service: &AutomationService, repo: &AutomationRepo, params: &Value) -> RequestReply {
    let Some(automation_id) = params.get("automationId").and_then(Value::as_str) else {
        return fail(-32602, "automationId is required");
    };
    let existing = match find_automation(repo, automation_id) {
        Ok(existing) => existing,
        Err(message) => return fail(-32602, message),
    };
    let scope = scope_of(&existing);
    let mut payload = params.clone();
    if let Some(object) = payload.as_object_mut() {
        object.remove("automationId");
        object.remove("workspacePath");
        object.remove("workspaceIdentity");
    }
    let update: UpdateParams = match serde_json::from_value(payload) {
        Ok(update) => update,
        Err(error) => return fail(-32602, format!("updateAutomation 参数非法：{error}")),
    };
    match service.update(automation_id, &update, &scope, now_ms()) {
        Ok(Some(automation)) => result(json!({ "automation": automation })),
        Ok(None) => fail(-32602, format!("automation not found: {automation_id}")),
        Err(error) => fail(-32603, error),
    }
}

fn delete(service: &AutomationService, repo: &AutomationRepo, params: &Value) -> RequestReply {
    let Some(automation_id) = params.get("automationId").and_then(Value::as_str) else {
        return fail(-32602, "automationId is required");
    };
    let existing = match find_automation(repo, automation_id) {
        Ok(existing) => existing,
        Err(message) => return fail(-32602, message),
    };
    match service.delete(automation_id, &scope_of(&existing)) {
        Ok(_) => result(json!({ "deleted": true })),
        Err(error) => fail(-32603, error),
    }
}

fn list_runs(
    service: &AutomationService,
    repo: &AutomationRepo,
    params: &Value,
) -> RequestReply {
    let Some(automation_id) = params.get("automationId").and_then(Value::as_str) else {
        return fail(-32602, "automationId is required");
    };
    let existing = match find_automation(repo, automation_id) {
        Ok(existing) => existing,
        Err(message) => return fail(-32602, message),
    };
    match service.list_runs(automation_id, &scope_of(&existing)) {
        Ok(runs) => result(json!({ "runs": runs })),
        Err(error) => fail(-32603, error),
    }
}

fn find_automation(repo: &AutomationRepo, automation_id: &str) -> Result<Automation, String> {
    repo.list(&AutomationScope::default())
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|row| row.automation_id == automation_id)
        .ok_or_else(|| format!("automation not found: {automation_id}"))
}

fn scope_of(automation: &Automation) -> AutomationScope {
    AutomationScope {
        workspace_path: Some(automation.workspace_path.clone()),
        workspace_identity: automation.workspace_identity.clone(),
    }
}

fn scope_for_target(target_task_id: Option<&str>) -> Result<AutomationScope, String> {
    let scope = target_task_id
        .and_then(crate::automation_scope::lookup)
        .or_else(|| crate::automation_scope::lookup(""));
    let Some(scope) = scope else {
        return Err(
            "无法确定 automation 的 workspace：会话未登记作用域，请先打开 Bot 会话再创建"
                .to_string(),
        );
    };
    Ok(AutomationScope {
        workspace_path: Some(scope.workspace_path),
        workspace_identity: scope.workspace_identity,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn result(value: Value) -> RequestReply {
    RequestReply::Result(value)
}

fn fail(code: i64, message: impl Into<String>) -> RequestReply {
    RequestReply::Error {
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use automation_service::AutomationServiceOptions;

    fn temp_db(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gpui-automation-bridge-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join("tasks-index.sqlite")
    }

    #[test]
    fn create_list_and_binding_roundtrip() {
        let db = temp_db("roundtrip");
        let repo = AutomationRepo::with_busy_timeout(db.clone(), 5_000);
        let service = AutomationService::new(
            AutomationRepo::with_busy_timeout(db, 5_000),
            AutomationServiceOptions { offset_secs: 0 },
        );
        let session = "bridge-sess-roundtrip";
        let workspace = "C:/bridge-ws";
        crate::automation_scope::register(session, workspace, Some("id".to_string()));

        let create = json!({
            "method": "automation/create",
            "params": {
                "targetTaskId": session,
                "title": "bridge task",
                "cronExpr": "0 0 1 1 *",
                "prompt": "hi",
                "recurring": true,
            }
        });
        let created = handle_request(&service, &repo, &create).expect("handled");
        let RequestReply::Result(value) = created else {
            panic!("create failed: {created:?}");
        };
        assert_eq!(value["automation"]["title"], "bridge task");

        let list = json!({"method": "automation/list", "params": {}});
        let RequestReply::Result(value) = handle_request(&service, &repo, &list).expect("handled")
        else {
            panic!("list failed");
        };
        assert_eq!(value["automations"].as_array().map(Vec::len), Some(1));

        let binding = json!({
            "method": "automation/checkTaskBinding",
            "params": { "targetTaskId": session }
        });
        let RequestReply::Result(value) =
            handle_request(&service, &repo, &binding).expect("handled")
        else {
            panic!("binding failed");
        };
        assert_eq!(value["bound"], true);
    }
}
