# Bot GPUI 外壳（Rust + GPUI 最终形态的第一步）

产品方向已定：**Rust + GPUI 为最终目标**。这个 crate 不再只是一次性评估，而是逐步长成
Bot 的新外壳：自绘标题栏 / 左栏真实名册 / 中栏按 Bot 隔离的会话 / 右栏真实资料。

> **2026-11-15：WebView UI 已退役，本 crate 是 Mode Bot 唯一 UI。** 安装包自包含
> `agent/node/config` 运行时；旧 WebView 安装不再收到更新，数据根同源。
> 已知能力缺口：IM 扫码/编辑与 **IM 运行时**；浏览器/电脑操控的 workspace 开关可用、
> Agent 插件工具可驱动，GPUI 独立控制面板未移植。定时任务已完整可用。

**不接 Node Host sidecar**：直接复用 Mode Bot 壳同一套 `agent-process`（N1 控制面），
走 `createSession → session/subscribe → session/send → model.streaming`。

## 运行

从仓库根目录（`D:\Mode Bot\Mode`）执行，PowerShell：

```powershell
$env:MODE_BUILTIN_PROVIDER_CONFIG_FILE = "D:/Mode Bot/Mode/config/provider/mode-builtin.json"
$env:MODE_STORAGE_DIR = "$HOME/.mode-bot-pilot/.mode/cli"          # 隔离会话库（与产品同结构）
$env:MODE_SESSION_DB_PATH = "$HOME/.mode-bot-pilot/.mode/cli/db/db.sqlite"
$env:BOT_PILOT_WORKSPACE = "$HOME/mode-bot-ws"     # 可选：Bot 目录为空时的兜底
$env:BOT_PILOT_DATA_ROOT = "$HOME/.mode-bot/.mode" # 可选：名册数据根；默认同产品
cargo run --manifest-path apps/bot-gpui-pilot/Cargo.toml
```

两种自动模式（用于无人值守验收）：

| 变量 | 行为 |
| --- | --- |
| `BOT_PILOT_SELFTEST=1` | 连接后自动发「只回复两个字：收到」，打印增量与 `turn.completed`，然后退出（0=成功） |
| `BOT_PILOT_DEMO=1` | 连接后自动发同一条消息，但窗口保留，用于截图/人工看中文渲染 |

实测记录（2026-11-13）：

```text
[selftest] prompt accepted
收到
[selftest] turn completed
exit=0
```

窗口工作集约 55MB（同机 ModeBot 约 125MB；未计入 agent 子进程）。

## 结构

```
src/main.rs        窗口外壳：32px 自绘标题栏、260px 左栏（Bot/频道）、会话区、300px 右栏
src/data.rs        真实名册（只读）：teammates.json + groups.json → Bot/Group/ModelSpec
src/markdown.rs    极简 markdown 解析（手写 ~200 行 + 单测）：标题/段落/代码块/列表/引用/分隔线
                   + 行内粗体/斜体/行内代码/链接
src/highlight.rs   极简代码高亮（手写 tokenizer + 单测）：js/ts、rust、python、bash、json
                   的关键字/字符串/注释/数字/大写类型着色
src/state.rs       试点会话映射 botId → sessionId（落 ~/.mode-bot-pilot/state.json）
src/backend.rs     后台线程 + tokio：多会话控制面（私聊 + 频道成员会话），事件带成员归属
src/text_input.rs  gpui 官方 input 示例的移植版（EntityInputHandler / IME 组合输入 / 选区 / 剪贴板）
src/theme.rs       Bot 深色 token（与 tokens.css 对齐）
```

- 独立嵌套 workspace：`Cargo.toml` 末尾的 `[workspace]` 让试点自带 lock/target，
  **不并入主 workspace**，避免 CI 的 `clippy --workspace` 和非 Windows 平台被连带编译。
- UI 框架统一用 `gpui-kit =0.10`（内部固定 `gpui-pre =0.3.12` + `gpui-pre-platform`）；
  依赖名仍叫 `gpui`，现有 `use gpui::...` 通过 kit facade 解析。Markdown 由
  `gpui-kit` 的 `TextView` 负责（GFM/表格/代码块/选择复制）。

## 已验证

- 外壳：自绘标题栏拖拽/最小化/关闭、双击标题栏最大化、左栏选中切换、右栏三标签切换；
  窗口按钮已用 Win32 实测（`IsIconic` / `IsZoomed` / 进程退出），关闭即退出进程。

