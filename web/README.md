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
| `bun test` | unit tests (reducer, interpolation, replay, reconnect, signatures, derived stats, sign-up JWS) |
| `bun run typecheck` | `tsc --noEmit`, strict |

Mock knobs: `H2_MOCK_HISTORY_MIN` (minutes of simulated history at boot,
default 20), `H2_MOCK_TAMPER=<n>` (corrupt every nth event's data so the
"bad signature" state shows up), `H2_MOCK_STATIC=<dir>` (also serve a built
app with SPA fallback, the way the Rust server will:
`bun run build && H2_MOCK_STATIC=$PWD/dist bun run mock`),
`H2_MOCK_GAME_ART=1` (serve a stand-in `/art/game` manifest so the game-art
path runs; see "Game art" below). Without it `/art/game` answers 404, like a
server with no game art.

**Against the real server.** `h2 live serve` serves `web/dist` at `/`, so
build and open the server's URL. To use Vite's dev server with a real
server instead of the mock, run `H2_LIVE_URL=http://127.0.0.1:8074 bunx vite`.
You can also point any running copy of the app at any server from the
dashboard's "server" item, or with a deep link: `?server=https://live.example.net`.
The choice and recent servers are kept in localStorage.

## Two sides

**The dashboard** (`/`, `/dash/:section`, `/dash/players/:tag`, `src/dash/`) is the "live" side:
a glowing orb, a wireframe tunnel and an arc of menu bars (live now,
friends, players, game history, sign up / profile, server), each opening a
bezelled list panel. Keyboard: arrows move, Enter (or `a`) selects,
Escape / Backspace / ArrowLeft (or `b`) goes back; the mouse works too.

**The stats site** (`/stats/...`, `src/stats/`) is the record, laid out like
a mid-2000s game stats site: a fixed 902px column on a navy page, a header
with the Welcome box, a black nav strip, breadcrumbs, a banner strip, then a
290px menu column (pill menu, People Finder, reports, Recent Games) beside
the content. Pages: "h2 live Online" (`/stats`: the featured live map and
the games spotlight), game details (`/stats/games/:id`: overview, the Game
Viewer with the live tactical map or the replay, and the post game carnage
report with Overview / Carnage / Medals / Weapons tabs), players and
leaderboards (`/stats/players`), a player's service record, game history
with its preview pane, and career stats with Tool of Destruction, the Medal
Chest and compare (`/stats/players/:tag`, `/history`,
`/career?compare=<tag>`), and the event log (`/stats/events`). The old
`/games/:id`, `/players/...` and `/events` paths redirect. The shell and
shared chrome are in `stats/Layout.tsx`, the menu-column pieces and map
thumbnails in `stats/Side.tsx`. Below 760px the column goes full width and
the menu column drops under the content.

**Derived stats.** Medals, sprees, places, the weapon breakdown ("Tool of
Destruction"), EXP and rank are computed in the browser from the games'
events (`src/state/derive.ts`); the API does not serve them. They are
marked with a `*` and a tooltip. They only cover games this browser has
loaded, and a game's snapshot carries at most its recent kills, so they are
estimates for long histories. The Tool of Destruction pairs each weapon's
"kills with" (gold bar) with "killed by" (blue bar). Medals (original names;
the badges are raster art from `public/art/medals`, named after the medal
ids): Opening Act (first kill of a game), Last Word
(last kill), Payback (kill your last killer within 10 s), Twofer / Hat
Trick / Full House / Encore (2 / 3 / 4 / 5+ kills, each within 4 s of the
last), Hot Streak / Wildfire / Untouchable (5 / 10 / 15 kills without
dying). EXP is games completed plus two per win.

**Sign up in the browser.** The dashboard's "sign up" makes a new Ed25519
key with WebCrypto, signs the same account request `h2 live account create`
sends (compact JWS, header `{alg: "EdDSA", kid, typ: "h2live-account"}`,
payload the canonical JSON of `{gamertag, jwk, invite?, aud, iat}`) and
POSTs it as `application/jose` to `/v1/accounts` (`src/state/account.ts`).
`aud` is the server's id from its well-known document and `iat` the time in
seconds; the server refuses a request for another server or more than five
minutes off its clock, and so does the mock (`mock/account.ts`). The private
key is kept as a JWK (its `d`) in this browser's IndexedDB (`h2live` /
`accounts`), so anyone with access to the browser profile can read it; the
UI says so. "Download for h2" saves an `accounts.json`-style file for
`h2 live account import <file>`. "Friends" are the people you have played
with.

