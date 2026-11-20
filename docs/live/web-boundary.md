# Live web: the dashboard, the stats site and the line between them

The web app (`web/`, live.forbes.red) has two faces:

- **the dashboard** (`/`, `/dash/...`, `web/src/dash`): the "Live" side, in
  the style of the original console dashboard: green light on black, the
  orb, glass pods, bevelled plates, blade lists;
- **the stats site** (`/games/halo2/...`, `web/src/stats`): the game's record, in
  the style of the Halo 2 and Halo 3 era stats sites: a navy fixed column,
  blue title bars, dense tables.

The stats site is Halo 2's, under `/games/` so another game's can sit
beside it (`STATS_ROOT` in `web/src/routes.ts`; `/games` alone goes to
it). It was at `/stats` first: the servers answer `/stats/*` with a 301 to
the same page under `/games/halo2` (`page_meta::moved` in
`crates/live/live-server`, both homes), and the app redirects the old paths too.

Both recreate a look and a layout language only. No real logos, marks,
wordmarks, emblems, medal art or proprietary fonts; the site is "h2 live".

This page says which side a screen belongs on (§1), checks every current
page against that (§2), and sets out the dashboard's dense style for
screens that carry a lot of numbers (§3). Admin and server screens are in
[live-admin.md](live-admin.md); the API is in [live-api.md](live-api.md).

## 1. The rule

**The stats site is the public, linkable game record.** Games and their
carnage reports, players' service records (history, career, campaign),
maps, clans, leaderboards and co-op campaign records. These are pages you
would paste into a chat to show someone a result, they read the same to
anyone who opens them, and they stay true after the fact.

**Everything else is the dashboard.** Live state (who is online, open
sessions, games in progress as a list), your own account, friends,
messages and Xbox Live teams, running the server, and the Live service's
admin: accounts, messages, player reports and usage.

The one exception is **Halo Admin**, which stays on the stats site (§1.1).

The test for a new screen: *would you send someone the link to show them
what happened?* If yes, it is the stats site. If it is about *now*, about
*you*, or about *running the service*, it is the dashboard.

Three consequences:

1. **One home per thing.** A thing lives on one side. The other side may
   link to it, or show a one-line teaser that links to it, but does not
   rebuild it. (Today "who's online" is built three times; §2.)
2. **A live game's page is on the stats site.** `/games/halo2/games/:id` is the
   same URL before and after the game ends, and after it ends it is the
   carnage report: the record. The dashboard's "live now" list links to it.
3. **Actions are the dashboard's.** A stats page can show that two players
   are friends or that a player is in a clan (public facts), but adding a
   friend, answering an invite, joining or running a team, and every Live
   admin action happen on the dashboard. A stats page links there.

### 1.1 Two admins

- **Halo Admin** (`/games/halo2/admin`, stats site): the game's configuration.
  Playlists (hoppers), the message of the day, custom content and mods.
  It stays where it is, in the stats site's style.
- **Live admin** (`/dash/admin`, dashboard, in the dense style of §3): the
  hub for usage, dedicated hosts, the raw event log and the style sample.
  Confirmed server admins open it from the dashboard's Settings.
  Account, invite and player report tools sit under `/dash/server`.

The line: if it changes what the game shows or plays, it is Halo Admin;
if it is about the service, the accounts on it or what they do, it is
Live admin.

### 1.2 Teams and clans

The protocol has one thing, Xbox Live Teams; the site has two faces of it.

- **Teams** are the dashboard's: the service feature. Creating a team,
  recruiting, answering, roles, leaving; the team list on your account.
- **Clans** are the stats site's: a team seen as a Halo record, with its roster
  and a service record with shared games, wins, kills and medals derived
  from loaded game data. Other ideas for the clan record:
  - clan versus clan history, from games where two clans' members met;
  - clan leaderboards by window, beside the player ones;
  - a clan's game history and "clan of the week";
  - the clan's emblem and colours on its members' carnage report rows.

### 1.3 The event log, twice

- **Stats: the game record's events** (`/games/halo2/events`): a public view of
  only the events that make the record: games, players joining and
  leaving games, results, kills, scores and rank changes.
  `web/src/stats/eventKinds.ts` defines the allowed event kinds and streams.
  Medals are derived from game kills.
- **Dashboard: the raw log** (`/dash/admin/events`): every event, every
  stream the API makes available to the signed-in admin. Open **Event Log**
  from dashboard Settings or the Live admin hub. The log opens at the
  latest page, with older and newer pages and expandable event data.

The API does not change: `GET /v1/events` keeps its access rules (the
privacy redaction in `crates/live/live-server/src/sessions.rs` decides what
each reader sees), because the web verifier and other clients read it.
Hiding the raw log from non-admins is a choice of the web app, not a
security boundary.

