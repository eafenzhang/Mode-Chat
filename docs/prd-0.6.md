# ModeChat v0.6.0 产品需求文档（PRD）

> 版本：v0.6.0｜作者：产品｜日期：2026-11-26
> 基线：`c86c899`（`src/main.rs` 29,919 行）
> 配套技术方案：`docs/ui-shell-removal-plan.md`（下称「删除方案」）。**本文 §2/§3/§5 是它引用的章节号**，它 §8 反向引用本文。
> 输入：`docs/testing/0.5.1-shell-parity-diff.md`（14 项两壳差分）、`docs/testing/0.5.1-ui-review.md`（U-01…U-06）、用户决定「删除 DS 风格 UI，仅保留微信风格 UI，基于微信风格优化」。

---

## 1. 背景与目标

### 1.1 为什么现在必须删

仓库里有两套**各自手写**的渲染代码，不是同一组件换皮：

```
wechat_shell  :25961   ← 默认，微信风四栏
classic_shell :25925   ← 只有 BOT_PILOT_CLASSIC_UI=1 才走（:3830）
shell()       :25917   if self.wecom_mode { wechat } else { classic }
```

`0.5.1-shell-parity-diff.md` 量出 14 项不等价（A 方向 5 项、B 方向 6 项、C 方向 3 项）。这个数字本身就是证据：**同一个能力永远只接进一边**。v0.5.1 修的 Q-01/Q-02/Q-04/Q-05 四处，全是这个病的发作，不是四次独立的事故。

用户已明确决定：**删掉 DS 风格（classic 三栏）外壳，只保留微信风外壳一套，然后基于微信风做一轮优化。**

### 1.2 目标（三句话）

1. 仓库里只有一套外壳（微信风），classic 不留可回退开关，删干净。
2. 删除过程中**不丢任何一个用户可达的功能**；被删除代码里唯一比微信壳好的地方（空会话欢迎区）搬过来。
3. 在微信壳上做一轮 UI 与交互优化：对齐间距、补交互反馈、修长文本兜底。视觉语言必须是微信风，不动四栏骨架。

### 1.3 这不是什么

- 不是重做 UI。骨架、配色、图标体系、信息架构都不动。
- 不是补 v0.5 的欠债清单。除「引用条」外，v0.6 不新增任何双击可达的新入口。
- 不是重命名工程。`wecom_*` → `wechat_*` 已经由 `d9c07e9` / `85aa39a` / `c86c899` 三个提交做完，本版不再改名字（除删除导致的必然清理）。

---

## 2. 范围与不做

### 2.1 删除范围（D-CUT 一个提交，约 3,033 行：main.rs 3,013 + theme.rs 20）

逐函数的「删/留」判定与调用点证据见**删除方案 §3.1 / §3.2 / §3.4 / §4**，本文不重复列表。这里只定口径：

| 批 | 内容 | 量 | 判定依据 |
| --- | --- | --- | --- |
| 第一批 | `classic_shell`、`titlebar`、`window_button`、`sidebar`、`main_bot_card`、`unread_badge`、`chat_header`、`transcript`、`composer`、`right_panel`、`status_color`、`control_card`、`chat_status`、`two_line`、`text_block` + 16 个辅助 free fn | 约 2,772 行（含上方注释）/ 2,554 行（从 `fn` 行起） | 唯一调用方都在将被删除的 classic 函数内 → 删除方案 §3 |
| 第二批 | `settings_overlay`、`theme_picker`、`wecom_mode` 字段、`BOT_PILOT_CLASSIC_UI` 环境变量、`SIDEBAR_WIDTH`、`BUBBLE_MAX_WIDTH`、`active_tab` 字段 | 约 241 行 | 删完第一批才变成死代码；`wecom_mode` 的两处反向条件必须同批删，否则编译不过 → 删除方案 §6.1 |
| `theme.rs` 连带 | `set_light`、`hairline`、`ink_line`、`subtle_hover`、`accent_border`、`chief`、`bubble_user`、`bubble_assistant`、`member_color`、`shadow_1` + `RADIUS_COMPOSER`/`RADIUS_COMPOSER_MULTI` 两常量 + `shadow_1` 断言 | 未计入上面两个数 | 语料全在 classic 里；`theme.rs` 无 `allow(dead_code)`，不删则 `clippy -D warnings` 必红 |

**五处易漏项（`@前端` 交叉验证发现，实测调用点已复核）**：

```
control_card  def=8698   ← right_panel@20532                          166 行
chat_status   def=8978   ← status_color@18683、right_panel@20523        7 行
status_color  def=18683  ← right_panel@20803                            7 行
two_line      def=23307  ← sidebar@18859/18897                         37 行
text_block    def=24666  ← group_details@21430/21434                    8 行
```

`status_color` 与 `chat_status` 是成对的：`status_color` 唯一调用在 `right_panel@20803`，`chat_status` 只被 `status_color@18683` 和 `right_panel@20523` 用。**这五个函数不删，`clippy` 会在第一批结束时直接失败。**

**`theme::set_light` 的陷阱**：`theme::set_light` 已随 D-CUT 删除，删除前唯一调用在 classic `theme_picker`；但 **`wechat_theme::set_light` 是另一个函数**，`src/main.rs` 里两处（`theme_picker` 的浅色分支与启动时读偏好）都在调用，**必须保留**；`src/theme.rs:138` 的 `set_light_runtime` 与之成对，同样保留。同理 `RADIUS_BUBBLE` 必须留（`skeleton_rows` 微信壳空态在用，常量在 `src/theme.rs:499`）。

**D1 与 D2 必须合成一个提交**：`theme::set_light` / `theme::accent_border` 只有 `theme_picker` 用，而 `theme_picker` 在第二批；拆成两次提交，第一批结束时 `clippy` 一定报 unused 而失败。「删除批 vs 优化批」的分界线仍然有效，只是删除批内部不再拆。

**行数预期改成区间**：删完 `main.rs` 落在 **26,850–27,350 行**（含 `theme.rs` 的清理与运行状态接回 +10）。拿 `27,327` 当精确值会对不上，因为统计口径（含不含注释行）差 12 行以上。

**`theme.rs` 不按行数验收，改按两向 grep**（删除方案 §6.4 的 609 是「只算函数体 42 行」的口径；实际切割会把每个死函数上方的 `///` 文档注释一并收走，边执行边核必然对不上）。判据改为：

```powershell
# 该删的全部为 0
Select-String -Path src\theme.rs -Pattern 'fn set_light\(|fn hairline|fn ink_line|fn subtle_hover|fn accent_border|fn chief|fn bubble_user\(|fn bubble_assistant|fn member_color\(|fn shadow_1\(|RADIUS_COMPOSER|shadow_1\(\)\.len\(\)'
# 该留的全部 ≥1
Select-String -Path src\theme.rs -Pattern 'fn shadow_2|fn shadow_3|fn shadow_4|fn ring_4|fn member_color_index|fn bubble_user_ink|fn set_light_runtime|RADIUS_BUBBLE'
```

实测（D-CUT 工作区）：该删 12 个模式 **全 0**，该留 8 个符号 **全 1**，`theme.rs` 从 651 降到 **597**（删 54 行 = 42 行函数体 + 5 行文档注释 + 7 行空行）。**597 就是通过，不再要求 609。**

**必须保留**（被微信壳 / 设置页 / 弹层共用，删一个就编译失败或静默丢功能）：`kv_row`、`short_id`、`count_members`、`visible_group_members`、`count_bots`、`is_group_prompt`、`model_label`、`notify_background`、`close_or_hide`、`approval_reason_view`、`approval_note`、`member_label`、`selected_ids`、`overlay_center`、`dialog_card`、`switch`、`dialog_field`、`compose_with_quote`、`user_text_with_mentions`、`fenced_code`、`history_attachment_view`、`attachment_chips`、`error_bar`、`current_engine_is_mode`、`attachments_allowed`、`stage_attachment_paths`、`stage_image_bytes`、`start_screenshot_capture`、`toggle_voice_typing`、`remove_attachment`、`empty_state`、`skeleton_rows`、`kit_markdown_el`、`compaction_status_row`、`tool_copy_button`、`tool_actions_row`、`group_memory_section`。完整表 + 微信壳侧调用点见**删除方案 §4**。

