# Performance catalog provenance

Modlock projects `perf-catalog.json` from Deadlock Mod Manager catalog version
`2026.11.13-1` at commit `7c7c90cc256f1599701f4a3e8fa5e911003f1c75`.
The upstream file has Git blob `66cf707f061fc09e7a4a6df64cde5fab6a0cb4dc`
and SHA-256 `f7a36be0a023e71d2830ad3a85253ec4d721c3fd6ea80ba1d63120cfbc1e8280`.
[Pinned catalog][upstream-catalog]

The upstream sources for editing and regeneration are the
[generator, curated definitions and input pins][upstream-package].
Its [input manifest][input-pins] lists the revision and checksum of each
SchemaExplorer dump, GameTracking snapshot and GPL preset file. Use a local
copy of the pinned upstream catalog and project it from the repository root:

```sh
python3 backend/data/project-perf-catalog.py /path/to/upstream/catalog.json > /path/to/modlock-catalog.json
```

After the checksum and projection succeed, replace the checked-in file with
the output. The projection removes provider reference
rows unused by Modlock and Valve's copied help prose and extracted enum
labels. [perf-descriptions.json](perf-descriptions.json) contains the original
descriptions Modlock uses for 69 preset entries that depended on the help
fallback and one curated description that repeated the dumped wording.
The eight presets, overlay values, attribution, guard rules, stock matching
inputs and other curated descriptions are preserved.

## Component terms

| Component | Origin and notice |
| --- | --- |
| Catalog, curated definitions and generator | Original Deadlock Mod Manager contributions are GPLv3. The pinned upstream [license](perf-LICENSE.md) and [notices](perf-NOTICES.md) are retained. Relative upstream paths in the notices refer to the pinned upstream tree. |
| Preset contributions | The pinned OptimizationLock and OptiLock preset sources use GPLv3. The upstream notices also credit GPLv3 DeadTune descriptions. [OptimizationLock license][optimizationlock] · [OptiLock license][optilock] · [DeadTune license reference][deadtune] |
| Console variable metadata | SchemaExplorer `schemas/deadlock.json` at `15018faf41a82b12179fc652f462bc3a462065e9`, SHA-256 `ed671173e9be4410dd256182a5e3f868d5326d7f0a283192eaf052261c7c3aeb`. The repository's MIT notice is retained verbatim, Git blob `215860b4a4852e90a4516e76195861deeb44a943`. The dump describes Valve's game. [Pinned schema source][schema] · [SchemaExplorer MIT notice](perf-LICENSE-SchemaExplorer.txt) |
| Stock history | The 42 parsed stock build records carry Valve's functional paths, version identifiers and scalar values used to compare a local `gameinfo.gi`. The catalog keeps 419 distinct path/value pairs across these builds and omits the raw configurations and their explanatory comments. The [input manifest][input-pins] pins the source snapshots. |
| Gameplay rules | Upstream credits Grimoire (MIT, esoc). The exact revision consulted by the catalog authors is undocumented. The included notice matches the license at revision `c9ed4546084878384b7424f82236f7a583bb8e55`, Git blob `848d9ecf1a2e2855f6fe68b4a1a688ba0ae46837`. [Grimoire license reference][grimoire] · [Grimoire MIT notice](perf-LICENSE-grimoire.txt) |

The retained Valve-derived fields serve as technical references for
compatibility. Valve's game files and SchemaExplorer's software license have
separate provenance. This record has no independent permission from Valve
to redistribute its input files or authored descriptions. Upstream's
[package documentation][upstream-readme] also identifies Valve as the source
of the dumped data.

[upstream-catalog]: https://github.com/deadlock-mod-manager/deadlock-mod-manager/blob/7c7c90cc256f1599701f4a3e8fa5e911003f1c75/packages/perf-catalog/data/catalog.json
[upstream-package]: https://github.com/deadlock-mod-manager/deadlock-mod-manager/tree/7c7c90cc256f1599701f4a3e8fa5e911003f1c75/packages/perf-catalog
[upstream-readme]: https://github.com/deadlock-mod-manager/deadlock-mod-manager/blob/7c7c90cc256f1599701f4a3e8fa5e911003f1c75/packages/perf-catalog/README.md#sources-and-licences
[input-pins]: https://github.com/deadlock-mod-manager/deadlock-mod-manager/blob/7c7c90cc256f1599701f4a3e8fa5e911003f1c75/packages/perf-catalog/sources.json
[schema]: https://github.com/ValveResourceFormat/SchemaExplorer/blob/15018faf41a82b12179fc652f462bc3a462065e9/schemas/deadlock.json
[optimizationlock]: https://github.com/Sqooky/OptimizationLock/blob/334b09bc3f2c38177bb726fba4d179bd17cd13ec/LICENSE
[optilock]: https://github.com/dacooderr/OptiLock/blob/77489664c3e2367c486a81ffd62b0f3f28d3454d/LICENSE.md
[deadtune]: https://github.com/simulieren/deadtune/blob/0cc18ce078a6dea06e95d9bcc5b03d85fb8e4a7f/LICENSE
[grimoire]: https://github.com/Slush97/grimoire/blob/c9ed4546084878384b7424f82236f7a583bb8e55/LICENSE
