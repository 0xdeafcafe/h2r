# Live web: the dashboard, the stats site and the line between them

The web app (`web/`, live.forbes.red) has two faces:

- **the dashboard** (`/`, `/dash/...`, `web/src/dash`): the "Live" side, in
  the style of the original console dashboard: green light on black, the
  orb, glass pods, bevelled plates, blade lists;
- **the stats site** (`/stats/...`, `web/src/stats`): the game's record, in
  the style of the Halo 2 and Halo 3 era stats sites: a navy fixed column,
  blue title bars, dense tables.

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
2. **A live game's page is on the stats site.** `/stats/games/:id` is the
   same URL before and after the game ends, and after it ends it is the
   carnage report: the record. The dashboard's "live now" list links to it.
3. **Actions are the dashboard's.** A stats page can show that two players
   are friends or that a player is in a clan (public facts), but adding a
   friend, answering an invite, joining or running a team, and every Live
   admin action happen on the dashboard. A stats page links there.

### 1.1 Two admins

- **Halo Admin** (`/stats/admin`, stats site): the game's configuration.
  Playlists (hoppers), the message of the day, custom content and mods.
  It stays where it is, in the stats site's style.
- **Live admin** (`/dash/admin`, dashboard, in the dense style of §3): the
  Live service and its people. Accounts, messages, player reports (the
  report queue the voice work is building lands here), usage, and the raw
  event log. The server screens (`/dash/server`: invites, accounts,
  import) lead into it.

The line: if it changes what the game shows or plays, it is Halo Admin;
if it is about the service, the accounts on it or what they do, it is
Live admin.

### 1.2 Teams and clans

The protocol has one thing, Xbox Live Teams; the site has two faces of it.

- **Teams** are the dashboard's: the service feature. Creating a team,
  recruiting, answering, roles, leaving; the team list on your account.
- **Clans** are the stats site's: a team seen as a Halo record. Roster,
  combined numbers, standings. Clans get Halo features over time (ideas,
  not built):
  - a clan service record: games together, wins, kills, medals, maps;
  - clan versus clan history, from games where two clans' members met;
  - clan leaderboards by window, beside the player ones;
  - a clan's game history and "clan of the week";
  - the clan's emblem and colours on its members' carnage report rows.

### 1.3 The event log, twice

- **Stats: the game record's events** (`/stats/events`): a public view of
  only the events that make the record: games, players joining and
  leaving, results, kills, scores. It is the "show your working" for the
  carnage reports.
- **Dashboard: the raw log** (`/dash/admin/events`): every event, every
  stream, admin-only *in the web UI*.

