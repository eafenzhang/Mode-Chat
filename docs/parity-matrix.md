# ModeChat v0.5.0 对标盘点矩阵（DS Bot v0.3.6 → ModeChat v0.4.0）

> 基准：`D:\Mode Bot\DS-Bot`（v0.3.6，`README.zh.md` + `ds-bot/README.md` + `ds-bot/src/host|client|office`）与本机 DS Bot 实机界面。
> 被盘点对象：`D:\ModeChat`（v0.4.0，`Cargo.toml:25`，`src/main.rs` 27,771 行）。
> 证据格式：`源文件:行号`；DS-Bot 侧证据同样给出行号，便于复核。
> 状态图例：✅ 已有（可用且对得上）｜🟡 半成品（有代码/有界面，但不通、不完整或语义不同）｜❌ 缺失。
>
> **本文档状态已按 v0.5.0 交付结果更新（2026-11-25）。** 本轮实现项在「证据」列给出新行号；未实现项在「差距」列写明**明确降级说明**（降级到 v0.6 或标为不在 v0.5 范围），与阶段验收口径「全部实现或明确降级说明」一致。

统计口径：共 **136 项**，✅ 91 项、🟡 18 项、❌ 27 项（见文末汇总）。v0.4.0 基数为 ✅ 73 / 🟡 22 / ❌ 41，本轮净增 ✅ 18 项。

---

## A. Bot 身份与生命周期（13 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| A01 | 新建 Bot（名字/角色/工作目录） | `README.zh.md:58` | ✅ | `src/main.rs:2488`（BotFormState）、`src/data.rs:114`（create_bot） | 无 |
| A02 | 编辑 Bot | `ds-bot/src/host/tools.js:363`（update_bot） | ✅ | `src/main.rs:10480`（save_bot_form）、`src/data.rs:554` | 只会改 7 个字段，改不了模型/指令 |
| A03 | 删除 Bot（二次确认） | `ds-bot/src/host/tools.js:298`（stop_bot/删除走 UI） | ✅ | `src/data.rs:605`、`src/main.rs:15507` | 无 |
| A04 | Bot 形象：20 字符 + 12 色 + 自选照片/头像库 | `ds-bot/src/client/characters.js`、`avatar-library.js` | 🟡 | `src/main.rs:2522`（9 图标）、`src/main.rs:2552`（5 色）、`src/bot_icon.rs:1` | 只有 9 组图标 + 5 色，无照片上传、无头像库 〔降级 v0.6〕 |
| A05 | Bot 指令（instructions）可编辑 | `ds-bot/src/host/roster.js`（instructions 字段） | 🟡 | 字段存在：`crates/bot-team/src/teammate.rs:36`；但 `src/data.rs:160` 新建固定写空串，UI 无编辑入口 | 能力有、入口无——用户无法写指令 〔降级 v0.6〕 |
| A06 | 换工作目录后会话在新目录继续 | `ds-bot/README.md`（Working folders） | 🟡 | `src/data.rs:554` 只改名册；`src/main.rs:19148`（`restore_draft`）仅在新会话时读目录 | 改目录不重建会话，旧会话仍跑旧目录 〔降级 v0.6〕 |
| A07 | Bot 详情页（模型/审批/会话/待办/目录） | `ds-bot/src/client/details-drawer.js` | ✅ | `src/main.rs:19585`（bot_details） | 无记忆行（见 A08） |
| A08 | 详情页「记忆」行 + 管理入口 | `ds-bot/src/client/memory-panel.js`、`details-drawer.js` | ✅ | `src/main.rs:21128`（`bot-details-memory-row` 显示条数）、`src/main.rs:21143` 打开 `SettingsDetail::BotMemory`（`:23213`）、面板体 `src/main.rs:9749` | 无 |
| A09 | 侧栏分组（section）归组/折叠/染色 | `ds-bot/src/client/folder-menu.js`、`host/folders.js` | ✅ | `src/main.rs:16098`（CRUD）、`src/main.rs:16132`（染色）、`src/main.rs:12499` | 无 |
| A10 | 置顶 / 隐藏 / 恢复显示 | `ds-bot/src/client/sidebar.js`（pushpin） | ✅ | `src/data.rs:41`（pinned）、`:79`（hidden_bots）、`src/main.rs:12761` | 无 |
| A11 | Bot 实时状态（正在做什么） | `ds-bot/src/client/member-status.js`、`host/feed.js` | 🟡 | `src/backend.rs:441`（member 状态事件）、`src/main.rs:2440`（push_member_delta） | 只有“正在输出”，没有“正在做什么/在哪一步” 〔降级 v0.6〕 |
| A12 | 主卡位（Main bot 卡片） | `README.zh.md:68` | ✅ | `src/main.rs:17676`（main_bot_card）、`src/data.rs:37` | 与 Chief 解耦，语义已对 |
| A13 | Bot 详情页用量/待办 | `ds-bot/src/client/usage-page.js` | ✅ | `src/main.rs:19885`（待办）、`src/main.rs:14841`（用量） | 无 |

