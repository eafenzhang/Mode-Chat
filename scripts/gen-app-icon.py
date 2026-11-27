"""生成应用图标（微信绿底 + 白色气泡），输出 assets/icons/app.png 与 app.ico。

为什么用脚本而不是提交一张位图：图标有 1024 PNG（关于页/快捷方式）和 7 档 ICO
（任务栏/资源管理器/安装器）两份，尺寸必须同源；图形坐标直接取自矢量分层
（app-layer-bubble/visor/eyes.svg），改颜色或缩小时两边一起改。

运行：python scripts/gen-app-icon.py
"""
import os
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_PNG = os.path.join(ROOT, "assets", "icons", "app.png")
OUT_ICO = os.path.join(ROOT, "assets", "icons", "app.ico")

WECHAT_GREEN = (0x0D, 0xA8, 0x69, 255)
WHITE = (255, 255, 255, 255)

# 图形整体缩小系数：0.93 = 比原版小 7%（用户要求「缩小一点点」）。
K = 0.93
CX, CY = 512.0, 512.0
SUPER = 4  # 超采样倍数，保证圆角/斜边抗锯齿

# 矢量分层里的原始坐标（见 assets/icons/app-layer-*.svg）。
BUBBLE = (111.6, 191.1, 800.9, 624.8, 241.4)  # x, y, w, h, r
TAIL = [(211.0, 764.8), (151.3, 938.0), (395.6, 807.4)]
VISOR = (256.4, 367.2, 511.2, 309.6, 133.5)
EYES = [(370.0, 443.8, 79.5, 159.0, 39.8), (574.5, 443.8, 79.5, 159.0, 39.8)]
VISOR_ROTATION = -6.0
VISOR_PIVOT = (512.0, 560.0)


def scale_point(x, y):
    return (CX + (x - CX) * K, CY + (y - CY) * K)


def main():
    size = 1024 * SUPER
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    def sc(v, c):
        return (c + (v - c) * K) * SUPER

    # 1) 微信绿圆角底（半径 22% = 225，Windows 应用图标档位）
    radius = 225 * SUPER
    draw.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=WECHAT_GREEN)

    # 2) 白色气泡 + 尾巴
    x, y, w, h, r = BUBBLE
    draw.rounded_rectangle(
        [sc(x, CX), sc(y, CY), sc(x + w, CX), sc(y + h, CY)],
        radius=r * K * SUPER,
        fill=WHITE,
    )
    draw.polygon([(sc(px, CX), sc(py, CY)) for px, py in TAIL], fill=WHITE)

    # 3) 内窗 + 眼睛：同一图层旋转 -6°（与 SVG 的 rotate(-6 512 560) 一致）
    layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ld = ImageDraw.Draw(layer)
    x, y, w, h, r = VISOR
    ld.rounded_rectangle(
        [sc(x, CX), sc(y, CY), sc(x + w, CX), sc(y + h, CY)],
        radius=r * K * SUPER,
        fill=WECHAT_GREEN,
    )
    for ex, ey, ew, eh, er in EYES:
        ld.rounded_rectangle(
            [sc(ex, CX), sc(ey, CY), sc(ex + ew, CX), sc(ey + eh, CY)],
            radius=er * K * SUPER,
            fill=WHITE,
        )
    pivot = (sc(VISOR_PIVOT[0], CX), sc(VISOR_PIVOT[1], CY))
    layer = layer.rotate(VISOR_ROTATION, resample=Image.BICUBIC, center=pivot)
    img = Image.alpha_composite(img, layer)

    img = img.resize((1024, 1024), Image.LANCZOS)
    img.save(OUT_PNG, "PNG")

    img.save(
        OUT_ICO,
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print(f"wrote {OUT_PNG}")
    print(f"wrote {OUT_ICO}")


if __name__ == "__main__":
    main()
