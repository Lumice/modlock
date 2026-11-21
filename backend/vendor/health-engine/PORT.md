Ported from https://github.com/deadlock-mod-manager/deadlock-mod-manager commit
92bebec63fd27145551a22c88d29f1d8e74d746b.
GPL-3.0; LICENSE.md accompanies this source. Codec MIT notices/licenses remain
in the vpkmanager and source2-model sibling crates.

localization_overlay and vdata_history retain upstream analysis and embedded
fingerprint tables. Removed the Tauri event emitter, ts-rs annotations and the
LocalizationOverlayPlan::write method. Upstream tests are disabled with cfg(any())
because they depend on non-ported writers/test corpora; native synthetic tests
cover the public adapter. mod_health retains only types and finding/status rules.
The lib adapter exposes read-only standalone analysis with persisted observed
history. source2-model keeps only VPK extraction; vpk-parser keeps Rust parser,
types and errors. vpkmanager keeps resource/KV3/texture-header/material evidence
and in-memory rebuild helpers. Audio, image encoders, GLB, meshopt, native zstd,
VPK packing, FFI and frontend code are excluded from compilation.

See [embedded data provenance](assets/PROVENANCE.md) for the asset blobs and
their generation inputs. The disabled upstream compatibility test that uses
the reconstructed Baba skeleton track order now reads it from the local
`DMM_BABA_TRACK_ORDER` JSON path alongside its local game and mod inputs.
