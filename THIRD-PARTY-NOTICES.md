# Third-party source

`backend/vendor/json` contains json-rust 0.12.4, copyright Maciej Hirsz,
under its MIT/Apache-2.0 dual license. Both license texts and the upstream
tests are included unchanged.

Source repository: https://github.com/maciejhirsz/json-rust
Pinned Git source tree: `0775592d339002ab148185264970c2a6e30b5d37`.
Each fetched file was verified against its Git blob SHA before writing the
original bytes. The directory contains the manifest, licenses, README, sources,
and tests, excluding the image and benchmark files.

This dependency is vendored because this build environment cannot resolve the
Cargo registry. Application crates use the local path dependency and build
offline. json-rust handles JSON DOM parsing and serialization; bounded private framing and validation scanners enforce catalog admission. Upstream compiler warnings
are retained; application builds can use Cargo's normal dependency lint cap.

Catalog normalization, query, synchronization, detail/file metadata, activity,
ordered collection reads and hero resolution in
`backend/src/catalog` adapt Deadlock Mod Manager implementation/data from
https://github.com/deadlock-mod-manager/deadlock-mod-manager at
`976637e2d63329ffef6f87eca40e2aea681acc94`. Adapted on 2026-10-28 for the native
worker, bounded SQLite FFI and typed identities, with stricter native limits.
Upstream contributors retain their copyrights. The upstream GPLv3 license is
retained unchanged as `backend/src/catalog/UPSTREAM-LICENSE` (upstream
`LICENSE.md`, Git blob `a5eae1527c5eadf27de0874ded503475baa6420f`). The generated
hero registry is retained with its original generation notice. Catalog source
headers identify the adapted modules. Program-level distribution/license review
remains part of release preparation; no publication is performed here.


## Optional bundled-font dependencies

`backend/vendor/ttf-parser` contains ttf-parser **0.24.1**, copyright Yevhenii
Reizner, under MIT OR Apache-2.0; both original license files are included.
Source: https://github.com/harfbuzz/ttf-parser/tree/v0.24.1
Pinned Git source tree: `dd0dcf1fd18d208414377e327c39b56f84c8778d`.
The connected GitHub read API supplied the 65 Rust library source files plus
Cargo.toml, README and both licenses. Original bytes were matched against
all 69 Git blob SHA-1 values before manifest adaptation. The exact listing,
paths, sizes and upstream Git digests are in `UPSTREAM-GIT-BLOBS.json`.

`backend/vendor/quick-xml` contains quick-xml **0.41.0**, copyright
Johann Tuffe and contributors, under MIT (`LICENSE-MIT.md`).
Source: https://crates.io/crates/quick-xml/0.41.0
Cached registry crate SHA-256:
`e660451e55124f798a69a5af3f49ccfbefbd41910eefd25caf2393e1f3473ec1`.

`backend/vendor/memchr` contains memchr **2.8.3**, copyright Andrew Gallant
and contributors, under Unlicense OR MIT (`UNLICENSE`, `LICENSE-MIT`, `COPYING`).
Source: https://crates.io/crates/memchr/2.8.3
Cached registry crate SHA-256:
`cf8baf1c55e62ffcace7a9f06f4bd9cd3f0c4beb022d3b367256b91b87513d98`.

Quick-xml and memchr library sources, licenses, readmes and original manifests
were copied from the existing Cargo registry source cache. Each
`UPSTREAM-SOURCE.json` records the source URL, cached crate digest and SHA-256
of each copied upstream file; `.cargo_vcs_info.json` preserves upstream VCS
provenance. These records describe source acquisition, not runtime validation.

For all three, `Cargo.toml.upstream` preserves the unmodified upstream manifest.
Only active manifests are adapted for the supported offline feature set:
ttf-parser uses std without default layout/variation/glyph-name features;
quick-xml has no default features and depends on local-path memchr; memchr
uses std. Unused registry optional/development dependencies are removed.
Names of disabled features remain declared for upstream cfg compatibility;
they are not supported build configurations. Library source and license bytes
are unchanged. Application manifests/lock and CMake watches use only local
paths. Upstream source includes its existing cfg(test) modules; no test was
added or executed in this unit. Quick-xml is vendored for the subsequent
bounded fonts.conf editor; this unit does not edit configuration or render fonts.


