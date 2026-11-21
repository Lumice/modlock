#!/usr/bin/env python3
"""Curated Qt deployment; intentionally keep the host graphics stack external."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

from appimage_qml import stage_qml
from appimage_sources import RUNTIME_COMMIT, RUNTIME_HASH, rpm_inventory, sha256, stage_notices

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('repo', type=Path)
parser.add_argument('build', type=Path)
parser.add_argument('app', type=Path)
parser.add_argument('--release-metadata', type=Path)
args = parser.parse_args()
repo, build, app = args.repo, args.build, args.app
release = json.loads(args.release_metadata.read_text()) if args.release_metadata else None
copied = {}
if app != build / 'Modlock.AppDir':
    raise SystemExit('Refusing unexpected AppDir path')
if app.exists():
    shutil.rmtree(app)

def query(key):
    return Path(subprocess.check_output(['qtpaths6', '--query', key], text=True).strip())

def copy(src, dst, distribution=True):
    dst.parent.mkdir(parents=True, exist_ok=True)
    original = src.resolve(strict=True)
    shutil.copy2(original, dst)
    if distribution:
        copied[dst] = (original, sha256(dst))

def elf(path):
    if path.is_file():
        with path.open('rb') as handle:
            return handle.read(4) == b'\x7fELF'
    return False

for name in ('modlock', 'modlock-helper', 'modlock-network'):
    copy(build / name, app / 'usr/bin' / name, distribution=False)
qml = query('QT_INSTALL_QML')
stage_qml(qml, app / 'usr/qml', copy)
plugins = query('QT_INSTALL_PLUGINS')
for group in ('platforms', 'imageformats', 'tls', 'wayland-decoration-client',
              'wayland-graphics-integration-client', 'wayland-shell-integration', 'xcbglintegrations'):
    for source in sorted((plugins / group).glob('*.so')):
        if group == 'imageformats' and source.name not in ('libqjpeg.so','libqgif.so','libqsvg.so','libqwebp.so','libqico.so'):
            continue
        if group == 'platforms' and source.name not in ('libqwayland.so','libqxcb.so','libqoffscreen.so'):
            continue
        copy(source, app / 'usr/plugins' / group / source.name)
# OpenSSL is dlopened by Qt's TLS backend, so ldd alone cannot discover it.
for name in ('libssl.so.3', 'libcrypto.so.3'):
    copy(query('QT_INSTALL_LIBS') / name, app / 'usr/lib' / name)
# Host ABI/graphics/driver libraries are deliberately not shipped.
external = re.compile(r'^(ld-linux|lib(c|m|mvec|dl|pthread|rt|resolv|util)\.so|lib(ICE|SM|GL|EGL|GLX|OpenGL|GLES|vulkan|drm|gbm|wayland-|X11|Xext|Xrender|Xfixes|Xcursor|Xi\.|Xrandr|Xinerama|Xdamage|Xcomposite|Xau|Xdmcp|xcb))')
queue = [p for p in app.rglob('*') if elf(p)]
seen = set()
while queue:
    binary = queue.pop()
    if binary in seen:
        continue
    seen.add(binary)
    result = subprocess.run(['ldd', str(binary)], text=True, capture_output=True, check=True)
    if 'not found' in result.stdout:
        raise SystemExit(f'Missing dependency for {binary}: {result.stdout}')
    for name, source in re.findall(r'^\s*(\S+) => (/\S+)', result.stdout, re.M):
        if external.match(name):
            continue
        target = app / 'usr/lib' / name
        if not target.exists():
            # Avoid selecting this machine's glibc-hwcaps/x86-64-v3 variant.
            baseline = query('QT_INSTALL_LIBS') / name
            copy(baseline if baseline.exists() else Path(source), target)
            queue.append(target)
# Sound previews decode MP3/Vorbis in-app and play PCM through QAudioSink, so
# no Qt media backend plugin or FFmpeg library may enter the bundle. This
# host's FFmpeg is a Packman --enable-nonfree build that must never be copied.
forbidden = sorted(str(p.relative_to(app)) for p in app.rglob('*')
                   if re.match(r'lib(av(codec|format|util|device|filter)|sw(resample|scale)|postproc)\.so', p.name)
                   or p.relative_to(app).as_posix().startswith('usr/plugins/multimedia'))
if forbidden:
    raise SystemExit(f'Media backend or FFmpeg files entered the bundle: {forbidden}')
if not (app / 'usr/lib/libQt6Multimedia.so.6').exists():
    raise SystemExit('Qt Multimedia (QAudioSink) is missing from the bundle')
# openSUSE ships ICU's data separately from its small libicudata stub.
for data in Path('/usr/share/icu').glob('*/icudt*l.dat'):
    major = re.search(r'icudt(\d+)', data.name).group(1)
    if (app / 'usr/lib' / f'libicudata.so.{major}').exists():
        copy(data, app / 'usr/share/icu' / data.name)

for p in app.rglob('*'):
    if elf(p):
        subprocess.run(['strip', '--strip-unneeded', str(p)], check=True)
(app / 'AppRun').write_text('''#!/bin/sh
set -eu
APPDIR=${APPDIR:-$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)}
export LD_LIBRARY_PATH="$APPDIR/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export ICU_DATA="$APPDIR/usr/share/icu"
export QT_PLUGIN_PATH="$APPDIR/usr/plugins"
export QML_IMPORT_PATH="$APPDIR/usr/qml"
export QML2_IMPORT_PATH="$APPDIR/usr/qml"
exec "$APPDIR/usr/bin/modlock" "$@"
''')
(app / 'AppRun').chmod(0o755)
(app / 'usr/bin/qt.conf').write_text('[Paths]\nPrefix=..\nLibraries=lib\nPlugins=plugins\nQmlImports=qml\n')
(app / 'modlock.desktop').write_text((repo / 'packaging/modlock.desktop.in').read_text().replace('@BINARY@', 'AppRun'))
copy(repo / 'packaging/modlock.svg', app / 'modlock.svg', distribution=False)
(app / '.DirIcon').symlink_to('modlock.svg')
doc = app / 'usr/share/doc/modlock'
stage_notices(repo, doc)
rpm_inventory(app, doc, copied, release=bool(release))
if release:
    source = (f"Modlock public Git repository: {release['url']}\n"
              f"Release tag: {release['tag']}\n"
              f"Commit: {release['commit']}\n"
              f"Matching source archive: {release['sources']}\n"
              f"Download the source archive alongside the AppImage.\n")
else:
    source = ('Local preview build. Keep its complete matching source with the binary.\n'
              'This preview does not include a verified source companion.\n'
              f'AppImage runtime SHA-256: {RUNTIME_HASH}\n')
source += (f'AppImage runtime commit: {RUNTIME_COMMIT}\n'
           'Build: tools/build-appimage.sh\n'
           'Notices and source records are in usr/share/doc/modlock.\n')
(doc / 'SOURCE.txt').write_text(source)
shutil.copy2(repo / 'packaging/appimage-runtime.md', doc / 'APPIMAGE-RUNTIME.md')
(doc / 'BUNDLED-LIBRARIES.txt').write_text('\n'.join(sorted(p.name for p in (app / 'usr/lib').iterdir())) + '\n')
# Normalize timestamps for repeatable SquashFS assembly with the same toolchain.
epoch = int(os.environ['SOURCE_DATE_EPOCH'])
for p in sorted(app.rglob('*'), reverse=True):
    os.utime(p, (epoch, epoch), follow_symlinks=False)
os.utime(app, (epoch, epoch))
