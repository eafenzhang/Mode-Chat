# Mode Chat UI 设计规范

> 本文档是**约束文件**，不是建议。新增界面先对照本表，再写代码。
> 最近一次全量审查：2026-11-23（见文末「审查结论」）。

## 1. 为什么要有这份规范

审查前 `src/main.rs` 的真实统计：

| 维度 | 现状 | 问题 |
| --- | --- | --- |
| 字号 | 8 种（10/11/12/13/14/15/17/22） | 10 与 11 混用，同一层级两种字号 |
| 圆角 | 12 种（3/4/5/6/7/8/9/10/12/999…） | `theme::RADIUS_*` 只有 23 处引用，裸 `rounded(px(N))` **141 处** |
| 间距 | gap 10 种（1/2/3/4/5/6/8/10/12/14） | 存在 5、14 这类不在 4 的倍数上 |
| padding | 8 种（2/4/6/8/10/12/14/16） | 同上 |
| 控件 | 131 处走组件，**93 处裸 `cursor_pointer`** | 同样是按钮，圆角/高度/悬停各不相同 |

结论：**设计令牌存在但没有被使用**。规范的第一步是把令牌变成唯一入口。

## 2. 间距（4px 基准）

只用这六档，其余一律不写。

| 令牌 | 值 | 用途 |
| --- | --- | --- |
| `S1` | 4px | 图标与文字、标签与值 |
| `S2` | 8px | 同一行内元素之间 |
| `S3` | 12px | 卡片内边距、列表行左右 |
| `S4` | 16px | 区块之间、页面左右留白 |
| `S5` | 24px | 大区块分隔 |
| `S6` | 32px | 页面顶部/底部的呼吸区 |

废弃：5 / 10 / 14 / 18 等非 4 倍数值。

## 3. 字号（四级）

| 令牌 | 字号 / 行高 | 字重 | 用途 |
| --- | --- | --- | --- |
| `T_Label` | 11 / 16 | 常规 | 辅助说明、状态、路径、时间 |
| `T_Body` | 12 / 18 | 常规 | 列表行标题、正文 |
| `T_Title` | 13 / 18 | 600 | 分区标题、卡片标题 |
| `T_Page` | 15 / 24 | 600 | 页面标题 |
| `T_Metric` | 22 / 28 | 600 | 只用于数字看板 |

废弃：10px（并入 11）、14px（并入 13 或 15）、17px。

## 4. 圆角

| 令牌 | 值 | 用途 |
| --- | --- | --- |
| `R_Tag` | 4 | 标签、徽标 |
| `R_Chip` | 6 | 小按钮、输入框内元素 |
| `R_Field` | 8 | 输入框、列表行、小卡片 |
| `R_Card` | 12 | 卡片、面板 |
| `R_Dialog` | 16 | 对话框、设置外壳 |
| `R_Full` | 999 | 开关、头像、圆形按钮 |

新增 `R_Card = 12`（现有 `RADIUS_CARD = 14` 与 12 并存，统一到 12）。

## 5. 颜色

只用 `theme::` 与 `wechat_theme::` 的语义令牌，禁止硬编码 `rgb()`。

| 分组 | 令牌 |
| --- | --- |
| 背景 | `app` / `panel` / `card` / `raised` / `inset` / `hover` |
| 文字 | `ink` / `ink_secondary` / `ink_tertiary` / `ink_placeholder` |
| 边界 | `edge` / `hairline` / `ink_line` |
| 强调 | `accent` / `accent_soft` / `accent_ink` |
| 语义 | `success` / `danger` / `warning` / `chief` |
| 阴影 | `shadow_1` … `shadow_4` |

双主题（Bot 深色 / 企业微信）通过 `wechat_theme::active()` 分支，**每个组件自己处理**，
调用方不判断皮肤。

## 6. 控件清单

所有控件集中在 `src/ui_kit.rs`，页面只允许调用这层。

### 6.1 容器

