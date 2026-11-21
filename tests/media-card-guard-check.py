#!/usr/bin/env python3
"""Exercise hidden and revealed thumbnails through real list/grid delegate reuse."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from integrationcheck import raster


with tempfile.TemporaryDirectory(prefix="m124-") as scratch:
    root = Path(scratch)
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("XDG_") and key not in ("DISPLAY", "WAYLAND_DISPLAY")}
    for key, leaf in zip(("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
                          "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "TMPDIR"), "hcdksrt"):
        folder = root / leaf
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    first, second = root / "first.png", root / "second.png"
    raster(first, (210, 45, 65))
    raster(second, (30, 115, 220))
    env.update(XDG_CONFIG_DIRS=env["XDG_CONFIG_HOME"], XDG_DATA_DIRS=env["XDG_DATA_HOME"],
               QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software", QT_FORCE_STDERR_LOGGING="1",
               QML_DISK_CACHE_PATH=str(root / "k" / "qml"), QT_LOGGING_RULES="*.debug=false",
               UI_PREVIEW_FIRST=first.as_uri(), UI_PREVIEW_SECOND=second.as_uri(),
               UI_PREVIEW_ALTERNATE_HIDDEN="1")
    for shared_url in (False, True):
        env["UI_PREVIEW_SECOND"] = first.as_uri() if shared_url else second.as_uri()
        result = subprocess.run([sys.argv[1], "--demo", "--rows", "240", "--check-ui", sys.argv[2]],
                                env=env, timeout=25)
        if result.returncode:
            raise SystemExit(result.returncode)
        print("PASS reuse with", "shared" if shared_url else "distinct", "image URLs", flush=True)
