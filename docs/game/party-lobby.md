# The party lobby's modes

Status: Built: the header names the lobby's mode, X opens the GAME MODE picker for the party leader, and a pick switches among MULTIPLAYER, CUSTOM GAMES and FORGE (CO-OP is reached from the Xbox Live menu and Split Screen's co-op lobby). The prompt line's tag edit is not built: the line shows only on a lobby tag that has the block ("The prompt line"). Slice T6 (the host's mode on Xbox Live) is design.
Code: `crates/extensions/game/src/lobby_tabs.rs` and `lobby_tabs/` (the modes, the header, the picker, the switch), `crates/extensions/game/src/lobby_mode.rs` (Game Setup's rows by mode, the start refusal), `crates/extensions/game/src/lobby_start.rs` (`Mode`), `crates/extensions/game/src/lobby_status.rs` (the reason line), `crates/extensions/game/src/clankermatch.rs` (the Clanker playlists), `crates/extensions/game/src/clankers.rs` (`admits_page`, the Clankers by mode), `crates/extensions/game/src/custom_games.rs` (the Xbox Live menu's mode rows). Tests: named per slice below.
Switch: none. The modes are how the lobby works: the `party-lobby` feature is `always_on`, so `-party-lobby` stops boot with the reason ([feature-engine.md](../engine/feature-engine.md#switches)). FORGE follows the `forge` switch ([switches.md](../runtime/switches.md)).

The pregame lobby (`pregame_lobby`, screen 14) is in one of five modes:
MULTIPLAYER, CUSTOM GAMES, FORGE and CO-OP. Its header names
the mode. The party leader presses X to open the GAME MODE picker, and
the lobby becomes the mode picked at once, with the same party in it.
The picker replaces Game Setup's Change Mode... and Switch To rows. The
one rule: **a mode is a name for lobby state the game already has;
nothing here stores a mode of its own.** Each mode is a retail lobby
mode plus, for the custom lobby's three, a mode of ours, and the lobby's
mode is read back from that state on every machine.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### The modes

A mode is two parts (`lobby_tabs::Tab::parts`): the retail lobby mode
(`LOBBY_MODE_SET`: 0 co-op, 2 custom game, 3 matchmaking; **E**,
[live-coop.md](../live/live-coop.md) §11.1, [bots.md](bots.md) §8) and
the lobby's mode of ours (`lobby_start::Mode`).

| Mode | Retail mode | `lobby_start::Mode` | The retail flow it is |
|---|---|---|---|
| MULTIPLAYER | 3 | Multiplayer | OptiMatch: a playlist, then the search; the Clanker playlists are among them |
| CUSTOM GAMES | 2 | Multiplayer | the custom game lobby |
| FORGE | 2 | Forge | the custom game lobby, as a Forge session ([forge-firefight-ui.md](forge-firefight-ui.md), "The lobbies") |
| CO-OP | 0 | Multiplayer | the co-op campaign lobby ([live-coop.md](../live/live-coop.md) §11) |

`Tab::of(retail mode, Mode)` is the inverse, and the lobby's mode is
always its answer for the lobby as it stands. Our mode counts only in
the custom lobby, so every lobby state has a mode. A lobby therefore
opens in the mode of the row that made it: Split Screen and a hosted
System Link game in CUSTOM GAMES; on the Xbox Live menu
([live-custom-games.md](../live/live-custom-games.md), "Rows"),
OptiMatch and Clankermatch in MULTIPLAYER, Co-op Campaign in CO-OP, and
Custom Games and Forge in their respective modes. Firefight and Siege are game types in Change Rules.

**The Xbox Live menu's mode rows.** Custom Games and Forge
each run retail's Custom Game press (a Live squad session, then
`LOBBY_MODE_SET(2)`) and ask for their mode
(`lobby_tabs::request`). The custom lobby's first panel update takes the
request, for the party leader in retail mode 2, and runs the switch to
that mode ("Switching"), with its checks and refusals. A request is
taken once and lapses after a minute (`lobby_tabs::REQUEST_FOR`); any
other row of the menu drops it (`lobby_tabs::cancel_request`). Building
the Xbox Live menu's list clears the lobby's mode of ours, so the lobby
opens in CUSTOM GAMES and Custom Games' own request changes nothing.

**One multiplayer lobby.** There is no Clanker mode. MULTIPLAYER's
playlist is the choice: a Clanker playlist (one the server marks,
`clankermatch::is_clanker`) is a game against Clankers, any other a
game against people. Change Playlist... lists both kinds on one page
(`clankermatch::Scope::All`), and `ui.lobby.tabs` says which kind the
held playlist is (`clanker`, read from the lobby's hopper at
`0x54E7B8`/`0x54E7BC`, `lobby_tabs::header::held_hopper`).

**Theater is not a mode.** It stays the main menu's sixth row
([forge-firefight-ui.md](forge-firefight-ui.md), "The main menu"). A
mode keeps the roster and the party; Theater has neither: it is one
player's film browser, and a film's page shows the recorded roster and
plays in an isolated instance ([theater.md](theater.md)). It becomes a
mode when a party can watch a film together, as Halo 3's Theater lobby
does ([notes/forge-prior-art.md](notes/forge-prior-art.md)).

### Where each mode runs

The lobby's place is `lobby_tabs::Place`: Split Screen (the session is
not networked, `bots::lobby::NETWORKING`), System Link, or Xbox Live.
`lobby_tabs::header::place_now` tells the last two by the squad
session's network class at `+0x18` (`variant_ext::SESSION_NETWORK_CLASS`):
1 System Link, 2 Xbox Live. Session creation (`0x59bd0`) stores the
class its caller `0x59890(class)` passes, and the Xbox Live paths
(`0x199e1f`, `0x23100d`, `0x23107b`) pass 2; our Live service leaves
session creation to the game. The `+0x4980` field
(`bots::phantom::session::at::CLASS`) is the squad's stage, which
`0x5a220` sets to 1 in every pregame lobby, so it cannot tell a Live
party from a System Link one. `lobby_tabs::available(tab, place, facts)`
answers `Avail::On` or `Avail::Off(reason)`.

