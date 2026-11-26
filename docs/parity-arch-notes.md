# ModeChat v0.5.0 对标 DS-Bot — 架构侧技术核实

> 作者：架构
> 基准：`D:\Mode Bot\DS-Bot`（v0.3.6 源码）+ 本机 DS Bot 实机
> 被核实项目：`D:\ModeChat`（Rust + GPUI，v0.5.0，576 测试）
> 用途：`docs/parity-matrix.md` 的技术注释来源。每条带 `源文件:行号`，产品可直接搬进矩阵"证据"列。
> 分类口径：
> - **框架性缺口** = 现有结构承载不了，必须先改架构/加抽象层（工期按天算，会带动多个模块）
> - **补实现** = 骨架和抽象已在，缺接线或页面（工期按小时～半天）

---

## 0. 先给结论（优先级排序）

| # | 差距 | 影响 | 类型 | 建议版本 |
|---|---|---|---|---|
| 1 | Agent 没有记忆写入工具（remember/forget/recall 全缺） | 记忆只能靠"回合后回顾"被动产生，用户说"记住这个"无效 | 框架性 | v0.5 必做 |
| 2 | IM 只有微信 + 企微两条通道（飞书/钉钉/QQ/Telegram/Discord/Slack 全缺运行时） | 对标差距最大的一块 | 框架性 | v0.5 至少补飞书 |
| 3 | 连接器只支持 stdio MCP，无 HTTP/SSE、无 OAuth | DS 37 个连接器大多 remote+OAuth，MC 只能用本地命令型 | 框架性 | v0.5 必做 |
| 4 | 浏览器是全局单例，所有 Bot 共用一个 Chrome | 与"每 Bot 独立登录态"的产品承诺直接冲突 | 框架性 | v0.5 必做 |
| 5 | 群目标没有验收标准/证据/打回 | 群派活不闭环，Goal 只是阶段列表 | 半成品 | v0.5 必做 |
| 6 | 本地 Agent 只有 4 个 ACP 引擎，无 Claude/Codex 两种驱动协议 | 覆盖不了用户手头大多数 CLI | 框架性 | v0.5.1 |
| 7 | Office 文档能力为零 | 一个完整功能域缺失 | 缺失 | v0.5.1（工作量最大） |
| 8 | 隐私 Bot 只隔离会话和搜索，没隔离文件与团队外发 | 安全承诺不成立，属 bug 级 | 框架性 | v0.5 必做 |
| 9 | 内置浏览器面板缺失（只有"用系统浏览器打开 URL"） | 体验差距 | 框架性 | v0.5.1 |
| 10 | 定时任务 UI 只有裸 cron 输入框 | 体验差距，实现极便宜 | 补实现 | v0.5 |
| 11 | 用量缺按 Bot/模型分组 | 体验差距 | 补实现 | v0.5 |
| 12 | 移动端与远程配对（link/relay/mobile）完全缺失 | 独立子系统 | 缺失 | v0.6 |

---

## 1. 引擎与工具链

### 1.1 内嵌引擎（rpi / pi-rust）— 已有
- 引擎接入：`src/rpi_engine.rs:1043`（`RpiSession::open`）、`:1227` send、`:1244` steer、`:1267` abort
- 会话持久化 + 恢复：`src/rpi_engine.rs:1275`（import_markdown_session）、`:1376`（history_entries）
- 系统提示：`src/rpi_engine.rs:338`（`system_prompt_for`，拼 `AGENTS.md` + 团队记忆；v0.5 起 `:346` 的 `system_prompt_for_bot` 追加 Bot 自己的记忆）
- 内置工具只有 4 个：`src/rpi_engine.rs:373`（`tool_set`：read / write / edit / bash）
- 结论：引擎本体完整，**不需要改架构**。

### 1.2 上下文压缩（compaction）— 半成品（隐式已有，UI 无感知）
- rpi 侧默认开启：`rpi-harness/src/types.rs:325`（`DEFAULT_COMPACTION_SETTINGS { enabled: true, reserve_tokens: 16384, keep_recent_tokens: 20000 }`）
- MC 没有覆盖该配置：`src/rpi_engine.rs:1137`（`AgentHarnessOptions::default()`）到 `:1158` 只设了 model/tools/system_prompt/thinking/session/emitter/before_tool_call，**所以自动压缩其实在跑**
- 但压缩进度事件没接：`rpi-harness/src/events.rs:147`（`HarnessEvent::CompactionProgress`）。MC 侧 `CompactionSettings` / `HarnessEvent::Compaction` / `/compact` 三个词在 `src` + `crates` 全仓 **0 命中**（`grep compact` 命中的 46 处全是 `compact_tokens` 这个数字格式化函数，如 `src/main.rs:21669`（`compact_tokens`），与上下文压缩无关）；`UiEmitter`（`src/rpi_engine.rs:847` 结构体、`:863` emit）不认识该事件
- 也没有 `/compact` 手动触发入口
- 分类：**补实现**。接一个事件 + 一个命令项即可，1～2 小时。

