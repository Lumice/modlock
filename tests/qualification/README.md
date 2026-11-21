# Disposable helper qualification

Run from the checkout with an existing Release build containing sibling
`deadlock-helper` and `deadlock-network` executables:

```fish
python3 -I tests/qualification/qualify.py --helper /tmp/sol-qual/build/deadlock-helper --network
```

Omit `--network` for local qualification. `--submission 714763` selects the default
small GameBanana submission, Mina's Smaller Crosshair. Only one submission is
resolved; the smallest current file on its first page must declare at most 2 MiB.
No alternate submissions are tried. Reinstall necessarily downloads the same
submission again. There is no automatic retry of a failed download. Status polls
use 250 ms locally and one second for remote work; uncertain remote admission
retries retain the original identity.

This is a plain opt-in script, deliberately outside the default CTest suites.
Python's standard library is sufficient. ZIP uses `zipfile`; 7z requires `7z`,
`lsar`, and `unar`; RAR uses a generated uncompressed RAR4 containing a valid VPK
and requires `lsar` and `unar`. Missing optional archive tools produce SKIP rows.
All five formats were exercised on this machine.

Each run creates and retains a uniquely named `/tmp/deadlock-qualification-*`
directory, printed at startup. It contains `results.json`, `binary.json`,
`rpc.jsonl`, `helper.stderr`, pinned options details, launch evidence, and the
interrupted Apply journal. The binary and its network sibling are copied into
that directory to keep helper restarts consistent during concurrent rebuilds.
The fixture, fake Steam installation, upstream state, native database, HOME,
XDG data/config/cache, subprocess temporary files, and archive inputs live there.
By default the script never discovers or opens the real upstream or native library.
With `--real-library`, it follows the app’s Flatpak-then-XDG upstream discovery,
opens sources read-only, and copies state, profile manifests, their referenced
VPKs and gameinfo into a separate `/tmp/deadlock-real-copy-*` root. No whole-game
copy or hard links are used. The copied game locator points to the fake game.
Import, toggle, Apply and whole-tree Restore operate only on that copy; source
file hashes and timestamps are checked afterward. The copy and all private RPC
logs are deleted in a `finally` block. Only counts, reason codes and warning codes
are retained. An absent upstream state produces SKIP.
It never invokes `native.launch.commit`, Steam, or the game. Data is retained for
inspection; remove that printed temporary directory when finished.

Assertions verify actual behavior and exit nonzero on FAIL. BLOCKED means an
upstream prerequisite failed, not a passing test. The script continues independent
checks after failures. RAR uses its own fixture so a failed Restore cannot block
launch and interruption tests. The options test changes a two-choice folder
import to one selected option, then reverses the profile order.

Apply checks directory VPK bytes and the full CRLF gameinfo against independently
constructed expectations. Restore fingerprints every file and directory, including
permissions, in the fake game tree; persistent ownership markers are deliberately
counted. Local formats must deploy the exact input VPK bytes. The remote path
checks selected directory hashes against deployed VPKs after commit (unreached in
the recorded run). Launch checks idle status, zero initial admission, ready review,
no dispatch/observation/retained child, and unchanged game files. Interruption uses
a PATH-local `sha256sum` wrapper that pauses only when hashing a deployed VPK;
this is after the real helper journals its generation and staging file identities.
The harness reads that journal in read-only SQLite mode, kills the helper's private
process group with SIGKILL, restarts it, checks `recoveryRequired`, calls Recover,
and compares the entire game tree. The wrapper is then removed.

See [RESULTS.md](RESULTS.md) for the completed full run, open defects, test limits,
and CTest results. A failing qualification is useful evidence, not a release pass.

## Extended workflows

Run all workflows, including the explicitly requested read-only real-library
qualification, with:

```fish
python3 -I tests/qualification/qualify.py --helper /tmp/sol-qual2/build/deadlock-helper --network --real-library
```

Workflow A uses its own schema-33 import, initially Store2, with a disabled
legacy row and an unrelated row. A null replacement-target receipt establishes
link eligibility. It uses purpose `link` through replacement start, prepare,
review and commit, checks the Store4 transition and empty fonts envelope,
historical source fields, user metadata, row position, flags, import ledgers and
old blob retention. Enable/Apply compares actual deployed bytes to immutable
payload bytes. A subsequent fresh resolve performs Check and Reinstall on the
linked row and repeats Apply/Restore. Both normal remote install and link use
only Mod 714763/file 1809951; each reuses its initial resolution and needs a
fresh second resolution for Check. Four payload downloads total are expected.
These remain helper-protocol tests, not Qt button automation.

Workflow B never prints titles, profile IDs, paths, source JSON or private RPC
errors. Unavailable reasons are mapped to stable harness reason codes; unknown
reasons become `unavailablePayload`. Real-copy failures return a bounded code.
The retained qualification root contains only synthetic fixture data and the
aggregate real-copy result. Remove it after collecting the result if desired.
