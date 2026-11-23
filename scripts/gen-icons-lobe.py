"""抓图标资产：Phosphor(通用) + Iconify logos(连接器) + LobeHub(模型厂商)。

数据源是本地已下载的 @iconify-json 包（不走 API，避免限流），产物入库，
不进构建链（与 scripts/gen-icons.mjs 同定位）。

用法：
    node ../AppData/Local/Temp/icon-sets 里已有的包
    python scripts/gen-icons-lobe.py [--icon-sets DIR] [--lobe DIR]
"""
import argparse
import json
import os
import re
import sys

ARG = argparse.ArgumentParser()
ARG.add_argument("--icon-sets", default=os.path.join(os.path.expandvars(r"%TEMP%"), "icon-sets"))
ARG.add_argument("--lobe", default=os.path.join(os.path.expandvars(r"%TEMP%"), "lobe-icons", "package", "icons"))
ARGS = ARG.parse_args()

APP = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(APP, "assets", "icons")

# ---- Phosphor：Rust 枚举名 → 图标名 -----------------------------------------
# ---- Remix Icon：Icon 枚举名 -> Remix 图标名 ----------------------------------
#
# 全部用 `-line` 变体：线框、圆角柔和，正是替换 Phosphor 实心的目的。
# 少数没有 -line 后缀的（hashtag / list-check）直接用原名。
RI = {
    "Search": "search-line",
    "Plus": "add-line",
    "Ellipsis": "more-line",
    "Settings": "settings-3-line",
    "X": "close-line",
    "Check": "check-line",
    "ArrowUp": "arrow-up-line",
    "Stop": "stop-line",
    "ChevronLeft": "arrow-left-s-line",
    "ChevronRight": "arrow-right-s-line",
    "ChevronDown": "arrow-down-s-line",
    "Paperclip": "attachment-line",
    "Minus": "subtract-line",
    "WindowMaximize": "checkbox-blank-line",
    "WindowRestore": "file-copy-line",
    "PanelRight": "layout-right-line",
    "Hash": "hashtag",
    "Pin": "pushpin-line",
    "Star": "star-line",
    "Crown": "vip-crown-line",
    "Pencil": "edit-line",
    "Trash": "delete-bin-line",
    "EyeOff": "eye-off-line",
    "Archive": "archive-line",
    "Chart": "line-chart-line",
    "Cpu": "cpu-line",
    "Plug": "plug-line",
    "Calendar": "calendar-line",
    "Refresh": "refresh-line",
    "Download": "download-line",
    "MessageCircle": "chat-3-line",
    "Bot": "robot-line",
    "ListChecks": "list-check-2",
    "Building": "building-line",
    "Smile": "emotion-happy-line",
    "Shield": "shield-check-line",
    "Terminal": "terminal-box-line",
    "Folder": "folder-line",
    "Scissors": "scissors-line",
    "Mic": "mic-line",
    "BellOff": "notification-off-line",
    "Key": "key-2-line",
    "Info": "information-line",
    "MessageSquare": "message-2-line",
    "Sparkles": "sparkling-line",
    "Contacts": "contacts-line",
    "MessageCircleFilled": "chat-3-fill",
    "Brain": "brain-line",
    "Pushpin": "pushpin-line",
    "Sun": "sun-line",
    "Moon": "moon-line",
    "Monitor": "computer-line",
    "SidebarExpand": "layout-left-line",
    "PaneCollapse": "arrow-right-double-line",
    "PaneExpand": "arrow-left-double-line",
}

# 侧栏 / 设置菜单：21 组，(线, 面)
MENU = {
    "chat": ("chat-3-line", "chat-3-fill"),
    "contacts": ("contacts-line", "contacts-fill"),
    "channels": ("hashtag", "grid-fill"),
    "gear": ("settings-3-line", "settings-3-fill"),
    "sliders": ("equalizer-line", "equalizer-fill"),
    "models": ("stack-line", "stack-fill"),
    "plugins": ("puzzle-line", "puzzle-fill"),
    # 智能体用 CPU（电路感），Bot 用机器人：两个都用 robot 会撞脸
    "agents": ("cpu-line", "cpu-fill"),
    "bot": ("robot-2-line", "robot-2-fill"),
    "memory": ("brain-line", "brain-fill"),
    "im": ("smartphone-line", "smartphone-fill"),
    "connectors": ("plug-2-line", "plug-2-fill"),
    "schedules": ("calendar-line", "calendar-fill"),
    "keys": ("key-2-line", "key-2-fill"),
    "abilities": ("flashlight-line", "flashlight-fill"),
    "usage": ("line-chart-line", "line-chart-fill"),
    "about": ("information-line", "information-fill"),
    "feedback": ("megaphone-line", "megaphone-fill"),
    "browser": ("global-line", "global-fill"),
    "computer": ("computer-line", "computer-fill"),
    "more": ("more-2-line", "more-2-fill"),
}

