//! Standard plugin manifest (MCP servers only in v1) and local registry shape.
//!
//! A plugin is a local folder with `modebot-plugin.json`; the registry stores only
//! metadata + the manifest's MCP server descriptors (no executables are copied).
//! `${secret:NAME}` placeholders are allowed in `args` / `env` / `headers`; values
//! are resolved from the local secret store at turn dispatch time. Skills point at
//! relative folders that are copied into a workspace skill root on demand.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginMcpServer {
    pub name: String,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSecretDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSkill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Relative folder (or .md file) inside the plugin folder.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mcp_servers: Vec<PluginMcpServer>,
    #[serde(default)]
    pub secrets: Vec<PluginSecretDefinition>,
    #[serde(default)]
    pub skills: Vec<PluginSkill>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Local folder path (absolute); never exported in team templates.
    pub path: String,
    #[serde(default)]
    pub mcp_servers: Vec<PluginMcpServer>,
    /// Derived from `${secret:NAME}` placeholders (names only, values never stored here).
    #[serde(default)]
    pub required_secrets: Vec<String>,
    /// Declared labels/descriptions for the required secrets (OAuth client pairs etc.).
    #[serde(default)]
    pub secret_definitions: Vec<PluginSecretDefinition>,
    #[serde(default)]
    pub skills: Vec<PluginSkill>,
    /// SHA-256 of the raw manifest bytes at install time (tamper evidence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginsFile {
    pub version: u32,
    #[serde(default)]
    pub plugins: Vec<InstalledPlugin>,
}

impl PluginsFile {
    pub fn empty() -> Self {
        Self {
            version: 1,
            plugins: Vec::new(),
        }
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

const SECRET_PREFIX: &str = "${secret:";

pub fn validate_secret_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("密钥名为空".to_string());
    }
    if !valid_secret_name(trimmed) {
        return Err(format!("密钥名不合法：{trimmed}（需字母/下划线开头，仅字母数字下划线，≤64）"));
    }
    Ok(trimmed.to_string())
}

fn valid_secret_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    name.len() <= 64
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// Parse `${secret:NAME}` placeholders; malformed placeholders are rejected.
pub fn secret_placeholders(value: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find(SECRET_PREFIX) {
        let after = &rest[start + SECRET_PREFIX.len()..];
        let Some(end) = after.find('}') else {
            return Err(format!("密钥占位缺少右花括号：{value}"));
        };
        let name = after[..end].trim();
        if !valid_secret_name(name) {
            return Err(format!("密钥名不合法：{name}"));
        }
        names.push(name.to_string());
        rest = &after[end + 1..];
    }
    Ok(names)
}

fn required_secrets_from_servers(servers: &[PluginMcpServer]) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for server in servers {
        for value in server
            .env
            .values()
            .chain(server.headers.values())
            .chain(server.args.iter())
        {
            if let Ok(found) = secret_placeholders(value) {
                names.extend(found);
            }
        }
    }
    names.into_iter().collect()
}

