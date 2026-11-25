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

/// 远程 MCP 的鉴权方式（v0.5.0 W7：只做 header / query key，不做 OAuth）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAuth {
    /// 无需凭据（公共远程 MCP）。
    None,
    /// `Authorization: <prefix><secret>`（如 `Bearer <token>`）。
    Header {
        name: &'static str,
        prefix: &'static str,
        secret: &'static str,
    },
    /// URL query 参数带 key（高德这类）。
    Query {
        name: &'static str,
        secret: &'static str,
    },
}

/// 远程 MCP 连接器（Streamable HTTP）。
///
/// 与 stdio 目录分开列：老条目一个字段都不用改，删除远程条目也不会影响
/// stdio 的 `CATALOG`。OAuth 型远程（GitHub Copilot / Notion / Linear 等）
/// 明确不在 v0.5 范围，见 §0.1。
pub struct RemoteConnector {
    pub id: &'static str,
    pub name: &'static str,
    /// dev / docs / work / data / search / design / commerce
    pub category: &'static str,
    pub about: &'static str,
    pub url: &'static str,
    /// 额外的固定请求头（`(name, value)`）。
    pub headers: &'static [(&'static str, &'static str)],
    pub auth: RemoteAuth,
    pub secret_label: &'static str,
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

/// v0.5.0 W7 新增的远程（HTTP）连接器：选的是**公共或 key 型**远程 MCP，
/// 不需要 OAuth 回调就能连。连不上取决于用户是否填了 key，未填时 UI 标「缺凭据」。
pub const REMOTE_CATALOG: &[RemoteConnector] = &[
    RemoteConnector {
        id: "huggingface",
        name: "Hugging Face",
        category: "data",
        about: "Models, datasets, and Spaces",
        url: "https://huggingface.co/mcp",
        headers: &[],
        auth: RemoteAuth::None,
        secret_label: "",
    },
    RemoteConnector {
        id: "exa",
        name: "Exa",
        category: "search",
        about: "Web search for agents",
        url: "https://mcp.exa.ai/mcp",
        headers: &[],
        auth: RemoteAuth::None,
        secret_label: "",
    },
    RemoteConnector {
        id: "deepwiki",
        name: "DeepWiki",
        category: "docs",
        about: "Documentation for public code repositories",
        url: "https://mcp.deepwiki.com/mcp",
        headers: &[],
        auth: RemoteAuth::None,
        secret_label: "",
    },
    RemoteConnector {
        id: "context7",
        name: "Context7",
        category: "docs",
        about: "Library documentation and code examples",
        url: "https://mcp.context7.com/mcp",
        headers: &[],
        auth: RemoteAuth::None,
        secret_label: "",
    },
    RemoteConnector {
        id: "mslearn",
        name: "Microsoft Learn",
        category: "docs",
        about: "Microsoft product documentation",
        url: "https://learn.microsoft.com/api/mcp",
        headers: &[],
        auth: RemoteAuth::None,
        secret_label: "",
    },
    RemoteConnector {
        id: "cloudflare-docs",
        name: "Cloudflare Docs",
        category: "docs",
        about: "Cloudflare product documentation",
        url: "https://docs.mcp.cloudflare.com/mcp",
        headers: &[],
        auth: RemoteAuth::None,
        secret_label: "",
    },
    RemoteConnector {
        id: "amap",
        name: "Amap",
        category: "search",
        about: "Maps, places, and routes in China",
        url: "https://mcp.amap.com/mcp",
        headers: &[],
        auth: RemoteAuth::Query {
            name: "key",
            secret: "CONNECTOR_AMAP_KEY",
        },
        secret_label: "高德 Web 服务 key",
    },
    RemoteConnector {
        id: "github-copilot",
        name: "GitHub Copilot MCP",
        category: "dev",
        about: "Repositories, issues, and pull requests（需 PAT）",
        url: "https://api.githubcopilot.com/mcp/",
        headers: &[(
            "X-MCP-Toolsets",
            "context,repos,issues,pull_requests",
        )],
        auth: RemoteAuth::Header {
            name: "Authorization",
            prefix: "Bearer ",
            secret: "CONNECTOR_GITHUB_COPILOT_TOKEN",
        },
        secret_label: "GitHub PAT（copilot 权限）",
    },
];

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

/// 远程（HTTP）目录里的一条；stdio 目录里没有。
pub fn remote_spec(id: &str) -> Option<&'static RemoteConnector> {
    REMOTE_CATALOG.iter().find(|item| item.id == id)
}