## B. Main bot 与权限（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| B01 | Main bot 全局唯一 | `README.zh.md:68` | ✅ | `src/data.rs:96`（demote_other_chiefs）、`:175`、`:590` | 无 |
| B02 | Main bot 通过对话管理其他 Bot | `ds-bot/src/host/tools.js:325/363`（create_bot/update_bot） | ❌ | `src/*.rs` 中 grep `"create_bot"`/`"update_bot"` = 0 命中 | 只能手工点界面，Bot 无法自己建/改 Bot 〔降级 v0.6〕 |
| B03 | 权限预设（只读 / 工作区 / 完全访问） | `ds-bot/README.md`（Permission presets） | 🟡 | `src/data.rs:46`（approval_level）、`src/main.rs:19665`（ask/auto/full 三档） | 三档是“工具审批”语义，不是文件系统权限预设 〔降级 v0.6〕 |
| B04 | 审批白名单可视化维护 | `ds-bot/src/client/approval-notices.js` | ✅ | `src/main.rs:19712`、`src/main.rs:21857`（SettingsDetail::Allowlist） | 无 |
| B05 | Bot 互相可见 / 互发消息 | `ds-bot/src/host/tools.js:80`（list_bots）、`:167`（message_bot） | ✅ | `src/bot_tools.rs:58`（`tools` 注册 whoami/list_bots/message_bot/post_to_group）、`src/backend.rs:557`（`Command::AskBot`，120s 超时、目标在跑即拒、隐私直拒） | 无 |

## C. 群聊协作（15 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| C01 | 群聊增删改（成员/目录/策略） | `README.zh.md:74` | ✅ | `src/data.rs:1719/1738/1771`、`src/main.rs:12795` | 无 |
| C02 | 群聊共享工作目录生效于所有成员 | `ds-bot/README.md`（group folder） | ✅ | `src/data.rs:62`；建成员会话时群目录优先：`src/main.rs:3865-3871` | 无（改群目录后已建会话需重进，属可接受） |
| C03 | 管理员先答 / 全员轮流（responder_policy） | `ds-bot/README.md`（admin-first / all-members） | ✅ | `src/backend.rs:1417-1428` | 无 |
| C04 | @ 点名（含 @所有人、边界规则） | `ds-bot/src/shared/mentions.js` | ✅ | `src/backend.rs:1409`（bot_team::mentions::parse_mentions）、`crates/bot-team/src/mentions.rs:110` | 无 |
| C05 | 成员无事可说时静默（`[PASS]`） | `ds-bot/src/host/groups.js`（[PASS]） | 🟡 | `crates/bot-team/src/mentions.rs:110`（is_pass_reply）、`crates/bot-team/src/orchestration.rs:85` | 判定函数存在，群回合是否吞掉回复未接（需实测确认） 〔降级 v0.5 保持现状〕 |
| C06 | 每成员每群一个独立隐藏会话 | `ds-bot/README.md`（hidden Session per group） | ✅ | `src/backend.rs:1286-1370`、`:1304`（member_session_key） | 无 |
| C07 | 群目标 Goal（对齐/阶段/验收） | `ds-bot/src/host/goals.js:670`（group_goal） | ✅ | `src/group_goal.rs:50/87`、`crates/bot-team/src/goal.rs:270/408`、UI `src/main.rs:1058` | 无 |
| C08 | 阶段“证据”校验后才推进 | `ds-bot/src/host/goals.js:19`（STAGE_ACCEPTANCE_MAX）、`:760/767`（pass/rework） | ✅ | `crates/bot-team/src/goal.rs:136/142/147`（SetAcceptance/PassStage/ReworkStage）、`:208`（migrate_goal）、`src/team_tools.rs:193/208/223`（三个工具）、UI `src/main.rs:1568/1592/1387` | 无 |
| C09 | 派活台账（子任务 + 负责人 + 期望结果） | `ds-bot/src/host/tools.js:167`、`groups.js` | ✅ | `src/group_tasks.rs:210/253`、`src/team_tools.rs:197`（team_assign） | 无 |
| C10 | 任务 Stop / Retry / Reassign / Cancel | `ds-bot/src/host/groups.js`（task actions） | ✅ | `src/main.rs:1609/1668`、`src/group_tasks.rs:347/371/392/446` | 无 |
| C11 | 群公告 notice（共享指令） | `ds-bot/src/client/group-settings.js`（notice） | ❌ | `src/data.rs:58-72`（Group 无 notice 字段） | 只能靠 instructions/memory 顶替（未入 v0.5 范围，见 §5 风险表） 〔降级 v0.6〕 |
| C12 | “+ → 引用某个 Bot 的对话”作为上下文 | `ds-bot/src/client/composer.js`（Reference a chat） | ❌ | 只有消息级引用 `src/main.rs:18821`、`compose_with_quote`（`src/main.rs` 内） | 不能把另一段对话整体带入（未入 v0.5 范围） 〔降级 v0.6〕 |
| C13 | 从群外往群里发消息（post_to_group / group_relay） | `ds-bot/src/host/tools.js:607`、`ds-bot/src/host/groups.js:1439` | ✅ | `src/bot_tools.rs:103`（post_to_group 工具）、`src/backend.rs:565`（`Command::PostGroup`）、`BackendEvent::GroupPost` 走既有 `send_group_in_turn` | `group_relay`（IM 群里的转发）未做，仅 Bot 主动发言 |
| C14 | 群成员状态行（谁在跑、跑什么） | `ds-bot/src/client/member-status.js` | 🟡 | `src/backend.rs:441-442`、`src/main.rs:2458`（finish_member） | 只有成员标签，无状态聚合行 〔降级 v0.6〕 |
| C15 | 群记忆注入到每个成员回合 | `ds-bot/README.md`（group memory snapshot） | ✅ | `src/data.rs:396`、`src/backend.rs:1473` | 无 |

