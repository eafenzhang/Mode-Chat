# classic（DS 风格三栏）外壳删除方案

> 基线：`c86c899`（`src/main.rs` 29,919 行）。行号一律按这个提交。
> 配套需求：`docs/prd-0.6.md`。本文按产品草案的编号引用：§2 范围与不做、§3 可执行验收、§5 微信壳优化清单；**编号若与 PRD 最终版不同，以 PRD 为准**。
> 本文件向 PRD 提供「删/留」判定的行号依据；PRD 向本文件提供「删干净」与「不新增功能」的验收口径。

## 1. 结论

| 项 | 数字 | 说明 |
| --- | --- | --- |
| 第一批删除 | **2,592 行**（代码 2,566 + 紧随空行 26） | 27 个函数/方法，见 §3 |
| 第一批后 `main.rs` | **27,327 行** | 29,919 − 2,592 |
| 第二批（删完第一批才变死代码） | **241 行** | `settings_overlay` 177 + `theme_picker` 54 + `wecom_mode`/env/const/字段 10，见 §6.1 |
| 第二批后 `main.rs` | **27,086 行** | |
| `chat_detail_panel` 去留 | +10 / −109 | 需 Chief 拍板，见 §6.2 |
| 锁内 `src/main.rs` 锚点 | 142 条：删 17 / 平移 31 / 不变 94 | 见 §7.1 |
| 文档里 `src/main.rs` 锚点 | 136 处：落在删除区 10 处 | 见 §7.2 |

**关键判断：`right_panel` 那 2,169 行不是「皮肤」。** 它内部是 `bot_details`(426) / `group_details`(36) / `chat_detail_panel`(109) / 会话菜单动作（`apply_detail_menu_action`，295），承载模型选择、审批档位、记忆入口、历史会话、待办、用量。删之前必须逐项确认微信壳有等价物，否则会静默丢功能。逐项核对结果见 §3.4。

## 2. 三块分类

- **A 删除**：唯一调用方都是将被删除的 classic 函数。证据见 §3。
- **B 必须保留**：微信壳（`wechat_*`）或设置页/弹层在调，只是「位置落在 classic 区间里」。证据见 §4。
- **C 第二批**：删了第一批之后才变成死代码，或者需要先定一个功能去留。见 §6。

## 3. 第一批删除清单（逐函数）

调用点证据格式：`行号@所在函数`。同一个符号的全部调用点都已列全（用标识符边界匹配扫的，不是人工挑的）。

### 3.1 classic 外壳骨架与三栏

| 函数 | 行号 | 行数 | 判定 | 调用点证据 |
| --- | --- | --- | --- | --- |
| `classic_shell` | 25925-25959 | 35 | 删 | `shell@25921`（分支入口）；微信壳 `wechat_shell:25961` |
| `titlebar` | 18691-18765 | 75 | 删 | `classic_shell@25932`；微信壳 `wechat_titlebar:26000` |
| `window_button` | 18767-18797 | 31 | 删 | `titlebar@18748/18751/18761`；微信壳 `wechat_window_button:26090` |
| `sidebar` | 18799-19074 | 276 | 删 | `classic_shell@25939`；微信壳 `wechat_nav:26251` |
| `main_bot_card` | 19084-19211 | 128 | 删 | `sidebar@19012`；微信壳无等价（见 §8.2 A12） |
| `unread_badge` | 19213-19221 | 9 | 删 | `sidebar@18869/18898`；微信壳未读在 `wechat_entries:26456/26490`、`wechat_list_row:21824` |
| `chat_header` | 19243-19332 | 90 | 删 | `classic_shell@25948`；微信壳会话头在 `wechat_chat:27595` 起 |
| `transcript` | 19341-19765 | 425 | 删 | `classic_shell@25949`；微信壳 `wechat_transcript:27740`。**唯一要搬走的是首屏欢迎区，见 §5.2** |
| `composer` | 20176-20491 | 316 | 删 | `classic_shell@25954`；微信壳 `wechat_composer:28599` |
| `right_panel` | 20493-20824 | 332 | 删 | `classic_shell@25956`；微信壳 `chat_info_panel:26969` |
| `status_color` | 18683-18689 | 7 | 删 | 唯一调用 `right_panel@20803` |

