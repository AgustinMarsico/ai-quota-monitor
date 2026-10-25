#!/usr/bin/env bash
set -euo pipefail
ROOT="$(dirname "$(dirname "$(realpath "$0")")")"
cargo fmt --manifest-path "$ROOT/Cargo.toml" --check
cargo clippy --manifest-path "$ROOT/Cargo.toml" --locked --all-targets -- -D warnings
env -u OPENCODE_API_KEY cargo test --manifest-path "$ROOT/Cargo.toml" --locked
python3 "$ROOT/tests/operations.py"
python3 "$ROOT/tests/oauth_operations.py"
RUFF="${RUFF:-$ROOT/target/test-env/bin/ruff}"
"$RUFF" format --check "$ROOT/scripts" "$ROOT/tests"
"$RUFF" check --cache-dir "$ROOT/target/ruff-cache" "$ROOT/scripts" "$ROOT/tests"
npm --prefix "$ROOT/integrations/opencode" run typecheck
npm --prefix "$ROOT/integrations/opencode" run lint
npm --prefix "$ROOT/integrations/opencode" test
node "$ROOT/tests/gnome.mjs"