/// Validate a manifest read from a local plugin folder.
pub fn validate_manifest(manifest: PluginManifest) -> Result<PluginManifest, String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "plugin schemaVersion 必须为 1，实收 {}",
            manifest.schema_version
        ));
    }
    let id = non_empty_trimmed(&manifest.id).ok_or_else(|| "plugin id 为空".to_string())?;
    let name = non_empty_trimmed(&manifest.name).ok_or_else(|| "plugin name 为空".to_string())?;
    let mut names = std::collections::HashSet::new();
    let mut servers = Vec::with_capacity(manifest.mcp_servers.len());
    for (index, server) in manifest.mcp_servers.into_iter().enumerate() {
        let server_name =
            non_empty_trimmed(&server.name).ok_or_else(|| format!("mcpServers[{index}] name 为空"))?;
        if !names.insert(server_name.clone()) {
            return Err(format!("mcpServers 名称重复：{server_name}"));
        }
        let transport = match server.transport.as_deref().map(str::trim) {
            None | Some("") | Some("stdio") => "stdio",
            Some("http") => "http",
            Some(other) => {
                return Err(format!(
                    "mcpServers[{index}] transport 非法：{other}（只允许 stdio / http）"
                ))
            }
        };
        if transport == "stdio" && non_empty_trimmed(server.command.as_deref().unwrap_or("")).is_none()
        {
            return Err(format!("mcpServers[{index}] stdio 缺少 command"));
        }
        if transport == "http"
            && non_empty_trimmed(server.url.as_deref().unwrap_or("")).is_none()
        {
            return Err(format!("mcpServers[{index}] http 缺少 url"));
        }
        for (key, value) in server.env.iter().chain(server.headers.iter()) {
            if key.trim().is_empty() {
                return Err(format!("mcpServers[{index}] 的 env/headers 含空键"));
            }
            secret_placeholders(value)
                .map_err(|error| format!("mcpServers[{index}] {key}：{error}"))?;
        }
        for (arg_index, arg) in server.args.iter().enumerate() {
            secret_placeholders(arg)
                .map_err(|error| format!("mcpServers[{index}] args[{arg_index}]：{error}"))?;
        }
        if let Some(command) = server.command.as_deref() {
            if command.contains(SECRET_PREFIX) {
                return Err(format!("mcpServers[{index}] command 不允许密钥占位"));
            }
        }
        if let Some(url) = server.url.as_deref() {
            if url.contains(SECRET_PREFIX) {
                return Err(format!("mcpServers[{index}] url 不允许密钥占位（请用 headers）"));
            }
        }
        servers.push(PluginMcpServer {
            name: server_name,
            transport: Some(transport.to_string()),
            command: server.command.map(|value| value.trim().to_string()),
            args: server.args.into_iter().map(|value| value.trim().to_string()).collect(),
            env: server.env,
            url: server.url.map(|value| value.trim().to_string()),
            headers: server.headers,
        });
    }
    let mut secret_names = std::collections::BTreeSet::new();
    let mut secret_definitions = Vec::new();
    for (index, definition) in manifest.secrets.into_iter().enumerate() {
        let name = validate_secret_name(&definition.name)
            .map_err(|error| format!("secrets[{index}]：{error}"))?;
        if !secret_names.insert(name.clone()) {
            return Err(format!("secrets 名称重复：{name}"));
        }
        secret_definitions.push(PluginSecretDefinition {
            name,
            label: definition.label.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
            description: definition.description.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
        });
    }
    // Every placeholder must have a definition so the settings UI can label it.
    for name in required_secrets_from_servers(&servers) {
        if !secret_names.contains(&name) {
            secret_names.insert(name.clone());
            secret_definitions.push(PluginSecretDefinition {
                name,
                label: None,
                description: None,
            });
        }
    }
    let mut skill_names = std::collections::BTreeSet::new();
    let mut skills = Vec::new();
    for (index, skill) in manifest.skills.into_iter().enumerate() {
        let name = non_empty_trimmed(&skill.name)
            .ok_or_else(|| format!("skills[{index}] name 为空"))?;
        if !skill_names.insert(name.clone()) {
            return Err(format!("skills 名称重复：{name}"));
        }
        let raw_path = non_empty_trimmed(&skill.path)
            .ok_or_else(|| format!("skills[{index}] path 为空"))?;
        let normalized = raw_path.replace('\\', "/");
        if normalized.starts_with('/')
            || normalized.contains("../")
            || normalized == ".."
            || normalized.contains(':')
        {
            return Err(format!("skills[{index}] path 必须是插件内相对路径：{raw_path}"));
        }
        if normalized.contains(SECRET_PREFIX) {
            return Err(format!("skills[{index}] path 不允许密钥占位"));
        }
        skills.push(PluginSkill {
            name,
            description: skill.description.trim().to_string(),
            path: normalized,
        });
    }
    Ok(PluginManifest {
        schema_version: 1,
        id,
        name,
        version: manifest.version.trim().to_string(),
        description: manifest.description,
        mcp_servers: servers,
        secrets: secret_definitions,
        skills,
    })
}

pub fn installed_from_manifest(manifest: &PluginManifest, path: &str) -> InstalledPlugin {
    InstalledPlugin {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        path: path.to_string(),
        required_secrets: required_secrets_from_servers(&manifest.mcp_servers),
        secret_definitions: manifest.secrets.clone(),
        skills: manifest.skills.clone(),
        digest: None,
        mcp_servers: manifest.mcp_servers.clone(),
    }
}