### 1.3 团队管理工具（Agent 自己管 Bot/群）— 框架性缺口
- DS 有 15 个团队工具：`ds-bot/src/host/tools.js:80`(list_bots)、`:167`(message_bot)、`:298`(stop_bot)、`:325`(create_bot)、`:363`(update_bot)、`:453`(list_bot_models)、`:496`(create_group)、`:526`(update_group)、`:590`(delete_group)、`:607`(post_to_group)、`:670`(ask_user)、`:726`(show_view)、`:1129`(list_capabilities)、`:1153`(save_capability)、`:1183`(delete_capability)
- MC 只有 6 个，且只发给群里的 Chief：`src/team_tools.rs:118`（`tools()`）、`:139`-`:251`（team_roster / team_goal_show / team_goal_create / team_goal_stage / team_task_list / team_assign；v0.5 起 `:542` 的 `assign` 仍在，另加 team_goal_acceptance / team_goal_pass / team_goal_rework 共 9 个）
- MC 全仓 grep `"message_bot"` / `"create_bot"` / `"post_to_group"` / `"list_bots"` **全部 0 命中**（`name: "create_bot"` 亦 0 命中）
- 服务层其实够用：`src/data.rs:117`（create_bot）、`:649`（update_bot）、`:598`（delete_bot）、`src/im.rs:91`/`:102`（建微信/企微 IM bot）
- 缺两件事：
  1. "向另一个 Bot 的会话后台直投一条消息"的通道——`src/backend.rs` 只有群内的 `note_dispatch`（`:1629`），没有跨会话投递原语
  2. 工具层把 data.rs 的建/改/删包出来
- 分类：**框架性**（第 1 点要动 backend 的命令分发），第 2 点顺带就做完。

### 1.4 show_view（图表/表单渲染）— 框架性缺口
- DS：`ds-bot/src/host/tools.js:726`（工具定义）、`ds-bot/src/client/widget-cell.js`、`cells.js`（渲染）
- MC：grep `show_view` **0 命中**；只有 `ask_user_question`（`src/rpi_engine.rs:2211`）这一种结构化交互
- 分类：**框架性**。GPUI 侧要新写 chart/table/form 三种组件 + 一份 JSON 协议，属 UI 侧大件。建议排阶段 3 之后；时间不够可先只做 form + table。

### 1.5 技能（Skills）与能力（Capabilities）— 框架性缺口
- DS：`ds-bot/src/host/capabilities.js:54`（install）、`:92`-`:131`（mcp / skill / pack 三种 kind），skills 由 DSH 注入
- rpi 侧有现成挂点：`AgentHarnessResources.skills`（`rpi-harness/src/types.rs` 的 `resources` 字段）
- MC：`src/rpi_engine.rs:1137`-`1158` **没有设置 `resources`**；`AgentHarnessResources` 在 `src` + `crates` **0 命中**；`skills` 在 `src` **0 命中**（唯一相关是注释 `src/acp.rs:835`）
- 能力页是只读 4 行硬编码：`src/capabilities.rs:15`（`rows()`）
- 分类：**框架性**。要定 `skills/<name>/SKILL.md` 目录约定 + 扫描 + 注入 resources + 能力页改可编辑。

---

## 2. 记忆与压缩

### 2.1 记忆存储层 — 已有，且比 DS 完整
- `crates/bots-service/src/memory_store.rs`（1506 行）：`tidy_memory:659`、`recall_context:752`、`move_memory:805`、`history:1224`、`undo:1258`、`plan_index_overflow:508`、`maintain_index:586`
- 分类：**无需改动**，这是 MC 的强项。

### 2.2 记忆注入 — 半成品（预算写死）
- MC：`src/rpi_engine.rs:357` 注入团队记忆、`src/backend.rs:1667` 注入群记忆，两处都是 `memory_context(root, 4_000)` 固定 4000 字符（`src/data.rs:384`、`:401`）
- DS：按模型上下文窗口动态算预算，`ds-bot/src/host/memory.js:688`（`shownEntries`）+ `ds-bot/src/host/memory-entries.js:75`（`fitWindow`）
- 分类：**补实现**。`memory_context` 已有 `max_chars` 参数，把 4000 换成按模型窗口算的比例即可。

### 2.3 记忆写入工具（remember / forget / recall）— 框架性缺口（记忆域最大问题）
- DS：`ds-bot/src/host/memory.js:854`（bot-memory）、`:1075`（remember）、`:1114`（forget）、`:1151`（recall），另有 `ds-bot/src/host/memory-search.js:9`（rankEntries）、`memory-pending.js`（待确认队列）
- MC：全仓 grep 记忆工具名 **0 命中**。记忆只能由 `src/memory_review.rs`（回合后自动回顾）被动写入，且 `apply_operations` 只写 `mode-bot:group:<id>` 文件记忆（`src/memory_review.rs:341`、`:376`、`:397`）
- 后果：用户对 Bot 说"记住我不吃香菜"，Bot 无法落盘；只能等回顾流程自己判断，且写不进 Bot 自己的私有记忆。
- 分类：**框架性**（新增工具 + 写权限归属判定 + 与 memory_store 的 scope 打通），但存储层不用动，属"接线 + 工具"型框架缺口。**v0.5 必做。**

### 2.4 Bot 私有记忆面板 — 半成品（缺页面）
- DS：`ds-bot/src/client/memory-panel.js:325`（`MemoryDialog`，文件 449 行）
- MC：只有团队记忆页 `src/main.rs:14651`（`settings_page_team_memory`）和群记忆区 `src/main.rs:16057`（`group_memory_section`）
- 分类：**补实现**（纯 UI）。

### 2.5 记忆回顾 — 已有
- `src/memory_review.rs:83`（build_prompt）、`:196`（parse_operations）、`:318`（apply_operations）、`src/backend.rs:755`（memory_review）、`:2239`（run_memory_review）
- 分类：无需改动。

---

## 3. 群聊 / 派活 / 目标

### 3.1 群任务台账与派活 — 已有
- `crates/bot-team/src/tasks.rs`：状态机 `:18`（`TaskState`）、`apply_transition:207`、`route_task:763`
- `src/team_tools.rs:246`（team_assign）、`src/backend.rs:1503`（只有 Chief 拿到团队工具）、`:1957`（note_dispatch）、`src/main.rs:3957`（每回合派活上限 4 次）
- 分类：**无需改架构**。

