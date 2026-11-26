# Mode Chat 发布

## 仓库结构

**只有一个仓库**：`eafenzhang/Mode-Chat`（公开）。源码与安装包同仓，客户端的
自动更新清单也从同仓读。

> 历史：2026-11-24 之前是「私有源码库 + 公开更新源库」双仓结构
> （`Mode-Chat` + `Mode-Chat-Releases`）。合并后 `Mode-Chat-Releases`
> **已归档为只读**，仅保留 v0.2.1 及更早版本的资产，供旧客户端升级过渡。
> 归档不影响 Release 下载：已实测 v0.2.1 / v0.2.0 清单仍可匿名获取（HTTP 200）。

## CI（首选）

`.github/workflows/release.yml`：

- **触发：只有 tag `v*`（以及手动 `workflow_dispatch`）。push `main` 不发版。**
  - 2026-11-26 之前是「push `main` 也发版，用 `paths-ignore` 挡文档改动」。
    这条路挡不住一切：v0.6.1 就是「推了一笔 `scripts/shot-probe.ps1`」自动算出
    patch+1 发出去的，成了一版「只有文档和自查脚本变化」的发布。
    `paths-ignore` 是白名单，新目录一出现就漏；而且它表达不了「改了代码但先不发版」。
  - 所以现在是：**发版 = 打 tag**。`git tag v0.x.y && git push chat v0.x.y`。
    push `main` 只跑 `anchor-gate.yml`（锚点门禁），不发版。
- 版本：手动输入 > tag >（手动触发时）HEAD 上已有的 `v*` tag > 最近 tag 的 patch+1
- **版本号必须与 `Cargo.toml` 的 `[package].version` 一致**，不一致时 `Resolve version`
  直接 `exit 1`，不打包。理由：包内版本是打包时注入的 `MODE_BOT_VERSION`，源码版本是
  `Cargo.toml`；两者不一致时「从 tag 重建」会得到另一个版本号的包，这个 release 不可复现。
  v0.6.1 就是这种事故（tag `v0.6.1` → `02bb019`，而那个提交里 `Cargo.toml` 仍是 `0.6.0`，
  包自报 `0.6.1`）——该版本已发布、不撤，由后续版本把版本号对齐。
  读版本必须读 `[package]` 段：`[workspace.package]` 里也有一个 `version`，`head -n1` 会取到它，
  那样这条校验在「两个值碰巧相同」时放行、在「只改了一个」时误拦截，等于没有分辨力。
- 门禁（任一失败直接 exit 1，不进 Package）：
  1. `cargo test --workspace --locked --no-fail-fast`（`--workspace` 才跑全部 crate；`--no-fail-fast` 保证失败时总数仍为 577 而不是随执行顺序变）
  2. `cargo clippy --all-targets --locked -- -D warnings`
  3. `scripts\ui-lint.ps1 -Enforce`（UI 令牌）
  4. `scripts\verify-parity-anchors.ps1`（对标矩阵/PRD 锚点 + 指纹锁 + 汇总数字）
  5. `scripts\verify-arch-anchors.ps1`（架构笔记锚点 + 符号就近）
- 产物：`ModeChat-v<版本>-windows-x64.zip`、`-setup.exe`、`gpui-latest.json`
- 上传：`Mode-Chat` 的 Release（标记 latest）

无需任何 Secret —— `github.token` 就够写本仓 Release。

两个锚点脚本的仓库根从脚本位置反推，CI 检出到 runner 临时目录也能跑。
指出 DS-Bot 基线 / pi-rust checkout 的锚点需要外部树，用
`MODE_PARITY_DS_ROOT` / `MODE_PARITY_RPI_ROOT` 指过去；CI 不设时这些锚点
**跳过并打印 `skipped-external` 计数**，不把「没校验」当成「校验通过」。

## 本地兜底（Actions 不可用时）

```powershell
$env:MAKENSIS = "D:\ModeChat\.release-tools\nsis-3.10\makensis.exe"
node scripts/package-mode-chat.mjs --version=0.2.1 --require-installer
# 产物：dist/mode-chat/{zip,setup,gpui-latest.json}

git tag v0.2.1
git push chat v0.2.1
```

