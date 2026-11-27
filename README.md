# Mode Chat

Rust + GPUI 桌面客户端，Mode Bot 的新一代外壳（替代 WebView 版本）。

单二进制、无 Node / TypeScript 运行时；对话引擎内嵌
[pi-rust](https://github.com/bigfish1913/pi-rust)。

## 下载

安装包在 [Releases](https://github.com/eafenzhang/Mode-Chat/releases/latest)：

| 文件 | 说明 |
| --- | --- |
| `ModeChat-v<版本>-setup.exe` | 安装版，按用户装到 `%LOCALAPPDATA%\Programs\ModeChat` |
| `ModeChat-v<版本>-windows-x64.zip` | 免安装，解压后直接跑 `ModeChat.exe` |

应用内「设置 → 关于」可检查更新，更新源就是本仓库的 Release。

## 能力

- **会话** — 私聊 / 群聊 / IM 频道，右侧常驻信息栏（引擎、模型、用量、待办、本轮改动）
- **插话（Steering）** — 回合进行中 `Ctrl+Enter` 把文字推进引擎的 steering 队列，而不是排队等下一轮；普通 `Enter` 仍是排队。仅内嵌引擎支持，外部 ACP 会话退化为排队
- **旁路会话（`/side`）** — 一次性会话，不写主 transcript、不进历史与全局搜索，落盘在 `mode-chat-sessions/.side`
- **隐私 Bot** — 会话单独落在 `mode-chat-sessions/.private/<botId>`，不进全局搜索、不能进群、不能当团队管理员
- **团队管理员（Chief）** — 全局唯一（新建与编辑都会校验），在群里额外持有团队工具：`team_roster`、`team_goal_show|create|stage`、`team_task_list`、`team_assign`。一轮最多派发 4 个任务
- **定时任务** — cron 触发的自动化，跨 Bot 汇总
- **连接器** — GitHub / Notion / Slack / Linear / Postgres 等 MCP 连接器

## 构建

```powershell
cargo build --release          # 产物 target/release/bot-gpui-pilot.exe
cargo test                     # 152 个测试
cargo clippy --all-targets --locked -- -D warnings
```

打包（需要 NSIS）：

```powershell
$env:MAKENSIS = "D:\ModeChat\.release-tools\nsis-3.10\makensis.exe"
node scripts/package-mode-chat.mjs --version=0.2.1 --require-installer
```

## 数据

数据根与 Mode Bot 相同：`~/.mode-bot/.mode`（名册、群聊、会话、凭据、自动化）。
用 `BOT_PILOT_DATA_ROOT`（数据根）和 `BOT_PILOT_STATE_FILE`（单实例锁与窗口状态）可隔离一个测试实例。

## 目录结构

```
src/         GPUI 应用（Windows）
crates/      纯 Rust 服务（名册/身份、群聊/记忆/IM、自动化、连接器、凭据）
assets/      图标与字体
examples/    开发用探针
docs/        设计规范与发布说明
```

## 文档

- [UI 设计规范](docs/ui-design-spec.md) — 组件层、间距/字号/圆角令牌、弹窗 vs 详情栏规则
- [发布说明](docs/release.md) — CI 流程、本地兜底打包、更新源约定
- [试点笔记](docs/pilot-notes.md) — 早期实现记录

## 许可

MIT，见 [LICENSE](LICENSE)。第三方组件与图标许可见 [NOTICE.md](NOTICE.md)。