### 3.2 群目标（Goal）— 半成品（缺验收/证据/打回）
- MC：`crates/bot-team/src/goal.rs:53`（`GoalStage`：只有 id/title/status/note）、`:64`（`GroupGoal`：version/status/title/description/stages）
- DS：有 `acceptance`（`ds-bot/src/host/goals.js:19` `STAGE_ACCEPTANCE_MAX = 1000`）、有 pass/rework 两个动作（`ds-bot/src/host/goals.js:760` / `:767`）、有"值班守则"（`ds-bot/src/host/goals.js:411` 明确要求成员回报后自检或交叉检查再 pass）
- 分类：**半成品**，但闭环关键。数据结构加 2 个字段（`stage.acceptance` / `goal.acceptance`）+ 2 个动作，要带迁移（旧 `goal.json` 无这两字段按空串处理）。**v0.5 必做。**

### 3.3 群内排队消息可视化 — 半成品
- MC：`src/main.rs:5206`（`queue_current`）、`:4730`（`MAX_QUEUED = 5`）、`:4769`（`flush_queue`），排队隐式，界面无独立队列 dock
- DS：`ds-bot/src/client/queue-dock.js:99`（`QueueDock` 独立组件）
- 分类：**补实现**（UI）。

---

## 4. 隐私 Bot

### 4.1 已做的隔离 — 已有
- 数据校验：`src/data.rs:88`（`check_chief_private`）、`:590`（`demote_other_chiefs`）
- 会话落盘隔离：`src/backend.rs:369`（隐私 Bot 不复用主目录旧会话，新会话落 `.private/<id>`）
- 全局搜索排除：`src/main.rs:6527`、`:6623`（`is_private_session`）
- 不进群：`src/main.rs:3771`、`:4881`、`:8007`
- 分类：**无需改动**。

### 4.2 缺的隔离 — 框架性缺口（安全级）
- DS 有三道守卫：`ds-bot/src/host/privacy.js:242`（guardFiles 文件读写归属）、`:96`（ownerOfPath）、`:308`（privateClash 冲突检测）、`:172`（guardTeam 团队外发审批）
- MC：只有 `is_private` 布尔；`before_tool_call` 里只做审批策略（`src/rpi_engine.rs:422` `hook()` → `:435` `decide()`），**没有文件路径归属判定**
- 后果：隐私 Bot 的 read/write/edit/bash 可读任意路径，包括别的 Bot 目录。`README.zh.md:84` 的承诺不成立。
- 分类：**框架性**。要在 `ApprovalGate::decide` 前加"路径归属 → 越界即审批/拒绝"，并定义"哪些目录属于哪个 Bot"。**v0.5 必做。**

---

## 5. IM 通道

### 5.1 现状 — 只有 2 条通道真跑
- 微信（iLink 长轮询）：`src/im_runtime.rs:239`（`refresh`）、`:247`（`filter(|bot| bot.enabled && bot.provider == BotProvider::Weixin)`）、`:647`（`start_loop`）
- 企业微信：`src/im_runtime.rs:295`（`refresh_wecom`）、`:350`（`start_wecom_loop`）、`crates/bots-service/src/wecom_ws.rs`（WS 通道 310 行）
- 扫码注册：`src/im_service.rs:506`（微信）、`:618`（企微），二维码渲染 `:770`
- 平台枚举齐全但只有名字：`crates/bots-service/src/lib.rs:63`（`BotProvider`）、字段也留着（`:291` feishu_app_id、`:311` dingtalk_client_id），但**没有适配器**
- Telegram/Discord 被显式标记下线：`crates/bots-service/src/lib.rs:59`（`RETIRED_BOT_PROVIDERS = ["telegram","webhook","discord","astrbot"]`）
- 分类：飞书/钉钉 = **缺失**；Telegram/Discord = **被主动下线**（与 DS 相反）

### 5.2 DS 的对标面 — 8 个通道
- `ds-bot/src/shared/im-platforms.js:6`（`IM_PLATFORMS`）：telegram / weixin / feishu(lark) / dingtalk / wecom / qq / discord / slack
- 每个有字段声明（token / appId+appSecret / clientId+clientSecret …），运行时在 `ds-bot/src/host/im/platforms/*.js`（8 个文件，每个 190～380 行）
- 卡片交互（提问卡/审批卡按钮）：`ds-bot/src/host/im/index.js:592`（`cardButtons.question`）

### 5.3 结论
- 分类：**框架性缺口**。MC 的 `ImRuntime` 把"微信"写死在 refresh 里（`:247`），不是插件式；要支持飞书必须先把 `ImRuntime` 抽成 `trait ImChannel` + 注册表，再照 `ds-bot/src/host/im/platforms/feishu.js` 的语义写 Rust 适配器。
- 建议：v0.5 先补**飞书**（用户群最可能真用），QQ/钉钉/Telegram/Discord/Slack 排 v0.6。工作量：抽象层 1 天 + 每通道 1～2 天。

---

## 6. 连接器与能力

### 6.1 现状 — 16 个，全部 stdio
- 目录：`crates/connectors-service/src/lib.rs:43`（`CATALOG`）= github / notion / slack / postgres / filesystem / redis / elasticsearch / kubernetes / gitlab / cloudflare / linear / figma / playwright / memory / sequential-thinking / brave-search
- 解析只产出 stdio：`crates/connectors-service/src/lib.rs:448`（`resolve_mcp_servers`），每条写死 `transport: "stdio"`（`:333`）
- MCP 宿主只实现 stdio：`src/mcp_host.rs:1`（注释即写明"轻量 MCP 宿主（stdio）"）、`:224`（`spawn_server` 用 `Command` 起子进程）
- 无 OAuth：`crates/connectors-service/src/lib.rs:42` 注释明确写"OAuth/浏览器回调类…先不列进来"

