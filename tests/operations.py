#!/usr/bin/python3
"""Offline operational acceptance: installation, daemon recovery and migration."""

import json
import os
import signal
import sqlite3
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

root = Path(__file__).resolve().parents[1]
binary = root / "target/debug/ai-quota"


class Operations(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(
            dir=root / "target", prefix="operations-"
        )
        self.path = Path(self.temp.name)
        self.config = self.path / "config.json"
        self.config.write_text(
            json.dumps(
                {
                    "notifications": False,
                    "providers": [
                        {
                            "provider": "codex",
                            "codex_bin": str(root / "tests/fixtures/mock-codex.py"),
                        }
                    ],
                }
            )
        )
        self.base = [
            str(binary),
            "--config",
            str(self.config),
            "--state-dir",
            str(self.path / "state"),
            "--cache",
            str(self.path / "cache/status.json"),
        ]

    def tearDown(self):
        self.temp.cleanup()

    def call(self, *args):
        return subprocess.run(
            self.base + list(args), capture_output=True, text=True, check=True
        )

    def test_daemon_restart_and_single_instance(self):
        child = subprocess.Popen(
            self.base + ["daemon"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE
        )
        try:
            deadline = time.monotonic() + 5
            while (
                not (self.path / "cache/status.json").exists()
                and time.monotonic() < deadline
            ):
                time.sleep(0.05)
            self.assertTrue((self.path / "cache/status.json").exists())
            duplicate = subprocess.run(
                self.base + ["daemon"],
                capture_output=True,
                text=True,
                timeout=3,
                check=False,
            )
            self.assertNotEqual(duplicate.returncode, 0)
            self.assertIn("already running", duplicate.stderr)
            child.send_signal(signal.SIGTERM)
            self.assertEqual(child.wait(timeout=4), 0)
            child.stderr.close()
            sample = json.loads(self.call("status", "--json").stdout)
            before = sample["providers"][0]["last_success_at"]
            (self.path / "cache/status.json").unlink()
            child = subprocess.Popen(
                self.base + ["daemon"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
            )
            deadline = time.monotonic() + 5
            while (
                not (self.path / "cache/status.json").exists()
                and time.monotonic() < deadline
            ):
                time.sleep(0.05)
            sample = json.loads(self.call("status", "--json").stdout)
            self.assertEqual(sample["providers"][0]["last_success_at"], before)
            child.kill()
            child.wait(timeout=3)
            self.call("refresh")  # kernel releases locks after forced termination
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            if child.stderr:
                child.stderr.close()

    def test_install_uninstall_staging(self):
        prefix = self.path / "prefix"
        unit = self.path / "units"
        config = self.path / "config"
        subprocess.run(
            [
                "python3",
                str(root / "scripts/install.py"),
                "--prefix",
                str(prefix),
                "--unit-dir",
                str(unit),
                "--config-dir",
                str(config),
                "--gnome-dir",
                str(self.path / "gnome"),
                "--binary",
                str(binary),
            ],
            check=True,
            capture_output=True,
        )
        self.assertTrue((prefix / "bin/ai-quota").exists())
        self.assertTrue((self.path / "gnome/ai-quota@local/view.mjs").exists())
        self.assertTrue((self.path / "gnome/ai-quota@local/presentation.mjs").exists())
        self.assertTrue(
            (self.path / "gnome/ai-quota@local/schemas/gschemas.compiled").exists()
        )
        subprocess.run(
            [
                "systemd-analyze",
                "--user",
                "verify",
                str(unit / "ai-quota-monitor.service"),
            ],
            check=True,
            capture_output=True,
        )
        custom = config / "config.json"
        original = custom.read_bytes()
        custom.write_bytes(original)
        subprocess.run(
            ["python3", str(root / "scripts/uninstall.py"), "--prefix", str(prefix)],
            check=True,
            capture_output=True,
        )
        self.assertFalse((prefix / "bin/ai-quota").exists())
        self.assertEqual(custom.read_bytes(), original)

    def test_gnome_only_install_staging(self):
        destination = self.path / "gnome-only/ai-quota@local"
        sentinel = self.path / "monitor-state"
        sentinel.write_text("existing monitor state")
        existing = {
            path.relative_to(self.path): path.read_bytes()
            for path in self.path.rglob("*")
            if path.is_file()
        }
        command = [
            "python3",
            str(root / "scripts/install_gnome.py"),
            "--destination",
            str(destination),
        ]
        subprocess.run(command, check=True, capture_output=True)
        files = {
            path.relative_to(self.path)
            for path in self.path.rglob("*")
            if path.is_file()
        }
        self.assertTrue((destination / "schemas/gschemas.compiled").is_file())
        self.assertEqual(
            (destination / "view.mjs").read_bytes(),
            (root / "integrations/shared/view.mjs").read_bytes(),
        )
        self.assertTrue(
            all(
                path.is_relative_to("gnome-only/ai-quota@local")
                for path in files - existing.keys()
            )
        )
        subprocess.run(command, check=True, capture_output=True)
        for path, contents in existing.items():
            self.assertEqual((self.path / path).read_bytes(), contents)

    def test_legacy_wal_import_idempotent_and_read_only(self):
        source = self.path / "legacy"
        source.mkdir()
        database = source / "usage.sqlite3"
        legacy = sqlite3.connect(database)
        legacy.execute("PRAGMA journal_mode=WAL")
        legacy.execute(
            "CREATE TABLE samples(id INTEGER PRIMARY KEY,sampled_at INTEGER,bucket_id TEXT,window_kind TEXT,duration_minutes INTEGER,resets_at INTEGER,used_percent REAL,remaining_percent REAL)"
        )
        legacy.execute(
            'INSERT INTO samples VALUES(1,?,"codex","primary",300,0,22,78)',
            (int(time.time()),),
        )
        legacy.commit()
        before = {p.name: p.read_bytes() for p in source.iterdir()}
        self.call("migrate", "--provider", "codex", "--from", str(source))
        self.call("migrate", "--provider", "codex", "--from", str(source))
        self.assertEqual(before, {p.name: p.read_bytes() for p in source.iterdir()})
        db = sqlite3.connect(self.path / "state/usage.sqlite3")
        self.assertEqual(db.execute("SELECT count(*) FROM samples").fetchone()[0], 1)
        db.close()
        legacy.close()
        self.assertIn("78", self.call("export").stdout)
        imported = json.loads(self.call("status", "--json").stdout)
        self.assertEqual(imported["providers"][0]["state"], "stale")

    def test_go_transport_with_fake_curl_and_no_network(self):
        auth = self.path / "auth.json"
        auth.write_text('{"opencode-go":{"key":"fixture-key"}}')
        self.config.write_text(
            json.dumps(
                {
                    "notifications": False,
                    "providers": [{"provider": "opencode-go", "auth_file": str(auth)}],
                }
            )
        )
        commands = self.path / "bin"
        commands.mkdir()
        curl = commands / "curl"
        curl.write_text(
            '#!/usr/bin/python3\nimport os,sys,json,time\nassert "fixture-key" not in " ".join(sys.argv)\nassert "Authorization: Bearer fixture-key" in sys.stdin.read()\ncode=os.environ.get("FIXTURE_HTTP","200")\nif code=="network":sys.exit(7)\nif code=="200":time.sleep(2.1)\nprint("HTTP/2 "+code+"\\r\\nRetry-After: 600\\r\\n\\r\\n"+json.dumps({"usage":{"rolling":{"percent":9,"resetInSec":600}}})+"\\n__AI_QUOTA_HTTP__"+code)\n'
        )
        curl.chmod(0o755)
        env = dict(os.environ, PATH=str(commands) + ":/usr/bin:/bin")
        env.pop("OPENCODE_API_KEY", None)
        out = subprocess.run(
            self.base + ["refresh", "--json"],
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        self.assertEqual(
            json.loads(out.stdout)["providers"][0]["quotas"][0]["remaining_percent"], 91
        )
        reading = json.loads(out.stdout)["providers"][0]
        self.assertGreaterEqual(
            reading["quotas"][0]["resets_at"] - reading["last_success_at"], 599
        )
        for code, kind in [
            ("401", "authentication"),
            ("429", "rate_limited"),
            ("network", "network"),
        ]:
            # separate state avoids intentionally persisted backoff from previous case
            args = self.base + [
                "--state-dir",
                str(self.path / code),
                "--cache",
                str(self.path / (code + ".json")),
                "refresh",
                "--json",
            ]
            out = subprocess.run(
                args,
                capture_output=True,
                text=True,
                env=dict(env, FIXTURE_HTTP=code),
                check=False,
            )
            self.assertEqual(out.returncode, 1)
            self.assertEqual(
                json.loads(out.stdout)["providers"][0]["error"]["kind"], kind
            )
            self.assertNotIn("fixture-key", out.stdout + out.stderr)

    def test_check_no_persistence_and_stored_backoff(self):
        before = {
            p.relative_to(self.path): p.read_bytes()
            for p in self.path.rglob("*")
            if p.is_file()
        }
        sample = json.loads(self.call("check", "--json").stdout)
        self.assertEqual(sample["providers"][0]["quotas"][0]["remaining_percent"], 78)
        self.assertEqual(
            before,
            {
                p.relative_to(self.path): p.read_bytes()
                for p in self.path.rglob("*")
                if p.is_file()
            },
        )
        self.call("refresh")
        cache = self.path / "cache/status.json"
        snapshot = json.loads(cache.read_text())
        snapshot["providers"][0].update(
            {
                "state": "stale",
                "error": {
                    "kind": "rate_limited",
                    "message": "fixture cooldown",
                    "retry_after_seconds": 600,
                },
                "next_attempt_at": int(time.time()) + 600,
            }
        )
        cache.write_text(json.dumps(snapshot))
        original = cache.read_bytes()
        out = subprocess.run(
            self.base + ["check", "--json"], capture_output=True, text=True, check=False
        )
        self.assertEqual(out.returncode, 1)
        self.assertEqual(cache.read_bytes(), original)
        self.assertEqual(
            json.loads(out.stdout)["providers"][0]["error"]["kind"], "rate_limited"
        )

    def test_notifications_urgent_bounded_and_deduplicated(self):
        home = self.path / "fixture-home"
        home.mkdir()
        (home / ".fixture-mode").write_text("low")
        self.config.write_text(
            json.dumps(
                {
                    "notifications": True,
                    "providers": [
                        {
                            "provider": "codex",
                            "codex_bin": str(root / "tests/fixtures/mock-codex.py"),
                            "codex_home": str(home),
                        }
                    ],
                }
            )
        )
        commands = self.path / "bin"
        commands.mkdir()
        notification = commands / "notify-send"
        record = self.path / "notification.json"
        notification.write_text(
            f'#!/usr/bin/python3\nimport json,os,sys,time\nfrom pathlib import Path\nPath({str(record)!r}).write_text(json.dumps({{"args":sys.argv,"pid":os.getpid()}}))\ntime.sleep(60 if os.environ.get("FIXTURE_NOTIFY_HANG")=="1" else 0)\n'
        )
        notification.chmod(0o755)
        env = dict(os.environ, PATH=str(commands) + ":/usr/bin:/bin")
        subprocess.run(
            self.base + ["refresh"], env=env, capture_output=True, check=True
        )
        message = json.loads(record.read_text())
        self.assertEqual(message["args"][message["args"].index("-u") + 1], "critical")
        record.unlink()
        subprocess.run(
            self.base + ["refresh"], env=env, capture_output=True, check=True
        )
        self.assertFalse(record.exists())
        start = time.monotonic()
        subprocess.run(
            self.base
            + [
                "--state-dir",
                str(self.path / "timeout-state"),
                "--cache",
                str(self.path / "timeout-cache.json"),
                "refresh",
            ],
            env=dict(env, FIXTURE_NOTIFY_HANG="1"),
            capture_output=True,
            check=True,
            timeout=5,
        )
        self.assertLess(time.monotonic() - start, 5)
        message = json.loads(record.read_text())
        self.assertFalse(Path(f"/proc/{message['pid']}/exe").exists())

    def test_ntfy_transport_stdin_secrets_and_http_failure_is_isolated(self):
        home = self.path / "ntfy-fixture-home"
        home.mkdir()
        (home / ".fixture-mode").write_text("low")
        token = self.path / "ntfy-token"
        token.write_text("fixture-secret-token\n")
        token.chmod(0o600)
        record = self.path / "curl-record.json"
        commands = self.path / "ntfy-bin"
        commands.mkdir()
        curl = commands / "curl"
        curl.write_text(
            f'#!/usr/bin/python3\nimport json,os,sys,time\nfrom pathlib import Path\nsettings=sys.stdin.read()\nPath({str(record)!r}).write_text(json.dumps({{"argv":sys.argv,"settings":settings}}))\ntime.sleep(float(os.environ.get("FIXTURE_NTFY_SLEEP","0")))\nprint(os.environ.get("FIXTURE_NTFY_STATUS","200"),end="")\n'
        )
        curl.chmod(0o755)
        self.config.write_text(
            json.dumps(
                {
                    "notifications": True,
                    "notification_transports": {
                        "desktop": False,
                        "ntfy": {
                            "enabled": True,
                            "server_url": "https://ntfy.sh",
                            "topic": "fixture_topic",
                            "token_file": str(token),
                        },
                    },
                    "providers": [
                        {
                            "provider": "codex",
                            "codex_bin": str(root / "tests/fixtures/mock-codex.py"),
                            "codex_home": str(home),
                        }
                    ],
                }
            )
        )
        env = dict(os.environ, PATH=str(commands) + ":/usr/bin:/bin")
        env.pop("OPENCODE_API_KEY", None)
        out = subprocess.run(
            self.base + ["refresh"],
            env=env,
            capture_output=True,
            text=True,
            check=True,
            timeout=15,
        )
        sent = json.loads(record.read_text())
        self.assertEqual(
            sent["argv"][1:],
            ["-q", "--silent", "--no-location", "--proto", "=https", "--config", "-"],
        )
        self.assertNotIn("fixture-secret-token", " ".join(sent["argv"]))
        self.assertIn("fixture-secret-token", sent["settings"])
        self.assertIn(r"\"topic\":\"fixture_topic\"", sent["settings"])
        self.assertIn(r"\"title\":\"codex quota alert\"", sent["settings"])
        self.assertIn(r"\"priority\":5", sent["settings"])
        self.assertIn("rotating_light", sent["settings"])
        self.assertIn("Consumed: 96.0%", sent["settings"])
        self.assertIn("Available: 4.0%", sent["settings"])
        self.assertIn("Reset: not reported", sent["settings"])
        self.assertNotIn("fixture-secret-token", out.stdout + out.stderr)
        cache = self.path / "cache/status.json"
        self.assertNotIn("fixture-secret-token", cache.read_text())

        # A non-success HTTP status is a delivery failure, not a quota-query failure.
        args = self.base + [
            "--state-dir",
            str(self.path / "http-fail-state"),
            "--cache",
            str(self.path / "http-fail-cache.json"),
            "refresh",
        ]
        failed_delivery = subprocess.run(
            args,
            env=dict(env, FIXTURE_NTFY_STATUS="503"),
            capture_output=True,
            text=True,
            check=True,
            timeout=15,
        )
        self.assertIn("96.0% used", failed_delivery.stdout)
        self.assertNotIn("query failed", failed_delivery.stderr.lower())

        # While delivery is held in the fake transport, another writer must be
        # able to acquire writer.lock and complete a quota refresh.
        record.unlink()
        lock_args = self.base + [
            "--state-dir",
            str(self.path / "lock-state"),
            "--cache",
            str(self.path / "lock-cache.json"),
            "refresh",
        ]
        slow = subprocess.Popen(
            lock_args,
            env=dict(env, FIXTURE_NTFY_SLEEP="5"),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        deadline = time.monotonic() + 5
        while not record.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertTrue(record.exists(), "fake transport did not start")
        concurrent = subprocess.run(
            lock_args,
            env=env,
            capture_output=True,
            text=True,
            timeout=8,
            check=True,
        )
        self.assertIn("96.0% used", concurrent.stdout)
        slow_out, slow_err = slow.communicate(timeout=15)
        self.assertEqual(slow.returncode, 0, slow_out + slow_err)

    def test_notifications_disabled_does_not_start_transport_or_read_token(self):
        home = self.path / "disabled-home"
        home.mkdir()
        (home / ".fixture-mode").write_text("low")
        commands = self.path / "disabled-bin"
        commands.mkdir()
        record = self.path / "must-not-run"
        curl = commands / "curl"
        curl.write_text(
            f"#!/usr/bin/python3\nfrom pathlib import Path\nPath({str(record)!r}).touch()\n"
        )
        curl.chmod(0o755)
        self.config.write_text(
            json.dumps(
                {
                    "notifications": False,
                    "notification_transports": {
                        "desktop": True,
                        "ntfy": {
                            "enabled": True,
                            "server_url": "https://ntfy.sh",
                            "topic": "fixture_topic",
                            "token_file": str(self.path / "nonexistent-token"),
                        },
                    },
                    "providers": [
                        {
                            "provider": "codex",
                            "codex_bin": str(root / "tests/fixtures/mock-codex.py"),
                            "codex_home": str(home),
                        }
                    ],
                }
            )
        )
        out = subprocess.run(
            self.base + ["refresh"],
            env=dict(os.environ, PATH=str(commands) + ":/usr/bin:/bin"),
            capture_output=True,
            text=True,
            check=True,
            timeout=8,
        )
        self.assertIn("96.0% used", out.stdout)
        self.assertFalse(record.exists())

    def test_notifications_test_uses_configured_transports_without_monitor_state(self):
        token = self.path / "notification-token"
        token.write_text("diagnostic-fixture-secret\n")
        token.chmod(0o600)
        record = self.path / "curl-test.json"
        commands = self.path / "test-bin"
        commands.mkdir()
        curl = commands / "curl"
        curl.write_text(
            f'#!/usr/bin/python3\nimport json,sys\nfrom pathlib import Path\nsettings=sys.stdin.read()\nPath({str(record)!r}).write_text(json.dumps({{"argv":sys.argv,"settings":settings}}))\nprint("200\\n{{}}",end="")\n'
        )
        curl.chmod(0o755)
        self.config.write_text(
            json.dumps(
                {
                    "notifications": True,
                    "notification_transports": {
                        "desktop": False,
                        "ntfy": {
                            "enabled": True,
                            "server_url": "https://ntfy.sh",
                            "topic": "diagnostic_topic",
                            "token_file": str(token),
                        },
                    },
                    "providers": [],
                }
            )
        )
        result = subprocess.run(
            self.base + ["notifications", "test"],
            env=dict(os.environ, PATH=str(commands) + ":/usr/bin:/bin"),
            capture_output=True,
            text=True,
            timeout=15,
            check=True,
        )
        self.assertIn("transport=desktop status=not_enabled", result.stdout)
        self.assertIn("transport=ntfy status=accepted", result.stdout)
        self.assertNotIn("diagnostic_topic", result.stdout + result.stderr)
        self.assertNotIn("diagnostic-fixture-secret", result.stdout + result.stderr)
        captured = json.loads(record.read_text())
        self.assertNotIn("diagnostic-fixture-secret", " ".join(captured["argv"]))
        self.assertIn(r"\"title\":\"AI Quota Monitor — Test\"", captured["settings"])
        self.assertIn(
            "Notificación de prueba del transporte configurado.", captured["settings"]
        )
        self.assertFalse((self.path / "state").exists())
        self.assertFalse((self.path / "cache/status.json").exists())
        help_result = subprocess.run(
            self.base + ["--help"], capture_output=True, text=True, check=True
        )
        self.assertIn("notifications test", help_result.stdout)

    def test_bash_prompt_preserved_and_cache_only(self):
        script = 'PS1="original> "; ai-quota(){ printf "fixture remaining"; }; source "$1"; first="$PS1"; source "$1"; [[ "$PS1" == "$first" ]] || exit 2; [[ "$PS1" == *"original> " ]] || exit 3; [[ "${PS1@P}" == *"fixture remaining"* ]] || exit 4'
        subprocess.run(
            [
                "bash",
                "--noprofile",
                "--norc",
                "-c",
                script,
                "test",
                str(root / "integrations/shell/bash.sh"),
            ],
            check=True,
            capture_output=True,
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