## D. 隐私 bot（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| D01 | 会话独立落盘、不进全局搜索 | `ds-bot/src/host/privacy.js:29-32` | ✅ | `src/rpi_engine.rs:223`（private_bot_id）、`src/backend.rs:335-353`、`:1359`（搜索跳过 `.private`） | 无 |
| D02 | 不能进群 | `README.zh.md:82` | ✅ | `src/main.rs:3795/4905/8015/10487`（成员列表过滤） | 无 |
| D03 | 不能当 Main bot / Chief | `README.zh.md:82` | ✅ | `src/data.rs:88`（check_chief_private）、`:96` | 无 |
| D04 | 删除后内容继续受保护（kept-private 登记） | `ds-bot/src/client/kept-private.js`、`ds-bot/src/host/privacy.js:31-32` | ✅ | `src/privacy.rs:80`（archive_private_bot）、`:51`（archived_private_bots）、`:98`（release_archive）、`src/data.rs:704`（删隐私 Bot 前登记）、UI `src/main.rs:12231-12277`（`private-archive-row-<id>`） | 无 |
| D05 | 出站共享需审批（写外部文件、给其他 Bot 发消息） | `ds-bot/src/host/privacy.js:172`（guardTeam）、`:242`（guardFiles） | ✅ | `src/privacy.rs:201`（precheck：路径归属最长前缀判定）、`src/rpi_engine.rs:437`（审批闸门先跑 precheck，`full` 档也拦） | 只覆盖 `read/write/edit/bash/powershell/mcp__*`；`computer_*`/`browser_*` 不在路径范围（见 §5 风险表 R-07） |

## E. 记忆（13 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| E01 | Markdown 记忆 + `## Index` + 主题文件 | `ds-bot/src/host/memory.js`、`memory-entries.js` | ✅ | `crates/bots-service/src/memory_store.rs:447`（splice_index_line）、`:586`（maintain_index） | 无 |
| E02 | 团队记忆进系统提示 | `README.zh.md`（In the prompt） | ✅ | `src/rpi_engine.rs:325-357`（system_prompt_for / system_prompt_for_bot） | 无 |
| E03 | Bot 自身记忆进系统提示 | 同上 | ✅ | `src/rpi_engine.rs:333`（`system_prompt_for_bot`，预算 `clamp(window*0.06, 2000, 24000)` 字符）、`src/data.rs:554`（`bot_memory_context`）、注入点 `:1806` | 无 |
| E04 | 群记忆进成员提示 | 同上 | ✅ | `src/backend.rs:1473` | 无 |
| E05 | 记忆工具 `remember` / `forget` / `recall` | `ds-bot/src/host/memory.js:1075/1114/1151` | ✅ | `src/memory_tools.rs:58`（`tools(bot_id)` 注册三个工具）、`src/data.rs:563`（read/save/delete/undo_bot_memory） | 无 |
| E06 | 群记忆自动回顾（idle 后一次模型调用） | `ds-bot/src/host/memory-review.js:1-6` | ✅ | `src/memory_review.rs:83/196/318`、`src/backend.rs:1911`（run_memory_review） | 无 |
| E07 | 私聊记忆自动回顾 | 同上 | ❌ | `src/backend.rs:1911-1919` 只处理 `group_id` | **明确降级到 v0.6**：私聊不自动回顾（手动 `remember` 已可用，见 E05） 〔降级 v0.6〕 |
| E08 | 记忆面板：团队 / 群 | `ds-bot/src/client/memory-panel.js` | ✅ | `src/main.rs:13972`、`:15103`（团队）、`:15211`（群） | 无 |
| E09 | 记忆面板：单个 Bot | 同上 | ✅ | `src/main.rs:9749`（`SettingsDetail::BotMemory` 列表/新增/撤销/编辑）、`src/main.rs:9792`（`MemoryScope::Bot`） | 无 |
| E10 | 记忆整理（tidy / 合并重复 / archive） | `ds-bot/src/host/memory-tidy.js` | ✅ | `crates/bots-service/src/memory_store.rs:659`（tidy_memory）、`:640`（maybe_daily_tidy 每日一次，写入路径自动触发 `:1133/1161`） | 自动整理已在跑；缺「手动整理 + 撤销整批」的 UI 入口 |
| E11 | 变更历史 + 撤销 | `ds-bot/src/host/memory.js`（journal.jsonl） | ✅ | `crates/bots-service/src/memory_store.rs:1224/1258`、`src/data.rs:522` | 无 |
| E12 | 条目移动到其他作用域 / 摘要 | `ds-bot/src/host/memory-copy.js` | 🟡 | `crates/bots-service/src/memory_store.rs:805`（move_memory，仅测试调用 `:1631`） | 能力有，UI 无入口 〔降级 v0.6〕 |
| E13 | 记忆安全（拒绝密钥/密码） | `ds-bot/README.md`（Safety） | ✅ | `src/memory_tools.rs:396`（`looks_sensitive`：密码/令牌/密钥/长数字，`:194` 返回中文拒绝语）、`src/memory_review.rs:196`（字段校验） | 无 |