### 3.2 right_panel 区间之后的辅助函数（本次任务点名的 20 个都在这张表里）

| 函数 | 行号 | 行数 | 判定 | 调用点证据 |
| --- | --- | --- | --- | --- |
| `bot_details` | 20975-21400 | 426 | 删 | `right_panel@20571`。功能点逐条对照见 §3.4 |
| `group_details` | 21402-21437 | 36 | 删 | `right_panel@20588`；群详情在 `chat_info_panel` 群分支 |
| `group_placeholder` | 21439-21460 | 22 | 删 | `transcript@19369` |
| `responder_label` | 21462-21469 | 8 | 删 | `group_details@21427` |
| `short_session` | 21507-21509 | 3 | 删 | `bot_details@20990/20991/21223` |
| `composer_radius` | 21541-21548 | 8 | 删 | `composer@20320`；微信壳输入卡用 `theme::RADIUS_FIELD`（`wechat_composer:28643`）。**连带删单测 `composer_radius_switches_on_multiline_or_attachments`（29536-29546）** |
| `icon_button` | 22542-22569 | 28 | 删 | `right_panel@20624/20646/20684` |
| `day_separator` | 22634-22646 | 13 | 删 | `transcript@19420/19700` |
| `msg_time` | 22648-22659 | 12 | 删 | `message_row@22802/22837`（`message_row` 同时删） |
| `load_earlier_row` | 22748-22770 | 23 | 删 | `transcript@19400/19682`；微信壳有自己的「显示更早」`wechat_row_el:22009` |
| `message_row` | 22774-22876 | 103 | 删 | `transcript@19476/19751`；微信壳行排版在 `wechat_row_el:21969` |
| `quote_button` | 22878-22902 | 25 | 删 | `message_actions@23016`（同时删） |
| `copy_button` | 22904-22920 | 17 | 删 | `message_actions@23017`（同时删） |
| `message_actions` | 23002-23036 | 35 | 删 | `transcript@19480/19755`；微信壳 `wechat_message_actions:22930` |
| `message_attachments` | 23066-23089 | 24 | 删 | `transcript@19441/19724`；微信壳附件视图 `history_attachment_view:23092`（保留） |
| `tool_card` | 25855-25913 | 59 | 删 | `transcript@19408/19691`；微信壳 `wechat_tool_card:25747` |

### 3.3 行数核对

```
27 个函数/方法（26 个符号，status_color 与 titlebar 同属 impl PilotApp）
代码行       2,566
紧随其后的空行  26
合计         2,592
29,919 − 2,592 = 27,327
```

### 3.4 `bot_details` / `right_panel` 的功能去向（删前必读）

这是整份清单里唯一有「删了会少功能」风险的地方，逐项对照：

| classic 里的能力 | 行号 | 微信壳等价物 | 判定 |
| --- | --- | --- | --- |
| 模型 / 引擎 / 状态行 | `bot_details:20985` 起 | `chat_info_panel:27060`（运行参数）、`:27373`（会话用量） | 有等价 |
| 审批档位（ask/auto/full） | `21053`-`21081` + `approval_note:21100` | `permission_menu_overlay:12894`（三档）+ `approval_note:20933`；入口在 `wechat_composer@28733` | 有等价（弹层形式） |
| 记忆行 → `BotMemory` | `21144` id、`21159` 派发 | `chat-info-memory-row`（`:27459`）→ `SettingsDetail::BotMemory`（`:27469`） | 有等价 |
| 历史会话列表（resume） | `21198`-`21223` | `panel-history-{target}`（`:27400`）→ `switch_to_history_session`（`:27411`） | 有等价 |
| 待办 | `21330` `const TODO_ROWS` | `ui::section("待办")`（`:27185`） | 有等价 |
| 群详情 / 群记忆 | `group_details:21402`、`right_panel@20589` | `group_memory_section:16710`（设置页也在用） | 保留函数，删调用 |
| **运行状态只读明细** | `chat_detail_panel:9318` 由 `right_panel@20823` 渲染 | **微信壳没有渲染点**：`chat_detail` 只被 `9318/9420/17460` 读写，`apply_detail_menu_action@17460` 设了它，但 `chat_info_panel` 不读 | **功能缺口，见 §6.3** |