### 2.2 两处必须由本文拍板的判定

**① A12「主卡位」：判定从 ✅ 降为 🟡，矩阵汇总 89/20/27 → 88/21/27。**

实测依据：`main_bot_card`（已随 D-CUT 删除）原只被 `sidebar` 调用；微信壳区间 `25799-29000` 内 `is_chief` / `is_main` / `chief` **0 命中**；侧栏行 `wechat_list_row`（`src/main.rs:18899`）只画头像、名字、时间、未读点，没有「这是 Main bot」的视觉位。

所以删除后**用户看不到「谁是主 Bot」的突出标识**——这是真实的功能损失，不是锚点问题。判定 🟡，锚点落到 `src/main.rs:22891`（`fn wechat_nav`），差距列写「微信壳只有平铺列表，无主卡位；补主卡位记 v0.7」。

同步改动：`docs/parity-matrix.md` 的汇总说明、A12 行、汇总表、A 域分域明细、文末「v0.5.1 的变动」表补一行。

**② 运行状态面板（D-CUT 前 109 行、第 9318–9426 行；原函数 `chat_detail_panel` 已随本次改造删除）：不再单独渲染卡片，改为高亮右栏基础卡。** 理由是右栏基础卡已涵盖它的全部内容（引擎/模型/会话 ID/消息数/排队/待处理拆分/最近一轮/工作目录），再画一张就是同一数据出现两次；高亮挂在基础卡上、id 仍是 `chat-detail-run`，保证「面板本来就开着」时点击也有可见变化。

现状是既有缺陷：`chat_detail` 只被 `chat_detail_panel` 内两处读（D-CUT 前第 9325 / 9420 行）、被 `DetailMenuAction::RunStatus` 写（D-CUT 前第 17460 行，微信壳的「⋯」也走这里），D-CUT 前唯一渲染点是 classic `right_panel`。也就是说**微信壳里点「⋯ → 运行状态」现在什么都不显示**。删 classic 会让它彻底没有渲染点。

判定：**接 10 行进 `chat_info_panel`**（删除方案 §6.2 选项 1）。理由两条：删掉它是功能回退（「⋯」里少一项）；保持现状则是把已知缺陷固化进新架构。这 10 行是「接回已有实现」，不构成新功能，**计入本版范围**。

### 2.3 明确不做（买断边界，验收时按此判断「超范围」）

| 不做项 | 理由 | 去处 |
| --- | --- | --- |
| **改微信壳四栏骨架**（右侧详情改抽屉、导航变浮层等） | 骨架改动会让「回归验证」变成「重新验收全部界面」 | v0.7，需单独立项 |
| **引用条**（「引用 张三：…」+ 清除） | classic 删除后引用 UI 已无实现；在微信壳新建引用交互 = 新功能，超出「优化」 | v0.7；矩阵 F10 保持 🟡，差距列改成「引用条待 v0.7，本版删掉了旧实现」 |
| **补主卡位**（A12） | 同上，属于新增视觉位 | v0.7 |
| **U-02 键盘焦点链**（`track_focus` 14 / `on_click` 300） | 焦点顺序是交互架构，需重新设计 Tab 序列 | v0.7 |
| **U-05 模型页错误语义** | 需先定「首次使用」与「数据损坏」的判别，不是 UI 活 | v0.7 |
| **修 14 项两壳差分** | A 方向 5 项、C 方向 3 项的源在 classic 里，删掉即消失；B 方向 6 项是微信壳本来的优势 | 随 classic 删除而作废（删除方案 §8），**不写成「14 项通过」** |
| **重命名剩余 `wecom_*`**（24 个、88 处，企微渠道协议层） | 与企业微信这个 IM 渠道本身同名是有意的，改名会与「微信 UI 皮肤」混淆 | 不做，永久保留 |
| **新增任何双击可达入口** | 「优化」不等于「加功能」 | v0.7 |

### 2.4 删除批暴露的三处功能位缺失与处置（本版拍板）

D-CUT 删完 classic 后 `clippy -D warnings` 报出 20 条 dead code，其中三处**不是单纯死代码，而是「原来只有 classic 有入口」的功能位**。逐项拍板：

| # | 功能位 | 现状证据 | 判定 | 计入 |
| --- | --- | --- | --- | --- |
| 1 | **会话重命名**（`SettingsDetail::RenameSession`） | HEAD 里唯一触发点是 classic `right_panel` 的 `hist-rename-{id}`「改名」按钮（`c86c899:21286-21309`）；微信壳历史会话行（`panel-history-{target}`）只有切换，没有改名。对话框、`sessions.set_title`、state.json 落盘全都还在 | **接回**。微信壳 `chat_info_panel` 历史会话行加一个「改名」按钮触发同一个 `SettingsDetail::RenameSession`，复用现成对话框与落盘逻辑，约 12 行。这是「给已存在功能补回丢失的入口」，不是新功能 | D3，与 O-2 同批 |
| 2 | **浏览器「打开 URL / 最近记录 / 已装浏览器」**（`browser_panel_section`、`record_browser_open`、`browser_recents`、`browser_url_input`、`save/load_browser_recents`、`installed_browsers`） | HEAD 里只在 classic `control_card(kind==1)` 内渲染（`c86c899:8861`）；微信壳浏览器页（`settings_page_browser`）只有「打开 / 关闭 / 页面截图 + 使用范围」 | **不接回，明确记为功能回退**。接回等于在设置页新增一整块 URL 输入 + 最近列表 + 已装浏览器列表，属于新功能，超出「优化」边界。矩阵 **G04 差距列必须改写**（现在写的是「只有用系统浏览器打开 URL + 最近 + 已装浏览器列表」，v0.6.0 起这句不再成立），保持 ❌，留给 v0.7 的内嵌浏览器面板 | 随 D-CUT 删除；矩阵改判归 D4 |
| 3 | **插件开关二次确认**（`PluginSwitchConfirm`、`apply_plugin_switch`、`open_plugin_confirm_dialog`、`data::set_plugin_switch`） | HEAD 里只在 classic `control_card` 的插件分页触发；微信壳插件页明写「暂未接入插件宿主」。`data::set_plugin_switch` 在框架层无消费者（只有 `src/data.rs:2676` 的单测） | **UI 删，数据层留**。`data.rs` 的 `set_plugin_switch` + 它的单测保留，加 `#[allow(dead_code)]`（同文件 `workspace_plugins:217` 已有同款先例）；插件宿主 K09 是 ❌ 降级 v0.6，v0.7 接宿主时直接用，删了要重写，且删测试会压低测试数 | 随 D-CUT 删除 UI；`data.rs` 只加注解 |

**纯死代码（无功能含义，直接删，不留记录）**：`avatar.rs` 的 `deg` / `shell` / `bot_avatar` / `channel_avatar` / `unread_dot`（`bot_avatar_rect` 保留，微信壳在用）、`sidebar_status_badge`、`nav_row`、`NavRowFocus`、`tool_state_color`、`settings_nav`（微信壳侧栏是 `wechat_nav`，导航一致性由 `settings_page_in_nav` 单测覆盖）。字段级 dead（`browser_url_input`、`PluginSwitchConfirm` 等）随所属结构体一起清；**`Bot.description` / `last_session_id` 不在其列**——它们由 `src/data.rs:2336` / 第 2344 行 序列化读，clippy 若仍报，报的是别的结构体，按报错的具体类型处理，不许顺手把 `data::Bot` 的字段删掉（会破坏 state.json 兼容）。

**本节的验收口径**：`cargo clippy --all-targets --locked -- -D warnings` **0 error**；`cargo test` 总数 **≥575**（`composer_radius` 单测随函数删除是唯一允许的下降，其余只许不变或上升）。

---

## 3. 可执行验收（删除批）

每条都能用一条命令或一张截图判定。**行号一律指 `c86c899`**，删除后按新行号复核 `grep` 结果即可。

