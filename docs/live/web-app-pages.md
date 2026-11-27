# Web app: the pages

Part of [The Live web app](web-app.md). Covers: The stats site; Open Games; Custom maps; Mods; Game variants; Game type scripts; Server stats; Derived stats; Medals; Sign up in the browser; Friends and clans; Fonts; Artwork.

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
skull use, the window's top medals (the server's), the weapon and map of the
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
(`src/state/sessions.ts`). **Join in Halo 2** opens the installed player.
**Filters** opens controls for multiplayer or campaign co-op, Clankers,
game type, named variant, map category, available public seats, campaign
and difficulty. **Apply** sends the choices with each session list page.
The variant field suggests names from sessions visible to the reader.
The browser remembers applied choices locally and **Clear filters**
restores the full list.

The game's detail page and a friend's lobby presence offer the same action
when the session admits the viewer. [Session links](live-custom-games.md#join-in-halo-2)

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
pack catalogue from `GET /v1/mod-packs`. One row shows the release, its
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
member, and can report a screenshot directly from its gallery. Its uploader
can edit the release metadata and submit in-game screenshots using
`h2 live mods capture`. Pending screenshots stay out of the gallery. Server
admins open **Screenshot review** from Live Admin, show an image on demand
and approve or remove it. Installation runs in the game. The mock serves
invented pack metadata and member artwork.
[Admin screenshot review](live-admin-moderation.md#screenshot-review)

### Game variants

A player's page lists the game variants they've published, the public
ones, under **Game Types** (name, game type, "scripted: <script>" when
it has one, date). Each links to its page (`statsPath("/variants/:id")`,
`web/src/stats/Variant.tsx`): name, description, author, game type, the
server's settings summary, the script with a link that opens it
read-only in the workbench (`statsPath("/rules")?script=<hash>`), the
`.h2variant` download and, for its author, Delete. The workbench's
**Make a game variant** and **My Game Types** tab are in
[rules-engine.md, "Workbench"](../game/rules-engine.md#workbench); the
routes are [live-api.md, "Game variants"](live-api-content.md#game-variants),
the pure half `web/src/lib/variants.ts`. The mock (`mock/variants.ts`)
serves every route in memory and seeds two variants for Sneaky Pete.

### Game type scripts

The Game Types page (`statsPath("/rules")`, `web/src/stats/Rules.tsx`
and `RulesTest.tsx`, the pure half `web/src/lib/rules.ts`) is the
workbench for the rules engine's scripts; what the server does for it is
in [rules-engine.md, "Workbench"](../game/rules-engine.md#workbench).
The page has one row of tabs, **Workbench**, **My Scripts** and **My
Game Types**, and no side menu. While it is shown the stats column is
1240 px wide rather than 902 (`styles/rules.css`), and under 1100 px
every part stacks.

The Workbench tab, top to bottom:

- **The bar.** Start from (a blank starter, a built-in or a saved
  script), Save (a sign-in link when signed out), Make a game variant,
  and the check's state: "Verifies", the script's name, its short hash
  and "saved" when the server has it, or the number of problems. A
  button that is off says why in its tooltip and in the line under the
  bar ("Save and Make a game variant are off. Fix the 2 problems in the
  script first."); the reasons are `saveBlocked`, `variantBlocked` and
  `whyOff` in `rules.ts`.
- **The script.** The Insert menus, the Monaco editor
  (`stats/RulesEditor.tsx`) and its shortcut line (quick fixes,
  rename, format, definition, references, outline). A read-only script
  has a bar with **Use as start** above them.
- **The check**, beside the script. **Problems** lists each error, then
  each warning, with its place; choosing one selects it in the editor,
  where Ctrl (Cmd)+. offers its fixes. **What a map needs** lists each
  label the script declares: its map text, its kind in words ("a set of
  zones, numbered") and the requirement in Forge's words, or "The script
  declares no labels: any map will do." **Budget** has the
  meters and each handler's worst case, and **Capabilities** the
  lobby's words. The page checks as you type.
- **Test.** No form: the server makes the matches up
  (`POST /v1/rules/simulate`). One strip sets them up (`SimStrip`): the
  players, 2 to 16, as a slider with a stepper; team play; and the seed
  with a die that rolls a new one (`rerolled`). The page plays 25
  matches by itself after each check and each change to the strip.
  Under the strip is the summary (`BatchView`): who wins, as bars with
  "9 of 25"; how the games end and how long they run; the best and
  average top score; the costliest tick against the budget, as a meter;
  and the warnings (`simWarnings`), script errors first, each with a
  link to a seed that shows it. **All matches** unfolds one row a
  match. Choosing a seed anywhere shows that match beside the summary.
- **Edit this match**, folded at the foot of Test. Opening it puts the
  shown match in the hand editor (`ScenarioEditor`) and uses it instead
  of the generated ones: how long to run, the round clock, the seed and
  team play; the seated players (slot, team by colour name, lives); the
  **Map** (its labelled things grouped by label, each with its number,
  team and class, **Add thing** and **Use the smallest map**; a match
  with no map of its own says it plays on the script's smallest map and
  offers **Make it my map**), with what the map lacks for the script's
  labels as problems above it and **Add what the script needs**; and
  the events in time order. "Player 0 kills Player 1, Add kill" adds a
  kill in one click and a menu adds any other event; a new event lands
  a second after the last and lengthens the run when it is past its
  end. Each event is a row that reads as a sentence, a zone or object
  picked by label and number ("capture 0"), and its time is
  typed as `0:02.50`, as seconds (`2.5`) or as ticks (`75t`)
  (`parseTickTime`). Every field is black on white, as the site's other
  fields are. Folding it goes back to the generated matches.
- **Match**, beside Test: one match in full. Its bar (`MatchBar`) names
  the seed, steps to the match before or after it in the batch, and has
  **Edit this match**; for an edited match it has **Back to generated
  matches**. Under it are how the match ended (the script, the score
  limit, the time limit), the final standings (team totals with team
  play), and a timeline: for each moment its time, what happened
  ("Player 0 killed Player 1", "Player 3 entered hill_corner 0"), the
  handlers that ran and their calls, who scored and their new score,
  and any team, lives or variable change. A long timeline shows its
  first 150 moments and **Show all**. An event no handler answered says
  so. An edited match whose map fails the script's labels runs nothing,
  and the results list the map's problems. While the script has problems the last results stay, dimmed, with
  the reason. **Copy as text** copies the trace, a line a tick.

A request that fails is put in words with whose fault it is
(`explainFailure`): the server's ("The Live server had a problem (HTTP
503). Your script isn't the cause; try again."), the connection's, the
account's, or the script's (the server's own message). A rate-limited
check asks again after three seconds. A server whose rules routes
answer 404 gets one notice, "This Live server doesn't have the script
tools yet", with **Ask the server again**; the draft is still kept in
the browser, the editor completes plain words, and Save, Make a game
variant and the test are off.

Signed out, everything but Save, Publish and the two list tabs works.
`?script=<hash>` opens a saved or built-in script read-only. The mock
(`mock/rules.ts`) answers every route from a few text checks, and makes
its matches from seeded random kills to ten points or two minutes. It
reads a script's `label` lines for the check's `labels` and `minimal`,
refuses a run whose map lacks them, and offers two quick fixes (a word
one letter off a label's name, an unused label) and a rename that
keeps a label's map text.

### Server stats

The home's windowed numbers come from the server's
summary (`GET /v1/stats/summary?window=`, docs/live/live-api.md "Stats
summary"), a projection the server keeps in day and hour buckets; the page
asks again every 30 seconds (`src/state/statsSummary.ts`) and draws its
boards, picks and heatmap with the pure helpers in `src/lib/statsHome.ts`.
A server without it (404) gets a note, and the live parts still show. The
mock (`mock/stats.ts`) counts its log the same way and folds in a made-up
month of games before the simulated history so the home has weeks to show;
`H2_MOCK_STATS_ARCHIVE=0` leaves that out. The kill feed shows a headshot
when the killing hit reports one.

### Derived stats

Sprees, places, the weapon breakdown ("Tool of Destruction"), EXP and rank
are computed in the browser from the games' events (`src/state/derive.ts`);
the API does not serve them. They are marked with a `*` and a tooltip. They
only cover games this browser has loaded, and a game's snapshot carries at
most its recent kills, so they are estimates for long histories. Tool of
Destruction groups scoring kills by their recorded damage source. A game
with no source data shows a message. With partial data, the "Not recorded"
row gives the share of kills without a source. A player's report pairs each
weapon's "kills with" (gold bar) with "killed by" (blue bar). EXP is games
completed plus two per win.

### Medals

Medals are the server's, never counted in the browser
([live-api.md](live-api-theater-stats.md#medals)). A game's record holds every player's
(a bot's included) from the game reads' `medals` rows, kept by slot in
`GameState.medals` (`src/state/reducer.ts`); a `GameMedal` streamed after
the last read adds one, and a later read replaces the counts and adds back
only the awards past its position. A career is the account's
`GET /v1/accounts/{who}/medals` (`src/state/accountMedals.ts`), or the sum
over its loaded games until that answers; with a tab, map or game-type
filter the chest sums the matching loaded games. The home's Top Medals is
the stats summary's `medals` for the window. Clans and the social model sum
the server's medals for their slots.

The names are the game's (`MEDALS` in `src/state/derive.ts`): Halo 2's 24
medals in the game's order, then our modes' (the superbounce family and
Incel). An id the site does not know shows by its id. The carnage report's
and a player's medal chests list them only once a game or player has one,
in rows: multi-kills, sprees, kill types, vehicle, objective, Incel, the
bounce medals, then any others. A Halo 2 medal's picture is the server's
game art (`medalKey`, [web-viewer.md](web-viewer.md)); our modes' are the
pack's (`PACK_MEDALS`, `public/art/ART.md`, "Batch 07" and "Batch 08"). A
medal with neither shows its name in a round badge (`MedalIcon`).

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
