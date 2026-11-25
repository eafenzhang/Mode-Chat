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
| 第二批 | `settings_overlay`、`theme_picker`、`wecom_mode` 字段、`BOT_PILOT_CLASSIC_UI` 环境变量、`SIDEBAR_WIDTH`、`BUBBLE_MAX_WIDTH`、`active_tab` 字段 | 约 241 行 | 删完第一批才变成死代码；`wecom_mode` 必须与 `:9040`、`:12118` 两处反向条件同批删，否则编译不过 → 删除方案 §6.1 |
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

**`theme::set_light` 的陷阱**：`theme.rs:143` 的 `set_light` 唯一调用在 `theme_picker@13106`，可删；但 **`wechat_theme::set_light` 是另一个函数**，`12463` 和 `29319` 都在调用，**必须保留**；`theme.rs:138` 的 `set_light_runtime` 在 `12462`/`29318` 也在用，同样保留。同理 `RADIUS_BUBBLE` 必须留（`skeleton_rows@24764` 微信壳空态在用）。

**D1 与 D2 必须合成一个提交**：`theme::set_light` / `theme::accent_border` 只有 `theme_picker` 用，而 `theme_picker` 在第二批；拆成两次提交，第一批结束时 `clippy` 一定报 unused 而失败。「删除批 vs 优化批」的分界线仍然有效，只是删除批内部不再拆。

**行数预期改成区间**：删完 `main.rs` 落在 **26,850–27,350 行**（含 `theme.rs` 的清理与运行状态接回 +10）。拿 `27,327` 当精确值会对不上，因为统计口径（含不含注释行）差 12 行以上。

**必须保留**（被微信壳 / 设置页 / 弹层共用，删一个就编译失败或静默丢功能）：`kv_row`、`short_id`、`count_members`、`visible_group_members`、`count_bots`、`is_group_prompt`、`model_label`、`notify_background`、`close_or_hide`、`approval_reason_view`、`approval_note`、`member_label`、`selected_ids`、`overlay_center`、`dialog_card`、`switch`、`dialog_field`、`compose_with_quote`、`user_text_with_mentions`、`fenced_code`、`history_attachment_view`、`attachment_chips`、`error_bar`、`current_engine_is_mode`、`attachments_allowed`、`stage_attachment_paths`、`stage_image_bytes`、`start_screenshot_capture`、`toggle_voice_typing`、`remove_attachment`、`empty_state`、`skeleton_rows`、`kit_markdown_el`、`compaction_status_row`、`tool_copy_button`、`tool_actions_row`、`group_memory_section`。完整表 + 微信壳侧调用点见**删除方案 §4**。

### 2.2 两处必须由本文拍板的判定

**① A12「主卡位」：判定从 ✅ 降为 🟡，矩阵汇总 89/20/27 → 88/21/27。**

实测依据：`main_bot_card`（`:19086`）只被 `sidebar@19012` 调用；微信壳区间 `25799-29000` 内 `is_chief` / `is_main` / `chief` **0 命中**；侧栏行 `wechat_list_row`（`:21754`）只画头像、名字、时间、未读点，没有「这是 Main bot」的视觉位。

所以删除后**用户看不到「谁是主 Bot」的突出标识**——这是真实的功能损失，不是锚点问题。判定 🟡，锚点落到 `:26251`（`fn wechat_nav`），差距列写「微信壳只有平铺列表，无主卡位；补主卡位记 v0.7」。

同步改动：`docs/parity-matrix.md` 的 `:8`（汇总说明）、`:33`（A12 行）、`:249-254`（汇总表）、`:260`（A 域分域明细）、文末「v0.5.1 的变动」表补一行。

**② `chat_detail_panel`（109 行，`:9318-9426`）：保留并接回微信壳，不删。**

现状是既有缺陷：`chat_detail` 只被 `:9325` / `:9420` 读、`:17460`（`DetailMenuAction::RunStatus`，微信壳的「⋯」也走这里）写，唯一渲染点是 `right_panel@20823`。也就是说**微信壳里点「⋯ → 运行状态」现在什么都不显示**。删 classic 会让它彻底没有渲染点。

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

---

## 3. 可执行验收（删除批）

每条都能用一条命令或一张截图判定。**行号一律指 `c86c899`**，删除后按新行号复核 `grep` 结果即可。

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

```powershell
Select-String -Path src\main.rs -Pattern 'fn shell\(' -Context 0,3
# 期望函数体只有一行 self.wechat_shell(window, cx)，没有 if/else
```

### 3.4 欢迎区搬进微信壳（grep + 截图，两条都要）

```powershell
Select-String -Path src\main.rs -Pattern 'wechat-welcome-suggestion-'
# 期望：命中 1 处（生成 id 的 format!），三条 chip 由它循环产出
```

截图（两张，均需在隔离根下拍）：
- 深色主题：新开一个 Bot 会话，0 条消息 → 有头像、欢迎语、三条建议 chip，点 chip 后文字进输入框且聚焦。
- 浅色主题：同上。
- 群聊会话不出现欢迎区（微信壳无「当前 Bot」语义）。