然后在 `Mode-Chat` 建 Release，上传上面三个文件，标记为 latest。

**核对口径（重要）**：CI 在 runner 上重新编译，产物与本地**不是逐字节相同**（v0.5.0 实测：本地 zip `0927cefa…`，发布产物 `dcfff443…`）。所以：

- 本地算出的 zip `sha256` 只能用来验证**打包脚本本身**没问题，不能拿去和 CI 产物比对；
- 发版核对只认**发布产物自身**：Release 里 zip 的 `sha256` == 同一 Release 里 `gpui-latest.json` 的 `platforms.windows-x86_64.sha256`，`size` 同理。

## 更新源约定

- 客户端清单：`https://github.com/eafenzhang/Mode-Chat/releases/latest/download/gpui-latest.json`
- 平台键：`windows-x86_64`（`url` / `sha256` / `size`）
- 安装器：按用户装到 `%LOCALAPPDATA%\Programs\ModeChat`，覆盖安装前 taskkill

## 版本注入

打包脚本以 `MODE_BOT_VERSION=<版本>` 构建，并运行 `--version` 校验；
`update::current_version()` 读同一个构建期变量。

## v0.6.0 的 tag 与 main 有差距（正常善后，不是流程失误）

```
tag  v0.6.0   eef7055   = 发版内容
main          > eef7055  tag 之后又落了若干笔
```

**这里不写死「多几笔」、也不写死「改了哪些目录」**：两个都会变。写「四笔」的时候还是四笔，
补完验收文档就成了五笔；写「只有 docs/scripts」的时候是对的，`d6b1c0a` 之后就不再成立。
**数量、路径集合都是会变的量，性质才是判据。**

性质说明：tag 之后的所有提交都是**发版之后**按反馈补的——发布说明补第 5 条（探针未限 y 区间）、
修那个探针、把第 5 条末句改成事实、验收文档补证据缺口、`release.md` 补这节自身、
`parity-matrix.md` 标题对齐版本号，以及**引用条**（`d6b1c0a`，tag 之后第一笔含 `src/` 的提交，
标题带 `[skip release]`，随 v0.7 发版）。**不是** v0.5.1 那种「先打 tag 后推 docs」造成的意外脱节。

两者在 git 图谱上看起来一样（tag 落后于 main），但一个是**发布后按反馈善后**，
一个是**发版顺序搞错**。看图谱的人需要能分清，否则会以为这一版又出了一次同类事故。
已发布的 tag 不重打，按既定规矩。

**所以判断这一节时不要用「src 是否零改动」当判据**——它只在不含功能提交时成立。
真正该看的是每笔提交的标题和 message：带 `[skip release]` 的就是不随本版发布的内容。

## 版本演进

| 版本 | 更新源 | 说明 |
| --- | --- | --- |
| ≤ 0.2.0 | `Mode-Chat-Releases` | 双仓结构 |
| ≥ 0.2.1 | `Mode-Chat` | 合并为单仓；0.2.1 同时发到旧仓，让老客户端能升上来 |

## v0.5.1 发布说明（发版时直接贴这份）

这一版是 v0.5.0 的缺陷修复与复验版本，**不新增功能范围**。相对 v0.5.0，代码改动集中在六处修复：

| 项 | 问题 | 修复 |
| --- | --- | --- |
| Q-01 / Q-02 | 默认外壳（`wecom_mode`）看不到上下文压缩状态，也没有命令面板入口，只有 classic 外壳有 | 默认壳补压缩状态行与 `composer-plus` 入口 |
| Q-03 | 能力页与设置行残留英文 | 补中文文案 |
| Q-04 | 默认壳消息没有动作条，无法复制正文 / 复制代码 | 补悬停动作条，「复制」「复制代码」实测可用（剪贴板逐字节相同） |
| Q-05 | 默认壳没有「新会话」入口 | 标题栏「+」与命令面板「新会话」实测可用，重启后会话仍在 |
| 对比度 | 右栏按钮对比度 2.3:1 偏低 | 提到 6.08:1 |

**验证情况**：五道门禁（test / clippy / ui-lint / parity / arch）全绿；`cargo test --workspace --locked` 37 个测试目标、576 passed / 0 failed / 2 ignored（2 项需真实浏览器）。对标矩阵 136 项：✅ 89 / 🟡 20 / ❌ 27。

