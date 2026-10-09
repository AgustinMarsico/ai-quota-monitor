#!/usr/bin/python3
"""Generate both providers offline in project sandbox. Never reads user credentials."""
import json
import os
from pathlib import Path
import shutil
import subprocess
root=Path(__file__).resolve().parents[1]
sandbox=root/'sandbox'
sandbox.mkdir(exist_ok=True,mode=0o700)
(sandbox/'bin').mkdir(exist_ok=True)
shutil.copyfile(root/'tests/fixtures/mock-curl.py',sandbox/'bin/curl')
(sandbox/'bin/curl').chmod(0o755)
auth=sandbox/'fixture-auth.json';auth.write_text('{"opencode-go":{"key":"fixture-key"}}');auth.chmod(0o600)
config=sandbox/'config.json'
config.write_text(json.dumps({'notifications':False,'providers':[{'provider':'codex','codex_bin':str(root/'tests/fixtures/mock-codex.py')},{'provider':'opencode-go','auth_file':str(auth)}]},indent=2))
binary=root/'target/release/ai-quota'
if not binary.exists():binary=root/'target/debug/ai-quota'
env=dict(os.environ,PATH=str(sandbox/'bin')+':/usr/bin:/bin');env.pop('OPENCODE_API_KEY',None)
command=[str(binary),'--config',str(config),'--state-dir',str(sandbox/'state'),'--cache',str(sandbox/'cache/status.json')]
subprocess.run(command+['refresh','--compact'],env=env,check=True)
print('OFFLINE FIXTURES ONLY — separate cache and history:')
import shlex
print(shlex.join(command+['status','--compact']))
