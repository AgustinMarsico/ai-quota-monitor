#!/usr/bin/python3
"""Offline Go HTTP fixture; never makes a network request."""

import json
import sys

if "fixture-key" in " ".join(sys.argv):
    sys.exit(2)
if "Authorization: Bearer fixture-key" not in sys.stdin.read():
    sys.exit(2)
print(
    "HTTP/2 200\r\nContent-Type: application/json\r\n\r\n"
    + json.dumps(
        {
            "usage": {
                "rolling": {"percent": 9},
                "weekly": {"percent": 55},
                "monthly": {"percent": 7},
            }
        }
    )
    + "\n__AI_QUOTA_HTTP__200"
)