**已知限制**（这些都是设计取舍或已明确降级到 v0.6，不是本版引入的 bug）：

- **飞书 / 钉钉 IM 通道未支持**：入口显示为空态并注明「v0.6.0 计划」，不做假入口。
- **连接器只支持 HTTP 型 MCP**：不含 OAuth 授权流程；需要登录态的连接器请等 v0.6。
- **浏览器登录态**：首次使用需在浏览器面板里重新登录一次，工具不会搬运用户的 Cookie。
- **压缩语义**：`/compact` 只做上下文压缩，**没有** checkpoint / handoff 语义（v0.6 计划）。手动点「立即压缩」时，若上下文还很短会提示「未压缩：上下文还很短」，这是正常跳过。
- **全库搜索是线性扫 JSONL**：没有 FTS 索引，会话很多时会慢（v0.6 计划）。
- **写记忆一定弹审批**：`full` 档也不自动放行 `remember`/`forget`，写记忆属于有副作用的操作，这是有意为之，不是 bug。群聊里同样需要人工确认。
- **跨 Bot 投递**：`message_bot` 最多等 60s；目标还在处理（例如卡在审批卡上）时返回「已投递，稍后它会自己回复」——消息已经送到，不用重发。
- **群成员与私聊共用一份 Bot 记忆**：按 `botId` 存，不按会话形态隔离，与 DS Bot 的 `<home>/memory/bots/<botId>/` 口径一致，属于预期行为。

**v0.6 待办**（本版已实测确认存在，不影响正常使用，故不阻塞发布）：

- **默认壳消息动作条与「↑ 回到顶部」浮钮会重叠**：把会话滚到顶部附近、用户消息的悬停动作条出现在右上时，会和浮钮压在一起。classic 外壳没有这个浮钮，属于默认壳独有。动作条本身只在悬停时短暂出现，功能不受影响；修复需要挪浮钮位置或给动作条加视口内约束，属交互层改动。
- **命令面板「新会话」可能一次点击建出两个空会话**：实测点一次生成了 bot 12 与 bot 13 各一个空会话，相隔 3 秒，**未复现**。只多出两个空会话，已有数据不受影响，原因待排查。
- **引用条未做**：默认壳的 `wecom_composer` 没有引用 UI，消息「引用」功能仍只存在于 classic 外壳。

**关于外部锚点**：`crates/rpi-harness/src/types.rs` 里引用的自动压缩默认值（`reserve_tokens 16384` / `keep_recent_tokens 20000`）来自 pi-rust 上游 checkout，**未在本仓库复核**，CI 中该锚点恒为 `skipped-external`。这不代表已校验通过，只是本仓无法验证。

## v0.5.0 已知限制（保留备查）

发版说明里带上这几条，避免用户按预期外的行为提 bug：

- **飞书 / 钉钉 IM 通道未支持**：入口显示为空态并注明「v0.6.0 计划」，不做假入口。
- **连接器只支持 HTTP 型 MCP**：不含 OAuth 授权流程；需要登录态的连接器请等 v0.6。
- **浏览器登录态**：首次使用需在浏览器面板里重新登录一次，工具不会搬运用户的 Cookie。
- **压缩语义**：`/compact` 只做上下文压缩，**没有** checkpoint / handoff 语义（v0.6 计划）。
  手动点「立即压缩」时，若上下文还很短会提示「未压缩：上下文还很短」，这是正常跳过。
- **全库搜索是线性扫 JSONL**：没有 FTS 索引，会话很多时会慢（v0.6 计划）。
- **写记忆一定弹审批**：`full` 档也不自动放行 `remember`/`forget`，写记忆属于有副作用的操作，
  这是有意为之，不是 bug。
- **跨 Bot 投递**：`message_bot` 最多等 60s；目标还在处理（例如卡在审批卡上）时返回
  「已投递，稍后它会自己回复」——消息已经送到，不用重发。
- **群成员与私聊共用一份 Bot 记忆**：按 `botId` 存，不按会话形态隔离，与 DS Bot 的
  `<home>/memory/bots/<botId>/` 口径一致，属于预期行为。

