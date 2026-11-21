# Hero skins

Browse → Hero skins opens a searchable grid of heroes. Tiles use popular cached skin thumbnails with initial badges as a fallback;
no game artwork is bundled. Selecting a tile
queries the existing catalog with kind `mod`, categories `["Skins"]`, heroes
`[display name]`, empty search and excludeFilters false. All heroes returns to
the grid. Existing sensitive-content and other user filters remain in effect.
Catalog rows and grid cards share CatalogRow; the hero precedes author/category.

Data: upstream Deadlock Mod Manager checkout
92bebec63fd27145551a22c88d29f1d8e74d746b,
packages/hero-parser/src/mapping.ts and src-rs/mapping.rs (71 internal aliases,
45 distinct display names). Ported into backend/data/hero-mapping.json;
qml/HeroData.js is its sorted distinct display-name projection. The UI tests check that all mapped display names occur in the picker. Adapted parser and
data remain GPL-3.0 under UPSTREAM-LICENSE. No new crate or network requirement.
The upstream HeroIcon component fetches images through useHeroCatalog; that
component provides no separate redistributable game-art license. Those images
were not copied. Mapping includes the upstream WIP/unreleased names as requested.

## Read-only RPC and required library hook

The path form of `native.hero.detect` accepts exactly `{ "vpkPath": "/absolute/package_dir.vpk" }`.
No dataDir or revision is needed: it neither opens the library nor writes files.
It uses the existing no-symlink regular-file VPK reader, capped at a 16 MiB
directory tree and 100,000 entries. It never extracts or reads resource payloads.
Malformed and unsupported directories fail with the existing unsupportedVpk
error. Directory packages may reference external archive members; inference
needs only their directory paths.

Result fields: hero (enum key or null), heroDisplay (catalog display name or
null), category (`hero`/`other`), internalNames, usesCriticalPaths, criticalPaths.
Model paths under models/heroes/, models/heroes_staging/ and models/heroes_wip/
contribute one vote per entry. Aliases merge by enum key; a unique maximum wins;
a tie or no mapped paths returns null. Critical script paths follow upstream.

Integrator hook: after loading/importing a local mod, resolve its admitted VPK
**directory** path from backend-owned payload/package metadata and call this RPC.
Bind the result to that library root + mod id + revision; discard stale results.
Use heroDisplay for the row/details hero, preserving any explicit user override
and authoritative catalog hero. Do not use the enum key as the catalog filter:
GreyTalon → Grey Talon, MoKrill → Mo & Krill, etc. Surface null as unknown.
Cache by payload identity, not by display filename. This change deliberately
leaves src/librarymodel.*, src/libraryrows.*, library documents and ModDetails
for the parallel implementer/integrator.

For mods with multiple VPK directories, call the module's detect_paths once on
the combined admitted entries in backend code; choosing the first per-file
winner can misclassify ties. The public path RPC handles one VPK at a time;
the mod-id form below implements multi-directory aggregation. Persisted/manual
hero assignment remains separate. Detection is inference from model paths, not a guarantee that
the entire package is a skin. Sound-only and material-only mods may remain unset.

## Checking hero detection

Run the Rust hero tests from `backend/` with `cargo test --offline --lib hero::tests`.
From the repository root, run `ctest --test-dir build -R '^hero-skins-ui-check$' --output-on-failure`
to check the picker, search and return navigation.

## Library and tile integration

`native.hero.detect` also accepts `{version:1,dataDir,profileId,modId,expectedRevision}`.
This read-only form resolves the selected directory packages from backend metadata,
combines their entry paths before voting, and returns modId, revision, fingerprint
and heroDisplay. It rejects stale revisions. Limits are 32 directories and 100,000
combined entries per mod; the existing 16 MiB per-directory tree cap still applies.
A process-lifetime cache holds at most 512 root + payload id/hash/size fingerprints,
including unknown results. Failed reads are retried on a later snapshot.

LibraryModel requests one mod at a time after a snapshot; heroDetectionEligible
protects explicit source heroes and overrides. Replies from old snapshot generations
are discarded. Rows/details, search and the category menu include detected heroes.
No inferred value is written to library records. Catalog facets supply each hero's
most-downloaded cached Skins thumbnail with a stable slug tie-break, shown through
ModPreview's existing cache/request/masking pipeline. Missing or failed artwork
retains initials; masked artwork stays concealed.