> **本节的判定原则（`7f430ce` 起生效，比任何单条 AC 都重要）**：
>
> **① 判据写成 grep 的项，必须同时给一张「能证伪」的截图。** grep 只证明「有代码」，不证明「用户能走到、且看到的是对的」。
>
> 出处是这两次真实事故：
> - `f904043` 补了 id `chat-detail-run`，grep 判据 1 命中，**判过**；第一次点开截图（`252`）才看到右栏三层嵌套：重复标题、死「返回」、右缘裁 44px。
> - `SettingsDetail::RenameSession` 枚举与弹窗一直都在（`grep = 1`），**但触发点是 0**，用户在微信壳里根本走不到改名。
>
> **② 能证伪的截图 = 拍出来就能看出「没做到」的截图。** 「卡片出现了」不算证伪条件；「会话」行值完整显示、页面上只有一处标题、没有多余的按钮，才是。
>
> **③ 涉及「用户是否能走到」的功能，截图必须包含走到的路径**（点击前后各一张，或一张带 id/焦点的状态图），不能只截最终静态画面。

### 3.1 删干净（grep 判定，全部必须为 0）

```powershell
# 1) classic 骨架与三栏函数全部消失
Select-String -Path src\main.rs -Pattern 'fn classic_shell|fn titlebar|fn window_button|fn sidebar\b|fn main_bot_card|fn unread_badge|fn chat_header|fn transcript\b|fn composer\b|fn right_panel|fn status_color'
# 期望：(no output)

# 2) 删除批的辅助函数消失
Select-String -Path src\main.rs -Pattern 'fn bot_details|fn group_details|fn group_placeholder|fn responder_label|fn short_session|fn composer_radius|fn icon_button|fn day_separator|fn msg_time|fn load_earlier_row|fn message_row|fn quote_button|fn copy_button|fn message_actions|fn message_attachments|fn tool_card'
# 删除前命中 16 个函数（实测 17 行：第 17 行是 composer_radius 的单测函数
#   `fn composer_radius_switches_on_multiline_or_attachments` :29541，它随函数一起删）
# 期望删除后：(no output)

# 3) 开关与死代码消失
Select-String -Path src\*.rs -Pattern 'BOT_PILOT_CLASSIC_UI|wecom_mode|SIDEBAR_WIDTH'
# 期望：(no output)
```

### 3.2 必须保留（grep 判定，全部必须 ≥1）

```powershell
Select-String -Path src\main.rs -Pattern 'fn kv_row|fn short_id|fn count_members|fn visible_group_members|fn is_group_prompt|fn compose_with_quote|fn fenced_code|fn overlay_center|fn dialog_card|fn switch|fn empty_state|fn skeleton_rows|fn kit_markdown_el|fn compaction_status_row|fn tool_actions_row'
# 期望：15 个符号全部命中。实测基线（c86c899）逐符号命中数：
#   kv_row 1  short_id 1  count_members 1  visible_group_members 1  is_group_prompt 1
#   compose_with_quote 1  fenced_code 1  overlay_center 1  dialog_card 1
#   switch 2（另一条是 switch_to_history_session:27544，同一前缀，删后仍应 ≥1）
#   empty_state 2（另一条是 empty_state_badged:24682，同一前缀）
#   skeleton_rows 1  kit_markdown_el 1  compaction_status_row 1  tool_actions_row 1
# 判据：删除后逐符号命中数不得为 0；switch / empty_state 允许从 2 降到 1。
```

### 3.3 单分支（grep + 代码审读）

**D-CUT 后 `fn shell(` 这个函数已整个删除**，`render` 直接调用 `self.wechat_shell(window, cx)`（`render` 内唯一一条该调用；**这里刻意不写行号**，O-1/O-3/O-4 每次改 `main.rs` 都会把它推下去，写死行号必然过期）。所以单分支的判据改为：

```powershell
# ① 不存在任何名为 shell 的分支函数
Select-String -Path src\main.rs -Pattern '^\s+fn shell\('        # 期望 0
# ② render 里直接进微信壳、没有 if/else 包一层
Select-String -Path src\main.rs -Pattern 'self\.wechat_shell\(window, cx\)'   # 期望 ≥1
# ③ 壳名唯一
Select-String -Path src\*.rs -Pattern 'fn (classic_shell|wecom_shell)'      # 期望 0
```

判据：①③ 为 0 且 ② ≥1。**不再要求 `fn shell(` 存在**——留一个空的转发函数反而是多余的间接层。

### 3.4 欢迎区搬进微信壳（grep + 截图，两条都要）

```powershell
Select-String -Path src\main.rs -Pattern 'wechat-welcome-suggestion-'
# 期望：命中 1 处（生成 id 的 format!），三条 chip 由它循环产出
```

截图（四张，均需在隔离根下拍）：
- 深色主题 · 单 Bot：新开一个 Bot 会话，0 条消息 → 有头像、欢迎语、三条建议 chip，点 chip 后文字进输入框且聚焦。
- 浅色主题 · 单 Bot：同上。
- 深色主题 · 群聊：新开一个群，0 条消息 → 九宫格成员头像 + 「群名 · N 人」+ 成员名单 + 「成员：…。发消息开始群聊对话；用 @成员名 点名。」
- 浅色主题 · 群聊：同上。

**实测映射（当前 `eb941a2`，`docs/testing/shots/`）**：

| 截图 | 对应判据 | 说明 |
| --- | --- | --- |
| `250-o1-chat-dark` / `254-o1-chat-light` | §3.4 单聊 | 头像、欢迎语、三条 chip；浅色那张还验了“点 chip 后填入输入框” |
| `251-o1-group-dark` / `253-o1-group-light` | §3.4 群聊 | 九宫格 + 「群名 · N 位成员」+ 成员名单 |
| `255-o1-chip-clicked-light` | §3.4 交互 | 点 chip 后输入框 = 建议文字、发送钮变绿 |
| `260-o2-3a-group-title-full` | §3.5 **3a** | 群聊选中 + 右栏打开，标题「前端走查群」完整、badge「就绪」，左栏选中项可见 |
| `261-o2-highlight-with-title` | §3.5 **判据 1 + 4a** | 「运行状态」标题 + 基础卡绿描边（判据 1 由 0 处变 1 处） |
| `257-o2-4b-after-collapse-reopen` | §3.5 **4b** | 折叠再展开后描边消失 |
| `258-o2-4c-switched-away` | §3.5 **4c 第二步** | 切到 bot 13，无描边 |
| `262-o2-4c-real-switchback` | §3.5 **4c 第三步** | 真实切回 bot 12（12:32:55，晚于 `258` 的 10:31:48），无描边；SHA256 `B5F37295… ≠ 257 的 DF68AE34…` |

**三张已作废、不要引用**：

- `252-o2-run-status-dark` —— 拍的是旧三层嵌套方案（`ui::DetailPanel` 外壳：重复标题、死「返回」、右缘裁 44px），`dda719d` 已否决该方案，由 `261` 取代；
- `259-o2-4c-switched-back` —— **已删除**。它与 `257` 的 SHA256 完全相同（`DF68AE34…`）、时间戳同为 10:28:31，是另存拷贝，不能作为 4c 第三步的证据；`eb941a2` 用真实拍摄的 `262` 替换。
- `256-o2-4a-highlight.png` —— 摄于 10:27:20，早于 `6930a4a`（补回「运行状态」标题）。它只有绿描边、**没有标题**，因此证明不了判据 1，由 `261` 取代（`261` 摄于 12:27:29，exe 编译于 12:16:32）。它唯一还有价值的用途是「4a 只加描边不加标题」这一缺陷形态的前后对照。

**截图前必须确认 exe 编译时间晚于最后一个 `src/` 提交**——`0706ebb` 之前编的二进制截群聊只会得到白屏，@前端 实际踩过（02:57:55 的 exe 对 03:01:04 的提交）；`eb941a2` 三张图的 exe 编译于 12:16:32，晚于 `6930a4a`。

**群聊空态是 D-CUT 引出的功能回退，不是「本来就没有」**：classic 的 `group_placeholder`（`c93ef4a~1` 的第 21439–21460 行）随 classic 一起被删了，它调用 `empty_state_badged(Some(channel_avatar(...)), "{group.name} · {count_members}", "成员：…。发消息开始群聊对话；用 @成员名 点名。")`。删掉后微信壳群聊 0 条消息只剩裸 `div()`。**已补齐**：`0f9b4b5` 修单聊（`wechat_welcome`），`0706ebb` 补群聊（`wechat_group_welcome`，id `wechat-group-welcome`，九宫格 + 「群名 · N 位成员」+ 成员名单）。验收仍**单聊、群聊分开记**，四张截图缺一不可。

