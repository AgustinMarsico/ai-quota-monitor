#!/usr/bin/python3
"""Real TUI probe in a PTY; all HOME/XDG paths are temporary inside target/."""

import argparse
import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path

import pyte

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--opencode", default=shutil.which("opencode"))
parser.add_argument("--plugin", default=str(root / "integrations/opencode/minimal.ts"))
parser.add_argument("--expect", default="AI_QUOTA_TUI_API_OK")
parser.add_argument(
    "--exercise", action="store_true", help="Check live cache update and /quota dialog"
)
args = parser.parse_args()
if not args.opencode:
    parser.error("OpenCode executable missing; specify --opencode")
with tempfile.TemporaryDirectory(
    prefix="opencode-probe-", dir=root / "target"
) as folder:
    home = Path(folder)
    for name in ["config", "cache", "data", "state", "project"]:
        (home / name).mkdir()
    config = home / "project/tui.json"
    config.write_text(
        json.dumps(
            {
                "$schema": "https://opencode.ai/tui.json",
                "plugin": [
                    [
                        args.plugin,
                        {"cachePath": str(home / "cache/ai-quota/status.json")},
                    ]
                ],
            }
        )
    )
    (home / "project/opencode.json").write_text(
        json.dumps(
            {
                "$schema": "https://opencode.ai/config.json",
                "autoupdate": False,
                "enabled_providers": ["probe"],
                "model": "probe/fixture",
                "provider": {
                    "probe": {
                        "npm": "@ai-sdk/openai-compatible",
                        "name": "Local test fixture",
                        "models": {"fixture": {"name": "Fixture"}},
                        "options": {
                            "baseURL": "http://127.0.0.1:9/v1",
                            "apiKey": "fixture-not-a-credential",
                        },
                    }
                },
            }
        )
    )
    (home / "cache/ai-quota").mkdir()
    sample = {
        "schema_version": 1,
        "generated_at": int(time.time()),
        "stale_seconds": 600,
        "providers": [
            {
                "provider": "codex",
                "profile": "default",
                "account": None,
                "plan": None,
                "model": None,
                "source": "fixture",
                "limitations": [],
                "last_success_at": int(time.time()),
                "last_attempt_at": int(time.time()),
                "error": None,
                "state": "fresh",
                "next_attempt_at": int(time.time()) + 180,
                "consecutive_failures": 0,
                "quotas": [
                    {
                        "window": "primary",
                        "label": "5h",
                        "used_percent": 22,
                        "remaining_percent": 78,
                        "duration_minutes": 300,
                        "resets_at": None,
                    }
                ],
            }
        ],
    }
    (home / "cache/ai-quota/status.json").write_text(json.dumps(sample))
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "TERM": "xterm-256color",
        "LANG": "C.UTF-8",
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / "config"),
        "XDG_CACHE_HOME": str(home / "cache"),
        "XDG_DATA_HOME": str(home / "data"),
        "XDG_STATE_HOME": str(home / "state"),
        "OPENCODE_DISABLE_DEFAULT_PLUGINS": "1",
        "OPENCODE_DISABLE_EXTERNAL_SKILLS": "1",
        "OPENCODE_DISABLE_MODELS_FETCH": "1",
        "OPENCODE_TUI_CONFIG": str(config),
    }
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
    child = subprocess.Popen(
        [args.opencode, str(home / "project")],
        env=env,
        cwd=home / "project",
        stdin=slave,
        stdout=slave,
        stderr=slave,
        start_new_session=True,
    )
    os.close(slave)
    captured = bytearray()
    screen = pyte.Screen(160, 40)
    stream = pyte.ByteStream(screen)
    deadline = time.monotonic() + 45
    phase = 0
    clean = b""
    needle = re.sub(rb"\s+", b"", args.expect.encode())
    try:
        while time.monotonic() < deadline and child.poll() is None:
            ready, _, _ = select.select([master], [], [], 0.2)
            if not ready:
                continue
            try:
                data = os.read(master, 65536)
            except OSError:
                break
            captured.extend(data)
            stream.feed(data)
            # Reply to terminal capability probes so the renderer can initialize.
            if b"\x1b[6n" in data:
                os.write(master, b"\x1b[1;1R")
            if b"\x1b[c" in data or b"\x1b[>0c" in data:
                os.write(master, b"\x1b[?1;2c")
            clean = "\n".join(screen.display).encode()
            normalized = re.sub(rb"\s+", b"", clean)
            if needle in normalized:
                if args.exercise and phase == 0:
                    print(f"PASS: OpenCode real TUI rendered {args.expect}")
                    sample["providers"][0]["quotas"][0].update(
                        {"used_percent": 30, "remaining_percent": 70}
                    )
                    (home / "cache/ai-quota/status.json").write_text(json.dumps(sample))
                    phase = 1
                    needle = b"Codex5h:70%"
                    captured.clear()
                elif args.exercise and phase == 1:
                    print(
                        "PASS: TUI updated from central cache without provider requests"
                    )
                    os.write(master, b"/quota")
                    time.sleep(0.5)
                    os.write(master, b"\r")
                    phase = 2
                    needle = b"Lastsuccess:"
                    captured.clear()
                else:
                    print(f"PASS: OpenCode real TUI rendered phase {phase}")
                    break
        else:
            pass
        if needle not in re.sub(rb"\s+", b"", clean):
            (root / "target/opencode-probe-failure.log").write_bytes(
                re.sub(rb" {3,}", b"\n", clean)
            )
            logs = list((home / "data/opencode/log").glob("*.log"))
            if logs:
                (root / "target/opencode-probe-runtime.log").write_bytes(
                    logs[-1].read_bytes()
                )
            raise SystemExit(
                "FAIL: expected text missing; see target/opencode-probe-failure.log"
            )
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
        os.close(master)
