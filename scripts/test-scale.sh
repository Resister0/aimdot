#!/bin/bash
# Pixel-perfect check (#12): crosshair size + centre in physical pixels at display scales 1 / 1.25 / 1.5 / 1.75 / 2
# and odd / even resolutions, with and without an X offset - in headless KWins on the private test bus
# (docs/test-bus.conf), never on the real screen. Expect the same size (18 px for the default crosshair) everywhere
# and centre == screen centre (on an odd screen size an even-sized crosshair can only be 0.5 px off).
#
#   bash scripts/test-scale.sh [path/to/aimdot]
REPO=$(cd "$(dirname "$0")/.." && pwd)
BIN=$(realpath "${1:-$REPO/target/release/aimdot}")
for combo in "1 1280 720" "1 1366 768" "1 1279 719" "1.25 1280 720" "1.25 1366 768" "1.5 1280 720" "1.5 1281 721" "1.75 1280 720" "2 1280 720" "2 1281 721"; do
    set -- $combo
    for off in 0 7; do
        H=$(mktemp -d /tmp/adz.XXXX)
        printf '#!/bin/bash\npython3 -I %s %s %s %s %s\n' "$REPO/scripts/scale_probe.py" "$BIN" "$H" "$H/s.png" "$off" > "$H/in.sh"
        chmod +x "$H/in.sh"
        echo -n "scale $1 logical $2x$3: "
        timeout 60 dbus-run-session --config-file="$REPO/docs/test-bus.conf" -- kwin_wayland --virtual --socket "ch-test-sc$$" \
            --width "$2" --height "$3" --scale "$1" --exit-with-session "$H/in.sh" 2>/dev/null | grep screen
        rm -rf "$H"
    done
done
# #47 size as % of the screen height (size_mode=1): 18 px at 1080p -> 12 at 720p, 24 at 1440p, 36 at 2160p, centred.
for combo in "1 1280 720" "1 1920 1080" "1 2560 1440" "1 3840 2160" "2 1920 1080"; do
    set -- $combo
    H=$(mktemp -d /tmp/adz.XXXX)
    printf '#!/bin/bash\nPROBE_CFG=size_mode=1 python3 -I %s %s %s %s 0\n' "$REPO/scripts/scale_probe.py" "$BIN" "$H" "$H/s.png" > "$H/in.sh"
    chmod +x "$H/in.sh"
    echo -n "size_mode=1, scale $1 logical $2x$3: "
    timeout 60 dbus-run-session --config-file="$REPO/docs/test-bus.conf" -- kwin_wayland --virtual --socket "ch-test-sz$$" \
        --width "$2" --height "$3" --scale "$1" --exit-with-session "$H/in.sh" 2>/dev/null | grep screen
    rm -rf "$H"
done
