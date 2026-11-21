"""Locally generated ZIP/7z fixtures; no downloads or user input paths.

VPK contents are opaque on purpose: Task 1 transports bytes; Task 2 owns
package-family/VPK validation. Python zipfile and installed 7zz produce the
containers independently of the Rust decoder adapter.
"""
import pathlib
import binascii
import hashlib
import struct
import subprocess
import sys
import zipfile

root = pathlib.Path(sys.argv[1])
rar_lines = pathlib.Path(__file__).with_name("test_read_format_rar.rar.uu").read_bytes().splitlines()
rar_bytes = b"".join(binascii.a2b_uu(line) for line in rar_lines[1:-1])
assert hashlib.sha256(rar_bytes).hexdigest() == "d421b86f6290aefad61b2a36737253b2b30fe27c156bd95abfc230f24fe0307e"
(root / "genuine.rar").write_bytes(rar_bytes)
payload = b"archive-payload\n"
with zipfile.ZipFile(root / "valid.zip", "w", zipfile.ZIP_DEFLATED) as archive:
    archive.writestr("Folder/", b"")
    archive.writestr("Folder/skin_dir.vpk", payload)
    archive.writestr("Folder/skin_000.vpk", b"companion\n")
    archive.writestr("notes.txt", b"visible exclusion\n")
    archive.writestr("nested.zip", b"not expanded")
with zipfile.ZipFile(root / "stored.zip", "w", zipfile.ZIP_STORED) as archive:
    archive.writestr("skin.vpk", payload)
stored = bytearray((root / "stored.zip").read_bytes())
data_offset = 30 + struct.unpack_from("<H", stored, 26)[0]
corrupt = stored.copy()
corrupt[data_offset] ^= 0x20
(root / "corrupt.zip").write_bytes(corrupt)
central = stored.index(b"PK\x01\x02")
for name, delta in [("short-output.zip", 1), ("oversize-output.zip", -1)]:
    changed = stored.copy()
    struct.pack_into("<I", changed, 22, len(payload) + delta)
    struct.pack_into("<I", changed, central + 24, len(payload) + delta)
    (root / name).write_bytes(changed)
(root / "truncated.zip").write_bytes(stored[:data_offset + 3])
for name, members in [
    ("traversal.zip", [("../escape.vpk", payload)]),
    ("collision.zip", [("A.vpk", payload), ("a.vpk", payload)]),
]:
    with zipfile.ZipFile(root / name, "w", zipfile.ZIP_STORED) as archive:
        for path, data in members:
            archive.writestr(path, data)
with zipfile.ZipFile(root / "link.zip", "w") as archive:
    link = zipfile.ZipInfo("link.vpk")
    link.create_system = 3
    link.external_attr = 0o120777 << 16
    archive.writestr(link, "../outside")
source = root / "seven-source"
source.mkdir()
(source / "skin_dir.vpk").write_bytes(payload)
(source / "skin_000.vpk").write_bytes(b"companion\n")
for name, options in [("solid.7z", ["-ms=on"]), ("encrypted.7z", ["-pfixture-password", "-mhe=on"])]:
    subprocess.run(["/usr/bin/7zz", "a", "-t7z", "-mx=1", *options, str(root / name), "skin_dir.vpk", "skin_000.vpk"], cwd=source, check=True, stdout=subprocess.DEVNULL)
