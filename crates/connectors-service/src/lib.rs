//! 连接器目录 + 启用状态 + MCP server 解析。
//!
//! 产品口径（2026-11-12 v1）：DS-Bot 的连接器是「一个第三方能力 + 一条授权」；Mode 侧
//! 能等价承载的是 **MCP 服务器**——目录里的每一项就是一组 `command/args/env`，授权信息
//! 走本机密钥库（`${secret:NAME}` 占位），启用清单落在
//! `{data_root}/v2/connectors.json`（`data_root` 是 `.mode` 根，团队级）。
//!
//! 本 crate 只描述「怎么起服务」，不猜工具清单；缺密钥是硬错误并点名，不静默空跑。
//! Tauri 命令 / 环境变量 / MCP payload 组装留在壳侧。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use atomic_file::write_json_atomic;
use serde::{Deserialize, Serialize};

/// 目录里的一个密钥字段（渲染成一行密码输入）。
pub struct ConnectorSecret {
    pub name: &'static str,
    pub label: &'static str,
}

/// 目录条目：id 与 DS-Bot 对齐，字段够渲染卡片、够起一个 MCP 服务。
pub struct ConnectorSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// dev / docs / work / data / search / design
    pub category: &'static str,
    pub about: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    /// (env 名, 密钥库里的名字)
    pub env: &'static [(&'static str, &'static str)],
    pub secrets: &'static [ConnectorSecret],
}

const fn secret(name: &'static str, label: &'static str) -> ConnectorSecret {
    ConnectorSecret { name, label }
}