## F. 会话、上下文与输入（16 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| F01 | 历史会话切换（resume） | `ds-bot/src/host/parts.js` | ✅ | `src/main.rs:19787-19824`、`src/rpi_engine.rs:1166` | 无 |
| F02 | 新会话 | 同上 | ✅ | `src/main.rs:19117` | 无 |
| F03 | 回合中插话（steer，不打断） | `ds-bot/src/host/steer.js` | ✅ | `src/composer_input.rs:16`（SendSecondary）、`src/rpi_engine.rs:1029-1046` | 无 |
| F04 | 输入排队（Enter 排队 / 回合结束自动放行） | `ds-bot/src/client/queue-dock.js` | 🟡 | `src/main.rs:4491-4671`（队列逻辑）、`:4666-4676`；上限 5 条 | 逻辑有，但无排队条 UI（DS 有独立 dock），用户看不到排了几条 〔降级 v0.6〕 |
| F05 | 旁路会话 `/side`（不写主 transcript） | `ds-bot/src/host/side.js:1-5` | ✅ | `src/main.rs:7307-7402`、`:7336`（事件不进主 chat） | 无 |
| F06 | 上下文压缩 / checkpoint / handoff（按模型窗口的比例触发） | `ds-bot/src/host/steward.js:20-32` | ✅ | 自动压缩：引擎默认已开（`crates/rpi-harness/src/types.rs:327-330`，pi-rust checkout）；MC 接入：`src/rpi_engine.rs:1104`（`harness.events().watch` 监听，rev `29d8686` 的 `CompactionProgressEvent` 未实现 `HarnessEventOf`）、`:1115`（`SessionEvent::Compaction`）、`:1142`（`RpiSession::compact()` 手动压缩）；UI `src/main.rs:4654`（`chat.compacting`）、`:19273/19556`（`compaction_status_row`，`id="chat-compaction-status"`）、`:7349`（命令面板「立即压缩」） | checkpoint/handoff 语义未做（明确降级 v0.6），当前是「压缩 + 进度可见 + 可手动触发」 |
| F07 | 全库聊天搜索 | `ds-bot/src/host/chat-index.js:4` | ✅ | `src/rpi_engine.rs:1326-1365`、`src/main.rs:6541/6753` | 无 |
| F08 | FTS5 索引（快、可搜历史全部 part） | `ds-bot/src/host/chat-index.js:48` | 🟡 | 线性扫 JSONL（`src/rpi_engine.rs:1494`） | 会话多时慢、无 part 概念 〔降级 v0.6〕 |
| F09 | 长会话窗口化渲染 | `ds-bot/src/client/cells.js` | ✅ | `docs/pilot-notes.md:146`（最近 120 条）、`src/main.rs:19582`（chat_window） | 无 |
| F10 | Markdown（表格/代码块/列表/引用） | `ds-bot/src/client/text.js`、`mark.js` | ✅ | `src/main.rs:23001`（kit_markdown_el） | 无 |
| F11 | 思考过程折叠显示 | `ds-bot/src/client/turn-reasoning.js` | 🟡 | `src/main.rs:748`（MsgKind::Reasoning）、`:3167`（工具折叠组） | 思考段可显示，折叠组只覆盖工具卡 〔降级 v0.6〕 |
| F12 | 阻塞式提问卡（ask_user） | `ds-bot/src/host/tools.js:670` | ✅ | `src/rpi_engine.rs:1683`（ask_user_question）、UI `src/main.rs:5318` | 工具名不叫 ask_user，语义等价 |
| F13 | `show_view` 卡片（指标/表格/图表/表单/HTML） | `ds-bot/src/host/tools.js:726` | ❌ | grep `"show_view"` = 0 命中 | Bot 无法在回复里画图表/表单 〔降级 v0.6〕 |
| F14 | ACP 表单提问（elicitation） | `ds-bot/src/host/local-agents/acp.js` | ✅ | `src/main.rs:5469/5482`、`src/backend.rs:1259` | 无 |
| F15 | 图片附件（发送 + 历史回源） | `ds-bot/src/host/images.js` | ✅ | `src/rpi_engine.rs:1621`（images_from_attachments）、`src/main.rs:18474` | 无 |
| F16 | 语音转写 | `ds-bot/src/client/voice.js`、`speech.js` | ❌ | `src/main.rs:18557`（只调系统 Win+H）、`:14983` 自认“未接入” | 无 ASR 〔降级 v0.6〕 |

## G. 浏览器（6 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| G01 | 浏览器工具集（打开/读页/点击/输入/等待/上传…） | `ds-bot/src/host/browser/*.js` | ✅ | `src/browser_tools.rs:377-589`（20 个工具） | 无 |
| G02 | 每个 Bot 独立 profile（登录态互不干扰、重启保留） | `ds-bot/src/host/browser/session.js`（`bot:<id>` 分区） | ✅ | `src/browser_tools.rs:46`（`session_key`）、`:387`（`profile_dir` 用 `{data_root}/runtime/browser-profiles/<botId>`，去时间戳）、`:33`（`MAX_SESSIONS=4` LRU）、启动前清 `SingletonLock` | 无 |
| G03 | Cookie 导入（文件 / 本机浏览器） | `ds-bot/src/client/browser-pane.js`（import） | ❌ | `src/browser_tools.rs` 无 cookie 相关代码 | 未入 v0.5 范围（profile 复用已能保留登录态，见 G02） 〔降级 v0.6〕 |
| G04 | 内嵌浏览器面板（在右栏看页面） | `ds-bot/src/client/browser-pane.js` | ❌ | `src/main.rs:8343` 只有“用系统浏览器打开 URL + 最近 + 已装浏览器列表” | 未入 v0.5 范围 〔降级 v0.6〕 |
| G05 | 敏感动作需批准（发布/上传/下载/删除） | `ds-bot/src/host/browser/refs.js`、`handoff-card.js` | 🟡 | 全部浏览器工具走统一审批闸门 `src/rpi_engine.rs:446/503` | 无“发布/付款”专项分级，无接管卡片（v0.6） 〔降级 v0.6〕 |
| G06 | 浏览器插件（refs/script/task） | `ds-bot/src/host/browser-plugins.js` | ❌ | 无 | 未入 v0.5 范围 〔降级 v0.6〕 |

