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
import sys

ARG = argparse.ArgumentParser()
ARG.add_argument("--icon-sets", default=os.path.join(os.path.expandvars(r"%TEMP%"), "icon-sets"))
ARG.add_argument("--lobe", default=os.path.join(os.path.expandvars(r"%TEMP%"), "lobe-icons", "package", "icons"))
ARGS = ARG.parse_args()

APP = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(APP, "assets", "icons")

# ---- Phosphor：Rust 枚举名 → 图标名 -----------------------------------------
PHOSPHOR = {
    "Search": "magnifying-glass",
    "Plus": "plus",
    "Ellipsis": "dots-three",
    "Settings": "gear",
    "X": "x",
    "Check": "check",
    "ArrowUp": "arrow-up",
    "Stop": "square",
    "ChevronLeft": "caret-left",
    "ChevronRight": "caret-right",
    "ChevronDown": "caret-down",
    "Paperclip": "paperclip",
    "Minus": "minus",
    "WindowMaximize": "square",
    "WindowRestore": "copy",
    "PanelRight": "sidebar-simple",
    "Hash": "hash",
    "Pin": "push-pin",
    "Star": "star",
    "Crown": "crown",
    "Pencil": "pencil-simple",
    "Trash": "trash",
    "EyeOff": "eye-slash",
    "Archive": "archive",
    "Chart": "chart-line",
    "Cpu": "cpu",
    "Plug": "plug",
    "Calendar": "calendar-blank",
    "Refresh": "arrows-clockwise",
    "Download": "download-simple",
    "MessageCircle": "chat-circle",
    "Bot": "robot",
    "ListChecks": "list-checks",
    "Building": "buildings",
    "Smile": "smiley",
    "Shield": "shield",
    "Terminal": "terminal-window",
    "Folder": "folder",
    "Scissors": "scissors",
    "Mic": "microphone",
    "BellOff": "bell-slash",
    "Key": "key",
    "Info": "info",
    "MessageSquare": "chat",
    "Sparkles": "sparkle",
    "Contacts": "users",
    "MessageCircleFilled": "chat-circle-fill",
    "Brain": "brain",
    "Pushpin": "push-pin",
    "Sun": "sun",
    "Moon": "moon",
    "Monitor": "desktop",
}

# 侧栏 / 设置菜单：21 组，(线, 面)
MENU = {
    # 消息沿用对话气泡+点；「反馈」改成扩音器，两者不再撞脸
    "chat": ("chat-dots", "chat-dots-fill"),
    "contacts": ("users", "users-fill"),
    "channels": ("hash", "hash-fill"),
    "gear": ("gear", "gear-fill"),
    "sliders": ("faders", "faders-fill"),
    "models": ("stack", "stack-fill"),
    "plugins": ("puzzle-piece", "puzzle-piece-fill"),
    # 智能体用电路图，Bot 用机器人：两个都用 robot 会撞脸（设置菜单里上下相邻）
    "agents": ("circuitry", "circuitry-fill"),
    "bot": ("robot", "robot-fill"),
    "memory": ("brain", "brain-fill"),
    "im": ("device-mobile", "device-mobile-fill"),
    # 连接器用插头（插件页已占用拼图，两者语义也更容易区分）
    "connectors": ("plug", "plug-fill"),
    "schedules": ("calendar-blank", "calendar-blank-fill"),
    "keys": ("key", "key-fill"),
    "abilities": ("lightning", "lightning-fill"),
    "usage": ("chart-line", "chart-line-fill"),
    "about": ("info", "info-fill"),
    "feedback": ("megaphone", "megaphone-fill"),
    "browser": ("globe", "globe-fill"),
    "computer": ("desktop", "desktop-fill"),
    "more": ("dots-three-vertical", "dots-three-vertical-fill"),
}

# 线框字重：regular 用作默认，light 用作「细线」场景（替代原来的 stroke-width 微调）
LINE_WEIGHTS = ["", "-light"]

# ---- 连接器品牌：Iconify logos ----------------------------------------------
CONNECTORS = {
    "github": "github-icon",
    "gitlab": "gitlab-icon",
    "notion": "notion-icon",
    "slack": "slack-icon",
    "postgres": "postgresql",
    "kubernetes": "kubernetes",
    "cloudflare": "cloudflare-icon",
    "linear": "linear-icon",
    "figma": "figma",
    "playwright": "playwright",
    "redis": "redis",
    "elasticsearch": "elasticsearch",
    "brave": "brave",
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
    ph, ph_w, ph_h = load_set("ph")
    logos, logo_w, logo_h = load_set("logos")

    ph_dir = os.path.join(ASSETS, "phosphor")
    br_dir = os.path.join(ASSETS, "brands")
    pv_dir = os.path.join(ASSETS, "providers")

    # 清掉旧产物
    for d in (ph_dir, pv_dir):
        if os.path.isdir(d):
            for f in os.listdir(d):
                os.remove(os.path.join(d, f))

    wanted = set(PHOSPHOR.values())
    for line, solid in MENU.values():
        wanted.add(line)
        wanted.add(solid)

    ok, fail = 0, []

    # 图标名 → 输出文件名（去掉 fill 后缀的保留，按名字直接落盘）
    for name in sorted(wanted):
        entry = ph.get(name)
        if not entry:
            fail.append(("ph", name))
            continue
        write(os.path.join(ph_dir, name + ".svg"), iconify_svg(entry, ph_w, ph_h))
        ok += 1
        # 细线变体只给「线框」类（名字不带 -fill）
        if not name.endswith("-fill"):
            for w in LINE_WEIGHTS[1:]:
                e = ph.get(name + w)
                if e:
                    write(os.path.join(ph_dir, name + w + ".svg"), iconify_svg(e, ph_w, ph_h))
                    ok += 1
                else:
                    fail.append(("ph", name + w))

    for key, name in CONNECTORS.items():
        entry = logos.get(name)
        if not entry:
            fail.append(("logos", name))
            continue
        write(os.path.join(br_dir, key + ".svg"), iconify_svg(entry, logo_w, logo_h))
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
