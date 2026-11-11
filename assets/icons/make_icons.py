#!/usr/bin/env python3
"""Aimdot app icon: renders aimdot.svg (the master) into PNGs + a Windows .ico, next to this script.

    python3 assets/icons/make_icons.py

The icon (10 Nov 2026, "lock-on"): coral gradient tile (same style as Echo's purple one),
four white lock-on arcs around a white aim dot. Colours: #ff9a62 -> #ef6a4c (Aimdot coral) -> #e2445c.
Writes aimdot-16/24/32/48/64/128/256/512.png and aimdot.ico (16-256, each size rendered on its own, not scaled down),
and the Game Bar widget's icons gamebar-widget/GameBar/Icons/icon.targetsize-16/20/24/32/44/256.png (Microsoft's
rule: fill the square except a 1 px transparent border).
Needs Inkscape (Linux: the `inkscape` package) and Pillow.
"""
import pathlib, shutil, subprocess, sys
from PIL import Image

HERE = pathlib.Path(__file__).resolve().parent
NAME = "aimdot"
PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
GAMEBAR_SIZES = [16, 20, 24, 32, 44, 256]
GAMEBAR_DIR = HERE.parent.parent / "gamebar-widget" / "GameBar" / "Icons"


def main():
    inkscape = shutil.which("inkscape") or sys.exit("needs Inkscape (sudo dnf install inkscape)")
    for s in PNG_SIZES:
        subprocess.run([inkscape, str(HERE / f"{NAME}.svg"), "-w", str(s), "-h", str(s), "-o", str(HERE / f"{NAME}-{s}.png")],
                       check=True, capture_output=True)
    imgs = [Image.open(HERE / f"{NAME}-{s}.png").convert("RGBA") for s in ICO_SIZES]
    imgs[-1].save(HERE / f"{NAME}.ico", sizes=[(s, s) for s in ICO_SIZES], append_images=imgs[:-1])
    print(f"wrote {NAME}-{{{','.join(map(str, PNG_SIZES))}}}.png and {NAME}.ico")
    if GAMEBAR_DIR.is_dir():
        for s in GAMEBAR_SIZES:
            out = GAMEBAR_DIR / f"icon.targetsize-{s}.png"
            subprocess.run([inkscape, str(HERE / f"{NAME}.svg"), "-w", str(s - 2), "-h", str(s - 2), "-o", str(out)],
                           check=True, capture_output=True)
            tile = Image.new("RGBA", (s, s), (0, 0, 0, 0))
            tile.alpha_composite(Image.open(out).convert("RGBA"), (1, 1))
            tile.save(out)
        print(f"wrote {GAMEBAR_DIR.relative_to(HERE.parent.parent)}/icon.targetsize-{{{','.join(map(str, GAMEBAR_SIZES))}}}.png")


if __name__ == "__main__":
    main()
