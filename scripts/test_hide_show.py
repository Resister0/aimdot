#!/usr/bin/env python3
"""Hide / show test for the Linux overlay, run INSIDE a headless KWin by scripts/test-hide-show.sh (never on the
real screen). Starts the overlay on a test folder, drives it over its socket (settings changes + the test-only
"test-aim" message = right button held / let go), takes a screenshot after each step and checks whether the
crosshair is at the screen centre.

    argv: <aimdot binary> <test folder (CROSSHAIR_HOME)> <output folder> [x11]

Exit code 0 = every step showed what it should. Needs Pillow + spectacle.
"""
import os, socket, struct, subprocess, sys, time

from PIL import Image

binp, home, out = sys.argv[1:4]
x11 = len(sys.argv) > 4 and sys.argv[4] == "x11"
os.makedirs(out, exist_ok=True)
env = dict(os.environ, CROSSHAIR_HOME=home, CROSSHAIR_FAKE_AIM="1", WAYLAND_DEBUG="1")
if x11:
    env["CROSSHAIR_X11"] = "1"
log = open(os.path.join(out, "overlay.log"), "w")
proc = subprocess.Popen([binp, "--tray"], env=env, stdout=log, stderr=log)


def send(s, kind, body):
    b = (kind + "\n" + body).encode()
    s.sendall(struct.pack("<I", len(b)) + b)


def recv(s):
    n = struct.unpack("<I", s.recv(4, socket.MSG_WAITALL))[0]
    return s.recv(n, socket.MSG_WAITALL).decode().split("\n", 1)


def shown(name):
    """Screenshot; True when there are crosshair pixels within 20 px of the centre."""
    path = os.path.join(out, name + ".png")
    subprocess.run(["spectacle", "-b", "-n", "-f", "-o", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    im = Image.open(path).convert("RGB")
    w, h = im.size
    bg = im.getpixel((10, 10))
    box = im.crop((w // 2 - 20, h // 2 - 20, w // 2 + 20, h // 2 + 20))
    return sum(1 for y in range(box.height) for x in range(box.width) if box.getpixel((x, y)) != bg) > 0


sock = os.path.join(home, "aimdot.sock")
for _ in range(50):
    if os.path.exists(sock):
        break
    time.sleep(0.1)
time.sleep(1.0)
s = socket.socket(socket.AF_UNIX)
s.connect(sock)
s.settimeout(5)
state = None
while state is None:
    k, b = recv(s)
    if k == "state":
        state = b


def setting(key, val):
    global state
    lines = state.split("\n")
    lines = [f"{key}={val}" if l.startswith(key + "=") else l for l in lines]
    state = "\n".join(lines)
    send(s, "state", state)


# (what to do, expected: True = crosshair visible)
steps = [
    ("start, aim mode 'only while aiming', button up", None, False),
    ("button down", ("aim", "1"), True),
    ("button up", ("aim", "0"), False),
    ("button down again", ("aim", "1"), True),
    ("button up again", ("aim", "0"), False),
    ("button down a third time", ("aim", "1"), True),
    ("button up", ("aim", "0"), False),
    ("mode -> always shown, while hidden", ("set", "aim_mode", "0"), True),
    ("overlay off", ("set", "overlay_on", "0"), False),
    ("overlay on", ("set", "overlay_on", "1"), True),
    ("overlay off again", ("set", "overlay_on", "0"), False),
    ("mode -> hide while aiming, while off", ("set", "aim_mode", "2"), False),
    ("overlay on again", ("set", "overlay_on", "1"), True),
    ("button down (hide while aiming)", ("aim", "1"), False),
    ("button up", ("aim", "0"), True),
    ("mode -> only while aiming", ("set", "aim_mode", "1"), False),
    ("fast clicks, ending held", ("fast", "1010101"), True),
    ("fast clicks, ending up", ("fast", "0101010"), False),
    ("fast clicks again, ending held", ("fast", "101"), True),
    ("button up", ("aim", "0"), False),
    ("button down", ("aim", "1"), True),
]
fails = 0
for i, (what, act, want) in enumerate(steps):
    if act and act[0] == "aim":
        send(s, "test-aim", act[1])
    elif act and act[0] == "fast":
        for c in act[1]:
            send(s, "test-aim", c)
            time.sleep(0.005)
    elif act and act[0] == "set":
        setting(act[1], act[2])
    time.sleep(0.5)
    got = shown(f"{i:02d}")
    ok = got == want
    fails += not ok
    print(f"{'ok  ' if ok else 'FAIL'} {i:02d} {what}: {'shown' if got else 'hidden'} (want {'shown' if want else 'hidden'})")
s.close()
subprocess.run([binp, "--quit"], env=env)
time.sleep(0.5)
proc.kill()
print("PASS" if fails == 0 else f"{fails} step(s) failed")
sys.exit(1 if fails else 0)
