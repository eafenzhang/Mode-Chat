#!/usr/bin/env node
/**
 * Mode Chat 打包（单一 exe，无 Node/TS 运行时）。
 *
 * 产物（dist/mode-chat/）：
 *   - `ModeChat-v<版本>-windows-x64.zip`：`ModeChat.exe` + `README.md`
 *   - `ModeChat-v<版本>-setup.exe`：NSIS 按用户安装（%LOCALAPPDATA%\Programs\ModeChat）
 *   - `gpui-latest.json`：更新清单（指向 Mode-Chat 的 zip + sha256/size）
 *
 * 用法：
 *   node scripts/package-mode-chat.mjs
 *   node scripts/package-mode-chat.mjs --version=0.1.0 --require-installer
 *   node scripts/package-mode-chat.mjs --skip-build
 */
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const distDir = join(repoRoot, "dist", "mode-chat");
const exeName = "ModeChat.exe";
const cargoBin = "bot-gpui-pilot";

const args = process.argv.slice(2);
const skipBuild = args.includes("--skip-build");
const requireInstaller = args.includes("--require-installer");
const versionArg = args.find((arg) => arg.startsWith("--version="));

function packageVersion() {
  const cargo = readFileSync(join(repoRoot, "Cargo.toml"), "utf8");
  const match = cargo.match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error("Cargo.toml 缺少 [package].version");
  return match[1];
}

const version = versionArg?.split("=")[1]?.trim() || packageVersion();
if (!/^[0-9A-Za-z._-]+$/.test(version)) {
  console.error(`[package] 版本号非法：${version}`);
  process.exit(2);
}
const mib = (bytes) => `${(bytes / (1024 * 1024)).toFixed(2)} MiB`;

function build() {
  console.log(`[package] 构建 release（MODE_BOT_VERSION=${version}）…`);
  const result = spawnSync(
    "cargo",
    ["build", "--release", "--bin", cargoBin],
    {
      cwd: repoRoot,
      stdio: "inherit",
      shell: process.platform === "win32",
      env: { ...process.env, MODE_BOT_VERSION: version },
    },
  );
  if (result.status !== 0) {
    console.error("[package] 构建失败");
    process.exit(result.status ?? 1);
  }
}

function archive(sourceDir, zipPath) {
  rmSync(zipPath, { force: true });
  execFileSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      `Compress-Archive -Path '${sourceDir}\\*' -DestinationPath '${zipPath}' -Force`,
    ],
    { stdio: "inherit" },
  );
}

function findMakensis() {
  const candidates = [
    process.env.MAKENSIS,
    join(process.env.LOCALAPPDATA ?? "", "tauri", "NSIS", "makensis.exe"),
    "C:\\Program Files (x86)\\NSIS\\makensis.exe",
  ].filter(Boolean);
  return candidates.find((path) => existsSync(path)) ?? null;
}