### 3.5 运行状态面板接回（grep + 截图）

```powershell
Select-String -Path src\main.rs -Pattern 'chat-detail-run'
# 期望：命中 1 处（chat_info_panel 里的渲染点 id）
```

截图：微信壳「⋯ → 运行状态」→ 右栏出现运行状态卡片（不是点了没反应）。

### 3.6 行数

```
删除前 main.rs 29,919 行
第一批（D1+D2 合一个提交）后   ≈27,100 行   （−2,772 代码 +241 死代码 −10 接回 ≈ −2,833）
含 theme.rs 清理与运行状态接回 +10
超出的部分必须解释，否则视为删漏
```

判据：`(Get-Content src\main.rs | Measure-Object -Line).Lines` 落在 **26,850–27,350** 区间。

**不用精确值**：`2,772` 与 `2,554` 的 218 行差来自「含不含函数上方注释行」，两种口径都对；写成单一数字会让执行者反复对数。区间内即通过，超出区间必须给出解释（删漏 → 补删；多删 → 回滚）。

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

**测试数只允许上升或不变**：`composer_radius` 的单测（`:29536-29546`）随函数一起删，所以 `576` 会变成 `575`；此时必须由新增的欢迎区/运行状态测试补回来，总数不低于 575。低于 575 说明有人顺手删了别的测试。

### 3.8 文档与锁