⚠️ 最后一行是实测出来的既有缺陷，不是本次删除引入的：微信壳的「⋯ → 运行状态」点了只改状态、不显示面板。删 `right_panel` 会让它彻底失去渲染点，因此必须先定去留。

## 4. 必须保留（微信壳 / 设置页 / 弹层在用）

这些函数位置落在 classic 区间里，但调用方是共享代码。删错一个就编译失败，或功能静默消失。

| 符号 | 行号 | 微信壳侧调用点（证据） |
| --- | --- | --- |
| `kv_row` | 24531-24575 | 设置页：`settings_page_browser@11980/12074`、`settings_page_im@14795-14854`（8 处）、`settings_page_about@16312/16313` |
| `short_id` | 21537-21539 | `group_task_bar@4047`、`settings_detail_panel@10852/10922`、`open_delete_group_dialog@18609`、`render@2193/2555`、`retry_or_reassign@2132` |
| `count_members` | 21511-21517 | `wechat_entries@26532`、`settings_page_channels@14034` |
| `visible_group_members` | 21520-21527 | `wechat_entries@26532`、`settings_page_channels@14034` |
| `count_bots` | 21529-21535 | `settings_page_im@14708` |
| `is_group_prompt` | 21554-21566 | `wechat_transcript@27886`；单测 `29754-29758` |
| `model_label` | 21471-21476 | `chat_info_panel@26997` |
| `notify_background` | 21479-21493 | `apply_event@5024/5049/5098`、`refresh_runtime_status@4396` |
| `close_or_hide` | 21496-21505 | `wechat_titlebar@26084` |
| `approval_reason_view` | 20854-20931 | `approval_card_el@6725` |
| `approval_note` | 20933-20939 | `permission_menu_overlay@12989` |
| `member_label` | 20942-20955 | `question_card_el@5944`、`elicitation_card_el@6344`、`approval_card_el@6676` |
| `selected_ids` | 20958-20973 | `settings_detail_panel@11276`、`settings_page_bots@13994`、`settings_page_channels@14079/14108`、`apply_model_choice@18537` |
| `bot_index`（方法） | 19334-19339 | `settings_page_local_agents@13582/13688`（classic 那处 `transcript@19508` 随删除消失） |
| `overlay_center` | 22485-22508 | `palette_overlay@18363`、`find_dialog_overlay@18511` |
| `dialog_card` | 22610-22632 | `palette_overlay@18352`、`find_dialog_overlay@18483` |
| `switch` | 22511-22540 | 设置页 11 处（`settings_page_general@12132/12145/12164`、`settings_page_team_memory@15316`、`settings_tool_scope_section@24139/24163` 等） |
| `dialog_field` | 22424-22480 | `settings_page_im@14952` + `render@1375-2890`（10 处） |
| `compose_with_quote` | 22400-22421 | `send_current@5276`、`queue_current@5343`、`send_group_current@5576`、`interject_current@7805` |
| `user_text_with_mentions` | 22702-22746 | `wechat_row_el@22057` |
| `fenced_code` | 23039-23064 | `wechat_message_actions@22976`；单测 `29789-29798` |
| `history_attachment_view` | 23092-23210 | `wechat_row_el@22058` |
| `attachment_chips` | 20014-20105 | `wechat_composer@28660` |
| `error_bar` | 20108-20174 | `wechat_composer@28661` |
| `current_engine_is_mode` | 19768-19776 | `wechat_composer@28619`、`attachments_allowed@19808` |
| `attachments_allowed` / `current_engine_is_acp` | 19801 / 19816 | 附件链路（`stage_attachment_paths@19828`、`start_screenshot_capture@19913`） |
| `stage_attachment_paths` / `stage_image_bytes` | 19827 / 19877 | `wechat_composer@28654/28811` |
| `start_screenshot_capture` / `toggle_voice_typing` / `remove_attachment` | 19912 / 19975 / 19998 | `wechat_composer@28811/28826`、`attachment_chips@20043` |
| `empty_state` / `skeleton_rows` | 24677 / 24744 | `wechat_transcript@27777/27792`（空会话骨架） |
| `kit_markdown_el` | 24842-24900 区 | `wechat_row_el@22111`、`side_panel@8118` |
| `compaction_status_row` | 24722-24740 | `wechat_row_el`（`WecomRow::Compacting` 分支） |
| `tool_copy_button` / `tool_actions_row` | 25101 / 25180 | `wechat_tool_card`、`wechat_tool_group_card@25740` |
| `group_memory_section` | 16710-16836 | `settings_page_team_memory@15372`（classic `right_panel@20589` 那处随删除消失） |