## H. 电脑操控（6 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| H01 | 截屏 / 鼠标 / 键盘 / 窗口 / 剪贴板 | `ds-bot/src/host/computer/index.js` | ✅ | `src/computer_tools.rs:98-250`（14 个工具） | 无 |
| H02 | UI 树读写（按名字点/读/写） | `ds-bot/src/host/computer/index.js`（UIA） | ✅ | `src/cua_tree.rs:272-320`（computer_ui_tree/click/read/set_value） | 无 |
| H03 | 每 Bot 档位（只看 / 后台 / 可动前台） | `ds-bot/src/host/computer/index.js:42`（LEVEL_LABEL） | ❌ | `src/tool_scope.rs:1-9` 只有总开关 + Bot 黑名单；grep `computer_level` = 0 命中 | 无法按 Bot 分级 〔降级 v0.6〕 |
| H04 | 每 Bot 应用授权 + 一 Bot 一应用租约 | `ds-bot/src/host/computer/leases.js`、`catalog.js` | ❌ | 无 | 两个 Bot 会抢鼠标 〔降级 v0.6〕 |
| H05 | 屏幕顶部指示条 / 暂停 / 接管 | `ds-bot/src/client/computer.js` | ❌ | `src/main.rs:10944` 设置页只有“截屏预览” | 用户在 Bot 操作时无感知 〔降级 v0.6〕 |
| H06 | 系统设置/密码管理器永久禁止 | `ds-bot/src/host/computer/catalog.js`（DRIVER_REFUSED） | ❌ | 无 | 安全底线缺失 〔降级 v0.6〕 |

## I. 本地 Agent 与引擎（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| I01 | 内嵌引擎（自带工具/模型） | `README.zh.md:88` | ✅ | `src/rpi_engine.rs:894`（`RpiSession::open`，引擎入口；`:332` 是内置工具集） | 无 |
| I02 | 外置 ACP 引擎（spawn/续聊/表单） | `ds-bot/src/host/local-agents/acp.js` | ✅ | `src/acp.rs:122/167/384`（session/load） | 无 |
| I03 | 引擎选择 UI + 探测 | `ds-bot/src/client/local-agents-page.js` | ✅ | `src/main.rs:12361`、`src/data.rs:366` | 无 |
| I04 | Claude Code / Codex CLI / Cursor / Gemini / OpenCode 接入 | `ds-bot/src/host/local-agents/engines.js:5`（KINDS=acp/codex/claude）、`:16-27`（10 个引擎） | ❌ | `src/data.rs:337-344` 明确“只支持这四个，不再探测”；`src/acp.rs:122` 只有 ACP 一种驱动，无 driver 抽象 | 需新增 codex/claude 两种驱动协议 〔降级 v0.6〕 |
| I05 | 引擎能力差异提示（哪些能力 ACP 不支持） | `ds-bot/src/client/agent-cards.js` | 🟡 | `src/i18n.rs:256`（附件）、`:321`（插话） | 零散提示，无统一能力表 〔降级 v0.6〕 |

## J. IM 通道（9 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| J01 | 微信（iLink 扫码 + 长轮询） | `ds-bot/src/host/im/platforms/weixin.js` | ✅ | `src/im_service.rs:153`（ImQrProvider）、`src/im_runtime.rs:647`（start_loop） | 无 |
| J02 | 企业微信（智能机器人扫码） | `ds-bot/src/host/im/platforms/wecom.js` | ✅ | `src/im_runtime.rs:350`（start_wecom_loop）、`src/im_service.rs:17` | 无 |
| J03 | 飞书 | `ds-bot/src/host/im/platforms/feishu.js` | ❌ | 仅枚举 `crates/bots-service/src/lib.rs:68`；`src/im_runtime.rs` 无 feishu 运行时 | 配得上、跑不起来 〔降级 v0.6（已按空态交付）〕 |
| J04 | 钉钉 | `ds-bot/src/host/im/platforms/dingtalk.js` | ❌ | 仅枚举 `crates/bots-service/src/lib.rs:72` | 同上 〔降级 v0.6（已按空态交付）〕 |
| J05 | QQ / Telegram / Discord / Slack | 对应 `platforms/*.js` | ❌ | `crates/bots-service/src/lib.rs:59` 把 telegram/webhook/discord 列为“已下线直接丢弃” | 无 〔降级 不在路线图〕 |
| J06 | 绑定码（私聊/群绑定到工作区） | `ds-bot/src/host/im/index.js`（pairing） | ✅ | `src/im_service.rs:183`（BindCodeStore）、`src/im_runtime.rs:1785`（handle_bind） | 无 |
| J07 | 心跳（静默巡检） | `ds-bot/src/client/im-page.js`（heartbeat） | ✅ | `src/im_runtime.rs:520/547`（tick_heartbeats） | 无 |
| J08 | 卡片按钮（微信/QQ 用数字编号回选） | `ds-bot/src/shared/im-platforms.js`（buttons） | ✅ | 审批卡：`src/im_runtime.rs:2254`（`/permission <序号>`）、`:2302`（`/deny`）；提问卡：`src/im_runtime.rs:1434`（`SessionEvent::Question` → `notice`）、`:2433`（`/question <序号或文本>`）、`:2437/2491`（`pending_questions` 轮询） | 无平台原生按钮，靠 `/question` 文本回选（与微信审批卡同一套编号语义） |
| J09 | IM 消息镜像到主窗口 | `ds-bot/src/host/im/index.js`（mirror） | ✅ | `src/im_runtime.rs:159-238`（deliver_mirror） | 无 |