### 6.2 DS 的对标面 — 37 个，多数 remote + OAuth
- 目录：`ds-bot/src/host/connectors/catalog.js:20`（37 条：github/notion/linear/atlassian/sentry/stripe/paypal/square/cloudflare/vercel/supabase/neon/figma/canva/…）
- 鉴权三种：`ds-bot/src/host/connectors/catalog.js:5`（none / oauth / key）；OAuth 工厂 `ds-bot/src/host/connectors/catalog.js:18`；令牌注入 `ds-bot/src/host/connectors/index.js:90`
- 邮箱连接器是独立子系统：`ds-bot/src/host/connectors/mail/index.js`（591 行）+ `mail/transport.js`（297 行），MC 完全没有

### 6.3 结论
- 分类：**框架性缺口**。要补：① HTTP/SSE transport（`mcp_host.rs` 支持 URL 型 server）② OAuth PKCE 回调链路（本地回调端口 + 浏览器打开）③ 目录从 16 扩到 37 ④ 邮箱。
- 建议 v0.5 只做 ①②（让 github/notion/linear/figma 这类 OAuth 型能真连上），目录扩充和邮箱排 v0.5.1。

---

## 7. 定时任务

### 7.1 引擎层 — 已有，且强于 DS
- cron 解析（五/六/七段、`n#k`、`L/LW`、昵称）：`crates/automation-schedule/src/cron.rs:1`（模块注释）、`:49`（`CronSpec::parse`），注释明说对齐 croner 10
- 仓储/运行/服务三件套：`crates/automation-repo`、`automation-runtime`、`automation-service`
- 工具：`src/automation_tools.rs:71`（automation_create）、`:89`（automation_list）、`:124`（automation_run_now）
- 分类：**无需改架构**。

### 7.2 创建 UI — 半成品（只有裸 cron）
- MC：`src/main.rs:9545`（`SettingsDetail::NewSchedule` 面板 = 标题 + cron 输入框 + 指令 + 重复开关），默认值 `:9575` 的 `"0 9 * * *"`
- DS：四种选择器 at / daily / weekly / every，`ds-bot/src/host/schedules.js:60`（`selectorOf`），有校验文案（"Pick a time in the future" 等）
- 分类：**补实现**（纯 UI + 一个选择器→cron 的转换函数）。半天内可完成，性价比最高。

### 7.3 心跳 — 已有
- 规则：`crates/bots-service/src/heartbeat.rs`（240 行，`is_heartbeat_due` / `pick_heartbeat_conversation`）
- 宿主：`src/im_heartbeat.rs:22`（`heartbeat_conversations` 会话投影）、`src/im_runtime.rs:520`（`start_heartbeat_scheduler`）、`:547`（`tick_heartbeats`）
- 分类：无需改动。

---

## 8. ACP / 本地 Agent

### 8.1 现状 — 4 个引擎，1 种驱动协议
- 引擎白名单：`src/data.rs:366`（`local_agents()`），候选 = mode（内置）+ pi-acp / deepseek-acp / hermes / zcode-acp-server，注释明写"不再探测 Claude/Codex/Gemini 等"
- 命令映射：`src/acp.rs:122`（`engine_command`）、`:139`（`launch_args`，处理 `.js`→node、`zcode`→`server` 子命令、`hermes`→`acp` 子命令）
- 协议实现：`src/acp.rs:54`（`AcpCommand`）、`:167`（`spawn`）、`:387`（`session/new` / `session/load`）、`:472`（`session/request_permission`）、`:108`（`resolve_elicitation`）
- 分类：**已有 ACP 一种协议的全部要素**（含权限卡、提问卡、续会话）。

### 8.2 DS 的对标面 — 10 个引擎，3 种驱动
- `ds-bot/src/host/local-agents/engines.js:5`（`KINDS = ['acp','codex','claude']`）、`:16`（devin/claude-code/codex-cli/opencode/cursor-cli/gemini-cli/grok-cli/cline/qwen-code/goose）
- 三种驱动各有实现：`ds-bot/src/host/local-agents/acp.js`(267)、`codex.js`(204)、`claude.js`(232)，统一在 `adapter.js`(615)
- 还有 `discover.js`(153) 自动探测 + `ds-bot/src/host/local-agents/mcp.js`(131) 给外部 CLI 注入团队工具

### 8.3 结论
- 分类：**框架性缺口**。要支持 Claude Code / Codex CLI 必须新增两种驱动（stream-json / app-server JSON-RPC），`acp.rs` 目前单协议直连，没有 driver 抽象。
- 建议：v0.5 先把 4 个 ACP 引擎做稳（探测、签名状态、错误文案），v0.5.1 再加 `trait AgentDriver` + claude/codex 两个实现。
- 另：DS 通过 MCP 把团队工具注给外部 CLI（`local-agents/mcp.js`），MC 没有这条路径，同属一个框架缺口。

---

## 9. 浏览器

### 9.1 工具层 — 已有（纯 Rust CDP）
- `src/browser_tools.rs:116`（`launch`，独立 profile + 随机调试端口）、`:382`-`:583`（20 个 browser_* 工具）、`:862`（先全选再 CDP 插入，绕开输入兼容）
- 分类：工具层**不用改**。

### 9.2 会话归属 — 框架性缺口
- MC：`src/browser_tools.rs:28`（`static SESSION: OnceLock<tokio::sync::Mutex<Option<Arc<Browser>>>>`）——**全局单例**，`:274`（`get_session()`）谁调用都拿到同一个
- DS：每 Bot 一个浏览器实例（`README.zh.md:96`"每个 bot 有自己的浏览器，登录状态互不干扰"；`ds-bot/src/host/browser/session.js:94` `openBotBrowser({ botId, profileRoot, ... })`）
- 后果：两个 Bot 同时用浏览器会互相踩页面/登录态，与产品承诺直接冲突。
- 分类：**框架性**，改动明确：单例换 `HashMap<botId, Arc<Browser>>` + 每 Bot 一个 `user-data-dir`。**v0.5 必做**（1 天内）。