**本条属 D3，不属于 D-CUT 的验收**。classic `transcript` 删除后，「0 条消息且 `ready=true`」这条分支在微信壳里返回裸 `div()`（微信壳自身代码，与 classic 无关），屏幕上就是一片空白。**D-CUT 提交时这块空白是预期状态，按空白判阶段 2 失败是误判；D3 做完仍空白才算失败。** 两个阶段的判据分开：

> 行号口径：`if chat.messages.is_empty()` 在 D-CUT 前是第 27783 行、裸 `return div()` 在 27795；D-CUT 后分别是第 24542 行与第 24554 行（`c93ef4a` 起）。**这几个是历史/现状描述，不写成「文件:行号」的锚点形式**——那样会被 parity 当成当前 HEAD 的锚点去校验，而 27783 在 HEAD 上已不存在。
>
> **`0f9b4b5` 之后**，这条分支已改调 `self.wechat_welcome(bot.as_ref(), group.as_ref(), cx)`（`0706ebb` 起群聊也接上了），欢迎区实现在 `wechat_welcome`、`wechat_group_welcome` 两个函数里，chip id 前缀 `wechat-welcome-suggestion-`。所以本节上面那段「D-CUT 后是裸 div」描述的是 `c93ef4a`~`0f9b4b4` 这段时间的状态。

| 阶段 | 判据 | 空会话截图 |
| --- | --- | --- |
| D-CUT（阶段 2） | §3.1 / §3.2 / §3.3 的 grep + §3.6 行数 + §3.7 六道门禁 + 测试数 ≥575 | 不拍；拍了必然是空白，不作判据 |
| D3（阶段 3） | 本节 grep（`wechat-welcome-suggestion-` ≥1）+ 上面四张截图（单聊深浅 ×2、群聊深浅 ×2）+ **改名实机图三张**（见下） | 必拍；任一路径空白即失败 |

**改名实机图（D3 第三项，单独立判据）**：`e659eb7` 已把入口接回（`chat-info-rename` = 1、`SettingsDetail::RenameSession` 构造点 2 处），但**没有实机截图就还不算闭环**。必须三张、同一个实例连续拍，顺序不能倒：

1. **点之前**——右栏「最近会话」那一行，标题是旧名，可见「改名」按钮（左栏选中项也要在画面里，否则看不出改的是哪一行）；
2. **弹窗**——点「改名」后出现「重命名会话」弹窗，输入框里预填的是旧名（证明走的是 `SettingsDetail::RenameSession`，不是别的入口）；
3. **改完之后**——名字改成新值并保存，**那一行文案已经变成新名**，且右栏没有残留绿边。

判据可证伪：第 3 张若那行仍是旧名，说明只弹了窗没落盘（`sessions.set_title` / `sessions.save()` 链路断了）；若三张同 SHA256，就是复用图。这三张**不进**上面「四张互不相同」的集合，各自单独比对。

### 3.5 运行状态面板接回（grep + 截图）

```powershell
Select-String -Path src\main.rs -Pattern 'chat-detail-run'          # 期望 1（高亮容器的 id）
Select-String -Path src\main.rs -Pattern 'chat-detail-close'        # 期望 0（关闭按钮已并入右上角折叠）
Select-String -Path src\main.rs -Pattern 'detail-back'              # 期望 0（DetailPanel 的「返回」不该出现）
Select-String -Path src\main.rs -Pattern 'fn chat_detail_panel'     # 期望 0（函数已删除）
```

> 两条容易误判的期望值：
> - `chat-detail-close` 期望 **0**，不是 1：`dda719d` 决定删掉那个独立关闭按钮，因为基础卡右上角已有 `chat-info-collapse` 折叠按钮。前一版 PRD 写「期望 1 处」与决定自相矛盾，按判据跑必然不过。
> - `fn chat_detail_panel` 必须带 `fn ` 前缀。只写 `chat_detail_panel` 会命中 `:8845` 那句说明「`chat_detail_panel` 已删除」的注释，判据就红了——**判据要排除「解释自己已删除」的注释**。

截图（**判据四条，缺一条不算过**）：微信壳「⋯ → 运行状态」→ 右栏打开，基础卡带高亮
1. **有一处「运行状态」标题**（在基础卡上方，随高亮一起出现），且**不再有第二张统计卡**（`fn chat_detail_panel` 不得存在）。
   > 这一条前一版写成「只有一处」，实测是 **0 处**：`727a626` 删第二张卡时，外层 `ui::section("运行状态")` 也一起删了。结果是用户点「运行状态」→ 右栏只多一圈绿边、**没有任何文字说明他在看什么**。「点了有反应」要成立，**反应还得能读出来**。所以这里不是把判据放宽到 0，而是**把标题加回来**——只在高亮时渲染，正常状态不占版面。
2. **没有「返回」按钮**（`detail-back` 属 `DetailPanel::render`，接错层级时才会出现），也**没有第二个关闭按钮**（折叠按钮 `chat-info-collapse` 已存在）；
3. **宽度不再被裁 44px**，分两条验（`727a626` 删掉第二张卡后，原「会话」行**已不存在**，旧判据无法验证，改写如下）：
   - **3a** 右栏顶部标题行**完整显示会话名**（例 `前端走查群` 不出现半个字被切），右侧 badge 显示「就绪」；
   - **3b** 「会话 ID」行**按设计截断**（`truncate()` + 省略号，36 字符长 ID 在 296px 内放不下），判据是**右侧没有整块被裁**、省略号出现在文字末尾而不是中途消失。
4. **高亮是瞬时状态，三个动作分别验**（缺一条不算过）：
   - **4a** 右栏已打开时点「⋯ → 运行状态」→ 基础卡出现 accent 描边；
   - **4b** 折叠右栏再展开 → 描边消失（`set_right_panel(false)` 必须清 `chat_detail`）；
   - **4c** 切到另一个 **Bot**（或群）再切回 → 描边不回来。**判据按「换 Bot」验，不按「换会话」验**——`RunStatus` 的 key 是 bot 级 `bot:<id>`，换 session 时 key 不变，测出来的「不亮」是假象。

**四张截图必须互不相同（对「截图证据」的硬要求）**：`4a` 一张、`4b` 一张、`4c` 的两步各一张，共 4 张。**每张的 SHA256 必须互不相同**，并且 `4c` 切回那张的时间戳必须晚于切走那张。

> 出处：`259-o2-4c-switched-back.png` 与 `257-o2-4b-after-collapse-reopen.png` 曾经是**字节级完全相同**（同一 SHA256、同一时间戳 10:28:31），还比「切走」那张 `258` 早 3 分钟。也就是说 4c 的第三步根本没有截图，只有一张复用图。**验收动作里必须写「比对 SHA256」，不能只看文件名齐不齐**——文件名齐是最容易伪造的证据形态。

**3a 的截图必须让左栏选中项可见**：判据是「右栏顶部标题行完整显示会话名」，所以要拍**群聊被选中、右栏同时打开**的状态。只有右栏、左栏看不到选中项时，无法证明标题显示的是哪一个会话的名字。

**4b / 4c 的实现要求（`e659eb7` 已补齐，实测见下）**：`chat_detail` 是 `Option<SettingsDetail>`，`RunStatus` 的 key 是 **bot 级** `bot:<id>`（`bot_key()`，`:16337`），**不含 session_id**。所以「切会话」时 key 不变，`matches!` 一直命中——这一条不看代码会想当然以为「切走就自然不亮」。

必须清空 `chat_detail` 的 **4 个入口**（当前 `chat_detail = None` 实测 4 处，全部命中）：

| 入口 | 行 | 覆盖判据 |
| --- | --- | --- |
| `set_right_panel(false)` | `11255` | 4b 折叠 |
| `select_bot` | `3884` | 4c 切 Bot |
| `select_group` | `4145` | 4c 切群 |
| 改名按钮（切设置页前） | `24118` | 防止从设置返回时带出旧高亮 |

