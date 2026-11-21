# Silent audio fixtures

Self-made test inputs for `audio-preview-check`. Both files encode 0.25 s of
digital silence (11,025 frames of zero-valued 16-bit stereo PCM at 44,100 Hz).
They contain no recorded, sampled or provider content. They are dedicated to the
public domain under CC0 1.0.

Generated on 2026-11-14 on the development machine with installed CPU command-line
encoders (no audio device):

```sh
head -c 44100 /dev/zero > silence-44100-s16le-stereo-250ms.raw
ffmpeg -nostdin -hide_banner -loglevel error -f s16le -ar 44100 -ac 2 \
  -i silence-44100-s16le-stereo-250ms.raw -c:a libmp3lame -b:a 64k \
  -map_metadata -1 -fflags +bitexact -flags:a +bitexact -id3v2_version 0 \
  -write_xing 1 -y silence-250ms.mp3
oggenc -Q -r -B 16 -C 2 -R 44100 -q 0 --serial 1 \
  -o silence-250ms.ogg silence-44100-s16le-stereo-250ms.raw
```

Encoders: ffmpeg 9.0.2 CLI with libmp3lame (used only to make this file; Modlock
does not link or ship FFmpeg) and oggenc from vorbis-tools 1.4.3 (libVorbis
20200704). The raw input is not kept.

| File | Bytes | SHA-256 |
|---|---|---|
| `silence-250ms.mp3` | 2,506 | `640e157ae918c7ca4a4e1d08e7bbb30297de9aa94806791704815a7038917711` |
| `silence-250ms.ogg` | 3,985 | `252eed312607b5a5e6b43cfd7f46a801fdec968732baccb7617481aee9d08e38` |
