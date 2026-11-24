# Mode Chat 发布

## 仓库结构

**只有一个仓库**：`eafenzhang/Mode-Chat`（公开）。源码与安装包同仓，客户端的
自动更新清单也从同仓读。

> 历史：2026-11-24 之前是「私有源码库 + 公开更新源库」双仓结构
> （`Mode-Chat` + `Mode-Chat-Releases`）。合并后 `Mode-Chat-Releases` 仅保留
> 历史版本供老客户端过渡，不再更新。

## CI（首选）

`.github/workflows/release.yml`：

- 触发：push `main` / tag `v*` / 手动
- 版本：手动输入 > tag > 最近 tag 的 patch+1
- 门禁：`cargo test --locked` + `cargo clippy --all-targets --locked -- -D warnings`
- 产物：`ModeChat-v<版本>-windows-x64.zip`、`-setup.exe`、`gpui-latest.json`
- 上传：`Mode-Chat` 的 Release（标记 latest）

无需任何 Secret —— `github.token` 就够写本仓 Release。

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