- 真实名册（2026-11-13）：左栏来自 `teammates.json`（娜娜/小白）与 `groups.json`
  （发布小组 · 2 位成员）；右栏模型/最近会话/工作目录/说明均来自名册；频道显示成员、
  管理员、回应策略、指令、团队记忆；点击频道时 composer 换为「v4 群聊未接入」提示条。
- 多会话隔离：每个 Bot 用 `workspaceIdentity = mode-bot:teammate:<id>` 建独立会话；
  实测娜娜（`sess_48fec5b6…`）与小白（`sess_8b91b9ca…`）各自持有气泡，来回切换不串。
- 真实发送：在小白会话里键入 `hello xiaobai`（SendKeys）+ Enter，收到中文流式回复；
  会话 id 与娜娜不同，状态回到「就绪」。
- 人格来自工作区：团队成员的身份块由主程序写进 `{directory}/AGENTS.md`（`identitySync`）；
  小白未开启身份同步，所以回复自称来自工作区而非「小白」——这是产品语义，不是试点缺陷。
- 会话续用（2026-11-13）：Ready 时把 sessionId 写进 `~/.mode-bot-pilot/state.json`；
  重启先 `session/resume` 再读快照历史（跳过 synthetic 用户消息，取 text part，最多 80 条），
  实测重启后上一轮「收到」重新上屏、sessionId 不变、新消息接着同一会话。
- 审批闭环（2026-11-13）：权限卡来自 v4 会话流 `permission.requested`（toolName/reason/
  riskLevel/options），作答走 `resolveInteraction`（optionId）；实测「新建 tmp-approval-check/
  test.txt」连续弹出 Bash(high) 与 Write(medium) 两张卡，allow_once 两次后文件写入成功、
  卡片自动清掉、状态回「就绪」。
- 审批策略（2026-11-13）：直接复用产品纯逻辑 `bot-team::approval::decide`
  （ask 全问；auto 只放行只读；full 放行读写但 Bash/MCP 仍问；allowlist 精确匹配非
  Bash/MCP）。实测娜娜（full）的 Write 自动放行（日志 `[ui] auto 放行 Write`），
  Bash 仍弹卡；小白（ask）全弹。
- **会话模式修正**：`Auto` 是保留模式（shell 工具直接报 `Auto mode is reserved but not
  implemented yet`）；新建与恢复都收敛到 `Build`（恢复后补一次 `session/setMode`）。
- 工具活动卡（2026-11-13）：`tool.updated` 六种 kind 映射成一张卡按 `call_id`
  原地更新（scheduled/started→排队/运行，result→完成+输出预览，error→失败+报错，
  progress→运行+输出预览，batch 不渲染）；历史快照的 `ToolPart` 也回放成卡。
  实测「用 shell 看 hello.txt」：流式文本 → `⚙ Bash 完成 hello from 娜娜` → 最终回答；
  重启后卡与文本顺序保持。流式增量落点用 `streaming_index` 固定，工具卡插入不再截断增量。
- Markdown 渲染（2026-11-13 深夜）：助手气泡改走块级渲染——`StyledText` + `HighlightStyle`
  行内富文本（粗体/斜体/行内代码底色/链接下划线），标题加粗分级、列表带符号或序号、
  代码块独立盒（Consolas + 语言标签）、引用带左条、分隔线；用户气泡保持纯文本。
  解析器手写（不引 pulldown-cmark，避免版本 API 漂移），带 4 个单测；
  流式半截 markdown 也能边流边渲。GPUI 的 div 文本本身软折行，气泡不再需要硬换行。
- AskUserQuestion 闭环（2026-11-14 凌晨）：`interaction/requestUserInput` 反向请求由
  自写 handler 接住——解析 `questions[0]`（问题 + options[value,label]，无 questions 时回落
  `prompt`），转发成问题卡后持有（Deferred）；作答走 v4 `resolveInteraction`
  `{action:"accept", content:{answers:{问题:答案}, answer_0, answer}}`，取消走
  `action:"cancel"`。v4 `userInput.requested` 事件作为兜底问题源（简版 prompt+choices）。
