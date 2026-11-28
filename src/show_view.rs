//! `show_view` 工具（F13）：让 Bot 在对话里"展示视图"——表格 / 指标 / 说明。
//!
//! 架构取舍：DS 的 `show_view` 是独立卡片组件（HTML/图表/表单由前端渲染）；
//! 我们走 **markdown 渲染管线**：工具把结构化数据规范成 GFM 表格/列表文本，
//! 由 `kit_markdown_el` 渲染成与对话一致的样式。图表/原生表单不渲染（如实
//! 降级为表格/列表），换来零新增渲染组件与跨壳一致。

use std::sync::Arc;

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::Schema;
use rpi_ai::types::Tool;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const MAX_COLUMNS: usize = 8;
const MAX_ROWS: usize = 20;
const MAX_METRICS: usize = 12;

struct ShowViewTool {
    tool: Tool,
    /// 调用者身份（保留给未来的按 Bot 视图权限；当前仅记录）。
    #[allow(dead_code)]
    identity: String,
}

fn tool_def() -> Tool {
    Tool {
        name: "show_view".to_string(),
        description:
            "在对话里展示结构化视图：table（表格：columns + rows）、metrics（指标卡）、\
             note（说明文本）。适合状态汇总、对比、清单类结果；表格最多 8 列 × 20 行。"
                .to_string(),
        parameters: Schema(json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["table", "metrics", "note"]},
                "title": {"type": "string", "description": "视图标题（可选）"},
                "columns": {"type": "array", "items": {"type": "string"}, "description": "表格列名（table 必填）"},
                "rows": {"type": "array", "items": {"type": "array", "items": {"type": "string"}}, "description": "表格行（table 必填）"},
                "metrics": {"type": "array", "items": {"type": "object", "properties": {"label": {"type": "string"}, "value": {"type": "string"}}}, "description": "指标列表（metrics 必填）"},
                "text": {"type": "string", "description": "说明文本（note 必填）"}
            },
            "required": ["kind"]
        })),
        constrained_sampling: None,
    }
}

/// 构造 show_view 工具（会话级，主/IM 都注入）。
pub fn tools(identity: &str) -> Vec<HarnessTool> {
    let identity = identity.trim();
    if identity.is_empty() {
        return Vec::new();
    }
    vec![HarnessTool::new(Arc::new(ShowViewTool {
        tool: tool_def(),
        identity: identity.to_string(),
    }))]
}

fn cell(value: &Value) -> String {
    let text = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    // 表格单元格里不能有竖线/换行。
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn render_table(params: &Value) -> Result<String, String> {
    let columns: Vec<String> = params
        .get("columns")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(cell).collect())
        .unwrap_or_default();
    if columns.is_empty() || columns.len() > MAX_COLUMNS {
        return Err(format!("columns 必须是 1~{MAX_COLUMNS} 列"));
    }
    let rows: Vec<Vec<String>> = params
        .get("rows")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(MAX_ROWS)
                .map(|row| {
                    row.as_array()
                        .map(|cells| cells.iter().map(cell).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    if rows.is_empty() {
        return Err("rows 不能为空".to_string());
    }
    let mut out = String::new();
    out.push_str(&format!("| {} |\n", columns.join(" | ")));
    out.push_str(&format!(
        "| {} |\n",
        (0..columns.len()).map(|_| "---").collect::<Vec<_>>().join(" | ")
    ));
    for row in rows {
        let mut padded = row;
        padded.resize(columns.len(), String::new());
        padded.truncate(columns.len());
        out.push_str(&format!("| {} |\n", padded.join(" | ")));
    }
    Ok(out)
}

fn render_metrics(params: &Value) -> Result<String, String> {
    let metrics = params
        .get("metrics")
        .and_then(Value::as_array)
        .map(|items| items.iter().take(MAX_METRICS).collect::<Vec<_>>())
        .unwrap_or_default();
    if metrics.is_empty() {
        return Err("metrics 不能为空".to_string());
    }
    let mut out = String::new();
    for item in metrics {
        let label = item
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("-")
            .trim();
        let value = item
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| item.get("value").map(cell).unwrap_or_default());
        out.push_str(&format!("- **{label}**：{value}\n"));
    }
    Ok(out)
}

impl ShowViewTool {
    fn render(&self, params: &Value) -> Result<String, String> {
        let kind = params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let body = match kind {
            "table" => render_table(params)?,
            "metrics" => render_metrics(params)?,
            "note" => params
                .get("text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "note 需要 text".to_string())?
                .to_string(),
            other => return Err(format!("未知 kind：{other}（table/metrics/note）")),
        };
        Ok(match title {
            Some(title) => format!("### {title}\n\n{body}"),
            None => body,
        })
    }
}

#[async_trait]
impl AgentTool for ShowViewTool {
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
        let text = self
            .render(&params)
            .map_err(rpi_agent::AgentError::Tool)?;
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
