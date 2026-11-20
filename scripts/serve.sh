#!/usr/bin/env bash
# Serves the example pages locally: the bus forwarder (the relay's
# /bus/<code> stand-in) on 0.0.0.0:9312 and this repository over HTTP on
# 0.0.0.0:8099.
#
# Open http://<this machine's LAN address>:8099/examples/hub.html, not
# localhost: the node links carry the page's host, and a phone on the same
# network joins through it. Run scripts/build.sh first.
set -euo pipefail
cd "$(dirname "$0")/.."
HTTP=${HTTP:-8099} BUS=${BUS:-9312}
FWD=${CARGO_TARGET_DIR:-target}/debug/bus-forwarder
[ -x "$FWD" ] && [ -d dist/pkg ] && [ -d dist/pkg-node ] || { echo "run scripts/build.sh first" >&2; exit 1; }
"$FWD" "0.0.0.0:$BUS" &
trap 'kill $!' EXIT
echo "hub  http://$(hostname -I 2>/dev/null | awk '{print $1}'):$HTTP/examples/hub.html"
python3 -m http.server "$HTTP" --bind 0.0.0.0