## K. 连接器与能力（9 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| K01 | 连接器目录 | `ds-bot/src/host/connectors/catalog.js:20-`（37 个，多数 remote+OAuth） | 🟡 | `crates/connectors-service/src/lib.rs:174`（16 个 stdio 内置）、`:75`（`REMOTE_CATALOG` 8 个 HTTP 远程：huggingface/exa/deepwiki/context7/mslearn/cloudflare-docs/amap/github-copilot）、`:360`（`remote_spec`） | 总数 24（16+8），仍少于 DS 的 37；OAuth 型（GitHub/Gmail/Notion/Drive）不在内，见 K04 〔降级 v0.6（本轮已扩到 24）〕 |
| K02 | 填连接器密钥 | `ds-bot/src/client/connectors-page.js` | ✅ | `src/main.rs:14045`、`src/main.rs:23259`（ConnectorSecret）、`src/connectors.rs:38` | 无 |
| K03 | MCP stdio 工具接入（`mcp__<id>__*`） | `ds-bot/src/host/connectors/gateway.js` | ✅ | `src/mcp_host.rs:43/482`、`src/backend.rs:317` | 无 |
| K04 | OAuth 登录（GitHub / Gmail / Notion / Google Drive） | `ds-bot/src/host/connectors/oauth.js` | ❌ | `src/main.rs:15072` 自认“仍走外部凭据” | 需要 token 手填 〔降级 v0.6〕 |
| K05 | 邮箱连接器（Gmail/QQ/163） | `ds-bot/src/host/connectors/mail/` | ❌ | 无 | 无 〔降级 v0.6〕 |
| K06 | 从 Claude Desktop / Cursor / VS Code 导入配置 | `ds-bot/src/client/connectors-page.js`（import） | ❌ | 无 | 无 〔降级 v0.6〕 |
| K07 | 能力页（团队共享 MCP/技能/能力包） | `README.zh.md:112`、`ds-bot/src/host/capabilities.js` | 🟡 | `src/capabilities.rs:15-52`（只读 4 行、无开关） | 只能看，不能管 〔降级 v0.6〕 |
| K08 | 能力/能力包增删 + 按 Bot 启用 | `ds-bot/src/host/capabilities.js:21`、`client/capability-settings.js` | ❌ | grep `"save_capability"`/`list_capabilities` = 0 命中 | 无 〔降级 v0.6〕 |
| K09 | 插件宿主（第三方插件） | `ds-bot/README.md`（plugin bundle） | ❌ | `src/main.rs:12280-12288` 明写“暂未接入插件宿主” | 页面在，功能空 〔降级 v0.6〕 |

## L. Office 文件（3 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| L01 | 创建/检查/修改 docx、pptx、xlsx | `ds-bot/src/office/docx.js:1`、`pptx.js`、`xlsx.js`、`ds-bot/src/office/check.js:47` | ❌ | 全仓无 office 模块；`Cargo.toml` 无相关依赖 | 完全缺失 〔降级 v0.6〕 |
| L02 | 渲染成 PNG 检查排版 | `ds-bot/src/office/png.js` | ❌ | 无 | 无 〔降级 v0.6〕 |
| L03 | Office 技能注册（SKILL.md + CLI） | `ds-bot/src/host/office-skills.js:1-14` | ❌ | 无 | 无 〔降级 v0.6〕 |

## M. 用量（4 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| M01 | 总量 / 输入 / 输出 / 缓存 | `ds-bot/src/host/usage.js:41-45` | ✅ | `src/rpi_engine.rs:1478-1567`、`src/main.rs:14722-14747` | 无 |
| M02 | 按天趋势 | `ds-bot/src/client/usage-page.js` | ✅ | `src/main.rs:16046`（`usage-range-{1,7,30,90}` chip）、`src/rpi_engine.rs:1661`（`usage_grouped(days)` 的 `by_day`） | 无 |
| M03 | 按 Bot / 按模型拆分 | `ds-bot/src/host/usage.js`（byBot/byModel） | ✅ | `src/main.rs:16069`（`usage-group-{none,bot,model}` chip）、`src/rpi_engine.rs:1661`（`GroupedUsage{by_bot, by_model, sessions}`）、`:1835`（老会话按 header `cwd` 反查名册兜底） | 无 |
| M04 | 时间范围（24h～12 月 / 日历选日） | `ds-bot/src/client/usage-page.js`（range picker） | 🟡 | `src/main.rs:16046` 四档（1/7/30/90 天）、`src/main.rs:3258`（`usage_range`） | 四档够用；无日历选日、无 12 个月（明确降级，v0.6 再加） 〔降级 v0.6〕 |

## N. 定时任务（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| N01 | 列表 / 启停 / 立即运行 / 删除 | `ds-bot/src/client/schedules.js`、`host/schedules.js` | ✅ | `src/main.rs:14345`、`src/automations.rs:83-101` | 无 |
| N02 | 运行历史 | `ds-bot/src/host/schedules.js:122` | ✅ | `src/main.rs:14402`、`src/main.rs:21896`（AutomationHistory） | 无 |
| N03 | cron / 一次性 / 相对时间规则 | `ds-bot/src/host/schedules.js:34-38` | ✅ | `src/automations.rs:218-224`；cron 解析 `crates/automation-schedule/src/cron.rs:1-49`；规则构造 `src/schedule_builder.rs:56`（`to_request`） | 无 |
| N04 | 宿主内置调度器真正执行 | `ds-bot/src/host/schedules.js` | ✅ | `src/automation_scheduler.rs:3/20/40`（20s 轮询） | 无 |
| N05 | 对话内让 Bot 建任务 | `ds-bot/src/host/schedules.js`（tool） | ✅ | `src/automation_tools.rs:71-124`（create/list/set_enabled/delete/run_now） | 无 |
| N06 | 创建时的人类友好选择器（一次 / 每天 / 每周 / 每隔） | `ds-bot/src/host/schedules.js:60-86`（selectorOf） | ✅ | `src/schedule_builder.rs:19`（`When` 四型）、`:179`（`describe` 实时预览）、UI `src/main.rs:10173`（`schedule-when-*`）、`:10103`（once 小时后）、`:10130`（7 天 chip） | 无（日期用「几小时后」下拉代替日历，`to_request` 仍算绝对时间） |

