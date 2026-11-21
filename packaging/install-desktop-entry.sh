#!/bin/sh
# Install a menu entry and icon for the current user (no root needed).
# Usage: packaging/install-desktop-entry.sh /path/to/build/modlock
set -eu
binary=$(readlink -f "${1:?path to modlock}")
here=$(dirname "$(readlink -f "$0")")
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"
mkdir -p "$apps" "$icons"
python3 - "$here" "$binary" "$apps" <<'PYTHON'
from pathlib import Path
import sys
here,binary,apps=sys.argv[1:]
sys.path.insert(0,here)
from desktop_exec import quote_argument
text=(Path(here)/'modlock.desktop.in').read_text().replace('@BINARY@','/usr/bin/env '+quote_argument(binary))
(Path(apps)/'modlock.desktop').write_text(text)
PYTHON
cp "$here/modlock.svg" "$icons/modlock.svg"
if [ -e "$apps/deadlock-mods.desktop" ] || [ -L "$apps/deadlock-mods.desktop" ]; then
    printf 'An old launcher is still present: %s\n' "$apps/deadlock-mods.desktop"
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true
echo "Installed $apps/modlock.desktop"
if command -v xdg-mime >/dev/null; then
    for scheme in dlmm deadlock-mod-manager deadlock-modmanager; do
        xdg-mime default modlock.desktop "x-scheme-handler/$scheme"
    done
fi