**同名但不同函数的坑**：`composer_radius` 只有 classic 调用点（`composer@20320`），微信壳输入卡用 `theme::RADIUS_FIELD`（`28643`），别因为名字像就留下；反过来 `attachments_allowed` 名字不像外壳函数，却必须留。

## 5. 第一批的连带改动（不含在 2,592 行里）

### 5.1 `shell()` 变成单分支

```rust
// 25917-25923 现状：if self.wecom_mode { wechat_shell } else { classic_shell }
// 改后：直接 self.wechat_shell(window, cx)
```

### 5.2 首屏欢迎区搬进微信壳（这是 classic 唯一明显更优的地方，必须搬）

| 项 | classic 位置 | 搬到哪里 | 落法 |
| --- | --- | --- | --- |
| 欢迎语 + 头像 + 名字 + 角色 | `19535`-`19664`（`transcript` 内） | `wechat_transcript` 的 `chat.messages.is_empty() && chat.ready` 分支（现为 `27794-27795` 的裸 `return div()`） | 复用 `avatar::bot_avatar`，文案沿用 `tri!` 三语；气泡用 `wechat_theme` 的 assistant 气泡色 |
| 三条建议 chip | `19547`-`19580` | 同上 | id 用 `wechat-welcome-suggestion-{i}`；点击 `input.set_content` + 聚焦（低 28px、`RADIUS_TAG`、hover 用 `wechat_theme::nav_item_hover_bg()`） |
| 「今天 时间」小标题 | `19587`-`19601` | 同上 | 直接用 `time::format_hm` |

⚠️ 微信壳已有骨架分支（`27783-27793` 的 `skeleton_rows`）不能动；新欢迎区只替换「`ready` 且 0 条消息」那一个出口。群聊不显示欢迎区（没「当前 Bot」语义），与 `wechat_chat` 的 `is_bot_chat` 判定保持一致。

### 5.3 `BOT_PILOT_CLASSIC_UI` 开关

`wecom_mode` 字段共 5 处引用：`3359`（声明）、`3830`（初始化，读环境变量）、`9040`（`settings_overlay` 反条件）、`12118`（`theme_picker` 反条件）、`25918`（`shell()` 分支）。环境变量 `BOT_PILOT_CLASSIC_UI` 另在 `29291` 读一次（决定是否 `wechat_theme::set_active(true)`），`3358` 是字段注释。

第一批只删 `25918` 那个分支；字段与环境变量留到第二批（§6.1），因为 `9040`/`12118` 两处反向条件必须和字段一起清，否则删了字段编译不过。

## 6. 第二批：删完第一批才变成死代码的部分

### 6.1 明确可删（241 行）

| 项 | 行号 | 行数 | 为什么放第二批 |
| --- | --- | --- | --- |
| `settings_overlay` | 9034-9210 | 177 | 首行 `if self.wecom_mode \|\| !self.settings_open { return None }`，微信壳永不渲染；要先删 `wecom_mode` 才干净 |
| `theme_picker` | 13080-13133 | 54 | 唯一调用 `settings_page_general@12119` 在 `if !self.wecom_mode` 里，随字段一起删 |
| `wecom_mode` 字段 + 初始化 | 3359 / 3830 | 3 | 连带 `9040`、`12118` 两处反向条件（`9040` 那行随 `settings_overlay` 一起消失，`12118` 的 `if` 整块展开） |
| `BOT_PILOT_CLASSIC_UI` 环境变量 | 29291 | 3 | 改成无条件 `wechat_theme::set_active(true)` |
| `SIDEBAR_WIDTH` | 187 | 2 | 只在 `sidebar@18903`、`transcript@19345` 用，删完第一批就无人引用 |
| `active_tab` 字段 | 3445 | 2 | 写点 6 处（`3908/4169/7357/11813/17464`），读点全在 `right_panel`（`20531/20532/20758/20767/20782`）→ 删 `right_panel` 后变成只写不读的 dead field |