/// v1 目录：只收「有官方 MCP server、凭据是静态密钥」的连接器。
/// OAuth/浏览器回调类（Google Drive、Figma、Linear 等）等授权链路，先不列进来。
pub const CATALOG: &[ConnectorSpec] = &[
    ConnectorSpec {
        id: "github",
        name: "GitHub",
        category: "dev",
        about: "仓库、Issue、PR 与代码检索",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-github"],
        env: &[(
            "GITHUB_PERSONAL_ACCESS_TOKEN",
            "CONNECTOR_GITHUB_TOKEN",
        )],
        secrets: &[secret("CONNECTOR_GITHUB_TOKEN", "Personal access token")],
    },
    ConnectorSpec {
        id: "notion",
        name: "Notion",
        category: "docs",
        about: "页面与数据库读取、检索",
        command: "npx",
        args: &["-y", "@notionhq/notion-mcp-server"],
        env: &[("NOTION_TOKEN", "CONNECTOR_NOTION_TOKEN")],
        secrets: &[secret("CONNECTOR_NOTION_TOKEN", "Integration token")],
    },
    ConnectorSpec {
        id: "slack",
        name: "Slack",
        category: "work",
        about: "频道消息读取与发送",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-slack"],
        env: &[
            ("SLACK_BOT_TOKEN", "CONNECTOR_SLACK_BOT_TOKEN"),
            ("SLACK_TEAM_ID", "CONNECTOR_SLACK_TEAM_ID"),
        ],
        secrets: &[
            secret("CONNECTOR_SLACK_BOT_TOKEN", "Bot token"),
            secret("CONNECTOR_SLACK_TEAM_ID", "Team ID"),
        ],
    },
    ConnectorSpec {
        id: "postgres",
        name: "PostgreSQL",
        category: "data",
        about: "只读查询数据库结构与数据",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-postgres"],
        env: &[("DATABASE_URL", "CONNECTOR_POSTGRES_URL")],
        secrets: &[secret("CONNECTOR_POSTGRES_URL", "Connection string")],
    },
    ConnectorSpec {
        id: "filesystem",
        name: "Filesystem",
        category: "docs",
        about: "把 Bot 的工作目录挂成可读写文件工具",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-filesystem", "${workspace}"],
        env: &[],
        secrets: &[],
    },
    ConnectorSpec {
        id: "redis",
        name: "Redis",
        category: "data",
        about: "键值查询与调试（需连接串）",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-redis"],
        env: &[("REDIS_URL", "CONNECTOR_REDIS_URL")],
        secrets: &[secret("CONNECTOR_REDIS_URL", "Redis URL")],
    },
    ConnectorSpec {
        id: "elasticsearch",
        name: "Elasticsearch",
        category: "data",
        about: "索引、文档与查询（需地址与 API key）",
        command: "npx",
        args: &["-y", "@elastic/mcp-server-elasticsearch"],
        env: &[
            ("ES_URL", "CONNECTOR_ES_URL"),
            ("ES_API_KEY", "CONNECTOR_ES_API_KEY"),
        ],
        secrets: &[
            secret("CONNECTOR_ES_URL", "Elasticsearch URL"),
            secret("CONNECTOR_ES_API_KEY", "API key"),
        ],
    },
    ConnectorSpec {
        id: "kubernetes",
        name: "Kubernetes",
        category: "dev",
        about: "集群资源查询与 kubectl 操作（需 kubeconfig 路径）",
        command: "npx",
        args: &["-y", "mcp-server-kubernetes"],
        env: &[("KUBECONFIG", "CONNECTOR_KUBECONFIG")],
        secrets: &[secret("CONNECTOR_KUBECONFIG", "Kubeconfig path")],
    },
    ConnectorSpec {
        id: "gitlab",
        name: "GitLab",
        category: "dev",
        about: "仓库、MR 与 Issue（需 Personal Access Token）",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-gitlab"],
        env: &[(
            "GITLAB_PERSONAL_ACCESS_TOKEN",
            "CONNECTOR_GITLAB_TOKEN",
        )],
        secrets: &[secret("CONNECTOR_GITLAB_TOKEN", "Personal access token")],
    },
    ConnectorSpec {
        id: "cloudflare",
        name: "Cloudflare",
        category: "dev",
        about: "Workers / KV / D1 与日志查询（需 API token）",
        command: "npx",
        args: &["-y", "@cloudflare/mcp-server-cloudflare", "run"],
        env: &[("CLOUDFLARE_API_TOKEN", "CONNECTOR_CLOUDFLARE_TOKEN")],
        secrets: &[secret("CONNECTOR_CLOUDFLARE_TOKEN", "API token")],
    },
    ConnectorSpec {
        id: "linear",
        name: "Linear",
        category: "work",
        about: "Issue / 项目读取与更新（需 API key）",
        command: "npx",
        args: &["-y", "linear-mcp-server"],
        env: &[("LINEAR_API_KEY", "CONNECTOR_LINEAR_API_KEY")],
        secrets: &[secret("CONNECTOR_LINEAR_API_KEY", "Linear API key")],
    },
    ConnectorSpec {
        id: "figma",
        name: "Figma",
        category: "design",
        about: "设计稿结构读取与资源导出（需 API key）",
        command: "npx",
        args: &["-y", "figma-developer-mcp", "--stdio"],
        env: &[("FIGMA_API_KEY", "CONNECTOR_FIGMA_API_KEY")],
        secrets: &[secret("CONNECTOR_FIGMA_API_KEY", "Figma API key")],
    },
    ConnectorSpec {
        id: "playwright",
        name: "Playwright",
        category: "dev",
        about: "浏览器自动化：打开页面、点击与取文本（配合设置 → 浏览器）",
        command: "npx",
        args: &["-y", "@playwright/mcp@latest"],
        env: &[],
        secrets: &[],
    },
    ConnectorSpec {
        id: "memory",
        name: "Memory",
        category: "data",
        about: "跨会话的知识图谱笔记",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-memory"],
        env: &[],
        secrets: &[],
    },
    ConnectorSpec {
        id: "sequential-thinking",
        name: "Sequential thinking",
        category: "dev",
        about: "分步推理草稿（长任务拆解）",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-sequential-thinking"],
        env: &[],
        secrets: &[],
    },
    ConnectorSpec {
        id: "brave-search",
        name: "Brave Search",
        category: "search",
        about: "网页与新闻检索",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-brave-search"],
        env: &[("BRAVE_API_KEY", "CONNECTOR_BRAVE_API_KEY")],
        secrets: &[secret("CONNECTOR_BRAVE_API_KEY", "API key")],
    },
];

pub fn spec(id: &str) -> Option<&'static ConnectorSpec> {
    CATALOG.iter().find(|item| item.id == id)
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub enabled: Vec<String>,
}

fn default_version() -> u32 {
    1
}