# 线框字重：regular 用作默认，light 用作「细线」场景（替代原来的 stroke-width 微调）
LINE_WEIGHTS = ["", "-light"]

# ---- 连接器品牌 -------------------------------------------------------------
# 统一成单色 currentColor（跟侧栏图标同一套风格），颜色由调用方按品牌色染。
# 数据源优先 simple-icons（本身就是单色 24×24），个别没有的从 logos 压平。
# (图标集, 图标名, 是否强制压成单色)
CONNECTORS = {
    "github": ("simple", "github", False),
    "gitlab": ("simple", "gitlab", False),
    "notion": ("simple", "notion", False),
    # simple-icons 只有 slackware，Slack 取 mdi（单色 24×24）
    "slack": ("mdi", "slack", False),
    "postgres": ("simple", "postgresql", False),
    "kubernetes": ("simple", "kubernetes", False),
    "cloudflare": ("simple", "cloudflare", False),
    "linear": ("simple", "linear", False),
    "figma": ("simple", "figma", False),
    # playwright 只有 logos/devicon 的多色版，强制压平
    "playwright": ("logos", "playwright", True),
    "redis": ("simple", "redis", False),
    "elasticsearch": ("simple", "elasticsearch", False),
    "brave": ("simple", "brave", False),
}

# ---- IM 频道：手机端官方 logo（TDesign 的单色标，按品牌色染）--------------------
IM = {
    "wechat": ("tdesign", "logo-wechat"),
    "wecom": ("tdesign", "logo-wecom"),
}

# ---- 智能体引擎：官方最新标（LobeHub 单色矢量）----------------------------------
AGENTS = {
    "pi": ("lobe-src", "pi"),
    "hermes": ("lobe-src", "hermesagent"),
    "deepseek": ("lobe-src", "deepseek"),
    # ZCode = 智谱 Z.ai 的编码产品，用 Z.ai 官方标
    "zcode": ("lobe-src", "zai"),
}

# ---- 模型厂商：LobeHub ------------------------------------------------------
PROVIDERS = {
    "deepseek": "deepseek-color",
    "openai": "openai",
    "anthropic": "anthropic",
    "claude": "claude-color",
    "google": "google-color",
    "gemini": "gemini-color",
    "qwen": "qwen-color",
    "zhipu": "zhipu-color",
    "chatglm": "chatglm-color",
    "moonshot": "moonshot",
    "kimi": "kimi-color",
    "minimax": "minimax-color",
    "baichuan": "baichuan-color",
    "doubao": "doubao-color",
    "hunyuan": "hunyuan-color",
    "spark": "spark-color",
    "wenxin": "wenxin-color",
    "grok": "grok",
    "xai": "xai",
    "mistral": "mistral-color",
    "ollama": "ollama",
    "openrouter": "openrouter-color",
    "siliconcloud": "siliconcloud-color",
    "together": "together-color",
    "groq": "groq",
    "fireworks": "fireworks-color",
    "perplexity": "perplexity-color",
    "yi": "yi-color",
    "sensenova": "sensenova-color",
    "stepfun": "stepfun-color",
    "alibaba": "alibaba-color",
    "bytedance": "bytedance-color",
    "tencent": "tencent-color",
    "baidu": "baidu-color",
    "iflytek": "iflytekcloud-color",
}


def load_set(name):
    path = os.path.join(ARGS.icon_sets, f"x-{name}", "package", "icons.json")
    if not os.path.exists(path):
        sys.exit(f"缺少图标集数据：{path}")
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    # 每套图的画布在顶层 width/height；单个图标只有非默认尺寸时才覆盖。
    # Phosphor 是 256×256，不是 24×24 —— 按 24 写会整张图画到画布外。
    return data["icons"], data.get("width") or 24, data.get("height") or 24


