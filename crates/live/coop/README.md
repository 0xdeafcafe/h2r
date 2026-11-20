# coop

Co-op over Live, phase 0: what two machines need to run the same game in
step. Design and status: [docs/live/live-coop.md](../../../docs/live/live-coop.md)
(section 8 says what is built and what is left).

| module | what |
|---|---|
| `wire` | the one datagram: acknowledgements, plus every pad frame and tick hash not yet acknowledged |
| `link` | `Link` (an unreliable datagram path), `UdpLink` (direct, XNet base port + 4), `Fallback` (direct, with the relay while direct is silent), `sim_pair` (a simulated link on a fake clock) |
| `relay` | `RelayLink`: the Live server's `GET /v1/sessions/{id}/relay` as a `Link` |
| `schedule` | the two-port pad schedule with per-machine input delay |
| `peer` | one machine's end: `pump`, `begin_frame`, `report_hash`, `desync` |
| `tap` | `ScheduleTap`: the schedule as an `xapi::tap::InputTap` |

Nothing in it reads a clock or a device itself, and nothing in it knows the
guest. Nothing calls it from `boot`: Co-op over Live as built uses the retail
game's own synchronous campaign path
([live-coop.md](../../../docs/live/live-coop.md) section 10), and this crate is
the host-level lockstep alternative kept in reserve. `live-server` uses it for
the relay's datagram limit and its tests.

```sh
cargo test -p coop                       # schedule, links, peers over a simulated link, two toy guests
cargo test -p live-server --test relay   # the relay, and two peers through it
```
