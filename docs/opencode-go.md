# OpenCode Go and Console V2

The legacy monitor reads `OPENCODE_API_KEY`, or `opencode-go.key` / `apiKey`
from `$XDG_DATA_HOME/opencode/auth.json` (or `auth_file`). This is **not** the
Console session used by V2. Signing into V2 does not renew that legacy key.
The default `go_auth: "api_key"` preserves this adapter and its existing endpoint,
`https://opencode.ai/zen/go/v1/usage`. There is no automatic fallback between
credential types, and a Console token is never sent to the legacy endpoint.

## Console authentication

Console documents an OAuth device grant for other CLIs:
<https://opencode.ai/v2/docs/console/api/#device-flow>.
The monitor uses client ID `ai-quota-monitor` and `supports_org_scope=true`, so
the user selects their Go workspace during authorization. It never reads or
modifies V2's SQLite database, imports browser cookies, generates API keys,
enables balance/Extra Usage, or changes billing. No V1 binary is required.

The documented device flow does not offer a quota-only OAuth scope. The token
acts as the signed-in user within the selected workspace, so this is broader
than a dedicated read-only quota credential. The monitor only performs the
documented authorization/renewal POSTs and the quota GET, but the credential
itself must be protected accordingly. Do not create a service account or select
a paid plan as a workaround; neither is required by this implementation.

The monitor's OAuth session is independent of OpenCode. Tokens are saved only
in the monitor's private credential store, **not in configuration**, cache,
history, process arguments, or logs. This file is plaintext on disk with mode
0600 in a 0700 directory; use an encrypted home filesystem for protection at rest.
The default is
`$XDG_STATE_HOME/ai-quota/console-auth/<profile>/credentials.json`
(fallback `~/.local/state/...`). An optional `console_auth_file` can name a file
in a dedicated private directory. Do not point it at an OpenCode credential file
or a shared configuration directory.

Access tokens are refreshed 60 seconds before expiry. The monitor serializes
refreshes with a private lock and atomically saves rotated tokens with fsync.
**Refresh tokens are single use:** before making a refresh request it persists
a pending marker. Any uncertain outcome (network failure, crash, malformed reply,
or failed save) blocks further refresh attempts rather than replaying the token
and risking revocation. Reauthorize in that case. Workspace changes in a refresh
reply are rejected. HTTP 401 does not trigger an unbounded refresh/login loop.

## Quota endpoint and limitations

CodexBar reads `GET https://opencode.ai/console/api/go/status` using a Console
session **cookie** plus `x-org-id`. Its implementation does not establish OAuth
compatibility by itself. A real diagnostic on **2026-10-20** confirmed HTTP 200
with a monitor-owned, workspace-bound OAuth access token, **without cookies or
`x-org-id`**. The response included all three meters and their explicit
`resetsAt` timestamps. The diagnostic kept tokens in memory only and did not
write quota history/cache or send notifications. That session was discarded.

This is still an **undocumented quota endpoint**, distinct from the documented
device authorization endpoints. Compatibility was verified for the selected Go
workspace, not guaranteed for every account or future server version. Use the
diagnostic check below before enabling the adapter in a daemon.

The Console adapter sends only its own workspace-bound bearer token, without
cookies or a workspace override. It does not request billing/balance endpoints.
If Console rejects this authentication or changes its contract, the monitor
reports an error, retains previous successful quota data, and marks it stale.
Do not try to fix this by enabling paid usage or generating another key.
The safe fallback is the Console Go dashboard while requesting an official,
read-only OAuth quota API from OpenCode. There is no scraping fallback.

Only `access.meters.fiveHour`, `week`, and `month` are parsed. Percentage used is
`100 * usedMicroCents / limitMicroCents`; remaining is `100 - used`. Micro-cent
meters describe subscription allowances, **not prepaid credits or spend**.
Invalid or out-of-range meters fail validation, not silently clamped.
Missing windows are omitted, never rendered as 100% remaining. Missing resets
remain null. Only a meter's explicit RFC 3339 `resetsAt` is used; `access.endsAt`
is not assumed to be a monthly meter reset. No fixed 30-day month or per-model
allowance is inferred. Existing `last_success_at`, errors, aging, history,
alerts and deduplication continue through the common scheduler unchanged.

## Staged use (does not install or restart anything)

Build in this checkout, then use a **separate configuration file**. The example
`config/go-console.example.json` enables the existing Codex adapter and selects
`console_oauth` for Go. Copy your existing provider settings into the staged
file rather than replacing your active configuration wholesale.

```sh
cargo build --locked
target/debug/ai-quota --config /path/to/staged-config.json go-login --profile default
target/debug/ai-quota --config /path/to/staged-config.json check --provider opencode-go --json
```

