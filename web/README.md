# h2 live web app

The web front end for h2recomp's Xbox Live replacement (`crates/live-server`).
It shows everyone who has signed up, who is online, the games they are
playing as they happen (live top-down map, scoreboard, kill feed), replays of
finished games, and the raw event log with signature checks. It is built
against the contract in [docs/live-api.md](../docs/live-api.md) and
[docs/live-security.md](../docs/live-security.md).

Stack: TypeScript, SolidJS, @solidjs/router, Vite, plain CSS. Bun is the
package manager and test runner.

## Running

```sh
cd web
bun install
bun run dev        # mock server on :8075 + Vite on http://localhost:5174
```

Vite proxies `/v1` (including the WebSockets), `/healthz` and
`/.well-known` to the mock. Other scripts:

| script | does |
|---|---|
| `bun run mock` | the mock server alone (`H2_MOCK_PORT`, default 8075) |
| `bun run build` | production build into `web/dist` |
| `bun test` | unit tests (reducer, interpolation, replay, reconnect, signatures) |
| `bun run typecheck` | `tsc --noEmit`, strict |

Mock knobs: `H2_MOCK_HISTORY_MIN` (minutes of simulated history at boot,
default 20), `H2_MOCK_TAMPER=<n>` (corrupt every nth event's data so the
"bad signature" state shows up), `H2_MOCK_STATIC=<dir>` (also serve a built
app with SPA fallback, the way the Rust server will:
`bun run build && H2_MOCK_STATIC=$PWD/dist bun run mock`).

**Against the real server.** `h2 live serve` serves `web/dist` at `/`, so
build and open the server's URL. To use Vite's dev server with a real
server instead of the mock, run `H2_LIVE_URL=http://127.0.0.1:8074 bunx vite`.
You can also point any running copy of the app at any server with the
server picker in the top right, or with a deep link: `?server=https://live.example.net`.
The choice and recent servers are kept in localStorage.

## Architecture

**Snapshot, then one stream.** `src/state/live.ts` discovers the server
(`/.well-known/h2live.json`, resolving a relative `api`), loads REST
snapshots (`state/snapshot.ts`: accounts, presence, live and recent games
with their details, and the last few hundred log events for the event page),
then opens a single `/v1/stream?from=<position>&exclude=GameFrame` WebSocket
(`StreamClient` in `state/stream.ts`) for the whole app. Every read reports
the log position it was taken at; the stream starts after the oldest one,
and `state/reducer.ts` is written so the overlap is harmless (a known game
isn't restarted, kills are deduplicated by position, scores are absolute).
If the server reports no positions, the client replays the whole log from 0
instead. The reducer is pure (`applyEvent`, plus `apply*Snapshot`) and runs
inside Solid's `produce` and on plain objects in tests. On a dropped
connection the client reconnects with exponential backoff and jitter and
resumes after the last applied position (`from` is exclusive), so pages
update without polling. Games outside the snapshot (deep links, older
games on a profile) are fetched on demand.

**Frames are not reactive.** Live frames (up to 15 Hz per game) go into
`FrameHub`/`FrameBuffer` (`state/frames.ts`), plain objects that the canvases
read directly. Each buffer keeps a smoothed wall-clock-to-tick mapping and
renders about 5 ticks behind the newest frame, interpolating positions and
yaw (short way round) between the two frames around the render tick and
snapping on respawns. All canvases share one `requestAnimationFrame` loop
(`map/scheduler.ts`), skip drawing when off screen, and the draw code
(`map/render.ts`) uses the uploaded map outline when there is one, or an
auto-fitted grid from the bounds seen so far.

**Replays** fetch `/v1/games/{id}/frames` (1 Hz, following cursors) and the game stream's
events from `/v1/events?stream=game-<id>`; `ReplayTimeline` samples
positions, kills and the scoreboard at any tick and `ReplayClock` handles
play, pause, speed and seek (`state/replay.ts`).

**Signatures.** Every event is queued to a `Verifier` (`state/verify.ts`)
that checks the compact JWS with WebCrypto Ed25519 in batches, yielding
between batches, and checks the per-stream hash chain where it has
consecutive versions. Keys come from `/.well-known/h2live.json` (server) and
`/v1/accounts/by-xuid/{xuid}/keys` (accounts, valid between log positions), cached by `kid`. Results land in
a separate store keyed by position, so rendering never waits on crypto.

**Styling.** `src/styles/theme.css` holds every colour, font, radius and
shadow as custom properties (the map renderer reads its colours from them
too); `src/styles/app.css` only consumes tokens. Restyle by replacing the
theme.

**Mock.** `mock/` is a Bun server implementing the read and write
endpoints, both WebSockets, discovery and real Ed25519 signing (server key
plus one key per account, batch JWS for kill+score pairs, hash chain). A
small simulation signs people in and out and runs 1 to 3 concurrent games on
three made-up maps (one without an outline) with players walking between
waypoints, kills, scores, endings and new games.
