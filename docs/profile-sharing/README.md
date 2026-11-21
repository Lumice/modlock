# Share a profile

Open the profile menu to export a code. Send the code to another player. The recipient can review the mods, download missing files and create a profile. The code contains the load order, enabled state, selected options and GameBanana file references. Local-only mod names are optional; the code cannot distribute their files.

Modlock creates the code locally. If a linked mod has no download receipt, Modlock can send its package fingerprint to `api.deadlockmods.app` to match the exact file. If it cannot confirm a match, the review identifies the entry as the latest available version.

## Interchange format

Profile sharing uses `DLMM1-` followed by unpadded base64url of zlib-compressed canonical JSON. Import also accepts the original JSON document, pasted or selected with Choose file. Both input and expanded JSON are limited to 16 KiB; trailing compressed data, invalid base64url, duplicate keys, unknown fields, foreign providers, duplicate references, and non-integer schema numbers are rejected. There are at most 128 remote mods, 128 omitted names, and 99 selected options per mod.

The document has `format: "dlmm-profile"`, `version: 1`, `mods`, and `notShareable`. Array order defines load order. Entries contain a GameBanana submission, `fileId` (a decimal string or `"latest"`), selected option labels, and `enabled`. No URL, local ID, profile name, author, filesystem path, or device identifier is exported. Local-only names require the checkbox.

Downloaded mods use their existing file receipt. Association-only links (`nativeRemoteLink` version 2 with null receipt) use the managed package SHA-256 already recorded by import, exactly as the installed-mod identification path does. Export posts only the hash and empty content signature to `api.deadlockmods.app/api/v2/vpk-analyse-hashes`. It accepts certainty 100 / matchType sha256, the same SHA-256 and submission, and one unambiguous file ID present in that submission's normalized current/archived file list. All selected packages must identify the same file. Multipart packages conservatively fall back to latest, because a directory hash alone cannot prove their companions.

Exact resolutions are stored separately in the native catalog's state table under a SHA-256 of submission plus selected package family identities. This grants no download receipt or update/replacement authority. Already resolved exports and reviews work offline; changed payload families get a different key. Only export attempts network identification. It uses a bounded curl child (HTTPS, no redirects, 5-second request timeout, 1 MiB response limit) and a 15-second attempt budget (an in-flight request may add up to 5 seconds). Failed or ambiguous matches export `"latest"`, with an explicit note in both dialogs. They are retried on a later export. Compression uses the platform zlib library; no Cargo dependency was added.

Export lists mod titles and a total/local-only summary. Import uses catalog titles and thumbnail URLs where available; otherwise it shows `GameBanana mod <id>`. NSFW catalog thumbnails remain hidden. Variant labels are displayed without VPK extensions and filename separators. Review states are In your library, Will be downloaded (review in Downloads), and Not available. Latest-file entries open Downloads without pinning an old file ID. Downloaded records can satisfy a latest entry once its requested choices are available. Exact-file entries still require the exact requested file and choices. Association imports encode variant choices as `payload-<SHA256 of package family>` so renamed, byte-identical VPKs match after downloading on another computer. The UI calls these Original variant; it never displays the digest. Other choice labels retain their original exact matching. A missing choice is never silently substituted. If the latest download has changed the original variant bytes, review keeps it unavailable rather than inventing an equivalent choice.

Review never starts a download. Finish reviewed downloads into the current profile and review again. Profile creation requires every remote entry's choices to be available. Local-only names are informational. Creation preserves enabled state and load order and uses the root lock, revision comparison, game-idle checks and atomic Store operation receipt. Nothing applies to the game automatically.

The separate ProfileShareController uses an application-owned helper process so closing the dialog cannot terminate an admitted metadata write. Results are rejected after context changes. No LibraryModel changes are required. The existing Downloads workflow handles download review.

## Tests

From the repository root, run `ctest --test-dir build -R '^profile-share-check$' --output-on-failure`. For the Rust codec and matching tests, run `cargo test --offline --lib profile_share` from `backend/`. The fixtures use synthetic libraries and cover association resolution and offline reuse.
