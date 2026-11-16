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
| `bun test` | unit tests (reducer, interpolation, replay, reconnect, signatures, derived stats, sign-up JWS, player models and shots) |
| `bun run typecheck` | `tsc --noEmit`, strict |

Mock knobs: `H2_MOCK_HISTORY_MIN` (minutes of simulated history at boot,
default 20), `H2_MOCK_TAMPER=<n>` (corrupt every nth event's data so the
"bad signature" state shows up), `H2_MOCK_STATIC=<dir>` (also serve a built
app with SPA fallback, the way the Rust server will:
`bun run build && H2_MOCK_STATIC=$PWD/dist bun run mock`),
`H2_MOCK_GAME_ART=1` (serve a stand-in `/art/game` manifest so the game-art
path runs; see "Game art" below). Without it `/art/game` has no pictures,
like a server with no game art. The mock's own map geometry (below) is served
either way; `H2_MOCK_MAP_GEOMETRY=0` turns that off too, and then `/art/game`
answers 404. `H2_MOCK_SESSIONS=<n>` sets how many open sessions the mock keeps
for the dashboard's "find a game" (default 5; 0 shows its empty state).
`H2_MOCK_MODELS=1` adds stand-in player and weapon models (block figures,
`mock/models.ts`) to the art manifest, so the 3D view's model path runs;
without it players are capsules, as on a server with no models.

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

**Find a game.** The dashboard's "find a game" (`/dash/sessions`,
`src/dash/Sessions.tsx`) lists the open sessions hosts are advertising
(`GET /v1/sessions`): the host, the map, the game and players against the
most, with a plate of details, and one session's own screen with its roster
at `/dash/sessions/:id`. Both ask the server again every four seconds while
the tab is showing (`src/state/sessions.ts`). It is read-only: joining
happens in the game.

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
`h2 live account import <file>`.

**Friends and teams** come from the server (docs/live-api.md "Friends" and
"Teams"). The dashboard's friends and teams screens (`src/dash/Social.tsx`),
the stats site's team directory and team pages (`/stats/teams`,
`/stats/teams/:id`) and the Teams and Friends boxes on service records all
read them and follow the stream, so they change as events arrive
(`src/state/social.ts`). Writes are signed `h2live-event` JWS bodies sent to
`POST /v1/events` with an `events.write` token, the same way `h2 live friend`
and `h2 live team` send them (`src/state/sign.ts`). A team's combined stats
are derived from games where two or more members played on the same side.
The mock (`mock/social.ts`) seeds three teams and some friendships. A new web
account gets a friend request and a team invitation a few seconds after
signing up, and seeded players accept whatever you send them.

**Fonts.** All bundled fonts are under the SIL Open Font Licence, each with
its licence file beside it: Michroma (`src/assets/fonts`, the "h2 live" name
on the dashboard), and Orbitron and Barlow Condensed (`src/dash/fonts`, the
dashboard's headings and its condensed labels). The stats site draws its
wordmark as SVG (`stats/Wordmark.tsx`) and otherwise
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
  real emblem, layers and colours are picked from the XUID (`emblemChoice`),
  the colours from the manifest's `palette` when it has one;
  `<Emblem emblem={...}>` takes real values (`emblemFromIndices` turns
  profile colour indices into them).
- **Ranks**: our experience ladder is spread over the game's levels
  (`levelForExp`, then `rank/<1..50>`).
- **Map previews** (`map/<map>/preview`) replace the top-down thumbnail in
  game cards, Recent Games, history rows and preview, and the game page's
  header; a live game keeps its blips, drawn over the picture.
- **Game-type icons** go by the words of the variant name (`gametypeKey`:
  slayer, ctf, assault, oddball, king of the hill, juggernaut, territories).
- **Medals** (`medal/<alias>`): the derived medals that mean the same as
  one of the game's use its picture (`medalKey`: twofer, hat-trick,
  full-house, encore are the multi-kills; hot-streak, wildfire, untouchable
  the sprees). Opening Act, Last Word and Payback have no counterpart and
  keep the pack's.
