#!/usr/bin/python3
"""Remove only unchanged installed project files; preserve config, history and credentials."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--prefix", type=Path, default=Path.home() / ".local")
args = parser.parse_args()
manifest = args.prefix.resolve() / "share/ai-quota/manifest.json"
if not manifest.is_file():
    parser.error("No installation manifest found")
items = json.loads(manifest.read_text())
real_unit = Path.home() / ".config/systemd/user/ai-quota-monitor.service"
if any(Path(item["path"]) == real_unit for item in items):
    check = subprocess.run(
        ["systemctl", "--user", "is-active", "ai-quota-monitor.service"],
        capture_output=True,
        check=False,
    )
    if check.returncode == 0:
        parser.error(
            "Stop the new service first: systemctl --user disable --now ai-quota-monitor.service"
        )
for item in items:
    path = Path(item["path"])
    if (
        path.is_file()
        and hashlib.sha256(path.read_bytes()).hexdigest() == item["sha256"]
    ):
        path.unlink()
    elif path.exists():
        print(f"Preserved locally modified file: {path}")
manifest.unlink()
print(
    "Uninstalled project files; configuration and data preserved. Reload systemd manually if applicable."
)
