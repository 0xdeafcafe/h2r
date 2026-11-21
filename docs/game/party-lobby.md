# The party lobby's mode strip

Status: Design. The lobby's Forge and Firefight modes, their Game Setup rows, panel lines and start refusal are built (`lobby_mode.rs`, `lobby_start.rs`) and chosen on Game Setup's **Change Mode...** row, which slice T3 replaces with the strip.
Code: `crates/extensions/game/src/lobby_tabs.rs` and `lobby_tabs/` (the tabs, the strip, the switch; slices T1 to T6), `crates/extensions/game/src/lobby_mode.rs` (Game Setup's rows by tab, the start refusal), `crates/extensions/game/src/lobby_start.rs` (`Mode`), `crates/extensions/game/src/lobby_status.rs` (the reason line), `crates/extensions/game/src/clankermatch.rs` (the Clanker playlists). Tests: named per slice below.
Switch: `lobby-mode` (on), listed in [switches.md](../runtime/switches.md).

The pregame lobby (`pregame_lobby`, screen 14) has a strip of modes
across its top: MATCHMAKING, CUSTOM GAMES, FORGE, FIREFIGHT, CO-OP and
CLANKERS. The party leader moves along it with the bumpers and the
lobby becomes that mode at once, with the same party in it. The strip
replaces Game Setup's Change Mode... and Switch To rows. The one rule:
**a tab is a name for lobby state the game already has; the strip
stores nothing of its own.** Each tab is a retail lobby mode plus, for
three of them, a mode of ours, and the selected tab is read back from
that state on every machine.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### The tabs

A tab is three parts (`lobby_tabs::Tab::parts`): the retail lobby mode
(`LOBBY_MODE_SET`: 0 co-op, 2 custom game, 3 matchmaking; **E**,
[live-coop.md](../live/live-coop.md) §11.1, [bots.md](bots.md) §8), the
lobby's mode of ours (`lobby_start::Mode`) and whether the playlists
are the Clanker ones (`clankermatch::active`).

| Tab | Retail mode | `lobby_start::Mode` | Clanker playlists | The retail flow it is |
|---|---|---|---|---|
| MATCHMAKING | 3 | Multiplayer | no | OptiMatch: a playlist, then the search |
| CUSTOM GAMES | 2 | Multiplayer | | the custom game lobby |
| FORGE | 2 | Forge | | the custom game lobby, as a Forge session ([forge-firefight-ui.md](forge-firefight-ui.md), "The lobbies") |
| FIREFIGHT | 2 | Firefight | | the custom game lobby on a kit map ([firefight.md](firefight.md)) |
| CO-OP | 0 | Multiplayer | | the co-op campaign lobby ([live-coop.md](../live/live-coop.md) §11) |
| CLANKERS | 3 | Multiplayer | yes | Clankermatch ([bots.md](bots.md) §8) |

`Tab::of(retail mode, Mode, clanker)` is the inverse, and the selected
tab is always its answer for the lobby as it stands. A lobby therefore
opens on the tab of the row that made it: Split Screen and a hosted
System Link game on CUSTOM GAMES, and the Xbox Live menu's OptiMatch,
Clankermatch, Co-op Campaign and Custom Game rows on theirs.

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
`lobby_tabs::available(tab, place, facts)` answers `Avail::On` or
`Avail::Off(reason)`.

| Tab | Split Screen | System Link | Xbox Live |
|---|---|---|---|
| MATCHMAKING | off: "Matchmaking needs Xbox Live." | off, the same | on; off with no playlist listed: "No playlists are available." |
| CUSTOM GAMES | on | on | on |
| FORGE | on | on | on |
| FIREFIGHT | on | on | on |
| CO-OP | on | off: "Co-op campaign is not available on System Link." | on |
| CLANKERS | off: "Clankermatch needs Xbox Live." | off, the same | on; off with no Clanker playlist: "This server has no Clanker playlists." |

Two more reasons apply in every place:

- FIREFIGHT is off with no kit map installed (`lobby_start::kit_maps`):
  "No installed map carries Firefight."
- FORGE is off while a member is on another console
  (`lobby_mode::remote_members`): "Forge with players on other consoles
  is not available." It lifts with slice F5
  ([forge-design.md](forge-design.md), "Slices").

A tab that is off is drawn dimmed and cannot be selected. A tab whose
feature is switched off (`-forge`, `-firefight`, `-clankermatch`, the
co-op screens) is absent: the player turned it off, so there is no
reason to give. Nothing else removes a tab.

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
| MATCHMAKING | `[202 -244 182 -160]` | centred |
| CUSTOM GAMES | `[202 -155 182 -63]` | centred |
| FORGE | `[202 -58 182 -18]` | centred |
| FIREFIGHT | `[202 -13 182 57]` | centred |
| CO-OP | `[202 62 182 102]` | centred |
| CLANKERS | `[202 107 182 169]` | centred |
| Next | `[202 174 182 196]` | `Glyph::Black` |

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
  after it close up; the run stays left-aligned at -244.
- **The reason line** is the lobby's status text, where retail writes
  why a game cannot start (`lobby_status.rs`). A press toward a tab that
  is off shows that tab's reason there for four seconds
  (`lobby_tabs::Notice`).

The strip is updated where the lobby's panel is
(`lobby_options::update`, on `LOBBY_VARIANT_PANEL`), and the widgets are
found again on each update by their string ids (`ui_kit::ids` family
`lobby tabs`), not by kept addresses.

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

**What it does** (`lobby_tabs::switch`, in this order):

1. Sets the parts of ours: `lobby_start::set_mode`, the `firefight`
   setting held on for FIREFIGHT (`firefight::options::ENABLED`) and
   `clankermatch::set_active`.
2. Sets the retail mode when it differs, by the routine the retail
   Switch To row for that mode runs (below). The party's squad session
   is the same session before and after, so the roster stays and
   nobody leaves.
3. Puts the map inside the tab's category
   (`map_select::mode_category`): a map outside it is replaced by the
   category's first through `LOBBY_SET_MAP`, and the picture and name
   show it. CUSTOM GAMES takes any map.

The game rebuilds the lobby's pane for the retail mode itself, and the
next panel update draws the tab's lines and the strip.

| To | Retail routine | Opens on |
|---|---|---|
| CUSTOM GAMES, FORGE, FIREFIGHT (from mode 0 or 3) | Switch To: Custom Game's press (`LOBBY_MODE_SET(2)`) | the lobby's map and variant |
| CO-OP | Switch To: Co-Op Game's press after its gate (`0x2bba01`: `LOBBY_MODE_SET(0)`, then the campaign setup), with `coop_live`'s co-op arm set as the Xbox Live menu's row sets it | the level and difficulty the campaign setup picks |
| MATCHMAKING, CLANKERS | the playlist press's entry (`0x2b2181`: the hopper, then `LOBBY_MODE_SET(3)`) | the last playlist this profile played (`QUICKMATCH_HOPPER`) when it is listed for the tab, else the first listed (`HOPPER_LISTED`) |

A switch among CUSTOM GAMES, FORGE and FIREFIGHT changes no retail
state but the map.

### The lobby by tab

| Tab | Pane | Game Setup's rows | Panel | Start Game |
|---|---|---|---|---|
| MATCHMAKING | matchmaking | Change Playlist... (retail's action 9, the playlists) | retail's playlist lines | starts the search |
| CUSTOM GAMES | custom | Change Map, Change Rules, Quick Options, and ours (Clanker Options..., Halo 1 Options...) | retail's five lines with ours (`lobby_options.rs`) | retail's countdown |
| FORGE | custom | Change Map (the original maps), Forge Map..., Change Rules | "New Forge map" or the saved map's name | retail's; the session loads the draft (`forge_glue::request_draft`) |
| FIREFIGHT | custom | Change Map (the Firefight category), Firefight Options... | wave set, difficulty, starting lives | retail's, with the setting held |
| CO-OP | co-op | Select Level, Difficulty (retail's `load_level_lobby` and `difficulty_lobby`) | retail's level and difficulty lines | retail's co-op start (`coop_live`'s start check) |
| CLANKERS | matchmaking | Change Playlist... (the Clanker playlists) | retail's playlist lines | starts the search; the console hosts and the Clankers fill it (`clankers::Driver::tick_match`) |

- **Game Setup has no way to change mode.** `lobby_mode::arrange` drops
  Change Mode... (action 14), retail's Switch To rows (actions 6 to 8)
  and Switch To: Clankermatch (action 12). In a matchmaking lobby
  action 9 stays as the one row and reads Change Playlist...; it opens
  retail's playlist screen, which is a picker.
- The FORGE and FIREFIGHT rows, panel lines and refusal are
  `lobby_mode.rs`'s ([forge-firefight-ui.md](forge-firefight-ui.md),
  "The lobbies").
- No row or tab steps through values with arrows. A choice is a tab or
  a picker.

### Other machines

A member's lobby shows the same strip without glyphs. Its selected tab
is `Tab::of` over what retail replicates to members (the lobby mode,
the playlist, the map) with the member's own readings: a playlist the
server marks as a Clanker one gives CLANKERS (`clankermatch::listed`'s
marks), and a kit map in a custom lobby gives FIREFIGHT
(`lobby_start::is_kit_map`).

FORGE is the one tab retail's data cannot tell from CUSTOM GAMES. On
Xbox Live the host's tab travels as one byte beside the session's
other fields of ours (`live_proto::session::NewSession`, read by the
member as the variant extension is, `variant_ext.rs`). A leader cannot
select FORGE with a remote member present, so a member reads it only
by joining a Forge lobby, which cannot start.

### Snapshots

The feature has no section of its own. The selected tab is derived, the
widgets are guest memory, and `lobby_start::State` is in the
`lobby-mode` section (`lobby_mode::save_state`), which gains one byte
for `clankermatch::active`. A snapshot is refused while a switch's
guest calls are in flight (`SnapshotPolicy::section_forbid_while`).
The reason line's timer is not saved.

### With the strip off

`-lobby-mode` leaves the retail lobby: its header, the player count in
its own rectangle, Game Setup with retail's Switch To rows and Switch
To: Clankermatch, and every lobby in the mode its menu row opened.
Forge and Firefight are then reached only by the level boot
([level-boot.md](../runtime/level-boot.md)). There is no second way to
pick a mode in the menus.

## Interfaces

- **Guest**, all named in `crates/hooking/guest-layout/src/points.rs`:
  `PREGAME_LOBBY_INPUT` (the buttons), `PANE_TEXT_ADD` and `TEXT_SET`
  (the widgets), `LOBBY_VARIANT_PANEL` (the update), `LOBBY_MODE_SET`,
  `LOBBY_SET_MAP`, `LOBBY_START_STATE`, `LOBBY_ALL_READY`,
  `HOPPER_LISTED`, `QUICKMATCH_HOPPER`, `SQUAD_SETTINGS_LIST_CTOR`.
  Points the slices add: the lobby screen's build (where the widgets
  are added), the playlist press's entry (`0x2b2181`), Switch To:
  Custom Game's press and Switch To: Co-Op Game's press past its gate.