| Mode | Split Screen | System Link | Xbox Live |
|---|---|---|---|
| MULTIPLAYER | off: "Matchmaking needs Xbox Live." | off, the same | on; off with no playlist listed: "No playlists are available." |
| CUSTOM GAMES | on | on | on |
| FORGE | on | on | on |
| CO-OP | on | off: "Co-op campaign is not available on System Link." | on; off without co-op over Xbox Live: "Co-op over Xbox Live is turned off." |

FORGE is off in every place while a member is on another console
(`lobby_mode::remote_members`): "Forge with players on other consoles
is not available." It lifts with slice F5
([forge-design.md](forge-design.md), "Slices").

CO-OP on Xbox Live needs co-op over Live (`coop_live`, installed with
Live and off with `H2_NO_COOP_ROW=1`; `Facts::live_co_op` reads
`coop_live::lobby`). Split Screen's co-op lobby is retail's alone, so
CO-OP is on there with or without Live.

A Firefight variant remains selectable when its maps are missing. Change Map
lists the installed kit maps compatible with its wave set. Start Game explains
whether a compatible map is missing or the party needs to select one.
See [Firefight](firefight.md).

Xbox Live's playlists are counted as listed until a switch to
MULTIPLAYER finds none (`lobby_tabs::switch::no_playlists_seen`); the
mode is then off until the party leaves (`forget_playlists`, on
`PARTY_LEAVE`).

A mode that is off is listed in the picker with its reason. A mode
whose feature is switched off (`-forge`) is absent: the
player turned it off, so there is no reason to give. MULTIPLAYER,
CUSTOM GAMES and CO-OP are retail's lobby modes and always present
(`lobby_tabs::Present`). Nothing else removes a mode.

### The header and the prompt