**Fonts.** All bundled fonts are under the SIL Open Font Licence, each with
its licence file beside it: Michroma (`src/assets/fonts`, the "h2 live" name
on both sides), and Orbitron and Barlow Condensed (`src/dash/fonts`, the
dashboard's headings and its condensed labels). The stats site otherwise
uses system fonts (Arial for copy and headings, Verdana for the nav strip).

**Artwork.** Raster artwork (banner strips, page and map textures, 24 player
emblems, medals, rank badges) is original and lives in `public/art`,
documented in `public/art/ART.md`; `src/lib/art.ts` maps emblems, medal ids
and rank names to its files. Weapon silhouettes are small SVGs in
`components/WeaponIcon.tsx`. No third-party logos or artwork are in this
repo or in the build.

**Game art (optional, never shipped).** A server whose owner ran
`h2 live assets extract` on their own disc serves that art at
`/art/game/manifest.json` (docs/live-api.md "Optional game art"), by default
only to a browser on the same machine. The app asks once at startup and on a
server change (`src/state/gameart.ts`); a 404 or anything unreadable means
"use the art pack", and a miss is remembered for a minute. With a manifest,
any picture whose key it has replaces the pack's (`src/lib/gameart.ts`):

- **Emblems** (`emblem/fg/<n>`, `emblem/bg/<n>`) are two-mask layers, not
  colour pictures. `compositeEmblem` colours them per the manifest's rules
  (`rgb = colour_a * red + colour_b * blue`; the background is opaque, the
  foreground's coverage is `max(red, blue)`, PNG alpha is ignored) on a
  canvas, cached per layers and colours. Until telemetry carries a player's
  real emblem, layers and colours are picked from the XUID (`emblemChoice`);
  `<Emblem emblem={{fg, bg, primary, ...}}>` takes real values.
- **Ranks**: our experience ladder is spread over the game's levels
  (`levelForExp`, then `rank/<1..50>`).
- **Map previews** (`map/<map>/preview`) replace the top-down thumbnail in
  game cards, Recent Games, history rows and preview, and the game page's
  header; a live game keeps its blips, drawn over the picture.
- **Game-type icons** go by the words of the variant name (`gametypeKey`:
  slayer, ctf, assault, oddball, king of the hill, juggernaut, territories).
- Weapons stay as the SVG silhouettes (the manifest's `weapon/<n>` are not
  yet identified), and medals as the pack's (the disc has none).

The mock's stand-in (`mock/gameart.ts`) uses the pack's rank badges and
draws its own emblem masks, game-type glyphs and map postcards in the real
encoding, so nothing from the game is needed to run or test this path.

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
auto-fitted grid from the bounds seen so far. A map without an outline
answers 404 by design; the miss is remembered for five minutes
(`state/outlines.ts`) so pages don't keep asking. On the stats site the
canvas is transparent over a tiled map texture, and finished games get a
still thumbnail of where their knockouts fell.

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

**Styling.** `src/styles/theme.css` holds the tokens: `--s-*` for the
stats site, `--d-*` for the dashboard, and the map palette (the renderer
reads its colours from them; `.dash` swaps in a green map). `app.css` has
the shared pieces, `stats.css` and `dash.css` the two sides.

**Mock.** `mock/` is a Bun server implementing the read and write
endpoints, both WebSockets, discovery and real Ed25519 signing (server key
plus one key per account, batch JWS for kill+score pairs, hash chain). A
small simulation signs people in and out and runs 1 to 3 concurrent games on
three made-up maps (one without an outline) with players walking between
waypoints, kills, scores, endings and new games.
