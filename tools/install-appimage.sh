#!/usr/bin/env bash
# Explicit user action only; respects HOME/XDG_DATA_HOME for isolated tests.
set -euo pipefail
[[ $# == 1 && -f $1 ]] || { echo "Usage: $0 path/to/Modlock-…-x86_64.AppImage" >&2; exit 2; }
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
applications="$HOME/Applications"
data=${XDG_DATA_HOME:-$HOME/.local/share}
mkdir -p "$applications" "$data/applications" "$data/icons/hicolor/scalable/apps"
target="$applications/Modlock-x86_64.AppImage"
staging=$(mktemp "$applications/.dlmm-install.XXXXXX")
trap 'rm -f -- "$staging"' EXIT
cp -- "$1" "$staging"
chmod 755 "$staging"
mv -f -- "$staging" "$target"
python3 - "$repo" "$target" "$data" <<'PY'
from pathlib import Path
import sys
repo, target, data = map(Path, sys.argv[1:])
sys.path.insert(0,str(repo / 'packaging'))
from desktop_exec import quote_argument
text = (repo / 'packaging/modlock.desktop.in').read_text().replace('@BINARY@', '/usr/bin/env ' + quote_argument(str(target)))
(data / 'applications/modlock.desktop').write_text(text)
(data / 'icons/hicolor/scalable/apps/modlock.svg').write_bytes((repo / 'packaging/modlock.svg').read_bytes())
PY
if [[ -e "$data/applications/deadlock-mods.desktop" || -L "$data/applications/deadlock-mods.desktop" ]]; then
    printf 'An old launcher is still present: %s\n' "$data/applications/deadlock-mods.desktop"
fi
if command -v update-desktop-database >/dev/null; then
    update-desktop-database "$data/applications"
fi
printf 'Installed %s\nLauncher: %s\n' "$target" "$data/applications/modlock.desktop"

if command -v xdg-mime >/dev/null; then
    for scheme in dlmm deadlock-mod-manager deadlock-modmanager; do
        xdg-mime default modlock.desktop "x-scheme-handler/$scheme"
    done
fi
