from pathlib import Path

p = Path("src/backend.rs")
s = p.read_text(encoding="utf-8")

old = '''/// 有效引擎：每个 Bot 的选择优先，其次环境变量，最后 Mode Agent。
pub enum EngineChoice {
    Mode,
    Acp(String),
}

pub fn resolve_engine(selected: Option<&str>) -> EngineChoice {
    if let Some(raw) = selected
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("mode"))
        && let Some(command) = crate::acp::engine_command(raw)
    {
        return EngineChoice::Acp(command);
    }
    match local_engine() {
        Some(command) => EngineChoice::Acp(command),
        None => EngineChoice::Mode,
    }
}

/// 该 Bot 的回合是否由 Mode Agent（v4）执行（附件/频道能力判断用）。
pub fn engine_is_mode(selected: Option<&str>) -> bool {
    matches!(resolve_engine(selected), EngineChoice::Mode)
}'''
new = '''/// 有效引擎：每个 Bot 的选择优先，其次环境变量，最后内嵌 pi-rust。
pub enum EngineChoice {
    /// 内嵌 pi-rust（Mode Chat 默认）。
    Rpi,
    Mode,
    Acp(String),
}

pub fn resolve_engine(selected: Option<&str>) -> EngineChoice {
    if let Some(raw) = selected
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("mode"))
        && let Some(command) = crate::acp::engine_command(raw)
    {
        return EngineChoice::Acp(command);
    }
    match local_engine() {
        Some(command) => EngineChoice::Acp(command),
        None => {
            // 迁移期逃生门：MODE_CHAT_ENGINE=v4 仍可回退旧的 Mode Agent 链路。
            if std::env::var("MODE_CHAT_ENGINE").ok().as_deref() == Some("v4") {
                EngineChoice::Mode
            } else {
                EngineChoice::Rpi
            }
        }
    }
}

/// 该 Bot 的回合是否由完整引擎执行（内嵌 rpi 或旧 Mode Agent；ACP 引擎能力受限）。
pub fn engine_is_mode(selected: Option<&str>) -> bool {
    !matches!(resolve_engine(selected), EngineChoice::Acp(_))
}

/// 建一个内嵌 rpi 会话（新会话/恢复历史共用）。
async fn open_rpi_session(
    key: &str,
    workspace: &str,
    identity: &str,
    model: Option<ModelSpec>,
    resume: Option<String>,
    events: &UnboundedSender<BackendEvent>,
) -> Result<(crate::rpi_engine::RpiSession, ModelSpec), String> {
    let spec = model
        .or_else(env_model)
        .or_else(|| crate::rpi_engine::default_model_spec(identity, workspace))
        .ok_or_else(|| "未选择模型：请先在右栏或设置里为这个 Bot 选一个模型".to_string())?;
    let engine_model = crate::rpi_engine::resolve_model(&spec)?;
    let (approval_level, allowlist) = crate::rpi_engine::approval_policy(identity, workspace);
    let system_prompt = crate::rpi_engine::system_prompt_for(std::path::Path::new(workspace));
    let options = crate::rpi_engine::OpenOptions {
        workspace: workspace.into(),
        resume_path: resume.filter(|path| std::path::Path::new(path).is_file()),
        model: engine_model,
        thinking: crate::rpi_engine::thinking_level_for(&spec),
        system_prompt,
        events: events.clone(),
        key: key.to_string(),
        member: None,
        approval_level,
        allowlist,
    };
    let session = crate::rpi_engine::RpiSession::open(options).await?;
    Ok((session, spec))
}'''
assert s.count(old) == 1, "EngineChoice anchor"
s = s.replace(old, new, 1)

