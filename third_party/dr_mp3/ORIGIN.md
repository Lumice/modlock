# dr_mp3 (vendored, unmodified)

- File: `dr_mp3.h`, dr_mp3 v0.7.4 (2026-10-31)
- Source: https://github.com/mackron/dr_libs, tag `mp3-0.7.4`, commit `aec2dd20667f54acfdb66031217df6d240f56c52`
- Fetched 2026-11-14 from `https://raw.githubusercontent.com/mackron/dr_libs/aec2dd20667f54acfdb66031217df6d240f56c52/dr_mp3.h`
- Git blob SHA: `46ee903b62dd6e3eaf7e5039c2cf616d07c19c37` (matches the GitHub contents API for that commit)
- SHA-256: `f5c8020eeebb79877184432efdd2651f24869f5bb8f379d45450fd0aed1ee915`
- License (from the end of the file): choice of public domain (Unlicense) or MIT No Attribution, copyright 2023 David Reid. It is based on minimp3 (https://github.com/lieff/minimp3), which is dedicated to the public domain under CC0 1.0. Both notices are kept unchanged in the file.

Modlock compiles it once in `src/audiodecoder.cpp` with `DR_MP3_NO_STDIO`, decoding only from memory through counted allocation callbacks.
