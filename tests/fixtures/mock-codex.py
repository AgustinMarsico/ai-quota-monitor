#!/usr/bin/python3
import json
import os
import sys
import time
from pathlib import Path

mode_path = (
    Path(os.environ.get("CODEX_HOME", "/nonexistent-fixture-home")) / ".fixture-mode"
)
mode = mode_path.read_text() if mode_path.is_file() else "ok"
for line in sys.stdin:
    request = json.loads(line)
    if request["method"] == "initialize":
        print(json.dumps({"id": request["id"], "result": {}}), flush=True)
    elif request["method"] == "account/rateLimits/read":
        if mode == "hang":
            (mode_path.parent / ".fixture-pid").write_text(str(os.getpid()))
            time.sleep(60)
            continue
        if mode == "disconnect":
            sys.exit(0)
        if mode == "invalid":
            print("not valid JSON", flush=True)
            continue
        if mode == "expired":
            print(
                json.dumps(
                    {
                        "id": request["id"],
                        "error": {"code": 401, "message": "SECRET remote content"},
                    }
                ),
                flush=True,
            )
            continue
        print(json.dumps({"method": "notification", "params": {}}), flush=True)
        print(
            json.dumps(
                {
                    "id": request["id"],
                    "result": {
                        "rateLimits": {
                            "primary": {
                                "usedPercent": 96 if mode == "low" else 22,
                                "windowDurationMins": 300,
                            },
                            "secondary": {
                                "usedPercent": 38,
                                "windowDurationMins": 10080,
                            },
                        }
                    },
                }
            ),
            flush=True,
        )
