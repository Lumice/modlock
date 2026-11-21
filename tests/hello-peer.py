#!/usr/bin/python3
"""Exact hello guard: records every wire method, never touches an app root."""
import json
from pathlib import Path
import sys
root = Path(sys.argv[0]).parent
for line in sys.stdin:
    request = json.loads(line)
    with (root/'wire').open('a') as trace:
        trace.write(request['method'] + '\n')
    if request['method'] == 'hello':
        field = 'error' if (root/'hello-error').exists() else 'result'
        print(json.dumps({'jsonrpc':'2.0', 'id':request['id'], field:json.loads((root/'hello.json').read_text())}), flush=True)
    else:
        raise AssertionError('Post-hello traffic reached a deficient helper: ' + request['method'])