## 2. Audit

Every route and every page-level component, checked against §1.
"Stays" means it is already on the right side.

### The dashboard (`web/src/dash`)

| Screen | Route | Verdict | Why |
|---|---|---|---|
| Main menu (orb, ring of pods) | `/` | stays | The front door. |
| Live now | `/dash/live` | stays | Live state; rows link to `/games/halo2/games/:id`. |
| Friends | `/dash/friends` | stays | Your account, with actions. |
| Teams | `/dash/teams` | stays | The Xbox Live Teams feature: create, recruit, answer, roles (§1.2). A clan's record is the stats site's. |
| Players | `/dash/players`, `/dash/players/:tag` | stays | Presence ("who's online"). The player pod links to the service record. |
| Game history | `/dash/history` | removed | The Games tab (`/dash/games`) shows recent games; the full history is the stats site's (`/games/halo2/games`, a player's `/history`). |
| Settings, profile, create account | `/dash/settings`, `/dash/signup` | stays | Your account. |
| Account and recovery, recover, sign in | `/dash/account`, `/dash/recover`, `/dash/signin` | stays | Your account. |
| Server overview | `/dash/server` | stays | Running the service. |
| Invites, accounts, import, connect | `/dash/server/{invites,accounts,import,connect}` | stays | Running the service. |
| Live admin hub | `/dash/admin` | stays | Admin-only entry in Settings. Links to usage, hosts, the event log and the style sample. |
| Live admin: usage | `/dash/admin/usage` | stays | Usage accounting (§5), in the dense style. |
| Live admin: hosts | `/dash/admin/hosts` | stays | Running and recently ended dedicated hosts. |
| Live admin: accounts and player reports | `/dash/server/{accounts,reports}` | stays | Service administration. |
| Live admin: Event Log | `/dash/admin/events` | stays | The raw event log, admin-only in the web UI (§1.3). |
| Dense style page (new) | `/dash/admin/style` | new | The dense style on fixed data (§3). |

### The stats site (`web/src/stats`)

| Page or part | Route | Verdict | Why |
|---|---|---|---|
| Home: leaderboards, tallies (24 hours, all time), of the week, maps, weapons, game types, skulls, activity heatmap, clan standings, recent games, Server Report | `/games/halo2` | stays | The record, summed. |
| Home: "Live Now" live game cards | `/games/halo2` | stays, as a teaser | They link to the live game pages (§1.2). Keep it to the cards. |
| Home: "Open Games" table (open sessions) | `/games/halo2` | stays | Read-only view of advertised sessions alongside Live Now. |
| Header Welcome box: "Players Online (n)", "Friends Online (n)", "Games Live (n)" | all | stays, as links | One-line teasers. "Players Online" should open `/dash/players`, not `/games/halo2/players`. |
| Games | `/games/halo2/games` | stays | The record; its live tab links to live game pages. |
| A game, carnage report, campaign report | `/games/halo2/games/:id` | stays | The record. |
| 3D replay window | `/games/halo2/games/:id/3d` | stays | Part of a game's record. |
| Players directory and leaderboards | `/games/halo2/players` | stays, **splits** | Leaderboards and finding a service record are the record. The online-first sort and presence column are live state: sort by name or rank here, and leave presence to `/dash/players`. |
| Service record, history, career, campaign | `/games/halo2/players/:tag[/history,/career,/campaign]` | stays | The record. |
| Player page: Friends and Clans boxes (`PlayerSocial.tsx`) | `/games/halo2/players/:tag` | stays | Public friends and clan membership. The friends box links to the dashboard's friend controls. |
| Clans directory, a clan's page | `/games/halo2/clans`, `/games/halo2/clans/:id` | stays | Roster and clan service record (§1.2). Management links go to dashboard Teams. |
| Event Log | `/games/halo2/events` | stays | The game record's events (§1.3). |
| Halo Admin: message of the day, playlists, custom content | `/games/halo2/admin` | stays | Game configuration (§1.1). |
| Nav tabs "Event Log", "Halo Admin", "Server" | all | stay | "Event Log" now opens the game events view; "Halo Admin" stays for admins; "Server" leaves for the dashboard. |
| Sign-in link redemption | `/signin` | neither | A one-shot utility; no chrome. |

### Shared components (`web/src/components`, `web/src/stats/clanBits.tsx`)

Emblem, map pictures, game type, medal, rank, skull and weapon icons,
the scoreboard, the kill feed and the map canvas are drawn on both sides
and stay shared. `clanBits.tsx` (clan emblem, presence words) is shared
too and could move to `components/` when it next changes.

