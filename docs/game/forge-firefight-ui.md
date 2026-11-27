# Forge and Firefight UI: every screen and HUD piece as Halo 2 draws it

Status: Partly built. Slices U1 (the main menu), U2 (the lobby modes), U3 (the monitor HUD on the HUD kit), K1 (the HUD kit's monitor mode), U4 (the Forge screens in the game and the bindings table), U5 (the Firefight HUD), K4 (the gizmo's segment in the HUD kit), U6 (the markers and the gizmo on the HUD kit) and U7 (the compositor path removed) are built, with K2 and K3, the HUD of a wave test (slice FA3 of [forge-firefight.md](forge-firefight.md)), the Waves screens (slice FA5), the Enemy types page (slice FA6) and the AI tool (slice FA7); the rest is design.
Code: `crates/extensions/game/src/forge_screens.rs` and `forge_screens/` (the Forge screens), `forge_screens/menu.rs` (the Forge menu and its tabs), `forge_screens/waves.rs` (the Waves screens), `forge_controls/bindings.rs` (the bindings table), `lobby_mode.rs` (Game Setup's Map Variant row, the start refusal), `lobby_start.rs` (the mode), `forge_hud.rs` (the monitor HUD's feed), `firefight/{hud,options}.rs`, `map_select.rs` and `map_modes.rs` (the map list's mode tags), `forge_glue/gizmo.rs` (the gizmo's ends), `hud_kit_core::views::forge` and `content/forge/huds/forge.hud.yaml` (the monitor HUD's elements), `theater_ui.rs` (`menu`, the main menu's rows), `crates/apps/cli/src/play_cmd/forge.rs` and `crates/apps/app/src/forge_overlay.rs` (the play window's keys and mouse, handed to the session). Tests: named per slice below.
Switch: none of its own. `forge` and `firefight`, listed in [switches.md](../runtime/switches.md).

Forge is a lobby mode and Firefight is a game type of the party lobby
([party-lobby.md](party-lobby.md)), with a HUD each and, for Forge,
screens and markers over the world. Theater is the one row added to the
main menu. This page audits what
draws each one, names the retail screen or HUD element each must copy, and
orders the work. The one rule: **every pixel of Forge and Firefight UI
comes out of a retail draw routine reading retail tags.** Menus are kit
screens the game's own widgets draw ([ui-builder.md](ui-builder.md)); HUD
pieces are HUD kit elements the game's own HUD draws (the HUD kit, below).
The compositor draws no Forge or Firefight UI.

Evidence labels are those of [docs-style.md](../meta/docs-style.md), plus
**C**: the retail Halo 3, Reach and Infinite as publicly documented
([notes/forge-prior-art.md](notes/forge-prior-art.md)).

## The two kits

- **The UI kit** builds a screen from a retail screen tag: its panel art,
  header layout, list skin, fonts, colours and animation tracks
  ([ui-builder.md](ui-builder.md), "Screen kinds", "The Dialog kind",
  "The value picker", "Transitions"). The game constructs and draws the
  widgets. `h2 ui preview` draws the same `View` offline.
- **The HUD kit** compiles elements (text, number, meter, icon, and the
  message, prompt, training and navpoint channels) into records of the
  game's own HUD format; the game's HUD code draws them with the map's
  bitmaps, shaders and fonts. Its model is `crates/rewrites/hud-kit-core`,
  its runtime `crates/extensions/game/src/hud_kit.rs`, its preview
  `h2 hud preview`, and its page `docs/game/hud-kit.md`. All four are on
  the branch `sb4-integ` and not on `master`; every HUD slice below needs
  that branch landed first.

Both kits hold one layout per screen or HUD: a state changes texts,
fills and what shows, never the widget set.

## Audit

Paths without a crate are under `crates/extensions/game/src/`.

### What draws it

| Piece | Drawn by | Code |
|---|---|---|
| Main menu row Theater | the retail main menu list, grown from 5 rows to 6 with one added item widget | `theater_ui.rs` (`menu::Rows::count`), `layout_glue.rs` |
| Forge and Firefight lobbies | the retail pregame lobby and its Game Setup, in the mode the lobby is in | `lobby_mode.rs`, `lobby_start.rs` (`Mode`), [party-lobby.md](party-lobby.md) |
| Firefight Options | the shared settings dialog on Quick Options over Game Setup, drawn by the game | `firefight/options.rs`, `content/ui/screens/firefight.yaml` |
| Forge HUD | the game's HUD: the HUD kit's `forge` HUD, fed from the session each tick ("The monitor HUD"); no reticle until `forge.reticle` is built | `hud_kit_core::views::forge`, `forge_hud.rs` |
| Forge screens (object menu, Properties, Tools, Tool settings, Link, questions, the Forge menu, Controls) | the game's own widgets on retail tags ("The Forge screens") | `forge_screens.rs`, `forge_screens/` |
| Gizmo | the HUD kit's `forge.gizmo` world marks | `forge_glue/gizmo.rs` |
| Monitor marker | the HUD kit's navpoints | `forge_hud.rs` (`Points`) |
| Firefight HUD | the game's HUD: the HUD kit's `firefight` HUD, fed from `hud::current` each tick ("The Firefight HUD") | `hud_kit_core::huds::firefight`, `firefight/hud.rs` (`Feed`) |

### Why each reads as fake

**Firefight Options.** The nearest to retail: the shared settings dialog
([ui-builder.md](ui-builder.md) "The settings dialog") on Quick Options'
panel, skin and picture, the skull icon and the window's lines at the
top, the focused row's help under the rows, held to the tag by
`our_settings_dialogs_are_quick_options_inside`. Two gaps: its legend
advertises D-pad left and right as "Change" (`settings::legend`, as every
page with value rows does), and its opener is Game Setup
(`opener: game-setup.firefight` in its doc).

**The play window draws nothing of Forge.** It hands its keys and mouse
to the session (`crates/apps/cli/src/play_cmd/forge.rs`,
`app::forge_overlay`) and composites no page: the monitor's HUD, its
navpoints and gizmo, and the Forge screens are the game's, so they keep
the HUD's safe frame, hide with the HUD and appear in films and
captures. The monitor has no reticle until `forge.reticle` is built.

## Targets

Each piece copies one retail screen or HUD element, by tag.

| Piece | Copies | Built through |
|---|---|---|
| Main menu | `main_menu` (screen 6): six rows, no scroll | the retail list with one added row, Theater ("The main menu") |
| Forge and Firefight lobbies | `pregame_lobby` (screen 14) itself, with `squad_settings_dialog` (screen 23) as its Game Setup | the retail screens in the lobby's FORGE and FIREFIGHT modes ("The lobbies") |
| Firefight Options, Forge map | `quick_options` (screen 222) and `low_level_settings` (screen 223) | the shared settings dialog (`DialogBase::QuickOptions`), the value picker; built |
| Forge menu | the in-game Controller Settings list (screen 200), a tab strip of text widgets above it | a Forge `List` screen titled FORGE whose rows change in place as its tabs step (`forge_screens::menu`); GAME MENU, the multiplayer pause screen, keeps the game's rows and the Video and Voice pages `pause_menu.rs` adds ([settings-menu.md](settings-menu.md), "Pause menu") |
| Object menu, Tools, pickers in a game | `low_level_settings`' list on `error_dialog_large` (screen 240) | `kit.picker.game` (`picker::offer_list`) |
| Properties, Tool settings, Controls, Coordinates | the in-game Controller Settings list (screen 200) | a Forge `List` screen on that host (`forge_screens::screen::ForgeScreen`), value rows opening `kit.picker.game`; the shared settings dialog has no in-game host, so these keep their own class (`settings::NOT_SHARED`). They have no icon: `mp_games2` is in `mainmenu.map` only, and one arrives with map injection |
| Link, delete, overwrite, leave | `error_dialog_ok_cancel`: body text and two rows | `DialogBase::Large` with a body |
| Save as, a typed coordinate | the game's keyboard | `Effect::Keyboard` |
| Contextual prompt | the pickup prompt line, top left | HUD kit `Prompt` |
| Refusals and confirmations | the pickup notification list | HUD kit `Message` |
| First-use teaching | the training box | HUD kit `Training` |
| Budget, a wave test's enemies left | a grenade pill with its LCD count | HUD kit `Icon` (`Art::GrenadePill`) and `Text` (`Font::Lcd`) |
| Budget fill, Firefight's enemies left | the energy sword's battery strip | HUD kit `Meter` (`Art::StripMeter`) |
| Firefight's score, lives and bonus clock | the HUD's own text lines, above the score plate | HUD kit `Text` (`Font::Hud`) |
| Firefight's set, rounds, waves and skulls | Halo 3: ODST's Firefight HUD for what is shown (a numeral, a circle a round, a bar a wave, a picture a skull); the pictures are strokes of ours drawn as the HUD draws a letter | HUD kit `Text` and `Gauge` (`font::skulls`) |
| Reticle | a retail weapon's reticle widget | HUD kit `Icon` (`Art::Widget`) |
| Wave and round banner | the timed centred line of the message routine | a HUD kit channel, `Banner` (slice K2) |
| Held object, other monitors, link ends | the game's navpoint: arrow, waypoint marker, number | HUD kit `Navpoint` |
| Axis gizmo | none: the owner's exception (decision 2) | HUD kit world mark `Shape::Segment`, in an axis colour (slice K4) |

Why these and not a styled overlay: the game's draw gives the fonts,
shaders, colours, the HUD fit at wide aspects, split screen, hiding with
the HUD, films and captures without our copying any of them (the HUD
kit's page, "Where the kit draws"). An overlay has to reimplement each
and still differs.

## The main menu

**The room** (**V**, [ui-builder.md](ui-builder.md), "Where retail puts
dialog text"). The logo ends at y -19 and the list is anchored at
(-178, -65); the first row's text is 28 units high, from -37 to -65. The
safe area ends at -211. Rows have 174 units.

| Rows | Pitch that fits (`(174 - 28) / (rows - 1)`) | Reads as |
|---|---|---|
| 5 (retail) | the skin's own | retail |
| 6 | 29 | retail: one unit of text gap |
| 7 | 24 | text boxes overlap by 4 |
| 8 | 20 | text boxes overlap by 8; below the 22-unit floor `custom_games::MIN_PITCH` keeps |

**S**: the skin's item height, read with `h2 ui mainmenu main_menu`; the
table's fits follow from the text box alone.

**The design: six rows.**

```text
   Campaign
   Xbox Live
   Split Screen
   System Link
   Theater
   Settings
```

- **Theater** is the one row of ours. Forge and Firefight are not on
  the main menu: each is a mode of the lobby that Split Screen, System
  Link and an Xbox Live custom game open, and the Xbox Live menu has a
  row for each ("The lobbies"). This is how
  Halo 3 and Reach have it: Forge is a lobby with a Network choice
  (**C**, [notes/forge-prior-art.md](notes/forge-prior-art.md)).
  Theater has no party to keep, so it is a row here and not a mode
  ([party-lobby.md](party-lobby.md), "The modes").
- Six rows is what the retail pane's item widgets plus the one added
  widget hold (`layout_glue.rs:1151`), so nothing scrolls.
  `theater_ui::menu` keeps its text and press hooks (`ROW_TEXT`,
  `ROW_CHOSEN`) with the one entry, placed after System Link;
  `Rows::count` returns 6 with Theater on and 5 with it off.
- The rows are re-pitched as the Xbox Live menu's are, so six never
  pass -211: the list layout hook (`custom_games::repitch`, matched on
  the handler's vtable `theater_ui::menu::VTABLE`) with
  `theater_ui::menu::pitch(rows)`, 29 for six rows.
- The label takes the case the retail rows have (the disc preview
  decides: see the audit).

## The lobbies

Forge is a lobby mode and Firefight is a game type of the party lobby
([party-lobby.md](party-lobby.md)): the retail pregame lobby
(`pregame_lobby`, screen 14) in its custom game mode, with a mode of
ours on top (`lobby_start::Mode`). The roster, the map picture, Start
Game, Game Setup and the variant panel are the game's. The GAME MODE
picker, who may switch, where each mode runs and what a switch does to
the party are that page's; this section is what the two modes make of
the lobby. Game Setup has no row that changes the mode.

| Mode | `lobby_start::Mode` | Shown when | The session |
|---|---|---|---|
| CUSTOM GAMES | Multiplayer | always | retail's |
| FORGE | Forge | the `forge` feature is on | a Forge session starts in the monitor; D-pad Up switches to the biped ([forge-editing.md](forge-editing.md), "Controls") |
| FIREFIGHT | Firefight | the `firefight` feature is on and a settings store holds its choices | Firefight on a kit map ([firefight.md](firefight.md)) |

- The modes are in every lobby that has Change Map: Split Screen, a
  System Link game being hosted, and an Xbox Live custom game, which
  the Xbox Live menu's Custom Games and Forge rows open in
  their mode. Only the party leader switches, so the mode is the
  leader's, as the map is.
- The mode is held per lobby. Leaving the party (`PARTY_LEAVE`) clears
  it, and so does building the main menu's or the Xbox Live menu's
  list. Snapshots keep it (the `party-lobby` feature's section, which has
  no switch).
- The monitor is offered only in a session whose mode is Forge. In a
  Multiplayer or Firefight session D-pad Up is the game's own. A game
  started with no lobby (the level boot,
  [level-boot.md](../runtime/level-boot.md)) is a Forge session, so an
  agent can drive one: until a lobby's list is built the state has no
  lobby, and `lobby_start::forge_session` is true.

**Game Setup's rows by mode**, set after the list is built and the
other row features have added theirs (`lobby_mode::arrange`, written
back by `h1_options_row::rewrite_rows`):

| Mode | Rows |
|---|---|
| CUSTOM GAMES | retail's, with ours as they are, and Map Variant... (action 15) after Change Map... while the `forge` feature is on |
| FORGE | Change Map..., Map Variant... (action 15), then the rest with Clanker Options...; Firefight Options... is hidden |
| Custom Games with Firefight selected | Change Map... lists compatible kits and, in the design, the maps that have a map variant with AI ground; Map Variant... lists those variants. Change Rules opens the game types and variants, Map Waves among the Firefight ones (a variant with its own set ends its picker line "Has its own waves.") ([forge-firefight.md](forge-firefight.md), "Playing it"). |

Every mode's list loses the Switch To rows: the GAME MODE picker changes the mode
([party-lobby.md](party-lobby.md#the-lobby-by-tab)). Clanker Options...
seats its Clankers in all three modes
([party-lobby.md](party-lobby.md#clankers-by-tab)).

- **Map Variant...** opens the value picker (`kit.picker.map-variant`)
  over Game Setup with Original and the map variants saved for the lobby's
  map (`lobby_mode::map_variant_list`, over `forge::files::variants::list`);
  a variant that cannot be used is listed with its sentence as its line and
  A on it stays in the picker. The picker opens on a map variant the lobby's
  game variant suggests (`lobby_mode::map_variant_list_for`) when none is held.
  Its help line names the chosen variant. In
  FORGE the pick is the variant's file stem, the draft the session opens
  with (`lobby_start::forge_map`), and the start gate's census applies
  (`rules::gate`, `variants::load`); in CUSTOM GAMES it is the variant the
  game is built from (`map_variant::pick_installed`), and the variant
  panel's line under the map's is its name.
- **Change Map...** opens the installed multiplayer catalogue. A selected
  Firefight variant opens its compatible kit maps directly. B from those
  maps returns to Game Setup. With no compatible kit installed, the list
  is empty and Start Game reports the missing kit
  ([map-select.md](map-select.md)).
- **The game type row** in Forge picks the engine the session runs, and
  so which goals the object menu offers
  ([forge-editing.md](forge-editing.md), "The object menu"). It is
  Halo 3's Change game type (**C**). It opens on Slayer.
- **The variant panel** shows the mode's choices first in the lobby's
  own five option lines through `lobby_options.rs`: "New Forge map" or
  "Forge map" and the name for Forge; the wave count, difficulty and
  starting lives for Firefight (`lobby_mode::panel_lines`).
- **The title** is the lobby's header, which names the mode: FORGE or
  FIREFIGHT ([party-lobby.md](party-lobby.md), "The header and the
  prompt").
- **Start Game** is retail's. Forge takes the chosen draft through
  `forge_glue::request_draft` when the game starts. A Firefight record
  starts the director with the wave set named by its hash.
- **With players on other machines.** Firefight in a hosted lobby is the
  custom game it is with the setting on ([firefight.md](firefight.md),
  "Live"). Forge with a remote member needs the session log on the
  network (slice F5); without it FORGE is off while a remote
  member is present, and a Forge lobby a remote member then joins has
  Start Game refused with "Forge with players on other consoles is not
  available." The refusal holds the lobby's start state below ready
  (`LOBBY_START_STATE`, 7 for 9), so Start Game does nothing, and the
  sentence is the lobby's status line (`lobby_status.rs`), where retail
  writes its own reasons. A remote member is a squad session peer other
  than the local one that is not a phantom
  (`lobby_mode::remote_members`). A second local player joins as in
  Split Screen and plays as a player; a monitor for each local seat
  comes with F5's holds.

## Where the sections live

| Page | Covers |
|---|---|
| [forge-firefight-ui-session.md](forge-firefight-ui-session.md) | the monitor HUD, screens, markers, AI tool and Waves screens |

Each heading below is cited from code and moved to the page named after it.

## The Forge in-session UI

Moved to [forge-firefight-ui-session.md](forge-firefight-ui-session.md).

### The monitor HUD

Moved to [forge-firefight-ui-session.md](forge-firefight-ui-session.md).

### The screens

Moved to [forge-firefight-ui-session.md](forge-firefight-ui-session.md).

### Markers in the world

Moved to [forge-firefight-ui-session.md](forge-firefight-ui-session.md).

### The AI tool

Moved to [forge-firefight-ui-session.md](forge-firefight-ui-session.md).

### The Waves screens

Moved to [forge-firefight-ui-session.md](forge-firefight-ui-session.md).

## The Firefight HUD

A HUD kit `Hud` named `firefight` (`hud_kit_core::views::firefight`),
fed by `hud::view_on` from the mode blob as it is ([firefight.md](firefight.md),
"The HUD"; [hud-kit.md](hud-kit.md), "The Firefight HUD"): the score,
lives and progress lines, the enemies meter, the set's numeral with the
round and wave pips, the seven skull slots, all at the frame's bottom right
above the retail score plate, and the banner, which is the game's own timed
centred line (the `Banner` channel, K2). What is shown follows Halo 3:
ODST's Firefight HUD. The elements, their places and their checks are
there; a banner is an event (a set, round, skull or wave that begins), not
a stage.

**While editing** (built, FA3). A wave test in a Forge session draws
the Firefight HUD from the same mode blob, by what the local user is
(`hud::Scope`, `Scope::of(testing, in_monitor)`):

| The user is | `ff.banner` | `ff.enemies`, `ff.progress`, `ff.score`, skulls | `ff.lives` | The monitor HUD |
|---|---|---|---|---|
| in the monitor | shown | hidden, the pips too: the monitor's own pill shows the count | hidden | as it is, with `forge.enemies.*` in the grenade row's third place, after the budget and the rebuild pills |
| in the biped | shown | shown, in their match places | hidden: a test charges no lives | not drawn |

- `hud::view_on(mode, slot, scope, screen, banner)` takes the scope
  with the roster slot (`Scope::Match`, `Biped`, `Monitor`), so the
  client's view stays a pure function of the blob, the slot, the scope,
  the screen and the banner. The feed takes each local user's scope each
  tick (`forge_controls::monitor`); the monitor's layout is the banner
  alone, and the monitor HUD's count is `hud::test_enemies`.
- WAVE n, BOSS WAVE and WAVE CLEAR read over the monitor as over a
  player; the end of a test says "Waves cleared." or "Waves stopped." as
  a `forge.notice`, not VICTORY or GAME OVER.
- The tool line keeps the tool's name. Nothing else of the monitor's
  HUD changes in a test, and with no test the monitor draws nothing of
  Firefight.
- The monitor's motion tracker shows the actors as it shows any enemy
  (decision 3).

## Slices

Sizes as [forge-design.md](forge-design.md), "Features": **S** under 800
lines with tests, **M** under 2,500, **L** more. Lanes write code and
tests on synthetic data. K slices extend the HUD kit and sit before
their first user. The editing slices P1 to P6 are in
[forge-design.md](forge-design.md), "Slices".

| # | Slice | Size | Needs | Proofs |
|---|---|---|---|---|
| **U2** (built) | Lobby modes: `lobby_start::Mode` (picked in the party lobby's GAME MODE picker, [party-lobby.md](party-lobby.md)), Game Setup rows by mode, Map Variant picker, the map list's pane of supported modes, variant panel lines, the monitor only in Forge mode, the remote-member refusal; delete the kit lobbies | M | nothing | `Page` tables for Game Setup in each mode (`mode_lists_only_enabled_features`, `forge_mode_lists_change_map_and_forge_map`, `leaving_the_lobby_clears_the_mode`, `forge_with_a_remote_member_is_refused`, `the_monitor_needs_forge_mode`); `h2 ui preview kit.picker.map-variant`; `lobby_options` tests for the panel lines; content checks (no colon values, no repeated spaces) |
| **U1** (built) | Main menu: six rows. Remove the FORGE and FIREFIGHT entries, place Theater after System Link, re-pitch if needed | S | U2, landed together | table tests `the_main_menu_has_six_rows_with_theater`, `five_rows_without_theater`, `pitch_keeps_six_rows_above_the_footer`; the `main-menu` opener's preview with six rows; the layout checks |
| **U4** (built) | Forge screens in the game: the bindings table and Halo 3 pad gate, object menu, Properties from a table, Tools picker, Tool settings (Rotation snap), Link, questions, the Forge menu and its tabs, Controls; input handover | L | nothing | `h2 ui preview forge.objects`, `forge.properties`, `forge.tools`, `forge.settings`, `forge.link`, `forge.question`, `forge.menu`, `forge.controls --state all --animation all`; `Page` tables carried over from `ui_kit/forge/tests.rs` plus `x_opens_objects_or_properties_by_aim`, `no_is_focused_on_every_question`, `every_action_has_one_binding`, `the_controls_page_lists_the_bindings_table`, `game_menu_has_the_retail_rows_in_a_forge_game`, `back_opens_the_forge_menu_only_in_the_monitor`, `swapping_to_the_biped_closes_the_forge_menu`, `lb_and_rb_move_between_tabs`, `the_forge_menu_lists_the_old_forge_rows`; a gate table test that RB and LB rise and fall, RT held manipulates, hold Y deletes and B cancels; `list_class` runner test that a picker opens with its pick in a game; layout and content checks in every state; a disc test that each base resolves in `shared` |
| **K1** (built) | HUD kit: monitor mode (retail widgets gated off per user but the motion tracker, grenade origins without the row) | S | the HUD kit on `master` | `hud_kit_tests`: a retail widget of a monitor user is not drawn, the motion tracker is, and another user's widgets are; `h2 hud preview forge --state all --mode all --synthetic` |
| **U3** (built) | Forge HUD on the kit: `forge` HUD, feed from `forge_glue`, copy table with buttons from the bindings table. Not in it: the reticle (`forge.reticle`), the retail widgets' gate (K1), the gizmo and markers (U6) | M | U4's bindings | `h2 hud preview forge --state all`; the kit's checks (fit, glyphs, frame, retail, own, one layout, prompt, drawn) in every state and mode; `every_tool_state_has_one_prompt`, `the_prompts_read_as_the_game_does`, `the_budget_and_the_rebuilds_follow_the_view`, `each_hint_fits_the_training_box`, `the_pills_are_the_grenade_rows_first_places` (kit); `the_buttons_are_read_from_the_table`, `every_button_has_a_glyph_and_every_row_a_cue`, `a_notice_is_sent_once_each`, `a_hint_is_shown_once_for_its_time` (`forge_hud_tests.rs`); `each_yaml_is_the_hud_its_rust_was` |
| **K2** (built) | HUD kit: the `Banner` channel (the timed centred line) | S | the HUD kit on `master` | `FlatMemory` test of the text, start and duration fields (`the_banner_is_written_as_the_game_writes_its_timed_line`, `the_banner_is_written_on_a_change_of_text`); the prompt check extended to it; preview draws it at the routine's place (`the_banner_is_drawn_centred_above_the_frames_bottom`) |
| **U5** (built) | Firefight HUD on the kit | M | K2; kit `Message` | `h2 hud preview firefight --state all --mode all` (`the_firefight_hud_draws_and_passes_on_the_synthetic_sheet`); `the_hud_reads_only_the_mode_blob` unchanged; `every_stage_has_a_banner_or_none` table; the kit's checks replace `hud::check`; `the_feed_publishes_the_view_and_sends_the_notes_a_change_makes` |
| **K3** (built) | HUD kit: several navpoints for one HUD, the number drawn | S | the kit's navpoint number | `FlatMemory` test of n slots kept and released; preview with three points |
| **K4** (built) | HUD kit: `Shape::Segment`, a line between two world points at any slope, and `Style::Axis(n)`, one of three axis colours | S | the HUD kit on `master` | `a_segment_runs_between_its_two_points_unlifted` (`world.rs`); `a_gizmo_axis_is_a_segment_in_its_colour` (`hud_kit_world.rs`); `the_preview_draws_a_gizmo_segment_in_its_axis_colour` (the colour per axis, the `world` check on a fourth axis and on an axis off a world mark) |
| **U6** (built) | Forge markers as navpoints; the gizmo as three segments in the Axes setting's basis; remove the marker and gizmo painters | S | K3, K4, U3 (built) | table test: held, aimed, linking and two monitors give the expected points and segments; `h2 hud preview forge --state holding` |
| **U7** (built) | Remove the compositor path: `ScreenSpec::hud` and the `hud` screen kind, the Forge painter in `play_cmd/forge.rs` (the window keeps only the input hand-over), `forge.hud`, `forge.palette`, the compositor's menu page, the reticle page and their sheet entries. `Art::Solid` stays: Theater's playback page draws its bar with it; update [ui-builder.md](ui-builder.md) "Screen kinds" | S | U3 (built), U4, U6 | `cargo check -p cli --no-default-features --features host`; the sheet has no `forge.hud`; `docs-check` |

The AI tool, the Waves screens and the HUD of a wave test are slices
FA7, FA5 and FA3 of [forge-firefight.md](forge-firefight.md), "Slices",
with their proofs: `Page` tables for each page, the kit's checks on the
`forge` and `firefight` HUDs in every state and mode, and Studio's
`ui.render` and `hud.render` of each for the report.

The GAME MODE picker that replaces U2's Mode row is slices T1 to T7 of
[party-lobby.md](party-lobby.md). U2 and U4 need neither kit change and can start at once. U1 lands with
U2, so that Forge and Firefight are never without a way in. The kit slices are all built.

## Interfaces

- **HUD kit**: `hud_kit::register`, `Handle::publish`,
  `Handle::navpoint`; added here `Handle::retail(bool)` (K1), the
  `Banner` kind (K2), `Handle::navpoints(&[Point])` (K3) and the
  `Shape::Segment` and `Style::Axis` of a world mark (K4).
- **UI kit**: `list_class::open` for screens in a game;
  `picker::offer_list`; `Effect::Keyboard`. No new screen base.
- **Guest**: no new entry. The lobby header widget is reached through
  `TEXT_SET`, Game Setup's rows through `ROW_ACTION`; the banner's
  fields are the message state's timed text (`+0x454`, `+0x4d8`,
  `+0x4dc`, **E** in the HUD kit's page).
- **Agent API**: no new method. `ui.screen` and `ui.press` see the
  Forge screens, which are game screens; the lobby's mode is `ui.lobby.tabs`
  ([party-lobby.md](party-lobby.md), "Interfaces"); `hud.kit` (the HUD
  kit's build plan) reads the HUDs; `forge.status` and `firefight.state`
  are unchanged. The design adds `forge.waves` and `forge.waves.test`
  ([forge-firefight.md](forge-firefight.md), "Interfaces").

## Rules and limits

- No Forge or Firefight pixel is drawn by the compositor, and no colour,
  rectangle or font in their UI is a constant of ours: each is a tag
  field or a retail address read at run time.
- A choice opens a picker and a number opens the keyboard. No row draws
  arrows or steps through values as its only way to change.
- Copy: capitals in titles and HUD text widgets, sentence case in rows
  and prompts, single spaces, no colon between a label and its value, no
  "..." cut of a sentence. `ui_kit::content::check` holds the menus to
  it; the HUD kit's `prompt` and `fit` checks hold the HUD.
- One layout per screen and per HUD.
- The main menu has six rows at most and never scrolls. GAME MENU has
  no Forge row: a Forge row goes on a tab of the Forge menu.
- A row, group or tool that is not built, or whose feature is off, is
  absent, never greyed. A lobby mode that cannot run in this lobby is
  the one thing listed with its reason, in the GAME MODE picker
  ([party-lobby.md](party-lobby.md), "Where each mode runs").
- A screen or element with no retail counterpart is not built: it is
  changed until it has one, or dropped. The axis gizmo is the owner's
  exception (decision 2).
- A map whose interface lacks a base (Quick Options in a game) uses the
  base that is there (`error_dialog_large`, the Controller Settings
  host), never art of ours.

## Decisions

The owner's choices, which the sections above follow.

1. **Buttons.** Halo 3's mapping: X opens the object menu, or
   Properties with an object aimed at or held; hold Y deletes; B
   cancels a hold and backs out. It is the default of the one bindings
   table (`forge_controls::bindings::BINDINGS`).
2. **The axis gizmo stays** for precise turning. It gets a HUD kit
   version (slices K4 and U6) instead of being removed; no screen
   assumes it is gone.
3. **The motion tracker stays visible in monitor mode.** K1 gates every
   other retail widget off for a monitor user.
4. **Forge and Firefight are lobby modes**, as Halo 3 and Reach have
   Forge, picked in the party lobby's GAME MODE picker
   ([party-lobby.md](party-lobby.md)). The main menu has six rows, with
   Theater as the one of ours.
5. **Halo 2's look, the later games' behaviour.** Screens and HUD are
   Halo 2's widgets; what they do takes after Halo 3, Reach and
   Infinite ([forge-editing.md](forge-editing.md), "What it takes
   after").
6. **Player features are on by default.** A mode, tool or row shows
   whenever its feature is on and it is built.

## Evidence

- **Code** Every audit row: the path and line beside it.
- **V** The main menu's logo, list anchor and first row, the dialog
  tags' contents, Quick Options' and `low_level_settings`' parts, the
  safe area: [ui-builder.md](ui-builder.md).
- **E** The HUD's parts, the prompt, notification, training and navpoint
  routines and the timed centred line: the HUD kit's page on `sb4-integ`.
- **C** Halo 3's and Reach's lobbies, buttons and Start menu:
  [notes/forge-prior-art.md](notes/forge-prior-art.md), with its
  sources and what none of them confirms.
- **S** The main menu skin's item height; that the menu's font has lower
  case; which retail reticle and pill icons are used; that the retail
  score display shows a Firefight score; that the game's HUD has a line
  draw the `Segment` kind can use; K4 draws it through the window's world-line pass instead.

## Not verified

- That a row added to Game Setup shows in every lobby that opens it
  (Split Screen, a hosted System Link game, an Xbox Live custom game).
- That hiding, adding and reordering Game Setup's rows leaves each row
  drawing its own text and working.
- That the Map Variant picker opens on the dialogs' window over Game
  Setup, and that the variant panel updates when the lobby returns.
- That the status line's text call covers every start state the
  refusal shows in, and that holding the start state composes with the
  co-op lobby's hook on the same routine.
- Whether six main menu rows need the re-pitch, and the case the
  Theater label takes beside the retail rows.
- What a client's lobby shows for a host in the FORGE or FIREFIGHT
  mode ([party-lobby.md](party-lobby.md), "Other machines").
- That a kit screen on the dialogs' window opens over a running
  multiplayer game without pausing it, takes the pad from the player and
  gives it back on close.
- That a list page opened through the game's screen opener in a game
  draws over the world, and that the Forge menu's rows push their page
  above it on the same window and slot.
- That the Forge menu's strip sits between the title and the first row
  on the in-game Controller Settings (its cells are placed from the
  Controls tag's header and list anchor), and that the list sizes to a
  tab's rows in place there as Theater's library does on its dialog.
- That `ui_watch` reads no screen over a running game with nothing up,
  so the monitor takes its input (`forge_screens::ui_up`).
- Which dialog bases besides `error_dialog_large` are in `shared`.
- That the axis colours (`world::AXIS_COLOURS`, red, green and blue) read
  against the level; they are ours, not the HUD's colour table.
- Everything the HUD kit's page lists as not verified, which U3, U5 and
  U6 inherit.
- That the grenade row has a third place clear of the motion tracker and
  the retail panel in every screen mode and at wide aspects, for
  `forge.enemies.*`.
- That four list pages opened one above the other on the Controller
  Settings host come back in order with B, and that a picker opens over
  the fourth.
- That the banner channel draws for a user whose HUD is in monitor mode.

## Related

- [forge-map-variants-ui.md](forge-map-variants-ui.md): the readout, world marks, Properties rows by kind and the map variant screens.
- [forge-firefight.md](forge-firefight.md): what the AI tool and the Waves screens write, the wave test, and their slices.
- [party-lobby.md](party-lobby.md): the lobby's modes and the GAME MODE picker.
- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md): the UI kit.
- [forge-editing.md](forge-editing.md): what the Forge screens do.
- [forge-design.md](forge-design.md) "The UI", [forge.md](forge.md): Forge.
- [firefight.md](firefight.md) "The HUD", "Game Setup", "The lobby": Firefight.
- [settings-menu.md](settings-menu.md) "Pause menu", [map-select.md](map-select.md).
- [hud-editor.md](../editor/hud-editor.md), [native-post-hud.md](../graphics/native-post-hud.md), [button-icons.md](../input/button-icons.md).
- The HUD kit's page `docs/game/hud-kit.md` and the monitor look's page `docs/game/forge-monitor-appearance.md`, on the branches `sb4-integ` and `forge-look`.
