# Build and release the AppImage

Build on a disposable x86-64 openSUSE Tumbleweed builder. Use a snapshot whose
binary packages and sources you can still retrieve. Install the build dependencies
in [CONTRIBUTING.md](../CONTRIBUTING.md), plus Git, Python 3.11+, curl, RPM,
rpm2cpio, cpio, binutils, tar, gzip, bzip2, xz, zstd and squashfs. Public builds
also need Bubblewrap and unprivileged user namespaces for the runtime build. The Qt runtime
needs the imports and platform plugins listed in the build prerequisites.
Keep the signing keys from the builder's trusted openSUSE repositories available
to `rpm`. Leave space for the build, the source cache and the companion archives.

## Local preview

```sh
tools/build-appimage.sh /path/to/build
```

The script stages `Modlock.AppDir` in the build directory, then writes the
AppImage and its checksum to the checkout's ignored `build/` directory.
You can pass additional CMake options after the build directory.
`BUILD_JOBS` sets build and compression parallelism and defaults to two.
The preview omits the checkout's remote and the list of modified files.
Its dependency inventory can help identify inputs for a release.

## Public release

Clone the public repository into a clean checkout and switch to the public
release tag. Pass the public HTTPS Git clone URL, that tag, a scratch directory
for source RPMs, and a new build directory outside the checkout.

```sh
tools/build-appimage.sh --release "$public_git_url" "$release_tag" "$srpm_cache" "$build_dir"
```

The script verifies anonymously that the public tag resolves to the checked-out
commit. It builds the tracked contents of that commit with tests disabled and
remaps build paths. It checks each copied distribution file against the installed
RPM record before stripping ELF files or editing QML module declarations. It
checks the assembled runtime and extracts the finished image to check the
notices, dependency inventory and paths. It builds the runtime and every linked library from the pinned
source inputs and records the link map. Keep the source tree, builder and
cache outside personal application data.

The script writes these files to `release/` under the build directory.

- `Modlock-<tag>-x86_64.AppImage` and its SHA-256 file
- `Modlock-<tag>-x86_64-sources.tar` and its SHA-256 file

The source tar includes the full Modlock source from the verified public tag,
including vendored Cargo dependencies, decoders and build scripts. It contains
a source package or complete expanded OBS source revision for each packaged
RPM. These contain the distro spec, source inputs and patches. It also contains
the [pinned AppImage runtime and linked library sources](../packaging/appimage-runtime.md).
`RPM-SOURCES.tsv` maps each source package identity to its actual archive
or OBS source directory, and `SHA256SUMS` covers all companion files.

Source RPMs in the cache must match the exact source name, version, release
and OBS build revision of the installed binaries and have a trusted RPM
signature. The script downloads missing source RPMs from the official openSUSE
source repository. If a package has rolled off the mirror, it retrieves the
exact expanded revision from the public OBS API and verifies its directory
digest and each input against the installed RPM's recorded build source.
You can prefill the cache with signed matching source RPMs from an official
archive. Build from a different pinned builder if a required source cannot
be retrieved. Release assembly stops on missing or mismatched source.

The AppImage's `usr/share/doc/modlock/` includes the license texts and
separate lists of staged files, installed RPMs, Cargo dependencies and matching
source locations. The repackaged Qt libraries are dynamically linked. To use a
compatible modified build of a bundled library, extract the AppImage in an
empty directory with `--appimage-extract`, replace that library in
`squashfs-root/usr/lib` and run `squashfs-root/AppRun`. The corresponding
runtime source directory contains the recipe for relinking the static runtime.

Before publishing, verify the image on the oldest distro listed in the release
and check that it starts using FUSE or AppImage extraction. Review the package
notices and test the companion archive's checksums. Attach the image, the
matching source archive and their checksum files to the same published release.
Keep the source download freely accessible alongside the binary. The RPM
inventory and the public Modlock tag alone don't supply the distro and runtime
sources.
