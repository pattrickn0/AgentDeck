#!/bin/sh
# Adds Holt to the Linux app menu (GNOME, KDE and others that follow the freedesktop spec): writes
# ~/.local/share/applications/holt.desktop pointing at the built binary and installs the icon. The entry's
# name is the window's app id (`holt`), which is how Wayland desktops find the icon and the name.
#
#   scripts/install-desktop.sh                                     # target/release/holt
#   scripts/install-desktop.sh /path/to/holt                       # another binary
#   scripts/install-desktop.sh --uninstall
#
# `cargo build --release` already does this for target/release/holt (crates/app/build.rs); the script is for
# another binary or to uninstall. Run it again after moving the binary. Nothing outside ~/.local/share is touched.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
desktop="$data/applications/holt.desktop"
icon="$data/icons/hicolor/256x256/apps/holt.png"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data/applications" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$desktop" "$icon"
    refresh
    echo "removed $desktop and $icon"
    exit 0
fi

bin=${1:-$repo/target/release/holt}
if [ ! -x "$bin" ]; then
    echo "binary not found: $bin (run cargo build --release first)" >&2
    exit 1
fi
bin=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")

mkdir -p "$(dirname "$desktop")" "$(dirname "$icon")"
cp "$repo/crates/app/assets/holt.png" "$icon"
sed "s|@EXEC@|$bin|" "$repo/crates/app/assets/holt.desktop" > "$desktop"
refresh
echo "installed $desktop (Exec=$bin)"
