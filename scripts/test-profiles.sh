#!/bin/bash
# Profiles page + per-game switch test in a headless KWin (nothing on the real screen, real settings untouched).
# Uses docs/test-bus.conf so the test bus can't start portals (docs/headless-kwin-portal-fix.md).
#
#   bash scripts/test-profiles.sh [bin folder]      (default: target/debug)
#
# Needs kwin_wayland, Xwayland, xmessage, spectacle, python3. Screenshots + logs: $TMPDIR/aimdot-profiles/.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=$(realpath "${1:-$REPO/target/debug}")
OUT=${TMPDIR:-/tmp}/aimdot-profiles
rm -rf "$OUT"; mkdir -p "$OUT"
rc=0
for mode in norules rules; do
    H=$(mktemp -d /tmp/adp.XXXX) # short: the overlay's socket lives here
    {
        printf '; Aimdot configuration\n[app]\noverlay_on=1\npage=6\nactive_profile=Main\n'
        [ "$mode" = rules ] && printf 'games=xmessage=Sniper|cs2=Main\n'
        printf '\n[current]\ndot=1\ninner=0\ndot_size=6\ncolor=#00FF7F\n'
        printf '\n[profile:Main]\ndot=1\ninner=0\ndot_size=6\ncolor=#00FF7F\n'
        printf '\n[profile:Sniper]\ninner=0\ncircle=1\ncircle_radius=20\ncolor=#FF3030\ndot=1\n'
        printf '\n[profile:Classic]\ninner=1\n'
    } > "$H/config.ini"
    cat > "$H/inner.sh" <<EOF
#!/bin/bash
python3 -I "$REPO/scripts/test_profiles.py" "$BIN" "$H" "$OUT/$mode" $mode > "$OUT/$mode.txt" 2>&1
echo \$? > "$OUT/$mode.rc"
EOF
    chmod +x "$H/inner.sh"
    timeout 180 dbus-run-session --config-file="$REPO/docs/test-bus.conf" -- \
        kwin_wayland --virtual --xwayland --socket "ch-prof-$$" --width 1280 --height 1700 --exit-with-session "$H/inner.sh" \
        > "$OUT/kwin-$mode.log" 2>&1
    echo "== $mode"
    cat "$OUT/$mode.txt"
    [ "$(cat "$OUT/$mode.rc" 2>/dev/null)" = 0 ] || rc=1
    rm -rf "$H"
done
systemctl --user is-active --quiet xdg-document-portal.service && [ -d /run/user/$(id -u)/doc/by-app ] \
    || { echo "WARNING: xdg-document-portal is not active - fix: systemctl --user restart xdg-document-portal.service"; rc=1; }
exit $rc