| 组件 | 说明 |
| --- | --- |
| `page()` | 页面骨架：纵向排布，`S4` 外边距，区块间距 `S4` |
| `section(title)` | 分区标题 `T_Title` + 上下 `S3` |
| `section_action(title, action)` | 分区标题 + 右侧动作 |
| `card(children)` | 卡片：`R_Card` + `edge` 边框 + `card` 底色，内边距 `S3` |
| `card_row(children)` | 卡片内的分隔行（行间 0.5px `divider`） |
| `grid(cards, width)` | 卡片网格：按可用宽度自动算列数，所有分区共用同一列宽 |

### 6.2 行

| 组件 | 说明 |
| --- | --- |
| `list_row(title, detail)` | 标准行：标题 `T_Body` + 副标题 `T_Label`，右侧可挂动作 |
| `stat_row(label, value)` | 键值行：左标签 `T_Label`，右值 `T_Body` |
| `switch_row(title, detail, on)` | 开关行：整行可点，右侧 `switch` |

### 6.3 卡片

| 组件 | 说明 |
| --- | --- |
| `icon_card(icon, title, detail, badge)` | 带图标的卡片：图标 32 / 标题 `T_Title` / 说明 `T_Label` / 右上角徽标 |
| `stat_card(value, label)` | 数字看板卡：`T_Metric` + `T_Label` |

### 6.4 控件

| 组件 | 说明 |
| --- | --- |
| `button(id, label)` | 文本按钮：30 高、`R_Full`、`hover` 底色 |
| `button_ghost(id, label)` | 轻量文本按钮：无底色，只换文字色 |
| `button_icon(id, icon)` | 图标按钮：28 方角 |
| `chip(id, label, active)` | 筛选 chip：`R_Full`，选中用 `accent_soft` |
| `badge(label, tone)` | 徽标：`R_Tag`，tone = neutral / success / danger / accent |
| `switch(id, on)` | 开关：36×20 |

### 6.5 状态

| 组件 | 说明 |
| --- | --- |
| `empty(text)` | 空态：居中 `T_Label` + `ink_tertiary` |
| `note(text)` | 提示条 |
| `error(text)` | 错误条（`danger_soft` 底） |
| `field(label, input)` | 表单字段：标签 `T_Label` + 输入框 |
| `value_box(value, label)` | 只读值块：用于路径、诊断信息 |

### 6.6 可点击（2026-11-25 新增）

| 组件 | 说明 |
| --- | --- |
| `Clickable::pointer()` | **可点击元素的唯一入口**：trait 扩展，`use crate::ui_kit::Clickable as _;` 后任意 `div()` 链可直接 `.pointer()`。现有 139 处裸 `.cursor_pointer()` 全走这一层，`ui-lint.ps1` 规则 1 强制 |

为什么是 trait 而不是函数：调用方是形状各异的手写 `div()` 链（列表行、图标按钮、分段、徽标），
包成组件会改变布局参数；trait 只接管"鼠标变成手"一个职责，视觉零变化，可逐页替换。

### 6.7 按钮禁用态（2026-11-26 新增，O-7）

| 组件 | 说明 |
| --- | --- |
| `button_state(id, text, disabled)` | `button()` 的完整形态；`disabled == false` 时与 `button()` 完全一致 |
| `button_ghost_state(id, text, disabled)` | `button_ghost()` 的完整形态，同上 |
| `button(id, text)` / `button_ghost(id, text)` | 现有 60 个调用点的入口，等价于 `*_state(..., false)`，**不需要改** |

**为什么是「分支构造」而不是链式 `.disabled(bool)`**：`cursor_pointer()` 和 `hover()` 是在构造时
逐条加在 `Style` 上的，事后追加 `.cursor(Arrow)` / `.hover(|s| s)` 并不能可靠地把它们盖掉
（hover 是叠加的，后加只是又叠一层）。`button_state` 的禁用分支里**根本不构造**这两个方法，
所以不存在"覆盖不掉"的可能。**不要**再往一个已构造好的 `Stateful<Div>` 上补禁用样式。