The API does not change: `GET /v1/events` keeps its access rules (the
privacy redaction in `crates/live-server/src/sessions.rs` decides what
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
| Live now | `/dash/live` | stays | Live state; rows link to `/stats/games/:id`. |
| Find a game | `/dash/sessions`, `/dash/sessions/:id` | stays | Live state. |
| Friends | `/dash/friends` | stays | Your account, with actions. |
| Clans | `/dash/clans` | stays, renamed **teams** | The Xbox Live Teams feature: create, recruit, answer, roles (§1.2). A clan's record is the stats site's. |
| Players | `/dash/players`, `/dash/players/:tag` | stays | Presence ("who's online"). The player pod links to the service record. |
| Game history | `/dash/history` | stays, as a launcher | A list of recent games, each opening its stats page. It must not grow its own report. |
| Settings, profile, create account | `/dash/settings`, `/dash/signup` | stays | Your account. |
| Account and recovery, recover, sign in | `/dash/account`, `/dash/recover`, `/dash/signin` | stays | Your account. |
| Server overview | `/dash/server` | stays | Running the service. |
| Invites, accounts, import, connect | `/dash/server/{invites,accounts,import,connect}` | stays | Running the service. |
| Live admin: usage (new) | `/dash/admin`, `/dash/admin/usage` | new | Usage accounting (§5), in the dense style. |
| Live admin: accounts, messages, reports, raw event log | `/dash/admin/{accounts,messages,reports,events}` | to build | §1.1. Reports is the home for the player-report queue. |
| Dense style page (new) | `/dash/admin/style` | new | The dense style on fixed data (§3). |

### The stats site (`web/src/stats`)

| Page or part | Route | Verdict | Why |
|---|---|---|---|
| Home: leaderboards, tallies (24 hours, all time), of the week, maps, weapons, game types, skulls, activity heatmap, clan standings, recent games, Server Report | `/stats` | stays | The record, summed. |
| Home: "Live Now" live game cards | `/stats` | stays, as a teaser | They link to the live game pages (§1.2). Keep it to the cards. |
| Home: "Open Games" table (open sessions) | `/stats` | **moves** to the dashboard | Live state, and a copy of "find a game". Replace it with a count that links to `/dash/sessions`. |
| Header Welcome box: "Players Online (n)", "Friends Online (n)", "Games Live (n)" | all | stays, as links | One-line teasers. "Players Online" should open `/dash/players`, not `/stats/players`. |
| Games | `/stats/games` | stays | The record; its live tab links to live game pages. |
| A game, carnage report, campaign report | `/stats/games/:id` | stays | The record. |
| 3D replay window | `/stats/games/:id/3d` | stays | Part of a game's record. |
| Players directory and leaderboards | `/stats/players` | stays, **splits** | Leaderboards and finding a service record are the record. The online-first sort and presence column are live state: sort by name or rank here, and leave presence to `/dash/players`. |
| Service record, history, career, campaign | `/stats/players/:tag[/history,/career,/campaign]` | stays | The record. |
| Player page: Friends and Teams boxes (`PlayerSocial.tsx`) | `/stats/players/:tag` | **splits** | Clan membership and the friend list are public facts and can stay as links. Live presence on them and the add / accept / cancel friend button move to the dashboard (`/dash/players/:tag`); the stats page links there. |
| Clans directory, a clan's page | `/stats/clans`, `/stats/clans/:id` | stays, **splits** | Roster and combined numbers are the record and grow Halo features (§1.2). The team actions (join, leave, recruit, answer) move to the dashboard's teams screen; the clan page links there. |
| Event log | `/stats/events` | stays, **narrows** | Becomes the public view of the game record's events only (§1.3). The full raw log moves to `/dash/admin/events`, admin-only in the UI. |
| Halo Admin: message of the day, playlists, custom content | `/stats/admin` | stays | Game configuration (§1.1). |
| Nav tabs "Event Log", "Halo Admin", "Server" | all | stay | "Event Log" now opens the game events view; "Halo Admin" stays for admins; "Server" leaves for the dashboard. |
| Sign-in link redemption | `/signin` | neither | A one-shot utility; no chrome. |

### Shared components (`web/src/components`, `web/src/stats/clanBits.tsx`)

Emblem, map pictures, game type, medal, rank, skull and weapon icons,
the scoreboard, the kill feed and the map canvas are drawn on both sides
and stay shared. `clanBits.tsx` (clan emblem, presence words) is shared
too and could move to `components/` when it next changes.

### What moves, in order

1. Live admin with the usage screens (`/dash/admin/usage`), in the dense
   style. The "halo admin" pod on `/dash/server` stays; a "live admin" one
   joins it.
2. The raw event log to `/dash/admin/events`; `/stats/events` narrows to
   the game record's events.
3. The splits: open sessions off the stats home; presence, friend and
   team action buttons off the player and clan pages, with links across;
   the dashboard's clans screen becomes teams.
4. Live admin's accounts, messages and reports screens (reports once the
   report queue lands).

## 3. The dense dashboard style ("deck")

The console screens are a single 1280 × 720 picture scaled to the window,
with big type and a handful of rows. That is right for a menu and wrong
for an admin screen with thirty rows and four charts. The deck keeps the
dashboard's materials and drops the stage:

- **Same materials.** Orbitron for page titles (lower case, with the
  plate titles' glow), Barlow Condensed for everything else, the greens
  of the console screens, the select pill's yellow-green, the bevelled
  rim (light, dark, light) round a flat translucent green face, cut on the
  corners like the plates. The back control is the console's round "b".
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
| `DeckPage` | Trail, title, back (b, Escape, Backspace), the filter row, the grid. |
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
