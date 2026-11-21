#!/usr/bin/env bash
# Build the release runtime without exposing host tools or libraries to the linker.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
inputs=$(realpath -- "$1")
output=$(realpath -m -- "$2")
base_version=3.21.10
base_sha=d225c786541b5ae5ba006ef541701d9ecc7e732bd16317c08adfb93212345efa
arch=x86_64
archive="alpine-minirootfs-$base_version-$arch.tar.gz"
base_url="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/$arch/$archive"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/modlock-runtime.XXXXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
mkdir -- "$output"
mkdir -- "$scratch/root"
curl --fail --location --retry 3 --proto '=https' --proto-redir '=https' \
    "$base_url" -o "$scratch/$archive"
printf '%s  %s\n' "$base_sha" "$scratch/$archive" | sha256sum --check --status
tar --no-same-owner -xzf "$scratch/$archive" -C "$scratch/root"
cat > "$scratch/root/etc/apk/repositories" <<'EOF'
https://dl-cdn.alpinelinux.org/alpine/v3.21/main
https://dl-cdn.alpinelinux.org/alpine/v3.21/community
EOF
common=(bwrap --unshare-all --uid 0 --gid 0 --die-with-parent --new-session
        --bind "$scratch/root" / --proc /proc --dev /dev --clearenv
        --setenv HOME /root --setenv PATH /usr/bin:/bin:/usr/sbin:/sbin)
# Build tools come from the Alpine-signed repository. The root under / is
# disposable. The build itself runs with no network and the inputs read-only.
"${common[@]}" --share-net --ro-bind /etc/resolv.conf /etc/resolv.conf \
    /sbin/apk add --no-cache gcc=14.2.0-r4 binutils=2.43.1-r3 make=4.4.1-r2 \
    patch=2.7.6-r10 linux-headers=6.6-r1 meson=1.6.1-r0 samurai=1.2-r6 \
    pkgconf=2.3.0-r0 autoconf=2.72-r0 automake=1.17-r0 libtool=2.4.7-r3
"${common[@]}" --ro-bind "$inputs" /inputs --bind "$output" /work \
    --ro-bind "$repo/tools/appimage-runtime-build.sh" /runtime-build.sh \
    /bin/sh /runtime-build.sh "${BUILD_JOBS:-2}"
runtime="$output/runtime-x86_64"
if readelf -lW "$runtime" | grep -q INTERP || readelf -dW "$runtime" | grep -q NEEDED; then
    echo 'Release runtime unexpectedly depends on a shared library.' >&2
    exit 1
fi
{
    printf 'Alpine build root %s\nSHA-256 %s\n' "$base_url" "$base_sha"
    printf 'Runtime '
    (cd "$output" && sha256sum runtime-x86_64)
} >> "$output/runtime-build-tools.txt"