**用法约束：视觉禁用 ≠ 逻辑禁用。** `*_state` 只负责置灰、去手型、不挂 hover；
**`on_click` 仍会被挂上**。所有调用点必须在 handler 里自己守一次条件，否则会出现
「看起来灰、点了还有反应」。反过来也成立：只写 handler 守卫而不置灰，用户会以为按钮坏了。
两者必须同时做，由同一处 `disabled` 布尔值决定。

### 6.8 工作中行状态短语（2026-11-26 新增，O-3）

转录底部的「工作中 N 秒」不再是一句固定文案，按**本轮最后一条工具消息**所属类别分档：

| `tool_render::ToolFamily` | 短语 | 触发工具（举例） |
| --- | --- | --- |
| `Execute` | 正在执行命令 | `bash` / `powershell` / `shell` / `exec` |
| `Edit` | 正在改文件 | `edit` / `write` / `apply_patch` |
| `Search` | 正在检索 | `grep` / `find` / `glob` / `browser` / `fetch` / `web` |
| `Read` | 正在读文件 | `read` / `ls` / `list` / 带 `file` 的名字 |
| `Cua` | 正在操作界面 | `computer*` / `cua*` |
| `Other` / 缺失 / 空串 | 工作中 | 未知工具、本轮还没调用过工具 |

实现：`main.rs::working_phrase(Option<&str>)`，类别来自 `tool_render::family()`（纯函数，不引 i18n）。
短语在 `i18n.rs` 词表里各有一条英文译文。

三条边界，写清楚以免下一轮又改错：

1. **`browser` 归 `Search`，不单切一类**。`Search` 里已经含 `grep/find/glob/browser/fetch/http/web` 七种，
   为浏览器单切一类会让「搜索」这一档语义变窄，改动面也变大。
2. **`Other` 档不可证伪**——它与旧行为（固定「工作中」）完全相同。验收截图必须是 `Execute` 和
   `Read` 这类可对照的档位，拍两张「工作中」什么也证明不了。
3. **必须先触发过一次工具调用**。`wechat_working_row` 在 `!has_output && retry.is_none()` 时提前返回
   「正在输入」气泡，`has_output` 为假就永远到不了状态行——验收时先让本轮跑过一次工具，否则必然白测。
   `retry` 分支在 `else` 外面，重试中优先于任何工具状态，这一点不要动。

## 7. 页面骨架

### 7.1 设置页

- 左侧导航 220px，右侧内容区
- 内容区左右 `S4`，顶部标题 `T_Page`
- **多级页面走右侧详情栏**，不再弹对话框（见 §8）

### 7.2 列表 → 详情

一级：卡片网格或列表行。
二级：右侧详情栏（`detail_panel`），宽度 360，从右侧滑入，`Esc` 逐级返回。

## 8. 弹窗 vs 详情栏

审查发现 **24 个 `open_alert_dialog` + 15 个 `pending_*` 延迟弹窗状态**。
规则：

| 场景 | 形态 |
| --- | --- |
| 破坏性确认（删除） | 保留居中对话框，必须二次确认 |
| 单行输入（改密钥、改地址） | 改为详情栏内的内联表单 |
| 多字段表单（新建 Bot / 群聊 / 定时任务） | 改为详情栏 |
| 选择类（选模型、选运行时） | 改为详情栏 |
| 只读信息（运行状态、任务历史） | 改为详情栏 |

## 9. 动效

统一 `theme::motion_base()` + `theme::ease_out`，进场 150ms 淡入 + 上移 6px。
详情栏从右侧滑入（位移 24px + 淡入）。

## 9.1 空会话欢迎区（2026-11-26 新增，O-1）

`PilotApp::wechat_welcome`（`src/main.rs`，微信壳唯一实现）。classic 外壳删除后，空会话的
「头像 + 欢迎语 + 建议 chip」只有这一处，v0.6.0 起不再有第二套等价物。

