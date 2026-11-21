# Archive boundary fixtures

These tests mutate only unique `archive-boundary-*` temporary directories.
They require installed Python 3, 7zz, lsar and unar. No decoder executable guard
is relaxed. The generated VPK-named payloads are opaque transport bytes;
package validity and family resolution belong to Task 2.

`fixtures.py` builds ZIP files using Python's standard `zipfile`, and solid /
encrypted 7z using installed `/usr/bin/7zz`. Corruption changes a stored ZIP
payload byte without repairing its CRC; length variants change the declared
uncompressed length in both local and central headers. Tests invoke real
installed CLI binaries and the production parser. Those CLI characterization
tests do **not** qualify the guarded worker process path.

Three `host_` tests are explicitly ignored by default. Run them in a normal
host UID namespace, where `/`, `/usr`, `/usr/bin`, lsar, unar and sha256sum are
root-owned and satisfy the production executable guard:

```text
cargo test --offline --locked --manifest-path backend/Cargo.toml --target-dir build/archive-content/helper-target --lib host_ -- --ignored --nocapture
```

They cover worker snapshot/member bytes, hashes, provenance and cleanup;
corrupt/truncated/wrong-size/encrypted input; process stdout/stderr limits and
hash input lengths; cancellation after observing the owned child followed by
reap verification. A guarded-environment failure is not a passing qualification.
The default owned-child destructor test independently verifies kill/reap with
a real sleeping child; it does not qualify the guarded runner loop.

## Genuine RAR provenance

`test_read_format_rar.rar.uu` is copied unchanged from libarchive/libarchive,
tag `v3.7.7`, path `libarchive/test/test_read_format_rar.rar.uu`:

- Source: https://github.com/libarchive/libarchive/blob/v3.7.7/libarchive/test/test_read_format_rar.rar.uu
- Git blob SHA-1: `f197a2983302f5829de2719bdddcf43b8c59b201`
- Text SHA-256: `d1e75b4120995bce82fc4e72ebf8b80d18ea69fa34ac62de383b8393f92afa09`
- Decoded 336-byte RAR SHA-256: `d421b86f6290aefad61b2a36737253b2b30fe27c156bd95abfc230f24fe0307e`
- Expected contents are independently documented by upstream `test_read_format_rar_basic`:
  https://github.com/libarchive/libarchive/blob/v3.7.7/libarchive/test/test_read_format_rar.c
- Upstream distribution copyright/license is retained in `LIBARCHIVE-COPYING`,
  from https://github.com/libarchive/libarchive/blob/v3.7.7/COPYING
  (blob `1b9723574a7be8d9c107239fc298723a32c0d5ae`).

The original fixture contains `test.txt`, a symlink, a second text file and two
directories. Tests verify exact indexed text bytes, checksum result and link
exclusion. It contains no VPK and does not qualify RAR5, solid RAR, multipart
RAR, or full worker VPK extraction. It is neither generated nor renamed ZIP.
