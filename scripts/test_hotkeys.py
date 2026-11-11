#!/usr/bin/env python3
"""Hotkey test for the Linux overlay against a FAKE GlobalShortcuts portal - run inside a headless KWin on the
private test bus by scripts/test-hotkeys.sh. Never on the real session bus: there it would register shortcuts in
the real KDE.

    test_hotkeys.py portal <log file>                       the fake portal (owns org.freedesktop.portal.Desktop)
    test_hotkeys.py drive <aimdot binary> <test folder> <output folder>
    test_hotkeys.py x11 <aimdot binary> <test folder> <output folder>   no portal: the X11 window grabs the keys

The fake behaves like KDE where it matters: the first BindShortcuts takes the preferred triggers and keeps them;
later binds of the same ids keep the first keys (KDE ignores a changed preferred trigger). Needs dbus-python + gi.
"""
import json, os, socket, struct, subprocess, sys, time

DBUS_TEST = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")


def nice(trigger):
    """CTRL+ALT+x -> Ctrl+Alt+X (how KDE's trigger_description reads)."""
    names = {"CTRL": "Ctrl", "ALT": "Alt", "SHIFT": "Shift", "LOGO": "Meta"}
    parts = trigger.split("+")
    return "+".join([names.get(p, p) for p in parts[:-1]] + [parts[-1].upper() if len(parts[-1]) == 1 else parts[-1]])


def portal(logpath):
    import dbus, dbus.service, dbus.mainloop.glib
    from gi.repository import GLib

    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    log = open(logpath, "a", buffering=1)
    GS = "org.freedesktop.portal.GlobalShortcuts"
    keys = {}  # id -> trigger text, kept from the first bind (like KDE)
    sessions = {}  # session path -> owner

    def emit_response(path, results):
        class Req(dbus.service.Object):
            @dbus.service.signal("org.freedesktop.portal.Request", signature="ua{sv}")
            def Response(self, code, res):
                pass
        r = Req(bus, path)
        GLib.timeout_add(30, lambda: (r.Response(dbus.UInt32(0), results), r.remove_from_connection(), False)[-1])

    class Portal(dbus.service.Object):
        @dbus.service.method("org.freedesktop.host.portal.Registry", in_signature="sa{sv}", out_signature="")
        def Register(self, app_id, opt):
            log.write(json.dumps({"call": "Register", "app_id": str(app_id)}) + "\n")

        @dbus.service.method(GS, in_signature="a{sv}", out_signature="o", sender_keyword="sender")
        def CreateSession(self, opt, sender=None):
            who = sender.lstrip(":").replace(".", "_")
            sp = f"/org/freedesktop/portal/desktop/session/{who}/{opt['session_handle_token']}"
            sessions[sp] = sender
            rp = f"/org/freedesktop/portal/desktop/request/{who}/{opt['handle_token']}"
            log.write(json.dumps({"call": "CreateSession", "session": sp}) + "\n")
            emit_response(rp, {"session_handle": dbus.String(sp)})
            return dbus.ObjectPath(rp)

        @dbus.service.method(GS, in_signature="oa(sa{sv})sa{sv}", out_signature="o", sender_keyword="sender")
        def BindShortcuts(self, session, shortcuts, parent, opt, sender=None):
            who = sender.lstrip(":").replace(".", "_")
            got = []
            for sid, props in shortcuts:
                sid = str(sid)
                pref = str(props.get("preferred_trigger", ""))
                if sid not in keys:
                    keys[sid] = nice(pref) if pref else ""
                got.append({"id": sid, "description": str(props.get("description", "")), "preferred": pref})
            log.write(json.dumps({"call": "BindShortcuts", "session": str(session), "shortcuts": got}) + "\n")
            rp = f"/org/freedesktop/portal/desktop/request/{who}/{opt['handle_token']}"
            res = dbus.Array([dbus.Struct((dbus.String(g["id"]), dbus.Dictionary(
                {"description": dbus.String(g["description"]), "trigger_description": dbus.String(keys[g["id"]])},
                signature="sv"))) for g in got], signature="(sa{sv})")
            emit_response(rp, {"shortcuts": res})
            self.last = str(session)
            return dbus.ObjectPath(rp)

        @dbus.service.signal(GS, signature="osta{sv}")
        def Activated(self, session, sid, ts, opt):
            pass

        @dbus.service.signal(GS, signature="oa(sa{sv})")
        def ShortcutsChanged(self, session, shortcuts):
            pass

        # --- test controls ---
        @dbus.service.method("test.FakePortal", in_signature="ss", out_signature="")
        def Fire(self, sid, token):
            opt = {"activation_token": dbus.String(token)} if token else {}
            self.Activated(dbus.ObjectPath(self.last), sid, dbus.UInt64(int(time.time() * 1000)), dbus.Dictionary(opt, signature="sv"))

        @dbus.service.method("test.FakePortal", in_signature="ss", out_signature="")
        def Change(self, sid, trig):
            keys[str(sid)] = str(trig)
            self.ShortcutsChanged(dbus.ObjectPath(self.last), dbus.Array([dbus.Struct((dbus.String(sid), dbus.Dictionary(
                {"trigger_description": dbus.String(trig)}, signature="sv")))], signature="(sa{sv})"))

    p = Portal(bus, "/org/freedesktop/portal/desktop")
    p.last = "/"
    name = dbus.service.BusName("org.freedesktop.portal.Desktop", bus)  # keep: dropping it releases the name
    log.write(json.dumps({"call": "ready"}) + "\n")
    GLib.MainLoop().run()


