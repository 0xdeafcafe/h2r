# h2 live web app

The web front end for h2recomp's Xbox Live replacement (`crates/live-server`).
It shows everyone who has signed up, who is online, the games they are
playing as they happen (live top-down map, scoreboard, kill feed), replays of
finished games, and the raw event log with signature checks. It is built
against the contract in [docs/live/live-api.md](../docs/live/live-api.md) and
[docs/live/live-security.md](../docs/live/live-security.md).

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
app with SPA fallback, as `h2 live serve` does:
`bun run build && H2_MOCK_STATIC=$PWD/dist bun run mock`),
`H2_MOCK_GAME_ART=1` (serve a stand-in `/art/game` manifest so the game-art
path runs; see "Game art" below). Without it `/art/game` has no pictures,
like a server with no game art. The mock's own map geometry (below) is served
either way; `H2_MOCK_MAP_GEOMETRY=0` turns that off too, and then `/art/game`
answers 404. `H2_MOCK_SESSIONS=<n>` sets how many open sessions the mock keeps
for the dashboard's "find a game" (default 5; 0 shows its empty state).
`H2_MOCK_ACTORS=1` puts the mock campaign's AI and vehicles
(`mock/campaign.ts`: about thirty actors, two vehicles in use and three
parked empty, and a `bsp` zone that changes every 25 s, added by a frame
hook) into every game's frames; without it only games whose mode is
`campaign` get them. `H2_MOCK_INDOOR=1` shuts each arena's glb inside a busy building (an
inward-facing shell, partition walls, an upper floor, pillars and two
"machines", over two `bsp_<n>` zones and a `machines` node, plus a third
zone marked cinematic-only and a `backdrop` of far hills), for trying the 3D
view's cutaways and zones. `H2_MOCK_MODELS=1` adds stand-in player and weapon models (block figures,
`mock/models.ts`) to the art manifest, so the 3D view's model path runs:
the bipeds are skinned and carry a dozen clips (models version 2);
`H2_MOCK_MODELS=static` makes them the static version 1 kind; without it
players are capsules, as on a server with no models. Every game's frames
carry stances, speeds, actions and hits (`mock/action.ts`: players jump,
crouch, throw grenades, swing, reload, and hit who they fire at);
`H2_MOCK_ACTION=0` leaves them out, like a sender from before they existed.
Each arena also has furniture (`mock/fixtures.ts`): a lift platform that
rides up and down and a walking thing with no node in the glb (both in the
frames' `machines`), two pairs of teleporters with their links and a
gravity lift (in the glb's extras); `H2_MOCK_FURNITURE=0` leaves it out.
`H2_MOCK_ADMIN=1` makes every account
signed up in the browser an admin, so the dashboard's server screen shows "your server"
and its invites, accounts and import screens, and the stats site shows the
Halo admin area (`mock/admin.ts`). `H2_MOCK_HOPPER=1` serves a stand-in
playlist file (invented playlists, plain numbers) so the Halo admin's
playlist builder has something to edit; without it that tab says nothing
has been imported yet.

**Against the real server.** `h2 live serve` serves `web/dist` at `/`, so
build and open the server's URL. To use Vite's dev server with a real
server instead of the mock, run `H2_LIVE_URL=http://127.0.0.1:8074 bunx vite`.
You can also point any running copy of the app at any server from the
dashboard's "server" item, or with a deep link: `?server=https://live.example.net`.
The choice and recent servers are kept in localStorage.

## Two sides

**The dashboard** (`/`, `/dash/:section`, `/dash/players/:tag`, `src/dash/`) is the "live" side:
a glowing orb, a wireframe tunnel and an arc of menu bars, each opening a
bezelled list panel (`dash/menu.ts`). Signed out the menu leads with sign
in, create and recover, then live now, game history, find a game, players
and settings (the ones that need an account show greyed with a hint).
Signed in it is live now, find a game, friends, players, game history,
account (recovery code, authenticator, devices, sign out) and settings;
profile and the server (`/dash/server`: service admin) sit under settings.
The Halo admin area (MOTD, playlists, custom content) is on the stats site
at `/stats/admin` ([docs/live/live-admin.md](../docs/live/live-admin.md)).
Keyboard: arrows move, Enter (or `a`) selects,
Escape / Backspace / ArrowLeft (or `b`) goes back; the mouse works too.

**The stats site** (`/stats/...`, `src/stats/`) is the record, laid out like
a mid-2000s game stats site: a fixed 902px column on a navy page, a header
with the Welcome box, a black nav strip, breadcrumbs, a banner strip, then a
290px menu column (pill menu, People Finder, reports, Recent Games) beside
the content. The nav's tabs start with the home, "h2 live Online"; the
way back to the dashboard is the small "‹ Dashboard" chip in the header's
corner, not a tab, and every page's breadcrumbs start at "Home" (`/stats`).
Pages: the home (`/stats`, `stats/Home.tsx`: live now with the live games,
players online and open games; the last 24 hours and all-time totals;
leaderboards for today, this week, this month or all time (`?window=`):
most kills, best K/D with 3+ games, most wins, longest killing spree,
Legendary levels and time played; most played maps, most used weapons, an
activity heatmap by day and hour in your time, game types and playlists,
skull use, medals from loaded games (derived), the weapon and map of the
week, clan standings and recent games), games (`/stats/games`: the featured
live map and the games spotlight), game details (`/stats/games/:id`: overview, the Game
Viewer with the live tactical map or the replay, and the post game carnage
report with Overview / Carnage / Medals / Weapons tabs), players and
leaderboards (`/stats/players`), a player's service record, game history
with its preview pane, and career stats with Tool of Destruction, the Medal
Chest and compare (`/stats/players/:tag`, `/history`,
`/career?compare=<tag>`), and the event log (`/stats/events`).
Campaign and multiplayer are kept apart: K/D, wins, medals, EXP,
leaderboards and clan records count multiplayer games only, and campaign
has its own record (`/stats/players/:tag/campaign`: levels, completions,
time played, deaths, co-op partners), its own history tab
(`/history?tab=campaign`) and spotlight tab (`/stats?games=campaign`), and
a mission report in place of the carnage report. A game's type is shown
as Custom Game, Matchmaking: <playlist>, Campaign, Campaign Co-op or
Campaign Online, with the difficulty (`src/state/gametype.ts`). The mock
starts an occasional campaign game (`mock/coop.ts`). The old
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

**Custom maps** (docs/game/maps.md §5). A session's advertisement and a game
may carry a `map_id`, the map package played; `map` is then its base map.
The app looks each `map_id` up once per page load (`GET /v1/maps/{id}`,
`src/state/maps.ts`; the rules are pure in `src/lib/maps.ts`). "Find a
game" shows a custom map's name with a "custom map" tag, its author and
"needs download (2.4 MB)" from the package size; a 410 (taken down by an
admin) reads "custom map unavailable", a 404 shows as a plain map. Games
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

**Server stats.** The home's windowed numbers come from the server's
summary (`GET /v1/stats/summary?window=`, docs/live/live-api.md "Stats
summary"), a projection the server keeps in day and hour buckets; the page
asks again every 30 seconds (`src/state/statsSummary.ts`) and draws its
boards, picks and heatmap with the pure helpers in `src/lib/statsHome.ts`.
A server without it (404) gets a note, and the live parts still show. The
mock (`mock/stats.ts`) counts its log the same way and folds in a made-up
month of games before the simulated history so the home has weeks to show;
`H2_MOCK_STATS_ARCHIVE=0` leaves that out. The events carry no headshots,
so there are none.

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

**Friends and clans** come from the server (docs/live/live-api.md "Friends" and
"Teams"). Halo 2's clans are built on the Xbox Live Teams API, so the
protocol, the events and the code's types say team; everything the site
shows says clan. The dashboard's friends and clans screens
(`src/dash/Social.tsx`, `/dash/clans`), the stats site's clan directory and
clan pages (`/stats/clans`, `/stats/clans/:id`; the old `/stats/teams`
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
`/art/game/manifest.json` (docs/live/live-api.md "Optional game art"), by default
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
off by itself on a machine that can't keep up), orbit, top-down,
follow-a-player and free-fly cameras (F; WASD, Q/E down and up, drag with
either button to look or click to lock the mouse, wheel or trackpad for
speed, Shift to go faster, one finger to look and two to fly on a touch
screen, or a gamepad: `viewer/flycam.ts`), and a grid floor when the map has no geometry. The
window can be dragged, resized, maximised, or popped out to
`/stats/games/:id/3d`. The 2D map, the scrubber and the 3D window read one
replay session (`src/viewer/session.ts`), and a popped-out window keeps in
step with the page over a `BroadcastChannel`. `?view=3d` opens the window
and `?map=iso-ne` picks a view, so both can be linked to. The mock builds
two arenas of its own from its invented floor plans (`mock/arena.ts`:
slabs, ramps, pillars and cone trees, with a small rasteriser for the
pictures); its third map has none, so the fallbacks run too.

**Vehicles and AI.** A frame may list `vehicles` (those with someone in
them: kind, position, forward and up) and `actors` (living AI-controlled
bipeds, campaign), both optional. A vehicle is drawn as its glb from the
manifest's `models.vehicles`, turned by its forward and up, or as a box of
about its kind's size; a player's vehicle takes a light wash of their
colour. Whoever is in one is not drawn themselves: the vehicle stands for
them and carries their label (seat nodes are not used yet). Actors are
sampled, eased and drawn through the same path as players (an index made
from their id, negative so it never meets a player's): their species' glb
in its own colours, else the spartan's untinted, else a small capsule, with
a ring in their side's colour (covenant purple, human green, flood tan,
sentinel grey). They have no name label unless "AI names" is on (the button
shows when a game has actors) and no aim ray; their shots draw like
players'. At most 128 are drawn from a frame. On the 2D maps actors are
small squares and vehicles an outline under their riders.

**Seeing into a level.** A level's structure is single-sided and faces
into the playable space, so the 3D view culls its back faces whatever the
glb says and it can be looked into from outside; meshes under a `machines`
node are ordinary models and keep the side they came with. Every mesh is
found by walking the glb, so one `structure` node or many `bsp_<n>` ones
are the same to it. Two more ways in (`viewer/cutaway.ts`): **Cut** (key
`c`, `[` and `]` or the slider) hides the level above a height, and while
following a player keeps to just over their head; **See-through** (key `x`
steps off, *in focus* and *everyone*; *in focus* by default) cuts away
whatever of the level lies between the camera and the
player being followed (or pointed at in the scoreboard), else every player
(up to 8, nearest the camera first), a cone to each with a dithered rim,
leaving the floor they stand on, so a player inside a building is seen
through its walls and roof; *everyone* cuts to all of them as well as the
one in focus. Players, vehicles and machines are never cut. Both are a clipping
plane and a few lines in the map materials' shaders, shared by all of
them, so a big level costs no more. `?cut=<height>` and `?see=0` on the
pop-out page set them from a link. (Culling front faces instead was
tried: Halo's levels are thick-walled and faced inwards, so it shows the
inside of the near walls and ceilings rather than what is beyond them.)

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
(`viewer/players3d.ts`). It is life size up close and grows as the camera
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

**Animation, actions and damage (optional).** Frames may also say how each
player and AI biped is moving (`stance`: stand, crouch, airborne, vehicle,
dead; `speed`), what they just did (`actions`: grenade, melee, reload,
jump, switch, pickup; `grenade`: the type), who hit whom (`hits`: attacker
and victim as `p<index>` or an actor or vehicle id, the victim's position,
an amount and a kind), every vehicle in the world with whether anyone is in
it (`occupied`), and which zone of a campaign level is loaded (`bsp`). All
optional; a frame without them draws as before.

- *Clips.* A models table of version 2 has skinned bipeds whose glb carries
  clips named `<weapon_class>:<state>`, and gives each weapon stem its
  class (clips every class shares are `any:<state>`). `viewer/anim.ts`
  (pure) picks the looping state from the stance and speed (idle, walk,
  run, crouch, airborne, die, dead; a sender without speeds has them
  worked out from how far the player moved) and lays one-shots over it
  (fire on a frame's shots, grenade, melee, reload, switch from its
  actions, land or land_hard on coming down, hard after a drop of more
  than 1.2 units below the jump's peak); the clip is the held weapon's
  class's, else the rifle's, the unarmed, or any class that has the
  state, a missing loop falls back to a nearer one and a missing one-shot
  isn't played. The game's jump has no take-off clip: `airborne` is the
  whole leap (tuck, then hang) and plays once from leaving the ground,
  held; dying plays once (`die`) and holds its last frame; a body first
  seen already dead lies in `dead`.
  `BipedAnimator` (`viewer/players3d.ts`) plays them on a three.js mixer
  per biped: loops cross-fade, walk and run are paced to the ground speed,
  and a one-shot outweighs the loop only on the bones it moves. An AI's
  model faces where it aims and its spine takes a third of the aim's turn
  (at most 0.3 rad) and a pitch of at most 0.6 rad: turning a grunt's or
  hunter's whole spine to the aim swung its tank or back plates about. A
  version 1 (static) model glides in its pose as before, a capsule squats
  when crouched, and reduced motion leaves every model in its pose.
- *Bodies* (`viewer/bodies.ts`). A player who dies leaves their model
  where they fell (where the frames put the dead, which is their body),
  playing the death clip and holding it; respawning makes a new model and the body stays
  for a minute, at most 24 of them, all cleared on a seek. AI bodies come
  in the frames as actors with the `dead` stance and lie until the game
  clears them away.
- *Damage and grenades* (`map/events.ts`, pure; one `EventFeed` per view).
  A hit draws a line from attacker to victim that fades in a quarter of a
  second, and a hit marker with the damage floating up from the victim for
  under a second (the amount is a share of the victim's vitality, shown in
  percent); sustained fire on one victim is one marker with a rising
  number; the last hit before a victim is found dead is the kill: red,
  ringed, bigger, and it stays longer. A grenade throw draws the arc of the
  throw from the hand along the aim (the frames don't carry the grenade
  itself, so it shows the throw, not where it bounced). The 3D view
  (`viewer/effects3d.ts`) and both 2D maps draw them; with reduced motion
  the numbers stay put, the line holds instead of flashing, and no grenade
  flies.
- *Vehicles.* All of them are drawn; one nobody is in is darker (3D) or a
  faint grey outline (2D) and has no label. Once the frames carry
  vehicles the level's own copies (`vehi` placements and multiplayer
  vehicle spawns) are hidden, so a vehicle isn't drawn twice
  (`viewer/objects3d.ts`).
- *Zones* (`viewer/zones.ts`). When a level's glb has `bsp_<n>` nodes, the
  3D view shows only the zone in play (`ZoneFollower`): the one the frames
  name when the players are in it (by each zone's bounding box, a unit of
  margin), else the one the followed player, or most players, stand in,
  the zone already shown winning ties where zones overlap. A cutscene's
  zone isn't play. A new zone has to be wanted for 0.8 s before the view
  moves to it (at once on a seek), and the camera, unless following
  someone, moves to frame it. With nothing to go on it shows every zone
  not marked cinematic-only in its node's extras, and never none at all.
  A `backdrop` node (far scenery) is hidden. When the level has several,
  the toolbar's zone picker offers "Zone in play", "All zones", and each zone by its
  BSP's name (`extras.name`; cutscene zones too when "Cutscene zones" is
  ticked); its "Show" menu switches the backdrop, cutscene zones, decals,
  effects, sky and fog. The camera and the lights fit the playable zones
  only (`viewer/level3d.ts`).
- *Layers, sky and fog* (`viewer/layers3d.ts`, `viewer/sky3d.ts`). A zone's
  `decals_<n>` and `effects_<n>` meshes are drawn with their pictures,
  after the level and without depth: decals pulled just in front of their
  wall and multiplied, doubled or alpha-blended as their material's
  `extras.blend` says, glows and light shafts added, unlit and unfogged
  (docs/live/live-art.md, "Layers"). The level's `sky.glb` is drawn around the
  camera before everything else, moving with it but not turning, its
  additive layers left out; behind it is the sky's clear colour, else its
  fog colour. The sky's atmospheric fog is drawn as the game's, not
  three's: clear up to its start past the point in focus, thickening
  linearly to the sky's maximum density at its opaque distance and never
  thicker beyond, so an overview fades the far side instead of hiding it.
  `useGameFog` patches three's fog chunks for that (the cap is one shared
  uniform every material's `onBeforeCompile` gets; a material that sets
  its own must chain the prototype's). A sky without fog but with a
  sun-lit glow keeps the exponential haze in the glow's colour. With no sky the
  view keeps its own gradient and haze. The occlusion pass leaves out
  anything see-through that writes no depth (`viewer/aopass.ts`).

**Machines, teleporters and lifts (optional).** Frames may carry `machines`:
lift platforms, doors and other big scripted movers, each with the name of
the map glb's `machines` child it is (`node`), its place, facing and device
position. A live frame has only the ones that moved, a recorded one all of
them, so `FrameBuffer` keeps the last word on each and `sampleMachines`
looks a few frames back (`state/frames.ts`). The 3D view moves the named
node (`viewer/machines3d.ts`): one with a transform of its own is put where
the frame says; one whose mesh is already in world space is moved by how
far the machine is from where it was first seen. A machine the glb has no
node for is drawn from `models.objects.<kind>` when the server has that
glb, else as a box. A map glb's `asset.extras.features` lists teleporters
(with `links` between them) and gravity lifts (`map/features.ts`): the 3D
view stands a ring at each teleporter in its role's colour, arcs each link
through the air with an arrowhead where it arrives, and draws each lift as
a translucent column with an arrow along its push
(`viewer/features3d.ts`); the 2D maps draw rings, dashed curves and a disc
with chevrons, reading the extras from the head of the glb (a streamed read
that stops after the JSON, never the meshes). Both have a "Lifts &
teleporters" button when the map has any.

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
