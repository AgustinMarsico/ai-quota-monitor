#!/usr/bin/env bash
set -euo pipefail
ROOT="$(dirname "$(dirname "$(realpath "$0")")")"
cargo fmt --manifest-path "$ROOT/Cargo.toml" --check
cargo clippy --manifest-path "$ROOT/Cargo.toml" --locked --all-targets -- -D warnings
env -u OPENCODE_API_KEY cargo test --manifest-path "$ROOT/Cargo.toml" --locked
python3 "$ROOT/tests/operations.py"
npm --prefix "$ROOT/integrations/opencode" run typecheck
npm --prefix "$ROOT/integrations/opencode" run lint
npm --prefix "$ROOT/integrations/opencode" test
node "$ROOT/tests/gnome.mjs"
