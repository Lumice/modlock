"""Capture the origin of staged AppImage files and assemble their source companion."""
import argparse
import csv
from dataclasses import dataclass
import hashlib
import json
import os
import pwd
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
from urllib.parse import quote, urlsplit
import xml.etree.ElementTree as ET


RPM_FORMAT = r'%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\t%{SOURCERPM}\t%{DISTURL}\t%{BUILDTIME}\t%{LICENSE}'
RPM_HEADERS = ('RPM', 'Source RPM', 'OBS build source', 'Build time (Unix)', 'License')
SRPM_REPO = 'https://download.opensuse.org/source/tumbleweed/repo/oss/src'
RUNTIME_COMMIT = 'fedfad51acc2860b2916a1eef32ba4a500ff31ef'
RUNTIME_HASH = '617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46'
NOTICE = re.compile(r'^(LICENSE|COPYING|COPYRIGHT|UNLICENSE|NOTICE|THIRD.PARTY|PATENTS)', re.I)


def output(args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def tsv(path, headers, rows):
    with path.open('w', newline='') as handle:
        writer = csv.writer(handle, delimiter='\t', lineterminator='\n')
        writer.writerow(headers)
        writer.writerows(rows)


def stage_cargo_notices(repo, doc):
    backend = repo / 'backend'
    graph = json.loads(output([
        'cargo', 'metadata', '--offline', '--locked', '--format-version=1',
        '--filter-platform=x86_64-unknown-linux-gnu',
    ], cwd=backend))
    crates = {package['id']: package for package in graph['packages']}
    nodes = {node['id']: node for node in graph['resolve']['nodes']}
    root = graph['resolve']['root']
    pending = [root]
    seen = set()
    rows = []
    while pending:
        identifier = pending.pop()
        if identifier in seen:
            continue
        seen.add(identifier)
        pending.extend(dep['pkg'] for dep in nodes[identifier]['deps']
                       if any(kind['kind'] != 'dev' for kind in dep['dep_kinds']))
        if identifier == root:
            continue
        crate = crates[identifier]
        folder = Path(crate['manifest_path']).parent
        relative = folder.relative_to(backend / 'vendor')
        crate_name = f"{crate['name']}-{crate['version']}"
        notice_dir = doc / 'licenses/cargo' / crate_name
        # Keep separate copies when a registry and path crate share a version.
        if len(relative.parts) == 2 and any(
                other['id'] != identifier and other['name'] == crate['name']
                and other['version'] == crate['version'] for other in crates.values()):
            notice_dir = notice_dir.with_name(crate_name + '-registry')
        notices = {path for path in folder.iterdir() if path.is_file() and NOTICE.match(path.name)}
        if crate['license_file']:
            notices.add(folder / crate['license_file'])
        if not notices:
            raise ValueError(f'No license notice for {crate_name}')
        notice_dir.mkdir(parents=True, exist_ok=True)
        for path in sorted(notices):
            shutil.copy2(path, notice_dir / path.name)
        rows.append((crate['name'], crate['version'], crate['license'], str(relative),
                     str(notice_dir.relative_to(doc))))
    tsv(doc / 'CARGO-DEPENDENCIES.tsv', ('Crate', 'Version', 'License', 'Vendor path', 'Notices'), sorted(rows))


def stage_notices(repo, doc):
    doc.mkdir(parents=True, exist_ok=True)
    for src, name in [('LICENSE', 'LICENSE-GPL-3.0.txt'),
                      ('THIRD-PARTY-NOTICES.md', 'THIRD-PARTY-NOTICES.md'),
                      ('style/fonts/OFL.txt', 'OFL-Archivo-Narrow.txt')]:
        shutil.copy2(repo / src, doc / name)
    for path in sorted((repo / 'backend/data').glob('perf-*')):
        if NOTICE.match(path.name.removeprefix('perf-')):
            shutil.copy2(path, doc / path.name)
    for license in (repo / 'third_party/kde').glob('LICENSE*'):
        shutil.copy2(license, doc / ('KDE-' + license.name))
    stage_cargo_notices(repo, doc)
    decoder_dir = doc / 'licenses/decoders'
    decoder_dir.mkdir(parents=True)
    for path, marker in [('dr_mp3/dr_mp3.h', 'This software is available as a choice'),
                         ('stb_vorbis/stb_vorbis.c', 'This software is available under 2 licenses')]:
        _, suffix = (repo / 'third_party' / path).read_text().rsplit(marker, 1)
        (decoder_dir / (Path(path).stem + '.txt')).write_text(marker + suffix.split('*/', 1)[0])
    shutil.copy2(repo / 'packaging/licenses/minimp3.txt', decoder_dir / 'minimp3.txt')
    shutil.copytree(repo / 'packaging/licenses/runtime', doc / 'licenses/runtime')
    shutil.copy2(repo / 'packaging/appimage-runtime-sources.json', doc / 'RUNTIME-SOURCES.json')


@dataclass(frozen=True)
class Rpm:
    name: str
    source: str
    disturl: str
    build_time: str
    license: str

    @classmethod
    def parse(cls, line):
        fields = line.split('\t')
        if len(fields) != 5 or not re.fullmatch(r'[\w.+-]+\.src\.rpm', fields[1]):
            raise ValueError(f'No exact source RPM recorded for {fields[0]}')
        return cls(*fields)

    def identity(self):
        return self.source, self.disturl

    def record(self):
        return self.name, self.source, self.disturl, self.build_time, self.license


def rpm_inventory(app, doc, copied, release=False):
    """Track the original bytes from the resolved copy path, before local edits."""
    payload = []
    packages = {}
    for target, (original, digest) in sorted(copied.items()):
        query = output(['rpm', '-qf', '--qf', RPM_FORMAT + r'\n', str(original)])
        package = Rpm.parse(query)
        if release:
            if not original.is_relative_to('/usr') or not package.disturl.startswith(
                    'obs://build.opensuse.org/openSUSE:Factory/'):
                raise ValueError(f'Release requires openSUSE Factory RPMs: {package.name}')
            # rpm's per-file digest covers the source contents before strip and qmldir changes.
            files = output(['rpm', '-qf', '--qf', r'[%{FILENAMES}\t%{FILEDIGESTS}\n]', str(original)])
            if digest != dict(row.split('\t', 1) for row in files.splitlines() if '\t' in row).get(str(original)):
                raise ValueError(f'Installed file differs from the RPM payload: {original}')
        packages[package.name] = package
        payload.append((str(target.relative_to(app)), str(original), package.name, digest))
    tsv(doc / 'RPM-PAYLOAD.tsv', ('Payload', 'Installed file', 'RPM', 'Installed file SHA-256'), payload)
    tsv(doc / 'RPM-PACKAGES.tsv', RPM_HEADERS, (p.record() for p in sorted(packages.values(), key=lambda p: p.name)))
    stage_rpm_notices(doc, packages.values())


def stage_rpm_notices(doc, packages):
    # Qt and other openSUSE packages often install a single set of notices in a
    # sibling binary package from the exact same SRPM.
    identities = {package.identity() for package in packages}
    listing = output(['rpm', '-qa', '--qf',
                      RPM_FORMAT + r'\t[%{FILENAMES}\t%{FILEFLAGS:fflags}\n]' + '\x1f'])
    for entry in listing.split('\x1f'):
        fields = entry.split('\t', 5)
        if len(fields) != 6 or tuple(fields[1:3]) not in identities:
            continue
        name, source = fields[0], fields[1]
        for row in fields[5].splitlines():
            if '\t' not in row:
                continue
            path, flags = row.split('\t', 1)
            path = Path(path)
            if 'l' not in flags or not path.is_file():
                continue
            destination = doc / 'licenses/rpm' / source / name / path.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, destination)


