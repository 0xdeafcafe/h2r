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

   **Built.** `font::gear` draws the gear in code (a wheel,
   a hole and eight teeth; six below 14 pixels), square, as tall as the
   font's 'H' above the baseline (seven tenths of the ascent without
   one), standing on the baseline with a one-pixel gap before it.
   `boot::bot_mark::GearRewrite` adds it to **every** file under
   `maps\fonts` as the VFS serves it, after the icon sets
   (`boot::icons`), unless the font has its own U+2699. The font-slot
   audit is therefore not needed for the mark: whichever slot a field
   uses (keyboard-charset.md, "Which font draws which field", still not
   traced), its file has the gear. A font the rewrite cannot patch (one
   that does not parse) keeps a box there and says so with
   `H2_BOOT_VERBOSE=1`.

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

A moved retail row's press rewrites the press record's row (`[[esp+8]]`) to
its retail row and runs the original, as the co-op row does; after it
returns, the cursor row `0x510a18` is set to the row shown (the retail press
stored the retail row there). The browser's and co-op's presses store their
rows too (`custom_games::remember_row`, rows 0 to 7; Mods opens over the
menu and stores none). The menu restores its cursor only to rows 0 to 3
(`0x231195`: `cmp eax,4; jge`), so for rows 4 to 7 a `before` on the menu's
build `0x00231170` (`engine::points::LIVE_MENU_CURSOR`, vtable `0x00458b48`
slot `0x00458bec`, thiscall, no arguments, plain `ret`) does the build
itself: `[0x54e7bc] = -1`, `byte [0x54e7b8] = 0`, `0x22ee92(screen,
0x1901fc())`, `0x22f672(screen)`, then `0x24c591` (EAX the list at screen
`+0x818`, the row on the stack, `ret 4`) with the stored row
(`custom_games::LiveMenuCursor`, feature `live-menu-cursor`).

Rows 0 to 3 and "none" (-1) go to the retail build. The co-op row's
retail-lobby press (which runs retail row 2's press, storing 2) now sets its
own row after it, as the moved rows do. Every Live menu press sets
`clankermatch::active` (Clankermatch) or clears it (any other row). `h2 bots
test` and `h2 coop test --drive` navigate by the new rows
(`agent_proto::sim::menu`, `bots_drive`).

**The playlists (traced, confirmed from the disassembly).**
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

**Switch To: Clankermatch (built).** The
playlist screen's second opener (`0x002bbf06`, from `0x002bba93`) is the
lobby's **"Switch To: Matchmaking"**: a Game Setup (squad settings) row
whose press, `0x002bba8c` (EAX the press's player chain, the list handler on
the stack, `ret 4`), opens the playlist screen for the chain's player and
closes the dialog (`0x0022eeee`, `0x0022e957`). The list constructor
(`0x002bb4d2`) gives it action 8 in lobby kinds 4 (2, 3, 7, 8) and 5 (Change
Map, Change Rules, Quick Options, Switch To: Co-Op Game, Switch To:
Matchmaking), and action 9 in kind 6 (9, 6); the row-chosen jump table
(`0x002bb884`) sends 8 and 9 to the same press. `clankermatch::SwitchRow`
(feature `clankermatch-switch`) adds **"Switch To: Clankermatch"** (action
12, `clankermatch::SWITCH_ACTION`, past the Halo 1 Options row's 10 and the
Clankers row's 11) right under it, while the server has clanker hoppers
(`clankermatch::wants_switch_row`, `switch_placed`), drawn with
`custom_games::SWITCH_CLANKERMATCH_TEXT` (string id `ID_BASE + 0x3b`, the
Live menu rows' family). Its press sets `clankermatch::active` and calls
`0x002bba8c` itself, so the party goes to the clanker playlists and on to
the empty search and the fill (§8.4), as the Live menu's row does.

Retail's Switch To: Matchmaking now clears `active` first
(`clankermatch::switch_press`), so it always lists the human playlists,
whatever the Live menu chose last. The log says `clankermatch: Game Setup
list: Switch To: Clankermatch row added (N rows)` at each Game Setup, then
`clankermatch: Switch To: Clankermatch: opening the clanker playlists` and
`clankermatch: the playlists are the clanker hoppers` at the press.

The party lobby's Game Setup then drops every Switch To row, this one with
retail's (`lobby_mode::arrange`, the constructor's outermost `after`, so it
runs once this row is added): the GAME MODE picker changes mode, and its
MULTIPLAYER mode is the one matchmaking lobby. There, action 9 reads Change
Playlist... and lists the clanker and the human hoppers on one page
(`clankermatch::Scope::All`), so the playlist picked is the Clanker or
non-Clanker choice ([party-lobby.md](party-lobby.md#the-modes)).

## Where the sections live

| Page | Covers |
|---|---|
| [bots-clankermatch-search.md](bots-clankermatch-search.md) | the empty search, the lobby and the game-run checklist (8.4, 8.5) |

Each heading below is cited from code and moved to the page named after it.

### 8.4 Search and lobby (built: the empty search)

Moved to [bots-clankermatch-search.md](bots-clankermatch-search.md).

### 8.5 Game run (checklist)

Moved to [bots-clankermatch-search.md](bots-clankermatch-search.md).
