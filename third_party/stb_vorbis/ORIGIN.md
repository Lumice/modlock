# stb_vorbis (vendored, unmodified)

- File: `stb_vorbis.c`, stb_vorbis v1.24
- Source: https://github.com/nothings/stb, commit `9f755b604db3492df2cbeb39bf84b81e3e3932e3` (2026-11-12, "update version numbers"; latest commit touching `stb_vorbis.c` on 2026-11-14)
- Fetched 2026-11-14 from `https://raw.githubusercontent.com/nothings/stb/9f755b604db3492df2cbeb39bf84b81e3e3932e3/stb_vorbis.c`
- Git blob SHA: `f5e80a0f27cbbe78405d01803bcc5e3edb98c2bd` (matches the GitHub contents API for that commit)
- SHA-256: `b52cf518355f7b2d181f18b97b812f361ffeb26299d935c8cf29e842401d60a6`
- License (from the end of the file): choice of MIT License (copyright 2017 Sean Barrett) or public domain (Unlicense). The notice is kept unchanged in the file.

Modlock compiles it once in `src/audiodecoder.cpp` with `STB_VORBIS_NO_STDIO`, `STB_VORBIS_NO_PUSHDATA_API` and `STB_VORBIS_MAX_CHANNELS 2`, opening only memory buffers with a fixed caller-provided allocation arena.