**The header** is the lobby tag's own title widget (`[204 -270 180
70]`, font 2), the first text widget of the screen
(`lobby_tabs::header::find`). Retail's panel update writes its text;
then the party lobby's update sets it to the lobby's mode with
`TEXT_SET` (`Tab::label`): CUSTOM GAMES, FORGE, MULTIPLAYER
or CO-OP. The header shows on every machine, the leader's and the
members'. The capitals fit the rectangle (**S**, the disc test
`every_mode_label_fits_the_header`).

**The prompt** is one line in the lobby's info box, under the header:
U+E102 (the X button's glyph) and "Change Game Mode"
(`lobby_tabs::CHANGE_MODE_TEXT`, served from the `lobby tabs` string
family). The update sets its text and its visible byte: it shows for
the party leader only, and not while the countdown runs, the game
loads or a search has its lobby (`Shown::prompt_visible`). A member
never sees it, since a member's X does nothing here.

The update is the party lobby's own `before` on `LOBBY_VARIANT_PANEL`
(`lobby_tabs::header::panel`): it runs retail's update, then
`lobby_tabs::header::update`, which takes a request from the Xbox Live
menu first ("The modes"), then writes the header and the prompt. It is
ordered inside `lobby-options`'s hook on the same point
(`after_feature`), so with that feature on its answer's call to retail
comes through this one, and with `lobby-options=off` this one is the
only answer: the header names the mode either way. The widgets are
found again on each update, not by kept addresses.

### The prompt line

The prompt's widget is a text block of the lobby tag, not one the code
adds: UI is data. The block is pane 0's text block
`lobby_tabs::header::PROMPT_BLOCK` (48, the first after the tag's own
48), and the update finds it by its index. On a tag without it the
update sets the header only.

The tag edit that adds it is not built: there is no boot path that
applies a retail tag edit kept in the repository as data. What it is,
on `pregame_lobby`'s pane 0:

1. a copy of text block 10 appended as block 48, its rectangle
   `[95 -260 75 70]`, the line under the header's;
2. text block 0's rectangle set to `[135 -258 95 45]`, so the two do
   not overlap;
3. the pane's text block reflexive pointed at the grown array.

What shipping it needs: a repository data format for edits to retail
tags (made from our own data, not disc bytes) and a `vfs::Rewrite` of
`mainmenu.map` that applies it at boot, as `CardRewrite` adds its
bitmaps. The editor's `ui.duplicate` and `ui.set_string` make these
edits on a map; its save writes a whole map, which is disc-derived and
stays out of the repository.

### Changing mode

**The button.** X opens the GAME MODE picker (`lobby_tabs::input`, a
`before` on `PREGAME_LOBBY_INPUT` that takes button 2 of a button event
and answers handled). The handler runs only while the lobby is the
input screen, so a dialog over it (Game Setup, a picker, the playlists,
the friends list) keeps its own buttons. The bumpers are retail's.

**Who.** Only the party leader (`LOBBY_ALL_READY`, AL 1 when this
console leads the party). A member's X goes on to retail's handler.

**The picker** is the kit's value picker (`ui_kit::picker`,
`kit.picker.mode`) over the lobby, on the lobby's window and slot, for
the player who pressed X (`lobby_tabs::picker`). Its title is GAME MODE
and its values the modes of `picker::MODES` present, in title case:
Multiplayer, Custom Games, Forge. CO-OP is not a value: the
co-op lobby is reached from the Xbox Live menu's Co-op Campaign row
and Split Screen's co-op lobby. It opens on the lobby's mode (CUSTOM
GAMES in CO-OP). The right side says what the focused mode plays
(`Tab::line`), or, for a mode that is off here, its reason. A picks and
closes the picker, then runs the switch to the mode picked
(`switch::run_to`, the pick's `after` action); a pick of the lobby's
own mode changes nothing, and a mode that is off puts its reason on
the status line for four seconds (`lobby_tabs::Notice`). B closes it
and changes nothing.

### Switching

**When.** Not while the countdown runs or the game loads (start states
8 and 10 of `LOBBY_START_STATE`), and not during a search. The reason
line says "The game is starting." or "Cancel the search to change
mode."

**What it does** (`lobby_tabs::switch::run`, in this order; the
picker's pick and a Live menu row's request both reach it through
`switch::run_to`, which reads the lobby and asks `switch::ask` first):

1. Checks the lock and the target: the start state and the search
   (above), the map the mode needs (`switch::map_for`), the co-op gate
   for CO-OP (`[0x54D5A0]` set refuses it with "Co-op campaign is not
   available in this lobby.", as retail's row raises error 0x32), and
   for MULTIPLAYER the playlist it opens on. Nothing has changed if any
   of these refuses.
2. Records the selected lobby mode in `lobby_start::set_mode`.
3. Leaving CO-OP, takes `coop_live`'s co-op arm down
   (`coop_live::lobby().leave()`).
4. Sets the retail mode when it differs (`switch::plan`), by the
   routine the retail Game Setup row for that mode runs (below). The
   party's squad session is the same session before and after, so the
   roster stays and nobody leaves.
5. Keeps the selected multiplayer map when switching between Custom Games and Forge.
   Change Map applies the selected game variant's requirements in Custom Games.

The game rebuilds the lobby's pane for the retail mode itself, and the
next panel update draws the mode's lines and the header.

| To | Retail routine (`lobby_tabs::switch`) | Opens on |
|---|---|---|
| CUSTOM GAMES, FORGE (from mode 0 or 3) | Switch To: Custom Game's press (`to_custom`: `0x199a57` clears the lobby's choices, `LOBBY_MODE_SET(2)`, `0x19a864` the custom setup) | the lobby's map and variant |
| CO-OP | Switch To: Co-Op Game's press past its gate (`to_co_op`: `0x199a57`, `LOBBY_MODE_SET(0)`, the campaign difficulty read by `0x121100` and held by `0x148d42`, `[0x54E7CC]` cleared, `LOBBY_SET_DIFFICULTY`); on Xbox Live `coop_live::host_retail` first, as the Xbox Live menu's row arms co-op | the level and difficulty the campaign setup picks |
| MULTIPLAYER | the playlist press (`to_matchmaking`, as `0x2b2181` runs it for a hopper: `0x199a57`, `LOBBY_MODE_SET(3)`, `0x63fb0`, `0x148cfc` the hopper, `0x19a0af` publishes it, `PLAYLIST_REMEMBER`) | the last playlist this profile played (`QUICKMATCH_HOPPER`) when it is listed, else the first listed (`HOPPER_LISTED` with `Scope::All`, `switch::opening_hopper`); none listed turns the mode off |

A switch among CUSTOM GAMES and FORGE changes no retail
state but the map. A map the game refuses puts the mode back and says
"The game did not take the map for this mode." Every refusal is on the
reason line, which shows for four seconds over the status line's other
texts (`lobby_status::custom_game_text`).

### The lobby by tab

With Theater installed, Game Setup also offers **Theater** in the party lobby, including when the party returns from a game. It opens Recent for the selecting controller.

| Mode | Pane | Game Setup's rows | Panel | Start Game |
|---|---|---|---|---|
| MULTIPLAYER | matchmaking | Change Playlist... (retail's action 9; the Clanker and the human playlists on one page) | retail's playlist lines | starts the search; a Clanker playlist's search hosts and its Clankers fill the lobby (`clankers::Driver::tick_match`) |
| CUSTOM GAMES | custom | Change Map, Map Variant..., Change Rules, Quick Options, and ours (Clanker Options..., Halo 1 Options...) | retail's five lines with ours (`lobby_options.rs`), and the map variant's name under the map's | retail's countdown |
| FORGE | custom | Change Map (the installed multiplayer maps), Map Variant..., Change Rules, Quick Options, Clanker Options... | "New Forge map" or the map variant's name | retail's; the session loads the draft (`forge_glue::request_draft`) |
| CO-OP | co-op | Select Level, Difficulty (retail's `load_level_lobby` and `difficulty_lobby`) | retail's level and difficulty lines | retail's co-op start (`coop_live`'s start check) |

- **Game Setup has no way to change mode.** `lobby_mode::arrange` drops
  retail's Switch To rows (actions 6 to 8, and 9 in a custom game's
  list) and Switch To: Clankermatch (action 12) from every Game Setup
  list (`lobby_mode::SWITCH_ROWS`, `switches_mode`). The arrangement is
  the list constructor's outermost `after` (`HookRef::first`): `after`
  hooks run in reverse ([feature-engine.md](../engine/feature-engine.md#hook-ordering)),
  so it runs once Clanker Options, Firefight Options, Halo 1 Options and
  Switch To: Clankermatch are added, and drops or moves them too. In a matchmaking
  lobby action 9 stays as the one row and reads Change Playlist...
  (`lobby_mode::CHANGE_PLAYLIST_LABEL`); it opens retail's playlist
  screen, which is a picker.
- **Each matchmaking choice has one place.** The playlist is chosen only
  on MULTIPLAYER's Game Setup (the OptiMatch setup dialog): Change
  Playlist... is its one row and opens the one playlist page. Whether
  the game is against Clankers is the playlist: the server's Clanker
  playlists are on that page with the others, so there is no Clanker
  row and no Switch To: Clankermatch in the lobby. The Xbox Live menu's
  Clankermatch row is only a way in: it lists the Clanker playlists,
  and the lobby the pick makes is in MULTIPLAYER.
- The FORGE rows, panel lines and refusal are
  `lobby_mode.rs`'s ([forge-firefight-ui.md](forge-firefight-ui.md),
  "The lobbies").
- No row steps through values with arrows. A choice is a picker.

### Clankers by tab

Clanker Options... (`clankers::ROW_ACTION`) is on Game Setup in the
custom lobby's two modes, and its bots sit only there:
`clankers::admits_page` takes the page's bots in retail mode 2 only,
and the Clankers driver follows the lobby's mode twice a second
(`clankers::frame`, `Driver::set_lobby`, the mode from
`lobby_tabs::header::retail_now`).

| Mode | Clankers |
|---|---|
| CUSTOM GAMES, FORGE | the Clanker Options page's count, difficulty and style |
| MULTIPLAYER | a Clanker playlist's own bots from its roster, once its search hosts; never the page's. The matchmaking lobby's squad session is not distributed (protocol 1), so a phantom cannot join it before the search |
| CO-OP | none: the co-op lobby keeps its seats for people, and its Game Setup has no Clanker row |

The page's count is kept: back in a custom mode, the page's bots are
seated again.

### Other machines

A member's lobby shows the same header and no prompt. Its mode is
`Tab::of` over what retail replicates to members (the lobby mode, the
playlist, the map) with the member's own readings: a playlist the
server marks as a Clanker one is reported as `clanker` on MULTIPLAYER
(`clankermatch::is_clanker`). A custom lobby shows CUSTOM GAMES to its guests, including on kit maps.

FORGE is the one mode retail's data cannot tell from CUSTOM GAMES. On
Xbox Live the host's mode travels as one byte beside the session's
other fields of ours (`live_proto::session::NewSession`, read by the
member as the variant extension is, `variant_ext.rs`). A leader cannot
switch to FORGE with a remote member present, so a member reads it only
by joining a Forge lobby, which cannot start.

### Snapshots

The feature has no section of its own. The lobby's mode is derived, the
widgets are guest memory, and `lobby_start::State` is in the
`party-lobby` section (`lobby_mode::save_state`). Whether the playlist
is a Clanker one is read from the lobby's hopper, which is guest
memory. Restoring the section clears the cached Firefight kit map.
The lobby panel reads it again on redraw. A snapshot is refused while
a switch's guest calls are in flight (`SnapshotPolicy::section_forbid_while`).
The reason line's timer and a Live menu row's request are not saved.

## Interfaces

- **Guest**, all named in `crates/hooking/guest-layout/src/points.rs`:
  `PREGAME_LOBBY_INPUT` (X), `TEXT_SET` (the header and the prompt),
  `LOBBY_VARIANT_PANEL` (the update), `LOBBY_MODE_SET`,
  `LOBBY_SET_MAP`, `LOBBY_START_STATE`, `LOBBY_ALL_READY`,
  `HOPPER_LISTED`, `QUICKMATCH_HOPPER`, `PLAYLIST_REMEMBER`,
  `LOBBY_SET_DIFFICULTY`, `SQUAD_SETTINGS_LIST_CTOR`.
  The routines the switch calls are constants in
  `lobby_tabs/switch.rs` (`LOBBY_CHOICES_CLEAR` and the rest, each with
  the retail press it comes from).
- **Ours**: `lobby_start::{mode, set_mode}`, `lobby_mode::{arrange,
  remote_members, start_refused}`, `clankermatch::{Scope, set_scope,
  listed, is_clanker}`, `clankers::admits_page`,
  `map_select::mode_category`, `lobby_tabs::{request, cancel_request}`
  (the Xbox Live menu's rows), and from the co-op lobby's owner
  `coop_live::host_retail` and `coop_live::lobby().leave()`.
- **Agent API** ([agent-api.md](../runtime/agent-api.md)):
  `ui.lobby.tabs` answers `{place, leader, selected, header, clanker,
  tabs: [{tab, label, state, reason}], notice}` with `state` one of
  `selected`, `on`, `off`, `header` the text the update gives the
  header, and
  `clanker` whether MULTIPLAYER's playlist is a Clanker one (`null` in
  another mode); `ui.press` takes the action `change_mode` (X), and
  the picker is then a kit dialog `ui.audit` reads.
- **Preview**: `h2 ui preview kit.picker.mode` draws the picker over the
  lobby, as the leader of an Xbox Live lobby in CUSTOM GAMES sees it.

## Slices

Sizes as [forge-design.md](forge-design.md), "Features". Lanes write
code and tests on synthetic data; the agent-API checks are the
coordinator's, on a running game. Each slice lands alone and leaves the
lobby working.

| # | Slice | Size | Owns | Tests | Agent-API check |
|---|---|---|---|---|---|
| **T1** | The model, no hooks: `Tab`, `Place`, `Avail`, `parts`, `Tab::of`, `available`, the labels, lines and reasons | S | `lobby_tabs.rs`, `lobby_tabs/tests.rs` | `every_tab_round_trips_through_its_parts`, `there_is_no_clankers_tab`, `availability_by_place` (the table above, cell by cell), `a_tab_that_is_off_has_a_reason`, `a_feature_that_is_off_removes_its_tab`, `every_label_and_reason_passes_the_content_check`, `each_look_follows_the_selection_and_the_place` | none |
| **T2** | The header and the prompt, read-only: the update, the header set to the mode, the prompt found by its block and shown for the leader; `ui.lobby.tabs` | M | `lobby_tabs/header.rs`, `lobby_options.rs` (the update's call), `ui_kit/ids.rs` (the family), `agent_glue.rs` and `crates/extensions/agent-proto` (the method), [agent-api.md](../runtime/agent-api.md) | `FakeGuest` tests: `the_header_names_the_mode_after_retails_panel_update`, `the_header_and_the_prompt_line_are_found_by_their_place_and_index`, `the_prompt_line_is_the_leaders_and_hides_while_locked`, `the_place_and_the_retail_mode_come_from_the_session`, `the_place_is_the_squad_sessions_network_class_not_its_stage`, `the_squad_mode_is_read_in_a_lobby_state_only`, `a_member_reads_its_mode_from_the_lobby`, `multiplayer_reports_whether_its_playlist_is_a_clanker_one`, `ui_lobby_tabs_answers_each_mode_with_its_state_and_reason`; the disc test `every_mode_label_fits_the_header`; `string_ids.rs` over the family | Split Screen lobby: `ui.lobby.tabs` has `selected` `custom_games`, `header` CUSTOM GAMES, `multiplayer` `off` with "Matchmaking needs Xbox Live."; `ui.screen` has no layout finding |
| **T3** | Changing mode inside the custom lobby: X, leader only, the GAME MODE picker, the countdown lock, CUSTOM GAMES and FORGE, the map into the mode's category, the reason line; `change_mode`; the Xbox Live menu's mode rows | M | `lobby_tabs/input.rs`, `lobby_tabs/picker.rs`, `lobby_tabs/switch.rs`, `ui_kit/picker.rs` (`after`), `lobby_mode.rs` (`arrange`), `lobby_status.rs`, `ui_press.rs`, `custom_games.rs` (the rows) | `x_opens_the_picker_for_the_leader_only`, `the_bumpers_are_left_to_retail`, `the_picker_lists_the_four_modes_and_opens_on_the_lobbys`, `a_mode_that_is_off_says_why_and_an_absent_one_is_not_listed`, `the_setter_writes_nothing`, `a_picked_mode_switches_as_the_tab_did`, `run_to_reads_the_lobby_and_runs_the_switch`, `a_requested_switch_is_taken_once_and_expires`, `each_mode_row_lands_on_its_mode`, `no_switch_during_the_countdown`, `switching_to_firefight_holds_the_setting_and_picks_a_kit_map`, `firefight_and_co_op_are_reached_with_no_kit_map_and_no_live`, `firefight_without_a_kit_map_refuses_the_start_and_says_why`, `a_reason_shows_for_four_seconds`, `game_setup_has_no_mode_row`, `game_setup_lists_the_rows_of_each_mode`, `change_mode_is_x_and_needs_the_lobby` | Split Screen: `ui.press change_mode`, then `ui.audit` shows GAME MODE with Custom Games focused; Forge picked gives `selected` `forge` and `header` FORGE; Multiplayer picked stays and `notice` is Matchmaking's reason. Xbox Live menu: Forge opens the lobby in Forge mode |
| **T4** | CO-OP: the retail mode switch between 2 and 0 with the party kept, Switch To rows dropped from Game Setup, no page Clankers in co-op | M | `lobby_tabs/switch.rs`, `lobby_mode.rs` (`arrange`), `coop_live.rs` (`host_retail`), `clankers.rs` (`admits_page`) | `arrange_drops_the_switch_to_rows`, `the_arrangement_runs_after_every_row_is_added`, `a_list_with_nothing_to_change_is_left_as_built`, `co_op_runs_the_co_op_switch`, `leaving_co_op_puts_the_custom_mode_back`, `co_op_from_forge_runs_after_the_checks`, `co_op_behind_the_retail_gate_is_refused_by_name`, `the_plan_sets_the_retail_mode_only_when_it_differs`, `the_page_bots_stand_down_outside_a_custom_lobby` | Xbox Live, Co-op Campaign row: `ui.lobby.tabs` `selected` `co_op`, `header` CO-OP; `change_mode` and Custom Games picked gives the custom pane with the same roster and the page's Clankers seated again |
| **T5** | MULTIPLAYER: the playlist it opens on, the playlist press with a hopper, Change Playlist... over both kinds, the Clanker choice on one page, the search lock, Clanker Options in the custom modes | M | `lobby_tabs/switch.rs`, `lobby_tabs/header.rs` (`clanker`), `clankermatch.rs` (`Scope`), `lobby_mode.rs` (Change Playlist..., Clanker Options in Forge), `ui_kit/view.rs` (the Game Setup openers) | `multiplayer_runs_the_playlist_press_for_its_hopper`, `the_tab_opens_on_the_last_playlist_when_listed`, `else_on_the_first_listed`, `no_listed_playlist_turns_the_tab_off`, `no_switch_during_a_search`, `the_switch_rows_pick_the_playlists`, `each_row_lists_its_own_hoppers`, `quickmatch_never_gets_a_clanker_hopper`, `clanker_options_is_on_every_custom_tab`, `forge_mode_lists_change_map_and_forge_map` | Xbox Live custom game: Multiplayer picked, `ui.audit` shows the matchmaking pane with a playlist named, the roster unchanged, `ui.lobby.tabs` `clanker` false or true by the playlist; Game Setup has only Change Playlist..., whose screen lists both kinds |
| **T6** | The host's mode on Xbox Live: the byte in the session record, the member's read | S | `lobby_tabs/seen.rs`; with the Live owner, `crates/live/live-proto/src/session.rs` and `crates/live/live-hle` | `a_member_reads_forge_from_the_byte`, `without_the_byte_a_member_derives_the_tab`, `an_unknown_byte_is_refused_by_name` | two consoles on Xbox Live: the member's `ui.lobby.tabs` `selected` equals the host's in every mode |
| **T7** | The prompt line's tag edit: a data format for retail tag edits and the `mainmenu.map` rewrite that applies it ("The prompt line") | M | the rewrite and its data, with the map build's owner | the edit applied to a synthetic `pregame_lobby` tag: block 48 appended at its rectangle, block 0 moved, the reflexive repointed; no disc bytes in the data | Xbox Live lobby, leader: `ui.audit` lists "Change Game Mode" on pane 0; a member's has none |

T6 needs the Live session field, and T7 the map rewrite, each an edit
in another owner's files, named in its row.

## Rules and limits

- Nothing stores the lobby's mode. Every reader calls `Tab::of`.
- Only the leader changes mode; a member's X is retail's.
- A mode that cannot run here is in the picker with its reason, and its
  pick puts the reason on the status line. A mode is absent only when
  its feature is switched off.
- A switch never ends the party's session and never changes the
  roster.
- The header and the prompt are the lobby tag's own widgets; the code
  sets their text and the prompt's visibility and adds no widget.
- X acts only in the lobby's own input handler. The bumpers are
  retail's.
- MULTIPLAYER always holds a listed playlist. With none listed the
  mode is off; it never opens on an unlisted one.
- Clankers from the Clanker Options page sit only in the custom lobby
  (`clankers::admits_page`); the co-op lobby never fills with them.
- On System Link a member that joins a Forge lobby reads CUSTOM GAMES,
  and the game cannot start, until slice F5 puts the Forge session on
  the network.
- Game Setup has no row that changes mode, and there is no retail lobby
  to fall back to.
- The header depends on no other feature: `lobby-options=off` removes
  only the extra panel lines.

## Evidence

- **V** The header, player count, button and roster rectangles and the
  lobby's panes: `h2 ui mainmenu` on the lobby's tag.
- **E** `LOBBY_MODE_SET` and its modes, Switch To: Co-Op Game's press,
  the start states: [live-coop.md](../live/live-coop.md) §11.1. The
  playlist screen, its press, `HOPPER_LISTED`, `QUICKMATCH_HOPPER`,
  Switch To: Matchmaking and Game Setup's actions by lobby kind:
  [bots.md](bots.md) §8.
- **Code** X as button 2 of a UI button event: `ui_kit/input.rs`.
- **S** That font 2's capitals fit the header (the disc test of T2);
  the reasons' wording against retail's status texts.
- **Design** The header text, the prompt, the picker's modes and order,
  the availability table, the reasons.

## Not verified

- That the header is set on every pane: the update runs from the
  custom pane's variant panel update (`LOBBY_VARIANT_PANEL`), so the
  matchmaking and co-op panes may show retail's header text.
- What retail's handler does with X on each pane, and that X reaches
  `PREGAME_LOBBY_INPUT` as button 2 while a lobby button has the
  focus.
- That the picker's `after` action, run once the picker has closed,
  reaches the switch's retail routines from the lobby with no Game
  Setup open, and that a Live menu row's request, run from the first
  panel update of a fresh custom lobby, does the same.
- That the retail routines the switch runs (`to_custom`, `to_co_op`,
  `to_matchmaking`) keep the squad session and its members, and that
  members' lobbies follow each mode change.
- That the playlist press's routines, run without the playlist
  screen's own session creation (`0x2b2181` creates one when this
  console is not the leader), enter the matchmaking lobby from a custom
  one: the switch runs only for the leader.
- That `LOBBY_MODE_SET(0)` on a Split Screen squad gives the co-op
  lobby an Xbox Live squad gets.
- That the status text's write is reachable in every pane for the
  reason line, and that it composes with the Forge refusal and the
  "Setting up bots" text of `lobby_status.rs`.
- That the map set by a switch reaches members as a Change Map pick
  does.
- That `LOBBY_SET_MAP` called from the switch sets the map as Change
  Map's pick does, and that `LOBBY_START_STATE` with argument 0 reads
  the lobby's start state.
- That the matchmaking lobby is told from the session type (6 or 7) or
  the squad session's mode (`+0x49a4` equal to 2 in states 3 to 8, as
  `0x64160` reads it), and the co-op lobby from the session's campaign
  difficulty (`lobby_tabs::header::retail_mode`).
- That our rows (Research and Devolution, Party Games) show on SELECT
  GAME (screen 207): their hooks are on and the list has room, but a
  play of the single-row version found no row. The next check is
  the log line "tweakers: rows added to screen 207" and `ui.audit` on
  SELECT GAME scrolled past its seventh row
  ([community-gametypes.md](community-gametypes.md)).

## Related

- [forge-firefight-ui.md](forge-firefight-ui.md): the main menu, the
  Forge and Firefight modes' rows and panel, the Forge screens and HUDs.
- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md): the
  UI kit, the value picker, the preview.
- [bots.md](bots.md) §8 (Clankermatch and the playlists),
  [live-coop.md](../live/live-coop.md) §11 (the co-op lobby),
  [live-custom-games.md](../live/live-custom-games.md) (the Xbox Live
  menu).
- [firefight.md](firefight.md), [forge-design.md](forge-design.md),
  [map-select.md](map-select.md), [theater.md](theater.md).
- [notes/forge-prior-art.md](notes/forge-prior-art.md): Halo 3's and
  Reach's lobbies.
