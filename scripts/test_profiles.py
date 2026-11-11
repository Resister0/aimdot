"""Profiles page + per-game switch test, run inside a headless KWin by scripts/test-profiles.sh.

    argv: <bin folder> <test folder (CROSSHAIR_HOME)> <output folder> <rules|norules>

rules:   the overlay starts with the rule "xmessage -> Sniper". An X11 window (xmessage) comes to the front ->
         the overlay must switch to "Sniper"; when it closes -> back to "Main". Screenshots of the Profiles page.
norules: no rules: the overlay must start no detection thread (thread count + memory are printed for both).
Exit code 0 = every check passed.
"""
import os
import re
import subprocess
import sys
import time

bindir, home, out, mode = sys.argv[1:5]
os.makedirs(out, exist_ok=True)
env = dict(os.environ, CROSSHAIR_HOME=home)
log = open(os.path.join(out, "programs.log"), "w")
fails = []


def shot(name):
    p = os.path.join(out, name + ".png")
    subprocess.run(["spectacle", "-b", "-n", "-f", "-o", p], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    print("screenshot", p)


def active():
    try:
        text = open(os.path.join(home, "config.ini")).read()
    except OSError:
        return None
    m = re.search(r"^active_profile=(.*)$", text, re.M)
    return m.group(1) if m else None


def wait_active(want, secs=6.0):
    end = time.time() + secs
    while time.time() < end:
        if active() == want:
            return True
        time.sleep(0.2)
    return False


def status(pid):
    d = {}
    for line in open(f"/proc/{pid}/status"):
        k, _, v = line.partition(":")
        d[k] = v.strip()
    return d


def check(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        fails.append(what)


exe = os.path.join(bindir, "aimdot")
print("binary:", exe, "built", time.strftime("%H:%M:%S", time.localtime(os.path.getmtime(exe))))
overlay = subprocess.Popen([exe, "--tray"], env=env, stdout=log, stderr=log)


def thread_names():
    try:
        return [open(f"/proc/{overlay.pid}/task/{t}/comm").read().strip() for t in sorted(os.listdir(f"/proc/{overlay.pid}/task"))]
    except OSError:
        return []


# wait until the overlay is up (its socket), then (with rules) up to 6 s for the detection thread
end = time.time() + 6.0
while time.time() < end and not os.path.exists(os.path.join(home, "aimdot.sock")):
    time.sleep(0.1)
end = time.time() + (6.0 if mode == "rules" else 2.0)
while time.time() < end and (mode != "rules" or "aimdot-games" not in thread_names()):
    time.sleep(0.1)
time.sleep(0.5)
st = status(overlay.pid)
print(f"overlay: threads={st['Threads']} VmRSS={st['VmRSS']}")
names = thread_names()
print("overlay threads:", names)

if mode == "norules":
    check("aimdot-games" not in names, "no game-detection thread without rules")
else:
    check("aimdot-games" in names, "game-detection thread runs with a rule")
    # an X11 program without a rule (shows in "Running programs", must not switch anything)
    gears = subprocess.Popen(["glxgears", "-geometry", "200x150+900+40"], stdout=log, stderr=log)
    time.sleep(1.0)
    settings = subprocess.Popen([os.path.join(bindir, "aimdot-settings")], env=dict(env, CROSSHAIR_PAGE="6", CROSSHAIR_PROFILES_DEMO="1", CROSSHAIR_WINDOW="1180x1640"), stdout=log, stderr=log)
    time.sleep(4.0)
    shot("1-profiles-page")
    check(active() == "Main", "starts on Main")
    xm = subprocess.Popen(["xmessage", "-geometry", "300x80+40+40", "A game window (X11)"], stdout=log, stderr=log)
    check(wait_active("Sniper"), "X11 'game' in front -> Sniper")
    time.sleep(0.5)
    shot("2-game-in-front")
    xm.terminate()
    xm.wait()
    check(wait_active("Main"), "game closed -> back to Main")
    time.sleep(0.5)
    shot("3-back")
    st = status(overlay.pid)
    print(f"overlay after: threads={st['Threads']} VmRSS={st['VmRSS']} ctxt={st.get('voluntary_ctxt_switches')}")
    settings.terminate()
    settings.wait()
    gears.terminate()
    gears.wait()

# CPU while idle (rules or not): the overlay must sleep
t0 = open(f"/proc/{overlay.pid}/stat").read().split()
time.sleep(3.0)
t1 = open(f"/proc/{overlay.pid}/stat").read().split()
ticks = (int(t1[13]) + int(t1[14])) - (int(t0[13]) + int(t0[14]))
print(f"overlay CPU ticks in 3 s idle: {ticks}")
check(ticks <= 1, "overlay idle (<= 1 tick in 3 s)")

overlay.terminate()
overlay.wait()
print("FAILED:", fails if fails else "none")
sys.exit(1 if fails else 0)