```powershell
# 文档里落进删除区的锚点必须已重指（删除方案 §7.3）
Select-String -Path docs\parity-matrix.md,docs\testing\0.5.1-anchor-label-proposals.md -Pattern 'fn bot_details|main_bot_card'
# 期望：(no output)。实测基线（c86c899）：只命中 parity-matrix.md:33（A12 的 `main_bot_card`）
# 判据：删除+改判后为 0
```
```

锁的条数：417 → 按删除方案 §7.1 重算（删 17 / 平移 31 / 不变 94）。**代码、文档、锁必须在同一个提交里**，顺序见删除方案 §7.4。

---

## 4. 风险与回滚

| # | 风险 | 概率 | 影响 | 应对 |
| --- | --- | --- | --- | --- |
| R1 | 把「看着只被 classic 调」的共用函数一起删了，编译失败或功能静默消失 | 中 | 高 | 删除方案 §4 的「必须保留」表是唯一依据，删前逐个 `grep` 调用点；每批删完立刻 `cargo check`，不攒到最后 |
| R2 | 一次删 2,600 行，视觉回归无法逐项验收 | 高 | 中 | 分批提交（§6），每批自带 `cargo check`；欢迎区与运行状态面板各配截图 |
| R3 | 锚点大规模重排，混进一条「指向别人代码」的假锚点 | 中 | 中 | 严格按删除方案 §7.4：先删代码 → 再改文档 → 最后 `-UpdateAnchors -Baseline WorkTree` → 同一提交 → 两口径复核 |
| R4 | `wecom_mode` 与 `:9040` / `:12118` 不同批删，编译中断 | 高（若操作不规范） | 低 | 第二批把三者当一个原子改动；删除方案 §6.1 已写明 |
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
| **O-1** | 空会话欢迎区 + 三条建议 chip | classic `transcript:19535-19664`（头像 / 欢迎语 / `welcome-suggestion-{i}`）；微信壳 `:27795` 现在是裸 `return div()`，**一片空白** | 复用 `avatar` + `tri!` 三语；气泡用 `wechat_theme` 的 assistant 气泡色；chip 28px 高、`RADIUS_TAG`、hover 用 `wechat_theme::nav_item_hover_bg()`；id 用 `wechat-welcome-suggestion-{i}`；点击 `input.set_content` + 聚焦 | §3.4 的 grep + 深/浅两张截图 | P0（删除批同步） |
| **O-2** | 运行状态面板接回 | 现状：「⋯ → 运行状态」设了状态但无渲染点（`:17460` → `chat_detail`，`chat_info_panel` 不读） | 在 `chat_info_panel` 顶部按 `self.chat_detail.is_some()` 渲染一张卡，id `chat-detail-run`；样式复用 `ui::card` / `ui::stat_row` | §3.5 的 grep + 截图 | P0（删除批同步） |
| **O-3** | 工作中状态行的文案分档 | 现状 `:28181-28184` 只有「工作中 / 重试中」两种；DS Bot `cells.js:854-867` 按工具名给短语（`TOOL_PHRASES` 8 类正则 → Searching online / Running shell commands / Opening a file …） | 一行 12px 次级色 + spinner，短语按工具类别取；**不引 DS 的英文池**，写中文短句（「正在检索网页」「正在执行命令」「正在读文件」）；未知工具回落到「工作中」 | 截图两张：分别触发浏览器工具与 shell 工具，状态行文案不同；`grep` 短语常量表存在 | P1 |
| **O-4** | 计时器每秒自跳 | 现状 `elapsed_label(elapsed)` 在重渲染时取值；DS Bot `cells.js:837-850` 的 `Elapsed` 用 `setTimeout(1000 - ((now-at)%1000))` 对齐整秒自跳，父组件不重渲染 | 用 `cx.spawn` + `window.defer` 每秒 `cx.notify()`，不引入新依赖、不整页重绘 | 截图两张（间隔 ≥3 秒），秒数确实变化；`BOT_PILOT_DEBUG=1` 下渲染耗时打点不因它暴涨 | P1 |
| **O-5** | 间距统一归入 4px 档（U-01） | 基线量级 **260–280 处**不在 {0,4,8,12,16,24,32}（我的正则实测 269 / 总 536；@Chief 用另一条正则得 263 / 总 527——差异来自 `.p()` 单值、`.size()`、`.line_height()` 是否计入）。**这个数字只是量级参考，不是判据** | 先给 `scripts/ui-lint.ps1` 加第 5 条规则（间距值必须属于六档或 `ui_kit::S*`），再按**页**清零：会话列表 → 聊天区 → 右栏 → 设置页，每页改完截图对照 | **判据只有一条**：`ui-lint.ps1 -Enforce` 第 5 条规则 **0 违规**。外加每页改前/改后截图各一张 | P1 |
| **O-6** | 21 处 `truncate()` 缺 `min_w(px(0.))`（U-03） | 54 处 `truncate()`，21 处前 10 行内无 `min_w(px(0.))`；gpui 的 flex 子项 `min-width` 默认 `auto`，长 Bot 名 / 长路径会把父容器撑开而不是省略 | 只给**父容器是 flex 行且右侧有按钮**的位置补 `min_w(px(0.))`；父容器本身 `w_full` 的不补（避免多出无意义的省略号） | 逐处改完 `grep` 计数下降；超长 Bot 名（≥40 字）截图不撑破行 | P2 |
| **O-7** | `ui_kit::button` / `button_ghost` 补 disabled 形态（U-04） | `ui_kit.rs:405/433` 两个函数无 disabled 参数、无置灰视觉 | 加 `disabled(bool)`，置灰用 `theme::ink_tertiary()` + 去 `cursor_pointer` + 不挂 hover；现有调用点不用改（默认 `false`） | 截图：一个 disabled 按钮与一个正常按钮同屏；`grep` 新增签名 | P2 |

> 表中 O-3 / O-4 的 DS Bot 行号是 `D:\Mode Bot\DS-Bot` 的源码位置（v0.3.6），仅作参照，不在本仓。

### 5.1 优化批的边界

- **不动四栏**：导航 / 会话列表 / 聊天区 / 右侧详情栏的宽度、顺序、开合逻辑都不变（`PANEL_WIDTH`、`chat_info_panel` 位置不变）。
- **不新增入口**：O-1 的 chip 是「把输入框文字填进去」，不是新命令；O-2 的面板是已有菜单项的渲染点，不是新菜单项。
- **不换视觉体系**：图标继续 Lucide 线框、间距走六档、字号四档、圆角令牌，`ui-lint -Enforce` 是硬门禁。
- **每页改完立刻截图**：O-5 是机械替换，最容易出视觉回归，不做「全部改完再看」。

---

## 6. 任务拆分与依赖

四批，每批一个提交、每批自带验证。**不许合并成一次提交**（R2 的应对）。

**D1 与 D2 合成一个提交（本文定为 D-CUT）**，不是笔误：`theme::set_light` / `theme::accent_border` 的唯一调用点是 `theme_picker`，而 `theme_picker` 属于第二批。拆开提交，第一批结束时 `clippy -D warnings` 一定因 unused 报错。「删除批 / 优化批」的分界仍然成立，只是删除批内部不再拆。

| 批 | 内容 | 负责 | 前置 | 验证 |
| --- | --- | --- | --- | --- |
| **D-CUT** | ① 删 §2.1 第一批（约 2,772 行含注释）+ 五处易漏项（`control_card` / `chat_status` / `status_color` / `two_line` / `text_block`）；② 删第二批（241 行）+ `shell()` 改单分支 + `BOT_PILOT_CLASSIC_UI` 移除；③ 清 `theme.rs` 10 函数 + 2 常量 + `shadow_1` 断言 | 前端 | 删除方案补完五处易漏项与 `theme.rs` 清单 | 每删一组 `cargo check`；本批结束时 §3.1 / §3.2 / §3.3 的 grep 全过，`clippy -D warnings` 0 告警 |
| **D3** | 搬欢迎区（O-1）+ 接回运行状态面板（O-2） | 前端 | D-CUT 完成 | §3.4 / §3.5 的 grep + 四张截图（深浅 × 两功能） |
| **D4** | 文档锚点重指 + 锁重建 + 矩阵 A12 改判 | 产品（锚点语义）+ 架构（重排脚本） | D-CUT / D3 完成 | 删除方案 §7.4 五步；§3.8；矩阵 88/21/27 |
| **O 批** | O-3 / O-4 / O-5 / O-6 / O-7 | 前端（O-5 逐页） | D4 六道门禁全绿 | §5 各行「验收怎么验」 |
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