/// 目录里是否存在这个 id（stdio 或 remote 都算）。
pub fn known(id: &str) -> bool {
    spec(id).is_some() || remote_spec(id).is_some()
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
            enabled: state.enabled.into_iter().filter(|id| known(id)).collect(),
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
///
/// `transport` 仍是字符串，老消费者（`src/connectors.rs` 的 payload）不用改：
/// stdio 条目照旧是 `"stdio"`，`url`/`headers` 为空；远程条目是 `"http"`，
/// `command`/`args`/`env` 为空。这样新增字段对旧代码是纯增量。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorMcpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub transport: &'static str,
    /// HTTP transport 的地址（stdio 为空）。
    #[serde(default)]
    pub url: String,
    /// HTTP transport 的固定请求头（不含凭据；凭据在 `auth_header` 里单独给）。
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// 需要在调用时附加的鉴权头：`(头名, 完整值)`。日志里绝不能打这个值。
    #[serde(default, skip_serializing)]
    pub auth_header: Option<(String, String)>,
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
        // 远程（HTTP）条目：不带 command，只带 url + 头。
        if let Some(remote) = remote_spec(id) {
            let mut url = remote.url.to_string();
            let mut auth_header = None;
            match remote.auth {
                RemoteAuth::None => {}
                RemoteAuth::Header {
                    name,
                    prefix,
                    secret,
                } => {
                    let value = secrets.get(secret).ok_or_else(|| {
                        format!("连接器 {} 缺少凭据 {secret}", remote.name)
                    })?;
                    auth_header = Some((name.to_string(), format!("{prefix}{value}")));
                }
                RemoteAuth::Query { name, secret } => {
                    let value = secrets.get(secret).ok_or_else(|| {
                        format!("连接器 {} 缺少凭据 {secret}", remote.name)
                    })?;
                    let separator = if url.contains('?') { '&' } else { '?' };
                    url.push(separator);
                    url.push_str(name);
                    url.push('=');
                    url.push_str(&urlencode(value));
                }
            }
            let mut headers = BTreeMap::new();
            for (name, value) in remote.headers {
                headers.insert((*name).to_string(), (*value).to_string());
            }
            servers.push(ConnectorMcpServer {
                name: remote.id.to_string(),
                command: String::new(),
                args: Vec::new(),
                env: BTreeMap::new(),
                transport: "http",
                url,
                headers,
                auth_header,
            });
            continue;
        }
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
            url: String::new(),
            headers: BTreeMap::new(),
            auth_header: None,
        });
    }
    Ok(servers)
}

/// query 参数的最小转义（key 这类都是字母数字，但不想赌）。
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
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

    #[test]
    fn remote_public_connector_needs_no_secret() {
        let state = ConnectorState {
            version: 1,
            enabled: vec!["context7".into()],
        };
        let servers = resolve_mcp_servers(&state, &Default::default(), None).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].transport, "http");
        assert_eq!(servers[0].url, "https://mcp.context7.com/mcp");
        assert!(servers[0].command.is_empty());
        assert!(servers[0].auth_header.is_none());
    }

    #[test]
    fn remote_header_auth_builds_bearer_and_extra_headers() {
        let mut secrets = BTreeMap::new();
        secrets.insert(
            "CONNECTOR_GITHUB_COPILOT_TOKEN".to_string(),
            "ghp_x".to_string(),
        );
        let state = ConnectorState {
            version: 1,
            enabled: vec!["github-copilot".into()],
        };
        let servers = resolve_mcp_servers(&state, &secrets, None).unwrap();
        assert_eq!(
            servers[0].auth_header,
            Some(("Authorization".to_string(), "Bearer ghp_x".to_string()))
        );
        assert_eq!(
            servers[0]
                .headers
                .get("X-MCP-Toolsets")
                .map(String::as_str),
            Some("context,repos,issues,pull_requests")
        );
        // 鉴权头不参与序列化，避免顺手进了日志或前端 payload。
        let json = serde_json::to_string(&servers[0]).unwrap();
        assert!(!json.contains("ghp_x"), "{json}");
    }

    #[test]
    fn remote_query_auth_appends_key() {
        let mut secrets = BTreeMap::new();
        secrets.insert("CONNECTOR_AMAP_KEY".to_string(), "abc".to_string());
        let state = ConnectorState {
            version: 1,
            enabled: vec!["amap".into()],
        };
        let servers = resolve_mcp_servers(&state, &secrets, None).unwrap();
        assert_eq!(servers[0].url, "https://mcp.amap.com/mcp?key=abc");
        assert!(servers[0].auth_header.is_none());
    }

    #[test]
    fn remote_missing_key_is_a_hard_error() {
        let state = ConnectorState {
            version: 1,
            enabled: vec!["amap".into()],
        };
        let error = resolve_mcp_servers(&state, &Default::default(), None).unwrap_err();
        assert!(error.contains("Amap"), "{error}");
        assert!(error.contains("CONNECTOR_AMAP_KEY"), "{error}");
    }

    #[test]
    fn remote_ids_are_kept_in_state() {
        assert!(known("context7"));
        assert!(known("github"));
        assert!(!known("nope"));
        assert!(remote_spec("github").is_none(), "github 是 stdio 条目");
    }
}