def iconify_svg(entry, set_w, set_h):
    """把 iconify 的 body 还原成独立 svg 文件（与 API 输出的几何一致）。"""
    w = entry.get("width") or set_w
    h = entry.get("height") or set_h
    left = entry.get("left") or 0
    top = entry.get("top") or 0
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{left} {top} {w} {h}">'
        f'{entry["body"]}</svg>\n'
    )


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def main():
    ri, ri_w, ri_h = load_set("ri")
    logos, logo_w, logo_h = load_set("logos")

    ui_dir = os.path.join(ASSETS, "ui")
    br_dir = os.path.join(ASSETS, "brands")
    pv_dir = os.path.join(ASSETS, "providers")
    agent_dir = os.path.join(ASSETS, "agents")

    # 清掉旧产物（含上一代 phosphor 目录）
    old_ph = os.path.join(ASSETS, "phosphor")
    for d in (ui_dir, old_ph, pv_dir):
        if os.path.isdir(d):
            for f in os.listdir(d):
                os.remove(os.path.join(d, f))
            if d == old_ph:
                os.rmdir(d)

    wanted = set(RI.values())
    for line, solid in MENU.values():
        wanted.add(line)
        wanted.add(solid)

    ok, fail = 0, []

    for name in sorted(wanted):
        entry = ri.get(name)
        if not entry:
            fail.append(("ri", name))
            continue
        write(os.path.join(ui_dir, name + ".svg"), iconify_svg(entry, ri_w, ri_h))
        ok += 1

    # 单色源：simple-icons 按文件名、mdi/tdesign 走包内 icons.json
    simple_dir = os.path.join(ARGS.icon_sets, "x-simple", "package", "icons")
    mdi, mdi_w, mdi_h = load_set("mdi")
    tdesign, td_w, td_h = load_set("tdesign")

    def take(setname, name, force_mono):
        """返回 (svg 文本, 失败原因)"""
        if setname == "simple":
            path = os.path.join(simple_dir, name + ".svg")
            if not os.path.exists(path):
                return None, f"simple-icons 缺 {name}"
            text = open(path, encoding="utf-8").read()
            # simple-icons 没有 fill 属性，靠父级 fill 继承；显式补 currentColor
            if "fill=" not in text:
                text = text.replace("<svg ", '<svg fill="currentColor" ', 1)
            return text, None
        if setname == "logos":
            entry = logos.get(name)
            if not entry:
                return None, f"logos 缺 {name}"
            text = iconify_svg(entry, logo_w, logo_h)
            if not force_mono:
                return text, None
            mono = re.sub(r'fill="#[0-9a-fA-F]{3,8}"', 'fill="currentColor"', text)
            mono = re.sub(r'fill="url\([^)]*\)"', 'fill="currentColor"', mono)
            return mono, None
        if setname == "mdi":
            entry = mdi.get(name)
            if not entry:
                return None, f"mdi 缺 {name}"
            return iconify_svg(entry, mdi_w, mdi_h), None
        if setname == "tdesign":
            entry = tdesign.get(name)
            if not entry:
                return None, f"tdesign 缺 {name}"
            return iconify_svg(entry, td_w, td_h), None
        return None, f"未知图标集 {setname}"

    for key, (setname, name, force_mono) in CONNECTORS.items():
        text, err = take(setname, name, force_mono)
        if err:
            fail.append((setname, err))
            continue
        write(os.path.join(br_dir, key + ".svg"), text)
        ok += 1

    for key, (setname, name) in IM.items():
        text, err = take(setname, name, False)
        if err:
            fail.append((setname, err))
            continue
        write(os.path.join(br_dir, key + ".svg"), text)
        ok += 1

    # 智能体：LobeHub 单色矢量（覆盖原先手拆三层 + PNG 位图的临时方案）
    for key, (_src, name) in AGENTS.items():
        src = os.path.join(ARGS.lobe, name + ".svg")
        if not os.path.exists(src):
            fail.append(("lobe-src", name))
            continue
        write(os.path.join(agent_dir, key + ".svg"),
              open(src, encoding="utf-8").read())
        ok += 1

    for pid, name in PROVIDERS.items():
        src = os.path.join(ARGS.lobe, name + ".svg")
        if not os.path.exists(src):
            fail.append(("lobe", name))
            continue
        write(os.path.join(pv_dir, pid + ".svg"),
              open(src, encoding="utf-8").read())
        ok += 1

    print(f"assets ok={ok} fail={len(fail)}")
    for f in fail:
        print("  FAILED", f)
    if fail:
        sys.exit(1)


if __name__ == "__main__":
    main()