def fetch(url, destination, verify):
    """Fetch into a temporary file. Preserve any prior verified cache entry."""
    if destination.is_file():
        verify(destination)
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='download-', dir=destination.parent) as scratch:
        downloaded = Path(scratch) / destination.name
        subprocess.run(['curl', '--fail', '--location', '--silent', '--show-error',
                        '--proto', '=https', '--proto-redir', '=https', '--retry', '2',
                        '--connect-timeout', '15', '--max-time', '600',
                        url, '-o', str(downloaded)], check=True)
        verify(downloaded)
        downloaded.replace(destination)


def verify_srpm(path, expected, disturl):
    # Use the distribution's installed RPM trust database. Digests by themselves
    # don't prove that an arbitrary local file came from the distribution.
    result = output(['rpm', '-Kv', str(path)], env={**os.environ, 'LC_ALL': 'C'})
    if not re.search(r'\bSignature, key ID [a-f0-9]+: OK$', result, re.M | re.I):
        raise ValueError(f'Source RPM lacks a trusted signature: {expected}')
    metadata = output(['rpm', '-qp', '--qf',
                       r'%{NAME}-%{VERSION}-%{RELEASE}.src.rpm\t%{SOURCEPACKAGE}\t%{DISTURL}', str(path)])
    if metadata.split('\t') != [expected, '1', disturl]:
        raise ValueError(f'Source RPM identity does not match the installed build: {expected}')


