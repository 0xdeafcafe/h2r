# The party lobby's mode strip

Status: Built for slices T1 to T5: the strip is drawn and the bumpers switch among MULTIPLAYER, CUSTOM GAMES, FORGE, FIREFIGHT and CO-OP. Slice T6 (the host's tab on Xbox Live) is design.
Code: `crates/extensions/game/src/lobby_tabs.rs` and `lobby_tabs/` (the tabs, the strip, the switch; slices T1 to T6), `crates/extensions/game/src/lobby_mode.rs` (Game Setup's rows by tab, the start refusal), `crates/extensions/game/src/lobby_start.rs` (`Mode`), `crates/extensions/game/src/lobby_status.rs` (the reason line), `crates/extensions/game/src/clankermatch.rs` (the Clanker playlists), `crates/extensions/game/src/clankers.rs` (`admits_page`, the Clankers by tab). Tests: named per slice below.
Switch: none. The strip is how the lobby works: the `party-lobby` feature is `always_on`, so `-party-lobby` stops boot with the reason ([feature-engine.md](../engine/feature-engine.md#switches)). The FORGE and FIREFIGHT tabs follow the `forge` and `firefight` switches ([switches.md](../runtime/switches.md)).

The pregame lobby (`pregame_lobby`, screen 14) has a strip of modes
across its top: MULTIPLAYER, CUSTOM GAMES, FORGE, FIREFIGHT and CO-OP.
The party leader moves along it with the bumpers and the lobby becomes
that mode at once, with the same party in it. The strip replaces Game
Setup's Change Mode... and Switch To rows. The one rule:
**a tab is a name for lobby state the game already has; the strip
stores nothing of its own.** Each tab is a retail lobby mode plus, for
the custom lobby's three, a mode of ours, and the selected tab is read
back from that state on every machine.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### The tabs

A tab is two parts (`lobby_tabs::Tab::parts`): the retail lobby mode
(`LOBBY_MODE_SET`: 0 co-op, 2 custom game, 3 matchmaking; **E**,
[live-coop.md](../live/live-coop.md) §11.1, [bots.md](bots.md) §8) and
the lobby's mode of ours (`lobby_start::Mode`).

| Tab | Retail mode | `lobby_start::Mode` | The retail flow it is |
|---|---|---|---|
| MULTIPLAYER | 3 | Multiplayer | OptiMatch: a playlist, then the search; the Clanker playlists are among them |
| CUSTOM GAMES | 2 | Multiplayer | the custom game lobby |
| FORGE | 2 | Forge | the custom game lobby, as a Forge session ([forge-firefight-ui.md](forge-firefight-ui.md), "The lobbies") |
| FIREFIGHT | 2 | Firefight | the custom game lobby on a kit map ([firefight.md](firefight.md)) |
| CO-OP | 0 | Multiplayer | the co-op campaign lobby ([live-coop.md](../live/live-coop.md) §11) |

`Tab::of(retail mode, Mode)` is the inverse, and the selected tab is
always its answer for the lobby as it stands. Our mode counts only in
the custom lobby, so every lobby state has a tab. A lobby therefore
opens on the tab of the row that made it: Split Screen and a hosted
System Link game on CUSTOM GAMES, the Xbox Live menu's OptiMatch and
Clankermatch rows on MULTIPLAYER, and its Co-op Campaign and Custom
Game rows on theirs.

**One multiplayer lobby.** There is no Clanker tab. MULTIPLAYER's
playlist is the choice: a Clanker playlist (one the server marks,
`clankermatch::is_clanker`) is a game against Clankers, any other a
game against people. Change Playlist... lists both kinds on one page
(`clankermatch::Scope::All`), and `ui.lobby.tabs` says which kind the
held playlist is (`clanker`, read from the lobby's hopper at
`0x54E7B8`/`0x54E7BC`, `lobby_tabs::strip::held_hopper`).

**Theater is not a tab.** It stays the main menu's sixth row
([forge-firefight-ui.md](forge-firefight-ui.md), "The main menu"). A tab
keeps the roster and the party; Theater has neither: it is one
player's film browser, and a film's page shows the recorded roster and
plays in an isolated instance ([theater.md](theater.md)). It becomes a
tab when a party can watch a film together, as Halo 3's Theater lobby
does ([notes/forge-prior-art.md](notes/forge-prior-art.md)).

### Where each tab runs

The lobby's place is `lobby_tabs::Place`: Split Screen (the session is
not networked, `bots::lobby::NETWORKING`), System Link, or Xbox Live.
`lobby_tabs::strip::place_now` tells the last two by the squad session's
network class at `+0x18` (`variant_ext::SESSION_NETWORK_CLASS`): 1 System
Link, 2 Xbox Live. Session creation (`0x59bd0`) stores the class its
caller `0x59890(class)` passes, and the Xbox Live paths (`0x199e1f`,
`0x23100d`, `0x23107b`) pass 2; our Live service leaves session creation
to the game. The `+0x4980` field (`bots::phantom::session::at::CLASS`) is
the squad's stage, which `0x5a220` sets to 1 in every pregame lobby, so
it cannot tell a Live party from a System Link one.
`lobby_tabs::available(tab, place, facts)` answers `Avail::On` or
`Avail::Off(reason)`.

| Tab | Split Screen | System Link | Xbox Live |
|---|---|---|---|
| MULTIPLAYER | off: "Matchmaking needs Xbox Live." | off, the same | on; off with no playlist listed: "No playlists are available." |
| CUSTOM GAMES | on | on | on |
| FORGE | on | on | on |
| FIREFIGHT | on | on | on |
| CO-OP | on | off: "Co-op campaign is not available on System Link." | on; off without co-op over Xbox Live: "Co-op over Xbox Live is turned off." |

FORGE is off in every place while a member is on another console
(`lobby_mode::remote_members`): "Forge with players on other consoles
is not available." It lifts with slice F5
([forge-design.md](forge-design.md), "Slices").

CO-OP on Xbox Live needs co-op over Live (`coop_live`, installed with
Live and off with `H2_NO_COOP_ROW=1`; `Facts::live_co_op` reads
`coop_live::lobby`). Split Screen's co-op lobby is retail's alone, so
CO-OP is on there with or without Live.

**What a tab's game lacks does not turn the tab off.** FIREFIGHT is on
whatever maps are installed. With no map that carries the kit (a data
folder with only Halo 1 maps and the DLC has none) the tab opens, the
lobby keeps its map, and the lobby says what is missing:

- the variant panel shows "No installed map supports Firefight yet"
  and "Make one with h2 firefight kit" in place of Firefight's options
  (`lobby_mode::firefight_map_lines`);
- Start Game is held and the status line says "No installed map
  supports Firefight yet." (`lobby_mode::start_refusal`, reason
  `NoKitMap`). A Firefight lobby on a map without the kit while one is
  installed is held the same way: "This map does not support
  Firefight." (`NotKitMap`), with "Change Map lists the Firefight maps"
  on the panel.

Xbox Live's playlists are counted as listed until a switch to
MULTIPLAYER finds none (`lobby_tabs::switch::no_playlists_seen`); the
tab is then off until the party leaves (`forget_playlists`, on
`PARTY_LEAVE`).

A tab that is off is drawn dimmed and cannot be selected. A tab whose
feature is switched off (`-forge`, `-firefight`) is absent: the player
turned it off, so there is no reason to give. MULTIPLAYER, CUSTOM GAMES
and CO-OP are retail's lobby modes and always on the strip
(`lobby_tabs::Present`). Nothing else removes a tab. Every tab that is on switches: there is no
tab that is on and does nothing.

### The strip

The strip is text widgets added to the retail lobby screen with
`PANE_TEXT_ADD`, in the lobby's own font and colours, on the shell's
header band where the lobby's header sits. It has no art of ours and
no panel: the lobby is text on the shell's bands, and so is the strip.

**The room** (**V**, `h2 ui mainmenu` on the lobby's tag). The header
rectangle is `[204 -270 180 70]` and the player count text is
`[199 85 179 280]`; the buttons and the roster's first row start at
173 and 177. The band from 204 to 180 is the only room above the
roster inside the title-safe area (`ui_kit::layout::SAFE`), and the
header and the count fill it. The strip takes the band:

- **The header is hidden** (its visible byte, as `lobby_options.rs`
  hides a panel line). The selected tab is the screen's title, so the
  header would repeat it.
- **The player count keeps its right edge** and narrows to
  `[199 200 179 280]`, right-justified.

| Piece | Rectangle `[top left bottom right]` | Text |
|---|---|---|
| Previous | `[202 -270 182 -248]` | `Glyph::White` |
| MULTIPLAYER | `[202 -244 182 -160]` | centred |
| CUSTOM GAMES | `[202 -155 182 -63]` | centred |
| FORGE | `[202 -58 182 -18]` | centred |
| FIREFIGHT | `[202 -13 182 57]` | centred |
| CO-OP | `[202 62 182 102]` | centred |
| Next | `[202 107 182 129]` | `Glyph::Black` |

- **Font and colours.** Font 1, the lobby's text font, at the lobby
  texts' animation and depth bias (`mods_screen::page_layout`:
  `TEXT_ANIMATION`, `TEXT_BIAS`), so the strip comes and goes with the
  screen's own transition.
- **Looks.** Selected: white (`page_layout::WHITE`), the colour of the
  lobby's labels and names. On: pale blue (`PALE_BLUE`), the colour of
  its values. Off: pale blue at the 55% an unfocused retail row has.
  This is the game's focus idiom: the focused row bright, the others
  dim.
- **One layout.** Each tab has three text widgets in its cell, one per
  look, and the update shows one of them (the visible byte). A state
  changes what shows, never the widget set, as every kit screen holds
  ([ui-builder.md](ui-builder.md), "The model").
- **The glyphs** are the font's own button glyphs
  (`ui_kit::input::Glyph`). White is the left bumper and Black the right
  on a modern pad (`crates/platform/input/src/layouts.rs`), and each
  pad's set draws its own art ([button-icons.md](../input/button-icons.md)).
  They show for the party leader only. A glyph whose direction has no
  tab that is on shows dimmed.
- **Cells** are cut for the labels at the cell table's widths
  (`lobby_tabs::cells`), 5 units apart. With a tab absent the cells
  after it close up; the run stays left-aligned at -244, and Black's
  glyph follows the last cell (`lobby_tabs::next_glyph`).
- **The reason line** is the lobby's status text, where retail writes
  why a game cannot start (`lobby_status.rs`). A press toward a tab that
  is off shows that tab's reason there for four seconds
  (`lobby_tabs::Notice`).

The strip is updated after the lobby's panel update, by the party
lobby's own `before` on `LOBBY_VARIANT_PANEL`
(`lobby_tabs::strip::panel`): it runs retail's update, then
`lobby_tabs::strip::update`. It is ordered inside `lobby-options`'s
hook on the same point (`after_feature`), so with that feature on its
answer's call to retail comes through the strip's, and with
`lobby-options=off` the strip's is the only answer: the strip is drawn
and the retail header hidden either way. The widgets are found again on each update
by their block indices (`lobby_tabs::strip::BLOCK_BASE` and up, past any
of the tag's own, so no retail lookup by index finds one), not by kept
addresses. A pane that lacks them, at its first update or after its
rebuild on a mode change, has the missing ones added there and then.
Their texts are the `lobby tabs` string family's (`ui_kit::ids`), served
from our string table.

### Switching

**The buttons.** White moves to the previous tab that is on and Black
to the next (`lobby_tabs::step`); the ends do not wrap. A `before` on
`PREGAME_LOBBY_INPUT` takes buttons 4 and 5 and answers handled. The
handler runs only while the lobby is the input screen, so a dialog over
it (Game Setup, a picker, the playlists, the friends list) keeps its
own buttons, and in a game the bumpers are the game's. No retail menu
screen acts on these two buttons (**S**).

**Who.** Only the party leader (`LOBBY_ALL_READY`, AL 1 when this
console leads the party). A member's press does nothing and the strip
shows no glyphs.

**When.** Not while the countdown runs or the game loads (start states
8 and 10 of `LOBBY_START_STATE`), and not during a search. The reason
line says "The game is starting." or "Cancel the search to change
mode."

**What it does** (`lobby_tabs::switch::run`, in this order):

1. Checks the lock and the target: the start state and the search
   (above), the map the tab needs (`switch::map_for`), the co-op gate
   for CO-OP (`[0x54D5A0]` set refuses it with "Co-op campaign is not
   available in this lobby.", as retail's row raises error 0x32), and
   for MULTIPLAYER the playlist it opens on. Nothing has changed if any
   of these refuses.
2. Sets the parts of ours: `lobby_start::set_mode` and the `firefight`
   setting held on for FIREFIGHT (`firefight::options::ENABLED`).
3. Leaving CO-OP, takes `coop_live`'s co-op arm down
   (`coop_live::lobby().leave()`).
4. Sets the retail mode when it differs (`switch::plan`), by the
   routine the retail Game Setup row for that mode runs (below). The
   party's squad session is the same session before and after, so the
   roster stays and nobody leaves.
5. Puts the map inside the tab's category
   (`map_select::mode_category`): a map outside it is replaced by the
   category's first through `LOBBY_SET_MAP`, and the picture and name
   show it. CUSTOM GAMES takes any map. FIREFIGHT with no kit map
   installed, or before the map list is read, keeps the lobby's map
   (`map_for` answers none); its Start Game is held instead (above).

The game rebuilds the lobby's pane for the retail mode itself, and the
next panel update draws the tab's lines and the strip.

| To | Retail routine (`lobby_tabs::switch`) | Opens on |
|---|---|---|
| CUSTOM GAMES, FORGE, FIREFIGHT (from mode 0 or 3) | Switch To: Custom Game's press (`to_custom`: `0x199a57` clears the lobby's choices, `LOBBY_MODE_SET(2)`, `0x19a864` the custom setup) | the lobby's map and variant |
| CO-OP | Switch To: Co-Op Game's press past its gate (`to_co_op`: `0x199a57`, `LOBBY_MODE_SET(0)`, the campaign difficulty read by `0x121100` and held by `0x148d42`, `[0x54E7CC]` cleared, `LOBBY_SET_DIFFICULTY`); on Xbox Live `coop_live::host_retail` first, as the Xbox Live menu's row arms co-op | the level and difficulty the campaign setup picks |
| MULTIPLAYER | the playlist press (`to_matchmaking`, as `0x2b2181` runs it for a hopper: `0x199a57`, `LOBBY_MODE_SET(3)`, `0x63fb0`, `0x148cfc` the hopper, `0x19a0af` publishes it, `PLAYLIST_REMEMBER`) | the last playlist this profile played (`QUICKMATCH_HOPPER`) when it is listed, else the first listed (`HOPPER_LISTED` with `Scope::All`, `switch::opening_hopper`); none listed turns the tab off |

A switch among CUSTOM GAMES, FORGE and FIREFIGHT changes no retail
state but the map. A map the game refuses puts the mode back and says
"The game did not take the map for this mode." Every refusal is on the
reason line, which shows for four seconds over the status line's other
texts (`lobby_status::custom_game_text`).

### The lobby by tab

| Tab | Pane | Game Setup's rows | Panel | Start Game |
|---|---|---|---|---|
| MULTIPLAYER | matchmaking | Change Playlist... (retail's action 9; the Clanker and the human playlists on one page) | retail's playlist lines | starts the search; a Clanker playlist's search hosts and its Clankers fill the lobby (`clankers::Driver::tick_match`) |
| CUSTOM GAMES | custom | Change Map, Change Rules, Quick Options, and ours (Clanker Options..., Firefight Options..., Halo 1 Options...) | retail's five lines with ours (`lobby_options.rs`) | retail's countdown |
| FORGE | custom | Change Map (the original maps), Forge Map..., Change Rules, Quick Options, Clanker Options... | "New Forge map" or the saved map's name | retail's; the session loads the draft (`forge_glue::request_draft`) |
| FIREFIGHT | custom | Change Map (the Firefight category), Firefight Options..., Quick Options, Clanker Options... | wave set, difficulty, starting lives | retail's, with the setting held |
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
  and the lobby the pick makes is on MULTIPLAYER.
- The FORGE and FIREFIGHT rows, panel lines and refusal are
  `lobby_mode.rs`'s ([forge-firefight-ui.md](forge-firefight-ui.md),
  "The lobbies").
- No row or tab steps through values with arrows. A choice is a tab or
  a picker.

### Clankers by tab

Clanker Options... (`clankers::ROW_ACTION`) is on Game Setup in the
custom lobby's three tabs, and its bots sit only there:
`clankers::admits_page` takes the page's bots in retail mode 2 only,
and the Clankers driver follows the lobby's mode twice a second
(`clankers::frame`, `Driver::set_lobby`, the mode from
`lobby_tabs::strip::retail_now`).

| Tab | Clankers |
|---|---|
| CUSTOM GAMES, FORGE, FIREFIGHT | the Clanker Options page's count, difficulty and style |
| MULTIPLAYER | a Clanker playlist's own bots from its roster, once its search hosts; never the page's. The matchmaking lobby's squad session is not distributed (protocol 1), so a phantom cannot join it before the search |
| CO-OP | none: the co-op lobby keeps its seats for people, and its Game Setup has no Clanker row |

The page's count is kept: back on a custom tab, the page's bots are
seated again.

### Other machines

A member's lobby shows the same strip without glyphs. Its selected tab
is `Tab::of` over what retail replicates to members (the lobby mode,
the playlist, the map) with the member's own readings: a playlist the
server marks as a Clanker one is reported as `clanker` on MULTIPLAYER
(`clankermatch::is_clanker`), and a kit map in a custom lobby gives
FIREFIGHT (`lobby_start::is_kit_map`).

FORGE is the one tab retail's data cannot tell from CUSTOM GAMES. On
Xbox Live the host's tab travels as one byte beside the session's
other fields of ours (`live_proto::session::NewSession`, read by the
member as the variant extension is, `variant_ext.rs`). A leader cannot
select FORGE with a remote member present, so a member reads it only
by joining a Forge lobby, which cannot start.

### Snapshots

The feature has no section of its own. The selected tab is derived, the
widgets are guest memory, and `lobby_start::State` is in the
`party-lobby` section (`lobby_mode::save_state`). Whether the playlist
is a Clanker one is read from the lobby's hopper, which is guest
memory. A snapshot is refused while a switch's guest calls are in
flight (`SnapshotPolicy::section_forbid_while`).
The reason line's timer is not saved.

## Interfaces

- **Guest**, all named in `crates/hooking/guest-layout/src/points.rs`:
  `PREGAME_LOBBY_INPUT` (the buttons), `PANE_TEXT_ADD` and `TEXT_SET`
  (the widgets), `LOBBY_VARIANT_PANEL` (the update), `LOBBY_MODE_SET`,
  `LOBBY_SET_MAP`, `LOBBY_START_STATE`, `LOBBY_ALL_READY`,
  `HOPPER_LISTED`, `QUICKMATCH_HOPPER`, `PLAYLIST_REMEMBER`,
  `LOBBY_SET_DIFFICULTY`, `SQUAD_SETTINGS_LIST_CTOR`.
  The routines the switch calls are constants in
  `lobby_tabs/switch.rs` (`LOBBY_CHOICES_CLEAR` and the rest, each with
  the retail press it comes from).
- **Ours**: `lobby_start::{mode, set_mode}`, `lobby_mode::{arrange,
  remote_members, start_refused}`, `clankermatch::{Scope, set_scope,
  listed, is_clanker}`, `clankers::admits_page`,
  `map_select::mode_category`, and from the co-op lobby's owner
  `coop_live::host_retail` and `coop_live::lobby().leave()`.
- **Agent API** ([agent-api.md](../runtime/agent-api.md)):
  `ui.lobby.tabs` answers `{place, leader, selected, clanker, tabs:
  [{tab, label, state, reason}], notice}` with `state` one of
  `selected`, `on`, `off`, and `clanker` whether MULTIPLAYER's
  playlist is a Clanker one (`null` on another tab); `ui.press` takes
  the actions `tab_prev` and `tab_next`.
  `ui.screen`'s layout findings cover the strip's widgets as any
  text's.
- **Preview**: `h2 ui preview lobby.tabs --state all` draws the strip
  in each place and on each tab it can be on, and a member's
  (`lobby_tabs::preview`): the widgets the update shows, at their cells,
  in the lobby's font and colours, without the retail pane under them.
- **Designs**: `h2 ui preview design.lobby-tabs.a --state all` (and
  `.b`, `.c`) draws a design for the strip over the game shell, so the
  header band shows (`lobby_tabs::designs`): a window of three tabs with
  the bumpers at the band's ends; the selected tab as the lobby's header
  with a legend naming the tab each bumper moves to; all five tabs in
  the lobby's text case with the bumpers beside the player count. Each
  page's states are every tab selected in turn, the widest (`longest`),
  a Split Screen lobby (`unavailable`), a member's console (`member`)
  and a run with only retail's three modes (`retail-tabs`). The band has
  338 units for a line of text (x -270 to +68);
  `lobby_tabs::designs::tests` hold every design to it for every set of
  tabs.
- **Concepts of the whole lobby**: `h2 ui preview design.lobby.d --state
  all` (and `.e`, `.f`) draws the pregame lobby with a mode control, as a
  kit list screen over the game shell (`lobby_tabs::concepts`): the
  header, `name_field` and `lobby_display` from the lobby's tag at their
  own points, the map's picture, Start Game and Game Setup, the game's
  lines, the player count, the party and the footer legend.
  - `design.lobby.d`: the header names the mode (FORGE LOBBY) and a
    third button, MODE: FORGE, sits beside the lobby's two; A on it
    opens the kit's value picker on the modes, whose help line says why
    a mode can't run here. States: each mode, `mode-focused`, `picker`,
    `picker-unavailable`.
  - `design.lobby.e`: the header is the mode, with a mark for each mode
    beside it on the band (the current one white, one that can't run
    faint) and the bumpers at the marks' ends. States: each mode, and
    `member` (no bumpers, no buttons).
  - `design.lobby.f`: the modes are a list on the lobby's panel, on the
    `default` list skin, the lobby's mode lit. States: each mode with the
    list focused, and `buttons` with Start Game focused.

  The second button's rectangle, the look of a focused button and the
  player rows' cell art are not read from the lobby's tag: the pages
  place the first two and leave the third out
  (`lobby_tabs::concepts`, module comment).

## Slices

Sizes as [forge-design.md](forge-design.md), "Features". Lanes write
code and tests on synthetic data; the agent-API checks are the
coordinator's, on a running game. Each slice lands alone and leaves the
lobby working.

| # | Slice | Size | Owns | Tests | Agent-API check |
|---|---|---|---|---|---|
| **T1** | The model, no hooks: `Tab`, `Place`, `Avail`, `parts`, `Tab::of`, `available`, `step`, `cells`, `next_glyph`, the labels and reasons | S | `lobby_tabs.rs`, `lobby_tabs/tests.rs`, the `mod` line in `lib.rs` | `every_tab_round_trips_through_its_parts`, `there_is_no_clankers_tab`, `availability_by_place` (the table above, cell by cell), `a_tab_that_is_off_has_a_reason`, `a_feature_that_is_off_removes_its_tab`, `step_skips_tabs_that_are_off_and_names_the_first`, `the_ends_do_not_wrap`, `cells_stay_in_the_band_and_do_not_overlap`, `cells_close_up_without_a_tab`, `every_label_and_reason_passes_the_content_check` | none |
| **T2** | The strip drawn, read-only: the widgets added on the update, the three looks, the header hidden, the count narrowed, the update; `lobby.tabs` in the preview; `ui.lobby.tabs` | M | `lobby_tabs/strip.rs`, `lobby_options.rs` (the update's call), `lobby_mode.rs` (`set_header` removed), `ui_kit/ids.rs` (the family), `lobby_tabs/preview.rs` and `ui_kit/screens.rs` (`lobby.tabs`), `agent_glue.rs` and `crates/extensions/agent-proto` (the method), [agent-api.md](../runtime/agent-api.md) | `FakeGuest` tests: `one_look_shows_per_tab`, `the_header_is_hidden_and_put_back_with_the_strip_off`, `the_strip_is_found_again_by_its_ids`, `a_member_has_no_glyphs`, `the_place_and_the_retail_mode_come_from_the_session`, `the_place_is_the_squad_sessions_network_class_not_its_stage`, `the_squad_mode_is_read_in_a_lobby_state_only`, `multiplayer_reports_whether_its_playlist_is_a_clanker_one`; `the_strip_takes_the_lobby_texts_animation`; the preview's `every_state_draws_one_label_per_tab_present`; `string_ids.rs` over the family | Split Screen lobby: `ui.lobby.tabs` has `selected` `custom_games`, `multiplayer` `off` with "Matchmaking needs Xbox Live."; `ui.screen` has no layout finding |
| **T3** | Switching inside the custom lobby: the bumpers, leader only, the countdown lock, CUSTOM GAMES, FORGE and FIREFIGHT, the map into the tab's category, the reason line; `tab_prev` and `tab_next` | M | `lobby_tabs/input.rs`, `lobby_tabs/switch.rs`, `lobby_mode.rs` (`arrange`), `lobby_status.rs`, `ui_press.rs` | `a_member_cannot_switch`, `no_switch_during_the_countdown`, `black_and_white_are_handled_and_nothing_else_is`, `every_tab_that_is_on_is_reached_by_the_bumpers`, `switching_to_firefight_holds_the_setting_and_picks_a_kit_map`, `firefight_and_co_op_are_reached_with_no_kit_map_and_no_live`, `firefight_without_a_kit_map_refuses_the_start_and_says_why`, `kit_maps_say_whether_the_lobby_and_the_install_can_play_firefight`, `switching_to_forge_keeps_a_halo_2_map`, `a_press_toward_a_tab_that_is_off_shows_its_reason_for_four_seconds`, `game_setup_has_no_mode_row`, `game_setup_lists_the_rows_of_each_mode` | Split Screen: `ui.press tab_next` from CUSTOM GAMES gives FORGE, then FIREFIGHT (with or without a kit map installed), then CO-OP; `ui.lobby_options` has the tab's lines; `tab_prev` at CUSTOM GAMES stays and `notice` is Matchmaking's reason |
| **T4** | CO-OP: the retail mode switch between 2 and 0 with the party kept, Switch To rows dropped from Game Setup, no page Clankers in co-op | M | `lobby_tabs/switch.rs`, `lobby_mode.rs` (`arrange`), `coop_live.rs` (`host_retail`), `clankers.rs` (`admits_page`) | `arrange_drops_the_switch_to_rows`, `the_arrangement_runs_after_every_row_is_added`, `a_list_with_nothing_to_change_is_left_as_built`, `co_op_runs_the_co_op_switch`, `leaving_co_op_puts_the_custom_mode_back`, `co_op_from_forge_runs_after_the_checks`, `co_op_behind_the_retail_gate_is_refused_by_name`, `the_plan_sets_the_retail_mode_only_when_it_differs`, `the_page_bots_stand_down_outside_a_custom_lobby` | Split Screen with two profiles and an Xbox Live custom game: `tab_next` to CO-OP, `ui.lobby.players` is the same roster with no Clanker, `ui.audit` shows the co-op pane and Game Setup's level and difficulty rows; back to CUSTOM GAMES the same, with the page's Clankers seated again |
| **T5** | MULTIPLAYER: the playlist it opens on, the playlist press with a hopper, Change Playlist... over both kinds, the Clanker choice on one page, the search lock, Clanker Options on the custom tabs | M | `lobby_tabs/switch.rs`, `lobby_tabs/strip.rs` (`clanker`), `clankermatch.rs` (`Scope`), `lobby_mode.rs` (Change Playlist..., Clanker Options in Forge), `ui_kit/view.rs` (the Game Setup openers) | `multiplayer_runs_the_playlist_press_for_its_hopper`, `the_tab_opens_on_the_last_playlist_when_listed`, `else_on_the_first_listed`, `no_listed_playlist_turns_the_tab_off`, `no_switch_during_a_search`, `the_switch_rows_pick_the_playlists`, `each_row_lists_its_own_hoppers`, `quickmatch_never_gets_a_clanker_hopper`, `clanker_options_is_on_every_custom_tab`, `forge_mode_lists_change_map_and_forge_map` | Xbox Live custom game: `tab_prev` to MULTIPLAYER, `ui.audit` shows the matchmaking pane with a playlist named, the roster unchanged, `ui.lobby.tabs` `clanker` false or true by the playlist; Game Setup has only Change Playlist..., whose screen lists both kinds |
| **T6** | The host's tab on Xbox Live: the byte in the session record, the member's read | S | `lobby_tabs/seen.rs`; with the Live owner, `crates/live/live-proto/src/session.rs` and `crates/live/live-hle` | `a_member_reads_forge_from_the_byte`, `without_the_byte_a_member_derives_the_tab`, `an_unknown_byte_is_refused_by_name` | two consoles on Xbox Live: the member's `ui.lobby.tabs` `selected` equals the host's on every tab |

T6 needs the Live session field, an edit in another owner's files,
named in its row.

## Rules and limits

- The strip stores no selected tab. Every reader calls `Tab::of`.
- Only the leader switches; a member's strip is a read-out.
- A tab that cannot run here is dimmed with a reason on the status
  line. A tab is absent only when its feature is switched off.
- A switch never ends the party's session and never changes the
  roster.
- The strip's widgets stay inside the header band and the title-safe
  area, in the lobby's font and colours, with no bitmap of ours.
- The bumpers act only in the lobby's own input handler.
- MULTIPLAYER always holds a listed playlist. With none listed the
  tab is off; it never opens on an unlisted one.
- Clankers from the Clanker Options page sit only in the custom lobby
  (`clankers::admits_page`); the co-op lobby never fills with them.
- On System Link a member that joins a Forge lobby reads CUSTOM GAMES,
  and the game cannot start, until slice F5 puts the Forge session on
  the network.
- Game Setup has no row that changes mode, and there is no retail lobby
  to fall back to.
- The strip depends on no other feature: `lobby-options=off` removes
  only the extra panel lines.

## Evidence

- **V** The header, player count, button and roster rectangles and the
  lobby's panes: `h2 ui mainmenu` on the lobby's tag; the lobby texts'
  font, colours, animation and bias: `mods_screen::page_layout` and its
  disc test.
- **E** `LOBBY_MODE_SET` and its modes, Switch To: Co-Op Game's press,
  the start states: [live-coop.md](../live/live-coop.md) §11.1. The
  playlist screen, its press, `HOPPER_LISTED`, `QUICKMATCH_HOPPER`,
  Switch To: Matchmaking and Game Setup's actions by lobby kind:
  [bots.md](bots.md) §8.
- **Code** Bumpers as Black and White:
  `crates/platform/input/src/layouts.rs`. Buttons 4 and 5 in a UI
  event: `ui_kit/input.rs`.
- **S** That font 1's capitals fit the cells (the disc test of T2);
  that the player count's text fits 80 units (the same); that no
  retail menu screen acts on Black or White (`h2 disasm` of the shell
  screens' event handlers); the reasons' wording against retail's
  status texts.
- **Design** The band, the cells, the looks, the tabs and their order,
  the availability table, the reasons.

## Not verified

- That Black and White reach `PREGAME_LOBBY_INPUT` as button events 4
  and 5 while a lobby button has the focus.
- That a text widget added to the lobby screen after its build draws,
  keeps its place through the pane's rebuild on a mode change, and
  follows the screen's transition.
- That hiding the header widget and narrowing the player count's
  rectangle hold through the lobby's own updates.
- That the retail routines the switch runs (`to_custom`, `to_co_op`,
  `to_matchmaking`) keep the squad session and its members when run
  from the lobby's input handler with no Game Setup open, and that
  members' lobbies follow each mode change.
- That the playlist press's routines, run without the playlist
  screen's own session creation (`0x2b2181` creates one when this
  console is not the leader), enter the matchmaking lobby from a custom
  one: the switch runs only for the leader.
- That `LOBBY_MODE_SET(0)` on a Split Screen squad gives the co-op
  lobby an Xbox Live squad gets.
- That entering mode 3 with a hopper from a Split Screen or System
  Link lobby is refused by the game. The tabs are off there, so no
  press reaches it.
- That the status text's write is reachable in every pane for the
  reason line, and that it composes with the Forge refusal and the
  "Setting up bots" text of `lobby_status.rs`.
- That the map set by a switch reaches members as a Change Map pick
  does.
- That `LOBBY_SET_MAP` called from the lobby's input handler (an
  async answer of `PREGAME_LOBBY_INPUT`) sets the map as Change Map's
  pick does, and that `LOBBY_START_STATE` with argument 0 reads the
  lobby's start state.
- That the matchmaking lobby is told from the session type (6 or 7) or
  the squad session's mode (`+0x49a4` equal to 2 in states 3 to 8, as
  `0x64160` reads it), and the co-op lobby from the session's campaign
  difficulty (`lobby_tabs::strip::retail_mode`).
- That the strip is updated on every pane: it is drawn from the custom
  pane's variant panel update (`LOBBY_VARIANT_PANEL`).
- That the Tweakers row shows on SELECT GAME (screen 207): its hooks
  are on and the list has room, but the user's play found no row. The
  next check is the log line "tweakers: row added to screen 207" and
  `ui.audit` on SELECT GAME scrolled past its seventh row
  ([community-gametypes.md](community-gametypes.md)).

## Related

- [forge-firefight-ui.md](forge-firefight-ui.md): the main menu, the
  Forge and Firefight tabs' rows and panel, the Forge screens and HUDs.
- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md): the
  UI kit, the title-safe area, the preview.
- [bots.md](bots.md) §8 (Clankermatch and the playlists),
  [live-coop.md](../live/live-coop.md) §11 (the co-op lobby),
  [live-custom-games.md](../live/live-custom-games.md) (the Xbox Live
  menu).
- [firefight.md](firefight.md), [forge-design.md](forge-design.md),
  [map-select.md](map-select.md), [theater.md](theater.md).
- [notes/forge-prior-art.md](notes/forge-prior-art.md): Halo 3's and
  Reach's lobbies.