/// Registry validation: ids unique, paths non-empty, MCP names unique per plugin.
pub fn validate_plugins_file(file: PluginsFile) -> Result<PluginsFile, String> {
    if file.version != 1 {
        return Err(format!("plugins version 必须为 1，实收 {}", file.version));
    }
    let mut ids = std::collections::HashSet::new();
    let mut plugins = Vec::with_capacity(file.plugins.len());
    for (index, plugin) in file.plugins.into_iter().enumerate() {
        let id = non_empty_trimmed(&plugin.id).ok_or_else(|| format!("plugins[{index}] id 为空"))?;
        if !ids.insert(id.clone()) {
            return Err(format!("plugin id 重复：{id}"));
        }
        if non_empty_trimmed(&plugin.path).is_none() {
            return Err(format!("plugins[{index}] path 为空"));
        }
        let mut names = std::collections::HashSet::new();
        for server in &plugin.mcp_servers {
            if !names.insert(server.name.clone()) {
                return Err(format!("plugin {id} 的 MCP 名称重复：{}", server.name));
            }
        }
        let required_secrets = required_secrets_from_servers(&plugin.mcp_servers);
        plugins.push(InstalledPlugin {
            id,
            name: plugin.name.trim().to_string(),
            version: plugin.version.trim().to_string(),
            description: plugin.description,
            path: plugin.path,
            required_secrets,
            secret_definitions: plugin.secret_definitions,
            skills: plugin.skills,
            digest: plugin.digest,
            mcp_servers: plugin.mcp_servers,
        });
    }
    Ok(PluginsFile { version: 1, plugins })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> PluginManifest {
        PluginManifest {
            schema_version: 1,
            id: "com.example.github".into(),
            name: "GitHub".into(),
            version: "1.0.0".into(),
            description: String::new(),
            mcp_servers: vec![PluginMcpServer {
                name: "github".into(),
                transport: Some("stdio".into()),
                command: Some("npx".into()),
                args: vec!["-y".into(), "server".into()],
                env: BTreeMap::new(),
                url: None,
                headers: BTreeMap::new(),
            }],
            secrets: Vec::new(),
            skills: Vec::new(),
        }
    }

    #[test]
    fn validates_stdio_and_http_manifests() {
        assert!(validate_manifest(manifest()).is_ok());
        let mut http = manifest();
        http.mcp_servers[0].transport = Some("http".into());
        http.mcp_servers[0].command = None;
        http.mcp_servers[0].url = Some("https://example.test/mcp".into());
        assert!(validate_manifest(http).is_ok());
    }

    #[test]
    fn rejects_bad_transport_and_missing_targets() {
        let mut bad = manifest();
        bad.mcp_servers[0].transport = Some("smtp".into());
        assert!(validate_manifest(bad).is_err());

        let mut missing = manifest();
        missing.mcp_servers[0].command = None;
        assert!(validate_manifest(missing).is_err());
    }

    #[test]
    fn accepts_secret_placeholders_and_reports_required_names() {
        let mut secret = manifest();
        secret.mcp_servers[0]
            .env
            .insert("TOKEN".into(), "${secret:GITHUB_TOKEN}".into());
        secret.mcp_servers[0]
            .headers
            .insert("Authorization".into(), "Bearer ${secret:GITHUB_TOKEN}".into());
        secret.mcp_servers[0]
            .args
            .push("--key=${secret:EXTRA_KEY}".into());
        let validated = validate_manifest(secret).expect("placeholders are allowed");
        let installed = installed_from_manifest(&validated, "C:/p");
        assert_eq!(
            installed.required_secrets,
            vec!["EXTRA_KEY".to_string(), "GITHUB_TOKEN".to_string()]
        );
    }

    #[test]
    fn rejects_malformed_secret_placeholders() {
        let mut broken = manifest();
        broken.mcp_servers[0]
            .env
            .insert("TOKEN".into(), "${secret:GITHUB_TOKEN".into());
        assert!(validate_manifest(broken).is_err());

        let mut invalid_name = manifest();
        invalid_name.mcp_servers[0]
            .env
            .insert("TOKEN".into(), "${secret:bad-name}".into());
        assert!(validate_manifest(invalid_name).is_err());

        let mut in_command = manifest();
        in_command.mcp_servers[0].command = Some("${secret:CMD}".into());
        assert!(validate_manifest(in_command).is_err());
    }

    #[test]
    fn registry_rejects_duplicate_ids() {
        let plugin = installed_from_manifest(&validate_manifest(manifest()).unwrap(), "C:\\p");
        let file = PluginsFile {
            version: 1,
            plugins: vec![plugin.clone(), plugin],
        };
        assert!(validate_plugins_file(file).is_err());
    }
    #[test]
    fn secret_definitions_are_normalized_and_filled_from_placeholders() {
        let mut value = manifest();
        value.secrets = vec![PluginSecretDefinition {
            name: " GITHUB_TOKEN ".into(),
            label: Some("GitHub token".into()),
            description: None,
        }];
        value.mcp_servers[0]
            .env
            .insert("TOKEN".into(), "${secret:GITHUB_TOKEN} ${secret:EXTRA}".into());
        let validated = validate_manifest(value).unwrap();
        let names: Vec<String> = validated.secrets.iter().map(|item| item.name.clone()).collect();
        assert_eq!(names, vec!["GITHUB_TOKEN".to_string(), "EXTRA".to_string()]);
        assert_eq!(validated.secrets[0].label.as_deref(), Some("GitHub token"));
    }

    #[test]
    fn skills_require_safe_relative_paths_and_unique_names() {
        let mut valid = manifest();
        valid.skills = vec![PluginSkill {
            name: "release".into(),
            description: "ship it".into(),
            path: "skills/release".into(),
        }];
        assert!(validate_manifest(valid).is_ok());

        for bad in ["../escape", "C:/abs", "/abs", "skills/../../x"] {
            let mut value = manifest();
            value.skills = vec![PluginSkill {
                name: "release".into(),
                description: String::new(),
                path: bad.into(),
            }];
            assert!(validate_manifest(value).is_err(), "path should be rejected: {bad}");
        }

        let mut duplicate = manifest();
        duplicate.skills = vec![
            PluginSkill {
                name: "same".into(),
                description: String::new(),
                path: "a".into(),
            },
            PluginSkill {
                name: "same".into(),
                description: String::new(),
                path: "b".into(),
            },
        ];
        assert!(validate_manifest(duplicate).is_err());
    }

}
