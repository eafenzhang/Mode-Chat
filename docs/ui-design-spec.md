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

只用 `theme::` 与 `wecom_theme::` 的语义令牌，禁止硬编码 `rgb()`。

| 分组 | 令牌 |
| --- | --- |
| 背景 | `app` / `panel` / `card` / `raised` / `inset` / `hover` |
| 文字 | `ink` / `ink_secondary` / `ink_tertiary` / `ink_placeholder` |
| 边界 | `edge` / `hairline` / `ink_line` |
| 强调 | `accent` / `accent_soft` / `accent_ink` |
| 语义 | `success` / `danger` / `warning` / `chief` |
| 阴影 | `shadow_1` … `shadow_4` |

双主题（Bot 深色 / 企业微信）通过 `wecom_theme::active()` 分支，**每个组件自己处理**，
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
| 4 | `rgb(...)` | 主题与图标着色白名单文件 | **0**（5 处换 `theme::accent_ink()` / `wecom_theme::list_title_selected()`） |

四项合计 **0 处**，`-Enforce` 已可用于 CI 阻断（2026-11-25 达成）。

用法：

```powershell
# 报告模式（默认，不改退出码；收敛期间用）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1

# 门禁模式（CI 用，有违规即 exit 1）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ui-lint.ps1 -Enforce
```

收敛完成后把 release.yml 的调用改成 `-Enforce`，违规即阻断发布。

### 11.1 字号 AC 的正确口径（2026-11-25 修 U-06）

**旧口径是错的，不要再用**：`grep -c 'text_size(px(' src/*.rs` → 「仅 `ui_kit.rs` 令牌定义」。
这条判据无法区分合规与违规——`text_size(px(12.))` 合规、`text_size(px(14.))` 违规，
两者都被 `text_size(px(` 命中。实际 `src/` 全仓 330 处：225 处数字字面量 + 22 处 `px(T_*)`
+ 83 处变量，照字面永远不可能「仅 `ui_kit.rs`」，AC 永远不通过，等于没有 AC。

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

实测基线（2026-11-25，HEAD `2aeacce`）：330 处 `text_size(px(`，其中非令牌字面量 **0**，
`ui-lint -Enforce` 四项合计 0 违规。**不要再按 `grep -c` 的旧口径判定这条 AC。**
