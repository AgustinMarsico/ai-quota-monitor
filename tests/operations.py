#!/usr/bin/python3
"""Offline operational acceptance: installation, daemon recovery and migration."""
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
import time
import unittest
root=Path(__file__).resolve().parents[1]
binary=root/'target/debug/ai-quota'

class Operations(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(dir=root/'target',prefix='operations-')
        self.path=Path(self.temp.name)
        self.config=self.path/'config.json'
        self.config.write_text(json.dumps({'notifications':False,'providers':[{'provider':'codex','codex_bin':str(root/'tests/fixtures/mock-codex.py')}]}))
        self.base=[str(binary),'--config',str(self.config),'--state-dir',str(self.path/'state'),'--cache',str(self.path/'cache/status.json')]
    def tearDown(self):self.temp.cleanup()
    def call(self,*args):return subprocess.run(self.base+list(args),capture_output=True,text=True,check=True)
    def test_daemon_restart_and_single_instance(self):
        child=subprocess.Popen(self.base+['daemon'],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        try:
            deadline=time.monotonic()+5
            while not (self.path/'cache/status.json').exists() and time.monotonic()<deadline:time.sleep(.05)
            self.assertTrue((self.path/'cache/status.json').exists())
            duplicate=subprocess.run(self.base+['daemon'],capture_output=True,text=True,timeout=3)
            self.assertNotEqual(duplicate.returncode,0)
            self.assertIn('already running',duplicate.stderr)
            child.send_signal(signal.SIGTERM);self.assertEqual(child.wait(timeout=4),0)
            child.stderr.close()
            sample=json.loads(self.call('status','--json').stdout)
            before=sample['providers'][0]['last_success_at']
            (self.path/'cache/status.json').unlink()
            child=subprocess.Popen(self.base+['daemon'],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
            deadline=time.monotonic()+5
            while not (self.path/'cache/status.json').exists() and time.monotonic()<deadline:time.sleep(.05)
            sample=json.loads(self.call('status','--json').stdout)
            self.assertEqual(sample['providers'][0]['last_success_at'],before)
            child.kill();child.wait(timeout=3)
            self.call('refresh') # kernel releases locks after forced termination
        finally:
            if child.poll() is None:child.kill();child.wait()
            if child.stderr:child.stderr.close()
    def test_install_uninstall_staging(self):
        prefix=self.path/'prefix';unit=self.path/'units';config=self.path/'config'
        subprocess.run(['python3',str(root/'scripts/install.py'),'--prefix',str(prefix),'--unit-dir',str(unit),'--config-dir',str(config),'--gnome-dir',str(self.path/'gnome'),'--binary',str(binary)],check=True,capture_output=True)
        self.assertTrue((prefix/'bin/ai-quota').exists());self.assertTrue((self.path/'gnome/ai-quota@local/view.mjs').exists())
        subprocess.run(['systemd-analyze','--user','verify',str(unit/'ai-quota-monitor.service')],check=True,capture_output=True)
        custom=config/'config.json';original=custom.read_bytes();custom.write_bytes(original)
        subprocess.run(['python3',str(root/'scripts/uninstall.py'),'--prefix',str(prefix)],check=True,capture_output=True)
        self.assertFalse((prefix/'bin/ai-quota').exists());self.assertEqual(custom.read_bytes(),original)
    def test_legacy_wal_import_idempotent_and_read_only(self):
        source=self.path/'legacy';source.mkdir();database=source/'usage.sqlite3'
        legacy=sqlite3.connect(database);legacy.execute('PRAGMA journal_mode=WAL')
        legacy.execute('CREATE TABLE samples(id INTEGER PRIMARY KEY,sampled_at INTEGER,bucket_id TEXT,window_kind TEXT,duration_minutes INTEGER,resets_at INTEGER,used_percent REAL,remaining_percent REAL)')
        legacy.execute('INSERT INTO samples VALUES(1,?,"codex","primary",300,0,22,78)',(int(time.time()),));legacy.commit()
        before={p.name:p.read_bytes() for p in source.iterdir()}
        self.call('migrate','--provider','codex','--from',str(source))
        self.call('migrate','--provider','codex','--from',str(source))
        self.assertEqual(before,{p.name:p.read_bytes() for p in source.iterdir()})
        db=sqlite3.connect(self.path/'state/usage.sqlite3');self.assertEqual(db.execute('SELECT count(*) FROM samples').fetchone()[0],1);db.close();legacy.close()
        self.assertIn('78',self.call('export').stdout)
    def test_go_transport_with_fake_curl_and_no_network(self):
        auth=self.path/'auth.json';auth.write_text('{"opencode-go":{"key":"fixture-key"}}')
        self.config.write_text(json.dumps({'notifications':False,'providers':[{'provider':'opencode-go','auth_file':str(auth)}]}))
        commands=self.path/'bin';commands.mkdir();curl=commands/'curl'
        curl.write_text('#!/usr/bin/python3\nimport os,sys,json\nassert "fixture-key" not in " ".join(sys.argv)\nassert "Authorization: Bearer fixture-key" in sys.stdin.read()\ncode=os.environ.get("FIXTURE_HTTP","200")\nif code=="network":sys.exit(7)\nprint("HTTP/2 "+code+"\\r\\nRetry-After: 600\\r\\n\\r\\n"+json.dumps({"usage":{"rolling":{"percent":9}}})+"\\n__AI_QUOTA_HTTP__"+code)\n')
        curl.chmod(0o755)
        env=dict(os.environ,PATH=str(commands)+':/usr/bin:/bin');env.pop('OPENCODE_API_KEY',None)
        out=subprocess.run(self.base+['refresh','--json'],capture_output=True,text=True,env=env,check=True)
        self.assertEqual(json.loads(out.stdout)['providers'][0]['quotas'][0]['remaining_percent'],91)
        for code,kind in [('401','authentication'),('429','rate_limited'),('network','network')]:
            # separate state avoids intentionally persisted backoff from previous case
            args=self.base+['--state-dir',str(self.path/code),'--cache',str(self.path/(code+'.json')),'refresh','--json']
            out=subprocess.run(args,capture_output=True,text=True,env=dict(env,FIXTURE_HTTP=code))
            self.assertEqual(out.returncode,1);self.assertEqual(json.loads(out.stdout)['providers'][0]['error']['kind'],kind)
            self.assertNotIn('fixture-key',out.stdout+out.stderr)
    def test_bash_prompt_preserved_and_cache_only(self):
        script='PS1="original> "; ai-quota(){ printf "fixture remaining"; }; source "$1"; first="$PS1"; source "$1"; [[ "$PS1" == "$first" ]] || exit 2; [[ "$PS1" == *"original> " ]] || exit 3; [[ "${PS1@P}" == *"fixture remaining"* ]] || exit 4'
        subprocess.run(['bash','--noprofile','--norc','-c',script,'test',str(root/'integrations/shell/bash.sh')],check=True,capture_output=True)

if __name__=='__main__':unittest.main(verbosity=2)
