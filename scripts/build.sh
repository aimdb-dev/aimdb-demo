#!/usr/bin/env bash
# Builds both bundles into dist/pkg (the hub's page) and dist/pkg-node (the
# node page), and the local bus forwarder. dist/ is what a release ships (scripts/package.sh).
#
# Needs wasm-bindgen-cli matching the wasm-bindgen version in Cargo.lock
# (WASM_BINDGEN=<path> to choose one); runs wasm-opt when it is on PATH.
set -euo pipefail
cd "$(dirname "$0")/.."
WASM_BINDGEN=${WASM_BINDGEN:-wasm-bindgen}
want=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/version = "\(.*\)"/\1/p')
have=$("$WASM_BINDGEN" --version | awk '{print $2}')
if [ "$want" != "$have" ]; then
  echo "wasm-bindgen $have, Cargo.lock wants $want: cargo install wasm-bindgen-cli --version $want" >&2
  exit 1
fi
T=${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release
# Built one at a time: features are not unified across the two bundles, so
# the node bundle stays without the hub.
cargo build --release --target wasm32-unknown-unknown -p live-web
cargo build --release --target wasm32-unknown-unknown -p node-web
cargo build -p line-protocol --features forwarder
mkdir -p dist
for b in live_web:pkg node_web:pkg-node; do
  name=${b%%:*} out=dist/${b##*:}
  rm -rf "$out"
  "$WASM_BINDGEN" --target web --remove-name-section --remove-producers-section \
    --out-dir "$out" "$T/$name.wasm"
  if command -v wasm-opt >/dev/null; then
    wasm-opt -Os --enable-bulk-memory --enable-nontrapping-float-to-int \
      "$out/${name}_bg.wasm" -o "$out/${name}_bg.wasm"
  fi
  raw=$(stat -c %s "$out/${name}_bg.wasm")
  gz=$(gzip -9c "$out/${name}_bg.wasm" | wc -c)
  printf '%-9s %8d bytes, %7d gzip -9\n' "$name" "$raw" "$gz"
done
