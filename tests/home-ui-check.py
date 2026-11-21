#!/usr/bin/env python3
"""Verify Home/demo XDG boundaries across cold startup, canonical config, and stale lock fixtures."""

import configparser
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

CANONICAL_DEFAULTS = {
    "automaticApply": "true",
    "colourMode": "modlock",
    "hideNsfwMods": "true",
    "reduceMotion": "false",
    "showMaturePreviews": "false",
}

CANONICAL_INI = (
    "[display]\n"
    + "".join(f"{key}={value}\n" for key, value in CANONICAL_DEFAULTS.items())
).encode("utf-8")

STALE_LOCK = f"999999999\nother\n{socket.gethostname()}\n".encode("utf-8")


def isolated_env(case_root: Path) -> tuple[dict[str, str], Path, Path]:
    case_root.mkdir(mode=0o700, exist_ok=True)
    env = os.environ.copy()
    for key in tuple(env):
        if key.startswith("XDG_") or key in ("DISPLAY", "WAYLAND_DISPLAY", "QML_DISK_CACHE_PATH"):
            env.pop(key, None)
    keys = (
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
    )
    for index, key in enumerate(keys):
        folder = case_root / str(index)
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    env['MODLOCK_UI_TEST_ROOT'] = str(case_root)
    env.update(
        XDG_CONFIG_DIRS=env["XDG_CONFIG_HOME"],
        XDG_DATA_DIRS=env["XDG_DATA_HOME"],
        QT_QPA_PLATFORM="offscreen",
        QT_QUICK_BACKEND="software",
        QT_FORCE_STDERR_LOGGING="1",
        QT_LOGGING_RULES="*.debug=false",
        QML_DISABLE_DISK_CACHE="1",
        BROWSER="/usr/bin/false",
        DBUS_SESSION_BUS_ADDRESS=f"unix:path={case_root / 'no-bus'}",
    )
    return env, Path(env["XDG_CONFIG_HOME"]), Path(env["XDG_DATA_HOME"])


def snapshot_tree(root: Path) -> dict[str, bytes | None]:
    tree: dict[str, bytes | None] = {}
    for entry in sorted(root.rglob("*")):
        rel = entry.relative_to(root).as_posix()
        if entry.is_dir() and not entry.is_symlink():
            tree[rel + "/"] = None
        else:
            tree[rel] = entry.read_bytes()
    return tree


def run_modlock(binary: str, args: list[str], env: dict[str, str], case_name: str) -> subprocess.CompletedProcess[str]:
    result = subprocess.run([binary, *args], env=env, capture_output=True, text=True, timeout=20)
    if result.returncode != 0:
        sys.exit(
            f"FAIL home-ui-check {case_name}: exit {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def check_fresh_demo_startup(binary: str, case_root: Path) -> None:
    env, config_home, data_home = isolated_env(case_root)
    result = run_modlock(binary, ["--demo", "--smoke-test"], env, "fresh-demo-startup")
    combined = result.stdout + "\n" + result.stderr
    assert "UI check finished; mods" in combined and "sourceError false" in combined, combined
    data_tree = snapshot_tree(data_home)
    assert data_tree == {}, f"fresh-demo-startup wrote XDG_DATA_HOME: {sorted(data_tree)}"
    config_tree = snapshot_tree(config_home)
    expected_paths = {"modlock/", "modlock/settings.conf"}
    assert set(config_tree) == expected_paths, f"unexpected config tree: {sorted(config_tree)}"
    settings_bytes = config_tree["modlock/settings.conf"]
    assert settings_bytes == CANONICAL_INI, f"unexpected default settings.conf: {settings_bytes!r}"
    parser = configparser.ConfigParser()
    parser.optionxform = str
    parser.read_string(settings_bytes.decode("utf-8"))
    assert parser.sections() == ["display"], parser.sections()
    assert dict(parser.items("display")) == CANONICAL_DEFAULTS, dict(parser.items("display"))
    print("PASS home-ui-check fresh-demo-startup", flush=True)


def check_home_fixture(binary: str, case_root: Path, case_name: str, with_stale_lock: bool) -> None:
    env, config_home, data_home = isolated_env(case_root)
    modlock_dir = config_home / "modlock"
    modlock_dir.mkdir(mode=0o700)
    settings = modlock_dir / "settings.conf"
    settings.write_bytes(CANONICAL_INI)
    lock_file = modlock_dir / "settings.conf.lock"
    if with_stale_lock:
        lock_file.write_bytes(STALE_LOCK)

    before_config = snapshot_tree(config_home)
    before_data = snapshot_tree(data_home)
    assert before_data == {}, before_data

    result = run_modlock(binary, ["--demo", "--check-ui", "home"], env, case_name)
    assert "PASS demo profile boundary: real picker, disabled edits, byte-identical XDG data/config, no Play recap" in result.stdout, result.stdout
    assert "PASS UI home: start screen, status action, Library and return navigation" in result.stdout, result.stdout

    after_config = snapshot_tree(config_home)
    after_data = snapshot_tree(data_home)
    assert after_data == before_data, f"{case_name} wrote XDG_DATA_HOME: {sorted(after_data)}"
    assert settings.read_bytes() == CANONICAL_INI, f"{case_name} altered settings.conf"
    if with_stale_lock:
        assert lock_file.read_bytes() == STALE_LOCK, f"{case_name} altered settings.conf.lock"
    assert after_config == before_config, f"{case_name} altered XDG_CONFIG_HOME: before={sorted(before_config)} after={sorted(after_config)}"
    print(f"PASS home-ui-check {case_name}", flush=True)


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(f"Usage: {sys.argv[0]} <modlock-binary>")
    binary = sys.argv[1]
    with tempfile.TemporaryDirectory(prefix="hm-") as scratch:
        scratch_path = Path(scratch)
        check_fresh_demo_startup(binary, scratch_path / "f")
        check_home_fixture(binary, scratch_path / "c", "canonical-home", with_stale_lock=False)
        check_home_fixture(binary, scratch_path / "s", "stale-lock-home", with_stale_lock=True)


if __name__ == "__main__":
    main()
