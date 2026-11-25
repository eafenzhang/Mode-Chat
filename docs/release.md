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

- 触发：push `main` / tag `v*` / 手动
- 版本：手动输入 > tag > **HEAD 上已有的 `v*` tag** > 最近 tag 的 patch+1
  （push `main` 和 push tag 会各触发一次，`concurrency` 只保证串行不保证合并；
  所以「HEAD 已带 tag 就用那个版本」这一步必须有，否则同一次发版会多出一个空 patch Release）
- 门禁（任一失败直接 exit 1，不进 Package）：
  1. `cargo test --locked`
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

## 更新源约定

- 客户端清单：`https://github.com/eafenzhang/Mode-Chat/releases/latest/download/gpui-latest.json`
- 平台键：`windows-x86_64`（`url` / `sha256` / `size`）
- 安装器：按用户装到 `%LOCALAPPDATA%\Programs\ModeChat`，覆盖安装前 taskkill

## 版本注入

打包脚本以 `MODE_BOT_VERSION=<版本>` 构建，并运行 `--version` 校验；
`update::current_version()` 读同一个构建期变量。

## 版本演进

| 版本 | 更新源 | 说明 |
| --- | --- | --- |
| ≤ 0.2.0 | `Mode-Chat-Releases` | 双仓结构 |
| ≥ 0.2.1 | `Mode-Chat` | 合并为单仓；0.2.1 同时发到旧仓，让老客户端能升上来 |

## v0.5.0 已知限制

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

