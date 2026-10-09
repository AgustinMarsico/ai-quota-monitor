#!/usr/bin/python3
"""Reproducible user/staging install. Does not run systemctl or enable extensions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--prefix', type=Path, default=Path.home() / '.local')
parser.add_argument('--config-dir', type=Path, default=Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config')) / 'ai-quota')
parser.add_argument('--unit-dir', type=Path, default=Path.home() / '.config/systemd/user')
parser.add_argument('--gnome-dir', type=Path, help='Optional directory for ai-quota@local; does not enable it')
parser.add_argument('--binary', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
binary = args.binary or root / 'target/release/ai-quota'
if not binary.is_file():
    parser.error('Build first: cargo build --release --locked')
manifest = []
def install(source, destination, mode=0o644):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)
    manifest.append({'path': str(destination.resolve()), 'sha256': hashlib.sha256(destination.read_bytes()).hexdigest()})
prefix = args.prefix.resolve()
share = prefix / 'share/ai-quota'
install(binary, prefix / 'bin/ai-quota', 0o755)
for source in ['integrations/opencode/plugin.tsx', 'integrations/shared/view.mjs', 'schema/status-v1.schema.json']:
    install(root / source, share / Path(source).relative_to('integrations') if source.startswith('integrations/') else share / source)
args.config_dir.mkdir(parents=True, exist_ok=True)
args.config_dir.chmod(0o700)
config = args.config_dir.resolve() / 'config.json'
if not config.exists():
    data = json.loads((root / 'config/example.json').read_text())
    codex = shutil.which('codex')
    if codex:
        data['providers'][0]['codex_bin'] = codex
    config.write_text(json.dumps(data, indent=2) + '\n')
    config.chmod(0o600)
def systemd_quote(value):
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%') + '"'
unit = args.unit_dir.resolve() / 'ai-quota-monitor.service'
unit.parent.mkdir(parents=True, exist_ok=True)
unit.write_text('[Unit]\nDescription=Unified AI quota monitor\n\n[Service]\nType=simple\n'
                f'ExecStart={systemd_quote(prefix / "bin/ai-quota")} daemon --config {systemd_quote(config)}\n'
                'Restart=on-failure\nRestartSec=20\nTimeoutStopSec=60\nUMask=0077\nNoNewPrivileges=yes\n'
                f'Environment={systemd_quote("PATH=" + str(prefix / "bin") + ":/usr/local/bin:/usr/bin:/bin")}\n'
                'StandardOutput=journal\nStandardError=journal\n\n[Install]\nWantedBy=default.target\n')
unit.chmod(0o644)
manifest.append({'path': str(unit), 'sha256': hashlib.sha256(unit.read_bytes()).hexdigest()})
if args.gnome_dir:
    for source in (root / 'integrations/gnome/ai-quota@local').iterdir():
        install(source, args.gnome_dir / 'ai-quota@local' / source.name)
    install(root / 'integrations/shared/view.mjs', args.gnome_dir / 'ai-quota@local/view.mjs')
(share / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(f'Installed {prefix / "bin/ai-quota"}; configuration preserved at {config}')
print('Service and extensions are NOT enabled. See docs/installation.md for explicit activation.')