## O. 密钥与审批（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| O01 | 密钥库（只写不读、`${secret:NAME}` 引用） | `ds-bot/src/host/secrets.js` | ✅ | `src/secrets.rs:28-57`、`src/main.rs:14529` | 无 |
| O02 | 对话内 `request_secret` 安全卡片 | `ds-bot/src/host/secrets.js:423` | ❌ | grep `"request_secret"` = 0 命中 | Bot 要密钥只能让用户去设置页手填 〔降级 v0.6〕 |
| O03 | 工具审批卡（允许一次 / 始终允许 / 拒绝） | `ds-bot/src/host/approvals.js` | ✅ | `src/rpi_engine.rs:433-485`、`src/main.rs:5318` | 无 |
| O04 | 三档审批策略（ask/auto/full） | `ds-bot/src/client/permission-picker.js` | ✅ | `src/main.rs:19660-19703`、`src/data.rs:46` | 无 |
| O05 | IM 中禁止填写密钥卡 | `README.zh.md:106` | ❌ | 无（因为 O02 也没做） | 无 〔降级 v0.6〕 |

## P. 模型与服务商（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| P01 | 服务商模板 + 密钥 + baseUrl | `ds-bot/src/client/providers-page.js` | ✅ | `src/data.rs:819/1406`、`src/main.rs:11944`、`src/main.rs:23216-23316`（`SettingsDetail` 全套详情面板） | 无 |
| P02 | 服务商“测试连接” | `ds-bot/src/client/providers-page.js` | ✅ | `src/main.rs:11944` 区块（commit `d7ca085`） | 无 |
| P03 | 添加 / 删除模型 | `ds-bot/src/client/model-menu.js` | ✅ | `src/data.rs:1073`、`src/main.rs:23313`（AddModel） | 无 |
| P04 | 按模型配置上下文窗口（check line 由窗口算） | `ds-bot/README.md`（contextWindow） | ✅ | `src/model_limits.rs:20/22`（`FALLBACK_CONTEXT_WINDOW`/`FALLBACK_MAX_TOKENS`）、`:190`（read）、`:211`（resolve，用户值 > 模板 > 兜底）、`:236`（write）、`src/rpi_engine.rs:144-147`（引用常量，不再写死）、UI `src/main.rs:9505` | 无 |
| P05 | 每 Bot 模型 + 思考强度 | `README.zh.md:58` | ✅ | `src/data.rs:17/41`、`src/rpi_engine.rs:185`（thinking_level_for） | 无 |

## Q. 设置与外观（6 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| Q01 | 设置页结构（通用/模型/Bot/群/连接器/能力/浏览器/定时/用量/密钥/关于…） | `ds-bot/README.md`（Settings pages） | ✅ | `src/main.rs:21811-21829`（17 页） | 无 |
| Q02 | 设计令牌落地（4px 间距、四级字号、圆角令牌） | `docs/ui-design-spec.md`（本项目自有规范） | ✅ | `src/ui_kit.rs:25-45`（令牌）、`:128`（`Clickable::pointer`）；`scripts/ui-lint.ps1 -Enforce` 实测 **0 违规**（`cursor_pointer` 0 / 裸 `rounded(px())` 0 / 非令牌字号 0 / `rgb(` 0） | 无 |
| Q03 | 弹窗 → 右侧详情栏 | `ds-bot/src/client/details-drawer.js` | ✅ | `src/main.rs:21835`（SettingsDetail）、`docs/ui-design-spec.md:§8` | 剩余 6 个破坏性确认框，符合规范 |
| Q04 | 深浅双主题 + 皮肤 | `ds-bot/src/client/themes.js` | ✅ | `src/theme.rs:133/143`、`src/wecom_theme.rs` | 无 |
| Q05 | 多语言（DS：中/英/日/韩/德/法/西/繁） | `ds-bot/src/client/locales/*.js` | 🟡 | `src/i18n.rs:9-21`（zh/en，629 条） | 只有两语 〔降级 v0.6〕 |
| Q06 | 托盘 / 开机自启 / 单实例 / 窗口记忆 | `app/`（桌面壳） | ✅ | `src/tray.rs`、`src/autostart.rs`、`src/instance.rs:16`、`src/window_state.rs` | 无 |

## R. 数据、测试与发布（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| R01 | 数据根兼容旧版（`~/.mode-bot/.mode`） | `README.zh.md:128` | ✅ | `src/data.rs:6/292`、`MODE_BOT_DATA_ROOT` 隔离 | 无 |
| R02 | 会话落盘格式 + 迁移 | `ds-bot/src/host/parts.js` | 🟡 | `src/rpi_engine.rs:306`（sessions_root）、`examples` 内迁移工具 | 自有 JSONL 格式，跨版本迁移靠脚本 〔降级 v0.6〕 |
| R03 | 应用内检查更新 + 自替换 | `ds-bot/src/host/update.js` | ✅ | `src/update.rs:69/209/322`（SHA-256 + 替换/回滚） | 未接签名（minisign） |
| R04 | CI：test + clippy + NSIS + Release | `ds-bot` CI | ✅ | `.github/workflows/release.yml:54-104` | 无 |
| R05 | 自动化测试规模 | `ds-bot/test/` | ✅ | `src/*.rs` 175 个 `#[test]`、`crates/**` 331 个，合计 506；`cargo test --workspace --locked` 20 个测试目标全绿（183 passed / 2 ignored=需真实浏览器） | 无 |

---


## 汇总