- **Ours**: `lobby_start::{mode, set_mode}`, `lobby_mode::{arrange,
  remote_members, start_refused}`, `clankermatch::{active, set_active,
  listed}`, `map_select::mode_category`, and from the co-op lobby's
  owner one entry that arms co-op for a lobby already open
  (`coop_live`'s `want_coop_mode` and its start check, as the Xbox Live
  menu's row uses them).
- **Agent API** ([agent-api.md](../runtime/agent-api.md)):
  `ui.lobby.tabs` answers `{place, leader, selected, tabs: [{tab,
  label, state, reason}], notice}` with `state` one of `selected`,
  `on`, `off`; `ui.press` takes the actions `tab_prev` and `tab_next`.
  `ui.screen`'s layout findings cover the strip's widgets as any
  text's.
- **Preview**: `h2 ui preview lobby.tabs --state all` draws the retail
  lobby's pane with the strip in each place and on each tab
  (`ui_kit::preview::Retail` with added texts).

## Slices

Sizes as [forge-design.md](forge-design.md), "Features". Lanes write
code and tests on synthetic data; the agent-API checks are the
coordinator's, on a running game. Each slice lands alone and leaves the
lobby working.

| # | Slice | Size | Owns | Tests | Agent-API check |
|---|---|---|---|---|---|
| **T1** | The model, no hooks: `Tab`, `Place`, `Avail`, `parts`, `Tab::of`, `available`, `step`, `cells`, the labels and reasons | S | `lobby_tabs.rs`, `lobby_tabs/tests.rs`, the `mod` line in `lib.rs` | `every_tab_round_trips_through_its_parts`, `availability_by_place` (the table above, cell by cell), `a_tab_that_is_off_has_a_reason`, `a_feature_that_is_off_removes_its_tab`, `step_skips_tabs_that_are_off_and_names_the_first`, `the_ends_do_not_wrap`, `cells_stay_in_the_band_and_do_not_overlap`, `cells_close_up_without_a_tab`; `ui_kit::content::check` over every label and reason | none |
| **T2** | The strip drawn, read-only: the widgets on the lobby's build, the three looks, the header hidden, the count narrowed, the update; `lobby.tabs` in the preview; `ui.lobby.tabs` | M | `lobby_tabs/strip.rs`, `lobby_options.rs` (the update's call), `lobby_mode.rs` (`set_header` removed), `ui_kit/ids.rs` (the family), `ui_kit/preview.rs` and `ui_kit/screens.rs` (`lobby.tabs`), `agent_glue.rs` and `crates/extensions/agent-proto` (the method), `points.rs` (the lobby's build), [agent-api.md](../runtime/agent-api.md) | `FlatMemory` tests: `one_look_shows_per_tab`, `the_header_is_hidden_and_put_back_with_the_strip_off`, `the_strip_is_found_again_by_its_ids`, `a_member_has_no_glyphs`; `the_strip_takes_the_lobby_texts_animation`; disc tests (`#[ignore]`) `every_tab_label_fits_its_cell`, `the_player_count_fits_its_narrowed_rectangle`, `the_strip_cells_are_inside_the_header_band`; `string_ids.rs` over the family | Split Screen lobby: `ui.lobby.tabs` has `selected` CUSTOM GAMES, MATCHMAKING and CLANKERS `off` with their reasons; Change Mode... to Forge, then `selected` is FORGE; `ui.screen` has no layout finding |
| **T3** | Switching inside the custom lobby: the bumpers, leader only, the countdown lock, CUSTOM GAMES, FORGE and FIREFIGHT, the map into the tab's category, the reason line; `tab_prev` and `tab_next`; Change Mode... and its picker removed | M | `lobby_tabs/input.rs`, `lobby_tabs/switch.rs`, `lobby_mode.rs` (`arrange`, `MODE_ACTION`, `mode_pick`), `lobby_status.rs`, `ui_press.rs`, `ui_kit/view.rs` and `ui_kit/screens.rs` (`kit.picker.mode` removed), [switches.md](../runtime/switches.md), [firefight.md](firefight.md), [forge-design.md](forge-design.md), [map-select.md](map-select.md) | `a_member_cannot_switch`, `no_switch_during_the_countdown`, `black_and_white_are_handled_and_nothing_else_is`, `switching_to_firefight_holds_the_setting_and_picks_a_kit_map`, `switching_to_forge_keeps_a_halo_2_map`, `a_press_toward_a_tab_that_is_off_shows_its_reason_for_four_seconds`, `game_setup_has_no_mode_row` (the `Page` tables of `lobby_mode/tests.rs` per tab), `leaving_the_lobby_clears_the_mode` unchanged | Split Screen: `ui.press tab_next` from CUSTOM GAMES gives FORGE, then FIREFIGHT; `ui.lobby_options` has the tab's lines; `tab_prev` at CUSTOM GAMES stays and `notice` is Matchmaking's reason; Game Setup's rows in `ui.audit` have no Change Mode... |
| **T4** | CO-OP: the retail mode switch between 2 and 0 with the party kept, Switch To rows dropped from Game Setup | M | `lobby_tabs/switch.rs`, `lobby_mode.rs` (`arrange`), `points.rs` (the two Switch To presses) | `arrange_drops_the_switch_to_rows`, `co_op_runs_the_co_op_switch_after_ours_are_set`, `leaving_co_op_puts_the_custom_mode_back`, `co_op_is_off_on_system_link` | Split Screen with two profiles and an Xbox Live custom game with one remote member: `tab_next` to CO-OP, `ui.lobby.players` is the same roster, `ui.audit` shows the co-op pane and Game Setup's level and difficulty rows; back to CUSTOM GAMES the same |
| **T5** | MATCHMAKING and CLANKERS: the playlist a tab opens on, the entry with a hopper, Change Playlist..., the search lock, `clankermatch::active` in the snapshot section | M | `lobby_tabs/switch.rs`, `lobby_tabs/playlist.rs`, `clankermatch.rs`, `lobby_mode.rs` (the section's byte), `points.rs` (the playlist entry) | `the_tab_opens_on_the_last_playlist_when_listed`, `else_on_the_first_listed`, `no_listed_playlist_turns_the_tab_off`, `clankers_lists_only_clanker_hoppers`, `matchmaking_never_remembers_a_clanker_hopper` (`clankermatch::remembered` unchanged), `no_switch_during_a_search`, `the_section_round_trips_the_clanker_flag` | Xbox Live custom game: `tab_prev` to MATCHMAKING, `ui.audit` shows the matchmaking pane with a playlist named, the roster unchanged; Game Setup has only Change Playlist...; to CLANKERS, Start Game, and `ui.lobby.players` fills with Clankers |
| **T6** | The host's tab on Xbox Live: the byte in the session record, the member's read | S | `lobby_tabs/seen.rs`; with the Live owner, `crates/live/live-proto/src/session.rs` and `crates/live/live-hle` | `a_member_reads_forge_from_the_byte`, `without_the_byte_a_member_derives_the_tab`, `an_unknown_byte_is_refused_by_name` | two consoles on Xbox Live: the member's `ui.lobby.tabs` `selected` equals the host's on every tab |

T1 to T3 need nothing outside this lane and make the strip whole for
Split Screen and System Link. T4 needs the co-op lobby's entry, T5 the
Clanker playlists' marks, T6 the Live session field: each is an edit
in another owner's files, named in its row.

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
- A playlist tab always holds a listed playlist. With none listed the
  tab is off; it never opens on an unlisted one.
- On System Link a member that joins a Forge lobby reads CUSTOM GAMES,
  and the game cannot start, until slice F5 puts the Forge session on
  the network.
- Game Setup has no row that changes mode while the strip is on.

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
- That the retail Switch To presses and the playlist entry keep the
  squad session and its members when run from the lobby's input
  handler with no Game Setup open, and that members' lobbies follow
  each mode change.
- That `LOBBY_MODE_SET(0)` on a Split Screen squad gives the co-op
  lobby an Xbox Live squad gets.
- That entering mode 3 with a hopper from a Split Screen or System
  Link lobby is refused by the game. The tabs are off there, so no
  press reaches it.
- That the status text's write is reachable in every pane for the
  reason line, and that it composes with the Forge refusal and the
  "Setting up bots" text of `lobby_status.rs`.
- What Game Setup's action 9 reads in a matchmaking lobby before its
  text is ours.
- That the map set by a switch reaches members as a Change Map pick
  does.

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