结构（纵向居中，`S4` 段距，左右 `S6` 呼吸区）：

| 元素 | 令牌 | 说明 |
| --- | --- | --- |
| 头像 56×56 | `Self::wechat_avatar` | 与列表行同源，不新建头像组件 |
| 名字 | `T_Page` / 600 / `wechat_theme::chat_title()` | |
| 角色 | `T_Label` / `chat_subtitle()` | 空角色整行不渲染 |
| 欢迎气泡 | `R_Chip` + `bubble_in()` 底 + `bubble_text()` 字 / `T_Title` / 行高 `T_TITLE_LH + 6` / `max_w 460` / 内边距 14×10 | 形状照聊天气泡；**14/10 不是令牌值**，与现有气泡的 10×6 也不一致，属 O-5 间距批的待收敛项（届时统一到 `S3×S2` 或把气泡内边距也纳入令牌） |
| 引导语 | `T_Label` / `chat_subtitle()` | 「今天想先推进哪件事？」 |
| 建议 chip ×3 | 高 28、`R_Tag`、`composer_border()` 边、`T_Body`、`S2` 间距、`S3` 左右内边距 | id `wechat-welcome-suggestion-{i}` |

交互约定：

- **点 chip 把文案填进输入框并聚焦**（`set_content` + `focus_handle().focus()`），不直接发送。
  理由：用户经常要改一两个字再发，直接发送等于替他决定。
- **群聊不渲染**：微信壳没有「当前 Bot」这层语义，群会话走骨架/空白由会话自身状态决定。
- `chat.ready == false` 时仍走 `skeleton_rows()`（骨架），与欢迎区互斥。

可复用边界：这段只属于「空会话首屏」，**不要抽成通用卡片**。气泡的 `max_w 460` 与
第三行的引导语都是首屏专属；抽成组件后每个页面都要带一份无用参数。

## 9.2 右栏运行状态高亮（2026-11-26 新增，O-2）

「⋯ → 运行状态」不再渲染第二张统计卡（那张卡的 10 行与右栏基础卡完全重复），
改为**高亮右栏基础卡**：`chat_detail = Some(RunStatus{key})` 只作高亮标记，
`chat_info_panel` 在 `key` 与当前会话匹配时，给基础卡包一层 `R_Card` 圆角 +
`wechat_theme::accent()` 描边，并在外面挂一行 `ui::section("运行状态")` 与 id `chat-detail-run`。

**为什么必须高亮而不能只是「打开右栏」**：右栏本来就开着时，只打开面板等于什么都没发生，
用户照样觉得「点了没反应」——那正是 O-2 要修的原始缺陷。

**为什么必须带标题**：只有一圈绿边，用户读不出自己在看什么。
把判据放宽成「0 处标题」等于认可「有反应但说不出反应什么」，缺陷只是换了形态。

**高亮是瞬时 UI 状态，不跨会话、不落盘。** `chat_detail` 现在有五个清空点，
**新增任何「打开右栏」或「切会话」的入口，必须同时补一处清空**：

| 入口 | 位置 |
| --- | --- |
| 切 Bot | `select_bot` |
| 切群 | `select_group` |
| 折叠右栏 | `set_right_panel(open=false)` 的 `else` 分支 |
| 点「改名」跳设置页 | 改名按钮 handler |
| 切历史会话 | `switch_to_history_session` |

漏一处的表现是：切走再切回来，描边又亮起，像这个会话有某种状态。
`chat_detail` 不进磁盘（`save_ui_prefs` 只存 `muted / topmost / pinned_settings / right_panel_open`），
所以重启不会带出描边。

**同一个「瞬时 UI 状态」还有 `chat.quote`（引用）**：它目前全仓没有写入点，
v0.7 做引用条时必须同时补写入点、渲染点、以及同样这五处清空点——只做渲染等于做一个永远为空的控件。

## 10. 审查结论（2026-11-23）