**为什么必须清**：高亮表达的是「用户刚刚点了运行状态、正在看这一份」。留着它，用户下次展开右栏会看到一个说不清来源的描边；切回旧会话又会亮起来，看起来像「这个会话有某种状态」。**瞬时 UI 状态不能写成持久字段的副作用。**

> `switch_to_history_session`（`24243`）与 `new_session_for_selected`（`6830`）**不需要**单独清：前者接管「最近会话」行切换，后者是 Ctrl+N 新建会话，两者的 `RunStatus` key 都是 `bot:<id>`，而它们不改变 Bot 选择。**但如果哪天 `RunStatus` 的 key 改成带 session_id，这两处就必须补清**——写在这里，作为将来改 key 时的检查点。

**本版拍板：删掉第二张卡，改成给基础卡加高亮（`8c74a19` 那版「只留三行」已撤回）**

原因是查代码时发现的：基础卡所在的右栏**已经把全部数据都渲染了一遍**，`chat_detail_panel` 无论怎么删都删不出增量：

| 数据 | 基础卡（`chat_info_panel`） | `chat_detail_panel` 原第二张卡 |
| --- | --- | --- |
| 引擎 / 模型 / 会话 ID / 消息数 / 排队 / 最近一轮 | 有（基础卡 7 行） | 重复 |
| 工作目录 | 有（有 Bot 时） | 重复 |
| 状态 | 有（标题右侧 badge） | 重复 |
| 待处理拆分 | **已有**：`// 1) 待处理：审批 / 提问 / 表单` 三行 `stat_row` | 重复（`待审批/待回答/待填表单`） |
| 待办 / 排队明细 / 运行参数 / 本轮改动 / 会话用量 | 有（第 2–6 分区） | 无 |
| 错误 | 有（`error_bar`，带重试） | 无 |

所以第二张卡没有一行属于增量信息。「只留三行」会留下第二份「审批/提问/表单」，只是把重复从 7 行减到 3 行，**问题性质没变**。

**改为高亮后的形态**：

- `DetailMenuAction::RunStatus` 仍然设 `chat_detail = Some(RunStatus { key })` 并强制打开右栏（`16871` 的现有逻辑不变），但**不再驱动第二张卡**；
- 基础卡在 `chat_detail` 命中当前会话时加 accent 描边，并挂 id `chat-detail-run`（判据靠它）；
- 关右栏（`chat-info-collapse`）或切换会话时清空 `chat_detail`，高亮随之消失；
- `chat_detail_panel` 从「渲染一张卡」缩到「只判断是否高亮」，函数体应小于 20 行。

**为什么不能只是「打开右栏、不加高亮」**：右栏已经打开时点「运行状态」，界面没有任何变化，用户仍会认为「点了没反应」——那正是 O-2 要修的原始缺陷。高亮是让「点了有反应」在右栏已开时也成立的最小改动。

**高亮的配色与圆角只能用微信壳令牌**（`727a626` 首版用了 `theme::`，是这一段唯一的不一致）：

```
✗ .rounded(px(theme::RADIUS_CARD))     ← 14，classic 令牌
✗ .border_color(theme::accent())      ← 默认蓝：DARK_ACCENT = 0x1084fe / LIGHT_ACCENT = 0x4d6bfe
✓ .rounded(px(ui::R_CARD))            ← 12，ui_kit.rs:45，规范 §4 的 R_Card
✓ .border_color(wechat_theme::accent())  ← 默认微信绿 0x07c160（wechat_theme.rs）
```

**为什么必须换**：`theme::accent()` 在用户没选自定义色时返回 `DARK_ACCENT`/`LIGHT_ACCENT`——**那是蓝色**，不是微信绿。`chat_info_panel` 区间内 `wechat_theme::` **33 处**、`theme::` **2 处**（就是这两行新写的），所以不换的话，会给基础卡套一个蓝色边框，而周围全是微信绿系。用户选自定义主题色时更明显：周围跟着变，只有描边不变。

**顺带纠正一个数**：`ui_kit::R_CARD` 已经是 12 且 `src/ui_kit.rs:948` 断言它等于 `theme::RADIUS_MENU`，所以直接写 `ui::R_CARD`，不要写裸 `12.`。

**同批要清的两条尾巴（不影响判据，但会误导下一个读代码的人）**：

1. `settings_detail_panel` 开头的注释「会话专用的两种形态由 `chat_detail_panel` 渲染，这里不处理」——**该函数在 `727a626` 已整个删除**，注释指向一个不存在的函数。`RunStatus` 的 `return None` 分支本身要保留（`RunStatus` 仍不能被设置页渲染），只改注释。
2. `SettingsDetail::RenameSession` 上的 `#[allow(dead_code)] // v0.7 decision: session rename, no wechat-shell entry point yet.`——**入口已经有了**（`chat-info-rename`，`727a626`），这条 allow 的理由不成立。删掉这条 allow（如果 clippy 仍报，说明入口没接对，那才是真问题），顺带删掉它那句过时的理由注释。

**明确不做**：不新增基础卡没有的字段（上下文窗口、token 累计等），那是新功能，v0.7。

**这一节踩过的坑，写下来防止重犯**：`f904043` 只补了 id `chat-detail-run` 让 grep 判据通过，**但没人真点开看过**。第一次截图（`252`）才暴露出三层叠加：外层 `ui::section("运行状态")` + 内层 `ui::DetailPanel` 的 caption（第二个「运行状态」）+ `DetailPanel` 自带的 `PANEL_W=340` 被塞进 `chat_info_panel` 的 `320-24=296` 可用宽，右缘整块裁掉 44px，`detail-back` 还是个空闭包的死按钮。

根因一句话：**`ui::DetailPanel` 是「横向第三列」的骨架（`ui_kit.rs` 注释自己写着「侧栏详情面板」），`settings_detail_panel` 挂到 `body` 上位置是对的；`chat_detail_panel` 的返回值被挂进右栏内部的 body，位置是错的。** 修法是换挂载方式（去掉 `DetailPanel` 外壳、只返回 `ui::card` + `stat_row` 内容），不是调宽度。

因此本项的经验已上升为全局规则：**判据写成 grep 的项，必须同时给一张能证伪的截图**——grep 只证明「有代码」，不证明「用户能走到、且看到的是对的」。

### 3.6 行数

```
删除前 main.rs 29,919 行
第一批（D1+D2 合一个提交）后   ≈27,100 行   （−2,772 代码 +241 死代码 −10 接回 ≈ −2,833）
含 theme.rs 清理与运行状态接回 +10
超出的部分必须解释，否则视为删漏
```

判据：`([System.IO.File]::ReadAllLines('src\main.rs')).Count` 落在 **26,850–27,350** 区间。

> **必须用 `ReadAllLines` 或 `-split "`n"`**：`Get-Content | Measure-Object -Line` 不计空行，`git show HEAD:src/main.rs | Measure-Object -Line` 实测少算 673 行（29,246 vs 29,919），会把「删了 3,000 行」读成「只删了 2,300 行」而误判没删干净。

**不用精确值**：`2,772` 与 `2,554` 的 218 行差来自「含不含函数上方注释行」，两种口径都对；写成单一数字会让执行者反复对数。区间内即通过，超出区间必须给出解释（删漏 → 补删；多删 → 回滚）。

**D-CUT 实际结果（`c93ef4a`）**：

```
src/main.rs    29,919 -> 26,662   (-3,257)   ← 含 f904043 补的 chat-detail-run 容器 +9
src/theme.rs      651 ->    597   (-54)
src/avatar.rs     281 ->    190   (-91)
合计                                        -3,402 行（另加 10 处 #[allow(dead_code)] 注释）
```

`26,662` 比区间下限 `26,850` 低 188 行，原因两条，都不是漏算：

1. **`avatar.rs` 不在原清单里**：`bot_avatar` / `channel_avatar` / `unread_dot` / `shell` / `deg` 整条色相头像栈只被 classic 的 `message_row` 使用，微信壳走 `wechat_avatar`（第 22870 行）。这 91 行是删干净的结果，清单当时漏统计。
2. 多处连续双空行合并成一个。

判定界限：**低于下限只要能归到上面两类就通过**（空行合并、清单外连带删除且符号在微信壳有替代实现）；**不允许为了对齐区间反过来补空行**。

