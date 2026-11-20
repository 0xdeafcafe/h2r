# h2 live web app

The web front end for h2recomp's Xbox Live replacement
(`crates/live/live-server`). It shows everyone who has signed up, who is
online, the games they are playing as they happen (live top-down map,
scoreboard, kill feed), replays of finished games, and the raw event log with
signature checks. It is built against the contract in
[live-api.md](../docs/live/live-api.md) and
[live-security.md](../docs/live/live-security.md).

Stack: TypeScript, SolidJS, @solidjs/router, Vite, plain CSS, three.js for the
3D game viewer. Bun is the package manager and test runner.

## Running

```sh
cd web
bun install
bun run dev        # mock server on :8075 + Vite on http://localhost:5174
```

Vite proxies `/v1` (including the WebSockets), `/healthz` and `/.well-known`
to the mock. To use Vite's dev server with a real server instead:
`H2_LIVE_URL=http://127.0.0.1:8074 bunx vite`. `h2 live serve` serves
`web/dist` at `/`, so build first and open the server's URL. You can also
point a running copy at any server from the dashboard's "server" item, or
with `?server=https://live.example.net`.

## Scripts

| script | does |
|---|---|
| `bun run dev` | the mock and Vite together |
| `bun run mock` | the mock server alone (`H2_MOCK_PORT`, default 8075) |
| `bun run vite` | Vite alone, for use against a real server |
| `bun run build` | production build into `web/dist` |
| `bun run preview` | serve the production build |
| `bun test` | unit tests (reducer, interpolation, replay, reconnect, signatures, derived stats, sign-up JWS, player models and shots) |
| `bun run typecheck` | `tsc --noEmit`, strict |
| `bun run banner` | draw the stats banners from extracted game art |
| `bun run brand` | draw the favicons, app icons and link-preview card into `public/` |
| `bun run bench:telemetry`, `bun run bench:viewer` | benchmarks |

## Layout

- `src/state/`: the live store, the stream client, the pure reducer, frames and replays, signature verification.
- `src/dash/`: the dashboard (the "live" side). `src/stats/`: the stats site. `src/map/`, `src/viewer/`: the 2D map and the 3D viewer.
- `src/lib/`, `src/styles/`, `src/assets/`: pure helpers, CSS tokens, fonts.
- `mock/`: the Bun mock server, with real Ed25519 signing and a simulated world.
- `public/art/`: original raster art ([ART.md](public/art/ART.md)).
- `banner/`, `brand/`, `bench/`, `test/`: generators, benchmarks and tests.

## Docs

- [web-app.md](../docs/live/web-app.md): the pages, mock server knobs (`H2_MOCK_*`), stats and derived numbers, sign-up, social features, link previews, and the architecture.
- [web-viewer.md](../docs/live/web-viewer.md): the optional game art and the 3D game viewer.
- [web-boundary.md](../docs/live/web-boundary.md): what goes on the dashboard and what on the stats site.
- [live-admin.md](../docs/live/live-admin.md): the Halo admin area.
- [live.md](../docs/live/live.md): the service this app talks to.