- **关键语义**：AskUserQuestion/ExitPlanMode 的 permission.requested 只是等待态标记，
  **不能代答也不能弹卡**（弹了 allow 反而把 CLI 带进「已拒绝」路径）；必须屏蔽，
  等问题卡作答时同一个 resolveInteraction 把 permission 一起收掉。
  实测：问题卡（绿框 + A 写代码/B 休息/取消）→ 点 A → 工具卡「完成」（含答案回显）→
  Bot 按选择继续回复。
- **ACP elicitation 表单卡（2026-11-15）**：initialize 声明 `clientCapabilities.elicitation.form`；
  `elicitation/create`（form）按 `requestedSchema` 渲染文本 / 单选（`enum|oneOf|anyOf`）/
  多选（`array`）/ 布尔字段，`required` 未填时提交提示；提交回
  `{action:"accept", content:{...}}`，取消回 `{action:"decline"}`。Hermes 改走 `hermes acp`，
  与 pi-acp / deepseek-acp / zcode-acp-server 共用这条链路。
- CJK 渲染：用户/助手气泡里的中文正常显示（demo 截图）。
- 输入框：点击聚焦、光标/选区/剪贴板/发送（Enter）走官方 `EntityInputHandler` 路径。
- 多行：Shift+Enter 插入换行，↑/↓ 按视觉行移动，输入卡随行数增高（最多 7 行可见）。
- **编辑器滚动（2026-11-14）**：行数超出可视高度时自动滚屏保持光标可见
  （`compute_scroll` 纯函数，prepaint 按本帧行数/光标行算偏移），滚轮自由浏览
  （`on_scroll_wheel`），鼠标命中/IME 坐标同步减去偏移。
- **输入框软折行（2026-11-14）**：几何层从逻辑行改成视觉行——`wrap_segments` 用
  `request_measured_layout`（measure 闭包里按 `known/available width` + 上一帧字号）逐逻辑行
  shape 后按字形宽度切行（优先行尾空白断行，放不下按字符断，至少推进一个字符防死循环）；
  `LineGeom` 加 `hard_break`，↑/↓/光标 x/鼠标命中/选区/IME 下划线全部按视觉行。
  实测：粘贴 150+ 全角字符无换行长文本 → 卡片内折 3 行、光标跟随末行、↑ 在折行中间行间移动。
  已知取舍：Home/End 仍按逻辑行；宽度变化时高度滞后一帧（measured layout 每帧重算，观感无差）。

## 实现坑（GPUI）

- 应用层 `bind_keys` 只负责匹配按键；动作最终派发给焦点元素的 `on_action`。
  新增动作（如 Newline/Up/Down）必须同时在 `TextInput` 根节点注册 `on_action`，否则按键静默丢弃。
- Shift+Enter 另在 Send 处理器里用 `window.modifiers().shift` 兜底插入换行，避免绑定层差异。

## 已知限制（WebView 退役后）

- **IME**：2026-11-15 自动化全矩阵通过（8 项 + 双拼 N/A，证据
  `.acceptance/2026-11-15/ime-automated/`）；仍建议真人按 runbook §3 抽检签字。
- **WebView 已退役（2026-11-15，用户拍板跳过门禁）**：没有 WebView 兜底；
  IM 运行时/扫码编辑未移植；浏览器/电脑操控开关可写、Agent 插件可驱动，独立控制面板未移植。
- **定时任务**：设置页可管理定义/启停/历史；GPUI 内置 20 秒调度器，已有任务与
  「立即运行」会执行；CronCreate 对话内新建也已打通。
- **IM**：设置页只读汇总配置；运行时未移植，机器人不会实际收发。
- 长会话只渲染最近 120 条（「显示更早」按批放开），未做完整虚拟化；自动更新
  未接 minisign；窗口无 DWM 投影（Win10，gpui 内部 wndproc 不可改）。
- 频道历史回放（2026-11-14）：群成员会话也进试点状态文件（槽位
  `group:<gid><member_id>` → sessionId，Ready 时落盘）；重启先逐成员
  `session/resume`，历史按 `MessageTime.created` 跨成员归并排序（带成员标签，上限 80 条）
  一次打回 UI。实测：重启后点开发布小组，用户消息与娜娜的回复按时间上屏、状态
  「已恢复历史」。空会话 CLI 不落库（`Session not found`），按设计回落新建。
- 频道未对齐项：
  群审批/问答的成员归属显示未做（卡照常工作，路由按 requestId 已通）；
  频道记忆/团队记忆/goal 注入留空（群记忆在 groups.json 里的已随 prompt 注入）。

