"""生成图标资产：Boxicons(通用线框+实心) + Iconify logos(连接器) + LobeHub(模型厂商)。"""
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
# ---- Boxicons：Icon 枚举名 -> Boxicons 图标名（线框；实心自动加 -filled）----------
#
# Boxicons 是 24×24 网格、MIT、line/fill 严格成对（1884 对）。
# 线框版无后缀，实心版加 `-filled`。
BOX = {
    "Search": "search",
    "Plus": "plus",
    "Ellipsis": "dots-horizontal",
    "Settings": "gear",
    "X": "x",
    "Check": "check",
    "ArrowUp": "arrow-up",
    "Stop": "stop",
    "ChevronLeft": "chevron-left",
    "ChevronRight": "chevron-right",
    "ChevronDown": "chevron-down",
    "Paperclip": "paperclip",
    "Minus": "minus",
    "WindowMaximize": "square",
    "WindowRestore": "copy",
    "PanelRight": "sidebar",
    "Hash": "hashtag",
    "Pin": "pin",
    "Star": "star",
    "Crown": "crown",
    "Pencil": "pencil",
    "Trash": "trash",
    "EyeOff": "eye-slash",
    "Archive": "archive",
    "Chart": "chart-line",
    "Cpu": "chip",
    "Plug": "plug-connect",
    "Calendar": "calendar",
    "Refresh": "refresh-ccw",
    "Download": "save",
    "MessageCircle": "message-circle",
    "Bot": "robot",
    "ListChecks": "task",
    "Building": "building",
    "Smile": "smile",
    "Shield": "shield",
    "Terminal": "terminal",
    "Folder": "folder",
    "Scissors": "cut",
    "Mic": "microphone",
    "BellOff": "notification-slash",
    "Key": "key",
    "Info": "info-circle",
    "MessageSquare": "message",
    "Sparkles": "sparkle",
    "Contacts": "group",
    "MessageCircleFilled": "message-circle-filled",
    "Brain": "brain",
    "Pushpin": "pin",
    "Sun": "sun",
    "Moon": "moon",
    "Monitor": "monitor",
    "SidebarExpand": "sidebar",
    # 折叠 = 箭头指向右边缘；展开 = 指向左
    "SidebarCollapse": "sidebar-right",
    "PaneCollapse": "sidebar-right",
    "PaneExpand": "arrow-to-left",
}

# 侧栏 / 设置菜单：21 组，(线, 面)
MENU = {
    "chat": ("message-circle", "message-circle-filled"),
    "contacts": ("group", "group-filled"),
    "channels": ("hashtag", "hashtag-filled"),
    "gear": ("gear", "gear-filled"),
    "sliders": ("slider", "slider-filled"),
    "models": ("layers", "layers-filled"),
    "plugins": ("puzzle", "puzzle-filled"),
    # 智能体用 CPU，「Bot」用机器人：两个都用 robot 会撞脸
    "agents": ("chip", "chip-filled"),
    "bot": ("robot", "robot-filled"),
    "memory": ("brain", "brain-filled"),
    "im": ("mobile", "mobile-filled"),
    "connectors": ("plug-connect", "plug-connect-filled"),
    "schedules": ("calendar", "calendar-filled"),
    "keys": ("key", "key-filled"),
    "abilities": ("bolt", "bolt-filled"),
    "usage": ("bar-chart", "bar-chart-filled"),
    "about": ("info-circle", "info-circle-filled"),
    "feedback": ("megaphone", "megaphone-filled"),
    "browser": ("globe", "globe-filled"),
    "computer": ("desktop", "desktop-filled"),
    "more": ("dots-horizontal", "dots-horizontal-filled"),
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
    box, box_w, box_h = load_set("boxicons")
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

    wanted = set(BOX.values())
    for line, solid in MENU.values():
        wanted.add(line)
        wanted.add(solid)

    ok, fail = 0, []

    for name in sorted(wanted):
        entry = box.get(name)
        if not entry:
            fail.append(("boxicons", name))
            continue
        write(os.path.join(ui_dir, name + ".svg"), iconify_svg(entry, box_w, box_h))
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
