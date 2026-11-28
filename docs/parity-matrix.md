# ModeChat v0.6.0 对标盘点矩阵（DS Bot v0.3.6 → ModeChat v0.5.0 基线 → v0.6.0 收口）

> 基准：`D:\Mode Bot\DS-Bot`（v0.3.6，`README.zh.md` + `ds-bot/README.md` + `ds-bot/src/host|client|office`）与本机 DS Bot 实机界面。
> 被盘点对象：`D:\ModeChat`（v0.6.0，`Cargo.toml:25`，`src/main.rs` 26,856 行）。
> 证据格式：`源文件:行号`；DS-Bot 侧证据同样给出行号，便于复核。
> 状态图例：✅ 已有（可用且对得上）｜🟡 半成品（有代码/有界面，但不通、不完整或语义不同）｜❌ 缺失。
>
> **本文档状态已按 v0.6.0 更新（2026-11-26）。** 各实现项在「证据」列给出新行号；未实现项在「差距」列写明**明确降级说明**（降级到 v0.7 或标为不在 v0.6 范围）。历次变更：v0.5.0 交付 ✅91 / 🟡18 / ❌27 → v0.5.1 因判据变严 F06、F10 降为 🟡（✅89 / 🟡20）→ **v0.6.0 因删 classic 导致 A12 主卡位无等价物，降为 🟡（✅88 / 🟡21）**。
>
> **锚点基线：`git HEAD` 的已提交内容，不是工作区。** 校验命令 `powershell -File scripts/verify-parity-anchors.ps1`（默认 `-Baseline Head`）。理由：多人并行改 `src/main.rs` 时工作区行号会整体漂移，拿工作区校验会把好锚点误判成「指到了 `}`」——本轮前端未提交的 +45 行就制造过 10 个假失败。要对着工作区校验（改完代码、准备重排锚点前）用 `-Baseline WorkTree`。

统计口径：共 **136 项**，✅ 88 项、🟡 21 项、❌ 27 项（见文末汇总）。v0.4.0 基数为 ✅ 73 / 🟡 22 / ❌ 41，**从 v0.4.0 起累计**净增 ✅ 15 项（73 → 88，跨 v0.5.0 / v0.5.1 / v0.6.0 三次交付），不是单轮净增。

相对 v0.5.1 交付时的 ✅ 89 / 🟡 20 / ❌ 27，v0.6.0 的 ✅ 再减 1，落在 A12 主卡位：`main_bot_card` 随 classic 外壳一起删除，微信壳没有等价的主 Bot 视觉位，本版也不补（`docs/prd-0.6.md` §2.3 明确不做）。**这与 v0.5.1 的降级性质不同**：那两项是判据变严把记宽了的项改准，A12 是删代码带走的真实能力损失，所以补主卡位记 v0.7。

（v0.5.1 的 −2 记录：F06 与 F10 在 v0.5.0 被记为全 ✅ 但都留着降级项——F06 的 checkpoint/handoff 语义、F10 的引用条——复验改用「默认外壳为主判据」后按真实完成度重算。两项在默认外壳下的能力实际都比 v0.5.0 强。）

---

## A. Bot 身份与生命周期（13 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| A01 | 新建 Bot（名字/角色/工作目录） | `README.zh.md:58` | ✅ | `struct BotFormState`）、`create_bot`） | 无 |
| A02 | 编辑 Bot | `ds-bot/src/host/tools.js:363`（update_bot） | ✅ | `src/main.rs`（`save_bot_form`）、`update_bot`） | 只会改 7 个字段，改不了模型/指令 |
| A03 | 删除 Bot（二次确认） | `ds-bot/src/host/tools.js:298`（stop_bot/删除走 UI） | ✅ | `src/data.rs:701`（`delete_bot`）、`fn open_delete_bot_dialog`（`src/main.rs`） | 无 |
| A04 | Bot 形象：20 字符 + 12 色 + 自选照片/头像库 | `ds-bot/src/client/characters.js`、`avatar-library.js` | 🟡 | `for (key, _label) in bot_icon::ICONS`）、`for (hex, label) in bot_icon::COLORS`）、`src/bot_icon.rs:18`（`ICONS`）/`COLORS`） | 只有 1 组图标 + 8 色（`bot_icon.rs` 的 `ICONS`/`COLORS`；原记的「9 图标 + 5 色」已过时），无照片上传、无头像库 〔降级 v0.6〕 |
| A05 | Bot 指令（instructions）可编辑 | `README.zh.md`（instructions） | ✅ | `crates/bot-team/src/teammate.rs:36`（记录字段）→ `src/data.rs`（Bot.instructions 解析/写回 `update_bot`）；Bot 编辑对话框「指令」字段 → `sync_identity_fields` 写入 AGENTS.md「## 指令」 | 无 |
| A06 | 换工作目录后会话在新目录继续 | `ds-bot/README.md`（Working folders） | 🟡 | `update_bot`）只改名册；`src/main.rs`（`restore_draft`）仅在新会话时读目录 | 改目录不重建会话，旧会话仍跑旧目录 〔降级 v0.6〕 |
| A07 | Bot 详情页（模型/审批/会话/待办/目录） | `ds-bot/src/client/details-drawer.js` | ✅ | `src/main.rs:25082`（`chat_info_panel`，微信壳详情栏；classic 的 `bot_details` 已随 D-CUT 删除） | 无记忆行（见 A08） |
| A08 | 详情页「记忆」行 + 管理入口 | `ds-bot/src/client/memory-panel.js`、`details-drawer.js` | ✅ | `.id("chat-info-memory-row")`，微信壳详情栏）、枚举分支与面板体 `src/main.rs:9684`（`SettingsDetail::BotMemory`）；classic 的 `bot-details-memory-row` 已随 D-CUT 删除 | 无 |
| A09 | 侧栏分组（section）归组/折叠/染色 | `ds-bot/src/client/folder-menu.js`、`host/folders.js` | ✅ | `fn sidebar_sections_section`，分组 CRUD）、`:13114`（`section-up-*`）、`:13166`（`section-delete-*`）、`src/main.rs:751`（`sidebar_section_dot` 染色辅助） | 无 |
| A10 | 置顶 / 隐藏 / 恢复显示 | `ds-bot/src/client/sidebar.js`（pushpin） | ✅ | `src/data.rs:40`（`pinned`）、`:79`（`hidden_bots`）、`set_bot_flags`）、`src/main.rs`（「隐藏」菜单项）、`src/main.rs:14154`（「已隐藏」区） | 无 |
| A11 | Bot 实时状态（正在做什么） | `ds-bot/src/client/member-status.js`、`host/feed.js` | 🟡 | `src/backend.rs:191`（`SessionEvent::Delta` 带 `member`，成员状态事件）、`src/main.rs:2861`（`push_member_delta`） | 只有“正在输出”，没有“正在做什么/在哪一步” 〔降级 v0.6〕 |
| A12 | 主卡位（Main bot 卡片） | `README.zh.md:68` | ✅ | `src/data.rs:37`（`is_main`）→ `fn main_bot_card`（`src/main.rs:24440`：会话列表顶部固定突出卡，accent 软底 + 主 Bot 徽标 + 状态点 + 最近预览，点击直达；普通列表不重复占行，未设主 Bot 时不占位） | 无 |
| A13 | Bot 详情页用量/待办 | `ds-bot/src/client/usage-page.js` | ✅ | `ui::section(i18n::t("待办"))`，微信壳详情栏；classic 的 `bot_details` 已随 v0.6 D-CUT 删除）、`im-group-activation-{label}`，用量） | 无 |

