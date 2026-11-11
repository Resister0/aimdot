#!/bin/bash
# Dev install on Linux: puts Aimdot's icon + a menu entry for THIS repo's build into your user folders, so the
# app menu, the taskbar and Alt+Tab show Aimdot's icon. The settings window's Wayland app_id / X11 class is
# link.sonela.Aimdot (settings/src/main.rs) - KDE finds the icon only when the .desktop file has that exact name.
# Build first:   cargo build --release -p crosshair -p crosshair-settings
# Remove again:  bash scripts/install-dev-linux.sh --remove
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
ID=link.sonela.Aimdot
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
ICONS="$DATA/icons/hicolor"
APPS="$DATA/applications"

refresh() {
    # AIMDOT_NO_REFRESH=1: skip the desktop's caches (used by test runs into a scratch folder)
    [ "${AIMDOT_NO_REFRESH:-}" = 1 ] && return 0
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true
    command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--remove" ]; then
    rm -f "$APPS/$ID.desktop" "$ICONS/scalable/apps/$ID.svg"
    for s in 16 24 32 48 64 128 256 512; do rm -f "$ICONS/${s}x${s}/apps/$ID.png"; done
    refresh
    echo "Removed the Aimdot menu entry + icons."
    exit 0
fi

EXE="$REPO/target/release/aimdot-settings"
[ -x "$EXE" ] || echo "Note: $EXE isn't built yet (cargo build --release -p crosshair -p crosshair-settings)."

for s in 16 24 32 48 64 128 256 512; do
    install -Dm644 "$REPO/assets/icons/aimdot-$s.png" "$ICONS/${s}x${s}/apps/$ID.png"
done
install -Dm644 "$REPO/assets/icons/aimdot.svg" "$ICONS/scalable/apps/$ID.svg"

mkdir -p "$APPS"
cat > "$APPS/$ID.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Aimdot
GenericName=Crosshair overlay
Comment=A light crosshair overlay - change your crosshair here
Exec="$EXE"
Icon=$ID
StartupWMClass=$ID
Categories=Game;
Keywords=crosshair;overlay;aim;
Terminal=false
DESKTOP

refresh
echo "Installed: $APPS/$ID.desktop + icons in $ICONS (Exec: $EXE)"
