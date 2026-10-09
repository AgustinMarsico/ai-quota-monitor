#!/usr/bin/python3
import json
import sys
for line in sys.stdin:
    request = json.loads(line)
    if request['method'] == 'initialize':
        print(json.dumps({'id': request['id'], 'result': {}}), flush=True)
    elif request['method'] == 'account/rateLimits/read':
        print(json.dumps({'method': 'notification', 'params': {}}), flush=True)
        print(json.dumps({'id': request['id'], 'result': {'rateLimits': {'primary': {'usedPercent': 22, 'windowDurationMins': 300}}}}), flush=True)
