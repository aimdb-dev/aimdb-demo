# aimdb-demo

The engine of the [AimDB](https://github.com/aimdb-dev/aimdb) data contracts
live demo that runs at [aimdb.dev/demo](https://aimdb.dev/demo/): the
contracts, the hub, the nodes and the bus, compiled to WASM, with example
pages and the tests behind every claim the demo makes.

A visitor's phone becomes a node of an AimDB system. It sends a power reading
every second, in any of five firmwares, to a hub that knows the
`power_reading` contract in versions 1 and 2. For each reading the hub says
what it made of it: admitted, migrated, or rejected with a reason. A switch
turns contracts off, and every node is told what three ordinary receivers read
from its value instead.

Everything is real AimDB: contracts, envelopes, admission and migration.
Every verdict is the hub's own admission result. This repository holds the
engine and plain example pages; aimdb.dev serves its own pages on the same
bundles, released from here.

How it works, and why it is built this way: [DESIGN.md](DESIGN.md).

| Path | What |
|---|---|
| `contracts/r1`, `contracts/r2` | `power_reading` revisions, each with its `aimdb.lock`: @1 in watts, @2 in milliwatts with a migration from @1. r2 keeps r1's shape (tested). Immutable once merged. |
| `contracts/exhibits/v3` | `power_reading@3`, adds `string_id`: firmware newer than the hub. |
| `contracts/exhibits/unbumped` | Milliwatts still declared @1. Refused by r1's lock (tested). |
| `protocol` | The `/bus/<code>` frame format and rules (`may_send`, `hq_target`, `valid_role`), and `bus-forwarder`, a local stand-in for the relay. |
| `page-connector` | The in-page bus with ports, and the `/bus` client (`BusLink`). |
| `graphs` | `live-graphs`: the node gateway (five firmwares, and their untyped JSON), the hub (one route per place) and the three baseline receivers (feature `hub`), and running and flashing nodes in a page (feature `wasm`). |
| `web` | `live-web`, the hub page's bundle: `LiveHub` and `LocalNode`. Browser tests. |
| `node-web` | The node page's bundle: `Node` only. No hub, no observability. |
| `examples` | `hub.html` and `node.html`: plain pages on the bundles. |
| `tests/e2e.py` | The hub page and node pages in separate browsers through the forwarder. |
| `tools/contracts-json` | Writes the shape hashes from the locks as `contracts.json`, for the release. |

AimDB is pinned by git revision. For work against a local checkout, put a
`[patch]` in `.cargo/config.toml` (ignored by git).

## Run it

```sh
scripts/build.sh     # both bundles into dist/, and the forwarder
scripts/serve.sh     # forwarder on :9312, this repository on :8099
```

Open `http://<LAN address>:8099/examples/hub.html` and follow a "node" link,
on the same computer or on a phone on the same network. Use the LAN address,
not `localhost`: the node links carry the page's host.

`build.sh` needs `wasm-bindgen-cli` at the version in `Cargo.lock`
(`cargo install wasm-bindgen-cli --version 0.2.130`; `WASM_BINDGEN=<path>`
picks one) and runs `wasm-opt` when it is on `PATH`.

Hub page options: `?local` runs without a bus (the on-page node only);
`?bus=ws(s)://<host>/bus` joins another bus; `?code=` fixes the code.

## Test it

```sh
cargo test --workspace --features live-graphs/hub      # locks, migration, exhibits, protocol, baseline
cargo check -p contracts-r2 -p exhibit-v3 -p exhibit-unbumped --target thumbv7em-none-eabihf
CHROMEDRIVER=chromedriver CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  cargo test --target wasm32-unknown-unknown -p live-web   # verdicts in a browser
scripts/build.sh && CHROMEDRIVER=chromedriver python3 tests/e2e.py
```

`wasm-bindgen-test-runner` comes with `wasm-bindgen-cli` and must match its
version. CI (`.github/workflows/ci.yml`) runs all of these.

## Releases

Pushing a tag `v*` publishes a GitHub release with
`aimdb-demo-bundles-<tag>.tar.gz` and its `.sha256`
(`.github/workflows/release.yml`):

```text
aimdb-demo-bundles-<tag>/
  pkg/            live_web.js, live_web_bg.wasm, .d.ts   LiveHub, LocalNode
  pkg-node/       node_web.js, node_web_bg.wasm, .d.ts   Node
  contracts.json  shape hashes from the locks, generated at release time (tools/contracts-json)
  VERSION, LICENSE
```

The pages import `pkg/live_web.js` or `pkg-node/node_web.js` as ES modules
and call `init()`; the API is documented at the top of `web/src/lib.rs` and
`node-web/src/lib.rs`. `scripts/package.sh <version>` builds the same tarball
locally; it is reproducible.

## License

Apache-2.0, see [LICENSE](LICENSE).