- 代码块「复制 [lang]」按钮写入剪贴板（点击命中的自动化验证转人工）；
  不支持图片/HTML；工具卡折叠推迟（需先保留完整输出）；不支持附件/自动替换更新/无障碍全量。

## IME 人工验收步骤

1. 点输入框，切换微软拼音，输入「你好」，观察候选框位置与上屏文字；
2. 组合态中用 Esc 取消，确认输入框恢复；
3. 上屏后 Shift+Enter 换行，再 Enter 发送，确认多行输入卡增高与气泡内容；
4. 记录：候选框是否贴光标、组合下划线是否显示、回车是否误发送。

## 能力清单（2026-11-14，十九批）

| 面 | 能力 |
| --- | --- |
| 名册 | 真实 teammates.json / groups.json；策略（ask/auto/full）解析与**档位切换**；**审批白名单增删**；**模型选择**（读 provider_config.json 的已配置 provider）；搜索过滤 |
| 会话 | 私聊/频道按身份隔离多会话；resume 续用 + 历史回放（文本/工具卡/成员标签） |
| 交互 | 流式回复；工具活动卡；审批（人工卡 + 策略自动放行）；AskUserQuestion 问题卡；**引用回复**（消息旁「引用」→ composer 引用条 → 发送时拼 markdown 引用块） |
| 会话库 | **历史消息全文搜索**（Ctrl+F，输入即查、命中按会话去重、显示归属/时间/片段，点击或 Tab+Enter 跳到该会话）：只搜 `part` 的正文（`$.type` 为 text/markdown，思维链与工具输入不参与），按 rowid 倒序早停取 8×N 再按会话去重；查询在后台线程 + 120ms 防抖，1.3GB 真机库从「单次 2–11 秒」降到 50–300ms |
| 会话库 | **待办面板**（✓/●/○ + 高优先标记，最多 4 条 + 余量提示）与**用量**（回合数、总 tokens、首 token 延迟）：只读打开 `MODE_SESSION_DB_PATH` 的 `todo`/`turn_usage` 表 |
| 内容 | markdown 块级/行内；五族语言代码高亮；软折行 + 滚动编辑器；多行输入；消息时间戳 + 日期分隔（今天/昨天/日期） |
| 频道 | **新建/编辑/归档/删除频道**（成员复选、管理员、回应策略、规则、记忆）；@点名路由；**@ 提及候选面板**（输入 `@` 弹出成员 +「所有人」，Tab/Shift+Tab 选择、Enter 插入、Esc 收起，也可点击；只在群聊出现）；管理员接话；群规则/群记忆/团队记忆/goal 注入 |
| 主题 | **深色 / 浅色双主题**（设置 → 外观切换，落盘 `prefs.json` 下次沿用；深色取 Bot tokens，浅色取 DS DeepSeek 浅色调） |
| 键盘 | **⋯ 菜单可键盘操作**：`↑`/`↓` 移动高亮、`Enter` 执行、`Esc` 关闭（鼠标与键盘共用同一套动作与条目）；**历史搜索可键盘选择**：`Tab`/`Shift+Tab` 移动命中、`Enter` 跳转（↑/↓ 会被输入框吃掉，与命令面板同因）；关闭后焦点自动回输入框——顺带修掉「浮层消失→焦点悬空→之后打字全丢」；菜单打开期间 `Enter` 不会误发消息 |
| 视觉 | 输入卡焦点描边改 ink 30%（DS `focus-within`），发送键不可用时图标改三级色（原来白箭头落在浅灰底上几乎看不见） |
| 视觉 | 对齐 DS 规范第七批（**顶部胶囊 + tab**）：顶部胶囊可点击开合右侧面板（悬停淡入箭头，开=‹ 关=›；DS §10.10 的「宽 0→18」改为保留 18px 占位只淡入，避免胶囊宽度跳动）；右栏 tab 对齐 `.bt-tabs`（13/20、4×8、间距 2、未选中透明、选中 hover 底） |
| 外壳 | **窗口几何自校准**：gpui-windows 的 border offset 换算在本机 DWM 下不对称（每次重启窗口上移 4px）；首帧实测「请求 - 实际」并存 `offset_x/offset_y`，恢复时加回，两次重启 client 原点一致 |
| 视觉 | 对齐 DS 规范第六批（**自绘图标 + 组件与反馈**）：字形符号全换 `icons.rs` 自绘图标（20 画布 / 1.5 描边 / 同族线宽：搜索、＋、⋯、⚙、×、✓、发送、停止）；开关统一为 36×20 胶囊组件（身份同步等）；**Toast**（底部居中、2.5s 自动消失，接主题切换/置顶/归档/白名单）；对话框标题 15/20/600；**空态**按 §10.9 重做（居中、不套卡片框、Bot/频道头像做锚点）；**加载骨架**（三条气泡占位余弦呼吸，骨架色浅/深各一档，减少动效时静态） |
| 视觉 | 对齐 DS 规范第五批（**字体与组件**）：字号收敛到 11/16·12/16·13/18·14/24 四档（去掉 10px 与 16px）；圆角按档位（气泡 18、对话框 16、侧栏行 10、分段项 8、标签 4、按钮胶囊）；主/危险按钮文字改 `accent_ink`（浅色下白字对比）；右栏图标按钮去描边改悬停底；菜单项 30/13/px8 |
| 视觉 | 对齐 DS 规范第四批（细节）：工具卡改**单行 chip**（图标 + 工具名 + 状态 + 等宽预览，与助手气泡同缩进、预览与名字相同不重复）；引用回复改为纯文本格式（不再显示 `>`/`**` 原文）；输入提示精简 |
| 视觉 | 对齐 DS 规范第三批（细节）：消息时间/「引用」默认隐身、悬停该条才出现；composer 左侧「+」开命令面板；对话框淡入改用轻度过冲弹簧曲线（180ms） |
| 视觉 | 对齐 DS 规范第二批：侧栏**主 Bot 大卡**（95px/头像 60/身份 chip/未读角标）、行尾**未读徽标**、群聊头部**成员头像堆叠**、用户消息里的 **@提及 chip**、对话框 140ms 淡入 |
| 视觉 | 对齐 DS 规范第一批：气泡 14/24 无描边（Bot 侧 `BUBBLE_BOT`、用户侧 `BUBBLE_USER`）、宽 560；消息左侧 22px 头像 + 角色色名字；侧栏行 44/头像 36/圆角 12；右栏宽 320、头像 64、名字 17/500；聊天头部胶囊 40/头像 24；输入框胶囊 22；transcript 间距 12 |
| 智能体 | 设置 →「智能体」：探测本机 CLI 引擎（Mode Agent / PI / pi-acp / DeepSeek ACP / Hermes / ZCode ACP / Claude / Codex / Gemini），显示 已接入 / 已检测未接入 / 未安装 与路径；只有已接入适配层的引擎可点「选用」（其余仅展示，避免静默回落 Mode）。**引擎本体暂不改造**（用户 2026-11-14 拍板）：设置 →「智能体」可为**每个 Bot** 选引擎（Mode Agent / pi-acp / deepseek-acp / zcode-acp-server / hermes），选择存 `state.json.engines`，切换时自动重建会话；`BOT_PILOT_ENGINE=...` 仍作为未单独设置时的全局默认。pi-acp / deepseek-acp / zcode / **Hermes（`hermes acp`）** 都走 ACP（文本流、工具卡、Stop、`session/load` 续聊、`session/request_permission` 人审卡、`elicitation/create` 表单卡）；重启续聊与切换历史在本地引擎下均可用，历史重放帧不进入当前气泡；`zcode-acp-server` 裸跑是 TUI，启动时自动补 `server` 子命令。附件只在 Mode Agent 下可见可用；ACP 引擎的权限/表单请求已接现有卡片。 |
| 设置 | **S1 已落地（P4-9）**：设置窗口 920×自适应高（左导航 198 + 分隔线 + 右页滚动，打开即夺焦点、关闭回输入框）+ 15 页——通用（深浅外观/数据路径/团队记忆编辑）、模型（只读 provider_config）、**插件（workspace `enabledPlugins` 列表 + 开关写回 config.json）**、智能体（探测 + 每 Bot 选引擎）、Bot（列表/新建/编辑/恢复隐藏）、频道（列表/新建/编辑/隐藏与归档恢复）、**连接器（16 项目录/启停/内联密钥，与主程序共用 `connectors-service`）**、**IM（只读汇总 `bot-config.v3.json`：通道/绑定用户/工作区/凭据登记 + 派生状态；运行时/扫码/编辑未移植）**、定时任务（跨 Bot 汇总 `tasks-index.sqlite`：对话内 CronCreate 新建、列表/启停/立即排队（GPUI 调度器 ~20 秒认领）/运行历史/删除）、**密钥（插件 `${secret:NAME}` 密钥库 `v2/secrets.json`：列表/新增/二次确认删除，值只写不读）**、**能力（壳真实广告的 N6 能力集：群任务树/上下文包/静默运行已接线、电脑面板未接线）**、用量（当前会话 + 全库累计）、关于（版本/更新探测）、反馈（诊断/复制/发布页）、**更多（未移植清单：浏览器/电脑操控、插件市场、连接器 OAuth、IM 管理/运行时 + 数据目录入口）**。**S2 已完成**：prefs.json 偏好（旧 theme.txt 自动迁移）、浏览器/电脑插件开关写 workspace `.mode/config.json`（只改 `plugins.enabledPlugins`，原子写 + 二次确认，文件不存在不新建）、**团队记忆编辑**（设置 → 通用：列表/新增/编辑/删除/撤销，保留 kind/end/pin）；**S3**：六页全部落地（IM 为只读汇总，运行态/扫码/编辑未移植） |
| 更新 | 关于页检查更新 → `platforms.windows-x86_64`（url/sha256/size）→ 下载解压 → PowerShell 等待退出 → 备份 → 覆盖 → 重启，失败回滚；开发构建拒绝自替换；清单/zip 由打包脚本产出 |
| 外壳 | 自绘标题栏窗口控制；**托盘**（`Shell_NotifyIconW` 自研：左键唤起、右键菜单显示/退出）；新建 Bot 原子落盘；⚙ 设置窗口；左栏搜索；命令面板 |
| 验收 | selftest 无人值守链路；80 个单测（解析器/只读会话库/窗口几何/更新/定时任务/密钥/能力/连接器/IM/ACP/字号门禁）；窗口控制 Win32 实测 |