## B. Main bot 与权限（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| B01 | Main bot 全局唯一 | `README.zh.md:68` | ✅ | `src/data.rs:98`（`demote_other_chiefs`）、`:175`（晋升 Main bot 时调用）/`:682`（`demote_other_chiefs`，降级时调用） | 无 |
| B02 | Main bot 通过对话管理其他 Bot | `ds-bot/src/host/tools.js` | ✅ | `src/team_tools.rs`（chief 专属工具集新增 `team_create_bot` / `team_update_bot` / `team_stop_member`，走 data 层同一份名册与校验） | 仅 chief 成员的群会话装载（与 DS Main-only 一致） |
| B03 | 权限预设（只读 / 工作区 / 完全访问） | `ds-bot/README.md`（Permission presets） | 🟡 | `src/data.rs:46`（approval_level）、`src/main.rs:13053`（`permission_menu_overlay`，ask/auto/full 三档）、`:19694`（`approval_note`）；classic 的 `bot_details` 已删 | 三档是“工具审批”语义，不是文件系统权限预设 〔降级 v0.6〕 |
| B04 | 审批白名单可视化维护 | `ds-bot/src/client/approval-notices.js` | ✅ | `src/main.rs:13053`（`permission_menu_overlay`，三档审批）、`src/main.rs:11095`（`SettingsDetail::Allowlist` 分支） | 无 |
| B05 | Bot 互相可见 / 互发消息 | `ds-bot/src/host/tools.js:80`（list_bots）、`:167`（message_bot） | ✅ | `src/bot_tools.rs:78`（`list_bots` 工具；同一个 `tools` 里还有 whoami `:69`、message_bot `:87`、post_to_group `:104`）、`src/backend.rs:561`（`Command::AskBot`，60s 内没答完回「已投递」、目标在跑即拒、隐私直拒） | 无 |

## C. 群聊协作（15 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| C01 | 群聊增删改（成员/目录/策略） | `README.zh.md:74` | ✅ | `src/data.rs:1854`（`create_group`）/`:1873`（`update_group`）/`:1906`（`delete_group`）、`data::GroupDraft` 构造）、`src/main.rs:18953`（`delete_group` 调用） | 无 |
| C02 | 群聊共享工作目录生效于所有成员 | `ds-bot/README.md`（group folder） | ✅ | `src/data.rs:62`（`directory`）；建成员会话时群目录优先：`let workspace = match &group.directory`） | 无（改群目录后已建会话需重进，属可接受） |
| C03 | 管理员先答 / 全员轮流（responder_policy） | `ds-bot/README.md`（admin-first / all-members） | ✅ | `src/backend.rs:1686`（`responder_policy` 路由）/`:1684`（`chosen` 选取） | 无 |
| C04 | @ 点名（含 @所有人、边界规则） | `ds-bot/src/shared/mentions.js` | ✅ | `src/backend.rs:1684`（bot_team::mentions::parse_mentions）、`crates/bot-team/src/mentions.rs:110` | 无 |
| C05 | 成员无事可说时静默（`[PASS]`） | `ds-bot/src/host/groups.js`（[PASS]） | 🟡 | `crates/bot-team/src/mentions.rs:110`（is_pass_reply）、`crates/bot-team/src/orchestration.rs:85` | 判定函数存在，群回合是否吞掉回复未接（需实测确认） 〔降级 v0.5 保持现状〕 |
| C06 | 每成员每群一个独立隐藏会话 | `ds-bot/README.md`（hidden Session per group） | ✅ | `src/backend.rs:1578`（`member_session_key` 首次调用）/`:2426`（`member_session_key` 定义） | 无 |
| C07 | 群目标 Goal（对齐/阶段/验收） | `ds-bot/src/host/goals.js:670`（group_goal） | ✅ | `src/group_goal.rs:55`（`group_goal::read`）/`:105`（`write`，私有）/`:117`（`group_goal::apply`）、`crates/bot-team/src/goal.rs:208`（`migrate_goal`）/`:577`（`render_goal_prompt`）、UI `src/main.rs:1102`（`GoalStageEdit`）/`:1134`（`group_goal::apply` 调用）/`:1639`（`goal-stage-pass-*`） | 无 |
| C08 | 阶段“证据”校验后才推进 | `ds-bot/src/host/goals.js:19`（STAGE_ACCEPTANCE_MAX）、`:760/767`（pass/rework） | ✅ | `crates/bot-team/src/goal.rs:136/142/147`（SetAcceptance/PassStage/ReworkStage）、`:208`（migrate_goal）、`src/team_tools.rs:590/634/659`（三个工具）、UI `src/main.rs:1639`（`goal-stage-pass-*`）、`:1665`（`goal-stage-rework-*`）、`:1686`（`goal-stage-acceptance-*`）、`goal-acceptance-save`）、`:1730`（`goal-stage-error-*` 行内错误） | 无 |
| C09 | 派活台账（子任务 + 负责人 + 期望结果） | `ds-bot/src/host/tools.js:167`、`groups.js` | ✅ | `src/group_tasks.rs:210/253`、`team_assign` 工具名） | 无 |
| C10 | 任务 Stop / Retry / Reassign / Cancel | `ds-bot/src/host/groups.js`（task actions） | ✅ | `src/main.rs:2312`（`task-stop-*`）、`:2392`/`:2435`（`task-reassign-*`）、`:2102`（`cancel_subtree`）、`:2146`（`retry_or_reassign`）、`src/group_tasks.rs:347/371/392/446` | 无 |
| C11 | 群公告 notice（共享指令） | `ds-bot/src/client/group-settings.js`（notice） | 🟡 | `src/data.rs`（`Group.instructions` 群规则：编辑对话框「群聊规则」+ 注入群 prompt） | 无独立公告展示位（用群规则顶替，成员在 prompt 里看到） 〔降级 v0.7〕 |
| C12 | “+ → 引用某个 Bot 的对话”作为上下文 | `ds-bot/src/client/composer.js`（Reference a chat） | ✅ | 命令面板「引用 <Bot> 的最近对话」：取该 Bot 最近 6 条消息拼成引用块插入输入框（`PaletteCmd::QuoteBot`） | 无独立选择器（按 Bot 列举，直接取最近对话） |
| C13 | 从群外往群里发消息（post_to_group / group_relay） | `ds-bot/src/host/tools.js:607`、`ds-bot/src/host/groups.js:1439` | ✅ | `src/bot_tools.rs:104`（post_to_group 工具）、`src/backend.rs:569`（`Command::PostGroup`）、`BackendEvent::GroupPost` 走既有 `send_group_in_turn` | `group_relay`（IM 群里的转发）未做，仅 Bot 主动发言 |
| C14 | 群成员状态行（谁在跑、跑什么） | `ds-bot/src/client/member-status.js` | 🟡 | `src/backend.rs:191-194`、`src/main.rs:2879`（`finish_member`） | 只有成员标签，无状态聚合行 〔降级 v0.6〕 |
| C15 | 群记忆注入到每个成员回合 | `ds-bot/README.md`（group memory snapshot） | ✅ | `src/data.rs:404`、`src/backend.rs:1740`（`group_memory`） | 无 |

