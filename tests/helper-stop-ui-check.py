#!/usr/bin/python3
"""Run the real helper-stop notice scenario with a private synthetic peer."""
import ctypes
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile


def deny_sockets():
    # Install before launching Qt; children inherit the no-network policy.
    # This fixture uses stdio IPC and requires no sockets, including Unix ones.
    if platform.machine() != "x86_64":
        raise RuntimeError("The fixture socket filter requires Linux x86_64")

    class Filter(ctypes.Structure):
        _fields_ = [("code", ctypes.c_ushort), ("jt", ctypes.c_ubyte),
                    ("jf", ctypes.c_ubyte), ("k", ctypes.c_uint)]

    class Program(ctypes.Structure):
        _fields_ = [("length", ctypes.c_ushort), ("filters", ctypes.POINTER(Filter))]

    filters = (Filter * 5)(
        Filter(0x20, 0, 0, 0),             # seccomp_data.nr
        Filter(0x54, 0, 0, 0x3fffffff),    # include the x32 syscall variant
        Filter(0x15, 0, 1, 41),           # x86_64 socket
        Filter(0x06, 0, 0, 0x00050001),   # EPERM
        Filter(0x06, 0, 0, 0x7fff0000),   # allow other syscalls
    )
    policy = Program(len(filters), filters)
    libc = ctypes.CDLL(None, use_errno=True)
    libc.prctl.argtypes = [ctypes.c_int] + [ctypes.c_ulong] * 4
    for args in ((38, 1, 0, 0, 0), (22, 2, ctypes.addressof(policy), 0, 0)):
        if libc.prctl(*args):
            raise OSError(ctypes.get_errno(), "Cannot block fixture sockets")


def main():
    binary = str(Path(sys.argv[1]).resolve())
    tests = Path(__file__).resolve().parent
    with tempfile.TemporaryDirectory(prefix="hsu-") as tmp:
        root = Path(tmp)
        env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
               "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
               "QML_DISABLE_DISK_CACHE": "1", "BROWSER": "/usr/bin/false",
               "DBUS_SESSION_BUS_ADDRESS": "unix:path=" + str(root / "no-bus"),
               "ITEM53_PEER_DIR": str(root), "PYTHONPATH": str(tests),
               "MODLOCK_UI_TEST_ROOT": str(root), "PYTHONDONTWRITEBYTECODE": "1"}
        keys = ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
                "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR",
                "XDG_CONFIG_DIRS", "XDG_DATA_DIRS")
        for index, key in enumerate(keys):
            folder = root / str(index)
            folder.mkdir(mode=0o700)
            env[key] = str(folder)
        peer = root / "peer"
        peer.symlink_to(tests / "helper-stop-peer.py")
        deny_sockets()
        result = subprocess.run(
            [binary, "--helper", str(peer), "--data-dir", str(root / "native"),
             "--check-ui", "helper-stop-notice"], env=env,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            stdin=subprocess.DEVNULL, timeout=35)
        print(result.stdout, end="", flush=True)
        if result.returncode:
            return result.returncode
        if "PASS UI helper-stop-notice:" not in result.stdout:
            raise RuntimeError("The behavioral UI scenario did not complete")
        return 0


if __name__ == "__main__":
    sys.exit(main())