## Bounded metadata SHA-256 dependencies

The following complete production Rust sources are vendored from the immutable
Git commits below. No upstream test, example, benchmark or CI directories are
included. Existing source documentation and cfg(test) sections are unchanged.

| Crate | Version | Pinned commit | License |
| --- | --- | --- | --- |
| sha2 | 0.10.9 | 82c36a428f8d6f05f3bfccdedb243e9d1f85359d | MIT OR Apache-2.0 |
| digest | 0.10.7 | 344389411fd9718a0742435152e933a9e71461ee | MIT OR Apache-2.0 |
| crypto-common | 0.1.6 | 25614e2d5a4ccbb0cfde23367a93c8bcdbfe421a | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | 6d35952d3d3b124bc1049ad6fb406b42b1ce4bfe | MIT OR Apache-2.0 |
| generic-array | 0.14.9 | 02af3fea451ef810dd3bee78aa73ad46453b88a6 | MIT |
| typenum | 1.19.0 | 65d17389fad2919dab23c1e62cdf1226615c72bd | MIT OR Apache-2.0 |
| cfg-if | 1.0.4 | 3510ca6abea34cbbc702509a4e50ea9709925eda | MIT OR Apache-2.0 |
| version_check | 0.9.5 | d77ef9f27cc336719b2d839d09ee6635dd22f758 | MIT OR Apache-2.0 |

Repository URLs, exact upstream paths, immutable source URLs, Git blob SHA-1,
byte sizes and SHA-256 for every upstream file are recorded in
`backend/vendor/metadata-sha256/PINS.json` and `SOURCE-PROVENANCE.json`.
Each crate contains its complete original license text(s), README and original
`Cargo.toml.upstream`. Generic-array uses MIT; all other listed crates carry
both MIT and Apache-2.0 license texts. Original copyright notices are retained
verbatim in those license files; version_check's original manifest spells its
dual license `MIT/Apache-2.0`.

Only active Cargo manifests are specialized for the exact offline path graph
and supported software-only API. The application explicitly enables sha2's
`force-soft` feature with defaults disabled; no CPU detection, assembly,
registry dependency, system crypto library or runtime provider lookup is used.
Complete upstream architecture backend sources remain present but unselected.
`MANIFEST-ADAPTATIONS.md` describes supported features and manifest changes;
`ADAPTED-MANIFESTS.sha256` records hashes of the active manifests. The source
provenance index hashes `Cargo.toml.upstream`, not these adapted manifests.

Generic-array's unchanged `build.rs` and its path-only version_check build
dependency inspect the compiler version during normal compilation. Typenum's
production generated sources are checked in; its upstream test-only generator
is absent and explicitly disabled with `build = false`. No vendored Rust
implementation or license was rewritten. This adoption is source provenance
and build integration, not a cryptographic audit, vector qualification or
performance measurement.

## Pinned performance catalog and overlay grammar

[`backend/data/perf-catalog.json`](backend/data/perf-catalog.json) projects the
Deadlock Mod Manager catalog at
`7c7c90cc256f1599701f4a3e8fa5e911003f1c75` (catalog 2026.11.13-1).
`backend/src/performance/patch.rs` adapts
`92bebec63fd27145551a22c88d29f1d8e74d746b`'s performance patcher using this
project's conservative byte-span parser. The test `.gi` files are synthetic.

The catalog carries GPLv3 OptimizationLock (Sqooky, Eskay, Kaizuchaneru,
boot, Piggy and contributors) and OptiLock (dacooderr and contributors)
preset values and credits GPLv3 DeadTune for some descriptions. The
SchemaExplorer and Grimoire MIT notices accompany the respective
metadata and rule attributions. The catalog projection removes copied help
prose from Valve's dump and supplies original text where Modlock showed
that help in a preset. See [performance provenance and source
pins](backend/data/perf-PROVENANCE.md), the retained
[upstream GPLv3](backend/data/perf-LICENSE.md) and
[upstream notices](backend/data/perf-NOTICES.md).

