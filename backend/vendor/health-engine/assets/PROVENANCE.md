# Embedded compatibility data

Modlock uses these three files from the Deadlock Mod Manager
`apps/desktop/src-tauri/assets/` directory. Their contents match the Git
blobs in the [pinned upstream tree][dmm]. The GPLv3-covered upstream port is
described in [PORT.md](../PORT.md).

| Asset | Upstream Git blob | Contents and use |
| --- | --- | --- |
| [hero-model-bindings.json](hero-model-bindings.json) | `3d1d72d824a97bd264ac8d38bdb18663ed5dd71a` | Historic mappings between 43 hero identifiers and model paths. The repair planner uses this evidence to match old and new model locations. Modlock also uses the keys to recognize crosshair scopes. |
| [localization-history.bin](localization-history.bin) | `6751afde6c3a61ba1c27b8e3481ea030dab6a9b3` | 3,638 SHA-256 fingerprints over a path, token and value. The repair planner compares these to candidate localization text to recognize an older vanilla value. |
| [vdata-history.bin](vdata-history.bin) | `711a939ca86b26b17624e5b8359445c75a07ab7e` | `DMMVDH01` index of two resource paths, build numbers and SHA-256 row and field deltas for 120 builds (6407 through 6684). The repair planner uses the index as evidence when comparing old mod data with the installed game. |

## Editing and regeneration

- **Hero model bindings.** The checked-in JSON is the editable form.
  Its `sources` record pins Valve's November 2024 `heroes.vdata` snapshot
  and its SHA-256. The bindings retain hero keys and model paths from
  `m_strModelName`. The source file is mirrored by
  [GameTracking-Deadlock][heroes].
- **Localization history.** The [upstream generator][loc-source] supplies
  the `localization-history-index` command. See
  [localization-history.md](localization-history.md) for the corpus and
  format. The matching inputs are the 120 Unstoppable guard VPKs dated
  2026-03-24 through 2026-08-22 and the two English localization files
  named there. An archive-by-archive checksum list for those VPKs is
  unavailable in the pinned source.
- **VData history.** Its editable inputs are decoded KV3 snapshots of
  `scripts/abilities.vdata_c` and `scripts/heroes.vdata_c`, with the game
  build for each snapshot. The [consumer and encoder][vdata-source]
  describe the format and fingerprint rules. The available
  `encode_history_index` writes the newer `DMMVDH03` format. The pinned
  tree does not provide the exact generation command or raw input
  manifest for the vendored `DMMVDH01` file. The blob is reproducible as
  an exact upstream copy. Regenerating the legacy blob from original
  snapshots remains unverified.

The retained hero and resource names are compatibility identifiers. The
fingerprint files hold hashes and indexing metadata. They contain no raw
localization prose, decoded VData, model data or artwork. The upstream GPL
covers the ported implementation and upstream contributions. It does not
grant rights to redistribute Valve's input resources or third-party mod
archives. The Steam terms apply to access to Valve's game files. The
snapshot and archive input rights and complete historical input manifests
remain unverified. [Steam Subscriber Agreement][steam]

[dmm]: https://github.com/deadlock-mod-manager/deadlock-mod-manager/tree/92bebec63fd27145551a22c88d29f1d8e74d746b/apps/desktop/src-tauri/assets
[heroes]: https://github.com/SteamTracking/GameTracking-Deadlock/blob/5df28be343ce83699d7888b02d1559b5bc6b754c/game/citadel/pak01_dir/scripts/heroes.vdata
[loc-source]: https://github.com/deadlock-mod-manager/deadlock-mod-manager/blob/92bebec63fd27145551a22c88d29f1d8e74d746b/apps/desktop/src-tauri/examples/localization_overlay_poc.rs
[vdata-source]: https://github.com/deadlock-mod-manager/deadlock-mod-manager/blob/92bebec63fd27145551a22c88d29f1d8e74d746b/apps/desktop/src-tauri/src/mod_manager/vdata_history.rs
[steam]: https://store.steampowered.com/subscriber_agreement/#2