def drive(binp, home, out):
    import dbus
    os.makedirs(out, exist_ok=True)
    plog = os.path.join(out, "portal.log")
    fake = subprocess.Popen([sys.executable, "-I", __file__, "portal", plog])
    for _ in range(50):
        if os.path.exists(plog) and "ready" in open(plog).read():
            break
        time.sleep(0.1)
    env = dict(os.environ, CROSSHAIR_HOME=home, CROSSHAIR_HOTKEYS="portal")
    olog = open(os.path.join(out, "overlay.log"), "w")
    proc = subprocess.Popen([binp, "--tray"], env=env, stdout=olog, stderr=olog)
    sock = os.path.join(home, "aimdot.sock")
    for _ in range(50):
        if os.path.exists(sock):
            break
        time.sleep(0.1)
    time.sleep(1.0)
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock)
    s.settimeout(0.3)
    msgs = []

    def pump(t=0.6):
        end = time.time() + t
        while time.time() < end:
            try:
                n = struct.unpack("<I", s.recv(4, socket.MSG_WAITALL))[0]
                k, _, b = s.recv(n, socket.MSG_WAITALL).decode().partition("\n")
                msgs.append((k, b))
            except socket.timeout:
                pass

    def last(kind):
        return next((b for k, b in reversed(msgs) if k == kind), "")

    def kv(body, key):
        return next((l.split("=", 1)[1] for l in body.split("\n") if l.startswith(key + "=")), None)

    def send(kind, body):
        b = (kind + "\n" + body).encode()
        s.sendall(struct.pack("<I", len(b)) + b)

    bus = dbus.SessionBus()
    fp = dbus.Interface(bus.get_object("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop"), "test.FakePortal")
    calls = lambda: [json.loads(l) for l in open(plog)]
    fails = 0

    def check(what, ok, detail=""):
        nonlocal fails
        fails += not ok
        print(f"{'ok  ' if ok else 'FAIL'} {what}" + (f" ({detail})" if detail else ""))

    pump(1.0)
    c = calls()
    check("registered app id link.sonela.Aimdot", any(x["call"] == "Register" and x["app_id"] == "link.sonela.Aimdot" for x in c))
    binds = [x for x in c if x["call"] == "BindShortcuts"]
    check("one BindShortcuts with 8 ids", len(binds) == 1 and [g["id"] for g in binds[0]["shortcuts"]] == ["toggle", "next", "prev", "settings", "nudge_up", "nudge_down", "nudge_left", "nudge_right"])
    check("preferred triggers from the settings", binds and [g["preferred"] for g in binds[0]["shortcuts"]] == ["CTRL+ALT+x", "CTRL+ALT+Right", "CTRL+ALT+Left", "CTRL+ALT+e",
          "CTRL+ALT+SHIFT+Up", "CTRL+ALT+SHIFT+Down", "CTRL+ALT+SHIFT+Left", "CTRL+ALT+SHIFT+Right"],
          binds and str([g["preferred"] for g in binds[0]["shortcuts"]]))
    K = "Ctrl+Alt+X|Ctrl+Alt+Right|Ctrl+Alt+Left|Ctrl+Alt+E|Ctrl+Alt+Shift+Up|Ctrl+Alt+Shift+Down|Ctrl+Alt+Shift+Left|Ctrl+Alt+Shift+Right"
    check("info hk_kde = KDE's keys", kv(last("info"), "hk_kde") == K, kv(last("info"), "hk_kde"))
    state = last("state")
    check("overlay on at start", kv(state, "overlay_on") == "1")

    fp.Fire("toggle", "")
    pump()
    check("Activated toggle -> overlay off", kv(last("state"), "overlay_on") == "0", kv(last("state"), "overlay_on"))
    fp.Fire("toggle", "")
    pump()
    check("Activated toggle again -> overlay on", kv(last("state"), "overlay_on") == "1")
    seen = []
    for sid in ["next", "next", "prev"]:
        fp.Fire(sid, "")
        pump()
        seen.append(kv(last("state"), "active_profile"))
    check("Activated next, next, prev -> Alpha, Beta, Alpha", seen == ["Alpha", "Beta", "Alpha"], str(seen))
    n_raise = sum(1 for k, _ in msgs if k == "raise")
    fp.Fire("settings", "")
    pump()
    check("Activated settings, window open -> raise", sum(1 for k, _ in msgs if k == "raise") == n_raise + 1)

    for sid, want in [("nudge_up", ("0", "-1")), ("nudge_right", ("1", "-1")), ("nudge_down", ("1", "0")), ("nudge_left", ("0", "0"))]:
        fp.Fire(sid, "")
        pump(0.4)
        got = (kv(last("state"), "offset_x"), kv(last("state"), "offset_y"))
        check(f"Activated {sid} -> offset {want}", got == want, str(got))
    send("capture", "1")
    pump(0.3)
    fp.Fire("toggle", "")
    pump()
    check("no hotkey while the settings capture a key", kv(last("state"), "overlay_on") == "1")
    send("capture", "0")
    pump(0.3)

    fp.Change("next", "Meta+N")
    pump()
    check("ShortcutsChanged -> hk_kde updated", kv(last("info"), "hk_kde") == K.replace("Ctrl+Alt+Right", "Meta+N", 1), kv(last("info"), "hk_kde"))

    st = last("state").replace("hk_toggle=3,88", "hk_toggle=3,90")  # Ctrl+Alt+Z
    send("state", st)
    pump(1.0)
    binds = [x for x in calls() if x["call"] == "BindShortcuts"]
    check("key changed in settings -> bound again with the new preferred key", len(binds) == 2 and binds[1]["shortcuts"][0]["preferred"] == "CTRL+ALT+z",
          f"{len(binds)} binds")
    check("... KDE keeps its key (shown as it is)", (kv(last("info"), "hk_kde") or "").startswith("Ctrl+Alt+X|"), kv(last("info"), "hk_kde"))
    send("state", st)  # same keys again: no new bind
    pump(0.6)
    check("same keys again -> no extra bind", len([x for x in calls() if x["call"] == "BindShortcuts"]) == 2)

    # Idle: the hotkeys thread must sleep (no timer).
    def ctx():
        for t in os.listdir(f"/proc/{proc.pid}/task"):
            if open(f"/proc/{proc.pid}/task/{t}/comm").read().strip() == "hotkeys":
                return int(next(l.split()[1] for l in open(f"/proc/{proc.pid}/task/{t}/status") if l.startswith("voluntary_ctxt")))
    a = ctx()
    time.sleep(5)
    b = ctx()
    check("hotkeys thread idle 5 s: 0 wake-ups", a is not None and a == b, f"{a} -> {b}")
    rss = next(l.split()[1] for l in open(f"/proc/{proc.pid}/status") if l.startswith("VmRSS"))
    anon = next(l.split()[1] for l in open(f"/proc/{proc.pid}/status") if l.startswith("RssAnon"))
    print(f"rss {rss} kB, anon {anon} kB")

    s.close()
    # No settings window connected: the settings hotkey starts one, with KWin's activation token.
    if os.path.exists(os.path.join(os.path.dirname(binp), "aimdot-settings")):
        time.sleep(0.5)
        fp.Fire("settings", "tok123")
        time.sleep(1.5)
        started = []
        for pid in filter(str.isdigit, os.listdir("/proc")):
            try:
                envs = open(f"/proc/{pid}/environ", "rb").read().split(b"\0")
                if f"CROSSHAIR_HOME={home}".encode() in envs and open(f"/proc/{pid}/comm").read().startswith("aimdot-settin"):
                    started.append((int(pid), b"XDG_ACTIVATION_TOKEN=tok123" in envs))
            except OSError:
                pass
        check("Activated settings, no window -> settings started with the activation token", any(t for _, t in started), str(started))
        for pid, _ in started:
            os.kill(pid, 9)
    else:
        print("skip: no aimdot-settings next to the overlay")
    subprocess.run([binp, "--quit"], env=env)
    time.sleep(0.5)
    proc.kill()
    fake.kill()
    print("PASS" if fails == 0 else f"{fails} check(s) failed")
    return 1 if fails else 0


