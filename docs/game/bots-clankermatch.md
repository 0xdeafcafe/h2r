# Bots: Clankermatch

The matchmaking row, search and lobby (section 8). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 8. Clankermatch

### 8.1 What the player sees

**Clankermatch** is a new Xbox Live menu row, after OptiMatch, that plays
exactly like OptiMatch:

1. Pick a playlist.
2. The search runs, with the retail screen. A plausible delay: 4 to 20 s,
   drawn from the playlist's population.
3. The lobby fills with bots one by one, each with a gamertag, an emblem
   and a rank. Teams are balanced.
4. The match runs as a normal Live matchmade game: the same hopper
   playlists and variants, and the same carnage report.
5. Leaving ends the game.

The player's own console hosts. The bots join as phantom peers up to
the playlist's player limit.

### 8.2 Decided by the user

1. **Stats.** A Clankermatch game is its own kind: flagged in the events
   (`GameRecorded` carries `kind: "clankermatch"`) and in the read model,
   so it can be split out later. It counts in career totals. A game in a
   clanker playlist whose server mark says `ranked` also moves that
   playlist's own rank and leaderboard; unranked training games move none.
2. **Bot identities.** Bots get made-up gamertags, marked by a glyph that
   says "bot". We pick **U+2699 GEAR** (⚙). Common system fonts on the web carry
   it, and it is in the Basic Multilingual Plane, as Halo 2's UTF-16 names
   require (Repo: keyboard-charset.md). The game's fonts lack it; the
   keyboard's dingbats (star, flag, check, ballot x) are the only symbols
   close to it, and none of them reads as "bot". So we draw our own glyph
   and add it with `font::patch` (Repo: `crates/formats/font/src/patch.rs`, already
   used by `boot::icons` to add our icon glyphs at load). No disc glyph is
   copied.

   The gamertag itself stays inside the Live rules: ASCII letters, digits
   and single spaces, 1 to 15 characters, as `live_proto::Gamertag` checks.
   The glyph is not part of the account name. It is drawn before the name
   wherever a bot's name appears:
   - in the game, the host writes `⚙ ` plus the tag into the phantom's
     player configuration. The lobby, scoreboard and carnage report
     draw that name.
   - on the site, it is drawn from the server's bot flag.

   The server knows which accounts are bots (§9). Every client patches the
   font, since every client is our recomp. A box on a stray unpatched font
   slot is the failure mode.

   **Built (2026-11-19).** `font::gear` draws the gear in code (a wheel,
   a hole and eight teeth; six below 14 pixels), square, as tall as the
   font's 'H' above the baseline (seven tenths of the ascent without
   one), standing on the baseline with a one-pixel gap before it.
   `boot::bot_mark::GearRewrite` adds it to **every** file under
   `maps\fonts` as the VFS serves it, after the icon sets
   (`boot::icons`), unless the font has its own U+2699. The font-slot
   audit is therefore not needed for the mark: whichever slot a field
   uses (keyboard-charset.md, "Which font draws which field", still not
   traced), its file has the gear. A font the rewrite cannot patch (over
   the VFS rewrite limit, or one that does not parse) keeps a box there
   and says so with `H2_BOOT_VERBOSE=1`.

   The name budget: the configuration name holds 15 UTF-16 units and the
   NUL (`CONFIG_NAME_UNITS` 16). `bots::phantom::session::marked_name`
   writes the gear, a space and the tag; a 14-character tag loses the
   space (the glyph's own gap stays), a 15-character one also its last
   character. Host-made names are at most 14 ASCII characters and fit whole. The game record names a bot without the mark
   (`live_proto::bots::unmarked_units`, in `live-telemetry`'s player
   read), so the stats read model keeps the account's gamertag and the
   site draws its own mark from the bot flag. No player can type U+2699,
   so only a bot's name loses anything.
3. **Bot rank.** Bots are matched to the player's rank. The playlist sets
   bot skill from the human's level, so it is a fair fight. The level maps
   to the aim table's difficulty (§3.4) on a continuous scale, not just its
   four steps: reaction, error and lead are interpolated.

### 8.3 Menu row and playlists (traced, Repo; built)

**The row.** The Live menu's extra rows come from `custom_games.rs`'s
hooks: list count, add items, pane, row text `0x230f36` and row chosen
`0x230f92`. Clankermatch is row 2, right after OptiMatch:

| Row | What | Press |
|---|---|---|
| 0 | Quickmatch | retail |
| 1 | OptiMatch | retail |
| 2 | **Clankermatch** (ours, pane 4) | retail row 1's press, `clankermatch::active` set |
| 3 | Co-op Campaign (ours, pane 6; with Live), directly above Custom Games | co-op |
| 4 | Custom Games (ours, pane 7) | retail row 2's press, CUSTOM GAMES asked for |
| 5 | Forge (ours, pane 8; with `forge`) | retail row 2's press, FORGE asked for |
| 6 | Firefight (ours, pane 9; with `firefight`) | retail row 2's press, FIREFIGHT asked for |
| 7 | Custom Game Browser (ours, pane 5) | the Live browser |
| 8 | Mods (retail's Content Download, row 3, moved and renamed; pane 3) | Mods screen or the Map Pack dialog |

A row that is off leaves the list and the rows under it close up
(`custom_games::Menu::layout`): without the co-op row
(`H2_NO_COOP_ROW=1`) Custom Games is 3 and Mods 7. The lobby switches to
the mode a row asks for at its first panel update
([party-lobby.md](party-lobby.md), "The modes"). `h2 bots test` drives
Custom Games at row 4.

A moved retail row's press rewrites the press record's row (`[[esp+8]]`)
to its retail row and runs the original, as the co-op row does; after it
returns, the cursor row `0x510a18` is set to the row shown (the retail
press stored the retail row there). The browser's and co-op's presses
store their rows too (`custom_games::remember_row`, rows 0 to 7; Mods
opens over the menu and stores none). The menu restores its cursor only
to rows 0 to 3 (`0x231195`: `cmp eax,4; jge`), so for rows 4 to 7 a
`before` on the menu's build `0x00231170`
(`engine::points::LIVE_MENU_CURSOR`, vtable `0x00458b48` slot
`0x00458bec`, thiscall, no arguments, plain `ret`) does the build itself:
`[0x54e7bc] = -1`, `byte [0x54e7b8] = 0`, `0x22ee92(screen,
0x1901fc())`, `0x22f672(screen)`, then `0x24c591` (EAX the list at
screen `+0x818`, the row on the stack, `ret 4`) with the stored row
(`custom_games::LiveMenuCursor`, feature `live-menu-cursor`). Rows 0 to 3
and "none" (-1) go to the retail build. The co-op row's retail-lobby
press (which runs retail row 2's press, storing 2) now sets its own row
after it, as the moved rows do. Every Live menu press sets `clankermatch::active` (Clankermatch)
or clears it (any other row). `h2 bots test` and `h2 coop test --drive`
navigate by the new rows (`agent_proto::sim::menu`, `bots_drive`).

**The playlists (traced 2026-11-19, confirmed from the disassembly).**
OptiMatch's press (`0x231025`) opens the playlist screen through
`0x149f1e` with loader `0x00230c8d` (screen `0xdb`; vtable `0x00458c10`;
the list handler at `+0x614`, constructor `0x002b1e08`, vtable
`0x0045b138`, datum array "playlist saved game file list" `0x0045b190`).
Its add-items (`0x002b1ed5`) calls the item builder `0x002b1ff4` (EDI the
handler), which walks descriptor indices 0..16 and adds a row for index
`i` when:

1. `0x00192db0(i)` (ECX `i`, AL the answer, no stack arguments; the
   range, two gates, then a tail `jmp` to the validity check
   `0x001934f0`) is true; and
2. `0x00194610(i)` (list whatever the rank) or the player's rank is at
   least `0x001945c0(i)`; and `0x001943d0(i)` loads the hopper's block.

Each row's datum keeps `i` at `+4` (`0x002b20cb: mov [eax+4],ebx`). The
press (`0x002b2149`) reads it back (`mov edi,[eax+4]`) and starts
matchmaking through `0x002b2181`; the row text (`0x002b1f5f`) and the
initial cursor (`0x002b1eef`, the hopper at `0x00510990`) read it too. So
rows are compacted but never renumbered into indices: skipping a hopper
leaves no hole and breaks no mapping.

**The hook** is a `before` on `0x00192db0` (`engine::points::HOPPER_LISTED`;
its only caller is `0x002b2064`, return address `0x002b2069`):
`clankermatch::listed(i, marks, active)` is "the hopper is a clanker
hopper exactly when the Clankermatch row opened the screen"; a hopper of
the other kind gets AL 0 at once, the rest go to the game's own check.
The marks are the server's (`GET /v1/hoppers/clankers`, §9.4 item 1),
which the HLE asks for at each logon. With no marks (an older server) the
OptiMatch list is retail and Clankermatch's is empty.

Other readers stay unfiltered: the best-skill pass `0x00065530` and the
stats rank cache use `0x00192df0`/`0x001934f0` directly.

**Quickmatch (built).** Quickmatch (row 0, `0x00231045`) starts the
hopper `0x001902de` answers (ECX the local player; `[0x54e9dc +
player*0xc70]`, or -1 when flag bit 4 of `[0x54e8e0 + player*0xc70]` is
clear; its one caller `0x00231056`), through `0x001943d0` at
`0x00231066`, when the saved hopper is usable. The Live menu opens the playlist picker for an unset, out-of-range or Clanker selection. A playlist press
saves its hopper there (`0x002b21e3`: `push edi` (the hopper), `push
[ebp+0Ch]` (the player), `call 0x001902fc`, `ret 8`; its only caller).
Two hooks keep Quickmatch on human hoppers
(`clankermatch::PlaylistFilter`):

- a `before` on `0x001902fc` (`engine::points::PLAYLIST_REMEMBER`)
  returns at once (`ret 8`, nothing saved) during Clankermatch or for a
  clanker hopper (`clankermatch::remembered`), so the human playlist
  picked before stays;
- an `after` on `0x001902de` (`engine::points::QUICKMATCH_HOPPER`)
  answers -1 for a saved clanker hopper (a profile saved before the
  server marked it; `clankermatch::quickmatch_hopper`).

With no marks (an older server) both are retail.

**Switch To: Clankermatch (built 2026-11-19, the user's ask).** The
playlist screen's second opener (`0x002bbf06`, from `0x002bba93`) is the
lobby's **"Switch To: Matchmaking"**: a Game Setup (squad settings)
row whose press, `0x002bba8c` (EAX the press's player chain, the list
handler on the stack, `ret 4`), opens the playlist screen for the
chain's player and closes the dialog (`0x0022eeee`, `0x0022e957`). The
list constructor (`0x002bb4d2`) gives it action 8 in lobby kinds 4 (2,
3, 7, 8) and 5 (Change Map, Change Rules, Quick Options, Switch To:
Co-Op Game, Switch To: Matchmaking), and action 9 in kind 6 (9, 6); the
row-chosen jump table (`0x002bb884`) sends 8 and 9 to the same press.
`clankermatch::SwitchRow` (feature `clankermatch-switch`) adds **"Switch
To: Clankermatch"** (action 12, `clankermatch::SWITCH_ACTION`, past the
Halo 1 Options row's 10 and the Clankers row's 11) right under it, while
the server has clanker hoppers (`clankermatch::wants_switch_row`,
`switch_placed`), drawn with `custom_games::SWITCH_CLANKERMATCH_TEXT`
(string id `ID_BASE + 0x3b`, the Live menu rows' family). Its press sets
`clankermatch::active` and calls `0x002bba8c` itself, so the party goes
to the clanker playlists and on to the empty search and the fill (§8.4),
as the Live menu's row does. Retail's Switch To: Matchmaking now clears
`active` first (`clankermatch::switch_press`), so it always lists the
human playlists, whatever the Live menu chose last. The log says
`clankermatch: Game Setup list: Switch To: Clankermatch row added (N
rows)` at each Game Setup, then `clankermatch: Switch To: Clankermatch:
opening the clanker playlists` and `clankermatch: the playlists are the
clanker hoppers` at the press. The party lobby's Game Setup then drops
every Switch To row, this one with retail's (`lobby_mode::arrange`, the
constructor's outermost `after`, so it runs once this row is added):
the GAME MODE picker changes mode, and its MULTIPLAYER mode is the one
matchmaking lobby. There, action 9 reads Change Playlist... and lists
the clanker and the human hoppers on one page (`clankermatch::Scope::All`),
so the playlist picked is the Clanker or non-Clanker choice
([party-lobby.md](party-lobby.md#the-modes)).

### 8.4 Search and lobby (built: the empty search)

The game searches through `XOnlineMatchSearch`, which is ours in live-hle.
For a clanker hopper (`live_hle::clankermatch`):

1. The search returns no sessions after the delay, a uniform 4 to 20 s
   (not yet drawn from the playlist's population). The game widens and
   searches again after each answer (live-matchmaking.md §7.3), so the
   delay is the search run's: the first call of a run draws it, every
   call answers no sooner than the run's start plus it, and a call more
   than 10 s after the last answer (or of another hopper) starts a new
   run. The game then hosts, as OptiMatch does when a search finds
   nothing. The log says `live: clankermatch: searching clanker playlist
   P: no one to find; the console hosts in N s`.
2. The host's Clankers driver sees it host a clanker hopper's lobby
   (`Hle::clanker_hosted`: the advertised session's hopper is marked) and
   fills it (`clankers::Driver::tick_match`, twice a second): it asks
   Live for one roster of the whole lobby (`POST /v1/games/bots` with the
   clanker hopper's index as the playlist (`Rosters::playlist`, which
   falls back to `Hle::clanker_hosted` when no server match names one),
   no skill, so the server sets skill from the party's level, §8.2.3;
   `teams: true` in a team playlist) and seats the bots one at a
   time on `clankermatch::schedule` (each 0.5 to 3 s after the last,
   seeded by the session id), up to the playlist's most players less the
   party. The party is the squad session's player count
   (`bots::lobby::party_players`, `+0x1118`: every console of the party
   and its guests, never a phantom), read twice a second by the frame
   hook (`Driver::set_party`) and followed until the first bot sits; one
   person while it cannot be read. Team Skirmish is three bots for one
   person and two for a party of two: a bot for every human but the host
   would take the seat of the party's second console, whose join the full
   session then refuses. The log says `clankermatch: 4 seats, the party
   of 2 people, 2 bots` (or `the party of N people fills clanker playlist
   P's N seats: no bots`). Each bot joins the match's target session as a phantom peer
   with its rostered account and skill (§5.1, "Phantoms in the pregame
   lobby"). The driver waits for the roster before seating any bot.
   If Live refuses the roster, the host supplies the identities.
   In a team playlist each phantom without a roster team goes to the team
   with the fewest players, then the lowest total skill
   (`clankermatch::balance`), around the party, all of it on the host's
   team (a party keeps together): the host's first
   local listed player's configuration team (player `+0xd4 + 0x7c`,
   `bots::phantom::session::host_team`), read twice a second by the bots'
   frame hook (`clankers::frame`) and followed until the first bot sits,
   then kept so no bot changes team; 0 when unknown or not one of the
   playlist's teams. The frame hook now runs this with no bot in (it used
   to return first, so a Clankermatch with the Clankers page off never
   started its fill).
3. The match starts on the hopper's normal countdown once the phantoms
   bring it to the required player count (below, "Tracing the
   matchmaking lobby"). A lobby that has not started 20 s after its last
   bot was due says why (`Driver::stalled`, reading
   `bots::lobby::match_wait`: the host update's checks in its order),
   once per reason and again every 30 s while it holds: `clankers:
   clankermatch: clanker playlist P's match has not started N s after its
   last bot was due: <reason> [players 3/4, roster 3/4 present, peers 3
   (bots 2), maps [..], hopper statuses [..], arbitration [..], round
   none, matchmaking result 0x3, arbitration state 0x0]`. The reasons:
   the match session short of players, the matchmaking roster's players
   not all in the session, a peer's hopper status not 1, a peer that has
   not loaded the map, no arbitration round, a failed or pending
   arbitration registration, this console not hosting the match lobby,
   or Live not having answered the roster; "every traced check reads
   ready" when none holds. A handoff (the lobby's session deleted and the game's
   advertised) keeps the bots and carries the roster (§9.5); with no
   hosted session for 30 s, or another kind of session hosted, the fill
   ends and the phantoms leave. The game deletes the lobby's
   advertisement once its match session takes over, before the
   countdown ends, so the 30 s count only while this console does not
   serve a multiplayer game (its simulation world the distributed
   server, world mode 4: `bots::phantom::session::serves_game`,
   `clankers::Driver::set_hosts_game`; the game options' own mode is 2
   for every multiplayer game): the bots stay for the whole
   match and go 30 s after it. Before the game is first served, the
   match session this console hosts counts as the game
   (`bots::phantom::session::hosts_match`, `clankers::Driver::set_holds_match`:
   the matchmaking life cycle, types 6 to 9, its target session with a
   state, distributed, this console its host). Arbitration takes that
   session from live (5) to state 7 while the map loads, which can last
   longer than 30 s; the phantoms wait there as its peers ("is still in
   the session ... it stays") and the fill holds, so the game loads with
   them. Once the game has been served the held session no longer
   counts, and the bots go 30 s after the game whether it lingers or not.
4. The game record says `kind: "clankermatch"` with the playlist and
   session (`telemetry::Playlist::clankers`), and the server keeps it as
   its own kind (live-matchmaking.md §6.8).

The lobby's "searching for players" state is the game's own: it stays until
the session is full.

The initial implementation joined phantoms only during gameplay.
That left the lobby below the playlist minimum and kept the countdown
from starting. The current lobby join path adds the phantoms to the
target session before the countdown. It also runs with the Clankers
page disabled.

**Tracing the matchmaking lobby (2026-11-19, from the disassembly; no
run yet).**

- **The lobby is a pregame lobby.** A playlist press (`0x002b2149` →
  `0x002b21b9`) creates the squad session as the Live custom game row
  does (`0x59890(2, 0, 0)`), then sets lobby mode 3 (`0x199a03(3)`) and
  `0x63fb0`. The start state (`0x19a2ce`) tests "a pregame session"
  (`0x19a279` gives 3 for life-cycle types 1 and 2) **before** its
  matchmaking branch (`0x199971`: lobby kind 6, session `+0x49a4` 2), so
  the party's lobby passes it before the search. Once the search hosts,
  the life cycle is 6 and the lobby is the target session (cm-run2,
  below).
- **What the start counts.** The matchmaking branch's state comes from
  `0x66050(session, hopper)` (through `0x63e90`, hopper `+0x49c8` via
  `0x641f0`), mapped to the status texts by `0x19a5fd`. It reads, per
  session player (mask `S+0x111c`, records `S+0x1120`, `0x13c` each): an
  offline id (`0xBAD00000` at record `+8`) gives state 13; in ranked types
  (2, 4, 5) a guest flag (`+0x10dc & 3` from the record base) gives 12; a
  level byte (`+0x129`) under the hopper's `+0x5e4` gives 15. Per peer it
  wants the peer's hopper status `[peer + 0xac + hopper*4]` (peers at
  `S+0x58`, `0x10c` each) to be 1: 0 gives 14, 3 gives 2, anything else
  11. Then the party: the session's player count `S+0x1118` against the
  hopper's party bounds (types 1 to 3: at least 1; 4 and up `+0x5fc` or
  `+0x5f0`; at most `0x193250`), 6 when short. The match check
  (`0x7e2df`, in the lobby summary `0x7e210`) compares the lobby's player
  count with the hopper's fewest (`0x1932c0`) and most (`0x193300`), as
  `0x7067a` and `0x724a6` do for the matchmaking states. Phantoms
  count once they have joined the target session.
- **`S+0x4980` is the squad's matchmaking state**, set only through
  `0x5a220(state)`: 1, 2, 4, 5 around the lobby and the game (`0x6e596`
  and `0x727e1` set 4 as a game starts), 6 to `0x12` in matchmaking
  (`0x705f0` reads 11 to 13 as the searching states). So the phantom
  check's "4" holds in any running game, a matchmade one too: a
  Clankermatch's phantoms join its game like a custom game's.
- **`0x5f970` is no ready check**: it works on one peer's channel entry
  (`S+0x72dc + peer*0x14`: `+1` valid, `+2` connected, `+4` the
  channel) in states 5 to 8 and walks the other peers' entries; a
  phantom's (valid, not connected, channel -1) is what the host's join
  writes. `[S+0x78a8]` is set to the global `0x527fe8` at the session's
  setup (`0x58f07`); what it refuses is not traced.
- **Phantoms in the lobby: built** (§5.1, "Phantoms in the pregame
  lobby"). A phantom's peer carries this console's own properties, so its
  hopper status is the host's (1) and `0x66050` passes; its map status is
  the host's, so the countdown's end (`0x6e410`) does too. `S+0x4980` is
  not read for the lobby join (it matters only to the in-game player
  diff, and is 4 once the game runs). The fill's first line now says `N
  phantoms join the lobby as peers`.
- **To see in a run:** whether the game changes team assignments when
  a matchmade team game starts.

**The matchmaking host's lobby (traced 2026-11-19 from the user's
Clankermatch, cm-run2; built, no run yet).** The search hosted (`session
893954eb98f2e2fe advertised`) and the log said `bots: lobby facts: lobby
true, networking true, session type 6, session state 5: no pregame
lobby`; every local bot's gate said `screen 0xd2 (top 0xd2), dialog
none, signed_in false (slot 0x1): waits`, no phantom joined, and the
lobby never started.

- **Two sessions.** Beside the squad session `[0x527364]` (the party's,
  whose Live session `84ad6bf112ae91df` was advertised when the playlist
  was pressed) the game keeps a target session `[0x52736c]`: the
  session a matchmaking search joins or hosts.
  `0x65340` picks the session the game plays in by the life-cycle type
  `[0x527334]` (table `0x65520`): types 1 to 4 the squad session, 6 to 9
  the target session, 5 none. `0x59780` prefers the target session
  whenever it has a state; `0x593e0` tears both down.
- **The life cycle.** `[0x527334]` indexes ten state objects
  (`[0x527338 + 4·type]`, set up by `0x58f22`): type 6 is the object at
  `0x527500` (`0x70190`, the matchmaking one: its timers, `0x705f0`'s
  searching states). `0x19a279` maps the types to lobby kinds: 1 and 2
  give 3 (pregame), 6 and 7 give 2, 3 and 8 give 4 (a game: the in-game
  player diff `0x83220` runs only for types 3 and 8), 4 and 9 give 5, 5
  gives 7. So 6 is the hosted matchmaking lobby, 8 its game; the custom
  game's are 1 or 2 and 3.
- **The screen.** `0xd2` (constructor `0x2519f7`, vtable `0x45a398`)
  is the matchmaking lobby's. Its input `0x251afa` takes B and Back
  (buttons 1 and 13, `0x25217b`) and X (3), nothing else: a second
  controller's A or Start does nothing there. The custom lobby's join
  press (`0x250c80`) needs `0x19a279` 3 and refuses in a countdown or in
  types 2 and 3 (`0x19a902`), so on retail a guest joins a matchmaking
  party in its lobby before the search, never after it.
- **But the session adds any local user.** The per-frame pass `0x63d80`
  runs `0x65770` on every session with a state (the three at
  `[0x4ce03c]`), the target session too, and `0x646b0` and `0x65a60` read
  the session's state (3 to 8) and room (`+0x54` against `+0x4990`,
  `+0x1118`+1 against `+0x4994`), never the type. A bot signed in while
  the target session is up is added to it like any local user, as in the
  custom lobby. The party's squad session is in state 5 too, so the pass
  adds it there as well, as it would a guest signed in before the search.
- **Built.** `bots::lobby::game_session` mirrors `0x65340` (types 6 to 9
  the target session, else the squad's). `Facts` reads the target
  session's state and host in types 6 and 7, and
  `lobby::matchmaking_host` is a matchmaking lobby this console hosts
  (type 6 or 7, the target session in a lobby state, its host peer this
  console's). (The join gate opened there for local bots for a time;
  "Phantoms only" below closed it.) The
  phantoms' `host_lobby`, `live_session`, `owned_session` and the in-game
  `host_session` read the game session too, so a Clankermatch's phantoms
  join the target session (its hopper statuses are the host's, so
  `0x66050` passes them) and stay in it for the matchmade game (type 8).
  A matchmaking lobby another console hosts, or a search with no target
  session yet, takes none. The facts line says it: `bots: lobby facts:
  lobby true, networking true, session type 6, session state 5, match
  session state 5 (this console hosts it): a matchmaking lobby this
  console hosts`. `bots: lobby players: N in the game's session` says
  the target session's player count (`+0x1118`) on each change: what the
  matchmaking start counts.
- **The re-search every ~4 minutes** is the game's: the hopper's search
  timeout (`+0x59c`, read at `0x71c7a` against the time the lobby was
  hosted; 150 000 ms in the small playlists) runs out on a lobby that
  never reached its start, the game deletes its target session, searches
  again and hosts a new one (`80f478...`, then `86334c...`). The fill
  already follows it (`the game moved from session ... asking Live to
  carry its bots there`). With the bots in the target session the lobby
  reaches its fewest players within the fill's 15 s or so, far inside the
  timeout. At the hopper's maximum, `0x1931d0` gives a zero start wait.
  The host update (`0x71730`) compares the target membership count and IDs
  to its expected matchmaking roster (`+0x4a00`), then checks each peer's
  map status (`0x6e0f0`) and arbitration status (`0x72140`). For a full
  lobby that returns to the party, `live.matchmaking.trace` retains these
  inputs, the guest matchmaking result and the target key release.
  [Agent API](../runtime/agent-api.md#matchmaking-transitions)
- **Arbitration starts when the lobby fills.** Only a full, map-ready
  lobby reaches `0x71ac2`: it makes the round (`0x73ae0`,
  `XOnlineArbitrationCreateRoundID`) and sets it on the target
  (`0x5c1c0`, `+0x49e8`). The update's every-frame `0x72170` then
  registers it (`0x73bc0` → `0x924e0` → `XOnlineArbitrationRegister`), and
  `0x755d0` polls the task: success sets the arbitration state
  (`0x510518`) to 2, a failure calls `0x73dc0(1)` (state 0, result 1).
  This console's peer status (`0x73b10`, peer `+0xec`) reads 1 while
  registering, 2 registered, 5 after that failure. The phantoms copy
  it with the rest of the host's properties, so `0x72140` passes when
  the host's own registration does. `0x72260` checks the target peers
  that are also in the party (this console's own): a status above 2
  sets the matchmaking result to `0x11`, and `0x706f0` (`0x70932`)
  sends any result but 2 and 3 back to the party lobby (life cycle 1)
  with MATCHMAKING FAILED. The target key release and the session
  delete follow. Nothing before the fill registers, so a full
  Clankermatch lobby that drops to the party within a second of filling
  most likely took this branch; the logged result says which.
  The Live HLE logs each refused arbitration call (`live: arbitration
  <session>/<round>/register for <gamertag> failed: ...`) and the game
  logs the result (`live: matchmaking ended with result 0x11: this
  console's arbitration registration failed ...`). Registration and
  report retry through an outage, because the server writes a second
  one as nothing (live-matchmaking.md, "Arbitration (built)").
- **Live.** The match's bots went to the party's Live session too, and
  each was refused there (§9.5): now only the roster's session is told.

The matchmaking progress screen's countdown (`0x0f00188e`, token U+E45B)
reads the target session through `0x59590`. It shows **Setting up bots…**
at zero under the same host setup conditions as the custom lobby.
`lobby_status::MatchCountdown` replaces the temporary text used to
measure and draw the widget at `0x22d2ee`.

**Phantoms only.** In the 2026-11-19 run on `f5d3010c`, the retired
controller-backed bots appeared in the player's party. The local-player
pass (`0x63d80` → `0x65770`) put those signed-in users into both sessions.
The matchmade start then refused to proceed because their controller
ports had no device.

Clankermatch sets `Phantoms::set_match_only(true)` so its phantoms
join matchmaking life-cycle types 6 to 9 and wait while the party's
lobby is up. They appear in the target session with their rostered
gamertags, emblems and ranks. The flag clears when the fill ends.

**Grep the run's log** (`h2 play ... 2>&1 | tee run.log`), in order:

1. `live: clankermatch: clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]`
   at logon (or `the server sent no clanker hoppers` / `no clanker hoppers
   from the server`): the marks.
2. `clankermatch: the playlists are the clanker hoppers`: the row's (or
   the switch's) press.
3. `live: clankermatch: searching clanker playlist 8: no one to find; the
   console hosts in N s`: the held search. Missing: the search was not
   the clanker hopper's (check `live: optimatch` lines).
4. `live: session <id> advertised for ...` after it: the console hosted
   a new session. (The game may update a session it already advertises
   instead, which says nothing; step 5 decides.) Neither: the game never
   hosted after the empty search (live-matchmaking.md §7.3 says
   "likely").
5. `clankers: clankermatch: hosting clanker playlist 8 (session <id>): 7
   bots join over N s`: the fill started. Missing with 4 present: the
   advertisement carries no hopper attribute (`Hle::clanker_hosted`).
6. `clankers: asking Live for 7 bots for session <id>`, then `clankers:
   Live rostered bots for session <id>` (or `no roster from Live (...):
   host-made bots`).
7. `clankers: <gamertag> (phantom, rostered) on (...)` for each rostered
   phantom. The lobby facts should identify the matchmaking target
   session this console hosts. `bots: lobby players: N in the game's
   session` reports its growing player count.
8. `bots: phantom <gamertag> joins (peer N, in the pregame lobby)` in
   the lobby, before the countdown. `bots: phantom ... waits: not a lobby
   it can join (...)` names why a lobby refused one, `... waits: this
   console is not the game's host yet (...)` why a running game did.
9. `clankers: clankermatch: clanker playlist P's match has not started
   N s after its last bot was due: ...` (item 3 above): the lobby filled
   as far as it could and the game did not start; the reason names the
   check that holds it. `live: matchmaking ended with result 0x..: ...`
   says a match that failed back to the party, and `live: arbitration
   <session>/<round>/register for <gamertag> failed: ...` a refused
   registration.

**Run:** whether the game hosts after an empty search in every playlist
type, or needs the server's matcher to say "you host" (`MatchProposed`
with `host = me`). **Design:** use `MatchProposed` with `host = me`, and
with the bots as members, when the matcher exists (the sbmm-ranked lane).
The empty search is what is built now.

**Tests (no game):** `cargo test -p boot --lib custom_games` (the rows,
panes, texts, cursor rows), `-p boot --lib coop_live` (the seven-row
menu), `-p boot --lib clankermatch` (the filter, the schedule, the
balance, Quickmatch's hopper, the Switch To: Clankermatch row's place,
press and plan order), `-p game --lib clankers::tests::a_clankermatch`
(the fill: one roster, phantoms only, one bot at a time, no controller
port, balanced teams around the host, the handoff, the end, a refused
roster, the match-only flag, Team Skirmish for a party of two, the
party followed until the first bot sits, the stall line;
`a_full_party_plays_a_clankermatch_without_bots`), `-p boot --lib
bot_mark`, `-p font gear` (the glyph on a synthetic font),
`-p bots phantom::session` (the marked name within 15 units, the host's
team; `phantom::session::tests::lobby`: the lobby this console hosts, the
reservations, the peer properties, the configuration, the teams, the
switch, and a phantom joining the lobby, staying for the game, leaving,
rejoining and waiting; `phantom::session::tests::in_game`: the start's
wait without phantom machines and a phantom standing in the game;
`lobby::a_clankermatch_...`: the target session),
`-p bots --lib lobby` (the game session by life cycle, the matchmaking
host's lobby, its player count, the party's count; `lobby::match_wait`:
each stall reason in the host update's order), `-p live-hle --lib players_tests` (the
bots told to their roster's session only), `-p live-hle --lib clankermatch` (the delay and the run),
`-p live-hle --test server a_clanker_doubles_lobby` (a Doubles match
session read as 2 v 2 and its round registered for both consoles of a
party),
`-p live-proto bots hopper::clankers`, `-p live-telemetry
games_say_their_kind a_bots_name_reads_without_its_mark`, `-p cli
bots_drive coop_drive`, and the server's: `-p live-server --test
clankermatch` (Team Skirmish from roster to ranked game for a party
of one and of two), `-p live-worker a_clankermatch_game`.

### 8.5 Game run (checklist)

`h2 play --live-local` (or a Live server with the new default hoppers),
signed in on port 0, Clankers page off. `h2 bots test --clankermatch`
(§13.1) covers the lobby and match transitions. Observe the in-game
actions in step 8 separately.

1. The Xbox Live menu has nine rows (eight without co-op): Quickmatch,
   OptiMatch, Clankermatch, Co-op Campaign, Custom Games, Forge,
   Firefight, Custom Game Browser, Mods. Seven show above the hints and
   the list scrolls to the rest; each shows its own help pane
   (Clankermatch: "Matchmaking against bots...").
2. Custom Games opens the custom game lobby; back lands on Custom
   Games. Forge and Firefight open it in their mode. Custom Game Browser
   opens as before, and back from it lands on Custom Game Browser
   (`custom games: opening the browser`); the same for Co-op Campaign.
   Mods opens as Content Download did.
3. OptiMatch lists human and Clanker playlists in descriptor order.
4. Clankermatch lists Clanker Rumble, Clanker Slayer, Clanker Doubles, Clanker Snipers,
   Clanker BTB, BTB Heavies, Solo Training and Team Training (the
   log: `clankermatch: the playlists are the clanker hoppers`; at logon
   `live: clankermatch: clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]`).
5. Pick Clanker Rumble: the retail search screen runs for 4 to 20 s
   (`live: clankermatch: searching clanker playlist 8 ...`), then the
   game hosts its own lobby (does it? in every type: FFA and teams).
6. The match lobby fills with rostered phantoms (seven for Slayer), the
   countdown starts, and port 0 remains the only local user. Check each
   gear and gamertag in the lobby, scoreboard and carnage report.
7. In Team Slayer check 4 v 4, with phantoms on both teams. In Team
   Skirmish check 2 v 2 with three phantoms. Returning to the party should
   leave only the human there.
8. Let the match start on its own. Check the carnage report lists the
   bots and the site shows the Clankermatch playlist and BOT marks.
   Follow one phantom by gamertag for the gameplay checks below. For
   tick-level movement use the existing agent API's player physics and
   trace views (§3.8).
   - At a jump or obstruction, watch the phantom leave the ground. On a
     crouch approach or jump, watch it lower and stand again. With Taunts
     enabled and a qualifying death nearby, look for distinct crouches
     and releases over the body (§3.5).
   - At close range watch a melee connect. At grenade range look for an
     actual throw and a decrease in the selected grenade count. With both
     types stocked, check a swap selects the other type once.
   - With a partly empty magazine and reserve ammo, watch X reload. Give
     it two weapons and watch a Y tap complete one switch, including the
     plasma-pistol release and rifle follow-up of the noob combo. To check
     zoom and flashlight directly, add a scripted phantom in a hosted
     custom game with `sim.player.add`. Send right-stick clicks on a scoped
     weapon and White presses where a flashlight is available with
     `sim.player.pad`. See the [agent API](../runtime/agent-api.md).
   - Put a pickup, an empty vehicle and a slowly moving enemy vehicle in
     reach in separate attempts. Watch held X take the pickup, enter the
     free seat and board the occupied vehicle. Check the bot's unit parent
     and seat after entry (§11.4).
9. Leave after the game: the bots go within 30 s (`clankermatch: the game
   is over: the bots go`). Back on the Live menu, Quickmatch starts the
   human playlist picked last in OptiMatch, or opens its playlist picker if none
   (`clankermatch: hopper 8 not saved for Quickmatch` at the pick).
10. With a server that has no clanker marks: OptiMatch as retail,
    Clankermatch's list empty (say how the empty list looks).
11. In a Live lobby on MULTIPLAYER, open Game Setup: Change Playlist...
    is its one row, with no Switch To: Clankermatch (the log:
    `clankermatch: Game Setup list: Switch To: Clankermatch row added`,
    then `lobby mode: Game Setup list in Multiplayer (1 rows)`). Change
    Playlist... lists the human and the clanker playlists on one page.
    On CUSTOM GAMES, Game Setup has no Switch To row either.
