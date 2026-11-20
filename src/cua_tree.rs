//! Windows UI Automation 元素树：给电脑控制补上「按控件名操作」的一层。
//!
//! 纯 Rust（windows crate COM），提供树快照、按名称点击、按名称读写值。

use std::sync::Arc;

use async_trait::async_trait;
use rpi_agent::{AgentTool, AgentToolResult, TextContentOrImage};
use rpi_ai::types::Tool;
use rpi_ai::Schema;
use rpi_harness::types::HarnessTool;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationSelectionItemPattern, IUIAutomationTreeWalker, IUIAutomationValuePattern,
    UIA_InvokePatternId, UIA_SelectionItemPatternId, UIA_ValuePatternId,
};
use windows::core::BSTR;

const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 400;

fn automation() -> Result<IUIAutomation, String> {
    // SAFETY: COM 初始化可重复调用；MTA 与工具线程匹配。
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("初始化 UI Automation 失败：{error}"))
    }
}

fn root() -> Result<IUIAutomationElement, String> {
    let automation = automation()?;
    // SAFETY: COM 对象由 automation 持有，调用期间有效。
    unsafe { automation.GetRootElement() }
        .map_err(|error| format!("读取桌面根元素失败：{error}"))
}

fn name_of(element: &IUIAutomationElement) -> String {
    // SAFETY: 属性读取失败按空串处理。
    unsafe { element.CurrentName() }
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn children_of(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
) -> Vec<IUIAutomationElement> {
    let mut children = Vec::new();
    // SAFETY: walker 与元素在调用期间有效；返回元素持有引用计数。
    let mut current = unsafe { walker.GetFirstChildElement(element) }.ok();
    while let Some(child) = current {
        // SAFETY: 同上。
        current = unsafe { walker.GetNextSiblingElement(&child) }.ok();
        children.push(child.clone());
    }
    children
}

fn describe(element: &IUIAutomationElement, depth: usize) -> String {
    let name = name_of(element);
    // SAFETY: 属性读取失败使用默认值。
    let control = unsafe { element.CurrentControlType() }
        .map(|value| value.0)
        .unwrap_or(0);
    let automation_id = unsafe { element.CurrentAutomationId() }
        .map(|value| value.to_string())
        .unwrap_or_default();
    let class_name = unsafe { element.CurrentClassName() }
        .map(|value| value.to_string())
        .unwrap_or_default();
    let rect = unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default();
    let offscreen = unsafe { element.CurrentIsOffscreen() }
        .map(|value| value.as_bool())
        .unwrap_or(false);
    let mut line = format!(
        "{}{} | control={} | name={:?} | id={:?} | class={:?} | rect=({},{} {}x{})",
        "  ".repeat(depth),
        if depth == 0 { "root" } else { "item" },
        control,
        name,
        automation_id,
        class_name,
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    );
    if offscreen {
        line.push_str(" | offscreen");
    }
    line
}

fn walk(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: usize,
    out: &mut Vec<String>,
    count: &mut usize,
) {
    if depth > MAX_DEPTH || *count >= MAX_NODES {
        return;
    }
    out.push(describe(element, depth));
    *count += 1;
    for child in children_of(walker, element) {
        walk(walker, &child, depth + 1, out, count);
    }
}

/// 桌面元素树快照（名称、控件类型、AutomationId、类名、位置）。
pub fn tree() -> Result<String, String> {
    let automation = automation()?;
    let root = root()?;
    // SAFETY: COM 对象在调用期间有效。
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| format!("创建 UI Automation walker 失败：{error}"))?;
    let mut lines = Vec::new();
    let mut count = 0;
    walk(&walker, &root, 0, &mut lines, &mut count);
    Ok(lines.join("\n"))
}