需要改造的页面，按「裸样式密度」排序：

| 页面 | 裸 text_size | 裸 color | 裸 gap | 组件数 | 结论 |
| --- | --- | --- | --- | --- | --- |
| 连接器 | 4 | 4 | 7 | 2 | 卡片已成型，改用 `icon_card` |
| 用量 | 4 | 4 | 7 | 11 | 对齐 DS 看板样式 |
| 通用 | 0 | 0 | 0 | 27 | 已合规，作为样板 |
| IM频道 | 1 | 1 | 6 | 33 | 子页面改详情栏 |
| 定时任务 | 1 | 1 | 1 | 4 | 补组件化 |
| 能力 | 1 | 1 | 0 | 2 | 用 `badge` 替状态文字 |
| 智能体 | 1 | 1 | 0 | 10 | 改卡片 |
| 插件 | 0 | 0 | 0 | 4 | 改卡片 |

全局待办（2026-11-24 复核）：

| # | 项 | 状态 |
| --- | --- | --- |
| 1 | 128 处裸 `rounded(px(N))` → `theme::RADIUS_*` | ✅ 已完成（剩 1 处动态值 `size * 0.24`，按头像等比缩放，豁免） |
| 2 | 141 处裸 `cursor_pointer` → 组件（2026-11-25 `scripts/ui-lint.ps1 -Enforce` 复测：**0** 处） | ✅ 已完成（`ui_kit::Clickable::pointer()` 接管，139 处全改完） |
| 3 | 24 个弹窗按 §8 分类迁移 | ✅ 已完成（只剩 6 个：5 个删除确认 + 1 个插件开关确认，按 §8 保留） |

### 10.1 图标库（2026-11-24 决定）

通用 UI 图标最终选用 **Lucide**（ISC，纯线框，无面型变体）。

演进过程与结论：

| 阶段 | 库 | 问题 / 结论 |
| --- | --- | --- |
| 初版 | 手绘 24 网格 stroke | 覆盖面不足，缺品牌标 |
| 第二次 | Phosphor | 实心填充 + 方角外框，21px 下显厚重（用户反馈「过于方正」） |
| 第三次 | Remix Icon | 2px 描边、转角圆润，21 对菜单图标 line/fill 成对 |
| 第四次 | Boxicons | MIT、圆润度 35%，1884 组 line/filled 成对 |
| **现在** | **Lucide** | ISC、圆润度 8%（87 个候选库中最低＝最圆）、复杂度 39 命令、**纯线框无面型变体** |

为什么最终选 Lucide：用户明确要求「不要面型图标」。Lucide 是全部候选里唯一
同时满足「三项指标都最优 + 零面型变体」的库。代价是菜单选中态不能再用
「线框→实心」切换，改为靠 `text_color` 区分（accent 色）。

关键教训：**换库时名义尺寸不能照搬**。Phosphor 的 256 viewBox 图形铺满画布
（约占 95%），旧手绘 24 网格只有约 70%，同样写 24px 会大出 20~25%。
Lucide 是 24 网格、stroke-width 2，图形约占 85%，与旧版相当。

选中态：Lucide 无实心变体，`menu_icon` 的 `solid` 参数保留但不再影响资产，
选中由调用方传的 `color` 表达。新增测试 `every_icon_is_stroked_not_filled`
断言所有资产都含 `stroke` 与 `fill="none"`，防止以后误混面型图标。

图标族分工（不要混用）：

| 用途 | 来源 | 说明 |
| --- | --- | --- |
| 通用 UI（侧栏、菜单、按钮） | Remix Icon | 线框为主 |
| 连接器品牌标 | simple-icons / mdi / logos | 统一压成单色 `currentColor`，按品牌色染 |
| IM 频道 | TDesign | 手机端官方标 |
| 模型厂商 | LobeHub | 全彩，走 `img()` 而非 `svg()`（GPUI 的 svg 会拍平多色） |
| 智能体引擎 | LobeHub | 官方单色矢量 |