/// `{data_root}/v2/connectors.json`（data_root 是 `.mode` 根）。
pub fn state_path(data_root: &Path) -> PathBuf {
    data_root.join("v2").join("connectors.json")
}

pub fn read_state(path: &Path) -> ConnectorState {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            return ConnectorState {
                version: 1,
                enabled: Vec::new(),
            };
        }
    };
    match serde_json::from_str::<ConnectorState>(&raw) {
        Ok(state) => ConnectorState {
            version: 1,
            // 只保留目录里还存在的 id，目录改名不会留下幽灵条目。
            enabled: state
                .enabled
                .into_iter()
                .filter(|id| spec(id).is_some())
                .collect(),
        },
        Err(error) => {
            eprintln!("[mode-bot] 连接器状态损坏（{}）：{error}", path.display());
            ConnectorState {
                version: 1,
                enabled: Vec::new(),
            }
        }
    }
}

pub fn write_state(path: &Path, state: &ConnectorState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建连接器目录失败：{error}"))?;
    }
    let value = serde_json::to_value(state).map_err(|error| error.to_string())?;
    write_json_atomic(path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| error.to_string())
}

/// 起 MCP 服务器要的形状（与插件 MCP 同构，渲染层直接合并）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorMcpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub transport: &'static str,
}

/// 启用清单 → MCP 服务器。`workspace` 供 filesystem 这类需要根目录的连接器使用；
/// `${secret:NAME}` 由调用方用密钥库解析后传入，缺 key 直接报出「连接器 + env + 名字」。
pub fn resolve_mcp_servers(
    state: &ConnectorState,
    secrets: &BTreeMap<String, String>,
    workspace: Option<&str>,
) -> Result<Vec<ConnectorMcpServer>, String> {
    let mut servers = Vec::new();
    for id in &state.enabled {
        let Some(spec) = spec(id) else { continue };
        let mut args = Vec::with_capacity(spec.args.len());
        for arg in spec.args {
            if *arg == "${workspace}" {
                let value = workspace.unwrap_or("").trim();
                if value.is_empty() {
                    return Err(format!("连接器 {} 需要工作目录，但本轮没有可用目录", spec.name));
                }
                args.push(value.to_string());
            } else {
                args.push((*arg).to_string());
            }
        }
        let mut env = BTreeMap::new();
        for (env_name, secret_name) in spec.env {
            let value = secrets.get(*secret_name).ok_or_else(|| {
                format!(
                    "连接器 {} 的 {env_name}：缺少密钥 {secret_name}",
                    spec.name
                )
            })?;
            env.insert((*env_name).to_string(), value.clone());
        }
        servers.push(ConnectorMcpServer {
            name: spec.id.to_string(),
            command: spec.command.to_string(),
            args,
            env,
            transport: "stdio",
        });
    }
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mode-bot-connectors-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn state_round_trips_and_filters_unknown_ids() {
        let dir = sandbox("state");
        let path = state_path(&dir);
        assert!(read_state(&path).enabled.is_empty());
        let state = ConnectorState {
            version: 1,
            enabled: vec!["github".into(), "nope".into()],
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(read_state(&path).enabled, vec!["github".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_fills_secrets_and_workspace() {
        let mut secrets = BTreeMap::new();
        secrets.insert("CONNECTOR_GITHUB_TOKEN".to_string(), "tok".to_string());
        let state = ConnectorState {
            version: 1,
            enabled: vec!["github".into(), "filesystem".into()],
        };
        let servers = resolve_mcp_servers(&state, &secrets, Some("C:\\ws")).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "github");
        assert_eq!(
            servers[0]
                .env
                .get("GITHUB_PERSONAL_ACCESS_TOKEN")
                .map(String::as_str),
            Some("tok")
        );
        assert_eq!(servers[1].args.last().map(String::as_str), Some("C:\\ws"));
    }

    #[test]
    fn missing_secret_names_the_connector() {
        let state = ConnectorState {
            version: 1,
            enabled: vec!["slack".into()],
        };
        let error = resolve_mcp_servers(&state, &Default::default(), None).unwrap_err();
        assert!(error.contains("Slack"), "{error}");
        assert!(error.contains("SLACK_BOT_TOKEN"), "{error}");
    }
}