⚠️ `PANEL_WIDTH` **不能删**：`chat_info_panel@27525` 和 `set_right_panel@11837/11842` 还在用（微信壳详情栏宽度）。

### 6.2 `chat_detail_panel`（109 行）需要先定去留

```rust
// 唯一渲染点：right_panel@20823（将被删除）
// 唯一写入点：apply_detail_menu_action@17460（DetailMenuAction::RunStatus，微信壳也走这里）
```

三个选项，需 Chief 拍板：

- **选项 1（推荐）**：保留 `chat_detail_panel`，在 `chat_info_panel` 顶部按 `self.chat_detail.is_some()` 渲染一屏「运行状态」卡片。改动小（约 10 行接线），既修掉既有缺陷，也守住「不新增功能」。
- **选项 2**：删 `chat_detail_panel` + 删 `DetailMenuAction::RunStatus` 这个菜单项（`17023`、`17133`、`17180`、`17458`、`23397` 一并清）。净删约 120 行，但用户在「⋯」里会少一项，属于功能回退，需要在 PRD 的「不做」里写明。
- **选项 3**：保持现状（点了没反应），只在 PRD 记一条已知缺陷。**不建议**：现在能用的人少，删完 classic 就再没人发现它坏着。

### 6.3 第二批之后的行数

```
27,327 − 241（§6.1）= 27,086
选项 1 再 +10 行接线 ≈ 27,096；选项 2 再 −109 ≈ 26,977
```

## 7. 第一批之后的锚点影响

### 7.1 指纹锁

锁文件 `docs/parity-anchors.lock` 共 417 条，其中 `src/main.rs` 142 条：

| 分类 | 条数 | 处理 |
| --- | --- | --- |
| 落在删除区间（行没了） | **17** | 必须重指，见 §7.3 |
| 删除区之后（行号要平移） | **31** | `-UpdateAnchors` 自动重算 |
| 删除区之前（不变） | **94** | 不动 |

17 条死锚点里，11 条是 classic 独占代码（`main_bot_card:19086`、`bot_details:20975`、`message_actions:23003`、`bot-details-memory-row:21144` …），6 条是 `transcript` 内部行（`19403`、`19446`、`19480`、`19686`、`19729`、`19755`、`19759`）。

### 7.2 文档锚点

`src/main.rs` 锚点在文档里共 136 处，落在删除区的 10 处：

```
docs/parity-matrix.md                        :28 :29 :33 :34 :42 :43 :98 :210
docs/testing/0.5.1-anchor-label-proposals.md :33 :60
```

### 7.3 重指建议（需要 @产品 复核语义，不是我单方面改）

| 文档位置 | 现锚点 | 建议改成 | 依据 |
| --- | --- | --- | --- |
| `parity-matrix.md:28` A07 Bot 详情页 | `:20975` `bot_details` | `:26969`（`fn chat_info_panel`，main.rs） | 微信壳的详情栏就是它 |
| `:29` A08 记忆行 | `:21144`、`:21159` | `:27459`（`.id("chat-info-memory-row")`）、`:27469`（`SettingsDetail::BotMemory`） | 已实测同意 |
| `:33` A12 主卡位 | `:19086` `main_bot_card` | **无等价** → 建议把 A12 改判 🟡，锚点落到 `:26251`（`wechat_nav`）并注明「微信壳无独立 Main bot 大卡」 | 需要产品决定是否降级，**会动到矩阵 89 这个数** |
| `:34` A13 待办 | `:21330` | `:27185`（`ui::section("待办")`） | 等价 |
| `:42` / `:210` B03 / O04 审批档位 | `:21050`、`:21081` | `:12894`（三档循环）；说明里保留 `approval_note:20933` | 微信壳用弹层实现 |
| `:43` B04 | `:19759`、`:11165` | 保留 `:11165`（`SettingsDetail::Allowlist`）；`19759` 换成 `:21792`（`wechat-row-{}-{}` 行 id）**或**产品另选一个有符号的行 | 这处我倾向由产品按 B04 原文重挑 |
| `:98` F01 历史会话 | `:21198` | `:27399`（`panel-history-{target}`）、`:27411`（`switch_to_history_session`） | 等价 |
| `0.5.1-anchor-label-proposals.md:33/:60` | `:21349`、`:21163` | 这份是历史提案清单，建议在文首加一行「F 区已随 classic 删除，条目作废」，不逐条重指 | 避免为作废清单维护锚点 |

