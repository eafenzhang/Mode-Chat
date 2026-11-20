/**
 * 一次性生成图标资产（不进构建链，生成物提交）。
 *
 * 为什么：原版 Web 的图标就是 lucide-react（ISC）。GPUI 侧用 `svg().data()` 渲染
 * 同源 SVG 原文就能得到完全一致的几何/线宽/圆头，不需要手绘近似。
 *
 * 运行：`node apps/bot-gpui-pilot/scripts/gen-icons.mjs`
 */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const iconDir = resolve(here, "../assets/icons");
const lucideIcons = join(repoRoot, "node_modules/lucide-react/dist/esm/icons");

const HEAD =
  '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" ' +
  'fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" ' +
  'stroke-linejoin="round">';

const ICONS = [
  ["search", "search"],
  ["plus", "plus"],
  ["ellipsis", "ellipsis"],
  ["settings", "settings"],
  ["x", "x"],
  ["check", "check"],
  ["arrow-up", "arrow-up"],
  ["stop", "square"],
  ["chevron-left", "chevron-left"],
  ["chevron-right", "chevron-right"],
  ["paperclip", "paperclip"],
  ["minus", "minus"],
  ["panel-right", "panel-right"],
  ["hash", "hash"],
  ["pin", "pin"],
  ["star", "star"],
  ["crown", "crown"],
  ["pencil", "pencil"],
  ["trash", "trash-2"],
  ["eye-off", "eye-off"],
  ["archive", "archive"],
  ["chart", "activity"],
  ["cpu", "cpu"],
  ["plug", "plug"],
  ["calendar", "calendar"],
  ["refresh", "refresh-cw"],
  ["download", "download"],
  ["message-circle", "message-circle"],
  ["bot", "bot"],
  ["list-checks", "list-checks"],
  ["building", "building-2"],
  ["smile", "smile"],
  ["folder", "folder"],
  ["scissors", "scissors"],
  ["mic", "mic"],
  ["bell-off", "bell-off"],
  ["key", "key"],
  ["info", "info"],
  ["message-square", "message-square"],
  ["sparkles", "sparkles"],
  ["brain", "brain"],
];

function escapeAttr(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function renderNode([tag, attrs]) {
  const keys = Object.keys(attrs).filter((key) => key !== "key");
  const rendered = keys.map((key) => `${key}="${escapeAttr(attrs[key])}"`).join(" ");
  return `<${tag}${rendered ? ` ${rendered}` : ""}/>`;
}

async function renderLucide(moduleName) {
  const module = await import(pathToFileURL(join(lucideIcons, `${moduleName}.mjs`)).href);
  return module.__iconNode.map(renderNode).join("");
}

function renderSvg(body, { fill = "none" } = {}) {
  if (fill === "currentColor") {
    return HEAD.replace('fill="none"', 'fill="currentColor"').replace(' stroke="currentColor"', ' stroke="none"') + body + "</svg>\n";
  }
  return HEAD + body + "</svg>\n";
}

// 窗口按钮（原版 packages/ui/src/components/icons/windowIcons.ts 的同源几何）。
const EXTRA = {
  // 微信「通讯录」：单人轮廓 + 右侧三条列表线。
  "contacts":
    '<circle cx="9" cy="8" r="3"/><path d="M4 19c0-3.3 2.2-6 5-6s5 2.7 5 6"/><path d="M16 9h4M16 12h4M16 15h4"/>',
  "window-maximize": '<rect x="6" y="6" width="12" height="12" rx="1.5"/>',
  "window-restore":
    '<path d="M9 6h7a2 2 0 0 1 2 2v7"/><rect x="6" y="9" width="9" height="9" rx="1.5"/>',
  // 标题栏置顶钉：比 lucide pin 更简洁的经典图钉（与窗控同 24 画布、1.7 描边）。
  "pushpin":
    '<path d="M7.5 3h9l-1.5 5 2.5 2v2H6.5v-2l2.5-2-1.5-5Z"/><path d="M12 12v8"/>',
};

// 微信风格的实心气泡（当前会话 tab 激活态；微信的激活图标是实心绿色）。
const FILLED_EXTRA = {
  "message-circle-filled":
    '<path d="M12 3C6.9 3 3 6.36 3 10.5c0 2.1 1.04 4 2.72 5.34L5 21l4.03-2.06c.94.22 1.94.34 2.97.34 5.1 0 9-3.36 9-7.5S17.1 3 12 3Z"/>',
};

// 机器人面（原版 ui/avatar.tsx 的四段内联 SVG，24 viewBox）。
const BOT_FACES = [
  '<rect x="5" y="7" width="14" height="11" rx="4"/><circle cx="9.5" cy="12" r="1.2" fill="currentColor" stroke="none"/><circle cx="14.5" cy="12" r="1.2" fill="currentColor" stroke="none"/><path d="M9.8 15.5h4.4"/>',
  '<rect x="5" y="7" width="14" height="11" rx="4"/><path d="M9.3 10.5v3M14.7 10.5v3"/><path d="M9.8 16h4.4"/>',
  '<rect x="5" y="7" width="14" height="11" rx="4"/><path d="M12 7V4"/><circle cx="12" cy="3.2" r="1" fill="currentColor" stroke="none"/><circle cx="9.5" cy="12.5" r="1.3" fill="currentColor" stroke="none"/><circle cx="14.5" cy="12.5" r="1.3" fill="currentColor" stroke="none"/>',
  '<rect x="5" y="7" width="14" height="11" rx="4"/><circle cx="9.5" cy="12.5" r="1" fill="currentColor" stroke="none"/><circle cx="14.5" cy="12.5" r="1" fill="currentColor" stroke="none"/><path d="M9.6 15.4c1.4 1.1 3.4 1.1 4.8 0"/>',
];

await mkdir(iconDir, { recursive: true });

for (const [outputName, lucideName] of ICONS) {
  const body = await renderLucide(lucideName);
  const svg = renderSvg(body, { fill: outputName === "stop" ? "currentColor" : "none" });
  await writeFile(join(iconDir, `${outputName}.svg`), svg, "utf8");
}

for (const [name, body] of Object.entries(EXTRA)) {
  await writeFile(join(iconDir, `${name}.svg`), renderSvg(body), "utf8");
}

for (const [name, body] of Object.entries(FILLED_EXTRA)) {
  await writeFile(join(iconDir, `${name}.svg`), renderSvg(body, { fill: "currentColor" }), "utf8");
}

for (const [index, body] of BOT_FACES.entries()) {
  await writeFile(join(iconDir, `bot-face-${index}.svg`), renderSvg(body), "utf8");
}

const license = await readFile(join(repoRoot, "node_modules/lucide-react/LICENSE"), "utf8");
await writeFile(join(iconDir, "LICENSE-lucide.txt"), license, "utf8");

console.log(
  `generated ${ICONS.length + Object.keys(EXTRA).length + Object.keys(FILLED_EXTRA).length + BOT_FACES.length} icons`,
);
