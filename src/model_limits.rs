//! 模型窗口 / 输出上限：读取优先级 + 落盘（v0.5.0 W4，contract §2.1）。
//!
//! 三层取值（前者覆盖后者）：
//! 1. `{data_root}/v2/provider_config.json` 的 `providerModelRules[].config.contextWindow/maxTokens`
//!    —— 用户显式填的值，全程按 `serde_json::Value` 读写，不收紧 schema；
//! 2. 内置模板 `assets/provider/mode-builtin.json` 的
//!    `config.modelRules[]` 里按 `modelMatch` / `apiTypeMatch` / `baseUrlMatch` 命中的规则；
//! 3. `FALLBACK_*` 兜底常量。
//!
//! **旧数据零迁移**：没这两个键就走模板/兜底；老版本读到未知键会忽略。
//!
//! 契约 §2.1：`read` / `resolve` / `write` / `placeholder` 是给 UI 的固定接口，
//! 本期 UI 侧还没接线，所以对未接线项放开 `dead_code`。

#![allow(dead_code)]

use std::path::PathBuf;

/// 兜底上下文窗口（v0.4 里这俩数字写死在 `rpi_engine::resolve_model`）。
pub const FALLBACK_CONTEXT_WINDOW: i64 = 128_000;
/// 兜底最大输出 tokens。
pub const FALLBACK_MAX_TOKENS: i64 = 16_384;

/// 一次查询的结果：两个字段都可能是 `None`（= 该层没给值）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelLimits {
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
    /// 模板给出的窗口（UI 占位符用；不是用户填的）。
    pub template_context_window: Option<i64>,
    /// 模板给出的输出上限（UI 占位符用）。
    pub template_max_tokens: Option<i64>,
}

fn data_root() -> PathBuf {
    crate::data::data_root()
}

fn provider_config_path() -> PathBuf {
    data_root().join("v2").join("provider_config.json")
}

fn read_provider_value() -> Option<serde_json::Value> {
    let bytes = std::fs::read(provider_config_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_provider_value(value: &serde_json::Value) -> Result<(), String> {
    atomic_file::write_json_atomic(
        &provider_config_path(),
        value,
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| format!("provider_config.json 写入失败（{error}）"))
}

/// `config.modelConfigRules.providerModelRules` 里"这个 provider+model"那条规则的 `config` 对象。
fn rule_config<'a>(
    value: &'a serde_json::Value,
    provider_id: &str,
    model_id: &str,
) -> Option<&'a serde_json::Value> {
    value
        .get("config")?
        .get("modelConfigRules")?
        .get("providerModelRules")?
        .as_array()?
        .iter()
        .find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
                && rule.get("modelId").and_then(serde_json::Value::as_str) == Some(model_id)
        })?
        .get("config")
}

fn rule_config_mut<'a>(
    value: &'a mut serde_json::Value,
    provider_id: &str,
    model_id: &str,
) -> Option<&'a mut serde_json::Value> {
    value
        .get_mut("config")?
        .get_mut("modelConfigRules")?
        .get_mut("providerModelRules")?
        .as_array_mut()?
        .iter_mut()
        .find(|rule| {
            rule.get("providerId").and_then(serde_json::Value::as_str) == Some(provider_id)
                && rule.get("modelId").and_then(serde_json::Value::as_str) == Some(model_id)
        })?
        .get_mut("config")
}

