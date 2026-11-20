#!/usr/bin/env bash
# Packs dist/ (scripts/build.sh) as the release artifact that aimdb.dev's
# demo pages are served with:
#
#   aimdb-demo-bundles-<version>.tar.gz
#     pkg/            live_web.js, live_web_bg.wasm, .d.ts: LiveHub, LocalNode (the hub's page)
#     pkg-node/       node_web.js, node_web_bg.wasm, .d.ts: Node (a phone)
#     contracts.json  shape hashes from the aimdb.lock files, generated here (tools/contracts-json)
#     VERSION, LICENSE
#   aimdb-demo-bundles-<version>.tar.gz.sha256
#
# usage: scripts/package.sh <version>      e.g. scripts/package.sh v0.1.0
set -euo pipefail
cd "$(dirname "$0")/.."
version=${1:?usage: scripts/package.sh <version>}
[ -f dist/pkg/live_web_bg.wasm ] && [ -f dist/pkg-node/node_web_bg.wasm ] || { echo "run scripts/build.sh first" >&2; exit 1; }
name=aimdb-demo-bundles-$version
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
cp -r dist/pkg dist/pkg-node LICENSE "$stage/$name/"
cargo run -q --locked -p contracts-json > "$stage/$name/contracts.json"
echo "$version" > "$stage/$name/VERSION"
# Reproducible: fixed order, owner and times.
tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner --mtime='2026-01-01' -czf "$name.tar.gz" "$name"
sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
cat "$name.tar.gz.sha256"
