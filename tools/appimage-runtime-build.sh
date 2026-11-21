#!/bin/sh
# Runs inside the isolated Alpine build root with source inputs mounted at /inputs.
set -eu
export LC_ALL=C
export SOURCE_DATE_EPOCH=1792442122
jobs=$1
prefix=/work/local
musl=/work/musl
gcc_headers=$(gcc -print-file-name=include)

cd /work
tar -xf /inputs/musl/musl-1.2.5.tar.gz
(
    cd musl-1.2.5
    # Apply the package's checked-in patches in APKBUILD order.
    awk '/^sha512sums="/ { in_sums=1; next }
         in_sums && $2 ~ /\.patch$/ { print $2 }' /inputs/musl/APKBUILD |
    while read -r source_patch; do
        patch -p1 < "/inputs/musl/$source_patch"
    done
    ./configure --prefix="$musl" --disable-shared --enable-gcc-wrapper
    make -j"$jobs"
    make install
    # Alpine GCC needs this archive for configure probes in the build root.
    gcc -c -fno-stack-protector /inputs/musl/__stack_chk_fail_local.c -o /work/stack-check.o
    ar rcs "$musl/lib/libssp_nonshared.a" /work/stack-check.o
)

export CC="$musl/bin/musl-gcc"
export CFLAGS="-Os -fPIC -ffunction-sections -fdata-sections -idirafter /usr/include"
export LDFLAGS="-static"
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig"

tar -xf /inputs/zlib/zlib-1.3.2.tar.gz
(
    cd zlib-1.3.2
    patch -p1 < /inputs/zlib/CVE-2026-85091.patch
    ./configure --static --prefix="$prefix"
    make -j"$jobs"
    make install
)

tar -xf /inputs/zstd/zstd-1.5.6.tar.gz
(
    cd zstd-1.5.6
    make -C lib -j"$jobs" libzstd.a
    mkdir -p "$prefix/lib" "$prefix/include"
    cp lib/libzstd.a "$prefix/lib/"
    cp lib/zstd.h lib/zstd_errors.h lib/zdict.h "$prefix/include/"
)

tar -xf /inputs/type2-runtime/type2-runtime-*.tar.gz
runtime_source=$(printf '%s\n' /work/type2-runtime-*)
tar -xf /inputs/libfuse/fuse-3.15.0.tar.xz
(
    cd fuse-3.15.0
    patch -p1 < "$runtime_source/patches/libfuse/mount.c.diff"
    meson setup out --prefix="$prefix" --libdir=lib --default-library=static \
        -Ddisable-libc-symbol-version=true -Dutils=false -Dexamples=false -Dtests=false -Duseroot=false
    ninja -j"$jobs" -C out
    ninja -j"$jobs" -C out install
)

tar -xf /inputs/squashfuse/squashfuse-0.5.2.tar.gz
(
    cd squashfuse-0.5.2
    ./autogen.sh
    ./configure --prefix="$prefix" --disable-shared --without-xz --without-lzo --without-lz4 \
        --with-zlib="$prefix" --with-zstd="$prefix" --disable-demo
    make -j"$jobs"
    make install
    mkdir -p "$prefix/include/squashfuse"
    cp ./*.h "$prefix/include/squashfuse/"
)

tar -xf /inputs/mimalloc/mimalloc-2.1.7.tar.gz
(
    cd mimalloc-2.1.7
    # Upstream's static amalgamation provides the allocator override.
    "$CC" $CFLAGS -DMI_SECURE=4 -DMI_STATIC_LIB -DMI_LIBC_MUSL -Iinclude -c src/static.c -o "$prefix/mimalloc.o"
)

cd "$runtime_source/src/runtime"
version=$(basename "$runtime_source" | cut -c 15-21)
# Link only inputs built above. Neither the builder's libc nor libgcc can
# enter the binary because we name every input and disable default libraries.
gcc -c -std=gnu99 -Os -D_FILE_OFFSET_BITS=64 \
    "-DGIT_COMMIT=\"https://github.com/AppImage/type2-runtime/commit/$version\"" \
    -nostdinc -isystem "$musl/include" -isystem "$gcc_headers" -idirafter /usr/include \
    -I"$prefix/include" -I"$prefix/include/squashfuse" -I"$prefix/include/fuse3" \
    -ffunction-sections -fdata-sections -fPIE -Wall runtime.c -o /work/runtime.o
gcc -nostdlib -static-pie -T data_sections.ld \
    -Wl,--build-id=sha1,--gc-sections,-Map,/work/runtime-link.map \
    "$musl/lib/rcrt1.o" "$musl/lib/crti.o" /work/runtime.o "$prefix/mimalloc.o" \
    -Wl,--start-group "$prefix/lib/libsquashfuse.a" "$prefix/lib/libsquashfuse_ll.a" \
    "$prefix/lib/libzstd.a" "$prefix/lib/libz.a" "$prefix/lib/libfuse3.a" "$musl/lib/libc.a" \
    -Wl,--end-group "$musl/lib/crtn.o" -o /work/runtime-x86_64
strip --strip-debug --strip-unneeded /work/runtime-x86_64
printf 'AI\002' | dd of=/work/runtime-x86_64 bs=1 count=3 seek=8 conv=notrunc 2>/dev/null

{
    gcc --version
    ld --version
    apk --repositories-file /dev/null info -vv | sort
} > /work/runtime-build-tools.txt
