#!/bin/bash
# Hide / show test of the Linux overlay in a headless KWin (nothing on the real screen, real settings untouched):
# Wayland layer-shell, then the X11 window through Xwayland. Uses docs/test-bus.conf so the test bus can't start
# portals (they would break the real Flatpak document portal - docs/headless-kwin-portal-fix.md).
#
#   bash scripts/test-hide-show.sh [path/to/aimdot]      (default: target/release/aimdot)
#
# Needs kwin_wayland, Xwayland, spectacle, python3 + Pillow. Screenshots + logs: $TMPDIR/aimdot-hide-show/.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=$(realpath "${1:-$REPO/target/release/aimdot}")
SCALE=${SCALE:-1}   # SCALE=1.25 tests a fractional scale
OUT=${TMPDIR:-/tmp}/aimdot-hide-show
rm -rf "$OUT"; mkdir -p "$OUT"
rc=0
for mode in wayland x11; do
    H=$(mktemp -d /tmp/adh.XXXX) # short: the overlay's socket lives here
    printf '; Aimdot configuration\n[app]\noverlay_on=1\nmonitor=0\ndisplay_mode=0\naim_mode=1\n' > "$H/config.ini"
    [ -n "${EXTRA_CFG:-}" ] && printf '%s\n' "$EXTRA_CFG" >> "$H/config.ini"   # e.g. EXTRA_CFG=show_fade=1
    cat > "$H/inner.sh" <<EOF
#!/bin/bash
python3 -I "$REPO/scripts/test_hide_show.py" "$BIN" "$H" "$OUT/$mode" $mode > "$OUT/$mode.txt" 2>&1
echo \$? > "$OUT/$mode.rc"
EOF
    chmod +x "$H/inner.sh"
    timeout 180 dbus-run-session --config-file="$REPO/docs/test-bus.conf" -- \
        kwin_wayland --virtual --xwayland --socket "ch-test-$$" --width 1280 --height 720 --scale "$SCALE" --exit-with-session "$H/inner.sh" \
        > "$OUT/kwin-$mode.log" 2>&1
    echo "== $mode"
    cat "$OUT/$mode.txt"
    [ "$(cat "$OUT/$mode.rc" 2>/dev/null)" = 0 ] || rc=1
    rm -rf "$H"
done
# The real Flatpak document portal must still be up.
systemctl --user is-active --quiet xdg-document-portal.service && [ -d /run/user/$(id -u)/doc/by-app ] \
    || { echo "WARNING: xdg-document-portal is not active - fix: systemctl --user restart xdg-document-portal.service"; rc=1; }
exit $rc