function makeInstaller(stageDir, setupPath) {
  const makensis = findMakensis();
  if (!makensis) {
    console.warn("[package] 未找到 makensis，跳过安装器（设 MAKENSIS=<路径> 可指定）");
    return false;
  }
  const nsiPath = join(distDir, "installer.nsi");
  const escape = (value) => value.replace(/"/g, "'");
  const appIcon = join(repoRoot, "assets", "icons", "app.ico");
  const versionQuad = [...version.split("."), "0", "0", "0", "0"].slice(0, 4).join(".");
  const script =
    "\uFEFF" +
    `Unicode true
SetCompressor /SOLID lzma
!include "MUI2.nsh"

!define APP_NAME "Mode Chat"
!define APP_EXE "${exeName}"
!define MUI_ICON "${escape(appIcon)}"
!define MUI_UNICON "${escape(appIcon)}"
!define MUI_WELCOMEPAGE_TITLE "安装 Mode Chat ${version}"
!define MUI_WELCOMEPAGE_TEXT "Mode Chat 是 Rust + GPUI 桌面客户端，内置对话引擎。$\\r$\\n$\\r$\\n安装程序会先卸载旧版本，再按用户安装到 %LOCALAPPDATA%。"
!define MUI_FINISHPAGE_TITLE "Mode Chat 安装完成"
Name "\${APP_NAME}"
OutFile "${escape(setupPath)}"
BrandingText "Mode Chat ${version}"
InstallDir "$LOCALAPPDATA\\Programs\\ModeChat"
InstallDirRegKey HKCU "Software\\ModeChat" "InstallDir"
RequestExecutionLevel user
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "${versionQuad}"
VIAddVersionKey "ProductName" "Mode Chat"
VIAddVersionKey "FileDescription" "Mode Chat 安装程序"
VIAddVersionKey "FileVersion" "${version}"
VIAddVersionKey "ProductVersion" "${version}"
VIAddVersionKey "CompanyName" "Mode Chat"
VIAddVersionKey "LegalCopyright" "MIT"

!define MUI_ABORTWARNING
AutoCloseWindow true
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  ; 先静默卸载旧版本：只删程序文件与快捷方式，用户数据（~/.mode-bot）不动。
  ReadRegStr $0 HKCU "Software\\ModeChat" "InstallDir"
  StrCmp $0 "" done
  IfFileExists "$0\\uninstall.exe" 0 done
  DetailPrint "卸载旧版本：$0"
  ExecWait '"$0\\uninstall.exe" /S _?=$0'
  done:
FunctionEnd

Section "主程序（必需）" SecMain
  SectionIn RO
  nsExec::Exec '"$SYSDIR\\taskkill.exe" /F /IM \${APP_EXE}'
  Pop $0
  Sleep 800
  SetOutPath "$INSTDIR"
  File "${escape(join(stageDir, exeName))}"
  File "${escape(join(stageDir, "README.md"))}"
  WriteRegStr HKCU "Software\\ModeChat" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\\\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\\\${APP_NAME}\\\${APP_NAME}.lnk" "$INSTDIR\\\${APP_EXE}"
  CreateShortcut "$SMPROGRAMS\\\${APP_NAME}\\卸载.lnk" "$INSTDIR\\uninstall.exe"

  WriteRegStr HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ModeChat" "DisplayName" "\${APP_NAME} ${version}"
  WriteRegStr HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ModeChat" "DisplayVersion" "${version}"
  WriteRegStr HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ModeChat" "DisplayIcon" "$INSTDIR\\\${APP_EXE}"
  WriteRegStr HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ModeChat" "UninstallString" "$INSTDIR\\uninstall.exe"
  WriteRegDWORD HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ModeChat" "NoModify" 1
  WriteRegDWORD HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ModeChat" "NoRepair" 1
SectionEnd

Section "创建桌面图标" SecDesktop
  CreateShortcut "$DESKTOP\\\${APP_NAME}.lnk" "$INSTDIR\\\${APP_EXE}"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT \${SecMain} "Mode Chat 主程序与运行文件。"
  !insertmacro MUI_DESCRIPTION_TEXT \${SecDesktop} "在桌面创建快捷方式（默认勾选）。"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  SetAutoClose true
  nsExec::Exec '"$SYSDIR\\taskkill.exe" /F /IM \${APP_EXE}'
  Pop $0
  Sleep 800
  Delete "$INSTDIR\\\${APP_EXE}"
  Delete "$INSTDIR\\README.md"
  Delete "$INSTDIR\\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\\\${APP_NAME}\\\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\\\${APP_NAME}\\卸载.lnk"
  RMDir "$SMPROGRAMS\\\${APP_NAME}"
  Delete "$DESKTOP\\\${APP_NAME}.lnk"
  DeleteRegKey HKCU "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ModeChat"
  DeleteRegKey HKCU "Software\\ModeChat"
SectionEnd
`;
  writeFileSync(nsiPath, script, "utf8");
  try {
    execFileSync(makensis, ["/V2", nsiPath], { stdio: "inherit" });
    return true;
  } catch (error) {
    console.warn(`[package] makensis 失败（安装器未产出）：${error.message}`);
    return false;
  } finally {
    rmSync(nsiPath, { force: true });
  }
}

/**
 * 发布包必须是 GUI 子系统（PE Subsystem = 2）。
 *
 * 控制台子系统（3）双击启动会先开一个黑框窗口，关掉那个窗口等于关掉程序
 * （2026-11-23 用户反馈）。这里读 PE 头做发布门禁，防止 `windows_subsystem`
 * 属性被误删后又发出带黑框的包。
 */
function assertGuiSubsystem(exePath) {
  const buffer = readFileSync(exePath);
  if (buffer.toString("latin1", 0, 2) !== "MZ") {
    throw new Error(`${exePath} 不是 PE 文件`);
  }
  const peOffset = buffer.readUInt32LE(0x3c);
  const subsystem = buffer.readUInt16LE(peOffset + 24 + 68);
  if (subsystem !== 2) {
    throw new Error(
      `发布包是控制台子系统（PE Subsystem=${subsystem}）：启动会弹黑框窗口。` +
        "检查 src/main.rs 顶部的 windows_subsystem 属性（release 必须是 GUI 子系统）",
    );
  }
}

if (!skipBuild) build();
const sourceExe = join(repoRoot, "target", "release", `${cargoBin}.exe`);
if (!existsSync(sourceExe)) {
  console.error(`[package] 找不到产物：${sourceExe}（先去掉 --skip-build）`);
  process.exit(1);
}
try {
  assertGuiSubsystem(sourceExe);
} catch (error) {
  console.error(`[package] ${error.message}`);
  process.exit(1);
}
console.log("[package] 子系统校验通过：GUI（双击不弹控制台）");
const versionProbe = spawnSync(sourceExe, ["--version"], {
  encoding: "utf8",
  windowsHide: true,
});
const versionText = `${versionProbe.stdout ?? ""}${versionProbe.stderr ?? ""}`.trim();
if (versionProbe.status !== 0 || !versionText.endsWith(` ${version}`)) {
  console.error(
    `[package] 版本注入校验失败：期望以「 ${version}」结尾，实际「${versionText || "(无输出)"}」`,
  );
  process.exit(1);
}
console.log(`[package] 版本注入校验通过：${versionText}`);

const stageDir = join(distDir, "stage");
rmSync(stageDir, { recursive: true, force: true });
mkdirSync(stageDir, { recursive: true });
copyFileSync(sourceExe, join(stageDir, exeName));
copyFileSync(join(repoRoot, "README.md"), join(stageDir, "README.md"));

const zipName = `ModeChat-v${version}-windows-x64.zip`;
const zipPath = join(distDir, zipName);
archive(stageDir, zipPath);
const setupName = `ModeChat-v${version}-setup.exe`;
const setupPath = join(distDir, setupName);
const hasInstaller = makeInstaller(stageDir, setupPath);
if (requireInstaller && !hasInstaller) {
  console.error("[package] 要求安装器但 makensis 不可用");
  process.exit(1);
}
rmSync(stageDir, { recursive: true, force: true });

const exeBytes = statSync(sourceExe).size;
const zipBytes = statSync(zipPath).size;
const setupBytes = hasInstaller && existsSync(setupPath) ? statSync(setupPath).size : 0;

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

const zipSha256 = sha256File(zipPath);
const releaseBase = (
  process.env.MODE_CHAT_RELEASE_BASE_URL ??
  `https://github.com/eafenzhang/Mode-Chat/releases/download/v${version}`
).replace(/\/+$/, "");
const manifestPath = join(distDir, "gpui-latest.json");
writeFileSync(
  manifestPath,
  JSON.stringify(
    {
      version,
      pub_date: new Date().toISOString(),
      notes: "Mode Chat",
      platforms: {
        "windows-x86_64": {
          url: `${releaseBase}/${zipName}`,
          sha256: zipSha256,
          size: zipBytes,
        },
      },
    },
    null,
    2,
  ) + "\n",
  "utf8",
);

console.log("");
console.log("[package] 产物");
console.log(`  二进制        ${exeName}  ${mib(exeBytes)}`);
console.log(`  压缩包        ${zipName}  ${mib(zipBytes)}`);
if (setupBytes > 0) {
  console.log(`  安装器        ${setupName}  ${mib(setupBytes)}`);
} else {
  console.log("  安装器        未产出（需要 makensis）");
}
console.log(`  更新清单      gpui-latest.json  sha256=${zipSha256}`);
console.log("[package] Mode Chat 为单 exe 包：不包含 Node/TS agent 运行时");