`backend/vendor/regex` 1.13.1, `regex-automata` 0.4.18,
`regex-syntax` 0.8.11 and `aho-corasick` 1.1.5 are Rust regex dependencies,
under their included MIT/Apache-2.0 dual licenses. Their manifests use local
path dependencies so the helper continues to build offline.

Local json-rust fix: signed integer decoding uses an i128 intermediate to accept
negative i64 minimum journal timestamps and reject out-of-range positive values.

Autoexec managed-section marker grammar, crosshair defaults/ranges, URL share
format, KV3 hero-settings parser and documented renderer geometry are adapted
from deadlock-mod-manager/deadlock-mod-manager commit 92bebec (GPL-3.0), specifically
`autoexec_manager.rs`, `hero_settings.rs`, `crosshair_settings.rs` and
`packages/crosshair`. The native transaction and controller integration is new.

## Compatibility engine and hero tables

`backend/vendor/health-engine`, `source2-model`, `vpk-parser` and
`vpkmanager` are ported from Deadlock Mod Manager commit
`92bebec63fd27145551a22c88d29f1d8e74d746b` under GPLv3. The
[port notes](backend/vendor/health-engine/PORT.md) describe the changes.
The resource codecs retain their
[vpkmerge](backend/vendor/vpkmanager/LICENSE-vpkmerge) and
[ValveResourceFormat](backend/vendor/vpkmanager/LICENSE-ValveResourceFormat)
MIT notices. The wire-format implementation comes from ported source code.
The [embedded data record](backend/vendor/health-engine/assets/PROVENANCE.md)
identifies the origins, formats and regeneration inputs for the historical
lookups.

[hero-mapping.json](backend/data/hero-mapping.json) transcribes the 71
alias, key and display-name triples in Deadlock Mod Manager's
[hero parser](https://github.com/deadlock-mod-manager/deadlock-mod-manager/blob/92bebec63fd27145551a22c88d29f1d8e74d746b/packages/hero-parser/src-rs/mapping.rs)
at the same pin. The JSON is the editable local representation of these
identifiers. The distinct catalog alias registry has its generation notice
at the top of [heroes.rs](backend/src/catalog/heroes.rs).

## KDE palette

Modlock's AppImage palette fallback adapts KDE Frameworks color formulas.
Two Breeze colorschemes are used in source tests. The
[KDE source record](third_party/kde/ORIGIN.md) identifies the authors,
references and exact test fixture revisions. The
[LGPL-2.0-or-later license text](third_party/kde/LICENSE-LGPL-2.0-or-later.txt)
accompanies the adaptation and fixtures.

## Archivo Narrow

Copyright 2019 The Archivo Narrow Project Authors. The unmodified font is
under the [SIL Open Font License 1.1](style/fonts/OFL.txt). The AppImage
includes this license at `usr/share/doc/modlock/OFL-Archivo-Narrow.txt`.

## Sound preview decoders

Browse sound previews decode MP3 and Ogg Vorbis from memory with two vendored,
unmodified single-file decoders. Origin, revision, Git blob SHA and SHA-256 for
each are recorded beside the file; the license texts are kept unchanged at the
end of each file.

- `third_party/dr_mp3/dr_mp3.h`: dr_mp3 v0.7.4, https://github.com/mackron/dr_libs
  tag `mp3-0.7.4` (`aec2dd20667f54acfdb66031217df6d240f56c52`). Choice of public
  domain (Unlicense) or MIT No Attribution, copyright 2023 David Reid. Based on
  minimp3 (https://github.com/lieff/minimp3), dedicated to the public domain
  under CC0 1.0.
- `third_party/stb_vorbis/stb_vorbis.c`: stb_vorbis v1.24,
  https://github.com/nothings/stb at `9f755b604db3492df2cbeb39bf84b81e3e3932e3`.
  Choice of MIT License (copyright 2017 Sean Barrett) or public domain
  (Unlicense).

Audio output uses Qt Multimedia's `QAudioSink` only. Modlock does not use
`QMediaPlayer`, Qt's FFmpeg or GStreamer media plugins, or any libav* library.

## AppImage

The [AppImage release guide](docs/appimage-release.md) describes the
package and source records. The AppImage carries its third-party notices
under `usr/share/doc/modlock/`, including the `licenses/` directory.
