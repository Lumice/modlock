#!/usr/bin/env python3
"""Exercise the built GUI and helper using temporary synthetic stores only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import struct
import tempfile
import zlib


def fingerprint(path):
    return hashlib.sha256(path.read_bytes()).digest(), path.stat().st_mtime_ns


def raster(path, color):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    width, height = 160, 100
    raw = (b"\0" + bytes(color) * width) * height
    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--helper", required=True, type=Path)
    parser.add_argument("--source-dir", required=True, type=Path)
    args = parser.parse_args()
    args.source_dir = args.source_dir.resolve()
    binary, helper = args.binary.resolve(), args.helper.resolve()
    env = os.environ | {"QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
                        "QT_FORCE_STDERR_LOGGING": "1", "QT_LOGGING_RULES": "*.debug=false"}
    fixture = json.loads((args.source_dir / "tests/fixtures/upstream-state-v33.json").read_text())
    state = fixture["local-config"]["state"]
    state["localMods"][0].update(name='<b>Literal title</b><img src="http://127.0.0.1:9/metadata">',
                                author="<b>Literal author</b>", heroOverride="<b>Ivy</b>",
                                category="<b>Category</b>")
    state["localMods"][2]["status"] = "<b>raw-status</b>"
    state["profiles"]["default"]["name"] = "<b>Default</b>"
    secret = "SYNTHETIC_SECRET_DO_NOT_EXPOSE"

    with tempfile.TemporaryDirectory(prefix="deadlock-ui-") as temporary:
        directory = Path(temporary)
        env |= {"XDG_CONFIG_HOME": str(directory / "config"), "XDG_DATA_HOME": str(directory / "native-data")}
        source = directory / "source" / "state.json"
        source.parent.mkdir()
        source.write_text(json.dumps(fixture))
        before = fingerprint(source)
        # Record the actual helper PID, then replace the wrapper process with the helper.
        wrapper = directory / "helper-wrapper"
        pidfile = directory / "helper.pid"
        wrapper.write_text("#!/usr/bin/python3\nimport os, sys, time\nfrom pathlib import Path\n"
                           "Path(os.environ['UI_HELPER_PID']).write_text(str(os.getpid()))\n"
                           "time.sleep(0.3)\n"
                           "os.execv(os.environ['UI_REAL_HELPER'], [os.environ['UI_REAL_HELPER']] + sys.argv[1:])\n")
        wrapper.chmod(0o755)
        env |= {"UI_HELPER_PID": str(pidfile), "UI_REAL_HELPER": str(helper)}
        first, second = directory / "first.png", directory / "second.png"
        raster(first, (210, 45, 65))
        raster(second, (30, 115, 220))
        env |= {"UI_PREVIEW_FIRST": first.as_uri(), "UI_PREVIEW_SECOND": second.as_uri()}

        def run(scenario, *options, expected=0):
            completed = subprocess.run([str(binary), "--check-ui", scenario, *map(str, options)],
                                       env=env, capture_output=True, text=True, timeout=20)
            output = completed.stdout + completed.stderr
            assert secret not in output, "private fixture fields leaked"
            assert completed.returncode == expected, f"{scenario}: exit {completed.returncode}\n{output}"
            assert "PASS UI" in output, f"{scenario}: UI assertions did not run\n{output}"
            print(f"PASS integration {scenario}")

        run("snapshot", "--source", source, "--helper", wrapper)
        assert fingerprint(source) == before, "GUI changed source bytes or mtime"
        pid = int(pidfile.read_text())
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError("helper survived frontend exit")
        env["UI_CHOSEN_SOURCE"] = str(source)
        run("source-choice", "--source", directory / "missing.json", "--helper", helper)
        run("profiles", "--source", source, "--helper", helper)
        run("profile-option", "--source", source, "--helper", helper, "--profile", "other")
        assert fingerprint(source) == before, "profile browsing changed the source"

        # Change only our synthetic source after the UI has shown its first snapshot.
        process = subprocess.Popen([str(binary), "--check-ui", "refresh", "--source", str(source),
                                    "--helper", str(helper)], env=env, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True)
        try:
            assert process.stdout.readline().strip() == "READY_FOR_REFRESH"
            changed = json.loads(json.dumps(fixture))
            changed["local-config"]["state"]["localMods"] = []
            source.write_text(json.dumps(changed))
            modified = fingerprint(source)
            out, err = process.communicate(timeout=20)
            assert process.returncode == 0 and "PASS UI" in out + err, out + err
            assert secret not in out + err
            assert fingerprint(source) == modified, "refresh wrote source"
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()
        print("PASS integration refresh")
        run("empty", "--source", source, "--helper", helper)

        for name, content in [("invalidState", "{"), ("unsupportedVersion", '{"local-config":{"version":34,"state":{}}}')]:
            source.write_text(content)
            error_before = fingerprint(source)
            run("error:" + name, "--source", source, "--helper", helper)
            assert fingerprint(source) == error_before
        run("error:sourceNotFound", "--source", directory / "missing.json", "--helper", helper)
        # Bypass discovery so this still tests failure to start the RPC helper.
        run("error:helper", "--source", source, "--helper", directory / "missing-helper",
            "--data-dir", directory / "missing-helper-data")
        # Isolated discovery proves default mode never falls back to samples.
        isolated = env | {"HOME": str(directory / "home"), "XDG_DATA_HOME": str(directory / "data")}
        saved, env = env, isolated
        run("first-run-welcome", "--helper", helper)
        env = saved
        run("demo", "--demo", "--rows", 240)
        run("home-settings-summary", "--demo")
        run("demo-large", "--demo", "--rows", 10000)
        run("library-filters", "--demo")
        run("previews", "--demo")
        assert list(source.parent.iterdir()) == [source], "reading created source-side files"

        launcher = args.source_dir / "run-native.fish"
        # Reuse the build under test; never reconfigure the source tree's ./build.
        launch_env = env | {"MODLOCK_BUILD_DIR": str(args.binary.resolve().parent)}
        launched = subprocess.run(["fish", str(launcher), "--demo", "--smoke-test"], cwd=directory,
                                  env=launch_env, capture_output=True, text=True, timeout=120)
        assert launched.returncode == 0, launched.stdout + launched.stderr
        print("PASS integration cross-directory launcher")
    print("PASS integration all checks")


if __name__ == "__main__":
    main()
