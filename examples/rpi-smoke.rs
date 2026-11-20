//! 内嵌 pi-rust 的最小烟测：
//! 读取环境变量 → 建 Provider/Model → 内存会话 → AgentHarness → 发一句话 → 打印流式事件。
//!
//! 运行（PowerShell）：
//!   $env:RPI_API_KEY="..."; $env:RPI_BASE_URL="https://tokendance.space/gateway";
//!   $env:RPI_MODEL="deepseek-v4.1-flash"; $env:RPI_API="anthropic";
//!   cargo run --example rpi-smoke

use std::env;
use std::io::Write;
use std::sync::Arc;

use futures::future::BoxFuture;
use rpi_agent::{AgentEmitter, AgentEvent};
use rpi_ai::providers::anthropic::AnthropicProvider;
use rpi_ai::providers::openai_completions::OpenAiCompletionsProvider;
use rpi_ai::{Api, AssistantMessageEvent, InputModality, Model, Provider};
use rpi_harness::session::types::SessionRepo;
use rpi_harness::session::{
    CounterIdGenerator, InMemorySessionRepo, Session, SystemClock,
};
use rpi_harness::types::{AgentHarnessOptions, HarnessTool};
use rpi_harness::agent_harness::AgentHarness;
use rpi_tools::{
    create_bash_tool, create_edit_tool, create_read_tool, create_write_tool, ExecutionToolContext,
    OsExecutionEnv,
};

struct Printer;

impl AgentEmitter for Printer {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'static, ()> {
        self.try_emit(event);
        Box::pin(async {})
    }

    fn try_emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => match assistant_message_event {
                AssistantMessageEvent::TextDelta { delta, .. } => {
                    print!("{delta}");
                    let _ = std::io::stdout().flush();
                }
                AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                    eprint!("{delta}");
                }
                other => {
                    let text = format!("{other:?}");
                    eprintln!("[assistant] {}", text.chars().take(600).collect::<String>());
                }
            },
            AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => println!("\n[tool] {tool_name} {args}"),
            AgentEvent::ToolExecutionEnd {
                tool_name, is_error, ..
            } => println!("[tool end] {tool_name} error={is_error}"),
            other => {
                let text = format!("{other:?}");
                eprintln!("[event] {}", text.chars().take(400).collect::<String>());
            }
        }
    }
}

#[tokio::main]
#[allow(clippy::field_reassign_with_default)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = env::var("RPI_API_KEY")?;
    let base_url =
        env::var("RPI_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".to_string());
    let model_id = env::var("RPI_MODEL").unwrap_or_else(|_| "deepseek-chat".to_string());
    let api = match env::var("RPI_API").unwrap_or_else(|_| "openai".to_string()).as_str() {
        "anthropic" => Api::AnthropicMessages,
        _ => Api::OpenaiCompletions,
    };
    let prompt = env::var("RPI_PROMPT").unwrap_or_else(|_| "只回复两个字：收到".to_string());

    // 必须与 Provider::id() 一致：harness 按 model.provider 找 provider。
    let provider_id = match api {
        Api::AnthropicMessages => "anthropic",
        _ => "mode",
    };
    let mut model = Model::new(
        model_id.clone(),
        model_id.clone(),
        api.clone(),
        provider_id,
        base_url.clone(),
    );
    model.input = vec![InputModality::Text, InputModality::Image];
    model.context_window = 128_000;
    model.max_tokens = 8_192;

    let http = reqwest012::Client::builder().build()?;
    let provider: Arc<dyn Provider> = match api {
        Api::AnthropicMessages => Arc::new(AnthropicProvider::with_models(
            Some(key),
            http,
            vec![model.clone()],
        )),
        _ => Arc::new(OpenAiCompletionsProvider::with_models(
            "mode",
            Some(key),
            http,
            vec![model.clone()],
        )),
    };

    let cwd = env::current_dir()?;
    let env_handle: Arc<OsExecutionEnv> = Arc::new(OsExecutionEnv::with_cwd(cwd));
    let tool_ctx = ExecutionToolContext::new(env_handle.clone(), Some(env_handle));
    let tools = vec![
        HarnessTool::new(create_read_tool(&tool_ctx, None)),
        HarnessTool::new(create_write_tool(&tool_ctx)),
        HarnessTool::new(create_edit_tool(&tool_ctx)),
        HarnessTool::new(create_bash_tool(&tool_ctx, None)),
    ];

    let repo = InMemorySessionRepo::new(
        Arc::new(SystemClock),
        Arc::new(CounterIdGenerator::new()),
    );
    let storage = repo.create(&Default::default()).await?;
    let session = Session::new(Arc::new(storage), None);

    let mut options = AgentHarnessOptions::default();
    options.model = model;
    options.models = vec![provider];
    options.tools = tools;
    options.active_tool_names = ["read", "write", "edit", "bash"]
        .into_iter()
        .map(String::from)
        .collect();
    options.system_prompt = Some("你是 Mode Chat 的内嵌测试助手。".to_string());
    options.session = session;
    options.agent_emitter = Some(Arc::new(Printer));

    let harness = AgentHarness::create(options).await?;
    eprintln!("[smoke] session={} model={model_id}", harness.session_id());
    let lane = harness.lane("main");
    lane.prompt_text(&prompt, Vec::new()).await?;
    println!("\n[smoke] done");
    Ok(())
}
