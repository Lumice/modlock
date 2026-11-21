"""Stage the QML modules used by the Modlock AppImage."""
from pathlib import Path
import shutil


def stage_qml(qml, destination, copy=shutil.copy2):
    # Explicit transitive closure for this app's imports and DeadlockStyle's fallback.
    modules = ['Qt/labs/folderlistmodel', 'QtCore', 'QtQml', 'QtQml/Models', 'QtQml/WorkerScript', 'QtQuick',
               'QtQuick/Window', 'QtQuick/Layouts', 'QtQuick/Templates',
               'QtQuick/Controls', 'QtQuick/Controls/impl', 'QtQuick/Controls/Basic',
               'QtQuick/Controls/Basic/impl', 'QtQuick/Controls/Fusion',
               'QtQuick/Controls/Fusion/impl', 'QtQuick/Dialogs',
               'QtQuick/Dialogs/quickimpl', 'QtQuick/Effects', 'QtQuick/Shapes']
    for module in modules:
        source = qml / module
        if not source.exists():
            continue  # Some Qt versions put implementation types in the parent plugin.
        for path in source.rglob('*'):
            relative = path.relative_to(source)
            if not path.is_file() or path.suffix == '.qmltypes':
                continue
            # Do not recursively pull in sibling modules/styles.
            if any((source / Path(*relative.parts[:i]) / 'qmldir').exists()
                   for i in range(1, len(relative.parts))):
                continue
            if any(p.startswith('+') and p != '+Fusion' for p in relative.parts):
                continue
            target = destination / module / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            copy(path, target)
            if path.name == 'qmldir':
                lines = target.read_text().splitlines()
                target.write_text('\n'.join(l for l in lines if not (
                    l.startswith('optional import QtQuick.Controls.') and
                    not l.startswith('optional import QtQuick.Controls.Fusion ')) and
                    not any('+' + s + '/' in l for s in ['Imagine','Material','Universal'])) + '\n')