## 快捷键与无障碍底线（P4-6）

| 键 | 行为 |
| --- | --- |
| `Enter` / `Shift+Enter` | 发送 / 换行（仅输入框聚焦时） |
| `Esc` | 从最上层开始关一层：⋯菜单 → 删除确认 → 新建/编辑 → 设置 → 搜索（同时清空搜索词） |
| `Ctrl+K` | 展开左栏搜索并聚焦 |
| `Ctrl+N` | 给当前 Bot 开新会话（与右栏「新会话」同路径；生成中不打断） |
| `Ctrl+,` | 打开设置信息面板 |
| `Alt+↓` / `Alt+↑` | 在左栏条目间移动（Bot 与频道连成一个环，自动建会话） |
| `Alt+A` / `Alt+D` | 批准 / 拒绝当前审批卡（A 优先 allowOnce/allowAlways，不接受只读类之外的猜测） |
| `Ctrl+B` / `Ctrl+E` | 新建 Bot / 编辑当前 Bot |
| `Ctrl+G` | 新建频道（成员默认全选，可点选调整） |
| `Ctrl+F` | 历史消息全文搜索（输入即查；点击命中跳转到该会话） |
| `Ctrl+P` | 命令面板（输入过滤；`Tab`/`Shift+Tab` 选择、`Enter` 执行、`Esc` 关闭） |
| `Enter`（对话框内） | 提交当前对话框（频道对话框；Esc 取消） |
| `Tab` / `Shift+Tab` | 在可聚焦控件间移动（侧栏「+」、顶部胶囊、composer「+」/输入框/发送、右栏 ⚙/⋯/三个分页），焦点用强调色描边/淡底标出 |
| `Enter`（控件上） | 激活当前聚焦控件（Tab 键上下文，不会误发消息） |
| `↑` / `↓` / `Enter`（⋯ 菜单） | 移动高亮 / 执行当前项（菜单打开时焦点在菜单上，不用鼠标） |
| `Tab` / `Shift+Tab` / `Enter`（搜索历史） | 移动命中 / 跳转（输入框里 `↑`/`↓` 会被文本编辑吃掉，所以用 Tab） |

