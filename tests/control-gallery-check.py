#!/usr/bin/env python3
"""Capture real control states in short private paths; keep only requested PNGs."""
import argparse
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import zlib


def require(condition, message):
    if not condition:
        sys.exit(str(message))


repo = Path(__file__).resolve().parent.parent
# Verify the cells that the running gallery actually renders.
styled = {p.stem for p in (repo / "style").glob("*.qml")} - {"Tokens"}
required_states = {
    "Button": ("checked hovered", "checked disabled"),
    "Flat Button": ("checked hovered", "checked disabled"),
    "ToolButton": ("checked pressed", "checked disabled"),
    "Switch": ("checked disabled",),
    "CheckBox": ("checked disabled", "disabled pressed"),
    "RadioButton": ("checked focus", "checked disabled"),
    "TabButton": ("checked pressed", "checked disabled"),
    "ItemDelegate": ("highlighted", "highlighted disabled"),
    "MenuItem": ("highlighted", "pressed", "checked", "checked disabled", "checked highlighted"),
    "ScrollBar": ("default", "hovered", "pressed", "inactive"),
    "ScrollIndicator": ("default", "inactive"),
}

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", type=Path)
parser.add_argument("--capture-dir", type=Path)
args = parser.parse_args()
images = []
for theme in ("dark", "light"):
    with tempfile.TemporaryDirectory(prefix="g-") as scratch:
        env = {k: v for k, v in os.environ.items()
               if not k.startswith("XDG_") and k not in ("DISPLAY", "WAYLAND_DISPLAY", "QML_DISK_CACHE_PATH")}
        for i, key in enumerate(("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
                                 "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "TMPDIR")):
            folder = Path(scratch) / str(i)
            folder.mkdir(mode=0o700)
            env[key] = str(folder)
        env.update(XDG_CONFIG_DIRS=env["XDG_CONFIG_HOME"], XDG_DATA_DIRS=env["XDG_DATA_HOME"],
                   QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software", MODLOCK_GALLERY_THEME=theme,
                   QT_QUICK_CONTROLS_STYLE="DeadlockStyle", QT_SCALE_FACTOR="1",
                   QT_FORCE_STDERR_LOGGING="1", QT_LOGGING_RULES="*.info=true", MODLOCK_UI_TEST_ROOT=scratch)
        path = Path(scratch) / "gallery.png"
        run = subprocess.run([str(args.binary.resolve()), "--demo", "--screenshot-page", "gallery", "--screenshot", str(path)],
                             env=env, capture_output=True, text=True, timeout=18)
        require(run.returncode == 0, str(run.returncode) + run.stdout + run.stderr)
        require("GALLERY_THEME " + theme in run.stderr, run.stderr)
        cells = re.findall(r"^GALLERY_CELL ([^\t\r\n]+)\t([^\r\n]+)$", run.stderr, re.M)
        labels = [label for label, state in cells]
        require(len(labels) == len(set(labels)), "duplicate gallery captions")
        rendered = {}
        for label, state in cells:
            control = label.partition(" · ")[0]
            rendered.setdefault(control, set()).add(state)
        missing = styled - rendered.keys()
        require(not missing, f"styled controls without gallery cells: {sorted(missing)}")
        for control, states in required_states.items():
            missing = set(states) - rendered.get(control, set())
            require(not missing, f"missing colour states for {control}: {sorted(missing)}")
        require("Play split button · focus" in labels, "missing Play focus cell")
        data = path.read_bytes()
        require(data[:8] == b"\x89PNG\r\n\x1a\n", "invalid PNG signature")
        width, height = struct.unpack(">II", data[16:24])
        compressed = b""
        offset = 8
        while offset < len(data):
            size = struct.unpack(">I", data[offset:offset+4])[0]
            if data[offset+4:offset+8] == b"IDAT": compressed += data[offset+8:offset+8+size]
            offset += size + 12
        pixels = zlib.decompress(compressed)
        require(len(set(pixels)) > 32, "empty or flat image")
        images.append(data)
        print(f"PASS {theme}: {len(labels)} labelled cells, {width}x{height}")
require(images[0] != images[1], "theme override did not affect capture")
if args.capture_dir:
    args.capture_dir.mkdir(parents=True, exist_ok=True)
    for theme, image in zip(("dark", "light"), images):
        path = args.capture_dir / f"gallery-{theme}.png"
        path.unlink(missing_ok=True)
        path.write_bytes(image)
        print(path)