def drive_x11(binp, home, out):
    """No shortcuts portal + X11 window: the overlay grabs the keys itself; xdotool presses them. Runs on a nested
    Xephyr inside the headless KWin's Xwayland (XTEST key presses don't reach grabs through Xwayland itself)."""
    os.makedirs(out, exist_ok=True)
    if not os.environ.get("WAYLAND_DISPLAY", "").startswith("ch-test") or os.environ.get("DISPLAY") in (None, "", ":0"):
        sys.exit("refusing: the X11 part must run inside the headless test KWin (never on the real display)")
    n = 70 + os.getpid() % 20
    xephyr = subprocess.Popen(["Xephyr", f":{n}", "-screen", "1280x720x24", "-ac", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(50):
        if os.path.exists(f"/tmp/.X11-unix/X{n}"):
            break
        time.sleep(0.1)
    time.sleep(0.5)
    env = dict(os.environ, CROSSHAIR_HOME=home, CROSSHAIR_X11="1", CROSSHAIR_HOTKEYS="x11", DISPLAY=f":{n}")
    olog = open(os.path.join(out, "overlay-x11.log"), "w")
    proc = subprocess.Popen([binp, "--tray"], env=env, stdout=olog, stderr=olog)
    sock = os.path.join(home, "aimdot.sock")
    for _ in range(50):
        if os.path.exists(sock):
            break
        time.sleep(0.1)
    time.sleep(1.0)
    s = socket.socket(socket.AF_UNIX)
    s.connect(sock)
    s.settimeout(0.3)
    msgs = []

    def pump(t=0.6):
        end = time.time() + t
        while time.time() < end:
            try:
                n = struct.unpack("<I", s.recv(4, socket.MSG_WAITALL))[0]
                k, _, b = s.recv(n, socket.MSG_WAITALL).decode().partition("\n")
                msgs.append((k, b))
            except socket.timeout:
                pass

    last = lambda kind: next((b for k, b in reversed(msgs) if k == kind), "")
    kv = lambda body, key: next((l.split("=", 1)[1] for l in body.split("\n") if l.startswith(key + "=")), None)

    def send(kind, body):
        b = (kind + "\n" + body).encode()
        s.sendall(struct.pack("<I", len(b)) + b)

    key = lambda k: subprocess.run(["xdotool", "key", k], env=env, capture_output=True)
    fails = 0

    def check(what, ok, detail=""):
        nonlocal fails
        fails += not ok
        print(f"{'ok  ' if ok else 'FAIL'} x11: {what}" + (f" ({detail})" if detail else ""))

    pump(1.0)
    info = last("info")
    check("platform x11, no portal -> no hk_kde, all keys grabbed", kv(info, "platform") == "x11" and kv(info, "hk_kde") is None and kv(info, "hk_ok") == "1,1,1,1,1,1,1,1", info.replace("\n", " "))
    key("ctrl+alt+x")
    pump()
    check("Ctrl+Alt+X -> overlay off", kv(last("state"), "overlay_on") == "0")
    key("ctrl+alt+x")
    pump()
    check("Ctrl+Alt+X again -> overlay on", kv(last("state"), "overlay_on") == "1")
    key("ctrl+alt+Right")
    pump()
    check("Ctrl+Alt+Right -> next profile", kv(last("state"), "active_profile") == "Alpha", kv(last("state"), "active_profile"))
    subprocess.run(["xdotool", "key", "Num_Lock"], env=env)
    key("ctrl+alt+x")
    pump()
    check("works with Num Lock on too", kv(last("state"), "overlay_on") == "0")
    subprocess.run(["xdotool", "key", "Num_Lock"], env=env)
    key("ctrl+alt+shift+Up")
    pump(0.4)
    check("Ctrl+Alt+Shift+Up -> 1 px up", kv(last("state"), "offset_y") == "-1", kv(last("state"), "offset_y"))
    # Held keys: the nudge repeats, show/hide doesn't (X auto-repeat).
    on0 = kv(last("state"), "overlay_on")
    subprocess.run(["xdotool", "keydown", "ctrl+alt+x"], env=env)
    time.sleep(1.2)
    subprocess.run(["xdotool", "keyup", "ctrl+alt+x"], env=env)
    pump(0.6)
    flips = sum(1 for k, b in msgs[-40:] if k == "state") 
    check("Ctrl+Alt+X held 1.2 s -> toggles once", kv(last("state"), "overlay_on") != on0, f"{on0} -> {kv(last('state'), 'overlay_on')}")
    subprocess.run(["xdotool", "keydown", "ctrl+alt+shift+Down"], env=env)
    time.sleep(1.2)
    subprocess.run(["xdotool", "keyup", "ctrl+alt+shift+Down"], env=env)
    pump(0.6)
    y = int(float(kv(last("state"), "offset_y") or 0))
    check("Ctrl+Alt+Shift+Down held 1.2 s -> repeats (several px)", y >= 2, f"offset_y {y}")
    key("ctrl+alt+x") if kv(last("state"), "overlay_on") == "1" else None
    pump(0.4)
    send("capture", "1")
    pump(0.3)
    key("ctrl+alt+x")
    pump()
    check("capture in the settings -> keys let go", kv(last("state"), "overlay_on") == "0")
    send("capture", "0")
    pump(0.3)
    st = last("state").replace("hk_toggle=3,88", "hk_toggle=3,90")
    send("state", st)
    pump()
    key("ctrl+alt+x")
    pump(0.4)
    key("ctrl+alt+z")
    pump()
    check("key changed to Ctrl+Alt+Z -> old key free, new key works", kv(last("state"), "overlay_on") == "1")
    s.close()
    subprocess.run([binp, "--quit"], env=env)
    time.sleep(0.5)
    proc.kill()
    xephyr.kill()
    xephyr.wait()
    for f in (f"/tmp/.X11-unix/X{n}", f"/tmp/.X{n}-lock"):  # Xephyr killed: its socket + lock stay behind
        if os.path.exists(f):
            os.remove(f)
    print("PASS" if fails == 0 else f"{fails} check(s) failed")
    return 1 if fails else 0


if __name__ == "__main__":
    if not DBUS_TEST or "/run/user/" in DBUS_TEST:
        sys.exit("refusing: run through scripts/test-hotkeys.sh (private test bus), never on the real session bus")
    if sys.argv[1] == "portal":
        portal(sys.argv[2])
    elif sys.argv[1] == "x11":
        sys.exit(drive_x11(*sys.argv[2:5]))
    else:
        sys.exit(drive(*sys.argv[2:5]))
