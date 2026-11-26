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
- 但压缩进度事件没接：`rpi-harness/src/events.rs:147`（`HarnessEvent::CompactionProgress`）。MC 侧 `CompactionSettings` / `HarnessEvent::Compaction` / `/compact` 三个词在 `src` + `crates` 全仓 **0 命中**（`grep compact` 命中的 46 处全是 `compact_tokens` 这个数字格式化函数，如 ``fn compact_tokens`（`src/main.rs`），与上下文压缩无关）；`UiEmitter`（`src/rpi_engine.rs:847` 结构体、`:863` emit）不认识该事件
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
- MC：只有团队记忆页 `src/main.rs:14657`（`settings_page_team_memory`）和群记忆区 `src/main.rs:16063`（`group_memory_section`）
- 分类：**补实现**（纯 UI）。

### 2.5 记忆回顾 — 已有
- `src/memory_review.rs:83`（build_prompt）、`:196`（parse_operations）、`:318`（apply_operations）、`src/backend.rs:755`（memory_review）、`:2239`（run_memory_review）
- 分类：无需改动。

---

## 3. 群聊 / 派活 / 目标

### 3.1 群任务台账与派活 — 已有
- `crates/bot-team/src/tasks.rs`：状态机 `:18`（`TaskState`）、`apply_transition:207`、`route_task:763`
- `src/team_tools.rs:246`（team_assign）、`src/backend.rs:1503`（只有 Chief 拿到团队工具）、`:1957`（note_dispatch）、`src/main.rs:3959`（每回合派活上限 4 次）
- 分类：**无需改架构**。

### 3.2 群目标（Goal）— 半成品（缺验收/证据/打回）
- MC：`crates/bot-team/src/goal.rs:53`（`GoalStage`：只有 id/title/status/note）、`:64`（`GroupGoal`：version/status/title/description/stages）
- DS：有 `acceptance`（`ds-bot/src/host/goals.js:19` `STAGE_ACCEPTANCE_MAX = 1000`）、有 pass/rework 两个动作（`ds-bot/src/host/goals.js:760` / `:767`）、有"值班守则"（`ds-bot/src/host/goals.js:411` 明确要求成员回报后自检或交叉检查再 pass）
- 分类：**半成品**，但闭环关键。数据结构加 2 个字段（`stage.acceptance` / `goal.acceptance`）+ 2 个动作，要带迁移（旧 `goal.json` 无这两字段按空串处理）。**v0.5 必做。**

### 3.3 群内排队消息可视化 — 半成品
- MC：`src/main.rs:5209`（`queue_current`）、`:4733`（`MAX_QUEUED = 5`）、`:4772`（`flush_queue`），排队隐式，界面无独立队列 dock
- DS：`ds-bot/src/client/queue-dock.js:99`（`QueueDock` 独立组件）
- 分类：**补实现**（UI）。

---

## 4. 隐私 Bot

### 4.1 已做的隔离 — 已有
- 数据校验：`src/data.rs:88`（`check_chief_private`）、`:590`（`demote_other_chiefs`）
- 会话落盘隔离：`src/backend.rs:369`（隐私 Bot 不复用主目录旧会话，新会话落 `.private/<id>`）
- 全局搜索排除：`src/main.rs:6530`、`:6626`（`is_private_session`）
- 不进群：`src/main.rs:3771`、`:4884`、`:8010`
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
- MC：`src/main.rs:9548`（`SettingsDetail::NewSchedule` 面板 = 标题 + cron 输入框 + 指令 + 重复开关），默认值 `:9578` 的 `"0 9 * * *"`
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
- MC：`src/main.rs:8674`（`browser_panel_section`）= 只看"最近打开的 URL" + "已安装浏览器"，点了调系统浏览器（`:8300` `shell_open::open_url`）
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

- MC 已有：本会话 `src/rpi_engine.rs:1981`（`session_usage`）、总量 `:1993`（`usage_totals`）、按天 `:1692`（`usage_daily`），页面 `src/main.rs:13982`（四张卡 + 7 天堆叠柱），数据源是扫 JSONL
- 缺：**按 Bot / 按模型分组**（DS：`ds-bot/src/host/usage.js:182` SQL `GROUP BY session, provider, model, kind, slot`，客户端 `usage-page.js` 732 行）；费用估算两边都没有价格表，不算差距
- 分类：**补实现**。`UsageSummary`（`src/session_db.rs`）加 provider/model 维度聚合 + 页面加分组切换。半天。

---

## 13. 技术风险（提前报）

1. **GPUI 里嵌 WebView**（9.3）——本项目唯一"可能做不出来"的地方。GPUI 没有 webview 组件，要自己用 `windows` crate 建 `WebView2` 子窗口并做 GPUI 布局对齐、层级、焦点、缩放。建议先做 2 小时 spike，再决定 v0.5 是否承诺。
2. **隐私 Bot 文件隔离**（4.2）——改动会碰所有工具的 `before_tool_call` 路径，需回归全部工具。但这是安全承诺，不能拖。
3. **IM 抽象层重构**（5.3）——`im_runtime.rs` 2498 行、`im_service.rs` 782 行，抽 trait 会动现有微信/企微两条已跑通通道，回归成本高。建议先写飞书适配器、**不动**微信/企微代码路径，用注册表旁挂。
4. **数据结构改动必须有迁移**（群目标加 acceptance、多档位、浏览器 profile）——按 goal 规则"旧 `~/.mode-bot` 数据不得丢失"，每个字段按"缺失即默认"处理，不允许收紧 `deny_unknown_fields`。
5. **会话显示名的键是「路径字符串」，不是路径身份**（2026-11-26 记录，v0.6 不修，留给下一版）——`titles` 用完整 jsonl 路径当 key（`src/state.rs:143` 的 `title()`、`src/state.rs:148` 的 `set_title()`），`sessions` / `history` 存的也是同一份路径串（`src/backend.rs:1213` 的 `session_id: path`），而这份路径的**拼写**取决于 `data_root()`（`src/data.rs:2217`）当时从哪里拿到根：

   - `BOT_PILOT_DATA_ROOT` 给什么就用什么；
   - 否则 `MODE_DATA_BASE_DIR`；
   - 否则 `home_dir()` 拼 `.mode-bot/.mode`（`home_dir()` 取 `USERPROFILE`，再退 `HOME`）。

   本机实测两种拼写**同时存在**：`USERPROFILE` 是长名 `C:\Users\Administrator`，`TEMP` 是 8.3 短名 `C:\Users\ADMINI~1\...`。验收时隔离根建在 `TEMP` 下，于是 `titles` 的 key 带 `ADMINI~1`；生产默认走 `USERPROFILE`，key 带长名。

   两种拼写指向同一个文件（`Test-Path` 都为真、文件长度一致），但**字符串不相等**（`-ceq` 为 `False`）。所以坏的不是「文件找不到」——`std::fs` 由系统解析别名，会话照常能读；坏的是所有**按字符串比对**的查找：`title()` 命中不到、`src/main.rs:24076` 的 `filter(|id| live != id)` 判不等。

   表现是**改名静默失效**：弹窗能开、能保存、写盘成功、重启后名字没了，而 `set_title` 不报错。触发条件不是「将来上游换写法」，而是**同一台机器上两种来源混用**：在短名隔离根里改过名，再用默认根启动，名字就不见了。

   判据：**键必须有归一化，不能靠上游路径字符串恰好一致**。两种修法，v0.6 都不做：

   - 存储层归一化：写入/查找前统一规范化（展开短名、统一分隔符、大小写口径），并在 `StateFile` 里加 `version` 迁移把老 key 重写一次；
   - 或改用**真正的身份**当 key（会话文件 stem 里的 session uuid），路径只留作「怎么打开它」的值。后者更好：uuid 不随目录拼写变化，也天然消除「同一会话两个别名各存一条 title」。

   连带一条同源的小问题：`titles` **从不清理**——全仓只有 `set_title` 里那一处 `remove`（`src/state.rs:151`），会话文件删了、Bot 删了都不回收。量很小（每条一个短串），但根因和上面一样：**把路径当身份用，就没有可靠的生命周期**。

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
| 多语言 | 只有中/英两档：`src/i18n.rs:12`（`set_lang` 只认 `"en"`，其余中文），下拉只有两项 `src/main.rs:12037`（`fn language_row`） | 7 种：`ds-bot/src/client/i18n.js:1695`（zh-TW/ja/ko/es/fr/de + 默认中英），另有 6 份 locale 各 2270～2278 行 | 缺失 | 补实现（体力活） |
| 语音朗读 | 0 命中（`tts`/`speech`/`voice` 在 `src` 全为 0） | `ds-bot/src/client/voice.js:81`（`VoiceMode`）、`ds-bot/src/client/speech.js`(152)、`ds-bot/src/client/read-page.js:22`（`readPage`） | 缺失 | 框架性（接系统 TTS） |
| 手机端 / 远程配对 | 0 命中（`pairing`/`PairCode`/`mobile` 全为 0） | `link/src/pairing.ts:55`（`pairingUri`）、`ds-bot/src/host/phone.js:18`、`ds-bot/src/host/mobile.js:160`、`mobile/`（React Native 整套） | 缺失 | 框架性（独立子系统，v0.6） |
| 学习卡（技能自学习） | 0 命中（`learning` 全为 0） | `ds-bot/src/client/learning-card.js:164`（`LearningTurnTail`）、`ds-bot/src/host/learning.js:296`（install） | 缺失 | 框架性 |
| 诊断 / 反馈页 | 只有"复制诊断"文本：`src/main.rs:14390`（`settings_page_feedback`，内容是一段静态格式化串 `:14392`）、`src/i18n.rs:587` | `ds-bot/src/client/diagnostics.js:42`（`clientFacts`）、`ds-bot/src/client/feedback.js:46`（`FeedbackPage`）、`:126`（`ErrorToast`） | 半成品 | 补实现 |
| 主题 / 强调色 | 已有：`src/theme.rs`(595)、`src/wechat_theme.rs`(236)、`src/main.rs:12053`（`fn accent_row`） | `client/themes.js`(124) | 已有 | — |
| 托盘 / 自启 / 自动更新 | 已有：`src/tray.rs:265`、`src/autostart.rs:34`、`src/update.rs:69` | `app/src/tray.mjs`、`app/src/updates.mjs`(215) | 已有 | — |
| 头像 / Bot 图标 | 已有：`src/avatar.rs`(262)、`bot_icon.rs`(120)、`agent_icons.rs`(70)、`brand_icons.rs`(307) | `client/avatar-library.js`(149)、`agent-logos.js`(52) | 已有 | — |
| 会话全文搜索 | 已有：`src/session_db.rs`(381)、`src/main.rs:1030`（`FindState`）+ `:2859`（`find_dialog`）、隐私过滤 `:6530`、`src/rpi_engine.rs:1536`（`search_sessions`） | `ds-bot/src/host/chat-index.js`(276) | 已有 | — |
| 旁路会话（side chat） | 已有：`src/main.rs:7414`（`PaletteCmd::SideChat` 定义 → `:7570` 派发 / `:7805` `open_side`） | `ds-bot/src/client/side-chat.js`(155)、`host/side.js`(377) | 已有 | — |
| 队列可视化 | 有上限 5 条但无独立 UI：`src/main.rs:4733` | `ds-bot/src/client/queue-dock.js:99`（`QueueDock`） | 半成品 | 补实现 |

---

## 16. 附：可直接抄进 parity-matrix 的 63 条（证据已核）

| # | 功能域 | 条目 | 判定 | 证据（MC） | 证据（DS） |
|---|---|---|---|---|---|
| 1 | Bot | 新建/编辑/删除 Bot | 已有 | `src/data.rs:117` / `:649` / `:598`；`src/main.rs:3737` | `ds-bot/src/host/tools.js:325/363` |
| 2 | Bot | 头像与图标库 | 已有 | `src/avatar.rs`(262)、`src/bot_icon.rs`(120) | `ds-bot/src/client/avatar-library.js`(149) |
| 3 | Bot | 每 Bot 模型选择 | 已有 | `src/main.rs:10159`（models 页）、`src/rpi_engine.rs:55` | `ds-bot/src/host/models.js:101` |
| 4 | Bot | 工作目录切换 | 已有 | `src/data.rs:553` | `ds-bot/src/host/tools.js:363` |
| 5 | Bot | 权限/审批三档 + 白名单 | 已有 | `src/rpi_engine.rs:267`（approval_policy）、`:422`（hook） | `ds-bot/src/host/approvals.js`(110) |
| 6 | Bot | 记忆面板（查看/编辑/撤销） | 半成品（仅团队/群） | `src/main.rs:14657`、`:14432` | `ds-bot/src/client/memory-panel.js:325` |
| 7 | Bot | 记忆写入工具 remember/forget/recall | 缺失 | 0 命中 | `ds-bot/src/host/memory.js:1075/1114/1151` |
| 8 | Bot | 插话（steer） | 已有 | `src/backend.rs:697`（interject）、`src/rpi_engine.rs:1244` | `ds-bot/src/host/steer.js`(255) |
| 9 | Bot | 旁路会话 /side | 已有 | `src/main.rs:7798`（open_side） | `ds-bot/src/host/side.js`(377) |
| 10 | Bot | 会话全文搜索 | 已有 | `src/main.rs:1030`、`src/rpi_engine.rs:1536` | `ds-bot/src/host/chat-index.js`(276) |
| 11 | Main bot | Chief 身份与降级规则 | 已有 | `src/data.rs:88`、`:590` | `ds-bot/src/host/roster.js`(900) |
| 12 | Main bot | 管理其它 Bot/群（工具化） | 缺失 | 0 命中 | `ds-bot/src/host/tools.js:325/363/496/526/590` |
| 13 | 群聊 | 建群/改群/删群 | 已有 | `src/data.rs:1816`（create_group）、`src/main.rs:3717` | `ds-bot/src/host/groups.js`(1885) |
| 14 | 群聊 | 群成员路由（@/全员/管理员） | 已有 | `src/main.rs:4520`（route_group_members） | `ds-bot/src/host/groups.js` |
| 15 | 群聊 | 群任务台账 + 派活 | 已有 | `src/team_tools.rs:246`、`src/backend.rs:1503` | `ds-bot/src/host/tools.js` |
| 16 | 群聊 | 群目标（阶段） | 半成品 | `crates/bot-team/src/goal.rs:53/64` | `ds-bot/src/host/goals.js:670` |
| 17 | 群聊 | 目标验收标准 pass/rework | 缺失 | 0 命中 | `ds-bot/src/host/goals.js:760/767` |
| 18 | 群聊 | 群记忆自动回顾 | 已有 | `src/memory_review.rs:83`、`src/backend.rs:2239` | `ds-bot/src/host/memory-review.js:23` |
| 19 | 群聊 | 排队消息 dock | 半成品 | `src/main.rs:4733` | `ds-bot/src/client/queue-dock.js:99` |
| 20 | 隐私 bot | 独立会话落盘 + 搜索排除 | 已有 | `src/backend.rs:369`、`src/main.rs:6530` | `ds-bot/src/host/privacy.js:222` |
| 21 | 隐私 bot | 文件归属守卫 | 缺失 | 0 命中 | `ds-bot/src/host/privacy.js:242` |
| 22 | 隐私 bot | 团队外发审批 | 缺失 | 0 命中 | `ds-bot/src/host/privacy.js:172` |
| 23 | 本地 Agent | ACP 引擎接入（pi/deepseek/hermes/zcode） | 已有 | `src/acp.rs:122/167`、`src/data.rs:366` | `ds-bot/src/host/local-agents/acp.js`(267) |
| 24 | 本地 Agent | Claude Code / Codex 驱动 | 缺失 | 0 命中（只有 acp 一种） | `ds-bot/src/host/local-agents/engines.js:5`、`codex.js`(204)、`claude.js`(232) |
| 25 | 本地 Agent | 引擎自动探测 + 登录态 | 部分 | `src/data.rs:366`（探 PATH） | `ds-bot/src/host/local-agents/discover.js`(153) |
| 26 | 本地 Agent | 给外部 CLI 注入团队工具（MCP） | 缺失 | 0 命中 | `ds-bot/src/host/local-agents/mcp.js`(131) |
| 27 | 浏览器 | browser_* 工具（20 个） | 已有 | `src/browser_tools.rs:382`-`:583` | `ds-bot/src/host/browser/session.js` |
| 28 | 浏览器 | 每 Bot 独立浏览器 profile | 缺失（全局单例） | `src/browser_tools.rs:28`、`:274` | `ds-bot/src/host/browser/session.js:94` |
| 29 | 浏览器 | Cookie 导入 | 缺失 | 0 命中 | `app/src/cookie-import.mjs:41`、`app/src/cookie-parse.mjs:115` |
| 30 | 浏览器 | 内置浏览器窗格（标签/接管） | 缺失 | `src/main.rs:8674`（只有"用系统浏览器打开"） | `ds-bot/src/client/browser-pane.js:523` |
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
| 47 | 用量 | 会话/总量/按天 | 已有 | `src/rpi_engine.rs:1981/1993/1692`、`src/main.rs:13982` | `ds-bot/src/host/usage.js`(630) |
| 48 | 用量 | 按 Bot / 模型分组 | 缺失 | 未见聚合维度 | `ds-bot/src/host/usage.js:182` |
| 49 | 记忆 | Markdown 记忆 + 索引 + 历史 + 撤销 | 已有 | `crates/bots-service/src/memory_store.rs:659/1224/1258` | `ds-bot/src/host/memory.js` |
| 50 | 记忆 | 团队/群双作用域 | 已有 | `src/data.rs:380/396` | `ds-bot/src/host/memory.js:854` |
| 51 | 记忆 | 上下文预算按模型窗口自适应 | 缺失（写死 4000） | `src/data.rs:384/401` | `ds-bot/src/host/memory-entries.js:75` |
| 52 | 定时任务 | cron 引擎（五/六/七段） | 已有 | `crates/automation-schedule/src/cron.rs:49` | `ds-bot/src/host/schedules.js` / croner |
| 53 | 定时任务 | 创建向导（at/daily/weekly/every） | 缺失（只有裸 cron） | `src/main.rs:9548` | `ds-bot/src/host/schedules.js:60` |
| 54 | 定时任务 | Agent 侧创建/查询定时任务工具 | 已有 | `src/automation_tools.rs:71/89/124` | `ds-bot/src/host/schedules.js:211` |
| 55 | ACP | 会话/权限卡/提问卡 | 已有 | `src/acp.rs:387/472/108` | `ds-bot/src/host/local-agents/acp.js` |
| 56 | ACP | 断线重连 / 会话恢复 | 已有 | `src/acp.rs:167`（resume 走 session/load） | `ds-bot/src/host/resume.js`(451) |
| 57 | 通用 | 供 Agent 使用的 ask_user | 已有 | `src/rpi_engine.rs:2211` | `ds-bot/src/host/tools.js:670` |
| 58 | 通用 | 多语言（>2） | 缺失 | `src/i18n.rs:12`（只 zh/en） | `ds-bot/src/client/i18n.js:1695` |
| 59 | 通用 | 语音朗读 | 缺失 | 0 命中 | `ds-bot/src/client/voice.js:81`、`ds-bot/src/client/read-page.js:22` |
| 60 | 通用 | 手机端 / 远程配对 | 缺失 | 0 命中 | `link/src/pairing.ts:55`、`ds-bot/src/host/mobile.js:160` |
| 61 | 通用 | 诊断 / 反馈 | 半成品 | `src/main.rs:14390` | `ds-bot/src/client/diagnostics.js:42`、`ds-bot/src/client/feedback.js:46` |
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

---

## 18. 锚点门禁的能力边界（已用负向用例实测，别当成"全都能拦"）

这一节写的不是"门禁做了什么"，而是**它拦不住什么**。每条都附了一个实测过的漏检用例。

### 18.1 `remap-anchors-from-diff.ps1` 的 token 自检只守"脚本自己不吃 token"

自检比较的是**同一次运行内**改写前后的 token 总数：

```
$beforeTotal = Measure-MainTokens          # 运行前
... 改写 ...
$afterTotal  = Measure-MainTokens          # 运行后
$expected    = $beforeTotal - $deleted + $added
SELF-CHECK OK ⇔ $afterTotal -eq $expected
```

负向用例：先手工删掉一个锚点 token 再跑脚本 → 输出 `SELF-CHECK OK`。因为 before / after 一起变小，等式仍然成立。

**结论：它证明的是"这次改写没有净丢 token"，不是"文档里的锚点一个不少"。** 后者只能靠指纹锁的 `unlocked` / `stale-in-lock`，以及人工核 `anchors checked` 的基数。

实测（`727a626 → f79768a` 干跑）：

```
hunks 9   删 6   增 23
tokens remapped 220
tokens before=343 deleted=0 expected=343 actual=343
SELF-CHECK OK
```

同一脚本修 bug 前的错误输出留档：真实 `+23/−6`，旧版本报"删 0 增 8"——hunk 头省略 `,N` 时 `$Matches[2]` 是 `$null`，而 `$null -ne ''` 为 True，于是 `[int]$null = 0`，整条替换型 hunk 被跳过。别用 `$Matches[2].Success` 判（`$Matches` 是 hashtable，取出元素是 string，`.Success` 恒为 `$null`）。

### 18.2 裸行号 `:N` 的归属是"同行最近的前置完整锚点"，不是语义归属

回填规则只看位置：同一行里、在 `:N` 之前**最近**的那个完整锚点。所以一行里出现多个文件时，裸行号会被归给紧邻的那一个，哪怕它语义上属于更早提到的文件。

真实的漏检（`docs/prd-0.6.md` 的 O-3 那一行）：句子先写"`wechat_working_row`，`src/main.rs:25007` 起"，中间插了一个 `tool_render.rs` 的 `:26` 引用，后面的裸 `:25011` / `:25017` 就被归给 `tool_render.rs`：

```
src/tool_render.rs :25011 (裸行号) -> OUT OF RANGE (max 434)
src/tool_render.rs :25017 (裸行号) -> OUT OF RANGE (max 434)
```

**这次是运气好——越界才报错。** 如果 `tool_render.rs` 有 3 万行，它会拿到 `tool_render.rs` 里某个无关代码行，判据照样绿。

修法是**在文档里补 `src/main.rs:` 前缀**（`5959f99`），不是让脚本去猜语义：语义归属无法自动推断，猜错会把别人的锚点指到错文件上，那是比漏检更坏的错。

现在门禁会把这个数摆出来（只计数、不判失败），不再靠人去发现：

```
bare-line anchors checked: 124  skipped(no file earlier on the line): 38  ambiguous-file: 60
```

当前 60 条里有 5 条**靠行数也分不出来**（两个文件的行数都够得着）：`parity-matrix.md` 的 `:20`、`:602`、`:1030`、`:345`、`:1914`。这 5 条只能人工核语义；其余 55 条是「越界才被迫唯一」——文件一变长就会失去这个运气。

### 18.3 `drifted` 只比"这一行的内容"，重复行会假一致

指纹锁存的是一行去空白后的内容摘要。同一段文本在文件里出现多次时，位移后内容比对**必然通过**，但语义已经指错了地方。

实例：`.github/workflows/release.yml` 里 `if ($LASTEXITCODE -ne 0) { exit 1 }` 出现三次（ui-lint 步、parity 步、arch 步），另有 bash 的 `if [ -z "$latest" ]; then` 只出现一次。前者位移后 `drifted` 报不出来，后者能精确报出来。

2026-11-26 那一轮就是靠"唯一那条能报的"定位到 `cfd5805` 给 workflow 插了 6 行，再人工逐条核出正确位置是 `54 / 85 / 127`。**所以 `drifted: 0` 不等于锚点都对，只等于"没有一条位移后的内容变了"。**

### 18.4 `reanchor-from-lock` 这类"自动重指"会把区间压成单 token

我自己写了一半就删了：apply 阶段把 `a.rs` 的 `:100-120`、`:100/110/120` 这类区间/多位置写法压成一个 token，`anchors checked` 从 248 掉到 200——**悄悄少扫 48 条，而 `RESULT` 仍然全绿**。

判据：重建锚点前后必须比对 `anchors checked` 的基数，掉数就是少扫，不是"干净了"。

**补充：区间写法的锁代价。** 前半段是「自动重指会少扫」，这里方向相反——**写单点和写区间，对锁的影响不一样。**

| 写法 | 落进锁的键 | 对锁的增量 |
| --- | --- | --- |
| `src/main.rs:205` | `src/main.rs:205` | +1 |
| `src/main.rs:205-221` | `src/main.rs:205`、`src/main.rs:221` | **+2** |


**区间锚点在锁里是两条记录，重建锁时要一起处理。** 只补起点不补终点，终点会一直留在 `unlocked` 里——而 `unlocked` 是判失败的那一项，不是静默跳过。

实测：产品在 PRD 里补了一句 `src/main.rs:205-221` 解释窗口宽度随右栏开合变化，`parity -Baseline WorkTree` 立刻报 `unlocked=2`；跑一次 `-UpdateAnchors` 后 `fingerprint` 从 380 变成 382。他写的只是「一个区间」，锁里却多出两条记录。

**所以「重建锁」和「补一条锚点」不是一比一的关系。** 写区间的人以为加了一条，实际加了两条；反过来，代人重建锁的人如果只补其中一个端点，另一个端点会一直留在 `unlocked` 里——而 `unlocked` 是判失败的那一项，不是静默跳过。

推论：**区间写法比单点贵，但并不比单点保护得更好**（理由见 18.10：终点的指纹往往没有区分力）。确实需要指整块代码时，写「起点行 + 块内一个符号名」比写「起点-终点」划算。

### 18.5 门禁扫哪些文档是写死的

`verify-parity-anchors.ps1` 的 `$docs` 只有三个文件：

```
docs/parity-matrix.md
docs/prd-0.5.md
docs/prd-0.6.md
```

所以：

- `docs/plans/**`、`docs/testing/**` 里的锚点**完全不校验**——按今天 HEAD 去改历史快照等于篡改，这是有意排除，不是遗漏；
- 本文档（`docs/parity-arch-notes.md`）只被 `verify-arch-anchors.ps1` 扫，那是**弱校验**：文件存在 + 行号不越界 + 该行非空 + 符号名在 ±20 行内出现。**没有指纹锁**，所以本文件里的行号漂了不会被 `drifted` 抓到，只能靠每次跑 `verify-arch-anchors.ps1` 时的 `bad=0`。

### 18.6 判据必须有「区分力」：一个在两种状态下读数相同的量，不是判据

这一轮所有假证据是同一个形状：**用一个在目标状态和非目标状态下都会成立的量，去证明目标状态成立。**

四个实测案例，每个都通过了当时所有的硬指标：

| 案例 | 用的量 | 为什么没有区分力 |
| --- | --- | --- |
| `264-4b-3`（折叠≠展开） | 绿边像素 = 0 | 折叠时是 0，展开后清除完成也是 0 |
| 同上 | 文件哈希不同 | 两张废图哈希也不同 |
| 同上 | 时间晚于 exe | 废图同样晚于 |
| 产品那次 `496/69/69` | 右栏某区域内容量 | 右栏折起后聊天区撑满，量到的是聊天区 |
| 我自己那次「自检 OK」 | token 总数守恒 | before/after 一起变小仍然守恒 |

**能区分两种状态的可测量值才是判据，其余只是伴随现象。** 落地成两条可执行要求：

1. **每条判据写下来时，必须能回答「在非目标状态下这个量是多少」**。答不上来（或答出来和目标状态一样），这条判据就是废的。`264-4b-3` 的正解是同时要求两条：右栏底色列数 > 40（折叠时应为 0）**且** 精确 `0x07C160` 像素 = 0（亮时应 > 1000）。任一条单独都不行。
2. **量值要选「精确值」而不是「阈值内的近似值」**。同一个「绿边」判据，产品量出 21、Chief 换个阈值量出 50——因为就绪 badge 的底色是浅绿 tint `rgb(217,255,217)`，会被宽松阈值算进去。**只用精确 `0x07C160`（卡片描边色）计数，「= 0」这个结论才不随阈值漂移。**

推论一条，适用于所有门禁而不只是截图：**判住一个量的「存在」很难，判住两个互斥状态下的「差异」才可靠。** 所以锚点门禁里真正有用的不是 `anchors checked > 0`，而是「同一份文档在 `-Baseline Head` 和 `-Baseline WorkTree` 两个口径下必须一致」——那是两个状态之间的差异，不是单点读数。

### 18.7 一条回路验证：`263-3` 证明的比它声称的多一个前提

`263-3` 那张图（「最近会话」那行从短 id 变成 `发布前走查-263`）常被当成「改名落盘生效」的完整证据。它确实证明了这条链，但**它之所以能成立，依赖一个没有写在任何判据里的前提**。把回路走完，这个前提就露出来了。

**回路（每一步都是代码事实，不是推断）：**

```
labels      src/main.rs:24088-24092  label = title(&session_id).unwrap_or(display_session_id)
rename_id   src/main.rs:24095        rename_id = session_id.clone()      ← 与 label 同一个变量
history     src/main.rs:24080        来自 self.sessions.history_of(&bot.id)
push_history src/main.rs:6846/7342/7550/24286  old_id = chat.session_id.clone()
write       src/main.rs:9114         set_title(&session_id, &title)      ← 写的就是 rename_id
```

所以：**`titles` 的 key 只能由 `history` 里的串写入，而 `history` 里的串只来自 `chat.session_id`。** 两条路径共用同一个来源变量，形态不可能不一致——除非 `chat.session_id` 在「写入那一刻」和「渲染那一刻」之间变过。

这才是真正的边界：**`263-3` 成立是因为行上的 label 和改名按钮的 id 来自同一次 `history_of` 迭代**，不是因为「路径 key 的形态碰巧一致」。前者是结构保证，后者是运气。

**由此得到一个必须写下来的限制**：改名入口只挂在「最近会话」那几行上（`src/main.rs:24132` 的 `chat-info-rename-{rename_row}` 是唯一入口），而当前会话被「最近会话」那段的 `filter(|id| live != id)` 排除在外。**结论：当前正在用的会话没法改名，必须先切走，它才会出现在最近会话里。**

这不是缺陷报告，是设计边界——但它影响验收：`263-2` / `263-3` 拍的是「切走之后再改名」，不是「给当前会话改名」。任何按「打开一个会话就地改名」写的用例都会失败，而失败原因不在改名功能。

**另有一处口径**：`chat.session_id` 对 ACP 引擎（`acp.rs` 的 `AcpEvent::Ready`）是协议给的 opaque id，对 pi-rust 引擎（`src/backend.rs:1213` 的 `session_id: path`）是文件全路径。两套来源都能进 `history`，所以 `titles` 的 key 实际上混着两种形态——但它们各自自洽，因为每条记录的写入和读取都走同一条来源。**跨引擎切换同一个 Bot 时才需要额外小心**，这一点记在风险清单第 5 条里。

### 18.8 `4b` 的视觉证据可以用一条穷举证明替代：`chat_detail` 全仓只有 8 处

`4b` 那组截图卡了四轮：`264-4b-3` 没拍到展开、`266-4b-1` 两次存成了「未亮」。反复失败之后我先停手，去做了一件事——**把 `chat_detail` 在全仓的所有出现位置穷举出来**。结果是 8 处，一个不多：

```
src/main.rs:3279   chat_detail: Option<SettingsDetail>      字段声明
src/main.rs:3775   chat_detail: None                        初始化
src/main.rs:3884   self.chat_detail = None;                 select_bot 清
src/main.rs:4145   self.chat_detail = None;                 select_group 清
src/main.rs:11255  self.chat_detail = None;                 set_right_panel(false) 清
src/main.rs:16815  self.chat_detail = Some(..RunStatus..)   ← 唯一写入 Some 的地方
src/main.rs:23720  self.chat_detail.as_ref()                唯一读取（决定是否画绿边）
src/main.rs:24149  this.chat_detail = None;                 点「改名」跳设置页前清
```

**这条穷举能证明的事，比一张截图强：**

1. **绿边出现的充要条件是 `chat_detail == Some(RunStatus { key })` 且 `key` 等于当前会话键**（`src/main.rs:23719`-`:23722` 的 `matches!` 带 `if` 守卫）。
2. **整个程序里只有 `src/main.rs:16815` 能把它设成 `Some`**，而那里只在点「⋯ → 运行状态」时执行。
3. **三个清空点之外没有第四个写入者**，也没有从 `pilot-state.json` / `ui-prefs` 反序列化回来的路径——`chat_detail` 只挂在 `PilotApp` 上（`:3279`），`save_ui_prefs` 存的是 `muted / topmost / pinned_settings / right_panel_open` 四项，不含它（`src/main.rs:712`-`:718`）。

所以「关右栏再展开，绿边不回来」这件事**不需要靠截图证明**：从 `11255` 清掉之后，能再把它设回去的代码只有 `16815`，而重新展开走的 `11251`-`:11256` 那条路径里没有它。

**那截图还证明什么？** 证明「清空之后渲染出来的样子确实没有绿边」——这属于**渲染层**的事实，穷举证明不了。而这一层已经被 `265-4c-3` 覆盖了：同一处 `chat_detail = None`（`src/main.rs:3884` 的 `select_bot` 分支），清完之后 `exactGreen = 0`、右栏底色存在。**`11255` 和 `3884` 是同一行语句、同一个效果，只是入口不同。**

**结论与适用边界：**

- 可以说：`set_right_panel(false)` 的清空逻辑**在代码层确定成立**，且「清空后的渲染效果」已由 4c 的同型帧覆盖；
- 不能说：`4b` 那张「折叠 → 展开」的因果帧存在。**它确实不存在，记缺失，不要用别的帧顶替。**
- 这条穷举也顺手排除了一个隐患：`chat_detail` 不进磁盘，所以**重启后不会带着一个来源不明的描边**——这正是 `11253` 那条注释想保证的事，现在有证据了。

**教训**：「视觉证据缺失」不等于「事实不明」。当一条 UI 事实可以靠「所有写入点穷举 + 只有一处能写入」证明时，穷举比截图更强，因为它覆盖所有路径而不只是拍到的那个。**但穷举不能替代「渲染出来长什么样」——那是另一回事，必须分开说。**

### 18.9 文档里的「时间」会被当成锚点：写时刻请用「12点16分32秒」

这一条来自一次真实失败：产品在 PRD 里写「`12:16:32` 与 `10:24:26`」，`parity` 直接报 `MALFORMED ANCHOR`。原因不是他写错了锚点，是他写了一段时间。

**机制（实测复现过，不是推断）：** 门禁里判断「`:` 后面跟数字」的正则是 `:(\d+(?:/\d+)*)`，它没有、也无法区分「行号」和「时分秒」。真正的触发条件比想象中窄，我实测了三种写法：

```
① 同一行里有合法文件锚点 + 时间裸写   `src/main.rs:11255` … 12:16:32   → 报 malformed: :16, :32
② 时间被反引号包住                  `src/main.rs:11255` … `12:16:32`   → 不报，静默通过
③ 整行没有任何合法文件锚点            时间 12:16:32 与 10:24:26          → 整行跳过，不报
```

也就是说：**只有「这一行已经有一个被反引号包好的合法文件锚点」时，行内其它裸 `:N` 才会被逐条检查**（`src/main.rs` 那一族脚本的 `MALFORMED ANCHOR` 分支就写在这个前提里），检查手段是数这个 `:` 前面出现了几个反引号——**偶数 = 落在反引号对之外 = 判定语法坏了**。

**所以规矩有两层，第二层比第一层重要：**

1. **写时刻不要写成 `12:16:32`**。写成「12点16分32秒」，或者写成 `2026-11-26 12:16:32`（日期在前，`:` 前是空格不是数字段）。这条是给写文档的人的。
2. **反引号包住任何东西，都会让它从这道检查里消失**。上面 ② 就是证据：包上反引号之后，`12:16:32` 不再报错——**不是因为它是合法的，是因为检查看不到它了**。这意味着「补一对反引号」可以让自己写的任何 `:N` 静默失效，包括真正该报的坏锚点。**这个「修复」只在它是文字时才是对的；如果它是锚点，包反引号等于把门禁关掉。**

**和 18.2 的关系（方向相反，同一族）：**

- `18.2`：真锚点被**归错文件**（裸行号继承同行最近的前置完整锚点）；
- `18.9`：普通文字被**当成锚点**（时间串被当成行号）。

两条都源于同一件事：**锚点的识别只靠字符形态，不靠语义。** 所以文档里凡是长得像 `文件:数字`、`:数字`、`数字:数字` 的东西，都会被当成锚点处理。

**这一节不动脚本。** 改正则去排除时间串（比如要求 `:` 前必须是行首或反引号）会把 `18.2` 那类裸行号的识别一起改坏，收益不抵风险。**规矩写在文档侧，成本为零。**
### 18.10 区间终点被结构校验豁免，指纹又常常是 `}` —— 等于没有漂移保护

18.4 说区间会被拆成两个端点。这里补另一半：**这两个端点的保护强度不一样。**

`verify-parity-anchors.ps1` 的校验按顺序有这几关，区间终点在第二关就被放行了：

```
if ($start -lt 1 -or $start -gt $lines.Count) { OUT OF RANGE }
if ($start -eq $rangeEnd -and $start -ne $positions[0]) { continue }   ← 终点在这里继续
if ($line -eq '') { EMPTY LINE }
elseif ($line -match '^[\(\)\[\]\{\};,]+$') { STRUCTURAL LINE }
```

**所以「锚点指到一行只有 `}` 的收尾行」这种明确坏锚点，起点会被拦、终点不会。** 而终点照样进锁、照样参与 `drifted` 比对——它的指纹就是那一行 `}`。

`}` 在本仓是高频行：`src/main.rs` 共 26809 行，其中 1883 行整行只有一个 `}`。拿它当指纹，后果和 18.3 的重复行假一致完全一样：**位移之后指纹仍然相同，`drifted` 报不出来。**

实测锁里 435 条，指纹恰为单字符 `}` 的 6 条，全部是区间终点：

```
ds-bot/src/host/schedules.js:86   src/backend.rs:194        src/computer_tools.rs:250
src/data.rs:72                    src/main.rs:221           src/rpi_engine.rs:371
```

文档里的区间终点去重后 33 个，落在锁里的 29 个，其中 6 个（约两成）指纹是 `}`。指纹为纯结构性字符（只有括号/分号/逗号）的共 8 条；另有 3 条指纹是空串，对应 `src/automations.rs` 的 101、224 与 `src/cua_tree.rs` 的 320，那三行现在确实是空行（它们已不在文档里，只以 `stale-in-lock` 形式留在锁里，所以没被 `EMPTY LINE` 判错）。

判据：**区间终点不能计进「已受保护的锚点」里。** 要让一条区间真的受保护，要么把终点改成「起始行 + 块内符号名」，要么另立一条指向块内有内容的一行（例如 `fn` 那一行本身）。

### 18.11 门禁能证明脚本「存在且被跟踪」，证明不了它「能跑」

`scripts/shot-probe.ps1` 在 `7731a3d` 进入仓库。入库时它是 **UTF-8 无 BOM**，里面有 686 个非 ASCII 字符，全是中文注释和判读提示。Windows PowerShell 5.1 在缺 BOM 时按系统 ANSI 代码页（本机 936 / GBK）解码，中文被拆成乱码字节，其中一部分吃掉换行，**整个脚本语法错误**，连 `param()` 都过不去。

实测同一份内容，两种编码：

```
无 BOM  powershell -NoProfile -ExecutionPolicy Bypass -File <副本>.ps1 <图>  → EXIT=1
        解析器报 Unexpected token ')'，同时把「4c-1 亮」显示成「4c-1 浜?」
有 BOM  同一条命令（无管道直调）                                          → EXIT=0
        正常输出 panel=present (left=953) / exactGreen=2248 / leftBand=y=84..139
```

两者去掉 BOM 与行尾差异后**逐字符相同**（137 行，`-ceq` 为 True），字节数差 3。

**这不是「又一个人为失误」，是门禁的盲区：**

- `anchor-gate.yml` 只跑三个锚点脚本，`shot-probe.ps1` 不在其中；就算在，CI 的 PowerShell 代码页也不保证与本机 5.1 一致；
- 「被跟踪、能 diff、有哈希」这三件事都不会因为它跑不起来而失败。**仓库里可以有一个大家都以为是工具、实际一跑就崩的脚本，而全部门禁全绿。**

判据：**带非 ASCII 的 `.ps1` 必须存 UTF-8 BOM，并且要有一条「无管道直调」的实跑记录**——`EXIT=0` 且输出里出现数字。`git show` 里能看到这个文件，什么都证明不了。

顺带一条读退出码的规矩：脚本输出接进管道后，`$LASTEXITCODE` 取到的可能是管道末端命令的。同一份脚本，直调拿到 0、接管道拿到 1，两个数都「真」，但只有一个说明脚本自己。**判退出码要在无管道直调之后立刻取。**

### 18.12 同一份脚本，三种读法给出三个退出码——只有一种说明脚本自己

18.11 说带非 ASCII 的 `.ps1` 必须存 UTF-8 BOM。这里补一个更麻烦的事实：**即使脚本是对的，退出码也会因为「怎么读」而变成不同的数。**

同一个 `scripts/shot-probe.ps1`（工作区版，UTF-8 有 BOM，语法自检 0 错误），同一条命令，三种读法：

```
直接跑、把输出接管道后取 $LASTEXITCODE
    powershell ... -File scripts\shot-probe.ps1 <图> 2>$null | Select-Object -First 2
    → 输出正确（panel=present left=953 exactGreen=2248），但 EXIT=-1

先跑一个子 pwsh，再由它 exit $LASTEXITCODE
    powershell -Command "& <脚本> <图>; exit $LASTEXITCODE"
    → EXIT=1

Start-Process -Wait -PassThru，stdout/stderr 各自重定向到文件
    → EXIT=0，stderr 为空
```

**三个数里只有 `0` 是脚本自己的。** `-1` 和 `1` 都不是：前者是 piped `$LASTEXITCODE` 的取值行为，后者是外层 `-Command` 的包装把自己失败也算了进去。

**这就是「判据里的常量不能随环境变」在退出码上的同一个病**：`EXIT=0` 这个结论本身没错，但「怎么取到它」有讲究，而这一点在最常用的写法（接管道、看最后一行）里恰好不成立。

可执行的规矩只有一条：**要判退出码，就用 `Start-Process -PassThru -Wait`，输出重定向到文件再读；不要接管道、不要套 `-Command`。** 顺带一条：`EXIT=-196608` 这类大负数表示「PowerShell 自己起不来」（本机实测是 `-File` 指向的路径不存在），不是脚本返回的退出码，看见就先去查路径。

**和 18.6 的关系**：`EXIT=0` 是一个单点读数。它只在「脚本被正确启动、输出没被管道截断」这个前提下有意义。换个读法，同一个脚本会报失败——**不是因为脚本坏了，是因为读的人换了秤。**