### 9.3 内置浏览器面板 — 框架性缺口
- MC：`src/main.rs:8671`（`browser_panel_section`）= 只看"最近打开的 URL" + "已安装浏览器"，点了调系统浏览器（`:8297` `shell_open::open_url`）
- DS：真正的内嵌窗格 `ds-bot/src/client/browser-pane.js:523`（`BotPanel`，838 行）+ `bot-browsers.js`（616 行），带标签页、favicon、接管按钮
- 分类：**框架性**——要在 GPUI 里嵌 WebView（GPUI 当前无 webview 组件），属技术风险项，见第 13 节。
- 建议 v0.5.1 评估，不在 v0.5 承诺。

---

## 10. 电脑操控

### 10.1 工具层 — 已有
- `src/computer_tools.rs:103-244`（14 个：screenshot / screen_size / move / click / type / key / scroll / windows / focus_window / window_screenshot / drag / clipboard_get / clipboard_set / cursor_pos）
- UI 树：`src/cua_tree.rs:277`-`:306`（`computer_ui_tree` / `ui_click` / `ui_read` / `ui_set_value`）
- 分类：**不用改**。

### 10.2 治理层 — 半成品
- DS：`ds-bot/src/host/computer/index.js`（1259 行，含档位）+ `ds-bot/src/host/computer/leases.js:7`（`createLeases`，**同一应用同一时间只允许一个 Bot 操作**）+ `ds-bot/src/host/computer/catalog.js`（86 行，应用白名单）+ 客户端顶部指示条 `app/src/computer-indicator.mjs`（138 行）；README 明确"档位：只看 / 后台操作 / 可以动前台"、"系统设置、密码管理这类应用永远不会交给 bot"（`README.zh.md:102`）
- MC：grep `lease` 在 `src/` **0 命中**（只命中 `ReleaseDC` 等 Win32 API 名，如 `src/computer_tools.rs:301`），无档位、无应用白名单（`ComputerLevel` / `allowed_apps` / `app_allowlist` 均 0 命中）、无顶部指示条（`indicator` 0 命中）；`computer_tools.rs` 直接操作
- 分类：**半成品偏框架**。要加：`ComputerLease`（按 app/窗口持有）、档位枚举（并入 `tool_scope::Family`）、应用黑名单、UI 顶部指示条。**v0.5 做档位+黑名单（安全相关），lease 和指示条 v0.5.1。**

---

## 11. Office

- MC：**零实现**。全仓只有 MIME 映射：`crates/bots-service/src/media.rs:126`（pdf/docx/xlsx/pptx 的 content-type）
- DS：完整一套 `ds-bot/src/office/`：`docx.js`(612) / `pptx.js`(390) / `xlsx.js`(125) / `check.js`(314) / `package.js`(211) / `png.js`(175) / `zip.js`(140) / `xml.js`(114) / `cli.js`(179)
- 分类：**缺失**。Rust 侧可用 `zip` + `quick-xml` 直接读写 OOXML，不需要 Node，但工作量大（新功能域，估 5～8 人天）。建议 v0.5.1，或 v0.5 只做"读 docx/xlsx 提取文本"垫底。

---

## 12. 用量

- MC 已有：本会话 `src/rpi_engine.rs:1981`（`session_usage`）、总量 `:1993`（`usage_totals`）、按天 `:1692`（`usage_daily`），页面 `src/main.rs:13976`（四张卡 + 7 天堆叠柱），数据源是扫 JSONL
- 缺：**按 Bot / 按模型分组**（DS：`ds-bot/src/host/usage.js:182` SQL `GROUP BY session, provider, model, kind, slot`，客户端 `usage-page.js` 732 行）；费用估算两边都没有价格表，不算差距
- 分类：**补实现**。`UsageSummary`（`src/session_db.rs`）加 provider/model 维度聚合 + 页面加分组切换。半天。

---

## 13. 技术风险（提前报）

1. **GPUI 里嵌 WebView**（9.3）——本项目唯一"可能做不出来"的地方。GPUI 没有 webview 组件，要自己用 `windows` crate 建 `WebView2` 子窗口并做 GPUI 布局对齐、层级、焦点、缩放。建议先做 2 小时 spike，再决定 v0.5 是否承诺。
2. **隐私 Bot 文件隔离**（4.2）——改动会碰所有工具的 `before_tool_call` 路径，需回归全部工具。但这是安全承诺，不能拖。
3. **IM 抽象层重构**（5.3）——`im_runtime.rs` 2498 行、`im_service.rs` 782 行，抽 trait 会动现有微信/企微两条已跑通通道，回归成本高。建议先写飞书适配器、**不动**微信/企微代码路径，用注册表旁挂。
4. **数据结构改动必须有迁移**（群目标加 acceptance、多档位、浏览器 profile）——按 goal 规则"旧 `~/.mode-bot` 数据不得丢失"，每个字段按"缺失即默认"处理，不允许收紧 `deny_unknown_fields`。

---

## 14. 本阶段给产品的"最小可交付"建议

v0.5.0 只承诺这 8 项（技术侧确认可行、影响最大）：

1. 记忆写入工具 remember/forget/recall（2.3）
2. 隐私 Bot 文件/外发隔离（4.2）
3. 群目标 acceptance + pass/rework（3.2）
4. 浏览器按 Bot 隔离（9.2）
5. 连接器 HTTP transport + OAuth（6.3 的 ①②）
6. 飞书 IM 通道（5.3）
7. 定时任务四种选择器（7.2）
8. 用量按 Bot/模型分组（12）

其余（Office、内置浏览器面板、show_view、Claude/Codex 驱动、移动端）明确标 v0.5.1 / v0.6，不进 v0.5.0 验收。

