#!/usr/bin/env python3
"""Aimdot installer pictures (NSIS): header.bmp (150x57, top right of the inner pages) and sidebar.bmp (164x314,
welcome + finish pages), 24-bit BMP like Echo's. Coral gradient (Aimdot's colours) + the app icon.

    python3 installer/make_images.py

Needs Pillow. Reads assets/icons/aimdot-128.png / aimdot-48.png.
"""
import pathlib
from PIL import Image, ImageDraw

HERE = pathlib.Path(__file__).resolve().parent
ICONS = HERE.parent / "assets" / "icons"
# Aimdot coral (settings/ui/theme.slint T.g0 / g1 / g2)
G0, G1, G2 = (0xFF, 0x8F, 0x6B), (0xEF, 0x6A, 0x4C), (0xE0, 0x48, 0x5E)


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def gradient(w, h):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            t = (x / max(w - 1, 1) + y / max(h - 1, 1)) / 2  # 135 deg, like T.grad
            px[x, y] = lerp(G0, G1, t / 0.55) if t < 0.55 else lerp(G1, G2, (t - 0.55) / 0.45)
    return img


def paste_icon(img, icon_name, size, pos):
    ic = Image.open(ICONS / icon_name).convert("RGBA").resize((size, size), Image.LANCZOS)
    base = img.convert("RGBA")
    base.alpha_composite(ic, pos)
    return base.convert("RGB")


def main():
    # header: white like Windows' page background, icon at the right
    header = Image.new("RGB", (150, 57), (255, 255, 255))
    header = paste_icon(header, "aimdot-48.png", 44, (150 - 44 - 8, (57 - 44) // 2))
    header.save(HERE / "header.bmp")
    # sidebar: coral gradient, icon in the upper third, a thin lighter line under it
    side = gradient(164, 314)
    d = ImageDraw.Draw(side)
    d.rectangle([32, 196, 131, 197], fill=(255, 214, 200))
    side = paste_icon(side, "aimdot-128.png", 100, (32, 70))
    side.save(HERE / "sidebar.bmp")
    print("wrote installer/header.bmp + installer/sidebar.bmp")


if __name__ == "__main__":
    main()
