#!/usr/bin/python3
"""Console OAuth transport acceptance tests. Synthetic credentials, no network."""

import json
import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/ai-quota"
FAKE_CURL = r"""#!/usr/bin/python3
import json, os, sys
from pathlib import Path
assert sys.argv[1] == "-q"
assert "--no-location" in sys.argv
assert sys.argv[sys.argv.index("--proto") + 1] == "=https"
assert not any("fixture-access" in arg or "fixture-refresh" in arg for arg in sys.argv)
settings = sys.stdin.read()
url = sys.argv[-1]
audit = Path(os.environ["FIXTURE_AUDIT"])
with audit.open("a") as out:
    out.write(url + "\n")
if url.endswith("/auth/device/token"):
    assert "grant_type=refresh_token" in settings
    assert "fixture-refresh" in settings
    if os.environ.get("FIXTURE_FAIL"):
        print('{"error":"invalid_grant"}\n__AI_QUOTA_HTTP__400', end="")
    else:
        print(json.dumps({"access_token":"rotated-access", "refresh_token":"rotated-refresh",
            "expires_in":3600,"org_id":"org_fixture"}) + "\n__AI_QUOTA_HTTP__200", end="")
elif url.endswith("/api/go/status"):
    assert "Authorization: Bearer " in settings
    assert "refresh" not in settings
    assert "--include" in sys.argv
    assert "Cookie" not in settings
    print('HTTP/2 200\r\n\r\n' + json.dumps({"access":{"meters":{
        "fiveHour":{"usedMicroCents":"25","limitMicroCents":"100"},
        "week":{"usedMicroCents":"30","limitMicroCents":"100"},
        "month":{"usedMicroCents":"40","limitMicroCents":"100"}
    }}}) + "\n__AI_QUOTA_HTTP__200", end="")
else:
    raise AssertionError("Unexpected endpoint")
"""


class ConsoleOAuthOperations(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / "target", prefix="oauth-")
        self.path = Path(self.temp.name)
        self.auth = self.path / "credentials/session.json"
        self.auth.parent.mkdir(mode=0o700)
        commands = self.path / "bin"
        commands.mkdir()
        curl = commands / "curl"
        curl.write_text(FAKE_CURL)
        curl.chmod(0o755)
        self.audit = self.path / "audit"
        self.env = dict(os.environ, PATH=str(commands), FIXTURE_AUDIT=str(self.audit))
        self.config = self.path / "config.json"
        self.config.write_text(
            json.dumps(
                {
                    "notifications": False,
                    "providers": [
                        {
                            "provider": "opencode-go",
                            "go_auth": "console_oauth",
                            "console_auth_file": str(self.auth),
                        }
                    ],
                }
            )
        )
        self.session(int(time.time()) + 3600)

    def tearDown(self):
        self.temp.cleanup()

    def session(self, expires):
        self.auth.write_text(
            json.dumps(
                {
                    "access_token": "fixture-access",
                    "refresh_token": "fixture-refresh",
                    "org_id": "org_fixture",
                    "expires_at": expires,
                }
            )
        )
        self.auth.chmod(0o600)

    def run_monitor(self, command, name="stage"):
        return subprocess.run(
            [
                str(BINARY),
                "--config",
                str(self.config),
                "--state-dir",
                str(self.path / (name + "-state")),
                "--cache",
                str(self.path / (name + "-cache.json")),
                command,
                "--provider",
                "opencode-go",
                "--json",
            ],
            env=self.env,
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )

    def test_diagnostic_does_not_persist_or_refresh(self):
        before = self.auth.read_bytes()
        result = self.run_monitor("check")
        self.assertEqual(result.returncode, 0, result.stderr)
        status = json.loads(result.stdout)["providers"][0]
        self.assertEqual(status["state"], "fresh")
        self.assertIsNotNone(status["last_success_at"])
        self.assertEqual(
            [q["remaining_percent"] for q in status["quotas"]], [75, 70, 60]
        )
        self.assertEqual(self.auth.read_bytes(), before)
        self.assertEqual(self.audit.read_text().count("/api/go/status"), 1)
        self.assertFalse((self.auth.parent / "session.lock").exists())
        self.assertFalse((self.path / "stage-state").exists())
        self.assertFalse((self.path / "stage-cache.json").exists())
        self.session(1000)
        result = self.run_monitor("check")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.audit.read_text().count("/api/go/status"), 1)

    def test_refresh_rotates_private_credentials(self):
        self.session(1000)
        result = self.run_monitor("refresh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(self.auth.read_text())["refresh_token"], "rotated-refresh"
        )
        self.assertEqual(self.auth.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.audit.read_text().count("/auth/device/token"), 1)
        self.assertNotIn("rotated-access", result.stdout + result.stderr)
        self.assertNotIn("rotated-refresh", result.stdout + result.stderr)

    def test_uncertain_refresh_cannot_be_replayed(self):
        self.session(1000)
        self.env["FIXTURE_FAIL"] = "1"
        result = self.run_monitor("refresh")
        self.assertEqual(result.returncode, 1)
        self.assertTrue(json.loads(self.auth.read_text())["refresh_pending"])
        result = self.run_monitor("refresh", "other")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.audit.read_text().count("/auth/device/token"), 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