---

## 15. 补充：其余功能域差距（矩阵别只覆盖 12 个大项）

| 功能点 | MC 现状（证据） | DS 现状（证据） | 判定 | 类型 |
|---|---|---|---|---|
| 多语言 | 只有中/英两档：`src/i18n.rs:12`（`set_lang` 只认 `"en"`，其余中文），下拉只有两项 `src/main.rs:12030`（`language_row`） | 7 种：`ds-bot/src/client/i18n.js:1695`（zh-TW/ja/ko/es/fr/de + 默认中英），另有 6 份 locale 各 2270～2278 行 | 缺失 | 补实现（体力活） |
| 语音朗读 | 0 命中（`tts`/`speech`/`voice` 在 `src` 全为 0） | `ds-bot/src/client/voice.js:81`（`VoiceMode`）、`ds-bot/src/client/speech.js`(152)、`ds-bot/src/client/read-page.js:22`（`readPage`） | 缺失 | 框架性（接系统 TTS） |
| 手机端 / 远程配对 | 0 命中（`pairing`/`PairCode`/`mobile` 全为 0） | `link/src/pairing.ts:55`（`pairingUri`）、`ds-bot/src/host/phone.js:18`、`ds-bot/src/host/mobile.js:160`、`mobile/`（React Native 整套） | 缺失 | 框架性（独立子系统，v0.6） |
| 学习卡（技能自学习） | 0 命中（`learning` 全为 0） | `ds-bot/src/client/learning-card.js:164`（`LearningTurnTail`）、`ds-bot/src/host/learning.js:296`（install） | 缺失 | 框架性 |
| 诊断 / 反馈页 | 只有"复制诊断"文本：`src/main.rs:14384`（`settings_page_feedback`，内容是一段静态格式化串 `:14386`）、`src/i18n.rs:587` | `ds-bot/src/client/diagnostics.js:42`（`clientFacts`）、`ds-bot/src/client/feedback.js:46`（`FeedbackPage`）、`:126`（`ErrorToast`） | 半成品 | 补实现 |
| 主题 / 强调色 | 已有：`src/theme.rs`(595)、`src/wechat_theme.rs`(236)、`src/main.rs:12046`（`accent_row`） | `client/themes.js`(124) | 已有 | — |
| 托盘 / 自启 / 自动更新 | 已有：`src/tray.rs:265`、`src/autostart.rs:34`、`src/update.rs:69` | `app/src/tray.mjs`、`app/src/updates.mjs`(215) | 已有 | — |
| 头像 / Bot 图标 | 已有：`src/avatar.rs`(262)、`bot_icon.rs`(120)、`agent_icons.rs`(70)、`brand_icons.rs`(307) | `client/avatar-library.js`(149)、`agent-logos.js`(52) | 已有 | — |
| 会话全文搜索 | 已有：`src/session_db.rs`(381)、`src/main.rs:1030`（`FindState`）+ `:2859`（`find_dialog`）、隐私过滤 `:6527`、`src/rpi_engine.rs:1536`（`search_sessions`） | `ds-bot/src/host/chat-index.js`(276) | 已有 | — |
| 旁路会话（side chat） | 已有：`src/main.rs:7411`（`PaletteCmd::SideChat` 定义 → `:7567` 派发 / `:7802` `open_side`） | `ds-bot/src/client/side-chat.js`(155)、`host/side.js`(377) | 已有 | — |
| 队列可视化 | 有上限 5 条但无独立 UI：`src/main.rs:4730` | `ds-bot/src/client/queue-dock.js:99`（`QueueDock`） | 半成品 | 补实现 |

---

## 16. 附：可直接抄进 parity-matrix 的 63 条（证据已核）