## D. 隐私 bot（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| D01 | 会话独立落盘、不进全局搜索 | `ds-bot/src/host/privacy.js:29-32` | ✅ | `private_bot_id`）、`src/backend.rs:335-353`、`src/rpi_engine.rs:1644`（`private_workspaces`：搜索跳过 `.private`） | 无 |
| D02 | 不能进群 | `README.zh.md:82` | ✅ | `.filter(|bot| !bot.is_private)` 八处（`src/main.rs:4381/4701/5622/8949/11651/11661/11668/19756`，最后一处在 `fn visible_group_members` 里）、群表单渲染 `:2506` | 无 |
| D03 | 不能当 Main bot / Chief | `README.zh.md:82` | ✅ | `src/data.rs:90`（check_chief_private）、`:98` | 无 |
| D04 | 删除后内容继续受保护（kept-private 登记） | `ds-bot/src/client/kept-private.js`、`ds-bot/src/host/privacy.js:31-32` | ✅ | `src/privacy.rs:80`（archive_private_bot）、`:51`（archived_private_bots）、`:98`（release_archive）、`archive_private_bot`，删隐私 Bot 前登记）、UI `src/main.rs:12470`（`archived_private_bots` 读归档列表）、`src/main.rs:12519`（`private-archive-row-*` 行）、`release_archive` 解除保护） | 无 |
| D05 | 出站共享需审批（写外部文件、给其他 Bot 发消息） | `ds-bot/src/host/privacy.js:172`（guardTeam）、`:242`（guardFiles） | ✅ | `src/privacy.rs:201`（`privacy::precheck`：路径归属最长前缀判定）、`src/rpi_engine.rs:451`（`cross_dir`：非 full 档先跑 precheck；full=完全自主时跳过） | 只覆盖 `read/write/edit/bash/powershell/mcp__*`；`computer_*`/`browser_*` 不在路径范围（见 §5 风险表 R-07） |

## E. 记忆（13 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| E01 | Markdown 记忆 + `## Index` + 主题文件 | `ds-bot/src/host/memory.js`、`memory-entries.js` | ✅ | `crates/bots-service/src/memory_store.rs:447`（splice_index_line）、`:586`（maintain_index） | 无 |
| E02 | 团队记忆进系统提示 | `README.zh.md`（In the prompt） | ✅ | `src/rpi_engine.rs:338-371`（system_prompt_for / system_prompt_for_bot） | 无 |
| E03 | Bot 自身记忆进系统提示 | 同上 | ✅ | `src/rpi_engine.rs:346`（`system_prompt_for_bot`，预算 `clamp(window*0.06, 2000, 24000)` 字符）、`bot_memory_context`）、注入点 `src/rpi_engine.rs:364` | 无 |
| E04 | 群记忆进成员提示 | 同上 | ✅ | `src/backend.rs:1740`（`group_memory` 注入成员回合） | 无 |
| E05 | 记忆工具 `remember` / `forget` / `recall` | `ds-bot/src/host/memory.js:1075/1114/1151` | ✅ | `src/memory_tools.rs:58`（`tools(bot_id)` 注册三个工具）、`read_bot_memory`）/`save_bot_memory`）/`delete_bot_memory`）/`undo_bot_memory`） | 无 |
| E06 | 群记忆自动回顾（idle 后一次模型调用） | `ds-bot/src/host/memory-review.js:1-6` | ✅ | `src/memory_review.rs:83/196/318`、`src/backend.rs:2482`（run_memory_review） | 无 |
| E07 | 私聊记忆自动回顾 | 同上 | ❌ | `src/backend.rs:2482-2256` 只处理 `group_id` | **明确降级到 v0.6**：私聊不自动回顾（手动 `remember` 已可用，见 E05） 〔降级 v0.6〕 |
| E08 | 记忆面板：团队 / 群 | `ds-bot/src/client/memory-panel.js` | ✅ | `src/main.rs`（`settings_page_team_memory`，团队）、`fn group_memory_section`（`src/main.rs`，群） | 无 |
| E09 | 记忆面板：单个 Bot | 同上 | ✅ | `src/main.rs:9684`（`SettingsDetail::BotMemory` 列表/新增/撤销/编辑）、`src/main.rs`（`MemoryScope::Bot`） | 无 |
| E10 | 记忆整理（tidy / 合并重复 / archive） | `ds-bot/src/host/memory-tidy.js` | ✅ | `crates/bots-service/src/memory_store.rs:659`（tidy_memory）、`:640`（maybe_daily_tidy 每日一次，写入路径自动触发 `:1133/1161`） | 自动整理已在跑；缺「手动整理 + 撤销整批」的 UI 入口 |
| E11 | 变更历史 + 撤销 | `ds-bot/src/host/memory.js`（journal.jsonl） | ✅ | `crates/bots-service/src/memory_store.rs:1224/1258`、`src/data.rs:532`（`undo_team_memory`）/`undo_bot_memory`） | 无 |
| E12 | 条目移动到其他作用域 / 摘要 | `ds-bot/src/host/memory-copy.js` | 🟡 | `crates/bots-service/src/memory_store.rs:805`（move_memory，仅测试调用 `:1631`） | 能力有，UI 无入口 〔降级 v0.6〕 |
| E13 | 记忆安全（拒绝密钥/密码） | `ds-bot/README.md`（Safety） | ✅ | `src/memory_tools.rs:397`（`looks_sensitive`：密码/令牌/密钥/长数字，`:194` 返回中文拒绝语）、`src/memory_review.rs:196`（`parse_operations` 字段校验） | 无 |

