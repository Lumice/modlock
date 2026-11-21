#!/usr/bin/env python3
"""Link the focused controller check against the already-built application objects."""
import shlex
import subprocess
import sys
from pathlib import Path

build = Path(sys.argv[1]).resolve()
source = Path(__file__).with_name('catalog-action-check.cpp').resolve()
project = source.parent.parent
obj = build / 'catalog-action-check.o'
binary = build / 'catalog-action-check'
flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'Qt6Quick', 'Qt6Qml', 'Qt6Network'], text=True))
subprocess.run(['c++', '-std=c++20', '-fPIC', '-I' + str(project / 'src'), *flags, '-c', str(source), '-o', str(obj)], check=True)
commands = subprocess.check_output(['ninja', '-C', str(build), '-t', 'commands', 'modlock'], text=True)
link = shlex.split(commands.splitlines()[-1])
link = link[link.index('/usr/bin/c++'):]
if '&&' in link:
    link = link[:link.index('&&')]
link = [str(obj) if arg.endswith('/src/main.cpp.o') else arg for arg in link]
link = [arg for arg in link if not arg.startswith('-Wl,--dependency-file=')]
link[link.index('-o') + 1] = str(binary)
subprocess.run(link, cwd=build, check=True)
subprocess.run([str(binary)], cwd=build, check=True)