### 10.2 详情面板与窗口宽度联动

规范补充（2026-11-24）：

- 侧栏 / 设置详情面板都是**真正的 flex 列**，不是浮层——浮层会盖住主内容右侧按钮。
- 窗口贴着最小宽度（900）时，打开面板要把**窗口整体加宽**（会话栏 +320、
  设置面板 +340），其它模块宽度不变；收起时还原。窗口已被手动拉宽时不撑窗口。
- 设置面板的开合检测放在 `render` 顶部按"上一帧是否显示"比较，因为
  `settings_detail` 有 60+ 处赋值点，逐处加钩子必然遗漏。
- 改窗口尺寸必须走 `window.defer`：在 `render` 内直接 resize 会破坏当前帧布局。

---

## 11. 令牌门禁（2026-11-25 新增）

`scripts/ui-lint.ps1` 把 §2/§3/§4/§5 的令牌要求变成可执行检查，四条规则：

| # | 规则 | 只允许出现在 | 当前违规 |
| --- | --- | --- | --- |
| 1 | `.cursor_pointer()` | `src/ui_kit.rs` | **0**（已全部改用 `ui_kit::Clickable::pointer()`） |
| 2 | `.rounded(px(<数字字面量>))` | `src/ui_kit.rs`、`src/theme.rs` | **0**（令牌参数与 `size * 0.24` 等比头像不算违规） |
| 3 | `text_size(px(<数字字面量>))` | `N` 必须是 `T_LABEL/T_BODY/T_TITLE/T_PAGE/T_METRIC` 的值（11/12/13/15/22） | **0**（残留 10/12.5/14/17 全并入令牌） |
| 4 | `rgb(...)` | 主题与图标着色白名单文件 | **0**（5 处换 `theme::accent_ink()` / `wechat_theme::list_title_selected()`） |

四项合计 **0 处**，`-Enforce` 已可用于 CI 阻断（2026-11-25 达成）。

**第 5 条（间距）已实现**（`spacing_offtoken`，`9822319`），但**默认不拦门禁**：它由独立开关 `-EnforceSpacing` 控制，`-Enforce` 的行为一字未改（仍只统计前四条）。当前 `main.rs` 里仍有非 4 倍数值的间距字面量，按用法拆开（`main.rs` 实测，2026-11-26）：
按用法拆开（`main.rs` 实测，2026-11-26）：

| 字面量 | 字面量合计 | 间距属性内 | 非间距属性（不检） |
| --- | --- | --- | --- |
| `px(14.)` | 37 | **29** | 8（`line_height` 2、`size` 4、`w` 1、`right` 1） |
| `px(10.)` | 53 | **52** | 1（`size`） |

**第 5 条只检「间距属性」，不是检所有 `px(N.)` 字面量。** 白名单：

```
间距（检）：.px .py .pt .pb .pl .pr .p .mx .my .mt .mb .ml .mr .gap
不检（非间距）：line_height / size / w / h / max_w / min_w / left / right / top / bottom
```

`line_height(px(14.))` 有 2 处（`main.rs`，随字号档变化），归**排版档**不归间距档：
把它当间距清掉会让中文行高从 14 变成 12 或 16，直接影响可读性。同理 `.size(px(14.))`
是图标尺寸、`.w(px(14.))` 是元素宽度，都不受 `spacing_offtoken` 管。

`px(0.)`（`min_w`/`min_h` 一类容器约束）也明确放行，不算视觉间距。

**所以 O-5 的基线要按「间距属性内」这一列报，不是按字面量合计**——否则
「37 处全清」会把行高和图标尺寸一起改坏。O-5 按 PRD §5.2 四步做：
① 加规则（六档白名单，`px(0.)` 放行）→ ② report-only 报基线 → ③ 逐页清零 → ④ 切 `-Enforce`。
**在 ① 之前，`-Enforce` 通过只证明旧 4 条干净，不代表间距合规。**