- **启动即聚焦输入框**：gpui 只把按键派发给「有焦点元素」的派发路径，没有焦点时字符输入与
  全局快捷键**都会静默丢失**；首帧把焦点交给输入框，之后焦点归用户。
- **焦点环**：输入卡聚焦时描边换成 ink 30%（与 DS `[data-composer-card]:focus-within` 一致；accent 描边留给真正的输入框）。
- **最小字号**：源码内 `text_size` 不得低于 10px，由单测 `a11y_tests::no_text_below_floor` 把门。
- **Tab 焦点链已做（核心控件级）**：固定句柄集合（侧栏「+」、顶部胶囊、composer
  「+」/输入框/发送、右栏 ⚙/⋯/三个分页），`Tab`/`Shift+Tab` 循环、`Enter` 激活、
  焦点用强调色描边/淡底标出；菜单/搜索/@候选等浮层仍在各自上下文里处理 Tab。
  对话框内按钮级 Tab 未做（对话框有 Enter 提交 + Esc 取消）。

## 已知限制（2026-11-15 更新）

- **滚动可用，但要窗口处于前台**：`overflow_y_scroll` 在本版本工作正常（右栏与设置对话框
  内容都能滚）；此前两次「滚不动」都是测试时窗口未激活 / 坐标偏差，不是 gpui 缺陷。
  即便如此，右栏仍按「少滚动」设计：历史默认 2 条 + 「还有 N 条较早会话」提示、
  身份标签移到头部芯片。
