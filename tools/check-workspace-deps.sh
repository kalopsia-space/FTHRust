#!/usr/bin/env bash
# Every entry of [workspace.dependencies] in Cargo.toml must be used by a crate
# (`name.workspace = true` or `name = { workspace = true, ... }`). An entry that
# nothing uses still shows up in Cargo.lock's feature resolution and in what
# `cargo update` and Dependabot touch (eframe and tracing-appender sat there
# unused until the 2026-10-31 audit).
#
#   tools/check-workspace-deps.sh [ROOT]     # ROOT defaults to the repository
set -euo pipefail
ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$ROOT"
mapfile -t names < <(sed -n '/^\[workspace.dependencies\]/,/^\[/p' Cargo.toml | grep -E '^[A-Za-z0-9_-]+ *=' | sed 's/ *=.*//')
[ ${#names[@]} -gt 0 ] || { echo "check-workspace-deps: no [workspace.dependencies] in $ROOT/Cargo.toml" >&2; exit 1; }
unused=()
for d in "${names[@]}"; do
    grep -qE "^$d(\.workspace *= *true| *= *\{[^}]*workspace *= *true)" crates/*/Cargo.toml || unused+=("$d")
done
if [ ${#unused[@]} -gt 0 ]; then
    echo "workspace dependencies no crate uses: ${unused[*]}" >&2
    exit 1
fi
echo "workspace dependencies all used (${#names[@]})"
