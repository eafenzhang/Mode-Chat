//! Agent 会话 → automation workspace 作用域映射。
//!
//! `automation/create` 的协议参数不带 workspace（由宿主按 session 注入）。
//! GPUI 在会话 Ready 时把 sessionId 映射到 Bot 的目录/身份；聊天里 CronCreate
//! 的 `targetTaskId` 就能找回正确 scope。另存一个「最近活跃」兜底给无映射的调用。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
#[allow(dead_code)]
pub struct Scope {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
}

fn sessions() -> &'static Mutex<HashMap<String, Scope>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Scope>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn active() -> &'static Mutex<Option<Scope>> {
    static ACTIVE: OnceLock<Mutex<Option<Scope>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

pub fn register(session_id: &str, workspace_path: &str, workspace_identity: Option<String>) {
    if session_id.trim().is_empty() || workspace_path.trim().is_empty() {
        return;
    }
    if let Ok(mut map) = sessions().lock() {
        map.insert(
            session_id.to_string(),
            Scope {
                workspace_path: workspace_path.to_string(),
                workspace_identity,
            },
        );
    }
}

pub fn set_active(workspace_path: &str, workspace_identity: Option<String>) {
    if workspace_path.trim().is_empty() {
        return;
    }
    if let Ok(mut slot) = active().lock() {
        *slot = Some(Scope {
            workspace_path: workspace_path.to_string(),
            workspace_identity,
        });
    }
}

#[allow(dead_code)]
pub fn lookup(session_id: &str) -> Option<Scope> {
    if let Ok(map) = sessions().lock()
        && let Some(scope) = map.get(session_id)
    {
        return Some(scope.clone());
    }
    active().lock().ok().and_then(|slot| slot.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup_prefers_session() {
        register("sess-test", "C:/ws", Some("id".to_string()));
        let scope = lookup("sess-test").expect("scope");
        assert_eq!(scope.workspace_path, "C:/ws");
        assert_eq!(scope.workspace_identity.as_deref(), Some("id"));
    }
}