## F. 会话、上下文与输入（16 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| F01 | 历史会话切换（resume） | `ds-bot/src/host/parts.js` | ✅ | `src/main.rs:22022`（历史会话列表，`HISTORY_ROWS`）、`panel-history-{target}` 行）、`history_entries`） | 无 |
| F02 | 新会话 | 同上 | ✅ | **三条入口**：Ctrl+N `src/main.rs:6992`（`on_new_session_shortcut`）、标题栏「+」`.id("wechat-new-session")` （点击回调调共用函数 `new_session_for_selected`）、命令面板「新会话」`PaletteCmd::NewSession` （派发 `:7795`）。共用实现是 `new_session_for_selected`（）：Ctrl+N 与标题栏「+」都走它；命令面板那条（`:7795`）是**行为等价但独立**的实现（自带 streaming 守卫、`push_history`、`sessions.save()`，未调共用函数），去重记 v0.6 | 无 |
| F03 | 回合中插话（steer，不打断） | `ds-bot/src/host/steer.js` | ✅ | `src/composer_input.rs:16`（SendSecondary）、`src/rpi_engine.rs:1300-1324` | 无 |
| F04 | 输入排队（Enter 排队 / 回合结束自动放行） | `ds-bot/src/client/queue-dock.js` | ✅ | `src/main.rs`（`queue_current`/`flush_queue`、`queued: Vec<QueuedSend>`、`MAX_QUEUED = 5`）；输入框上方「N 条消息排队中 · 回合结束后自动发送 + 清空」提示条；右栏「排队中的消息」明细 | 无 |
| F05 | 旁路会话 `/side`（不写主 transcript） | `ds-bot/src/host/side.js:1-5` | ✅ | `strip_prefix("/side")` 命令入口）、`fn open_side`）、`fn apply_side_event`，旁路事件不进主 chat） | 无 |
| F06 | 上下文压缩 / checkpoint / handoff（按模型窗口的比例触发） | `ds-bot/src/host/steward.js:20-32` | 🟡 | 自动压缩：引擎默认已开（`crates/rpi-harness/src/types.rs:327-330`，pi-rust checkout）；MC 接入：`harness.events().watch` 监听）、`SessionEvent::Compaction`）、`:1212`（`RpiSession::compact()`）；UI `src/main.rs`（`chat.compacting` 写入）、`:24818`（`chat.compacting` 读点）、`:19212`（`WecomRow::Compacting` 行分支）、行定义 `fn compaction_status_row`，`id="chat-compaction-status"` 在 `:21617`）、`:1030`（`PaletteCmd::Compact` 定义）→ `:7600`（派发）、入口：命令面板（`PaletteCmd::Compact`） | IM 侧 `/compact` 已接（BotCommand::Compact → compact_session）；checkpoint/handoff 语义未做（上游 pi-rust 无公开 API） | 〔降级 v0.6：checkpoint/handoff 语义〕 |
| F07 | 全库聊天搜索 | `ds-bot/src/host/chat-index.js:4` | ✅ | `src/rpi_engine.rs:1591-1630`、`src/main.rs:7310`（`on_find_shortcut`）/`search_sessions` 调用）/`fn find_dialog_overlay`（`src/main.rs`） | 无 |
| F08 | FTS5 索引（快、可搜历史全部 part） | `ds-bot/src/host/chat-index.js:48` | ✅ | `src/search_index.rs`（独立 FTS5 索引库 `v2/search-index.sqlite`，trigram 分词 CJK 友好，`refresh` @ `:52`，mtime 增量重建）；Ctrl+F 先查索引、不可用回退 `src/rpi_engine.rs:1591` 线性扫；隐私目录/workspace 排除 | 旧 db.sqlite 走原 LIKE（新会话全在 JSONL） |
| F09 | 长会话窗口化渲染 | `ds-bot/src/client/cells.js` | ✅ | `docs/pilot-notes.md:146`（最近 120 条）、`src/main.rs:255`（`const TRANSCRIPT_WINDOW = 120`）、`transcript_limit.saturating_add`，累计翻页） | 无 |
| F10 | Markdown（表格/代码块/列表/引用） | `ds-bot/src/client/text.js`、`mark.js` | 🟡 | `src/main.rs:22748`（`kit_markdown_el`，`.selectable(true)` `:22720`）**v0.6 删除 classic 后只剩微信壳一条路径**（`wechat_transcript:25966`）。**Q-04 的动作条**：`wechat_message_actions`（`:20938`）在 `wechat_row_el`（`:20197`）内两个调用点；动作条默认显示在正文下面（icon 样式），流式回答结束才出现；含「复制」`:20970`（按钮 id `wecom-copy-{index}`）、「引用」`:21002`（id `wechat-quote-{index}`）与「复制代码」（仅当 `:21020` 的 `fenced_code` 命中）。classic 的 `message_actions` 已随 D-CUT 删除 | 引用条已接入 composer；动作条 2026-11-27 改为正文下方 icon 样式 〔降级 v0.6〕 |
| F11 | 思考过程折叠显示 | `ds-bot/src/client/turn-reasoning.js` | 🟡 | `src/main.rs:803`（`MsgKind::Reasoning`，枚举在 `:801`）、`:3711`（`WecomRow::ToolGroup` 折叠组变体；渲染分支 `WecomRow::ToolGroup` 两处（`src/main.rs`/`:20462`）） | 思考段可显示，折叠组只覆盖工具卡 〔降级 v0.6〕 |
| F12 | 阻塞式提问卡（ask_user） | `ds-bot/src/host/tools.js:670` | ✅ | `src/rpi_engine.rs:2178`（`ask_user_question` 工具名）、UI `src/main.rs:5951`（`question_cards`）/（`question_card_el`） | 工具名不叫 ask_user，语义等价 |
| F13 | `show_view` 卡片（指标/表格/图表/表单/HTML） | `ds-bot/src/client/`（show_view） | ✅ | `src/show_view.rs`（工具 `show_view`：table/metrics/note → GFM 表格/列表文本，经 `kit_markdown_el` 渲染；主/群/IM 会话均注入） | 图表/原生表单/HTML 不渲染（降级为表格/列表） |
| F14 | ACP 表单提问（elicitation） | `ds-bot/src/host/local-agents/acp.js` | ✅ | `src/main.rs:6170`（`elicitation_cards`）/（`elicitation_card_el`）/`:6523`（`submit_elicitation`）、`src/backend.rs:144`（`ElicitationCard`） | 无 |
| F15 | 图片附件（发送 + 历史回源） | `ds-bot/src/host/images.js` | ✅ | `src/rpi_engine.rs:2106`（images_from_attachments）、`src/main.rs`（`stage_attachment_paths`） | 无 |
| F16 | 语音转写 | `ds-bot/src/client/voice.js`、`speech.js` | ❌ | `src/main.rs:19290`（`toggle_voice_typing`，只调系统 Win+H）、 自认“未接入” | 无 ASR 〔降级 v0.6〕 |

## G. 浏览器（6 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| G01 | 浏览器工具集（打开/读页/点击/输入/等待/上传…） | `ds-bot/src/host/browser/*.js` | ✅ | `src/browser_tools.rs:445`（`tools`，20 个工具 `:450`-`:651`） | 无 |
| G02 | 每个 Bot 独立 profile（登录态互不干扰、重启保留） | `ds-bot/src/host/browser/session.js`（`bot:<id>` 分区） | ✅ | `src/browser_tools.rs:46`（`session_key`）、`profile_dir` 用 `{data_root}/runtime/browser-profiles/<botId>`，去时间戳）、`:33`（`MAX_SESSIONS=4` LRU）、启动前清 `SingletonLock` | 无 |
| G03 | Cookie 导入（文件 / 本机浏览器） | `ds-bot/src/client/browser-pane.js`（import） | 🟡 | `src/browser_tools.rs`（`import_cookies`：Netscape cookies.txt 与 Puppeteer/Playwright JSON 自动识别 → CDP `Network.setCookies`）；设置 → 浏览器「导入 Cookie」入口 | 只做文件导入；本机浏览器 cookie 直读（DPAPI 解密）未做 〔降级 v0.7〕 |
| G04 | 内嵌浏览器面板（在右栏看页面） | `ds-bot/src/client/browser-pane.js` | 🟡 | 设置 → 浏览器「页面截图」可把当前页面抓进应用内预览；「打开/最近/已装」入口在插件页 | 真内嵌 WebView（wry 子窗口）未做：GPUI 自绘 surface 与 WebView2 子窗口合成风险高，评估后暂不接 〔降级 v0.7〕 |
| G05 | 敏感动作需批准（发布/上传/下载/删除） | `ds-bot/src/host/browser/refs.js`、`handoff-card.js` | 🟡 | 全部浏览器工具走统一审批闸门 `src/rpi_engine.rs:422`（`hook`）/`:503`（`ask_permission`） | 无“发布/付款”专项分级，无接管卡片（v0.6） 〔降级 v0.6〕 |
| G06 | 浏览器插件（refs/script/task） | `ds-bot/src/host/browser-plugins.js` | ✅ | `src/browser_tools.rs` 一体化工具集覆盖三形态：refs（`browser_snapshot` 编号 + `browser_click`/`browser_type`/`browser_select` 按引用操作）、script（`browser_eval`）、task（后台任务 + 浏览器工具组合：Bot 可 task_start 一个网页任务） | 架构不同（DS 三插件选一后端，我们单工具集含全部能力） |

