# Design

How the live demo works and why. Section numbers are referenced from the
code.

## 1. The contract

Sensor data changes all the time: firmware updates rename fields and change
units, newer devices add fields, and a fleet runs several versions at once.
In a typical pipeline nothing on the wire says which version wrote a value,
so these changes rarely fail. They turn into a NULL, a misread value or a gap
nobody sees.

With AimDB, every value carries its contract's name, version and shape, and
the receiver checks it. A known older version is migrated on admission; an
unknown one is rejected with a reason; a change outside the contract fails
the build.

The demo has one contract, `power_reading`, deliberately without any
industry:

| Version | Field | Where |
|---|---|---|
| @1 | `power_w`, whole watts | `contracts/r1`, kept in `contracts/r2` |
| @2 | `power_mw`, milliwatts; migrated from @1 by `power_w × 1000` | `contracts/r2` |
| @3 | adds `string_id` | `contracts/exhibits/v3`, newer than the hub |

Each revision crate is immutable once merged and has an `aimdb.lock`. A test
checks that revision 2 keeps revision 1's shape. Every shape hash a page
shows comes from these locks: the Rust code reads them through
`ContractShape::SHAPE`, and every bundle release carries them as
`contracts.json` for pages elsewhere, generated from the locks while the
release is packed (`tools/contracts-json`), so it cannot drift from them.

## 2. Nodes and firmwares

A node is one sender of `power_reading`: a visitor's phone, the on-page node
on the computer, or a simulated gateway. It can be flashed with any of five
firmwares:

| Firmware | On the wire | The hub's verdict |
|---|---|---|
| `v1` | `power_reading@1`, watts | admitted, migrated to @2 |
| `v2` | `power_reading@2`, milliwatts | admitted as current |
| `v3` | `power_reading@3`, adds `string_id` | rejected, `too_new` |
| `unbumped` | declared @1, milliwatt shape | rejected, `shape_mismatch` |
| `none` | plain JSON, no envelope | rejected, `malformed` |

`unbumped` is the runtime side of a build check: firmware that changes the
unit but still says @1 fails its test against revision 1's lock
(`contracts/exhibits/unbumped`). If a build skips that test anyway, the hub
still refuses it, because the declared shape is not @1's.

A node page sends one reading every second from the moment it connects, and
can pause. Flashing builds a new AimDB instance
and stops the old one: the old runner is aborted, or its tasks would stay
pending until the page closes. The phone page uses its own small bundle
(`node-web`): the node and the `/bus` client, without the hub.

## 3. The hub and its verdicts

The hub runs in the computer page. It has one route per place, each a record
of `power_reading@2` that admits versions 1 and 2, with its own admission
counters. The routes are separate records outside any application logic, so
the page reads only admission counters and the latest admitted value.

**Every verdict is the hub's own.** Admission updates a route's counters
synchronously inside `InboundDispatch::dispatch`, so the page's bus observer
reads each frame's outcome from its route's counters right after delivery.
Nothing is inferred from the frame. A test sends 30 frames from six senders,
as interleaved as one page can make them, and every frame gets its own
route's verdict.

The page shows a frame's label (contract, version, shape, the power field)
by decoding it with the shape it declares. That is display only; the verdict
always comes from the counters.

## 4. Places and the relay

Visitors' nodes reach the hub through a relay route, `/bus/<code>`. The relay
forwards frames and never parses a payload.

- **Frames.** One binary WebSocket message is one frame:
  `u8 topic length | topic | payload`.