- **对话框结构**：固定标题 + 可滚内容（`max_h = 视口高 - 96px`）+ 固定底栏，
  最小窗口 900×600 下也能看到标题与「关闭」。
- Tab 焦点链已做（核心控件级：侧栏 +、顶部胶囊、composer 三件、右栏 ⚙/⋯/三分页）；
  对话框内按钮级 Tab 未做（对话框有 Enter 提交 + Esc 取消）。
- **IME**：代码侧组合态守卫已加（组合中 Enter/Esc/Backspace/方向键不让位给发送/关浮层）。
  2026-11-15 用真实微软拼音完成自动化全矩阵：先用 `WM_IME_CONTROL` 把前台窗口切到
  中文 NATIVE，再逐键输入；候选框定位、空格上屏、组合态 Enter/Esc、中英混输、Shift
  切换、60 字长句、选区替换均通过，双拼因本机未启用记 N/A。证据与脚本见
  `.acceptance/2026-11-15/ime-automated/`；门禁仍建议真人抽检签字。
- 窗口几何持久化已做（位置/大小/最大化，5 秒轮询落盘 `window.json`）。
- **窗口无 DWM 投影**：样式已含 CAPTION/THICKFRAME（gpui 自绘标题栏），要投影需在
  wndproc 里处理 `WM_NCCALCSIZE`，wndproc 在 gpui 内部——本试点不改，宁缺勿滥。
- 任务栏/Alt-Tab 图标已设（随包嵌入 `bot-tauri/icons/icon.ico`，首帧写到试点目录再加载）。
- 托盘已补（`src/tray.rs`：左键唤起 + 右键「显示主窗口/退出」）；最小化到托盘未做
  （拦 `SC_MINIMIZE` 需要主窗口 wndproc，在 gpui 内部）。
- 自动更新：清单平台包 + SHA-256 + 下载/解压 + 备份/替换/回滚已做；开发构建默认拒绝自替换；**未接 minisign 签名**（需发布私钥 secret），当前依赖 HTTPS + SHA-256。NSIS 安装器已出，CI 腿已接（本地按 CI 原命令演练通过：exe 21.63 / zip 45.43 / setup 45.43MiB，含 agent/node/config 运行时；清单哈希自洽，`--version` 注入校验通过；runner 首跑待下一次 tag）。更新端点用 GPUI 专用的 `gpui-latest.json`，不与壳的 `latest.json` 混用；**2026-11-15 自包含 E2E**：0.0.98 → 0.0.99 下载/校验/替换运行时/重启成功，重启后再查为「已是最新」，替换脚本带文件锁重试与失败回退，证据 `.acceptance/2026-11-15/update-e2e-runtime/`；NSIS 静默装/卸实测通过，安装后在无主程序环境也能用 bundled 运行时建会话（证据 `retire-ui/installer.txt`）。
- **v4 会话面（默认）**：私聊/频道统一走 v4 命令面（createSession → subscribe →
  sendText），历史由快照 rows 回放，切换历史会话也走 v4 订阅；**附件**（私聊、
  单文件 ≤20MiB，上传分片 384KiB + sha256）、审批卡/问题卡（pending_interactions →
  现有卡片，作答走 v4 resolveInteraction）、频道群聊（成员会话 v4 + 群 prompt
  sendText，事件带成员归属）都只在这条链路上。**legacy 会话面已于 2026-11-14 删除**
  （按用户指示「直接删」），只剩 v4 一条实现，没有回落开关。