### 3.7 六道门禁（全部 EXIT=0）

```
cargo check --workspace --all-targets --locked
cargo test  --workspace --locked          （576 passed / 0 failed / 2 ignored，只允许因删除而变多或不变）
cargo clippy --all-targets --locked -- -D warnings
scripts\ui-lint.ps1 -Enforce              0 violations / PASS
scripts\verify-parity-anchors.ps1 -Baseline Head        drifted=0 unlocked=0
scripts\verify-parity-anchors.ps1 -Baseline WorkTree    drifted=0 unlocked=0
scripts\verify-arch-anchors.ps1            anchors=143 ok=92 bad=0
```

**测试数只允许上升或不变**：`composer_radius` 的单测随函数一起删（D-CUT 前它在第 29536–29546 行），所以 `576` 会变成 `575`；此时必须由新增的欢迎区/运行状态测试补回来，总数不低于 575。低于 575 说明有人顺手删了别的测试。

**D-CUT 实测（`c93ef4a`）**：`575 passed / 0 failed / 2 ignored`，正好等于下限，六道门禁全 `EXIT=0`，`parity` 双口径 `drifted=0 unlocked=0`、锁 **412 条**（原 419，随 classic 一起死掉 7 条）。**D3 加欢迎区与改名入口后，测试数必须 ≥575；低于此数视为回归。**

### 3.8 文档与锁

```powershell
# 文档里落进删除区的锚点必须已重指（删除方案 §7.3）
Select-String -Path docs\parity-matrix.md,docs\testing\0.5.1-anchor-label-proposals.md -Pattern 'fn bot_details|main_bot_card'
# 期望：(no output)。实测基线（c86c899）：只命中 parity-matrix.md:33（A12 的 `main_bot_card`）
# 判据：删除+改判后为 0
```

锁的条数：**419 条，其中 `src/main.rs` 144 条 → 删 17 / 平移 77 / 不变 50**（删除方案 §7.1，`17+77+50=144` 自洽；起作用的判据是三个数相加等于删除前的 `main.rs` 锚点数，不是某一版数字）。`419` 是 v0.6.0 PRD 提交（`454cb36`）后的实测值，不是 v0.5.1 的 417——那次提交为 A12 新增了 `wechat_list_row` 与 `wechat_nav` 两个锚点。**代码、文档、锁必须在同一个提交里**，顺序见删除方案 §7.4。

> 早前 PRD 写的「平移 31 / 不变 94」只算了 D1 旧区间；把 `settings_overlay`、`theme_picker` 与五处易漏项并进来后，删除区之后的锚点从 31 涨到 77。**以此处和删除方案 §7.1 为准**。

---

## 4. 风险与回滚

| # | 风险 | 概率 | 影响 | 应对 |
| --- | --- | --- | --- | --- |
| R1 | 把「看着只被 classic 调」的共用函数一起删了，编译失败或功能静默消失 | 中 | 高 | 删除方案 §4 的「必须保留」表是唯一依据，删前逐个 `grep` 调用点；每批删完立刻 `cargo check`，不攒到最后 |
| R2 | 一次删 2,600 行，视觉回归无法逐项验收 | 高 | 中 | 分批提交（§6），每批自带 `cargo check`；欢迎区与运行状态面板各配截图 |
| R3 | 锚点大规模重排，混进一条「指向别人代码」的假锚点 | 中 | 中 | 严格按删除方案 §7.4：先删代码 → 再改文档 → 最后 `-UpdateAnchors -Baseline WorkTree` → 同一提交 → 两口径复核 |
| R4 | `wecom_mode` 的两处反向条件不同批删，编译中断 | 高（若操作不规范） | 低 | 第二批把三者当一个原子改动；删除方案 §6.1 已写明 |
| R5 | 微信壳在删除后暴露出原本被 classic 掩盖的缺陷（例：运行状态面板） | 中 | 中 | §3.5 已定为必做项；其余同类在优化批逐条修，不做的不进本版 |
| R6 | 用户装了 `BOT_PILOT_CLASSIC_UI=1` 后启动，行为突变 | 低 | 低 | 环境变量被移除后即忽略；发布说明写「v0.6.0 起只有一套界面，该变量已废弃」 |

### 回滚

- 分四个提交（§6），**每个提交独立可 `git revert`**。不做 squash，保留逐步回退能力。
- 最坏情况整体回退：`git revert --no-commit <第一批>..<最后一批>` 后单次提交，回到 `c86c899` 的界面形态。
- 已发布版本不受影响：v0.5.1（`94ad7fc`）的 tag 与 Release 资产不动，用户可回装。
- **数据零风险**：本版不碰任何数据结构、不做迁移，`~/.mode-bot` 的读写路径不变。回滚不会丢数据。

---

## 5. 微信壳 UI 优化清单

> **参照口径**：用户已决定不再照搬 DS Bot 界面。本节引用的 DS Bot 代码只作「信息怎么组织、反馈什么时候出现」的参照，**视觉语言一律按微信风落**（`wechat_theme` 令牌、Lucide 线框、4px 间距档、四级字号）。凡与微信观感冲突的，按微信落。

