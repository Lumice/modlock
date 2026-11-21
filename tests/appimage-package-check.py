#!/usr/bin/env python3
"""Verify notice and provenance coverage in a staged or extracted AppImage."""
import csv
import hashlib
import json
import tarfile
from pathlib import Path
import sys

app = Path(sys.argv[1])
doc = app / 'usr/share/doc/modlock'
repo = Path(__file__).resolve().parents[1]

for path in ['perf-LICENSE-SchemaExplorer.txt', 'perf-LICENSE-grimoire.txt']:
    assert (doc / path).read_bytes() == (repo / 'backend/data' / path).read_bytes(), path
for path in (repo / 'third_party/kde').glob('LICENSE*'):
    assert (doc / ('KDE-' + path.name)).read_bytes() == path.read_bytes(), path
for path in (repo / 'packaging/licenses/runtime').glob('*/*'):
    notice = path.relative_to(repo / 'packaging/licenses/runtime')
    assert (doc / 'licenses/runtime' / notice).read_bytes() == path.read_bytes(), notice

def records(name, key):
    with (doc / name).open() as handle:
        return {row[key]: row for row in csv.DictReader(handle, delimiter='\t')}


cargo = records('CARGO-DEPENDENCIES.tsv', 'Crate')
for crate in ['keyvalues-parser', 'lz4_flex', 'ruzstd']:
    notices = list((doc / cargo[crate]['Notices']).glob('*'))
    assert any('Permission is hereby granted' in path.read_text() for path in notices), crate
for decoder in ['dr_mp3', 'stb_vorbis', 'minimp3']:
    assert (doc / 'licenses/decoders' / f'{decoder}.txt').stat().st_size > 400, decoder

payload = records('RPM-PAYLOAD.tsv', 'Payload')
packages = records('RPM-PACKAGES.tsv', 'RPM')
for directory in ('usr/lib', 'usr/plugins', 'usr/qml', 'usr/share/icu'):
    expected = {str(path.relative_to(app)) for path in (app / directory).rglob('*') if path.is_file()}
    assert expected == {path for path in payload if path.startswith(directory + '/')}, directory
for path, row in payload.items():
    assert row['RPM'] in packages, path
    if path.startswith('usr/plugins/'):
        assert '/plugins/' in row['Installed file'], path
    if path.startswith('usr/lib/'):
        assert not path.startswith(('usr/lib/libmvec.', 'usr/lib/libav')), path
for row in packages.values():
    assert row['Source RPM'].endswith('.src.rpm'), row
    assert row['OBS build source'].startswith('obs://'), row
source = (doc / 'SOURCE.txt').read_text()
assert 'Working tree changes' not in source
assert 'GPL-3.0 source:' not in source
if len(sys.argv) > 2:
    source_archive = Path(sys.argv[2])
    with source_archive.open('rb') as handle:
        assert f'Matching source archive SHA-256: {hashlib.file_digest(handle, "sha256").hexdigest()}' in source
    origins = records('RPM-SOURCES.tsv', 'Source RPM')
    assert set(origins) == {row['Source RPM'] for row in packages.values()}
    with tarfile.open(source_archive) as archive:
        prefix = source_archive.stem
        hashes = archive.extractfile(f'{prefix}/SHA256SUMS').read().decode().splitlines()
        manifest = dict(reversed(line.split('  ', 1)) for line in hashes)
        assert 'Modlock-source.tar.gz' in manifest
        for record in origins.values():
            source_path = record['Archive path']
            entry = archive.getmember(f'{prefix}/{source_path}')
            if entry.isdir():
                assert f'{source_path}/OBS-SOURCE.xml' in manifest
                assert any(p.startswith(source_path + '/') and p.endswith('.spec') for p in manifest)
            else:
                assert source_path in manifest
        for record in json.loads((doc / 'RUNTIME-SOURCES.json').read_text()):
            assert manifest['runtime/' + record['file']] == record['sha256']
        # The release's runtime source must correspond to the binary the
        # packager actually built and embedded.
        build_info = archive.extractfile(f'{prefix}/runtime/runtime-build-tools.txt').read().decode()
        runtime_digest = next(line.split()[1] for line in build_info.splitlines() if line.startswith('Runtime '))
        assert f'AppImage runtime SHA-256: {runtime_digest}' in source
        assert 'runtime/runtime-link.map' in manifest
        actual = {entry.name.removeprefix(prefix + '/') for entry in archive if entry.isfile()}
        assert actual == set(manifest) | {'SHA256SUMS'}
        for path, digest in manifest.items():
            with archive.extractfile(f'{prefix}/{path}') as handle:
                assert hashlib.file_digest(handle, 'sha256').hexdigest() == digest, path
    print(f'PASS matching source archive checksums and coverage for {len(origins)} source packages')
print(f'PASS AppImage notices and RPM provenance for {len(payload)} files and {len(packages)} packages')