| # | 功能域 | 条目 | 判定 | 证据（MC） | 证据（DS） |
|---|---|---|---|---|---|
| 1 | Bot | 新建/编辑/删除 Bot | 已有 | `src/data.rs:117` / `:649` / `:598`；`src/main.rs:3737` | `ds-bot/src/host/tools.js:325/363` |
| 2 | Bot | 头像与图标库 | 已有 | `src/avatar.rs`(262)、`src/bot_icon.rs`(120) | `ds-bot/src/client/avatar-library.js`(149) |
| 3 | Bot | 每 Bot 模型选择 | 已有 | `src/main.rs:10156`（models 页）、`src/rpi_engine.rs:55` | `ds-bot/src/host/models.js:101` |
| 4 | Bot | 工作目录切换 | 已有 | `src/data.rs:553` | `ds-bot/src/host/tools.js:363` |
| 5 | Bot | 权限/审批三档 + 白名单 | 已有 | `src/rpi_engine.rs:267`（approval_policy）、`:422`（hook） | `ds-bot/src/host/approvals.js`(110) |
| 6 | Bot | 记忆面板（查看/编辑/撤销） | 半成品（仅团队/群） | `src/main.rs:14651`、`:14426` | `ds-bot/src/client/memory-panel.js:325` |
| 7 | Bot | 记忆写入工具 remember/forget/recall | 缺失 | 0 命中 | `ds-bot/src/host/memory.js:1075/1114/1151` |
| 8 | Bot | 插话（steer） | 已有 | `src/backend.rs:697`（interject）、`src/rpi_engine.rs:1244` | `ds-bot/src/host/steer.js`(255) |
| 9 | Bot | 旁路会话 /side | 已有 | `src/main.rs:7795`（open_side） | `ds-bot/src/host/side.js`(377) |
| 10 | Bot | 会话全文搜索 | 已有 | `src/main.rs:1030`、`src/rpi_engine.rs:1536` | `ds-bot/src/host/chat-index.js`(276) |
| 11 | Main bot | Chief 身份与降级规则 | 已有 | `src/data.rs:88`、`:590` | `ds-bot/src/host/roster.js`(900) |
| 12 | Main bot | 管理其它 Bot/群（工具化） | 缺失 | 0 命中 | `ds-bot/src/host/tools.js:325/363/496/526/590` |
| 13 | 群聊 | 建群/改群/删群 | 已有 | `src/data.rs:1816`（create_group）、`src/main.rs:3717` | `ds-bot/src/host/groups.js`(1885) |
| 14 | 群聊 | 群成员路由（@/全员/管理员） | 已有 | `src/main.rs:4517`（route_group_members） | `ds-bot/src/host/groups.js` |
| 15 | 群聊 | 群任务台账 + 派活 | 已有 | `src/team_tools.rs:246`、`src/backend.rs:1503` | `ds-bot/src/host/tools.js` |
| 16 | 群聊 | 群目标（阶段） | 半成品 | `crates/bot-team/src/goal.rs:53/64` | `ds-bot/src/host/goals.js:670` |
| 17 | 群聊 | 目标验收标准 pass/rework | 缺失 | 0 命中 | `ds-bot/src/host/goals.js:760/767` |
| 18 | 群聊 | 群记忆自动回顾 | 已有 | `src/memory_review.rs:83`、`src/backend.rs:2239` | `ds-bot/src/host/memory-review.js:23` |
| 19 | 群聊 | 排队消息 dock | 半成品 | `src/main.rs:4730` | `ds-bot/src/client/queue-dock.js:99` |
| 20 | 隐私 bot | 独立会话落盘 + 搜索排除 | 已有 | `src/backend.rs:369`、`src/main.rs:6527` | `ds-bot/src/host/privacy.js:222` |
| 21 | 隐私 bot | 文件归属守卫 | 缺失 | 0 命中 | `ds-bot/src/host/privacy.js:242` |
| 22 | 隐私 bot | 团队外发审批 | 缺失 | 0 命中 | `ds-bot/src/host/privacy.js:172` |
| 23 | 本地 Agent | ACP 引擎接入（pi/deepseek/hermes/zcode） | 已有 | `src/acp.rs:122/167`、`src/data.rs:366` | `ds-bot/src/host/local-agents/acp.js`(267) |
| 24 | 本地 Agent | Claude Code / Codex 驱动 | 缺失 | 0 命中（只有 acp 一种） | `ds-bot/src/host/local-agents/engines.js:5`、`codex.js`(204)、`claude.js`(232) |
| 25 | 本地 Agent | 引擎自动探测 + 登录态 | 部分 | `src/data.rs:366`（探 PATH） | `ds-bot/src/host/local-agents/discover.js`(153) |
| 26 | 本地 Agent | 给外部 CLI 注入团队工具（MCP） | 缺失 | 0 命中 | `ds-bot/src/host/local-agents/mcp.js`(131) |
| 27 | 浏览器 | browser_* 工具（20 个） | 已有 | `src/browser_tools.rs:382`-`:583` | `ds-bot/src/host/browser/session.js` |
| 28 | 浏览器 | 每 Bot 独立浏览器 profile | 缺失（全局单例） | `src/browser_tools.rs:28`、`:274` | `ds-bot/src/host/browser/session.js:94` |
| 29 | 浏览器 | Cookie 导入 | 缺失 | 0 命中 | `app/src/cookie-import.mjs:41`、`app/src/cookie-parse.mjs:115` |
| 30 | 浏览器 | 内置浏览器窗格（标签/接管） | 缺失 | `src/main.rs:8671`（只有"用系统浏览器打开"） | `ds-bot/src/client/browser-pane.js:523` |
| 31 | 电脑操控 | computer_* 工具（14 个）+ UI 树 4 个 | 已有 | `src/computer_tools.rs:103-244`、`src/cua_tree.rs:277`-`:306` | `ds-bot/src/host/computer/index.js`(1259) |
| 32 | 电脑操控 | 档位 / 应用白名单 / lease / 顶部指示条 | 缺失 | 0 命中 | `ds-bot/src/host/computer/leases.js:7`、`computer/catalog.js` |
| 33 | IM | 微信（扫码 + 长轮询） | 已有 | `src/im_runtime.rs:247/647`、`src/im_service.rs:506` | `ds-bot/src/host/im/platforms/weixin.js`(343) |
| 34 | IM | 企业微信（含 WS 长连接） | 已有 | `src/im_runtime.rs:295/350`、`crates/bots-service/src/wecom_ws.rs` | `ds-bot/src/host/im/platforms/wecom.js`(228) |
| 35 | IM | 飞书 / Lark | 缺失 | 只有枚举名 `crates/bots-service/src/lib.rs:68` | `ds-bot/src/host/im/platforms/feishu.js`(383) |
| 36 | IM | 钉钉 | 缺失 | 只有枚举名 `crates/bots-service/src/lib.rs:72` | `ds-bot/src/host/im/platforms/dingtalk.js`(236) |
| 37 | IM | QQ / Telegram / Discord / Slack | 缺失/下线 | `crates/bots-service/src/lib.rs:59`（telegram/discord 标 retired） | `ds-bot/src/host/im/platforms/qq.js`(238) 等 |
| 38 | IM | 群激活策略 / 群审批 / 心跳 | 已有 | `src/im.rs:179/203/212`、`src/im_runtime.rs:520` | `ds-bot/src/host/im/index.js` |
| 39 | 连接器 | 本地 MCP（16 个，stdio） | 已有 | `crates/connectors-service/src/lib.rs:43`（CATALOG） | `ds-bot/src/host/connectors/catalog.js:20` |
| 40 | 连接器 | 远程 MCP（HTTP/SSE） | 缺失 | `src/mcp_host.rs:1`（只 stdio） | `ds-bot/src/host/connectors/index.js` |
| 41 | 连接器 | OAuth 授权链路 | 缺失 | `crates/connectors-service/src/lib.rs:42`（注释明说不做） | `ds-bot/src/host/connectors/oauth.js`(177) |
| 42 | 连接器 | 邮箱（Gmail/QQ/163） | 缺失 | 0 命中 | `ds-bot/src/host/connectors/mail/index.js`(591) |
| 43 | 能力 | 共享 MCP/Skill/Pack 管理 | 缺失（只读 4 行） | `src/capabilities.rs:15` | `ds-bot/src/host/capabilities.js:54/92` |
| 44 | 能力 | Skill 注入到引擎 | 缺失 | 0 命中（未设 resources） | `ds-bot/src/host/capabilities.js:125` |
| 45 | 能力 | show_view（图表/表格/表单） | 缺失 | 0 命中 | `ds-bot/src/host/tools.js:726` |
| 46 | Office | docx/pptx/xlsx 读写与渲染检查 | 缺失 | 只有 MIME：`crates/bots-service/src/media.rs:126` | `ds-bot/src/office/docx.js`(612) 等 9 个 |
| 47 | 用量 | 会话/总量/按天 | 已有 | `src/rpi_engine.rs:1981/1993/1692`、`src/main.rs:13976` | `ds-bot/src/host/usage.js`(630) |
| 48 | 用量 | 按 Bot / 模型分组 | 缺失 | 未见聚合维度 | `ds-bot/src/host/usage.js:182` |
| 49 | 记忆 | Markdown 记忆 + 索引 + 历史 + 撤销 | 已有 | `crates/bots-service/src/memory_store.rs:659/1224/1258` | `ds-bot/src/host/memory.js` |
| 50 | 记忆 | 团队/群双作用域 | 已有 | `src/data.rs:380/396` | `ds-bot/src/host/memory.js:854` |
| 51 | 记忆 | 上下文预算按模型窗口自适应 | 缺失（写死 4000） | `src/data.rs:384/401` | `ds-bot/src/host/memory-entries.js:75` |
| 52 | 定时任务 | cron 引擎（五/六/七段） | 已有 | `crates/automation-schedule/src/cron.rs:49` | `ds-bot/src/host/schedules.js` / croner |
| 53 | 定时任务 | 创建向导（at/daily/weekly/every） | 缺失（只有裸 cron） | `src/main.rs:9545` | `ds-bot/src/host/schedules.js:60` |
| 54 | 定时任务 | Agent 侧创建/查询定时任务工具 | 已有 | `src/automation_tools.rs:71/89/124` | `ds-bot/src/host/schedules.js:211` |
| 55 | ACP | 会话/权限卡/提问卡 | 已有 | `src/acp.rs:387/472/108` | `ds-bot/src/host/local-agents/acp.js` |
| 56 | ACP | 断线重连 / 会话恢复 | 已有 | `src/acp.rs:167`（resume 走 session/load） | `ds-bot/src/host/resume.js`(451) |
| 57 | 通用 | 供 Agent 使用的 ask_user | 已有 | `src/rpi_engine.rs:2211` | `ds-bot/src/host/tools.js:670` |
| 58 | 通用 | 多语言（>2） | 缺失 | `src/i18n.rs:12`（只 zh/en） | `ds-bot/src/client/i18n.js:1695` |
| 59 | 通用 | 语音朗读 | 缺失 | 0 命中 | `ds-bot/src/client/voice.js:81`、`ds-bot/src/client/read-page.js:22` |
| 60 | 通用 | 手机端 / 远程配对 | 缺失 | 0 命中 | `link/src/pairing.ts:55`、`ds-bot/src/host/mobile.js:160` |
| 61 | 通用 | 诊断 / 反馈 | 半成品 | `src/main.rs:14384` | `ds-bot/src/client/diagnostics.js:42`、`ds-bot/src/client/feedback.js:46` |
| 62 | 通用 | 主题 / 托盘 / 自启 / 自动更新 | 已有 | `src/theme.rs`、`src/tray.rs:265`、`src/autostart.rs:34`、`src/update.rs:69` | `app/src/tray.mjs`、`app/src/updates.mjs` |
| 63 | 通用 | 密钥库（只看已存名字） | 已有 | `src/secrets.rs:33/44` | `ds-bot/src/host/secrets.js`(553) |

