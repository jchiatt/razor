#!/usr/bin/env bash
# Build this checkout and install it; --link also selects it.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo 'Commit changes before installing.' >&2; exit 2; }
case "${1:-}" in '') select=0;; --link) select=1;; *) echo 'Usage: scripts/install.sh [--link]' >&2; exit 2;; esac
version=$(git rev-parse --short=12 HEAD)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cargo build --release --locked --quiet
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
RAZOR_BINARY="$target/release/razor" scripts/release.sh "$version" "$stage" "file://$stage"
RAZOR_SELECT=$select sh "$stage/install.sh"