- **Places.** The hub joins as `hq`; a node joins as place 1 to 8, taken from
  the link it was opened with (on aimdb.dev, a QR code). One member per place; a second one is
  refused with close code 4009 ("This place is taken. Scan the code on the
  screen again."). aimdb.dev's page shows one QR code at a time, for the next
  free place.
- **A node speaks only for its own place.** The relay does not say who sent
  a frame, so the topic is the sender's identity: a frame from place n is
  forwarded only when its topic starts with `n/` (`may_send`).
- **The hub reaches one place or all.** A frame from `hq` goes to the place
  its topic starts with (`3/feedback`), or to every place when its topic
  names none (`mode`) (`hq_target`).
- **The hub listens to its own bus.** Nothing published inside the computer
  page goes up the link, so the simulated fleet and the on-page node never
  reach a phone. The hub sends only its mode, to all, and each node's
  feedback, to that node.

Each place has its own hub route, `live://<n>/power`, and so its own
counters; the on-page node and every simulated gateway have one too, so every
verdict belongs to exactly one sender.

`bus-forwarder` applies the same rules locally, without the relay's limits.

## 5. The simulated fleet

A hub with one visitor looks empty, so a hub page can run simulated
gateways. The hub has a route for each of twelve names (`LiveHub.fleet()`),
and `hub.node(name)` gives a local node that sends through the same route and
the same admission as a visitor's node. Its frames never leave the page.

aimdb.dev's page runs 3 to 12 of them, one joining or leaving every 5
minutes, each sending every 2 to 6 seconds, mostly on @1 or @2, now and then
a single message as @3 or without a contract. Simulated gateways are always
labelled `sim` and never presented as other visitors.

## 6. Contracts off

The computer page has a switch that turns contracts off for the whole demo,
to show the visitors' own values without them.

- The hub sends its mode to every node (`mode`, `on` or `off`), at once on a
  flip and every 2 seconds for nodes that joined since.
- With contracts off, a node sends what an untyped device sends: its
  firmware's struct as plain JSON, field names and all, with no contract,
  version or shape (`node::untyped_json`, tested against each struct). The
  forgotten bump looks exactly like @2: without a version there is nothing to
  forget.
- These raw frames go on `<n>/raw`, which no hub route reads, so the
  admission counters keep counting only checked frames. The hub reads each
  raw message with three ordinary receivers (`baseline`): a dashboard written
  for the old firmware reads `power_w`; an analytics job told about the
  change by e-mail reads `power_w`, else `power_mw / 1000`; a lenient data
  lake import takes the first field named `power*` as watts. Nothing is
  rejected.
- Every phone is told what the hub made of each reading, on `<n>/feedback`:
  the verdict and admitted value with contracts on, the three readings with
  contracts off. The answer carries the reading's `seq`, so the phone holds
  it against the right value. A phone on @2 that showed "Admitted, 326 W" now
  shows 0 W, 326 W and 326 000 W; a phone on @1 shows 326 W three times,
  because where nothing changed the untyped pipeline works.
- On a flip every node resends its last reading in the new mode, so the
  phones change at once.

The baseline is labelled wherever it shows. It is not AimDB, and the page
never presents it as an admission result.

## 7. The node graph

`hub.view().graph` describes every node that has sent, for a page to draw:
its firmware as the hub reads it from the frame (`@1`, `@2`, `@3`,
`unbumped`, `none`; with contracts off the hub cannot tell, and says
`JSON`), the hub's last verdict, whether the three receivers disagreed, when
it was last heard, and its own route's counters. aimdb.dev's page draws it as
a star around the hub, fades nodes that fall silent, and shows one node in
detail when it is selected.

Only contracts are shown. No power charts or plant views: an application in
the foreground pulls attention away from what the demo is about.

## 8. Evidence

| Shown | Test |
|---|---|
| Every firmware gets its verdict; the admitted value; shapes from the locks | `web/tests/node.rs` `every_firmware_gets_its_own_verdict` |
| Six senders interleaved, every frame gets its own route's verdict; the graph's per-node firmware, verdict and counters | `web/tests/live.rs` `every_frame_gets_its_own_verdict_while_the_fleet_sends` |
| A send right after a flash arrives; flashing ends the old instance | `web/tests/live.rs` |
| Contracts off: everything passes, three receivers read it, the counters do not move | `web/tests/live.rs` `with_contracts_off_every_value_passes_and_the_receivers_guess`; `graphs/src/baseline.rs` |
| The same JSON, three receivers, three numbers; with the contract, one | `graphs/tests/without_contracts.rs` |
| A shape change without a version bump fails the build | `contracts/exhibits/unbumped`, test `by_the_r1_lock` |
| A place cannot send as another; the hub reaches one place or all | `protocol` unit tests; `tests/e2e.py` |
| The hub page and real node browsers over a socket: every firmware's verdict, the answer to the node, a taken place refused, the switch and the answers it brings | `tests/e2e.py` |
| The shape hashes published for other pages come from the locks | generated from them at release time, `tools/contracts-json` |
| The contract crates build for `thumbv7em-none-eabihf`, the host and `wasm32` | the commands in the README |

## 9. Limits

- **Per-frame verdicts rely on synchronous admission.** The page reads each
  frame's verdict from counters right after delivery. That holds while
  admission stays synchronous inside `dispatch`, which is how AimDB works
  today, not a documented guarantee. A per-message admission hook would
  remove the reliance.
- **Stopping an instance.** AimDB cancels an instance when its
  `runner.run()` future is dropped; the demo keeps each runner abortable and
  aborts it on a flash. A stop handle on the runner is requested in
  aimdb-dev/aimdb#495.
- **Leaving nodes.** The hub learns about a node from its first frame and
  never learns that it left. The view reports when each node was last
  heard, so a page can fade silent nodes; aimdb.dev's QR code moves only
  forward. Enough for one meeting with 8 places.
- **Mode on join.** A phone that joins while contracts are off learns the
  mode within 2 seconds.
- **The node bundle** still carries aimdb-core's `remote` feature, because
  `aimdb-wasm-adapter` enables it.