## H. 电脑操控（6 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| H01 | 截屏 / 鼠标 / 键盘 / 窗口 / 剪贴板 | `ds-bot/src/host/computer/index.js` | ✅ | `src/computer_tools.rs:100-252`（14 个工具） | 无 |
| H02 | UI 树读写（按名字点/读/写） | `ds-bot/src/host/computer/index.js`（UIA） | ✅ | `src/cua_tree.rs:272-320`（computer_ui_tree/click/read/set_value） | 无 |
| H03 | 每 Bot 档位（只看 / 后台 / 可动前台） | `ds-bot/src/host/computer/index.js:42`（LEVEL_LABEL） | ✅ | `src/tool_scope.rs`（`ComputerLevel` look/background/foreground，`ensure_computer_action` 执行层判定：只看禁输入、后台禁切前台）；设置 → 电脑操控「分级与安全」档位 chip | 无 |
| H04 | 每 Bot 应用授权 + 一 Bot 一应用租约 | `ds-bot/src/host/computer/leases.js`、`catalog.js` | ✅ | `src/tool_scope.rs`（`computer_app_grants` 白名单 + `ensure_app_granted`）、`src/computer_tools.rs`（`APP_LEASES` 进程级租约：同应用 5 分钟空闲释放、他 Bot 占用即拒） | 授权列表走 prefs 配置（UI 只读展示） |
| H05 | 屏幕顶部指示条 / 暂停 / 接管 | `ds-bot/src/client/computer.js` | 🟡 | `src/tool_scope.rs`（`computer_paused` 全局暂停：所有电脑操控含截屏被拒）；设置 → 电脑操控「全局暂停/恢复」按钮 | 无屏幕顶部悬浮指示条与接管手势（暂停即接管语义） 〔降级 v0.7〕 |
| H06 | 系统设置/密码管理器永久禁止 | `ds-bot/src/host/computer/catalog.js`（DRIVER_REFUSED） | ✅ | `src/tool_scope.rs`（`BUILTIN_BLOCKED` 内置系统设置/凭据管理器/密码管理器/锁屏关键词 + prefs `computer_blocklist`）、`src/computer_tools.rs`（输入/聚焦前查前台/目标窗口标题与进程名，命中即拒） | 无 |

## I. 本地 Agent 与引擎（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| I01 | 内嵌引擎（自带工具/模型） | `README.zh.md:88` | ✅ | `src/rpi_engine.rs:1098`（`RpiSession::open`，引擎入口；`:373` 是内置工具集） | 无 |
| I02 | 外置 ACP 引擎（spawn/续聊/表单） | `ds-bot/src/host/local-agents/acp.js` | ✅ | `src/acp.rs:122/167/384`（session/load） | 无 |
| I03 | 引擎选择 UI + 探测 | `ds-bot/src/client/local-agents-page.js` | ✅ | `fn settings_page_local_agents`）、`src/data.rs:376`（`local_agents`） | 无 |
| I04 | Claude Code / Codex CLI / Cursor / Gemini / OpenCode 接入 | `ds-bot/src/host/local-agents/engines.js:5`（KINDS=acp/codex/claude）、`:16-27`（10 个引擎） | ❌ | `src/data.rs:349-356` 明确“只支持这四个，不再探测”；`src/acp.rs:122` 只有 ACP 一种驱动，无 driver 抽象 | 需新增 codex/claude 两种驱动协议 〔降级 v0.6〕 |
| I05 | 引擎能力差异提示（哪些能力 ACP 不支持） | `ds-bot/src/client/agent-cards.js` | 🟡 | `src/i18n.rs:257`（“当前引擎不支持附件发送”）、`:319`（“回合进行中”） | 零散提示，无统一能力表 〔降级 v0.6〕 |

## J. IM 通道（9 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| J01 | 微信（iLink 扫码 + 长轮询） | `ds-bot/src/host/im/platforms/weixin.js` | ✅ | `src/im_service.rs:155`（`ImQrProvider` 枚举）/`:153`（其文档注）、`src/im_runtime.rs:1016`（start_loop） | 无 |
| J02 | 企业微信（智能机器人扫码） | `ds-bot/src/host/im/platforms/wecom.js` | ✅ | `src/im_runtime.rs:719`（start_wecom_loop）、`src/im_service.rs:17` | 无 |
| J03 | 飞书 | `ds-bot/src/host/im/platforms/feishu.js` | 🟡 | 外挂适配器 `adapters/feishu/adapter.mjs`（长连接帧协议按官方 SDK 结构实现：outbound_ws endpoint + 二进制帧 SIGNAL/DATA/CONTROL，OpenAPI 文本/图片/文件回复）；IM 频道页飞书卡可凭据创建（App ID/Secret） | 官方帧协议未公开、待真实凭据联调确认（出入可发适配器包在线修正） 〔降级 v0.7（待联调）〕 |
| J04 | 钉钉 | `ds-bot/src/host/im/platforms/dingtalk.js` | 🟡 | Stream 模式已接入：`crates/bots-service/src/dingtalk.rs`（gateway/帧/事件解析）、`src/im_runtime.rs`（refresh_dingtalk + stream 循环 + sessionWebhook 回复）、IM 频道页凭据创建（App Key/Secret） | 代码与 UI 完整，缺真实凭据联调；发文件未做（webhook 仅文本） 〔降级 待凭据联调〕 |
| J05 | QQ / Telegram / Discord / Slack | 对应 `platforms/*.js` | ❌ | `crates/bots-service/src/lib.rs:60` 把 telegram/webhook/discord 列为“已下线直接丢弃” | 无 〔降级 不在路线图〕 |
| J06 | 绑定码（私聊/群绑定到工作区） | `ds-bot/src/host/im/index.js`（pairing） | ✅ | `src/im_service.rs:183`（BindCodeStore）、`src/im_runtime.rs:2745`（`handle_bind`） | 无 |
| J07 | 心跳（静默巡检） | `ds-bot/src/client/im-page.js`（heartbeat） | ✅ | `src/im_runtime.rs:520/547`（tick_heartbeats） | 无 |
| J08 | 卡片按钮（微信/QQ 用数字编号回选） | `ds-bot/src/shared/im-platforms.js`（buttons） | ✅ | 审批卡：`src/im_runtime.rs:3214`（`PermissionRespond`，即 `/permission <序号>` 的处理）、`:3262`（`Deny`，即 `/deny`）、帮助文案 `:3070`；提问卡：`src/im_runtime.rs:2282`（`SessionEvent::Question` → `notice`）、`:3293`（`BotCommand::QuestionRespond`，`/question`）、`:2447`（`pending_questions` 轮询） | 无平台原生按钮，靠 `/question` 文本回选（与微信审批卡同一套编号语义） |
| J09 | IM 消息镜像到主窗口 | `ds-bot/src/host/im/index.js`（mirror） | ✅ | `src/im_runtime.rs:182-238`（`deliver_mirror`） | 无 |