## 改造进度（2026-11-15）

| 阶段 | 状态 | 内容 |
| --- | --- | --- |
| P0 落库门禁 | ✅ | 试点入库、clippy 零告警、验证 bin 移入 examples、主仓 CI 排除登记 |
| P1 数据面 | ✅ | 编辑/删除 Bot（原子写）、身份同步（AGENTS.md 受管块 + persona.json）、群历史原话、Stop 打断 |
| P2 内容层 | ✅ | GFM 表格、嵌套列表、可见滚动条、代码块复制、markdown 解析缓存 |
| P3 外壳 | ✅ | 新会话 + 历史会话切换 resume、右栏「⋯」菜单（置顶/隐藏/编辑/删除 + 设置恢复）、群卡成员归属、浏览器/电脑操控只读分页、托盘；IME 代码侧守卫已加，人工全矩阵待补 |
| P4 产品级 | ✅ | 会话库迁移工具、单实例锁 + 崩溃落盘、zip + **NSIS 安装器**与体积对账、**自更新（SHA-256 + 替换/回滚）**、无障碍底线（快捷键/焦点/Tab 焦点链/字号门禁）、浅色+深色双主题、右栏审批档位切换、设置窗口 15 页、v4 会话面翻默认并删除 legacy 单链路 |
| L3.1 定时任务页 | ✅ | 设置 →「定时任务」：跨 Bot 列表（状态/调度/下次运行/错误）、启停、立即排队（只写队列；GPUI 尚未内置调度器）、运行历史、二次确认删除；复用 `automation-repo`，与壳同一个 `tasks-index.sqlite` |
| P4-12 本地智能体 | ✅ | ACP（pi-acp / deepseek-acp / zcode-acp-server / **hermes acp**）、每 Bot 选引擎、人审权限卡、`session/load` 续聊、**elicitation 表单卡（文本/单选/多选/布尔，required 校验）** |
| P5 切换退役 | ✅ 2026-11-15（用户拍板跳过门禁） | WebView 构建产物已退役，GPUI 唯一 UI、自包含运行时、运行时更新带锁重试/失败回退；IME 自动化 8 项 + 双拼 N/A；一周稳定性仍在观察（质量建议）。已知缺口：IM 运行时/扫码编辑、浏览器/电脑操控独立面板（开关/Agent 插件可用）；定时任务已完整（CronCreate 新建 + 内置调度器执行） |
| 性能基线 | ✅ | 长会话：**窗口化渲染**（最近 120 条 + 「显示更早」），500 条合成历史下 release 渲染 <8ms/帧（未窗口化时 10–18ms）；调试用 `BOT_PILOT_FAKE_HISTORY=N` 造合成历史、`BOT_PILOT_DEBUG=1` 打 `[perf] render` |
| 性能基线 | ✅ | 启动到首帧 **87–89ms**（release）／407–501ms（debug）：`BOT_PILOT_DEBUG=1` 会打印 `[boot] first frame in N ms` |

**验证入口**：`cargo test --release`（86 单测）/ `cargo run --example panic-probe`（崩溃落盘）/
`node scripts/gpui-stability-report.mjs --since=<观察开始日> --gate`（一周稳定性）/
`cargo run --example migrate-sessions`（会话库迁移，默认演练）/
`node scripts/package-bot-gpui.mjs [--version=<v>] [--require-installer] [--resources=<ship>] [--skip-runtime]`（打包 zip + NSIS + `gpui-latest.json`（sha256）+ 体积对账；
传 `--version` 时构建注入 `MODE_BOT_VERSION`，并用 `bot-gpui-pilot.exe --version` 校验；
静默安装 `setup.exe /S`，静默卸载 `uninstall.exe /S`；Git Bash 下手动执行要加
`MSYS_NO_PATHCONV=1`，否则 `/S` 会被改写成路径）。

## 改造计划

产品化路线图见主仓 `docs/plans/2026-11-14-gpui-productize.md`
（P0 落库门禁 → P1 数据面收口 → P2 内容层 → P3 外壳完备 → P4 产品级横切面 → P5 切换退役，
含 5 个待拍板决策点 D1–D5 与各阶段验收标准）。
