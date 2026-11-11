#!/bin/bash
# Linux hotkeys test: the overlay against a FAKE GlobalShortcuts portal (scripts/test_hotkeys.py) on a private
# test bus inside a headless KWin. Never touches the real session bus (that would register shortcuts in the real
# KDE) or the real screen. docs/test-bus.conf = a bus that can't auto-start anything (no real portals).
#
#   bash scripts/test-hotkeys.sh [path/to/aimdot]      (default: target/release/aimdot)
#
# Second part: no portal + the X11 window (Xwayland) - the overlay grabs the keys itself, xdotool presses them.
# The test copies run with CROSSHAIR_HOTKEYS=portal / x11: they never read the real keyboards (evdev). The evdev
# combo logic is tested first with recorded key events (cargo test, keys::tests) - also never the real keyboard.
# Needs kwin_wayland, Xwayland, xdotool, python3 + dbus-python + gi. Logs: $TMPDIR/aimdot-hotkeys/.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=$(realpath "${1:-$REPO/target/release/aimdot}")
OUT=${TMPDIR:-/tmp}/aimdot-hotkeys
rm -rf "$OUT"; mkdir -p "$OUT"
(cd "$REPO" && cargo test -q -p crosshair keys::tests 2>&1 | grep -E "test result|FAILED|panicked") > "$OUT/evdev.txt"
grep -q "test result: ok" "$OUT/evdev.txt" && echo "PASS evdev combo logic (recorded key events)" || { echo "FAIL evdev combo logic"; cat "$OUT/evdev.txt"; }
H=$(mktemp -d /tmp/adk.XXXX) # short: the overlay's socket lives here
printf '; Aimdot configuration\n[app]\noverlay_on=1\naim_mode=0\n[profile:Alpha]\n[profile:Beta]\n' > "$H/config.ini"
cat > "$H/inner.sh" <<EOS
#!/bin/bash
python3 -I "$REPO/scripts/test_hotkeys.py" drive "$BIN" "$H" "$OUT" > "$OUT/result.txt" 2>&1
r1=\$?
rm -f "$H/aimdot.sock"
printf '; Aimdot configuration\\n[app]\\noverlay_on=1\\naim_mode=0\\n[profile:Alpha]\\n[profile:Beta]\\n' > "$H/config.ini"
python3 -I "$REPO/scripts/test_hotkeys.py" x11 "$BIN" "$H" "$OUT" >> "$OUT/result.txt" 2>&1
r2=\$?
echo \$((r1 + r2)) > "$OUT/rc"
EOS
chmod +x "$H/inner.sh"
timeout 120 dbus-run-session --config-file="$REPO/docs/test-bus.conf" -- \
    kwin_wayland --virtual --xwayland --socket "ch-test-$$" --width 1280 --height 720 --exit-with-session "$H/inner.sh" > "$OUT/kwin.log" 2>&1
cat "$OUT/result.txt"
rm -rf "$H"
rc=$(cat "$OUT/rc" 2>/dev/null || echo 1)
grep -q "test result: ok" "$OUT/evdev.txt" || rc=1
systemctl --user is-active --quiet xdg-document-portal.service && [ -d /run/user/$(id -u)/doc/by-app ] \
    || { echo "WARNING: xdg-document-portal is not active - fix: systemctl --user restart xdg-document-portal.service"; rc=1; }
exit $rc
