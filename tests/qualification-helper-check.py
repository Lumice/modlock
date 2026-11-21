#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import tempfile
import subprocess
import sys

path=Path(__file__).with_name('qualification')/'qualify.py'
spec=importlib.util.spec_from_file_location('qualification',path)
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
with tempfile.TemporaryDirectory(prefix='modlock-qualification-names-') as tmp:
    root=Path(tmp);source=root/'source';source.mkdir();frozen=root/'frozen';frozen.mkdir()
    for name in ['modlock-helper','modlock-network']:
        p=source/name;p.write_text(name);p.chmod(0o700)
    helper=module.freeze_binaries(source/'modlock-helper',frozen)
    assert helper.name=='modlock-helper'
    for name in ['modlock-helper','modlock-network']:
        (source/name).write_bytes(b'changed after qualification began')
        assert (helper.parent/name).read_text()==name
    assert sorted(p.name for p in helper.parent.iterdir())==['modlock-helper','modlock-network']
    print('PASS qualification freezes both new helper names and is independent of later rebuilds')

    for name in ['legacy-restore-check.py','storage-cross-version-check.py','game-format-cross-version-check.py']:
        result=subprocess.run([sys.executable,str(Path(__file__).with_name(name)),'--helper',str(helper),'--old-helper',str(root/'missing/deadlock-helper')],text=True,capture_output=True)
        assert result.returncode!=0 and 'Expected pre-rename helper is missing' in result.stderr,(name,result.stdout,result.stderr)
    print('PASS every historical suite fails immediately when its expected old helper is missing')
