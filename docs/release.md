# Mode Chat 发版

## CI（首选）

`.github/workflows/release.yml`：

- 触发：push `main` / tag `v*` / 手动；
- 版本：手动输入 > tag > 最近 tag 的 patch+1；
- 门禁：`cargo test --locked` + `cargo clippy --all-targets --locked -- -D warnings`；
- 产物：`ModeChat-v<版本>-windows-x64.zip`、`-setup.exe`、`gpui-latest.json`；
- 上传：私库 `Mode-Chat` Release + 公开更新源 `Mode-Chat-Releases`。

前置：

1. GitHub Actions 额度可用（当前账号在旧仓库也因账单出现「0 step、2 秒失败」）；
2. 仓库 Secret `UPDATE_REPO_TOKEN`：能写 `eafenzhang/Mode-Chat-Releases` 的 PAT。
   未配置时工作流会跳过公开更新源，只在私库出 Release。

## 本地兜底（Actions 不可用时）

```powershell
node scripts/package-mode-chat.mjs --version=0.1.0 --require-installer
# 产物：dist/mode-chat/{zip,setup,gpui-latest.json}

git tag v0.1.0
git push chat v0.1.0
```

然后在 **Mode-Chat** 与 **Mode-Chat-Releases** 两个仓库分别建同名 Release，
上传 `ModeChat-v0.1.0-windows-x64.zip`、`ModeChat-v0.1.0-setup.exe`、`gpui-latest.json`；
`Mode-Chat-Releases` 的那份标记为 latest，客户端才会看到更新。

## 更新源约定

- 客户端清单：`https://github.com/eafenzhang/Mode-Chat-Releases/releases/latest/download/gpui-latest.json`
- 平台键：`windows-x86_64`（`url` / `sha256` / `size`）
- 安装器：按用户安装到 `%LOCALAPPDATA%\Programs\ModeChat`，覆盖安装前 taskkill。

## 版本注入

打包脚本以 `MODE_BOT_VERSION=<版本>` 构建，并运行 `--version` 校验；
`update::current_version()` 读同一个构建期变量。
