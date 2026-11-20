# The Live web app

Status: Built. A SolidJS front end for the Xbox Live replacement, served by `h2 live serve` or the Worker.
Code: `web/` (`web/src/`, the dev mock in `web/mock/`). Tests: `web/test/`, run with `bun test`.
Switch: the `H2_MOCK_*` knobs below read only by the dev mock (`web/mock/`), so they are not in [switches.md](../runtime/switches.md).

The web front end for h2recomp's Xbox Live replacement (`crates/live/live-server`).
It shows everyone who has signed up, who is online, the games they are
playing as they happen (live top-down map, scoreboard, kill feed), replays of
finished games, and the raw event log with signature checks. It is built
against the contract in [live-api.md](live-api.md) and
[live-security.md](live-security.md). How to run it and its scripts are in
[web/README.md](../../web/README.md).

Stack: TypeScript, SolidJS, @solidjs/router, Vite, plain CSS. Bun is the
package manager and test runner.

## Running against a server

`bun run dev` starts the mock on :8075 and Vite on http://localhost:5174, and
Vite proxies `/v1` (including the WebSockets), `/healthz` and `/.well-known`
to the mock.

### Against the real server

`h2 live serve` serves `web/dist` at `/`, so
build and open the server's URL. To use Vite's dev server with a real
server instead of the mock, run `H2_LIVE_URL=http://127.0.0.1:8074 bunx vite`.
You can also point any running copy of the app at any server from the
dashboard's "server" item, or with a deep link: `?server=https://live.example.net`.
The choice and recent servers are kept in localStorage.

## Mock server

`mock/` is the dev server (`bun run mock`; the port is `H2_MOCK_PORT`, default 8075). `mock/` is a Bun server implementing the read and write
endpoints, both WebSockets, discovery and real Ed25519 signing (server key
plus one key per account, batch JWS for kill+score pairs, hash chain). A
small simulation signs people in and out and runs 1 to 3 concurrent games on
three made-up maps (one without an outline) with players walking between
waypoints, kills, scores, endings and new games.

### Knobs

