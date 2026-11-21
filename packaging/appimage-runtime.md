# AppImage type-2 runtime

Public Modlock AppImages compile the x86-64 runtime from the AppImage/type2-runtime
source at commit `fedfad51acc2860b2916a1eef32ba4a500ff31ef`. The build uses the
following inputs, with upstream URLs and SHA-256 checksums in `SOURCES.json`.

| Component | Public build source | Notice |
|---|---|---|
| type2-runtime | `fedfad51acc2860b2916a1eef32ba4a500ff31ef` | MIT |
| libfuse | 3.15.0 with the runtime's mount patch | LGPL-2.1 for the library files |
| squashfuse | 0.5.2 with zlib and zstd enabled | BSD-2-Clause |
| musl | 1.2.5 with the patches from Alpine 1.2.5-r12 | MIT and the notices in musl's COPYRIGHT |
| zstd and its copy of xxHash | 1.5.6 | BSD-3-Clause |
| zlib | 1.3.2 with Alpine's CVE-2026-85091 patch | Zlib |
| mimalloc | 2.1.7 with the secure configuration | MIT |

The archived source includes Modlock's `tools/appimage-runtime.sh` and
`tools/appimage-runtime-build.sh`. They create a disposable Alpine 3.21.10 build
root with GCC 14.2.0. The scripts fetch the builder from Alpine and check its
SHA-256. They install tools from Alpine's signed repositories in the build root,
then disable network access and mount the shipped source inputs read-only
during compilation. The final link names each locally built object and archive
explicitly. It uses no host runtime libraries or compiler support library.
`runtime-build-tools.txt` records the tools and runtime SHA-256 and
`runtime-link.map` identifies the linked inputs.

To rebuild or relink, unpack the companion archive. Verify its `SHA256SUMS`
and unpack `Modlock-source.tar.gz`. With Bubblewrap, unprivileged user
namespaces and the host packaging tools available, run the following from the
companion directory. To relink with a modified library, make the changes to
a copy of the inputs or adjust the inner build script before running it.

```sh
Modlock/tools/appimage-runtime.sh runtime runtime-rebuild
```

The result is `runtime-rebuild/runtime-x86_64`. Pass it as the
`--runtime-file` argument to appimagetool when packaging the AppDir.
AppImage assembly fills the reserved SquashFS digest field in the runtime.
Modlock compares the rest of the packaged runtime with the just-built binary.

Local previews use the upstream continuous binary for the same runtime source
commit, SHA-256 `617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46`.
Its published debug file has the matching build ID
`ef655b05ac26feebfd3bc24ec7ccb30389cefc33`. The preview runtime came from a
different build environment. Its full Alpine package revision set is
unconfirmed.

References

- [Source tree and upstream build scripts](https://github.com/AppImage/type2-runtime/tree/fedfad51acc2860b2916a1eef32ba4a500ff31ef)
- [Binary release API](https://api.github.com/repos/AppImage/type2-runtime/releases/tags/continuous)
- [Alpine 3.21 package recipes](https://github.com/alpinelinux/aports/tree/3.21-stable)