def copy_source_notices(source, destination):
    for src in sorted(source.iterdir()):
        if src.is_symlink() or not src.is_file():
            continue
        if NOTICE.match(src.name):
            destination.mkdir(exist_ok=True)
            shutil.copy2(src, destination / src.name)
        if not re.search(r'\.(?:tgz|tar(?:\.(?:gz|bz2|xz|zst))?)$', src.name):
            continue
        members = output(['tar', '-tf', str(src)]).splitlines()
        licenses = [m for m in members if len(Path(m).parts) in (1, 2)
                    and NOTICE.match(Path(m).name) and not m.endswith('/')]
        if not licenses:
            continue
        destination.mkdir(exist_ok=True)
        for index, name in enumerate(licenses):
            contents = subprocess.check_output(['tar', '-xOf', str(src), '--', name])
            if contents:
                (destination / f'{src.name}-{index}-{Path(name).name}').write_bytes(contents)


def rpm_source_notices(path, destination):
    names = output(['rpm', '-qpl', str(path)]).splitlines()
    if any(Path(name).name != name or name in ('.', '..') for name in names):
        raise ValueError(f'Unexpected paths in {path.name}')
    with tempfile.TemporaryDirectory(prefix='rpm-notices-', dir=destination.parent) as scratch:
        with subprocess.Popen(['rpm2cpio', str(path)], stdout=subprocess.PIPE) as cpio:
            subprocess.run(['cpio', '--quiet', '--extract', '--no-absolute-filenames'],
                           stdin=cpio.stdout, cwd=scratch, check=True)
            if cpio.wait() != 0:
                raise ValueError(f'Could not read {path.name}')
        copy_source_notices(Path(scratch), destination)


def obs_source(source, disturl, destination):
    match = re.fullmatch(
        r'obs://build.opensuse.org/openSUSE:Factory/[\w-]+/([a-f0-9]{32})-([\w.+:-]+)',
        disturl)
    if not match:
        raise ValueError(f'Unknown OBS source identity for {source}')
    revision, package = match.groups()
    # For OBS multibuild flavors the revision is also served by the base package.
    package = package.split(':', 1)[0]
    api = f'https://api.opensuse.org/public/source/openSUSE:Factory/{quote(package)}'
    query = f'?rev={revision}&expand=1'
    manifest = destination / 'OBS-SOURCE.xml'
    fetch(api + query, manifest, lambda path: None)
    metadata = ET.parse(manifest).getroot()
    entries = {}
    for e in metadata.findall('entry'):
        name, digest, size = (e.attrib[key] for key in ('name', 'md5', 'size'))
        if not re.fullmatch(r'[\w.+%-]+', name) or name in ('.', '..', manifest.name) or name in entries:
            raise ValueError(f'Unexpected OBS source name for {source}')
        if not re.fullmatch(r'[0-9a-f]{32}', digest):
            raise ValueError(f'Unexpected OBS digest for {name}')
        entries[name] = digest, int(size)
    obs_listing = ''.join(f'{entries[name][0]}  {name}\n' for name in sorted(entries))
    if (metadata.get('srcmd5') != revision
            or hashlib.md5(obs_listing.encode()).hexdigest() != revision
            or not any(name.endswith('.spec') for name in entries)):
        raise ValueError(f'OBS source listing does not match the installed build: {source}')
    for name, (digest, size) in sorted(entries.items()):
        def verify_file(path):
            with path.open('rb') as stream:
                actual = hashlib.file_digest(stream, 'md5').hexdigest()
            if path.stat().st_size != size or actual != digest:
                raise ValueError(f'OBS source checksum mismatch for {source}/{name}')
        fetch(f'{api}/{quote(name)}{query}', destination / name, verify_file)


