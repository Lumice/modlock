#!/usr/bin/env python3
"""Check the QML files staged for the AppImage, optionally from a supplied QML tree.

Direct imports come from Qt's own qmlimportscanner, so QML syntax (comments,
BOMs, same-line statements, nested template literals) is parsed by Qt itself.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def require(condition, message):
    if not condition:
        sys.exit(message)


def query(name):
    return subprocess.check_output(['qtpaths6', '--query', name], text=True).strip()


qml_root = Path(query('QT_INSTALL_QML'))
scanner = Path(query('QT_HOST_LIBEXECS')) / 'qmlimportscanner'
require(scanner.is_file(), f'qmlimportscanner not found at {scanner}')


def scan(*roots):
    # An empty import path stops the scanner following Qt's own module
    # dependencies, so it reports exactly the modules the given files import.
    with tempfile.TemporaryDirectory(prefix='qbc-empty-') as empty:
        args = [str(scanner)]
        for root in roots:
            args += ['-rootPath', str(root)]
        args += ['-importPath', empty]
        result = subprocess.run(args, capture_output=True, text=True)
    require(result.returncode == 0, f'qmlimportscanner failed: {result.stderr}')
    return {entry['name'] for entry in json.loads(result.stdout)
            if entry.get('type') == 'module' and entry['name'].startswith('Qt')}


def scanner_cases():
    # Syntax the earlier hand-written scanners got wrong; each file is scanned
    # alone and must report exactly its real import.
    cases = [
        ('\ufeffimport QtQuick.Effects\nItem {}\n', {'QtQuick.Effects'}),
        ('import /* c */ QtQuick.Shapes\nItem {}\n', {'QtQuick.Shapes'}),
        ('import QtQuick.Layouts /* x\n*/\nimport QtQuick.Window\nItem {}\n', {'QtQuick.Layouts', 'QtQuick.Window'}),
        ('import QtQuick.Layouts; Item { property string s: `a ${`b\nimport QtQuick.Fake\n`} c` }\n', {'QtQuick.Layouts'}),
    ]
    for text, expected in cases:
        with tempfile.TemporaryDirectory(prefix='qbc-') as folder:
            Path(folder, 'Case.qml').write_text(text, encoding='utf-8')
            found = scan(folder)
        require(found == expected, f'scanner case {text!r}: expected {sorted(expected)}, got {sorted(found)}')
    print(f'PASS {len(cases)} import scanner syntax cases')


def check_staged(destination, imports):
    missing = sorted(module for module in imports if not (destination / module / 'qmldir').is_file())
    require(not missing, f'Imported modules missing from the AppImage QML tree: {missing}')


scanner_cases()
repo = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(repo / 'tools'))
from appimage_qml import stage_qml

imports = {name.replace('.', '/') for name in scan(repo / 'qml', repo / 'style')}
require(imports, 'no Qt imports found')
if len(sys.argv) == 1:
    with tempfile.TemporaryDirectory(prefix='modlock-qml-stage-') as folder:
        destination = Path(folder)
        stage_qml(qml_root, destination)
        check_staged(destination, imports)
elif len(sys.argv) == 2 and Path(sys.argv[1]).is_dir():
    check_staged(Path(sys.argv[1]), imports)
else:
    sys.exit('Usage: qml-bundle-check.py [staged-qml-directory]')
print(f'PASS all {len(imports)} directly imported Qt modules were staged')