fn find_named(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    needle: &str,
    exact: bool,
    depth: usize,
) -> Option<IUIAutomationElement> {
    if depth > MAX_DEPTH {
        return None;
    }
    let name = name_of(element);
    let matched = if exact {
        name.eq_ignore_ascii_case(needle)
    } else {
        name.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    };
    if matched && !name.is_empty() {
        return Some(element.clone());
    }
    for child in children_of(walker, element) {
        if let Some(found) = find_named(walker, &child, needle, exact, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn invoke(element: &IUIAutomationElement) -> Result<(), String> {
    // SAFETY: 先尝试 Invoke，再尝试 SelectionItem，最后回落到鼠标点击中心点。
    unsafe {
        if let Ok(invoke) =
            element.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
        {
            return invoke
                .Invoke()
                .map_err(|error| format!("Invoke 失败：{error}"));
        }
        if let Ok(select) = element
            .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
        {
            return select
                .Select()
                .map_err(|error| format!("Select 失败：{error}"));
        }
        let rect = element
            .CurrentBoundingRectangle()
            .map_err(|error| format!("读取控件位置失败：{error}"))?;
        let x = (rect.left + rect.right) / 2;
        let y = (rect.top + rect.bottom) / 2;
        crate::computer_tools::click_at(x, y)
    }
}

/// 按名称点击控件。
pub fn click_name(name: &str) -> Result<String, String> {
    let automation = automation()?;
    let root = root()?;
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| format!("创建 UI Automation walker 失败：{error}"))?;
    let element = find_named(&walker, &root, name, false, 0)
        .ok_or_else(|| format!("没有找到名为「{name}」的控件"))?;
    let actual = name_of(&element);
    invoke(&element)?;
    Ok(format!("已点击控件「{actual}」"))
}

/// 读取控件当前值。
pub fn read_value(name: &str) -> Result<String, String> {
    let automation = automation()?;
    let root = root()?;
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| format!("创建 UI Automation walker 失败：{error}"))?;
    let element = find_named(&walker, &root, name, false, 0)
        .ok_or_else(|| format!("没有找到名为「{name}」的控件"))?;
    // SAFETY: 仅读取 ValuePattern。
    unsafe {
        let value = element
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .map_err(|error| format!("控件没有 ValuePattern：{error}"))?;
        Ok(value
            .CurrentValue()
            .map(|value| value.to_string())
            .unwrap_or_default())
    }
}

/// 写入控件值。
pub fn set_value(name: &str, value: &str) -> Result<String, String> {
    let automation = automation()?;
    let root = root()?;
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| format!("创建 UI Automation walker 失败：{error}"))?;
    let element = find_named(&walker, &root, name, false, 0)
        .ok_or_else(|| format!("没有找到名为「{name}」的控件"))?;
    // SAFETY: 仅写入 ValuePattern。
    unsafe {
        let pattern = element
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .map_err(|error| format!("控件没有 ValuePattern：{error}"))?;
        pattern
            .SetValue(&BSTR::from(value))
            .map_err(|error| format!("写入值失败：{error}"))?;
    }
    Ok(format!("已写入控件「{name}」"))
}

#[derive(Clone, Copy)]
enum Action {
    Tree,
    Click,
    Read,
    Write,
}

struct UiTool {
    tool: Tool,
    action: Action,
}

fn define(name: &str, description: &str, parameters: Value, action: Action) -> HarnessTool {
    HarnessTool::new(Arc::new(UiTool {
        tool: Tool {
            name: name.to_string(),
            description: description.to_string(),
            parameters: Schema(parameters),
            constrained_sampling: None,
        },
        action,
    }))
}

/// UI Automation 工具集。
pub fn tools() -> Vec<HarnessTool> {
    vec![
        define(
            "computer_ui_tree",
            "读取桌面/前台窗口的 UI Automation 元素树（名称、类型、AutomationId、位置）。",
            json!({"type": "object", "properties": {}}),
            Action::Tree,
        ),
        define(
            "computer_ui_click",
            "按控件名称（包含匹配）点击界面元素；优先 Invoke/SelectionItem，失败时点中心。",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
            Action::Click,
        ),
        define(
            "computer_ui_read",
            "按控件名称读取当前值（输入框/文本控件）。",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }),
            Action::Read,
        ),
        define(
            "computer_ui_set_value",
            "按控件名称写入值（输入框）。",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "value": {"type": "string"}
                },
                "required": ["name", "value"]
            }),
            Action::Write,
        ),
    ]
}

#[async_trait]
impl AgentTool for UiTool {
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
        let text = (|| -> Result<String, String> {
            match self.action {
                Action::Tree => tree(),
                Action::Click => {
                    let name = params
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "缺少 name".to_string())?;
                    click_name(name)
                }
                Action::Read => {
                    let name = params
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "缺少 name".to_string())?;
                    read_value(name)
                }
                Action::Write => {
                    let name = params
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "缺少 name".to_string())?;
                    let value = params
                        .get("value")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "缺少 value".to_string())?;
                    set_value(name, value)
                }
            }
        })()
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

#[cfg(test)]
mod tests {
    #[test]
    fn tree_has_root() {
        let text = super::tree().expect("uia tree");
        assert!(text.contains("root"));
    }
}
