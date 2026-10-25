#!/usr/bin/python3
"""Install only the GNOME extension; never reload Shell or touch the monitor."""

import argparse
import os
import shutil
import subprocess
from pathlib import Path


def install_extension(destination):
    root = Path(__file__).resolve().parents[1]
    source = root / "integrations/gnome/ai-quota@local"
    compiler = shutil.which("glib-compile-schemas")
    if compiler is None:
        raise RuntimeError("glib-compile-schemas is required (Fedora: glib2)")
    # Validate before copying anything into the target extension.
    subprocess.run(
        [compiler, "--strict", "--dry-run", str(source / "schemas")], check=True
    )
    installed = []
    for file in sorted(source.rglob("*")):
        if not file.is_file() or file.name == "gschemas.compiled":
            continue
        target = destination / file.relative_to(source)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(file, target)
        target.chmod(0o644)
        installed.append(target)
    view = destination / "view.mjs"
    shutil.copyfile(root / "integrations/shared/view.mjs", view)
    view.chmod(0o644)
    installed.append(view)
    subprocess.run([compiler, "--strict", str(destination / "schemas")], check=True)
    compiled = destination / "schemas/gschemas.compiled"
    compiled.chmod(0o644)
    installed.append(compiled)
    return installed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--destination",
        type=Path,
        default=Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
        / "gnome-shell/extensions/ai-quota@local",
        help="Extension directory, including ai-quota@local (also supports staging)",
    )
    args = parser.parse_args()
    install_extension(args.destination.resolve())
    print(f"GNOME extension installed at {args.destination.resolve()}")
    print("Log out and back in to load the updated JavaScript in GNOME/Wayland.")


if __name__ == "__main__":
    main()