def stage_rpm_sources(doc, cache, destination):
    package_list = doc / 'RPM-PACKAGES.tsv'
    with package_list.open() as handle:
        packages = list(csv.DictReader(handle, delimiter='\t'))
    srpms = {}
    for p in packages:
        source, disturl = p['Source RPM'], p['OBS build source']
        if srpms.setdefault(source, disturl) != disturl:
            raise ValueError(f'Multiple OBS build sources share the name {source}')
    (destination / 'rpm').mkdir()
    rows = []
    for source, disturl in sorted(srpms.items()):
        print(f'Checking corresponding source {source}', flush=True)
        path = cache / source
        def verify_srpm_identity(file):
            verify_srpm(file, source, disturl)
        notice_dir = doc / 'licenses/rpm' / source
        notice_dir.parent.mkdir(parents=True, exist_ok=True)
        try:
            fetch(f'{SRPM_REPO}/{quote(source)}', path, verify_srpm_identity)
        except subprocess.CalledProcessError as error:
            # curl's HTTP-error status includes disappeared rolling packages.
            # A present but untrusted local RPM must fail rather than falling back.
            if error.cmd[0] != 'curl' or error.returncode != 22:
                raise
            staged = destination / 'obs' / source.removesuffix('.src.rpm')
            obs_source(source, disturl, staged)
            if not notice_dir.exists():
                copy_source_notices(staged, notice_dir)
        else:
            staged = destination / 'rpm' / source
            shutil.copy2(path, staged)
            verify_srpm_identity(staged)
            if not notice_dir.exists():
                rpm_source_notices(staged, notice_dir)
        if not notice_dir.exists():
            raise ValueError(f'No license notices for {source}')
        rows.append((source, disturl, str(staged.relative_to(destination))))
    tsv(destination / 'RPM-SOURCES.tsv', ('Source RPM', 'OBS build source', 'Archive path'), rows)
    shutil.copy2(destination / 'RPM-SOURCES.tsv', doc / 'RPM-SOURCES.tsv')


def stage_release_sources(repo, doc, cache, destination, metadata, runtime_build=None):
    """Build the source companion from verified release inputs."""
    destination.mkdir()
    stage_rpm_sources(doc, cache, destination)
    manifest = repo / 'packaging/appimage-runtime-sources.json'
    for item in json.loads(manifest.read_text()):
        def verify_archive(path):
            if sha256(path) != item['sha256']:
                raise ValueError(f"Runtime source hash mismatch: {item['file']}")
        fetch(item['url'], destination / 'runtime' / item['file'], verify_archive)
    shutil.copy2(manifest, destination / 'runtime/SOURCES.json')
    shutil.copy2(repo / 'packaging/appimage-runtime.md', destination / 'runtime/README.md')
    if runtime_build is not None:
        subprocess.run([str(repo / 'tools/appimage-runtime.sh'),
                        str(destination / 'runtime'), str(runtime_build)], check=True)
        for name in ('runtime-build-tools.txt', 'runtime-link.map'):
            shutil.copy2(runtime_build / name, destination / 'runtime' / name)
    shutil.copy2(doc / 'RPM-PACKAGES.tsv', destination / 'RPM-PACKAGES.tsv')
    shutil.copy2(doc / 'RPM-PAYLOAD.tsv', destination / 'RPM-PAYLOAD.tsv')
    with (destination / 'Modlock-source.tar.gz').open('wb') as archive:
        subprocess.run(['git', '-C', str(repo), 'archive', '--format=tar.gz',
                        '--prefix=Modlock/', metadata['commit']], stdout=archive, check=True)
    files = sorted(path for path in destination.rglob('*') if path.is_file())
    (destination / 'SHA256SUMS').write_text(''.join(
        f'{sha256(path)}  {path.relative_to(destination)}\n' for path in files))
    # The source set is already compressed. Use tar for a single downloadable asset.
    output_name = metadata['sources']
    archive = destination.parent / output_name
    with tarfile.open(archive, 'w') as tar:
        tar.add(destination, arcname=output_name.removesuffix('.tar'))
    return archive


