#!/usr/bin/env bash
# Build on the oldest distribution you intend to support. No root required.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
release=false
if [[ ${1:-} == --release ]]; then
    if [[ $# -lt 5 ]]; then
        echo 'Usage: tools/build-appimage.sh --release PUBLIC_GIT_URL TAG SRPM_CACHE NEW_BUILD_DIR [CMake options]' >&2
        exit 2
    fi
    release=true
    url=$2
    tag=$3
    srpm_cache=$(realpath -m -- "$4")
    shift 4
fi
build=$(realpath -m -- "${1:-$HOME/.cache/modlock-build}")
if [[ $# -gt 0 ]]; then
    shift
fi
cache=${XDG_CACHE_HOME:-$HOME/.cache}/modlock-appimage-tools
scratch=$(mktemp -d "${TMPDIR:-/tmp}/modlock-appimage.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
source_tree=$repo
commit=$(git -C "$repo" rev-parse HEAD)
jobs=${BUILD_JOBS:-2}
if $release; then
    if [[ -e "$build" || "$build" == "$repo/"* ]]; then
        echo 'Release requires a new build directory outside the checkout.' >&2
        exit 1
    fi
    python3 "$repo/tools/appimage_sources.py" identity "$repo" "$url" "$tag" > "$scratch/release.json"
    source_tree="$scratch/source"
    mkdir -p "$source_tree"
    commit=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["commit"])' "$scratch/release.json")
    git -C "$repo" archive "$commit" | tar -xf - -C "$source_tree"
    # Use fresh Cargo outputs and stable paths for panic messages and diagnostics.
    unset CARGO_TARGET_DIR RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET
    account_home=$(getent passwd "$(id -u)" | cut -d : -f 6)
    printf -v CARGO_ENCODED_RUSTFLAGS -- '--remap-path-prefix=%s=/toolchain\x1f--remap-path-prefix=%s=/home/build\x1f--remap-path-prefix=%s=/usr/src/modlock\x1f--remap-path-prefix=%s=/build/modlock' \
        "$account_home" "$HOME" "$source_tree" "$build"
    export CARGO_ENCODED_RUSTFLAGS
    name="Modlock-$tag-x86_64"
    destination="$build/release"
    mkdir -p "$build"
    release_flags=(--release-metadata "$scratch/release.json")
else
    version=$(git -C "$repo" rev-parse --short "$commit")
    name="Modlock-$version-x86_64"
    destination="$repo/build"
    release_flags=()
fi
mkdir -p "$cache"
fetch() {
    local name=$1 url=$2 digest=$3
    if [[ ! -f "$cache/$name" ]]; then
        curl --fail --location --retry 3 --proto '=https' --proto-redir '=https' "$url" -o "$scratch/$name"
        printf '%s  %s\n' "$digest" "$scratch/$name" | sha256sum --check --status
        mv -- "$scratch/$name" "$cache/$name"
    fi
    if ! printf '%s  %s\n' "$digest" "$cache/$name" | sha256sum --check --status; then
        echo "Cached packaging tool failed its SHA-256 check: $name" >&2
        exit 1
    fi
}
[[ $(uname -m) == x86_64 ]] || { echo 'Only x86_64 is supported.' >&2; exit 1; }
fetch appimagetool-1.9.1.AppImage https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
# type2-runtime commit fedfad51acc2860b2916a1eef32ba4a500ff31ef.
# Upstream's continuous URL is mutable. The digest fails closed if it changes.
runtime="$cache/runtime-x86_64"
if ! $release; then
    fetch runtime-x86_64 https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64 617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46
fi
install -m755 "$cache/appimagetool-1.9.1.AppImage" "$scratch/appimagetool.AppImage"
(cd "$scratch" && ./appimagetool.AppImage --appimage-extract > /dev/null)
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$repo" show -s --format=%ct "$commit")}
cmake -S "$source_tree" -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Release "$@" -DBUILD_TESTING=OFF
cmake --build "$build" --target modlock --parallel "$jobs"
appdir="$build/Modlock.AppDir"
python3 "$repo/tools/appimage-bundle.py" "$source_tree" "$build" "$appdir" "${release_flags[@]}"
doc="$appdir/usr/share/doc/modlock"
if $release; then
    # Keep large temporary release artifacts on the build filesystem.
    staging=$(mktemp -d "$build/publish.XXXXXXXX")
    trap 'rm -rf -- "$scratch" "$staging"' EXIT
    python3 "$repo/tools/appimage_sources.py" sources "$repo" "$doc" "$srpm_cache" \
        "$staging/sources" "$scratch/release.json" --build-runtime "$staging/runtime-build"
    runtime="$staging/runtime-build/runtime-x86_64"
    runtime_digest=$(sha256sum "$runtime" | cut -d ' ' -f 1)
    printf 'AppImage runtime SHA-256: %s\n' "$runtime_digest" >> "$doc/SOURCE.txt"
    source_digest=$(sha256sum "$staging/$name-sources.tar" | cut -d ' ' -f 1)
    printf 'Matching source archive SHA-256: %s\n' "$source_digest" >> "$doc/SOURCE.txt"
else
    staging=$scratch
fi
output="$staging/$name.AppImage"
env -u APPDIR -u APPIMAGE ARCH=x86_64 "$scratch/squashfs-root/AppRun" --no-appstream --comp=zstd \
    --mksquashfs-opt=-no-xattrs \
    --mksquashfs-opt=-b --mksquashfs-opt=1048576 \
    --mksquashfs-opt=-Xcompression-level --mksquashfs-opt=22 \
    --mksquashfs-opt=-processors --mksquashfs-opt="$jobs" \
    --runtime-file="$runtime" -- "$appdir" "$output"
if $release; then
    runtime_size=$(stat --printf='%s' "$runtime")
    unsquashfs -quiet -no-progress -no-xattrs -processors "$jobs" -offset "$runtime_size" -d "$staging/extracted" "$output"
    python3 "$repo/tools/appimage_sources.py" verify \
        "$staging/extracted" "$output" "$runtime" "$repo" "$source_tree" "$build" --runtime-sha256 "$runtime_digest"
    python3 "$repo/tests/appimage-package-check.py" "$staging/extracted" "$staging/$name-sources.tar"
fi
(cd "$staging" && sha256sum -- "$name.AppImage" > "$name.AppImage.sha256")
mkdir -p "$destination"
if $release; then
    (cd "$staging" && sha256sum -- "$name-sources.tar" > "$name-sources.tar.sha256")
    mv -- "$staging/$name-sources.tar" "$staging/$name-sources.tar.sha256" "$destination/"
fi
mv -- "$output" "$staging/$name.AppImage.sha256" "$destination/"
stat --printf='%n: %s bytes\n' "$destination/$name.AppImage"
printf 'Bundled libraries: %s\n' "$doc/BUNDLED-LIBRARIES.txt"