- `H2_MOCK_HISTORY_MIN`: minutes of simulated history at boot, default 20.
- `H2_MOCK_TAMPER=<n>`: corrupt every nth event's data so the "bad signature" state shows up.
- `H2_MOCK_STATIC=<dir>`: also serve a built app with SPA fallback, as `h2 live serve` does: `bun run build && H2_MOCK_STATIC=$PWD/dist bun run mock`.
- `H2_MOCK_GAME_ART=1`: serve a stand-in `/art/game` manifest so the game-art path runs (see [web-viewer.md](web-viewer.md), "Game art"). Without it `/art/game` has no pictures, like a server with no game art.
- `H2_MOCK_MAP_GEOMETRY=0`: turn off the mock's own map geometry, which is otherwise served either way; `/art/game` then answers 404.
- `H2_MOCK_SESSIONS=<n>`: how many open sessions the mock keeps for the stats home's Open Games table (default 5; 0 hides the table).
- `H2_MOCK_QUIET=1`: end the games in progress after the history is seeded and start no more, so the stats home's Live Now panel shows the last game on a loop (with `H2_MOCK_HISTORY_MIN=0` too, no games at all: the map montage).
- `H2_MOCK_ACTORS=1`: put the mock campaign's AI and vehicles (`mock/campaign.ts`: about thirty actors, two vehicles in use and three parked empty, and a `bsp` zone that changes every 25 s, added by a frame hook) into every game's frames; without it only games whose mode is `campaign` get them.
- `H2_MOCK_INDOOR=1`: shut each arena's glb inside a busy building (an inward-facing shell, partition walls, an upper floor, pillars and two "machines", over two `bsp_<n>` zones and a `machines` node, plus a third zone marked cinematic-only and a `backdrop` of far hills), for trying the 3D view's cutaways and zones.
- `H2_MOCK_MODELS=1`: add stand-in player and weapon models (block figures, `mock/models.ts`) to the art manifest, so the 3D view's model path runs; the bipeds are skinned and carry a dozen clips (models version 2). `H2_MOCK_MODELS=static` makes them the static version 1 kind. Without it players are capsules, as on a server with no models.
- `H2_MOCK_ACTION=0`: leave out the stances, speeds, actions and hits that every game's frames otherwise carry (`mock/action.ts`: players jump, crouch, throw grenades, swing, reload, and hit who they fire at), like a sender from before they existed.
- `H2_MOCK_FURNITURE=0`: leave out each arena's furniture (`mock/fixtures.ts`): a lift platform that rides up and down and a walking thing with no node in the glb (both in the frames' `machines`), two pairs of teleporters with their links and a gravity lift (in the glb's extras).
- `H2_MOCK_ADMIN=1`: make every account signed up in the browser an admin, so the dashboard's server screen shows "your server" and its invites, accounts and import screens, and the stats site shows the Halo admin area (`mock/admin.ts`).
- `H2_MOCK_HOPPER=1`: serve a stand-in playlist file (invented playlists, plain numbers) so the Halo admin's playlist builder has something to edit; without it that tab says nothing has been imported yet.
- `H2_MOCK_CUSTOM_MAPS=0`, `H2_MOCK_STATS_ARCHIVE=0`, `H2_GAME_ART_URL`: described under "Custom maps" and "Server stats" below and "Game art" in [web-viewer.md](web-viewer.md).

## Two sides

### The dashboard

The dashboard (`/`, `/dash/:section`, `/dash/players/:tag`, `src/dash/`) is the "live" side:
a glass orb, a wireframe tunnel and an arc of menu blades. The selected blade
lights its connector. Recessed panels hold lists and details. Signed out the menu leads with sign
in, create and recover, then live now, games, players
and settings (the ones that need an account show greyed with a hint).
Signed in it is live now, games, friends, players,
account (recovery code, authenticator, devices, sign out) and settings;
profile and the server (`/dash/server`: service admin) sit under settings.
Server admins can open **Live admin** (`/dash/admin`) from the dashboard header
or Settings for usage, dedicated hosts, the raw event log and the style sample. On these
pages, the Back link, B, Escape and Backspace go to the previous page
when opened from the dashboard or another admin page. Direct visits fall
back to Settings or Server. The **Live admin** breadcrumb opens the hub.
In Usage, select a gamertag in **Top Accounts** to open its dashboard
profile (`/dash/players/:tag`).
Open **Games** (`/dash/games`) and select **Halo 2** to see recent games and,
when signed in, your service record. Its link to the stats site (named by
`STATS_NAME` in `src/brand.ts`) opens `/games/halo2`.
The Halo admin area (MOTD, playlists, custom content, Clanker Training) is on the stats site
at `/games/halo2/admin` ([docs/live/live-admin.md](live-admin.md)).

### Browser notifications

Signed-in players can open **Settings → notifications** to turn on alerts
for games with friends or games with other players. Both start off. The
browser stores the switches per server and account. Select **allow browser
notifications** on that screen to let the browser show alerts in the
background. A visible tab shows a dashboard toast regardless of browser
permission. Both alerts open the session detail, and clicking a browser
notification also asks the browser to focus the tab.

`web/src/state/notifications.ts` follows session changes on the app's live
stream. For those changes it reads the open sessions as the current account,
so endpoint and visibility checks match the server's listing. A host or
reported roster member can trigger a join alert while the game has a public
seat and its visibility allows the reader to join. Session advertisements
supply the lobby and its seats. Presence alone can also describe a solo
game or a game without a current advertisement. Initial and reconnect reads
establish a silent baseline. A tab alerts once per session and shows at most
one alert every eight seconds. The mock streams changes to its seeded sessions
so the settings and alerts work against the mock.

Direct-message alerts need a message model with recipient-only events, stable
message IDs and unread state. The current mailbox carries game invitations.
See [Game-invite mailboxes](live-api.md#game-invite-mailboxes) and the
[retail Live boundary](live-boundary.md).

#### Offline delivery (not built)

A service worker would subscribe through Web Push after the player opts in,
display incoming alerts and open or focus the appropriate page. The
`live-server` would add authenticated subscription registration and removal
per account and browser, retain the endpoint and encryption keys privately,
and check visibility and preferences when dispatching an event. Each server
identity would have a dedicated P-256 VAPID key pair. Discovery would publish
the public key, and the private key would live with the native server identity
or in a Worker secret. An event-driven sender would deduplicate deliveries,
retry transient failures and remove expired subscriptions. Native and
Cloudflare deployments would share the subscription API, dispatch rules and
parity tests, with storage and scheduled delivery behind their respective
bindings. [Live Worker architecture](live-worker.md)

### Clanker Training

Clanker Training is available there with an admin account
(`/games/halo2/admin?tab=training`). It lists opt-in
[`h2 sim --upload` recordings](../runtime/sim.md#watching-in-uplink)
and batches. Filter by map, tuning or batch, select two matches to compare
their summaries, or open a match for its report, bot stats and the shared
game viewer. Running streams follow the live edge. The **Live** control
returns to it after a seek. The training page shows the measured simulation
speed and the same viewer switches to replay at the end.
Keyboard: arrows move, Enter (or `a`) selects,
Escape / Backspace / ArrowLeft (or `b`) goes back; the mouse works too.

### The stats site

The stats site (`/games/halo2/...`, `src/stats/`; every link builds on
`STATS_ROOT` / `statsPath()` in `src/routes.ts`, and the old `/stats/...`
links redirect there) is the record, laid out like
a mid-2000s game stats site: a fixed 902px column on a navy page, a header
with the Welcome box, a black nav strip, breadcrumbs, a banner strip, then a
290px menu column (pill menu, People Finder, reports, Recent Games) beside
the content. The header's logo and the banner strip's mark are the
"h2 | UPLINK" wordmark (`stats/Wordmark.tsx`, original art), so the site
reads as `STATS_NAME` throughout. The nav's tabs start with the home,
"h2 Uplink"; the way back to the dashboard is the small "‹ Live Dashboard"
chip in the header's corner (`DASH_NAME`, as are the other links back to
it), not a tab, and every page's breadcrumbs start at "Home" (`/games/halo2`).
Pages: the home (`/games/halo2`, `stats/Home.tsx`: live now with the live games,
players online and open games; the last 24 hours and all-time totals;
leaderboards for today, this week, this month or all time (`?window=`):
most kills, best K/D with 3+ games, most wins, longest killing spree,
Legendary levels and time played; most played maps, most used weapons, an
activity heatmap by day and hour in your time, game types and playlists,
skull use, medals from loaded games (derived), the weapon and map of the
week, clan standings and recent games), games (`/games/halo2/games`: the featured
live map and the games spotlight), game details (`/games/halo2/games/:id`:
the match hero below the breadcrumbs, the Game Viewer with the live
tactical map or the replay, and the post game carnage report with Overview /
Carnage / Medals / Weapons tabs), players and
leaderboards (`/games/halo2/players`), a player's service record, game history
with its preview pane, and career stats with Tool of Destruction, the Medal
Chest and compare (`/games/halo2/players/:tag`, `/history`,
`/career?compare=<tag>`), and the event log (`/games/halo2/events`).
Campaign and multiplayer are kept apart: K/D, wins, medals, EXP,
leaderboards and clan records count multiplayer games only, and campaign
has its own record (`/games/halo2/players/:tag/campaign`: levels, completions,
time played, deaths, co-op partners), its own history tab
(`/history?tab=campaign`) and spotlight tab (`/games/halo2?games=campaign`), and
a mission report in place of the carnage report. A game's type is shown
as Custom Game, Matchmaking: <playlist>, Campaign, Campaign Co-op or
Campaign Online, with the difficulty (`src/state/gametype.ts`). The mock
starts an occasional campaign game (`mock/coop.ts`). The old
`/games/:id`, `/players/...` and `/events` paths redirect. The shell and
shared chrome are in `stats/Layout.tsx`, the menu-column pieces and map
thumbnails in `stats/Side.tsx`. Below 760px the column goes full width and
the menu column drops under the content.

### Open Games

The stats home's Open Games table (`/games/halo2`, `src/stats/Home.tsx`)
shows advertised sessions with the host, map, game type and player count.
It polls `GET /v1/sessions` every four seconds while the tab is showing
(`src/state/sessions.ts`). Join sessions from the game.

### Custom maps

Custom maps (docs/game/maps.md §5). A session's advertisement and a game
may carry a `map_id`, the map package played; `map` is then its base map.
The app looks each game's `map_id` up once per page load
(`GET /v1/maps/{id}`, `src/state/maps.ts`). Games
are titled by the package name, else the base map's; stats and filters
group by `map_id ?? map`, so a modded level is its own row ("<Base>
(modded)" until its name is known). The picture is the package's image
(`/v1/maps/{id}/image`, only when the summary has `image`), then the base
map's art. A package that says its geometry changed gets "geometry
changed, no 3D view" instead of the base map's 3D view. Admins pin
packages to playlists in the playlist builder (`GET /v1/hoppers/pins`,
`PUT /v1/hoppers/{id}/pins`) and moderate them under Admin, "Custom
content": packages and blobs with state and report counts, hide, delete,
block, reports, and the per-account upload switch (`src/admin/content.ts`,
a `content.moderate` token). A signed-in player can report a package from
its game's page. The mock (`mock/content.ts`) serves three invented
packages (one with a picture, one with changed geometry, one blocked) and
puts about a third of its sessions and multiplayer games on them or on an
id it doesn't know; `H2_MOCK_CUSTOM_MAPS=0` turns that off.

### Mods

The Mods tab (`/games/halo2/mods`, `web/src/stats/Mods.tsx`) reads the
pack catalog from `GET /v1/mod-packs`. One row shows the release, its
makers, uploader, version, year, online-play advisory, source, map count
and download size when available. Expand **View maps** to reach the
individual map pages. Newest and Most Downloaded, base map, category,
text and uploader filters apply to whole packs. The filters stay in the
URL, and each results page holds up to twenty packs.

A pack page (`/games/halo2/mods/packs/:id`) shows its cover, credits,
description, source and gallery. With no pack artwork it uses its members'
screenshots. A stock map preview is labelled **Stock map picture**.
Members keep their own URLs (`/games/halo2/mods/:id`), galleries,
contents and layer sizes. Signed-in players can report a pack or a
member. Its uploader can edit the release metadata and upload PNG, JPEG
or WebP screenshots to a cover and two additional slots on either page.
The Live server validates and stores the images. Installation runs in
the game. The mock serves invented pack metadata and member artwork.

### Server stats

The home's windowed numbers come from the server's
summary (`GET /v1/stats/summary?window=`, docs/live/live-api.md "Stats
summary"), a projection the server keeps in day and hour buckets; the page
asks again every 30 seconds (`src/state/statsSummary.ts`) and draws its
boards, picks and heatmap with the pure helpers in `src/lib/statsHome.ts`.
A server without it (404) gets a note, and the live parts still show. The
mock (`mock/stats.ts`) counts its log the same way and folds in a made-up
month of games before the simulated history so the home has weeks to show;
`H2_MOCK_STATS_ARCHIVE=0` leaves that out. The events carry no headshots,
so there are none.

### Derived stats

Medals, sprees, places, the weapon breakdown ("Tool of
Destruction"), EXP and rank are computed in the browser from the games'
events (`src/state/derive.ts`); the API does not serve them. They are
marked with a `*` and a tooltip. They only cover games this browser has
loaded, and a game's snapshot carries at most its recent kills, so they are
estimates for long histories. A game report with no recorded kill weapons
shows a message in Tool of Destruction. With partial data, the "Not recorded"
row gives the share of kills without a weapon. A player's Tool of
Destruction pairs each weapon's "kills with" (gold bar) with "killed by" (blue bar). Medals (original names;
the badges are raster art from `public/art/medals`, named after the medal
ids): Opening Act (first kill of a game), Last Word
(last kill), Payback (kill your last killer within 10 s), Twofer / Hat
Trick / Full House / Encore (2 / 3 / 4 / 5+ kills, each within 4 s of the
last), Hot Streak / Wildfire / Untouchable (5 / 10 / 15 kills without
dying). EXP is games completed plus two per win.

### Sign up in the browser

The dashboard's "sign up" makes a new Ed25519
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

### Friends and clans

Friends and clans come from the server (docs/live/live-api.md "Friends" and
"Teams"). Halo 2's clans are built on the Xbox Live Teams API, so the
protocol, the events and the code's types say team; everything the site
shows says clan. The dashboard's friends and clans screens
(`src/dash/Social.tsx`, `/dash/clans`), the stats site's clan directory and
clan pages (`/games/halo2/clans`, `/games/halo2/clans/:id`; the old `/games/halo2/teams`
paths redirect) and the Clans and Friends boxes on service records all
read them and follow the stream, so they change as events arrive
(`src/state/social.ts`). Writes are signed `h2live-event` JWS bodies sent to
`POST /v1/events` with an `events.write` token, the same way `h2 live friend`
and `h2 live team` send them (`src/state/sign.ts`). A clan's combined stats
are derived from multiplayer games where two or more members played on the
same side; its roster shows each member's role, rank and K/D.
The mock (`mock/social.ts`) seeds three clans and some friendships. A new web
account gets a friend request and a clan invitation a few seconds after
signing up, and seeded players accept whatever you send them.

### Fonts

All bundled fonts are under the SIL Open Font Licence, each with
its licence file beside it: Michroma (`src/assets/fonts`, the "h2 live" name
on the dashboard), and Orbitron and Barlow Condensed (`src/dash/fonts`, the
dashboard's headings and its condensed labels). The stats site draws its
wordmark as SVG (`stats/Wordmark.tsx`) and otherwise
uses system fonts (Arial for copy and headings, Verdana for the nav strip).

### Artwork

Raster artwork (banner strips, page and map textures, 24 player
emblems, medals, rank badges) is original and lives in `public/art`,
documented in `public/art/ART.md`; `src/lib/art.ts` maps emblems, medal ids
and rank names to its files. Weapon silhouettes are small SVGs in
`components/WeaponIcon.tsx`. No third-party logos or artwork are in this
repo or in the build.

## Names, icons and link previews

### Names

`src/brand.ts` holds the product names: `DASH_NAME` ("Live
Dashboard", /dash: the page titles, the header, the installed app's name),
`STATS_NAME` (the stats site, "h2 Uplink") and `SITE_NAME` ("h2 live", the
service itself, which only the link-preview card's mark carries), plus the
two sides' theme colours. Renaming is a one-line
change there; routes never change with a name. `vite.config.ts` writes the
names into `index.html` (`%H2_DASH_NAME%`, `%H2_STATS_NAME%`), and the
server reads them back out of the built HTML (`h2-dash-name`, `h2-stats-name`) for link previews. Names
considered for the stats site, avoiding anyone's trademarks as a product
name: **h2 Uplink** (chosen: a terminal you dial into for the record, in
the network-era voice of the old stats sites), h2 Debrief, h2 Dispatch,
Combat Record (too close to the Service Record pages), h2 Archive, and the
old "h2 live Online".

### Icons

`bun run brand` draws the favicons (`favicon.ico` at 16, 32 and
48 from the hand-tuned micro icon, `favicon.svg`), the Apple touch icon,
the manifest's 192, 512 and maskable 512 icons, `manifest.webmanifest` and
the default link-preview card (`og/default.jpg`, 1200 x 630) into
`public/`, from the project's own icon art (`art/app-icon/`, the iOS,
tvOS and Mac exports; `brand/plan.ts` lists them). A headless Chrome draws
them. Rerun it when the art or a name changes and commit the results:
they are our art, not the game's.

### Link previews

The app renders in the browser and crawlers run no
script, so the server writes each page's `<title>`, description, theme
colour, Open Graph and Twitter card tags into `index.html` in place of the
block between `<!-- page-meta -->` and `<!-- /page-meta -->`
(`crates/live/live-server/src/page_meta.rs`). A player's page gets their record
(games, wins, kills, K/D from the public stats), a game's its variant, map,
score, winner and players, a clan's its name, members and motto; other
pages their section. Only what an anonymous reader may see is used: a
hidden game or a private server gives the section's preview. The machine's
server does it in its SPA fallback; the Worker for the paths
`wrangler.toml` sends it first, reading `index.html` through the `ASSETS`
binding and asking the hub (`GET /v1/page-meta?path=`) only for a page that
names a player, game or clan. Pages stay `no-cache`.

Every page shares the default card for now. Planned: per-game and
per-player cards (a game's map picture, score and players; a player's
emblem and record) drawn on request at `/og/game/{id}.png` and
`/og/player/{tag}.png`, in Rust so both homes share it (a small rasteriser
such as `resvg`/`tiny-skia` with an OFL font embedded, built for wasm32 as
well), the map picture fetched from the published game art
(`H2_LIVE_GAME_ART_URL`, never from the repository), and the result kept
by the CDN under a key that includes the game's log position (an ended
game's card never changes). The open questions are the Worker's wasm size
and CPU budget for a 1200 x 630 render.

### The game page's header, pre-rendered

Both homes write the game's title, map, status, result and standings into
the `game-header-source` template in the initial HTML. The Worker gets the
same model from its `GET /v1/page-meta` call to the hub. `Game.tsx` mounts the
matching template content as the hero below the site header and
breadcrumbs. The template stays inert on other routes. Each player's
16×16 image uses the same appearance URL as the game report, with the
original art pack as a fallback.

The embedded JSON model lets the client keep the hero markup when its own
model matches. It replaces the markup when the game or score changes.
The TypeScript and Rust renderers share the goldens in
`test/fixtures/game-header.{json,html}`. Regenerate them with
`bun test/fixtures/gen-game-header.ts`. The PNG
pixel and palette fixtures are generated by
`H2_UPDATE_EMBLEM_GOLDEN=1 bun test test/emblem.test.ts`.

## Architecture

### Snapshot, then one stream

`src/state/live.ts` discovers the server
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
update without polling. The event reducer also flushes in hidden tabs for
browser notifications. Games outside the snapshot (deep links, older
games on a profile) are fetched on demand.

### Frames are not reactive

Live frames (up to 15 Hz per game) go into
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

### Replays

Replays fetch `/v1/games/{id}/frames` (1 Hz, following cursors) and the game stream's
events from `/v1/events?stream=game-<id>`; `ReplayTimeline` samples
positions, kills and the scoreboard at any tick and `ReplayClock` handles
play, pause, speed and seek (`state/replay.ts`).

### Signatures

Every event is queued to a `Verifier` (`state/verify.ts`)
that checks the compact JWS with WebCrypto Ed25519 in batches, yielding
between batches, and checks the per-stream hash chain where it has
consecutive versions. Keys come from `/.well-known/h2live.json` (server) and
`/v1/accounts/by-xuid/{xuid}/keys` (accounts, valid between log positions), cached by `kid`. Results land in
a separate store keyed by position, so rendering never waits on crypto.

### Styling

`src/styles/theme.css` holds the tokens: `--s-*` for the
stats site, `--d-*` for the dashboard, and the map palette (the renderer
reads its colours from them; `.dash` swaps in a green map). `app.css` has
the shared pieces, `stats.css` and `dash.css` the two sides. The dashboard's
shell (tunnel, orb, menu, prompts, transitions) is in `dash-shell.css`; the
orb, pods, bars and prompts are drawn in CSS and SVG, not images.

## Related

- [live-api.md](live-api.md): the contract the app is built against, including the optional game art.
- [live-security.md](live-security.md): signing and verification.
- [web-boundary.md](web-boundary.md): what goes on the dashboard and what on the stats site.
- [live-admin.md](live-admin.md): the Halo admin area.
- [web-viewer.md](web-viewer.md): the optional game art and the 3D game viewer.
- [live-art.md](live-art.md): extracting the game art the app can use.