## K. 连接器与能力（9 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| K01 | 连接器目录 | `ds-bot/src/host/connectors/catalog.js:20-`（37 个，多数 remote+OAuth） | 🟡 | `crates/connectors-service/src/lib.rs:174`（16 个 stdio 内置）、`:75`（`REMOTE_CATALOG` 8 个 HTTP 远程：huggingface/exa/deepwiki/context7/mslearn/cloudflare-docs/amap/github-copilot）、`:360`（`remote_spec`） | 总数 24（16+8），仍少于 DS 的 37；OAuth 型（GitHub/Gmail/Notion/Drive）不在内，见 K04 〔降级 v0.6（本轮已扩到 24）〕 |
| K02 | 填连接器密钥 | `ds-bot/src/client/connectors-page.js` | ✅ | `src/main.rs:9964`（`SettingsDetail::ConnectorSecret`）、`:15875`（`ConnectorSecret` 打开点）、`src/connectors.rs:38` | 无 |
| K03 | MCP stdio 工具接入（`mcp__<id>__*`） | `ds-bot/src/host/connectors/gateway.js` | ✅ | `src/mcp_host.rs:43/482`、`src/backend.rs:320`（`open_rpi_session`）/`:345`（`mcp_host::for_workspace`） | 无 |
| K04 | OAuth 登录（GitHub / Gmail / Notion / Google Drive） | `ds-bot/src/host/connectors/oauth.js` | ✅ | `src/oauth.rs`（PKCE S256 + state + 本地回调 `127.0.0.1:17890`，GitHub/Google/Notion token 交换，`begin` @ `:110`）；连接器页「OAuth 授权」区块；token 落 secrets.json `oauth:<平台>`，连接器按 Bearer 直接可用 | 各平台 OAuth 应用需用户自建（回调填本机地址） |
| K05 | 邮箱连接器（Gmail/QQ/163） | `ds-bot/src/host/connectors/mail/` | ❌ | 无 | 无 〔降级 v0.6〕 |
| K06 | 从 Claude Desktop / Cursor / VS Code 导入配置 | `ds-bot/src/client/connectors-page.js`（import） | ❌ | 无 | 无 〔降级 v0.6〕 |
| K07 | 能力页（团队共享 MCP/技能/能力包） | `README.zh.md:112`、`ds-bot/src/host/capabilities.js` | 🟡 | `src/capabilities.rs:15`（`capabilities::rows`，`:15-52` 只读 4 行、无开关） | 只能看，不能管 〔降级 v0.6〕 |
| K08 | 能力/能力包增删 + 按 Bot 启用 | `ds-bot/src/host/capabilities.js:21`、`client/capability-settings.js` | ❌ | grep `"save_capability"`/`list_capabilities` = 0 命中 | 无 〔降级 v0.6〕 |
| K09 | 插件宿主（第三方插件） | `ds-bot/README.md`（plugin bundle） | 🟡 | 内置插件开关已接工具注入链（`src/backend.rs` 两处注入点叠加 workspace enabledPlugins，浏览器/电脑操控子页有开关行）；插件页去掉「未接入」占位 | 第三方市场插件清单协议在闭源主程序，第三方能力走连接器（MCP） 〔降级 v0.7〕 |

## L. Office 文件（3 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| L01 | 创建/检查/修改 docx、pptx、xlsx | `ds-bot/src/office/docx.js:1`、`pptx.js`、`xlsx.js`、`ds-bot/src/office/check.js:47` | ❌ | 全仓无 office 模块；`Cargo.toml` 无相关依赖 | 完全缺失 〔降级 v0.6〕 |
| L02 | 渲染成 PNG 检查排版 | `ds-bot/src/office/png.js` | ❌ | 无 | 无 〔降级 v0.6〕 |
| L03 | Office 技能注册（SKILL.md + CLI） | `ds-bot/src/host/office-skills.js:1-14` | ❌ | 无 | 无 〔降级 v0.6〕 |

## M. 用量（4 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| M01 | 总量 / 输入 / 输出 / 缓存 | `ds-bot/src/host/usage.js:41-45` | ✅ | `src/rpi_engine.rs:1022`（`AgentEnd` 聚合 `input_tokens`/`output_tokens`/`cache_read`/`reasoning_tokens`）、`src/main.rs:16536-16574`（四张 `ui::stat_card`：回合 / 总 tokens / 输入 / 输出） | 无 |
| M02 | 按天趋势 | `ds-bot/src/client/usage-page.js` | ✅ | `usage-range-{days}` chip）、`usage_grouped(days)` 的 `by_day`） | 无 |
| M03 | 按 Bot / 按模型拆分 | `ds-bot/src/host/usage.js`（byBot/byModel） | ✅ | `usage-group-{mode}` chip）、`src/rpi_engine.rs:1786`（`GroupedUsage{by_bot, by_model, sessions}`）、`:1969`（老会话按 header `cwd` 反查名册兜底） | 无 |
| M04 | 时间范围（24h～12 月 / 日历选日） | `ds-bot/src/client/usage-page.js`（range picker） | 🟡 | `src/main.rs:16526` 六档（1/7/30/90/180/365 天）、`src/main.rs:3325`（`usage_range`） | 12 个月有档位；无日历选日（明确降级） 〔降级 v0.6〕 |

## N. 定时任务（6 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| N01 | 列表 / 启停 / 立即运行 / 删除 | `ds-bot/src/client/schedules.js`、`host/schedules.js` | ✅ | `src/main.rs:14664`（`.bg(ui_accent())` 新建 IM 提交按钮的着色，与  等五处 `!bot.is_private` 同属成员/表单路径）、`src/automations.rs:83-101` | 无 |
| N02 | 运行历史 | `ds-bot/src/host/schedules.js:122` | ✅ | `src/main.rs:10902`（`SettingsDetail::AutomationHistory` 面板）、`src/main.rs`（`AutomationHistory` 枚举分支） | 无 |
| N03 | cron / 一次性 / 相对时间规则 | `ds-bot/src/host/schedules.js:34-38` | ✅ | `src/automations.rs:218-224`；cron 解析 `crates/automation-schedule/src/cron.rs:1-49`；规则构造 `src/schedule_builder.rs:56`（`to_request`） | 无 |
| N04 | 宿主内置调度器真正执行 | `ds-bot/src/host/schedules.js` | ✅ | `src/automation_scheduler.rs:3/20/40`（20s 轮询） | 无 |
| N05 | 对话内让 Bot 建任务 | `ds-bot/src/host/schedules.js`（tool） | ✅ | `src/automation_tools.rs:71-124`（`automation_create` `:71` / `set_enabled` `:97` / `run_now` `:124`） | 无 |
| N06 | 创建时的人类友好选择器（一次 / 每天 / 每周 / 每隔） | `ds-bot/src/host/schedules.js:60-86`（selectorOf） | ✅ | `src/schedule_builder.rs:19`（`When` 四型）、`:179`（`describe` 实时预览）、UI `src/main.rs:10082`（`schedule-when-*`）、`:10086`（`once_hours` 小时后）、`:10151`（`schedule-day-*` 7 天 chip） | 无（日期用「几小时后」下拉代替日历，`to_request` 仍算绝对时间） |

## O. 密钥与审批（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| O01 | 密钥库（只写不读、`${secret:NAME}` 引用） | `ds-bot/src/host/secrets.js` | ✅ | `src/secrets.rs:28-57`、`fn settings_page_secrets`） | 无 |
| O02 | 对话内 `request_secret` 安全卡片 | `ds-bot/`（request_secret） | ✅ | `src/rpi_engine.rs`（`RequestSecretTool` 复用 QuestionGate 弹卡，值直接写 `secrets.json`，只把名字回模型；`${secret:NAME}` 引用） | 无 |
| O03 | 工具审批卡（允许一次 / 始终允许 / 拒绝） | `ds-bot/src/host/approvals.js` | ✅ | `src/rpi_engine.rs:435`（`decide`）/`:503`（`ask_permission`）、`src/main.rs:6677`（`approval_cards`）/（`approval_card_el`） | 无 |
| O04 | 三档审批策略（ask/auto/full） | `ds-bot/src/client/permission-picker.js` | ✅ | `src/main.rs:13053`（`permission_menu_overlay`）、`src/data.rs:46`（`approval_level`） | 无 |
| O05 | IM 中禁止填写密钥卡 | `ds-bot/`（im 密钥守卫） | ✅ | `src/rpi_engine.rs`：IM 会话（key="im"）不注入 `request_secret`（密钥不经 IM 传输） | 无 |