/// 内置模板里按 modelMatch/apiTypeMatch/baseUrlMatch 命中的第一条规则的 `config`。
///
/// 只做"够用"的匹配：`modelMatch` 必须命中（这是主键），`apiTypeMatch`/`baseUrlMatch`
/// 存在时必须同时命中。模板里的正则已是产品或上游写好的，这里直接复用 `regex`。
fn template_config(provider_id: &str, model_id: &str) -> Option<serde_json::Value> {
    let bytes = std::fs::read(data_root().join("v2").join("provider_config.json")).ok();
    let provider_value = bytes
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok());
    // provider 的 api 类型与 baseUrl：先看用户配置，再看模板默认。
    let (api_type, base_url) = provider_value
        .as_ref()
        .and_then(|value| {
            value
                .get("config")?
                .get("providerConfigRules")?
                .get("providerRules")?
                .as_array()?
                .iter()
                .find(|rule| {
                    rule.get("providerId").and_then(serde_json::Value::as_str)
                        == Some(provider_id)
                })
        })
        .map(|rule| {
            let config = rule.get("config");
            (
                config
                    .and_then(|config| config.pointer("/api/type"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                config
                    .and_then(|config| config.get("baseUrl"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            )
        })
        .unwrap_or((None, None));

    let catalog: serde_json::Value = serde_json::from_str(crate::rpi_engine::BUILTIN_CATALOG).ok()?;
    let rules = catalog
        .get("config")?
        .get("modelConfigRules")?
        .get("modelRules")?
        .as_array()?;
    for rule in rules {
        let pattern = rule.get("modelMatch").and_then(serde_json::Value::as_str)?;
        let Ok(regex) = regex::Regex::new(pattern) else {
            continue;
        };
        if !regex.is_match(model_id) {
            continue;
        }
        if let Some(expected) = rule.get("apiTypeMatch").and_then(serde_json::Value::as_str) {
            let actual = api_type.as_deref().unwrap_or("anthropic-messages");
            if !matches_pattern(expected, actual) {
                continue;
            }
        }
        if let Some(expected) = rule.get("baseUrlMatch").and_then(serde_json::Value::as_str)
            && let Some(actual) = base_url.as_deref()
            && let Ok(regex) = regex::Regex::new(expected)
            && !regex.is_match(actual)
        {
            continue;
        }
        return rule.get("config").cloned();
    }
    None
}

/// `apiTypeMatch` 可能写成 `.*`（任意）或具体类型。
fn matches_pattern(pattern: &str, actual: &str) -> bool {
    if pattern == ".*" || pattern.is_empty() {
        return true;
    }
    match regex::Regex::new(pattern) {
        Ok(regex) => regex.is_match(actual),
        Err(_) => pattern == actual,
    }
}

fn template_limits(provider_id: &str, model_id: &str) -> (Option<i64>, Option<i64>) {
    let Some(config) = template_config(provider_id, model_id) else {
        return (None, None);
    };
    let window = config
        .pointer("/properties/contextWindow")
        .and_then(serde_json::Value::as_i64);
    let max_tokens = config
        .pointer("/optionSpecs/maxOutputTokens/max")
        .and_then(serde_json::Value::as_i64);
    (window, max_tokens)
}

/// 读某 provider+model 的窗口/输出上限（用户值可能为 None）。
pub fn read(provider_id: &str, model_id: &str) -> ModelLimits {
    let explicit = read_provider_value()
        .as_ref()
        .and_then(|value| rule_config(value, provider_id, model_id).cloned());
    let (template_window, template_max_tokens) = template_limits(provider_id, model_id);
    let pick = |key: &str| {
        explicit
            .as_ref()
            .and_then(|config| config.get(key))
            .and_then(serde_json::Value::as_i64)
            .filter(|value| *value > 0)
    };
    ModelLimits {
        context_window: pick("contextWindow"),
        max_tokens: pick("maxTokens"),
        template_context_window: template_window,
        template_max_tokens,
    }
}

/// 解析成可直接赋给 `Model` 的 `(context_window, max_tokens)`。
pub fn resolve(limits: &ModelLimits) -> (i64, i64) {
    let window = limits
        .context_window
        .or(limits.template_context_window)
        .unwrap_or(FALLBACK_CONTEXT_WINDOW);
    let max_tokens = limits
        .max_tokens
        .or(limits.template_max_tokens)
        .unwrap_or(FALLBACK_MAX_TOKENS)
        // 输出上限不能超过窗口的一半（与 `write` 的校验同口径）。
        .min((window / 2).max(1));
    (window, max_tokens)
}

/// 给 UI 用的占位符值：模板值优先，否则兜底。
pub fn placeholder(limits: &ModelLimits) -> (i64, i64) {
    (
        limits
            .template_context_window
            .unwrap_or(FALLBACK_CONTEXT_WINDOW),
        limits.template_max_tokens.unwrap_or(FALLBACK_MAX_TOKENS),
    )
}

/// 写回用户显式值。`None` = 删掉该键（回到模板/兜底）。
pub fn write(
    provider_id: &str,
    model_id: &str,
    limits: &ModelLimits,
) -> Result<(), String> {
    if let Some(window) = limits.context_window
        && window < 1024
    {
        return Err("窗口必须是不小于 1024 的整数".to_string());
    }
    if let Some(max_tokens) = limits.max_tokens {
        if max_tokens < 1 {
            return Err("最大输出 tokens 必须是正整数".to_string());
        }
        let window = limits
            .context_window
            .or(limits.template_context_window)
            .unwrap_or(FALLBACK_CONTEXT_WINDOW);
        if max_tokens > (window / 2).max(1) {
            return Err("输出上限必须小于窗口的一半".to_string());
        }
    }
    let mut value = read_provider_value()
        .ok_or_else(|| "provider_config.json 读取失败（文件缺失或解析失败）".to_string())?;
    let Some(config) = rule_config_mut(&mut value, provider_id, model_id) else {
        return Err(format!(
            "找不到模型规则：{provider_id}/{model_id}（先在模型列表里加进来）"
        ));
    };
    let Some(object) = config.as_object_mut() else {
        return Err("模型规则格式异常".to_string());
    };
    for (key, next) in [
        ("contextWindow", limits.context_window),
        ("maxTokens", limits.max_tokens),
    ] {
        match next {
            Some(number) => {
                object.insert(key.to_string(), serde_json::Value::from(number));
            }
            None => {
                object.remove(key);
            }
        }
    }
    write_provider_value(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_falls_back_to_named_constants() {
        let (window, max_tokens) = resolve(&ModelLimits::default());
        assert_eq!(window, FALLBACK_CONTEXT_WINDOW);
        assert_eq!(max_tokens, FALLBACK_MAX_TOKENS);
    }

    #[test]
    fn resolve_caps_output_at_half_window() {
        let limits = ModelLimits {
            context_window: Some(8_000),
            max_tokens: Some(64_000),
            ..Default::default()
        };
        let (window, max_tokens) = resolve(&limits);
        assert_eq!(window, 8_000);
        assert_eq!(max_tokens, 4_000);
    }

    #[test]
    fn explicit_value_beats_template() {
        let limits = ModelLimits {
            context_window: Some(500_000),
            template_context_window: Some(FALLBACK_CONTEXT_WINDOW),
            ..Default::default()
        };
        assert_eq!(resolve(&limits).0, 500_000);
    }

    #[test]
    fn write_rejects_tiny_window() {
        let limits = ModelLimits {
            context_window: Some(512),
            ..Default::default()
        };
        let error = write("p", "m", &limits).unwrap_err();
        assert!(error.contains("1024"));
    }

    #[test]
    fn api_type_pattern_accepts_wildcard() {
        assert!(matches_pattern(".*", "anthropic-messages"));
        assert!(matches_pattern("openai-responses", "openai-responses"));
        assert!(!matches_pattern("openai-responses", "anthropic-messages"));
    }
}