---

## 17. 复现与自查

```powershell
# 1) 全部 `文件:行号` 证据的一次性校验（green=命中）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-arch-anchors.ps1

# 2) 零命中声明的复核（每条应为 0）
cd D:\ModeChat
(Get-ChildItem src,crates -Recurse -File -Include *.rs | Select-String -Pattern 'show_view' | Measure-Object).Count
(Get-ChildItem src,crates -Recurse -File -Include *.rs | Select-String -Pattern '"message_bot"|"create_bot"' | Measure-Object).Count
(Get-ChildItem src,crates -Recurse -File -Include *.rs | Select-String -Pattern 'CompactionSettings|HarnessEvent::Compaction' | Measure-Object).Count
(Get-ChildItem src,crates -Recurse -File -Include *.rs | Select-String -Pattern 'AgentHarnessResources' | Measure-Object).Count
(Get-ChildItem src,crates -Recurse -File -Include *.rs | Select-String -Pattern 'ComputerLease|allowed_apps|app_allowlist' | Measure-Object).Count

# 3) 编译基线
cargo check --workspace --message-format=short
```

本次实测（2026-11-25）：

```
$ cargo check --workspace --message-format=short
    Checking tasks-db v0.4.0 (D:\ModeChat\crates\tasks-db)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.83s
exit=0

show_view                                       0
"message_bot"|"create_bot"                      0
CompactionSettings|HarnessEvent::Compaction     0
AgentHarnessResources                           0
ComputerLease|allowed_apps|app_allowlist        0
```