- **Weapons** (`weapon/<alias>`, by the words of the reported name,
  `weaponKey`) are single masks, used as a CSS mask so they take the page's
  tint like the SVG silhouettes, which every other weapon keeps.

The mock's stand-in (`mock/gameart.ts`) uses the pack's rank badges and
medals and draws its own emblem masks, weapon masks, game-type glyphs and
map postcards in the real encoding, so nothing from the game is needed to
run or test this path. To look at a real server's art over the mock's
world, point only that route at it: `H2_GAME_ART_URL=http://127.0.0.1:8074`.

**Map geometry and the 3D view (optional, never shipped).** The same
extraction can write, per map, a `map.glb`, a `views.json` and pre-drawn
pictures of the level (top-down and four iso corners), named in the art
manifest's `maps` table. When a map has them, the game page's Tactical Map
lays players over the picture using the view's `world_to_pixel` matrix
(`src/map/views.ts`, `src/map/ViewMap.tsx`), with a switcher for the views
and pan and zoom; without them it is the fitted grid as before. The "3D view"
button opens the level in a window (`src/viewer/`): three.js, loaded as its
own chunk only then, with a sun and soft shadows, ambient occlusion (turned
off by itself on a machine that can't keep up), orbit, top-down and
follow-a-player cameras, and a grid floor when the map has no geometry. The
window can be dragged, resized, maximised, or popped out to
`/stats/games/:id/3d`. The 2D map, the scrubber and the 3D window read one
replay session (`src/viewer/session.ts`), and a popped-out window keeps in
step with the page over a `BroadcastChannel`. `?view=3d` opens the window
and `?map=iso-ne` picks a view, so both can be linked to. The mock builds
two arenas of its own from its invented floor plans (`mock/arena.ts`:
slabs, ramps, pillars and cone trees, with a small rasteriser for the
pictures); its third map has none, so the fallbacks run too.

**Players, aim and shots.** Frames may say each player's `species`, `aim`
(a unit direction), held `weapon` and `shots` fired since their last frame;
all four are optional and anything off the contract is dropped
(`state/frames.ts`). When the art manifest has a `models` table
(`viewer/models.ts`), the 3D view draws each player as their species' glb
(a spartan for a species it has no model of) with the held weapon's glb
under its `weapon` node, in the roster's colour: the glb's `COLOR_0` gives
each vertex a primary weight (R, the player's colour) and a secondary one
(G, a darker shade), and a model without it is tinted all over. Each glb is
fetched once and cloned per player; team mates share the coloured mesh
(`viewer/players3d.ts`). Nothing animates: the posed model glides and turns
with the smoothed frames. It is life size up close and grows as the camera
backs away, up to the size capsules are drawn at. Without models (the mock
by default, an older extraction) players stay capsules. Either way a faint
ray from the head shows the aim (level along the yaw when the frame has
none), the label names the weapon, and each frame's shots draw a muzzle
flash and up to three tracers along the aim that fade in 150 ms
(`map/shots.ts`), from the weapon's `muzzle` node or from chest height. The
2D maps draw a short aim line from each dot and a fading line for each
shot. With reduced motion nothing flashes: the aim ray turns bright while a
player is firing instead. Recorded frames are one a second, so a replay
shows a second's shots as one burst.

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
the shared pieces, `stats.css` and `dash.css` the two sides. The dashboard's
shell (tunnel, orb, menu, prompts, transitions) is in `dash-shell.css`; the
orb, pods, bars and prompts are drawn in CSS and SVG, not images.

**Mock.** `mock/` is a Bun server implementing the read and write
endpoints, both WebSockets, discovery and real Ed25519 signing (server key
plus one key per account, batch JWS for kill+score pairs, hash chain). A
small simulation signs people in and out and runs 1 to 3 concurrent games on
three made-up maps (one without an outline) with players walking between
waypoints, kills, scores, endings and new games.