### 7.4 重排执行顺序（关键，顺序错了门禁必红）

1. **先删代码**（`src/main.rs` 一批改完，立刻 `cargo check`）。
2. **再改文档锚点**（§7.3 那 10 处，用 `ReadAllLines` 核对目标行确实含预期符号）。
3. **然后 `-UpdateAnchors`**，必须带 `-Baseline WorkTree`（HEAD 口径会把锁写进临时基线目录，写完就被删）：
   ```
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-parity-anchors.ps1 -Baseline WorkTree -UpdateAnchors
   ```
   预期输出 `anchor lock rewritten: 417 entries (本次可达 364，保留 53 条够不到的)`。
4. **代码 + 文档 + 锁放同一个提交**，然后跑两口径复核：
   ```
   parity -Baseline Head      → drifted=0 unlocked=0
   parity -Baseline WorkTree  → drifted=0 unlocked=0
   ```
5. 六道门禁全绿后再进优化批（`cargo test` / `clippy` / `ui-lint` / `parity` / `arch` / `cargo check`）。

**为什么不能「先 `-UpdateAnchors` 再删」**：锁记录的是「某行上的原文」。函数删掉后那些行号指向别人的代码，指纹会立刻 `drifted`；而 `-UpdateAnchors` 会把**当时**的错误内容记进锁，等于把错误固化成基线。

### 7.5 `ui-lint` 与 `arch` 的连带检查

- `scripts/ui-lint.ps1:43` 按**文件名**白名单放行 `rgb()`，本次不涉及文件改名，不用改。
- `scripts/verify-arch-anchors.ps1` 的 143 条锚点里，`src/main.rs` 的 29 条（`docs/parity-arch-notes.md`）**0 条落在删除区**，但删完要重跑一次确认 `ok=92` 不变；行号平移由 `-UpdateAnchors` 不覆盖它（arch 笔记是另一套脚本校验），若报 `bad>0` 再按实际重指。

## 8. 与 PRD 的边界

- `docs/testing/0.5.1-shell-parity-diff.md` 的 14 项差分**不逐条修**：A 方向 5 项、C 方向 3 项的源都在 classic 里，删掉即消失；B 方向 6 项是微信壳本来就有的，删 classic 后不再构成「差分」。验收口径写「差分清单作废（源已删除）」，不要写成「14 项全部通过」。
- `docs/prd-0.6.md` §2 需写明「不做」：微信壳四栏骨架不动、不新增功能、不引入 Node/TS。
- 微信壳侧要吸收的只有两项：**空态欢迎区 + 建议 chip**（§5.2）；引用条属于优化批，不在删除批。

## 9. 给前端的执行清单（按顺序，每步都要 `cargo check`）

| 步骤 | 内容 | 立即验证 |
| --- | --- | --- |
| 1 | 删 §3.1 十项 + `status_color` | `cargo check --workspace --all-targets` |
| 2 | 删 §3.2 十六项 + `composer_radius` 单测 | 同上 |
| 3 | `shell()` 改单分支；`classic_shell` 删除 | 同上 |
| 4 | §5.2 欢迎区 + 三条 chip 搬进 `wechat_transcript` | 同上 + 截图（空会话、深色/浅色各一） |
| 5 | §7.4 文档锚点 → `-UpdateAnchors` → 同一提交 | `parity` 两口径 |
| 6 | 六道门禁全绿 | 见 §7.4 第 5 条 |

第 4 步做完必须截图留证：没有空态截图，「欢迎区搬过来了」就不算验收通过。