| # | 项 | 参照证据（现状 / DS Bot） | 微信风怎么落 | 验收怎么验 | 优先级 |
| --- | --- | --- | --- | --- | --- |
| **O-1** | 空会话欢迎区 + 三条建议 chip（**单聊 + 群聊两条路径**） | classic `transcript` 的欢迎区（头像 / 欢迎语 / `welcome-suggestion-{i}`，D-CUT 前第 19535–19664 行）；群聊侧 classic `group_placeholder`（`c93ef4a~1` 的第 21439–21460 行：九宫格头像 + 「群名 · N 人」+ 成员名单）。**两条路径均已完成**：单聊 `0f9b4b5`（`wechat_welcome`），群聊 `0706ebb`（`wechat_group_welcome`，id `wechat-group-welcome`） | 单聊：复用 `avatar` + `tri!` 三语；气泡用 `wechat_theme` assistant 气泡色；chip 28px、`RADIUS_TAG`、hover `nav_item_hover_bg()`；点击 `input.set_content` + 聚焦。群聊：`wechat_welcome` 同时接 Bot / Group，群分支复用 `wechat_group_avatar` / `wechat_channel_avatar` / `count_members` / `visible_group_members`，文案走 `tri!`（**未用 `empty_state_badged`**：它只接 `Option<AnyElement>` + 两行文案，套进来要覆盖内边距，所以群欢迎区结构直接写在 `wechat_group_welcome` 里） | §3.4 的 grep + **四张截图**（单聊深浅 ×2、群聊深浅 ×2） | **已完成**（`0f9b4b5` 单聊 / `0706ebb` 群聊）：截图 `250`/`254` 单聊深浅、`251`/`253` 群聊深浅、`255` 点 chip 后填入。**注意 `253` 摄于 09:24:59（修复前），只能作对照，不作当前代码通关证据** |
| **O-2** | 运行状态面板接回 | 现状（D-CUT 前）：「⋯ → 运行状态」设了状态但无渲染点（第 17460 行 → `chat_detail`，`chat_info_panel` 不读） | 在 `chat_info_panel` 里读 `self.chat_detail`，**不另建第二张卡**：`dda719d` 最终拍板为「基础卡加 accent 描边 + 仅高亮时挂一行 `ui::section("运行状态")`」，id `chat-detail-run` 挂在带描边的基础卡上。理由是第二张卡的 10 行里 7 行与基础卡完全重复，删卡不删功能。**不套 `ui::DetailPanel`**（它自带 `PANEL_W=340`，塞进 296 可用宽会裁掉 44px）。**不设 `chat-detail-close` 独立关闭按钮**：卡片右上角已有 `chat-info-collapse` 折叠按钮，多一个关闭按钮就是只写不读；高亮的清除靠 4b（折叠再展开）/ 4c（换 Bot）两个入口，`chat_detail = None` 共 4 处（`3884` / `4145` / `11255` / `24118`） | §3.5 的 grep + **四条截图判据**（见 §3.5） | **已完成**（`c93ef4a` 补 id → `dda719d` 改高亮方案 → `6930a4a` 补回标题 → `eb941a2` 真截图）：grep 四项通过（`chat-detail-run`=1、`chat-detail-close`=0、`detail-back`=0、`fn chat_detail_panel`=0），截图 `261`（标题 + 绿描边）/ `257`（4b）/ `258`+`262`（4c 两步）。**教训：只凭 grep 判过，漏掉了重复标题 / 死「返回」/ 右缘裁切；`259` 还出过与 `257` 同 SHA256 的假证据** |
| **O-3** | 工作中状态行的文案分档 | 现状 `i18n::t("重试中")` / `i18n::t("工作中")` 两档，没有按工具名分短语；DS Bot `ds-bot/src/client/cells.js:854-867` 按工具名给短语（`TOOL_PHRASES` 8 类正则 → Searching online / Running shell commands / Opening a file …） | 一行 12px 次级色 + spinner，短语按工具类别取；**不引 DS 的英文池**，写中文短句（「正在检索网页」「正在执行命令」「正在读文件」）；未知工具回落到「工作中」 | 截图两张：分别触发浏览器工具与 shell 工具，状态行文案不同；`grep` 短语常量表存在 | P1 |
| **O-4** | 计时器每秒自跳 | 现状 `elapsed_label(elapsed)` 在重渲染时取值；DS Bot `ds-bot/src/client/cells.js:837-850` 的 `Elapsed` 用 `setTimeout(1000 - ((now-at)%1000))` 对齐整秒自跳，父组件不重渲染 | 用 `cx.spawn` + `window.defer` 每秒 `cx.notify()`，不引入新依赖、不整页重绘 | 截图两张（间隔 ≥3 秒），秒数确实变化；`BOT_PILOT_DEBUG=1` 下渲染耗时打点不因它暴涨 | P1 |
| **O-5** | 间距统一归入 4px 档（U-01） | `0.5.1-ui-review.md` U-01；标准见 `docs/ui-design-spec.md` 第 2 节「只用这六档」（4 / 8 / 12 / 16 / 24 / 32），同节末「废弃 5 / 10 / 14 / 18」。实测量级：`.p/.px/.py/.pt/.pb/.pl/.pr/.gap/.m*` 共 **560** 处，档外 **276** 处（`px(6)` 95、`px(10)` 64、`px(2)` 50、`px(14)` 33、`px(5)` 9、`px(1)` 6、`px(9)` 6、`px(3)` 3、`px(7)` 3、`px(56)` 1）。**数字只作量级参考，不进 AC**（理由见 §5.2） | 见 §5.2：先给 `scripts/ui-lint.ps1` 加第 5 条规则并以 report-only 跑出基线，再按页清零（会话列表 → 聊天区 → 右栏 → 设置页），每页改完前后各一张截图 | 见 §5.2：唯一判据是第 5 条规则在 `-Enforce` 下 **0 违规**；基线数值以脚本输出为准，不写手数的数字 | P1 |
| **O-6** | 21 处 `truncate()` 缺 `min_w(px(0.))`（U-03） | 54 处 `truncate()`，21 处前 10 行内无 `min_w(px(0.))`；gpui 的 flex 子项 `min-width` 默认 `auto`，长 Bot 名 / 长路径会把父容器撑开而不是省略 | 只给**父容器是 flex 行且右侧有按钮**的位置补 `min_w(px(0.))`；父容器本身 `w_full` 的不补（避免多出无意义的省略号） | 逐处改完 `grep` 计数下降；超长 Bot 名（≥40 字）截图不撑破行 | P2 |
| **O-7** | `ui_kit::button` / `button_ghost` 补 disabled 形态（U-04） | `src/ui_kit.rs:405/433` 两个函数无 disabled 参数、无置灰视觉 | 加 `disabled(bool)`，置灰用 `theme::ink_tertiary()` + 去 `cursor_pointer` + 不挂 hover；现有调用点不用改（默认 `false`） | 截图：一个 disabled 按钮与一个正常按钮同屏；`grep` 新增签名 | P2 |

> 表中 O-3 / O-4 的 DS Bot 行号是 `D:\Mode Bot\DS-Bot` 的源码位置（v0.3.6），仅作参照，不在本仓。

### 5.1 优化批的边界

- **不动四栏**：导航 / 会话列表 / 聊天区 / 右侧详情栏的宽度、顺序、开合逻辑都不变（`PANEL_WIDTH`、`chat_info_panel` 位置不变）。
- **不新增入口**：O-1 的 chip 是「把输入框文字填进去」，不是新命令；O-2 的面板是已有菜单项的渲染点，不是新菜单项。
- **不换视觉体系**：图标继续 Lucide 线框、间距走六档、字号四档、圆角令牌，`ui-lint -Enforce` 是硬门禁。
- **每页改完立刻截图**：O-5 是机械替换，最容易出视觉回归，不做「全部改完再看」。

### 5.2 O-5 的验收口径（第 5 条间距规则）

**为什么不能把处数写进 AC**：同一份 `main.rs`、同一个「档外处数」，三次独立统计得到三个值——@前端 276、@Chief 263、@产品 269，差别只来自正则是否把 `.p()` 单值、`.size()`、`.line_height()` 算进来；classic 删除区里会随 D-CUT 一起消失的档外处数，@前端 实测 48、@产品 实测 39（按 28 个函数区间并集）。**这类数字随统计口径变，一旦写进 AC 就变成一条永远对不上的 AC**（U-06 同类教训）。

**所以 O-5 拆成 4 步，前 3 步是前置条件，第 4 步才是验收**：

| 步 | 动作 | 完成标志 |
| --- | --- | --- |
| ① 加规则 | 在 `scripts/ui-lint.ps1` 加第 5 条规则 `spacing_offtoken`：只检查 `.p/.px/.py/.pt/.pb/.pl/.pr/.gap/.m/.mx/.my/.mt/.mb/.ml/.mr` 的 `px(N)` 参数，`N` 属于 {4,8,12,16,24,32} 或在 `$SpacingTokens` 白名单内即通过；**`px(0.)` 一律放行**（它是 `min_w` / `min_h` 一类的容器约束，不是视觉间距）；规则名必须加进 `$order`，否则 `-Enforce` 根本不统计它 | `ui-lint.ps1` 能 grep 到 `spacing_offtoken`，且 `$order` 有 5 项 |
| ② 报基线 | 规则先以 **report-only** 上线（不计入 `-Enforce` 退出码），跑一次，把基线条数写进本文 §8 变更记录 | 一条带日期、数值取自脚本输出的基线记录 |
| ③ 逐页清零 | 会话列表 → 聊天区 → 右栏 → 设置页，每页改完一张前 / 后截图 | 4 页共 8 张截图 |
| ④ 切硬门禁 | 第 5 条规则从 report-only 切到 `-Enforce` | `ui-lint.ps1 -Enforce` 输出 `total violations: 0`，且规则清单里含 `spacing_offtoken` |

**明确不做**：不把 `px(0.)`、`.size()`、`.line_height()` 纳入第 5 条（后两类分别归字号规则与既有规则）；不为凑数把 `px(6)` 那 82 处一次性全改——先让规则上线报基线，再按页清零，否则一次性大改没人能看出视觉回归。

**D-CUT 之后的实测基线（`b887ddd`，规则尚未上线，这是 ② 报基线时应记录的数）**：

```
main.rs    总间距 425   在档 200  档外 225
ui_kit.rs  总间距   5   在档   0  档外   5   ← gap(px(2.)) / py(px(1.))，令牌层自身
全 src     总间距 430   在档 200  档外 230
档外值分布：px(6) 82  px(10) 51  px(2) 41  px(14) 28  px(5) 7  px(9) 5  px(1) 5  px(7) 3  px(3) 3
```

删除前全 src 是 545 / 274，D-CUT 带走约 44 处档外；**这也说明「删 classic 顺手把间距改干净」不成立**——删完还剩 230 处，O-5 仍是实打实的工作量。`ui_kit.rs` 那 5 处是令牌层自己的微间距（`gap(px(2.))`），要么进 `$SpacingTokens` 白名单、要么统一到 `S1`，**必须在 ① 之前定，否则 ④ 切 `-Enforce` 时会挡住自己**。

**@测试 的验收动作固定为一条命令**：`scripts/ui-lint.ps1 -Enforce`，要求输出里出现 `spacing_offtoken` 这一项且 `total violations: 0`。只看 `PASS` 不看规则名，会在第 5 条尚未上线时**假通过**——这是本项唯一必须防的坑。

