# Version 0.2.4 | 2026-05-17

More deprecations! Deprecates the various `::parse()` / `::raw_parse()` methods
in favor of the plain `parse()` function and the central `Parser` API. These
return a `PartialVdf` that can easily be converted to a `Vdf` through `.into()`
or `.into_vdf()`.

## Deprecations

- Deprecates `::parse()` and `::parse_raw()` on `Vdf` and `PartialVdf` [(#134)]

## Feat

- Add a `::into_vdf()` helper  to convert a `PartialVdf` -> `Vdf` [(#134)]
- Add `::into_owned()` to get a `'static` backed
  `PartialVdf`/`Vdf`/`Obj`/`Value` [(#136)]

[(#134)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/134
[(#136)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/136

# Version 0.2.3 | 2026-01-05

This release (along with future 0.2.x releases) will be focused on polishing up
the API while prepping for a future 0.3.0 release. That means we'll be seeing a
lot more deprecations in the next few releases :)

## Deprecations

- Overall the whole `keyvalues_parser::text` module is being shifted around.
  Originally the plan was for this crate to contain both the text and binary VDF
  parsers each in their own respective module. It turns out the underlying
  representations are _significantly_ different to the point that a binary VDF
  parser should really live in a different crate entirely
  - Deprecated the empty `keyvalues_parser::text::render` module for removal
    [(#110)]
  - Move the errors from `keyvalues_parser::text::parse::*Error` to
    `keyvalues_parser::error`. The original location will be removed in the
    future [(#111)]
  - Deprecate the `keyvalues_parser::text::parse::*_parse()` functions for
    removal. Use the new `Parser` API instead [(#115)]

## Feat

- Re-export our `pest` dependency in the crate root [(#113)]
- Add a new high-level `Parser` parsing API [(#115)]

## Docs

- Enable all features when building on `docs.rs` and add labels for our feature
  usage [(#112)]
- Update links to point to our new home at codeberg [(#117)] [(#118)] [(#123)]

## Refactor

- Switch to more succinct `Debug` impls for `Value` and `Obj` [(#125)] [(#126)]

## Internal

- Simplify the test suite [(#124)]

[(#110)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/110
[(#111)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/111
[(#112)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/112
[(#113)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/113
[(#115)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/115
[(#117)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/117
[(#118)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/118
[(#123)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/123
[(#124)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/124
[(#125)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/125
[(#126)]: https://codeberg.org/CosmicHarper/vdf-rs/pulls/126

# Version 0.2.2 | 2025-12-08

## Feat

- Add limited `Deserialize` impls for `Obj` and `Value` [(#101)]
  - Limited because VDF doesn't act as a fully self-describing format, so
    things may fail to deserialize for seemingly benign reasons. Use at your own
    risk

## Docs

- Add a crates.io badge to the README [(#103)]

## Internal

- Cleanup generated parser code [(#99)] [(#107)]
- Prune some test/benchmark assets [(#105)]

[(#99)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/99
[(#101)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/101
[(#103)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/103
[(#105)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/105
[(#107)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/107

# Version 0.2.1 | 2025-09-22

The main headline of this update is significantly slimming down our dependency
tree. We now depend solely on `pest` (and its transitive deps) :tada:

## Fix

- Don't overflow the stack when parsing strings containing many escaped chars [(#94)]

## Deps

- Drop `thiserror` for a manual implementation [(#56)]
- Run `cargo update` and `cargo upgrade` [(#58)]
- Commit generated parser code instead of generating with `pest_derive` [(#95)] [(#96)]

## Docs

- Fix incorrect indentation in README example [(#43)]
- Update installation docs to use `cargo add` [(#58)]
- Spruce up more `Cargo.toml` package fields [(#70)]
- Copy licenses into crates [(#75)]

[(#43)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/43
[(#56)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/56
[(#58)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/58
[(#70)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/70
[(#75)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/75
[(#94)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/94
[(#95)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/95
[(#96)]: https://github.com/CosmicHorrorDev/vdf-rs/pull/96
