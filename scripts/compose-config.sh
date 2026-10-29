#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
export SQLX_OFFLINE=true
exec cargo run --locked --quiet -p tpl-accounts --bin compose-config -- "$@"
