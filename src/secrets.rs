//! 插件密钥库：`{data_root}/v2/secrets.json`（与壳 `secrets.rs` 同一文件）。
//!
//! 设置 →「密钥」只列名称、永不回读值；插件 MCP 配置用 `${secret:NAME}` 占位，
//! 由壳在派发回合时解析。名称校验复用 `bot_team::plugin::validate_secret_name`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecretsFile {
    version: u32,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

impl SecretsFile {
    fn empty() -> Self {
        Self {
            version: 1,
            secrets: BTreeMap::new(),
        }
    }
}

/// `{data_root}/v2/secrets.json`（与壳同口径）。
pub fn path(data_root: &Path) -> PathBuf {
    data_root.join("v2").join("secrets.json")
}

/// 读单个密钥的值（OAuth token 交换等内部流程用；设置页仍只列名称）。
pub fn get(data_root: &Path, name: &str) -> Option<String> {
    read(&path(data_root))
        .secrets
        .get(name)
        .cloned()
        .filter(|value| !value.is_empty())
}

/// 只返回名称（升序）；缺失/损坏文件按空库处理。
pub fn list(data_root: &Path) -> Result<Vec<String>, String> {
    Ok(names_of(&read(&path(data_root))))
}

/// 读取全部密钥值（只供回合构造 MCP env；UI 不得调用）。
#[allow(dead_code)]
pub fn values(data_root: &Path) -> Result<BTreeMap<String, String>, String> {
    Ok(read(&path(data_root)).secrets)
}

/// 新增/覆盖一个密钥；返回最新名称列表。
pub fn set(data_root: &Path, name: &str, value: &str) -> Result<Vec<String>, String> {
    let name = bot_team::plugin::validate_secret_name(name)?;
    if value.is_empty() {
        return Err("密钥值不能为空（删除请用移除）".to_string());
    }
    let file_path = path(data_root);
    let mut file = read(&file_path);
    file.secrets.insert(name, value.to_string());
    write(&file_path, &file)?;
    Ok(names_of(&file))
}

/// 移除一个密钥（不存在也返回 Ok）；返回最新名称列表。
pub fn remove(data_root: &Path, name: &str) -> Result<Vec<String>, String> {
    let name = bot_team::plugin::validate_secret_name(name)?;
    let file_path = path(data_root);
    let mut file = read(&file_path);
    if file.secrets.remove(&name).is_some() {
        write(&file_path, &file)?;
    }
    Ok(names_of(&file))
}

fn read(path: &Path) -> SecretsFile {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return SecretsFile::empty();
    };
    match serde_json::from_str::<SecretsFile>(&raw) {
        Ok(file) if file.version == 1 => file,
        _ => SecretsFile::empty(),
    }
}

fn write(path: &Path, file: &SecretsFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建密钥目录失败：{error}"))?;
    }
    let value = serde_json::to_value(file).map_err(|error| error.to_string())?;
    atomic_file::write_json_atomic(path, &value, &atomic_file::WriteOptions::default())
        .map_err(|error| format!("写入密钥失败：{error}"))
}

fn names_of(file: &SecretsFile) -> Vec<String> {
    file.secrets.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot {
        dir: PathBuf,
    }

    impl TempRoot {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "bot-gpui-secrets-{}",
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
    fn missing_file_is_empty() {
        let temp = TempRoot::new();
        assert!(list(&temp.dir).expect("list").is_empty());
    }

    #[test]
    fn set_and_list_are_sorted() {
        let temp = TempRoot::new();
        set(&temp.dir, "TOKEN", "one").expect("set TOKEN");
        let names = set(&temp.dir, "ALPHA", "two").expect("set ALPHA");
        assert_eq!(names, vec!["ALPHA".to_string(), "TOKEN".to_string()]);
        assert_eq!(list(&temp.dir).expect("list"), names);
        let raw = std::fs::read_to_string(path(&temp.dir)).expect("read file");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(value["version"], 1);
        assert_eq!(value["secrets"]["TOKEN"], "one");
    }

    #[test]
    fn set_overwrites_and_trims_name() {
        let temp = TempRoot::new();
        set(&temp.dir, "  TOKEN  ", "one").expect("set");
        set(&temp.dir, "TOKEN", "two").expect("overwrite");
        let raw = std::fs::read_to_string(path(&temp.dir)).expect("read file");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(value["secrets"]["TOKEN"], "two");
        assert_eq!(list(&temp.dir).expect("list"), vec!["TOKEN".to_string()]);
    }

    #[test]
    fn invalid_name_and_empty_value_are_rejected() {
        let temp = TempRoot::new();
        assert!(set(&temp.dir, "", "x").is_err());
        assert!(set(&temp.dir, "bad-name", "x").is_err());
        assert!(set(&temp.dir, "TOKEN", "").is_err());
        assert!(list(&temp.dir).expect("list").is_empty());
    }

    #[test]
    fn remove_keeps_other_names_and_is_idempotent() {
        let temp = TempRoot::new();
        set(&temp.dir, "A", "1").expect("set A");
        set(&temp.dir, "B", "2").expect("set B");
        assert_eq!(
            remove(&temp.dir, "A").expect("remove A"),
            vec!["B".to_string()]
        );
        assert_eq!(
            remove(&temp.dir, "A").expect("remove A again"),
            vec!["B".to_string()]
        );
    }

    #[test]
    fn corrupt_file_is_treated_as_empty() {
        let temp = TempRoot::new();
        let file = path(&temp.dir);
        std::fs::create_dir_all(file.parent().unwrap()).expect("mkdir");
        std::fs::write(&file, "not json").expect("write");
        assert!(list(&temp.dir).expect("list").is_empty());
        set(&temp.dir, "TOKEN", "one").expect("set after corrupt");
        assert_eq!(list(&temp.dir).expect("list"), vec!["TOKEN".to_string()]);
    }
}