`go-login` is explicit and interactive: approve its displayed URL in your
browser. It does not query inference or billing. It refuses to replace an
existing credential file. To replace an expired/revoked or uncertain session,
stop concurrent monitor authentication operations and move **only the monitor's
own** credential file to a private backup, then run `go-login` again. Do not
move, copy, or reset OpenCode's credentials.

`check` is read-only: no SQLite writes, cache writes, notifications, credential
writes, lock creation, or refresh-token use. If the access token needs renewal,
it reports this instead of renewing it. Existing scheduler error backoff is
respected; an isolated `--cache /path/to/nonexistent-staged-cache.json` avoids
consulting an active cache's backoff without modifying it. `--state-dir` can also
point to a separate staged directory. A normal `refresh` or daemon poll may
renew the monitor's own credential and persist results.

For multiple Go profiles, use a separate credential file per profile and select
one with `go-login --profile NAME`. Authorization binds each file to the
workspace chosen in the browser. `doctor` checks local prerequisites without
network requests or renewal. Changing `--state-dir` affects history, not the
default credential location; use `console_auth_file` for an entirely separate
credential store.

## Updating an installed service without losing history

Do this manually **after** successful staged diagnostics; development does not
restart the active service or run the installer.

1. Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
   `cargo test --locked`, and `bash scripts/check.sh` in the checkout.
2. Build with `cargo build --release --locked`. Keep the old installed binary
   and configuration for rollback. Confirm the installed service's `ExecStart`
   and paths with `systemctl --user cat ai-quota-monitor.service`.
3. Stop the monitor service yourself for the maintenance window. Back up its
   **complete state directory**, including SQLite/WAL files, and its config
   and cache. Keep any monitor-owned credential backup private. Do not remove
   history or run migration again. The current notification policy adds SQLite
   tables idempotently on database open; keep a private backup and see
   [notifications](notifications.md).
4. Replace only the monitor binary at the path already used by the unit and
   merge `"go_auth": "console_oauth"` into the Go profile. Keep Codex settings,
   profiles, state/cache paths, GNOME presentation settings, plugin, unit and
   notification configuration intact.
5. Start the service yourself. Inspect `status --json` for fresh Go data and
   `last_success_at`, then inspect `history`. A failure must retain the last
   successful data as stale. Rollback restores the prior binary and config;
   it does not require deleting or replacing SQLite history.

## Sources

- Console authentication and single-use refresh tokens:
  <https://opencode.ai/v2/docs/console/api/>
- Go subscription versus optional balance usage:
  <https://opencode.ai/v2/docs/console/go/>
- V2 local HTTP API (no documented credential-export or quota-broker route):
  <https://opencode.ai/v2/docs/api/>
- CodexBar Console request and meter parser:
  <https://github.com/steipete/CodexBar/blob/main/Sources/CodexBarCore/Providers/OpenCodeGo/OpenCodeGoUsageFetcher.swift>

The documentation and CodexBar source were inspected on 2026-10-20. The
unauthenticated Console Go status endpoint returned HTTP 401; the explicitly
authorized OAuth diagnostic returned HTTP 200 as described above. Live token
expiry/rotation was not forced; renewal and uncertain-outcome handling were
validated with synthetic tokens and offline transports.

Validation **of the initial Console milestone (2026-10-20)**: `cargo fmt`,
Clippy with warnings denied, 42 Rust tests, 8 existing operational tests,
3 OAuth operational tests, Python Ruff, plugin typecheck/lint
and 5 plugin tests, plus GNOME presentation and lifecycle suites all passed.
The complete `scripts/check.sh` used the already available Node installation in
`target/tools/node-v24.15.0-linux-x64/bin` via a command-local PATH; no system
packages, active installation, or service were changed.

## Changing the Go authentication method and retry delays

The monitor records only the **authentication method name** (`api_key` or
`console_oauth`) alongside the durable provider status; it never stores secrets
there. When the configured method differs from the one last attempted, the next
daemon refresh may run immediately instead of honoring a backoff from the old
method. The new method starts with a fresh failure counter. On any subsequent
failure **with the same method**, normal backoff and retry limits apply. This
works in both directions and does not reset quotas, samples, notifications or
alert deduplication.

For pre-upgrade status records without an authentication marker, the monitor
infers the old method only if the last known successful source URL matches an
explicitly recognized API-key or Console endpoint. Otherwise it conservatively
preserves the existing backoff until the next scheduled attempt. A new method is
recorded after that attempt, including if it fails. This avoids repeated retry
bypasses after service restarts.
