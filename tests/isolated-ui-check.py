#!/usr/bin/env python3
"""Run a test command with short, disposable HOME and XDG paths."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

# Keep nested fixtures and AF_UNIX socket names short even when the build path
# or a user's TMPDIR is long. Each invocation owns and cleans up its tree.
with tempfile.TemporaryDirectory(prefix="ml-", dir="/tmp") as scratch:
    env = os.environ.copy()
    # cargo may be a rustup shim. Preserve the toolchain locations while
    # isolating application data underneath the new HOME.
    if not env.get("RUSTUP_HOME"):
        env["RUSTUP_HOME"] = str(Path.home() / ".rustup")
    if not env.get("CARGO_HOME"):
        env["CARGO_HOME"] = str(Path(scratch) / "cargo")
    for key in tuple(env):
        if key.startswith("XDG_") or key in ("DISPLAY", "WAYLAND_DISPLAY", "QML_DISK_CACHE_PATH"):
            env.pop(key, None)
    for key, leaf in zip(("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
                          "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "TMPDIR"), "hcdksrt"):
        folder = Path(scratch) / leaf
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    env.update(XDG_CONFIG_DIRS=env["XDG_CONFIG_HOME"], XDG_DATA_DIRS=env["XDG_DATA_HOME"],
               QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software", QT_FORCE_STDERR_LOGGING="1",
               DBUS_SESSION_BUS_ADDRESS="unix:path=" + scratch + "/no-bus",
               MODLOCK_UI_TEST_ROOT=scratch, PYTHONDONTWRITEBYTECODE="1")
    sys.exit(subprocess.run(sys.argv[1:], env=env).returncode)