### Remaining page moves

- Put live presence on the dashboard's player directory.
- Add dashboard message management when there is a web messages screen.

## 3. The dense dashboard style ("deck")

The console screens are a single 1280 × 720 picture scaled to the window,
with big type and a handful of rows. That is right for a menu and wrong
for an admin screen with thirty rows and four charts. The deck keeps the
dashboard's materials and drops the stage:

- **Same materials.** Barlow Condensed headings, the bevelled frames and
  recessed faces from `web/src/styles/dash-pages.css`, and the console
  list's row height, focus treatment and yellow-green selection. Back uses
  the dashboard prompt from `web/src/dash/parts.tsx`: Esc keycap, green B
  disc or left arrow, depending on the last input device.
- **Real pixels, not the stage.** The page scrolls; panels sit on a
  12-column grid (6 at medium widths, 1 on a phone). Body 14px, labels
  12px small capitals, tile figures 30px.
- **Panels, tiles, wells.** A *panel* is a bevelled plate with a header
  (title, a muted note, a chart/table toggle). A *tile* is a small panel
  for one metric. Charts and tables sit in a *well*: a recessed, near
  black green surface, so marks read against a flat ground.
- **One filter row**, above everything it scopes (the date range first,
  as presets).

### Components

All in `web/src/deck/`; the data shaping is pure and tested
(`web/test/deck.test.ts`), the rendering is tested on the server
(`web/test/deck-render.test.ts`).

| Component | What it is |
|---|---|
| `DeckPage` | Trail, title, Back (gamepad B, keyboard B, Escape or Backspace), the filter row, the grid. |
| `Panel` | The bevelled panel; with `table`, a toggle swaps its chart for its table. |
| `Presets` | The filter row's preset pills (a radio group). |
| `StatTile` | Label, value, change against the period before (arrow, sign and a word, coloured by whether up is good), sparkline. |
| `Sparkline` | 2px line in the quiet hue, the last point in the accent. |
| `TimeChart` | Days across, one axis: `line` (1 to 5 series), `area` (one series with a 10% wash), `stack` (columns, parts of a daily whole). Crosshair snapping to a day, a tooltip listing every series, arrow keys for the same. |
| `SeriesTable` | A time chart's table twin. |
| `RankBars` | Ranked horizontal bars, one hue, value at the tip, "other" in the quiet hue. |
| `SegBar` | A thin segmented bar for one whole's parts beside its figures (no text on it). |
| `DenseTable` | Sortable table: tabular figures, numbers right-aligned, an optional inline bar under a number. |
| `Legend` | Swatches for bars and areas, line keys for lines. |

`chart.ts` has the shaping: day ranges and filling missing days, compact
counts (1,284 / 12.9K / 4.2M), bytes (powers of 1024), axis ticks (clean
steps, binary ones for bytes), scales, paths, stacking, top-N with
"other", deltas and the crosshair's snap.

### Chart rules

Taken from the dataviz method the charts were designed with:

- The form follows the job: a single number is a tile, not a chart;
  parts of a whole over time are stacked columns; a ranking is ranked
  bars; "who uses the most" is a table with inline bars. No pies, no
  donuts, no dual axes.
- One axis per chart. Two measures of different scale are two charts.
- Thin marks: 2px lines, columns at most 24px wide with a 4px rounded top
  and a square base, 2px gaps between stacked parts, a 2px ring of the
  well's colour round end dots. Hairline, solid grid; the baseline a
  step brighter.
- A legend whenever there are two or more series; none for one (the
  panel title names it). Text never wears a series colour.
- Every chart has a table view and a keyboard path; the tooltip never
  holds the only copy of a value.

### Palette

Chart colours are in `web/src/deck/palette.ts`. The well is `#08180a`.
Categorical slots, in fixed order, never cycled:

| Slot | Hue | Hex |
|---|---|---|
| 1 | green | `#55a82a` |
| 2 | violet | `#8a7cf0` |
| 3 | amber | `#c98500` |
| 4 | blue | `#2b97c9` |
| 5 | rose | `#e0607a` |

Validated with the dataviz skill's checker against the well, dark mode:

```
validate_palette.js "#55a82a,#8a7cf0,#c98500,#2b97c9,#e0607a" --mode dark --surface "#08180a"
  [PASS] Lightness band         all 5 inside L 0.48–0.67
  [PASS] Chroma floor           all 5 >= 0.1
  [PASS] CVD separation         worst adjacent #e0607a↔#2b97c9 ΔE 12.6 (protan)
  [PASS] Normal-vision floor    worst adjacent #2b97c9↔#c98500 ΔE 26.0 (normal)
  [PASS] Contrast vs surface    all 5 >= 3:1
```