old = '''/// 会话引擎：Mode Agent 走 v4；本地智能体（ACP）走 acp 模块。
#[derive(Clone)]
enum SessionEngine {
    V4,
    Acp(crate::acp::AcpHandle),
}'''
new = '''/// 会话引擎：内嵌 pi-rust / 旧 Mode Agent（v4）/ 本地智能体（ACP）。
#[derive(Clone)]
enum SessionEngine {
    Rpi(Arc<crate::rpi_engine::RpiSession>),
    V4,
    Acp(crate::acp::AcpHandle),
}'''
assert s.count(old) == 1, "SessionEngine anchor"
s = s.replace(old, new, 1)

old = '''                                if let SessionEngine::Acp(handle) = &ctx.engine {
                    if !attachments.is_empty() {'''
new = '''                if let SessionEngine::Rpi(session) = &ctx.engine {
                    let images = match crate::rpi_engine::images_from_attachments(&attachments) {
                        Ok(images) => images,
                        Err(message) => {
                            let _ = events.send(BackendEvent::Session {
                                key,
                                event: SessionEvent::Error {
                                    member: None,
                                    message,
                                },
                            });
                            continue;
                        }
                    };
                    let session = Arc::clone(session);
                    let text = text.clone();
                    tokio::spawn(async move {
                        session.send(text, images).await;
                    });
                    continue;
                }
                                if let SessionEngine::Acp(handle) = &ctx.engine {
                    if !attachments.is_empty() {'''
assert s.count(old) == 1, "Send anchor"
s = s.replace(old, new, 1)

old = '''                    if let SessionEngine::Acp(handle) = &ctx.engine {
                        handle.stop();
                        continue;
                    }'''
new = '''                    if let SessionEngine::Rpi(session) = &ctx.engine {
                        let session = Arc::clone(session);
                        tokio::spawn(async move {
                            session.abort().await;
                        });
                        continue;
                    }
                    if let SessionEngine::Acp(handle) = &ctx.engine {
                        handle.stop();
                        continue;
                    }'''
assert s.count(old) == 1, "Stop anchor"
s = s.replace(old, new, 1)

old = '''                if let SessionEngine::Acp(handle) = &ctx.engine {
                    handle.stop();
                    continue;
                }'''
new = '''                if let SessionEngine::Rpi(session) = &ctx.engine {
                    let session = Arc::clone(session);
                    tokio::spawn(async move {
                        session.abort().await;
                    });
                    continue;
                }
                if let SessionEngine::Acp(handle) = &ctx.engine {
                    handle.stop();
                    continue;
                }'''
assert s.count(old) == 1, "StopMember anchor"
s = s.replace(old, new, 1)

old = '''            Command::Resolve {
                key,
                request_id,
                option_id,
            } => {
                let ctx_key = match sessions.get(&key) {'''
new = '''            Command::Resolve {
                key,
                request_id,
                option_id,
            } => {
                // 内嵌 rpi 的审批卡：request_id 是工具调用 id，按会话闸门逐个尝试。
                let allow = matches!(option_id.as_str(), "allow_once" | "allow_always");
                let mut resolved = false;
                for ctx in sessions.values() {
                    if let SessionEngine::Rpi(session) = &ctx.engine
                        && session.gate.resolve(&request_id, allow)
                    {
                        resolved = true;
                        break;
                    }
                }
                if resolved {
                    continue;
                }
                let ctx_key = match sessions.get(&key) {'''
assert s.count(old) == 1, "Resolve anchor"
s = s.replace(old, new, 1)

ensure = Path("scripts/patch_ensure.txt").read_text(encoding="utf-8")
newsession = Path("scripts/patch_newsession.txt").read_text(encoding="utf-8")
switch = Path("scripts/patch_switch.txt").read_text(encoding="utf-8")
marker = "                    EngineChoice::Mode => {}"
parts = s.split(marker)
assert len(parts) == 4, f"Mode arms count {len(parts)}"
s = parts[0] + ensure + parts[1] + newsession + parts[2] + switch + parts[3]
p.write_text(s, encoding="utf-8", newline="")
print("backend edits applied")