用法：

```powershell
# 报告模式（默认，不改退出码；收敛期间用）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1

# 门禁模式（CI 用，有违规即 exit 1）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1 -Enforce

# 间距档单独收紧（O-5 第 ④ 步之后才用；现在跑会红，因为基线还有 232 处）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1 -EnforceSpacing
```

**三个开关的结论互不相同，结项说明里必须写清楚**（2026-11-26 补，O-5 ①）：

| 命令 | 现在的结果 | 含义 |
| --- | --- | --- |
| `-Enforce` | PASS / exit 0 | 前四条令牌规则 0 违规 |
| `-EnforceSpacing` | **FAIL / exit 1** | 间距档还有 232 处档外，**不代表阶段没通过** |
| 两个都带 | FAIL / exit 1 | 前四条干净但间距没清零 |

**只看 `-Enforce` 的 PASS 就说「间距合规」是错的**：第 5 条不在它的统计范围内。
反过来，跑了 `-EnforceSpacing` 看到红也不能当作回归——那是 O-5 ③ 尚未开始。

**第 5 条自身的有效性靠增量验，不靠退出码**（2026-11-26 实测）：
基线 232 → 临时注入一处 `gap(px(6.))` → 233 → 删掉 → 232。两次都要记数。
`exit 1` 在「抓到 232 条」和「抓到 1 条」时是同一个值，分不开。

收敛完成后把 release.yml 的调用改成 `-Enforce`，违规即阻断发布。

### 11.1 字号 AC 的正确口径（2026-11-25 修 U-06）

**旧口径是错的，不要再用**：`grep -c 'text_size(px(' src/*.rs` → 「仅 `ui_kit.rs` 令牌定义」。
这条判据无法区分合规与违规——`text_size(px(12.))` 合规、`text_size(px(14.))` 违规，
两者都被 `text_size(px(` 命中。实际 `src/` 全仓 332 处，其中数字字面量 225 处，
照字面永远不可能「仅 `ui_kit.rs`」，AC 永远不通过，等于没有 AC。

**正确口径**：**非令牌字面量 = 0**。判定是「括号里**是否数字字面量**」× 「该数字**是否等于令牌值**」：

```
合规  text_size(px(T_LABEL))        px(T_*) 令牌
合规  text_size(px(font_size))      变量
合规  text_size(px(12.))            字面量，但 12 是 T_BODY 的值
违规  text_size(px(14.))            字面量，且 14 不在令牌表里
```

与 `scripts/ui-lint.ps1` 第 3 条规则完全一致（`$FontTokens = @('11.','12.','13.','15.','22.')`），
一条命令即验：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1 -Enforce
# 期望：[OK  ] text_size(px(non-token))   0
```

实测基线（2026-11-25，HEAD `205b7e0`）：332 处 `text_size(px(`，其中非令牌字面量 **0**，
`ui-lint -Enforce` 四项合计 0 违规。**不要再按 `grep -c` 的旧口径判定这条 AC。**

**`-Enforce` 与 `-EnforceSpacing` 是两个开关，名字像、结论相反，务必分清**：

```
-Enforce           只统计前四条（cursor_pointer / rounded_px / text_size_offtoken / rgb）
                   当前 total violations: 0 → EXIT=0，六道门禁用的就是它
-EnforceSpacing    只统计第 5 条（spacing_offtoken）
                   当前 232 处 → EXIT=1，它现在是红的（间距尚未清零，这是事实不是缺陷）
```

**判据可证伪**：`232` 不是硬编码常量，已做 +1 自测——临时注入一处 `gap(px(6.))` 后变 `233`、删除后回到 `232`、`git diff` 为空。**只看到 `PASS` 或只看到 `FAIL` 都不算验过，要看数字随注入变化。**

**发布后若有人跑 `-EnforceSpacing` 报红，那不是阶段 3 没通过**——O-5 ③（232 处逐页清零）已单独立项，不进 v0.6.0 的「已完成」。

