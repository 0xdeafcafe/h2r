# crates/live

Everything about Live and playing with other people: the wire types and server, the Cloudflare Worker, the game-side HLE, co-op transport, bots, films, cloud saves, telemetry and the dedicated host's data. A live crate may use `formats`, `runtime`, `platform`, `hooking` and `rewrites`; `extensions`, `tools` and `apps` use it. Every Live feature runs locally and on Cloudflare. See [crate-structure.md](../../docs/dev/crate-structure.md) and [live.md](../../docs/live/live.md); the layering is checked by `scripts/crate-graph.py layers --check`.

| Crate | What it is |
|---|---|
| [`actor-net`](actor-net/) | Actor presentation packets, snapshot recovery and per-client bandwidth scheduling. |
| [`bots`](bots/) | Multiplayer bots (Clankers): the brain and phantom peers. |
| [`cloud-saves`](cloud-saves/) | Cloud saves: merge at sign-in, push as the game writes. |
| [`coop`](coop/) | Co-op over Live: the peer transport, pad schedule and tick hash exchange. |
| [`film`](film/) | Seekable engine films and the local film library. |
| [`host-data`](host-data/) | The dedicated host's game data bundle: build, fetch, verify, cache. |
| [`live-hle`](live-hle/) | Xbox Live client HLE for the game. |
| [`live-proto`](live-proto/) | Wire types, signing and verification shared by clients and server. |
| [`live-server`](live-server/) | The Live replacement service on axum, SQLite or Postgres. |
| [`live-telemetry`](live-telemetry/) | Guest-state telemetry and live ingest. |
| [`live-worker`](live-worker/) | The Live server as a Cloudflare Worker. |
| [`sim-bench`](sim-bench/) | Sim mode's pure parts: match files, clock, metrics, A/B comparison. |