| 状态 | v0.4.0 | v0.5.0 | 占比（v0.5.0） |
| --- | --- | --- | --- |
| ✅ 已有 | 73 | **91** | 66.9% |
| 🟡 半成品 | 22 | **18** | 13.2% |
| ❌ 缺失 | 41 | **27** | 19.9% |
| 合计 | 136 | 136 | 100% |

分域明细（✅/🟡/❌，合计 136）：

| 域 | 项数 | ✅/🟡/❌ | 结论 |
| --- | --- | --- | --- |
| A Bot 身份 | 13 | 9/4/0 | 记忆行与管理入口已补（A08）；缺指令编辑、形象上传、换目录重建会话（降级 v0.6） |
| B Main bot | 5 | 3/1/1 | Bot 互相可见/互发消息已补（B05）；缺 Bot 自治建改 Bot（降级 v0.6） |
| C 群聊 | 15 | 11/2/2 | 阶段证据/通过/打回（C08）、群外发言（C13）已补；缺 notice、引用整段对话（降级 v0.6） |
| D 隐私 bot | 5 | 5/0/0 | 隔离闭环：会话/搜索/群 + 文件路径守卫（D05）+ 删除后归档（D04）全通 |
| E 记忆 | 13 | 11/1/1 | Bot 记忆进提示（E03）、三工具（E05）、面板（E09）、拒写凭据（E13）全通；私聊自动回顾降级 v0.6 |
| F 会话与输入 | 16 | 11/3/2 | 压缩状态可见 + 手动压缩已补（F06）；缺 show_view、语音、FTS5、排队条（降级 v0.6） |
| G 浏览器 | 6 | 2/1/3 | 每 Bot 独立 profile 已补（G02）；缺内嵌面板、Cookie 导入、插件（降级 v0.6） |
| H 电脑操控 | 6 | 2/0/4 | 工具齐，分级/租约/接管/永久禁止全未做（降级 v0.6） |
| I 本地 Agent | 5 | 3/1/1 | ACP 齐，codex/claude 原生驱动缺（降级 v0.6） |
| J IM | 9 | 6/0/3 | IM 提问卡可用 `/question` 编号回选（J08）；飞书/钉钉空态交付，运行时装线降级 v0.6 |
| K 连接器与能力 | 9 | 2/2/5 | 目录扩到 24（16 stdio + 8 远程 HTTP，K01）；OAuth/邮件/能力管理降级 v0.6 |
| L Office | 3 | 0/0/3 | 全缺（降级 v0.6） |
| M 用量 | 4 | 3/1/0 | 范围 1/7/30/90 + 按 Bot/按模型分组已补（M02/M03）；无日历选日（降级 v0.6） |
| N 定时任务 | 6 | 6/0/0 | 四型人类选择器 + 实时规则预览（N06）已补，写库经实测 |
| O 密钥与审批 | 5 | 3/0/2 | 缺对话内 request_secret（降级 v0.6） |
| P 模型 | 5 | 5/0/0 | 按模型配窗口/输出上限已补（P04），不再写死 128k |
| Q 设置与外观 | 6 | 5/1/0 | 令牌门禁 0 违规（Q02）；多语言仍双语（降级 v0.6） |
| R 数据/测试/发布 | 5 | 4/1/0 | 测试 506 个 `#[test]`、20 目标全绿；跨版本迁移仍靠脚本（降级 v0.6） |

### 本轮（v0.5.0）净关闭的 18 项

A08、B05、C08、C13、D04、D05、E03、E05、E09、E13、F06、G02、J08、M02、M03、N06、P04、Q02。

对应交付证据：`src/memory_tools.rs`、`src/bot_tools.rs`、`src/privacy.rs`、`src/model_limits.rs`、`src/schedule_builder.rs`（新增）；`src/backend.rs`（`Command::AskBot` / `PostGroup` / `Compaction`）、`src/rpi_engine.rs`（`system_prompt_for_bot` / `compact` / `usage_grouped`）、`src/browser_tools.rs`（按 Bot 隔离）、`crates/bot-team/src/goal.rs` + `src/group_goal.rs`（v2 迁移）、`crates/connectors-service/src/lib.rs`（远程 HTTP）、`src/im_runtime.rs`（`/question`）、`src/main.rs` + `src/ui_kit.rs`（令牌与各面板）。

### 结论（产品视角，v0.5.0 之后的剩余差距）

1. **记忆闭环已通**：Bot 现在有自己的记忆、能主动记/查/忘、下次进提示，面板可管（A08/E03/E05/E09）。剩余是「私聊自动回顾」（E07，v0.6）。
2. **团队协作最小闭环已通**：`list_bots` / `message_bot` / `post_to_group` 可用，隐私 Bot 不可被直连（B05/C13）。剩余是「Main bot 自己建改 Bot」（B02，v0.6）。
3. **上下文可见可配已通**：压缩有状态行、可手动触发，模型窗口按「用户值 > 模板 > 兜底常量」解析（F06/P04）。剩余是 checkpoint/handoff 语义（v0.6）。
4. **隐私承诺与实现对齐**：删除后目录继续受保护并可解除，越界路径即使 `full` 档也弹卡（D04/D05）。
5. **最大的剩余空缺在 H（电脑操控分级/租约/接管）与 L（Office 全无）**，两块都要独立立项，不在 v0.5 范围。
6. **IM 仍是两条线真跑**：微信/企微可用，飞书/钉钉已做成明确空态（不放假入口），运行时实现排 v0.6。
7. **UI 债清零**：`ui-lint.ps1 -Enforce` 0 违规（139 `cursor_pointer` / 44 字号 / 5 `rgb(` 全部收敛到令牌），`docs/ui-design-spec.md` §10/§11 债务计数同步归零。