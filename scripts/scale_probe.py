#!/usr/bin/env python3
"""One scale probe, run INSIDE a headless KWin by scripts/test-scale.sh: starts the overlay (test folder), takes a
screenshot and prints the crosshair's size + centre in physical pixels vs the screen centre.
    argv: <aimdot binary> <test folder> <screenshot path> <offset_x>"""
import os, sys, subprocess, time
from PIL import Image
binp, home, out, offx = sys.argv[1:5]
extra = os.environ.get("PROBE_CFG", "").replace(";", "\n")  # extra [app] lines, e.g. PROBE_CFG="size_mode=1"
open(os.path.join(home, "config.ini"), "w").write(f"[app]\noverlay_on=1\naim_mode=0\noffset_x={offx}\n{extra}\n")
p = subprocess.Popen([binp, "--tray"], env=dict(os.environ, CROSSHAIR_HOME=home), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(1.5)
subprocess.run(["spectacle", "-b", "-n", "-f", "-o", out], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
im = Image.open(out).convert("RGB"); w, h = im.size; bg = im.getpixel((3, 3))
px = im.load()
xs = [x for y in range(h // 2 - 60, h // 2 + 60) for x in range(w) if px[x, y] != bg]
ys = [y for y in range(h) for x in range(w // 2 - 60 + int(offx), w // 2 + 60 + int(offx)) if px[x, y] != bg]
print(f"screen {w}x{h} off {offx}: x {min(xs)}..{max(xs)} (w {max(xs)-min(xs)+1}, centre {(min(xs)+max(xs)+1)/2 - int(offx)} want {w/2})  y {min(ys)}..{max(ys)} (h {max(ys)-min(ys)+1}, centre {(min(ys)+max(ys)+1)/2} want {h/2})")
subprocess.run([binp, "--quit"], env=dict(os.environ, CROSSHAIR_HOME=home)); time.sleep(0.3); p.kill()