### 5.3 本 PRD 里行号的写法规则（阶段 3 的教训）

`main.rs` 在阶段 3 每一批 UI 改动都会整体下移（O-1 单聊 +113、群聊再 +76），**写死在 PRD 里的「文件:行号」会静默过期而门禁抓不到**。`36136db` 之后本文件已把全部「裸行号」改成纯文本（第 N 行），并由 `scripts/verify-parity-anchors.ps1` 的 `$docs` 覆盖本文件——两条一起做才算真解：只改写法、不进门禁，下一次照样能写回锚点形式。

所以本文件的行号分三类，写法不同：

| 类型 | 写法 | 例子 | 为什么 |
| --- | --- | --- | --- |
| **当前 HEAD 的活锚点** | 必须写「文件:行号」时，仓库内文件一律带 `src/` 或 `crates/` 前缀，外部对照树带 `ds-bot/` 或 `rpi-*` 前缀 | 现在共 **10 条**：仓库内 **8 条**（`src/theme.rs` 2、`src/main.rs` 3、`src/data.rs` 2、`src/ui_kit.rs` 1）受位置 + 符号 + 指纹三层校验；`ds-bot/src/client/cells.js` **2 条**属外部对照树，计在 `skipped-external` 里**只验文件存在、不验内容**（DS 源码不在本仓，改不了也不该改） | 不带前缀的 `theme.rs:N` 会被当成仓库根下的文件，直接 FILE NOT FOUND；带前缀才能被位置校验、符号校验和指纹锁覆盖 |
| **历史位置**（描述「删除前在哪」） | 纯文本，写成「第 N 行」，不加反引号、不写 `文件:N` | 「D-CUT 前 `if chat.messages.is_empty()` 在第 27783 行」 | 写成锚点会被当成当前引用去校验，而该行在 HEAD 上早已不存在 |
| **判据** | 只写 grep 模式 / id / 截图，不写行号 | `grep 'chat-detail-run' = 1`、`wechat-welcome-suggestion-` ≥1 | 判据要能长期成立，行号会漂 |

**门禁范围（`@架构` 已定，`36136db` 之后落地）**：`$docs` 已加入 `docs/prd-0.6.md`，`-Baseline Head` 会连本文件一起取 HEAD 版本，所以「工作区改了文档、HEAD 没改」不会让结果抖动。`docs/plans/**`、`docs/testing/**`、`docs/prd-0.5.md` 之外的历史快照保持显式排除——它们记录的是当时 tag 的行号，按今天的 HEAD 去改等于篡改历史实测。

---

## 6. 任务拆分与依赖

四批，每批一个提交、每批自带验证。**不许合并成一次提交**（R2 的应对）。

**D1 与 D2 合成一个提交（本文定为 D-CUT）**，不是笔误：`theme::set_light` / `theme::accent_border` 的唯一调用点是 `theme_picker`，而 `theme_picker` 属于第二批。拆开提交，第一批结束时 `clippy -D warnings` 一定因 unused 报错。「删除批 / 优化批」的分界仍然成立，只是删除批内部不再拆。

| 批 | 内容 | 负责 | 前置 | 验证 |
| --- | --- | --- | --- | --- |
| **D-CUT** | ① 删 §2.1 第一批（约 2,772 行含注释）+ 五处易漏项（`control_card` / `chat_status` / `status_color` / `two_line` / `text_block`）；② 删第二批（241 行）+ `shell()` 改单分支 + `BOT_PILOT_CLASSIC_UI` 移除；③ 清 `theme.rs` 10 函数 + 2 常量 + `shadow_1` 断言 | 前端 | 删除方案补完五处易漏项与 `theme.rs` 清单 | 每删一组 `cargo check`；本批结束时 §3.1 / §3.2 / §3.3 的 grep 全过，`clippy -D warnings` 0 告警 |
| **D3** | 搬欢迎区（O-1：单聊三 chip + **群聊九宫格/成员名单**）+ 接回会话改名入口（§2.4 ①） | 前端 | D-CUT 完成 | O-1 已闭环（`0f9b4b5` / `0706ebb`，截图 `021eb99`）；O-2 已闭环（`6930a4a` / `eb941a2`，截图 `260` / `261` / `262`）；**改名入口已接回（`e659eb7`）但缺实机截图**：`chat-info-rename` = 1、`SettingsDetail::RenameSession` 构造点 2 处（`9086` 面板分支 + `24120` 新入口）、保存链路 `9114 sessions.set_title` → `sessions.save()` 完整。**完成条件 = §3.4 grep + 四张截图 + 改名实机图**（点「改名」→ 出现「重命名会话」弹窗 → 改完名「最近会话」那行文案跟着变），三条齐了才算 D3 闭环 |
| **D4** | 文档锚点重指 + 锁重建 + 矩阵 A12 改判 | 产品（锚点语义）+ 架构（重排脚本） | D-CUT / D3 完成 | 删除方案 §7.4 五步；§3.8；矩阵 88/21/27 |
| **O 批** | O-3 / O-4 / O-5 / O-6 / O-7 | 前端（O-5 先加规则再逐页，见 §5.2） | D4 六道门禁全绿 | §5 各行「验收怎么验」+ §5.2 四步 |
| **V 批** | 回归验证 + 发布 v0.6.0 | 测试 | O 批完成 | 见目标验收 |

**依赖关系里唯一不能反序的**：D4 的锁重建必须在 D-CUT / D3 之后（删除方案 §7.4：先删代码 → 再改文档 → 最后重建锁）。反过来会把错误指纹固化成基线。

---

## 7. 与删除方案、老文档的对应

| 本文 | 对应 | 说明 |
| --- | --- | --- |
| §2.1 删除范围 | 删除方案 §3、§6.1 | 本文定「删什么」，方案定「为什么能删」（调用点证据） |
| §2.2② `chat_detail_panel` | 删除方案 §6.2 选项 1 | 本文已拍板选 1 |
| §2.2① A12 改判 | 删除方案 §7.3 第 3 行 | 本文已拍板降 🟡，并承担矩阵汇总改动 |
| §3.4 / §3.5 | 删除方案 §5.2 | 欢迎区落法以方案表格为准 |
| §3.7 门禁 | 删除方案 §7.4 第 5 条 | 同一组命令 |
| §5 优化清单 | `0.5.1-ui-review.md` §9（U-01…U-05） | U-01→O-5、U-03→O-6、U-04→O-7；U-02/U-05 不做 |
| §2.3 不做「修 14 项差分」 | `0.5.1-shell-parity-diff.md` 全篇、删除方案 §8 | 差分随 classic 删除而作废，**不写成「14 项通过」** |

---

## 8. 变更记录

| 日期 | 版本 | 变更 | 影响 |
| --- | --- | --- | --- |
| 2026-11-26 | 0.6.0-draft | 首版。删 classic（main.rs 约 3,013 行 + theme.rs 约 20 行）+ 微信壳优化（O-1…O-7）；拍板 A12 降 🟡、`chat_detail_panel` 保留接回；明确不做骨架改动、引用条、主卡位、U-02、U-05 | 矩阵 89/20/27 → **88/21/27** |
| 2026-11-26 | 0.6.0-draft2 | 吸收交叉验证：补五处易漏项（`control_card`/`chat_status`/`status_color`/`two_line`/`text_block`）与 `theme.rs` 10 函数 2 常量；D1+D2 合为 D-CUT；行数预期改区间；O-5 判据改为只认 `ui-lint` 第 5 条规则（数量级 260–280 仅作参考） | 无功能变更；删除清单从 27 项增至 42 项单元 |
| 2026-11-26 | 0.6.0-draft3 | 新增 §5.2「O-5 的验收口径」：第 5 条间距规则拆成「加规则 → 报基线 → 逐页清零 → 切硬门禁」四步，`px(0.)` 放行，判据只认 `-Enforce` 输出里 `spacing_offtoken` 为 0；O-5 行内数字改为单一来源实测（560 / 档外 276）且明确不进 AC | 删掉 AC 里对数字的依赖；测试验收动作收敛为一条命令 + 一个规则名 |