The ordering is what makes it safe: the green and the amber are far
apart in the order because under deuteranopia they collide (ΔE 3.5) if
they touch. So these slots are for forms where only neighbours touch
(stacks, lines, bars). Scatter plots and small multiples, where any two
marks can sit together, take two series at most. A sixth series is a
bug (`seriesColor` throws); fold the tail into "other".

Single-series marks use the dashboard's lime (`#9be34e`); de-emphasis,
sparklines and "other" use `#46703a`. Text: `#e6ffc0` (17:1 on the
well), `#a5ce78` (10:1), `#6f9a4c` (5.6:1). Status colours (good
`#7fe230`, warning `#f2c230`, critical `#ff5a3c`) are reserved and always
come with an icon and a word.

The deck is dark only, like the rest of the dashboard.

### The style page

`/dash/admin/style` lays the components out as the admin usage screen
will be, on fixed, seeded sample data (`web/src/deck/mock.ts`; it says so
in the filter row). It needs no sign-in and fetches nothing. Run it with
`bun run dev` in `web/`.

There is no scripted screenshot path for the web app (the only one is the
editor guide's, `scripts/editor-screenshots.sh`), so this page has no
picture in `docs/images/`.

## 4. Decided (2026-11-18)

- **Two admins** (§1.1): Halo Admin stays on the stats site for game
  configuration; the dashboard gets a Live admin for the service and its
  people.
- **The event log** (§1.3): the stats site keeps a public view of the
  game record's events; the raw log is the dashboard's, admin-only in the
  UI; `/v1/events` keeps its rules.
- **Teams and clans** (§1.2): teams to the dashboard, clans stay on the
  stats site and grow Halo features. Presence, friend and team action
  buttons move to the dashboard; stats pages link across.
- **Messages sent** counts friend requests and game invites, the two
  message events there are (`FriendRequested`, `GameInviteSent`). The
  counter is `messages` with a kind per message type, so text slots in as
  `messages.text` when a messaging feature exists.

## 5. Usage

The Live admin's usage screen (`/dash/admin/usage`) shows who uses the
most of the server and what they use it for. Open **Usage** from the hub
(`/dash/admin`) or from the pod on Server → Accounts. The counting and
the two endpoints it reads are in
[live-api.md](live-api.md) "Usage"; this section is what the screen
shows and where its numbers stop.

**On the dashboard, not the stats site.** Usage is about running the
service and the accounts on it (§1), and it is for admins only. The stats
site keeps what is about Halo: games, records, ranks. A player's row
links to their service record there, and nothing more crosses.

**The screen** is the deck (§3), with a range (7, 30 or 90 days) and a
sort for the accounts table:

- Six tiles: data uploaded, events written, messages sent, games played,
  sign-ins and player reports, each with its change against the range
  before (read as twice the range) and a sparkline. More reports is
  marked as bad news, not good.
- Data uploaded per day, stacked by kind: events, live frames, content,
  cloud saves.
- What is stored now, by kind (events, content, cloud saves).
- Events written, messages sent (friend requests and game invites), and
  active accounts with sign-ins, per day.
- Events by type, the largest seven and the rest folded into one row.
- The top 25 accounts by the chosen sort (uploaded, stored, events,
  messages, reported), with uploads split by kind, stored, events,
  messages, games, reports against them, active days and last active.
  The table re-sorts in place; the sort chooses who is in it.

A refetch keeps the last numbers on screen, dimmed. Signed out, or signed
in as someone who is not an admin, the screen says so and reads nothing.
The shaping is `web/src/deck/usageView.ts`, the reads
`web/src/admin/usage.ts`; the dev mock answers both endpoints from the
style page's seeded sample (`web/mock/admin.ts`), so `bun run dev` with
`H2_MOCK_ADMIN=1` draws it.

**Where the numbers stop.**

- Bytes uploaded are request bodies as received, which is the cost.
  Stored bytes are kept apart, and live frames are not stored at all
  (their 1 Hz record is in the events).
- Cloud saves stored counts from when counting began: saves written
  before then are not in it, and nothing goes back to add them.
- The co-op relay is not metered (in the Worker it has no line to the
  hub's store).
- Events by type are counts, not bytes.
- 400 days are kept. Older days are dropped by the write that starts a
  new day; stored totals have no day and stay.
- `h2 live store copy` copies the log, so the counts read from it rebuild
  on the other side. The metered counts (frames, content and save bytes,
  stored saves) are not copied yet and start again from zero.
- A later sink, Cloudflare Analytics Engine, would sit behind the same
  `meter` call, beside the in-process counts. It is not built.