def release_identity(repo, url, tag):
    parsed = urlsplit(url)
    if (parsed.scheme != 'https' or not parsed.hostname or parsed.username
            or parsed.password or parsed.query or parsed.fragment or not parsed.path.strip('/')
            or not re.fullmatch(r'https://[A-Za-z0-9.-]+/[A-Za-z0-9._/-]+', url)):
        raise ValueError('Release requires the public HTTPS Git clone URL without credentials')
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*', tag):
        raise ValueError('Use a release tag made from letters, numbers, dots, hyphens or underscores')
    git = ['git', '-C', str(repo)]
    if output(git + ['status', '--porcelain', '--untracked-files=all']):
        raise ValueError('Release requires a clean tag checkout')
    head = output(git + ['rev-parse', 'HEAD'])
    ref = f'refs/tags/{tag}'
    if output(git + ['rev-parse', '--verify', f'{ref}^{{commit}}']) != head:
        raise ValueError(f'Release tag {tag} must point at HEAD')
    # Verify that an anonymous downloader can resolve this exact public tag.
    with tempfile.TemporaryDirectory(prefix='modlock-git-') as scratch:
        env = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
        env.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null',
                   GIT_TERMINAL_PROMPT='0', GIT_ASKPASS='/bin/false')
        refs = output(['git', 'ls-remote', '--exit-code', url, ref, ref + '^{}'],
                      cwd=scratch, env=env)
    remote = {name: revision for revision, name in (line.split('\t') for line in refs.splitlines())}
    if remote.get(ref + '^{}', remote.get(ref)) != head:
        raise ValueError(f'Public release tag {tag} does not point at HEAD')
    return dict(url=url, tag=tag, commit=head, sources=f'Modlock-{tag}-x86_64-sources.tar')


def verify_packaged_release(app, image, runtime, paths, expected_runtime=RUNTIME_HASH):
    original = runtime.read_bytes()
    with image.open('rb') as handle:
        header = handle.read(len(original))
        if handle.read(4) != b'hsqs':
            raise ValueError('AppImage runtime size does not match the SquashFS offset')
    # appimagetool writes the payload digest into this reserved ELF section.
    sections = output(['readelf', '-SW', str(runtime)])
    match = re.search(r'\.digest_md5\s+\S+\s+[0-9a-f]+\s+([0-9a-f]+)\s+000010\s', sections, re.I)
    if not match:
        raise ValueError('AppImage runtime lacks the expected digest field')
    offset = int(match.group(1), 16)
    digest = slice(offset, offset + 16)
    if (sha256(runtime) != expected_runtime or header[:digest.start] != original[:digest.start]
            or header[digest.stop:] != original[digest.stop:]):
        raise ValueError('AppImage runtime differs from the verified input binary')
    home = str(Path.home())
    account_home = pwd.getpwuid(os.getuid()).pw_dir
    forbidden = {os.fsencode(path) for path in [home, account_home, *paths] if len(path) > 1}
    for path in app.rglob('*'):
        if path.is_symlink():
            content = os.fsencode(os.readlink(path))
        elif path.is_file():
            content = path.read_bytes()
        else:
            continue
        if any(pattern in content for pattern in forbidden):
            raise ValueError(f'Build machine path leaked into {path.relative_to(app)}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest='action', required=True)
    identity = actions.add_parser('identity')
    identity.add_argument('repo', type=Path)
    identity.add_argument('url')
    identity.add_argument('tag')
    sources = actions.add_parser('sources')
    sources.add_argument('repo', type=Path)
    sources.add_argument('doc', type=Path)
    sources.add_argument('cache', type=Path)
    sources.add_argument('destination', type=Path)
    sources.add_argument('metadata', type=Path)
    sources.add_argument('--build-runtime', type=Path)
    verify = actions.add_parser('verify')
    verify.add_argument('app', type=Path)
    verify.add_argument('image', type=Path)
    verify.add_argument('runtime', type=Path)
    verify.add_argument('paths', nargs='+')
    verify.add_argument('--runtime-sha256', default=RUNTIME_HASH)
    args = parser.parse_args()
    try:
        if args.action == 'identity':
            print(json.dumps(release_identity(args.repo, args.url, args.tag)))
        elif args.action == 'sources':
            stage_release_sources(args.repo, args.doc, args.cache, args.destination,
                                  json.loads(args.metadata.read_text()), args.build_runtime)
        else:
            verify_packaged_release(args.app, args.image, args.runtime, args.paths, args.runtime_sha256)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from None


if __name__ == '__main__':
    main()