## P. 模型与服务商（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| P01 | 服务商模板 + 密钥 + baseUrl | `ds-bot/src/client/providers-page.js` | ✅ | `src/data.rs:1563`（`provider_templates`）/`add_provider_from_template`）/`set_provider_api_key`）/`set_provider_base_url`）、`src/main.rs:10584`（`添加服务商`面板）/（`provider-add`）、`:21272-21304`（`enum SettingsDetail` 全套详情面板） | 无 |
| P02 | 服务商“测试连接” | `ds-bot/src/client/providers-page.js` | ✅ | `src/main.rs:13389` 区块（`测试连接`，commit `d7ca085`） | 无 |
| P03 | 添加 / 删除模型 | `ds-bot/src/client/model-menu.js` | ✅ | `src/data.rs:1386`（`add_model`）/`:1393`（`remove_model`）、`src/main.rs`（`AddModel` 分支）/`src/main.rs:10502`（`SettingsDetail::AddModel` 面板体） | 无 |
| P04 | 按模型配置上下文窗口（check line 由窗口算） | `ds-bot/README.md`（contextWindow） | ✅ | `src/model_limits.rs:20/22`（`FALLBACK_CONTEXT_WINDOW`/`FALLBACK_MAX_TOKENS`）、`:190`（read）、`:211`（resolve，用户值 > 模板 > 兜底）、`:236`（write）、`src/rpi_engine.rs:144-147`（引用常量，不再写死）、UI `src/main.rs` | 无 |
| P05 | 每 Bot 模型 + 思考强度 | `README.zh.md:58` | ✅ | `src/data.rs:17/41`、`thinking_level_for`） | 无 |

## Q. 设置与外观（6 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| Q01 | 设置页结构（通用/模型/Bot/群/连接器/能力/浏览器/定时/用量/密钥/关于…） | `ds-bot/README.md`（Settings pages） | ✅ | `src/main.rs:21242`（`SETTINGS_PAGES`，17 页，`:21262-21294`） | 无 |
| Q02 | 设计令牌落地（4px 间距、四级字号、圆角令牌） | `docs/ui-design-spec.md`（本项目自有规范） | ✅ | `src/ui_kit.rs:26`（`S1` 起，令牌 `:26-46`）、`:129`（`Clickable::pointer`）；`scripts/ui-lint.ps1 -Enforce` 实测 **0 违规**（`cursor_pointer` 0 / 裸 `rounded(px())` 0 / 非令牌字号 0 / `rgb(` 0） | 无 |
| Q03 | 弹窗 → 右侧详情栏 | `ds-bot/src/client/details-drawer.js` | ✅ | `enum SettingsDetail`）、`docs/ui-design-spec.md:§8` | 剩余 6 个破坏性确认框，符合规范 |
| Q04 | 深浅双主题 + 皮肤 | `ds-bot/src/client/themes.js` | ✅ | `src/theme.rs:133/143`、`src/wechat_theme.rs` | 无 |
| Q05 | 多语言（DS：中/英/日/韩/德/法/西/繁） | `ds-bot/src/client/locales/*.js` | 🟡 | `src/i18n.rs:9`（`LANG`）/`:21`（`is_en`）；zh/en 共 809 条词条 | 只有两语 〔降级 v0.6〕 |
| Q06 | 托盘 / 开机自启 / 单实例 / 窗口记忆 | `app/`（桌面壳） | ✅ | `src/tray.rs`、`src/autostart.rs`、`src/instance.rs:16`、`src/window_state.rs` | 无 |

## R. 数据、测试与发布（5 项）

| ID | 能力 | DS Bot 依据 | ModeChat | 证据（ModeChat） | 差距 |
| --- | --- | --- | --- | --- | --- |
| R01 | 数据根兼容旧版（`~/.mode-bot/.mode`） | `README.zh.md:128` | ✅ | `src/data.rs:6`（数据根说明，`MODE_DATA_BASE_DIR`）/`:289`（`cli_storage_dir`）、`MODE_BOT_DATA_ROOT` 隔离 | 无 |
| R02 | 会话落盘格式 + 迁移 | `ds-bot/src/host/parts.js` | 🟡 | `src/rpi_engine.rs:327`（`sessions_root`）、`examples` 内迁移工具 | 自有 JSONL 格式，跨版本迁移靠脚本 〔降级 v0.6〕 |
| R03 | 应用内检查更新 + 自替换 | `ds-bot/src/host/update.js` | ✅ | `src/update.rs:69/212/355`（SHA-256 + 替换/回滚） | 未接签名（minisign） |
| R04 | CI：test + clippy + NSIS + Release | `ds-bot` CI | ✅ | `.github/workflows/release.yml`（Test/Clippy + ui-lint/parity/arch 三道门禁 + Package；不再写行号——这个文件随发版流程改动频繁，区间终点又会落在 `if ($LASTEXITCODE -ne 0)` 这类重复行上） | 无 |
| R05 | 自动化测试规模 | `ds-bot/test/` | ✅ | `src/*.rs` 186 个 `#[test]` + 12 个 `#[tokio::test]`、`crates/**` 331 个 `#[test]` + 50 个 `#[tokio::test]`，合计 579 个测试属性；`cargo test --workspace --locked` 实测 37 个测试目标全绿（576 passed / 0 failed / 2 ignored=需真实浏览器）。579 − 1 个 `#[cfg(unix)]` 关闭 = 578 = 576 + 2，对得上 | 无 |

---


## 汇总

| 状态 | v0.4.0 | v0.6.0 | 占比（v0.6.0） |
| --- | --- | --- | --- |
| ✅ 已有 | 73 | **102** | 75.0% |
| 🟡 半成品 | 22 | **24** | 17.6% |
| ❌ 缺失 | 41 | **10** | 7.4% |
| 合计 | 136 | 136 | 100% |

分域明细（✅/🟡/❌，合计 136）：

