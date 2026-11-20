//! 设置 →「连接器」的数据面：复用 `connectors-service` 的目录/状态，
//! 密钥读取复用 `crate::secrets`（同一 `v2/secrets.json`）。

use std::path::Path;

use connectors_service::{
    read_state, resolve_mcp_servers, spec, state_path, write_state, ConnectorMcpServer,
    ConnectorSpec, ConnectorState,
};

/// 当前启用清单（未知 id 已在 crate 内过滤）。
pub fn state(data_root: &Path) -> ConnectorState {
    read_state(&state_path(data_root))
}

/// 启停一个连接器；未知 id 报错。返回最新状态。
pub fn set_enabled(
    data_root: &Path,
    id: &str,
    enabled: bool,
) -> Result<ConnectorState, String> {
    if spec(id).is_none() {
        return Err(format!("未知连接器 {id}"));
    }
    let file = state_path(data_root);
    let mut state = read_state(&file);
    state.enabled.retain(|item| item != id);
    if enabled {
        state.enabled.push(id.to_string());
    }
    write_state(&file, &state)?;
    Ok(state)
}

/// 该连接器缺少的密钥名（按目录顺序）。
pub fn missing_secrets(spec: &ConnectorSpec, configured: &[String]) -> Vec<&'static str> {
    spec.secrets
        .iter()
        .filter(|secret| !configured.iter().any(|name| name == secret.name))
        .map(|secret| secret.name)
        .collect()
}

/// 回合 `createSession.mcpServers` 的 payload（与壳协议 stdio 形状一致）。
pub fn payload(servers: Vec<ConnectorMcpServer>) -> Vec<serde_json::Value> {
    servers
        .into_iter()
        .map(|server| {
            serde_json::json!({
                "name": server.name,
                "command": server.command,
                "args": server.args,
                "env": server.env.into_iter()
                    .map(|(name, value)| serde_json::json!({ "name": name, "value": value }))
                    .collect::<Vec<_>>(),
                "isolation": "session",
            })
        })
        .collect()
}

/// 从数据根读启用清单 + 密钥并解析成 payload；缺密钥/坏库按空清单处理（与壳一致）。
pub fn payload_for_workspace(data_root: &Path, workspace: &str) -> Vec<serde_json::Value> {
    let state = state(data_root);
    if state.enabled.is_empty() {
        return Vec::new();
    }
    let secrets = match crate::secrets::values(data_root) {
        Ok(values) => values,
        Err(error) => {
            eprintln!("[connectors] 密钥库不可用：{error}");
            return Vec::new();
        }
    };
    match resolve_mcp_servers(&state, &secrets, Some(workspace)) {
        Ok(servers) => payload(servers),
        Err(error) => {
            eprintln!("[connectors] MCP 解析失败（本轮跳过）：{error}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot {
        dir: std::path::PathBuf,
    }

    impl TempRoot {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "bot-gpui-connectors-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self { dir }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn toggle_writes_shared_state_file() {
        let temp = TempRoot::new();
        assert!(state(&temp.dir).enabled.is_empty());
        let next = set_enabled(&temp.dir, "github", true).expect("enable");
        assert_eq!(next.enabled, vec!["github".to_string()]);
        let file = state_path(&temp.dir);
        assert!(file.ends_with("v2/connectors.json"), "{}", file.display());
        let raw = std::fs::read_to_string(&file).expect("read");
        assert!(raw.contains("\"github\""));
        let next = set_enabled(&temp.dir, "github", false).expect("disable");
        assert!(next.enabled.is_empty());
    }

    #[test]
    fn unknown_connector_is_rejected() {
        let temp = TempRoot::new();
        assert!(set_enabled(&temp.dir, "nope", true).is_err());
        assert!(state(&temp.dir).enabled.is_empty());
    }

    #[test]
    fn missing_secrets_are_reported_in_catalog_order() {
        let github = spec("github").expect("github");
        assert_eq!(
            missing_secrets(github, &[]),
            vec!["CONNECTOR_GITHUB_TOKEN"]
        );
        assert!(
            missing_secrets(github, &["CONNECTOR_GITHUB_TOKEN".to_string()]).is_empty()
        );
        let slack = spec("slack").expect("slack");
        assert_eq!(
            missing_secrets(slack, &["CONNECTOR_SLACK_BOT_TOKEN".to_string()]),
            vec!["CONNECTOR_SLACK_TEAM_ID"]
        );
    }

    #[test]
    fn catalog_has_expected_shape() {
        assert_eq!(connectors_service::CATALOG.len(), 16);
        assert!(spec("filesystem").expect("filesystem").secrets.is_empty());
    }

    #[test]
    fn payload_uses_protocol_stdio_shape() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("GITHUB_PERSONAL_ACCESS_TOKEN".to_string(), "tok".to_string());
        let payload = payload(vec![ConnectorMcpServer {
            name: "github".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "server-github".to_string()],
            env,
            transport: "stdio",
        }]);
        let first = &payload[0];
        assert_eq!(first["name"], "github");
        assert_eq!(first["command"], "npx");
        assert_eq!(first["args"][1], "server-github");
        assert_eq!(first["env"][0]["name"], "GITHUB_PERSONAL_ACCESS_TOKEN");
        assert_eq!(first["env"][0]["value"], "tok");
        assert_eq!(first["isolation"], "session");
    }

    #[test]
    fn payload_for_workspace_reads_state_and_secrets() {
        let temp = TempRoot::new();
        set_enabled(&temp.dir, "github", true).expect("enable");
        crate::secrets::set(&temp.dir, "CONNECTOR_GITHUB_TOKEN", "tok").expect("secret");
        let payload = payload_for_workspace(&temp.dir, "C:\\ws");
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0]["name"], "github");
        // 缺密钥时整轮跳过（与壳一致），不返回半截 server。
        crate::secrets::remove(&temp.dir, "CONNECTOR_GITHUB_TOKEN").expect("remove");
        assert!(payload_for_workspace(&temp.dir, "C:\\ws").is_empty());
    }
}