| 域 | 项数 | ✅/🟡/❌ | 结论 |
| --- | --- | --- | --- |
| A Bot 身份 | 13 | 10/3/0 | 记忆行与管理入口已补（A08）；A12 主卡位已补（会话列表顶部突出卡）；缺指令编辑、形象上传、换目录重建会话（降级 v0.6） |
| B Main bot | 5 | 4/1/0 | Bot 互相可见/互发消息已补（B05）；缺 Bot 自治建改 Bot（降级 v0.6） |
| C 群聊 | 15 | 12/3/0 | 阶段证据/通过/打回（C08）、群外发言（C13）已补；缺 notice、引用整段对话（降级 v0.6） |
| D 隐私 bot | 5 | 5/0/0 | 隔离闭环：会话/搜索/群 + 文件路径守卫（D05）+ 删除后归档（D04）全通 |
| E 记忆 | 13 | 11/1/1 | Bot 记忆进提示（E03）、三工具（E05）、面板（E09）、拒写凭据（E13）全通；私聊自动回顾降级 v0.6 |
| F 会话与输入 | 16 | 12/3/1 | F02 新会话入口三条（v0.5.1 Q-05：Ctrl+N `:6813`、标题栏「+」`:24445`、命令面板 `:7539`）、F06 压缩状态行（Q-01/Q-02，`fn compaction_status_row` `:21579`、行分支 `:19510`）、F10 消息复制（Q-04，`wechat_message_actions` `:19949`、复制 `:19987`）均在微信壳接通；注意 `:7539` 命令面板是**行为等价但独立**的实现，未调 `new_session_for_selected`（去重记 v0.6）；F10 的引用条因 `wechat_composer` 无引用 UI 保留〔降级 v0.6〕；FTS5 已补（search_index.rs）；另缺 show_view、语音（降级 v0.6） |
| G 浏览器 | 6 | 3/3/0 | 独立 profile/敏感审批/Cookie 文件导入/浏览器插件三形态均已接；内嵌 WebView 面板降级（wry 合成风险） |
| H 电脑操控 | 6 | 5/1/0 | 工具齐；分级（档位/应用授权+租约/永久禁止）已接；顶部指示条降级为暂停开关 |
| I 本地 Agent | 5 | 3/1/1 | ACP 齐，codex/claude 原生驱动缺（降级 v0.6） |
| J IM | 9 | 6/2/1 | IM 提问卡可用 `/question` 编号回选（J08）；钉钉 Stream 已接入待凭据联调，飞书无公开长连接协议保持空态 |
| K 连接器与能力 | 9 | 3/3/3 | 目录扩到 24（16 stdio + 8 远程 HTTP，K01）；OAuth/邮件/能力管理降级 v0.6 |
| L Office | 3 | 0/0/3 | 全缺（降级 v0.6） |
| M 用量 | 4 | 3/1/0 | 范围 1/7/30/90 + 按 Bot/按模型分组已补（M02/M03）；无日历选日（降级 v0.6） |
| N 定时任务 | 6 | 6/0/0 | 四型人类选择器 + 实时规则预览（N06）已补，写库经实测 |
| O 密钥与审批 | 5 | 5/0/0 | 缺对话内 request_secret（降级 v0.6） |
| P 模型 | 5 | 5/0/0 | 按模型配窗口/输出上限已补（P04），不再写死 128k |
| Q 设置与外观 | 6 | 5/1/0 | 令牌门禁 0 违规（Q02）；多语言仍双语（降级 v0.6） |
| R 数据/测试/发布 | 5 | 4/1/0 | 测试 579 个测试属性、37 个测试目标全绿（576 passed / 0 failed / 2 ignored，见 R05）；跨版本迁移仍靠脚本（降级 v0.6） |

### v0.5.0 交付时净关闭的 18 项（历史记录）

A08、B05、C08、C13、D04、D05、E03、E05、E09、E13、F06、G02、J08、M02、M03、N06、P04、Q02。

对应交付证据：`src/memory_tools.rs`、`src/bot_tools.rs`、`src/privacy.rs`、`src/model_limits.rs`、`src/schedule_builder.rs`（新增）；`src/backend.rs`（`Command::AskBot` / `PostGroup` / `Compaction`）、`src/rpi_engine.rs`（`system_prompt_for_bot` / `compact` / `usage_grouped`）、`src/browser_tools.rs`（按 Bot 隔离）、`crates/bot-team/src/goal.rs` + `src/group_goal.rs`（v2 迁移）、`crates/connectors-service/src/lib.rs`（远程 HTTP）、`src/im_runtime.rs`（`/question`）、`src/main.rs` + `src/ui_kit.rs`（令牌与各面板）。

### v0.6.0 的变动：A12 降到 🟡（删 classic 的必然结果）

| 项 | v0.5.1 | v0.6.0 | 依据 |
| --- | --- | --- | --- |
| A12 主卡位 | ✅ | 🟡 | `main_bot_card`（已随 classic 删除，原 `sidebar` 唯一调用）。微信壳区间内 `is_chief`/`is_main` 只出现在表单装载（`build_bot_form` `:11054`/`:11071`）与默认选中（`:11222`）里，**没有任何一处渲染成侧栏视觉位**；`wechat_list_row`（`:19003`）只画头像/名字/时间/未读点，**没有主 Bot 视觉位**。判定为真实功能损失，不是锚点问题 〔降级 v0.7：补主卡位〕 |

净变化：✅ −1、🟡 +1，合计仍 136。**这一项是「删掉的那套壳里唯一没有等价物、也不在本版补」的能力**；其余 classic 独有能力（欢迎区、运行状态面板）已在 v0.6 搬进/接回微信壳，见 `docs/prd-0.6.md` §2.2、§5 的 O-1 / O-2。

### v0.5.1 的变动：两项降级、一项证据补全

| 项 | v0.5.0 | v0.5.1 | 依据 |
| --- | --- | --- | --- |
| F06 压缩 / checkpoint | ✅ | 🟡 | 微信壳压缩状态行已补（Q-01/Q-02），但 checkpoint/handoff 语义仍未做，v0.5.0 记全 ✅ 是过宽；证据见 F06 行 |
| F10 Markdown | ✅ | 🟡 | 微信壳已补「复制 / 复制代码」（Q-04），但引用条仍缺（`wechat_composer` 无引用 UI），v0.5.0 记全 ✅ 是过宽；证据见 F10 行 |
| F02 新会话 | ✅ | ✅ | **状态不变**。v0.5.0 只引 1 处（`:19144`）且「差距」列写「无」，即当时判宽了（微信壳其实没有入口）；v0.5.1 由 Q-05 真实补齐三条入口，证据从 1 处扩到 3 处，✅ 靠真实修复撑住 |

三项净变化：✅ −2、🟡 +2，合计仍 136，增减只来自 F06 与 F10。**这不是功能回退**：三项在默认外壳下的实际能力都比 v0.5.0 强，降级原因是复验改用「默认外壳为主判据」后，把 v0.5.0 记宽了的项按真实完成度重算。

### 结论（产品视角，v0.6.0 之后的剩余差距）

1. **记忆闭环已通**：Bot 现在有自己的记忆、能主动记/查/忘、下次进提示，面板可管（A08/E03/E05/E09）。剩余是「私聊自动回顾」（E07，v0.6）。
2. **团队协作最小闭环已通**：`list_bots` / `message_bot` / `post_to_group` 可用，隐私 Bot 不可被直连（B05/C13）。剩余是「Main bot 自己建改 Bot」（B02，v0.6）。
3. **上下文可见可配已通**：压缩有状态行、可手动触发，模型窗口按「用户值 > 模板 > 兜底常量」解析（F06/P04）。剩余是 checkpoint/handoff 语义（v0.6）。
4. **隐私承诺与实现对齐**：删除后目录继续受保护并可解除，越界路径即使 `full` 档也弹卡（D04/D05）。
5. **最大的剩余空缺在 H（电脑操控分级/租约/接管）与 L（Office 全无）**，两块都要独立立项，不在 v0.5 范围。
6. **IM 仍是两条线真跑**：微信/企微可用，飞书/钉钉已做成明确空态（不放假入口），运行时实现排 v0.6。
7. **UI 债清零**：`ui-lint.ps1 -Enforce` 四项 0 违规（`cursor_pointer` 现存量 9 处全在 `src/ui_kit.rs`；`text_size(px(` 332 处，非令牌字面量 0；`rounded(px(` 188 处，字面量 0；`rgb(` 22 处，白名单外 0），`docs/ui-design-spec.md` §10/§11 债务计数同步归零。**旧口径是「命中即违规」的 grep 数字（139/44/5），不能用来判定这条 AC，正确判据见 spec §11.1。**

















