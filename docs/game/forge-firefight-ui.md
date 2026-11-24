# Forge and Firefight UI: every screen and HUD piece as Halo 2 draws it

Status: Partly built. Slices U1 (the main menu), U2 (the lobby modes), U3 (the monitor HUD on the HUD kit), K1 (the HUD kit's monitor mode), U4 (the Forge screens in the game and the bindings table), U5 (the Firefight HUD), K4 (the gizmo's segment in the HUD kit), U6 (the markers and the gizmo on the HUD kit) and U7 (the compositor path removed) are built, with K2 and K3; the rest is design.
Code: `crates/extensions/game/src/forge_screens.rs` and `forge_screens/` (the Forge screens and GAME MENU rows), `forge_controls/bindings.rs` (the bindings table), `lobby_mode.rs` (Game Setup's Map Variant row, the start refusal), `lobby_start.rs` (the mode), `forge_hud.rs` (the monitor HUD's feed), `firefight/{hud,options}.rs`, `map_select.rs` (the Firefight category), `forge_glue/gizmo.rs` (the gizmo's ends), `hud_kit_core::views::forge` and `content/forge/huds/forge.hud.yaml` (the monitor HUD's elements), `theater_ui.rs` (`menu`, the main menu's rows), `crates/apps/cli/src/play_cmd/forge.rs` and `crates/apps/app/src/forge_overlay.rs` (the play window's keys and mouse, handed to the session). Tests: named per slice below.
Switch: none of its own. `forge` and `firefight`, listed in [switches.md](../runtime/switches.md).

Forge and Firefight are modes of the party lobby
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
| Forge screens (object menu, Properties, Tools, Tool settings, Link, questions, Forge options, Controls) | the game's own widgets on retail tags ("The Forge screens") | `forge_screens.rs`, `forge_screens/` |
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
| Forge menu | the multiplayer pause screen, GAME MENU ([settings-menu.md](settings-menu.md), "Pause menu") | five rows added to the retail list, as `pause_menu.rs` adds Video and Voice |
| Object menu, Tools, pickers in a game | `low_level_settings`' list on `error_dialog_large` (screen 240) | `kit.picker.game` (`picker::offer_list`) |
| Properties, Tool settings, Forge options, Controls, Coordinates | the in-game Controller Settings list (screen 200) | a Forge `List` screen on that host (`forge_screens::screen::ForgeScreen`), value rows opening `kit.picker.game`; the shared settings dialog has no in-game host, so these keep their own class (`settings::NOT_SHARED`). They have no icon: `mp_games2` is in `mainmenu.map` only, and one arrives with map injection |
| Link, delete, overwrite, leave | `error_dialog_ok_cancel`: body text and two rows | `DialogBase::Large` with a body |
| Save as, a typed coordinate | the game's keyboard | `Effect::Keyboard` |
| Contextual prompt | the pickup prompt line, top left | HUD kit `Prompt` |
| Refusals and confirmations | the pickup notification list | HUD kit `Message` |
| First-use teaching | the training box | HUD kit `Training` |
| Budget, lives, enemies left | a grenade pill with its LCD count | HUD kit `Icon` (`Art::GrenadePill`) and `Text` (`Font::Lcd`) |
| Budget fill | the energy sword's battery strip | HUD kit `Meter` (`Art::StripMeter`) |
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

Forge and Firefight are modes of the party lobby
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
| FORGE | Forge | the `forge` feature is on | a Forge session: D-pad Up enters the monitor ([forge-editing.md](forge-editing.md), "Controls") |
| FIREFIGHT | Firefight | the `firefight` feature is on and a settings store holds its choices | Firefight on a kit map ([firefight.md](firefight.md)) |

- The modes are in every lobby that has Change Map: Split Screen, a
  System Link game being hosted, and an Xbox Live custom game, which
  the Xbox Live menu's Custom Games, Forge and Firefight rows open in
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
| FIREFIGHT | Change Map..., Firefight Options..., then the rest with Clanker Options...; the game type row is hidden |

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
- **Change Map...** opens the map select on the mode's category: the
  original maps for Forge, a **Firefight** category of kit maps for
  Firefight ([map-select.md](map-select.md), `map_select::mode_category`).
  B from those maps returns to Game Setup. A switch to the mode puts the
  lobby's map inside the category ([party-lobby.md](party-lobby.md),
  "Switching"). With no kit map installed FIREFIGHT still opens: the
  Firefight category is empty, so Change Map opens the retail map list,
  and the panel and Start Game say no installed map supports Firefight
  yet ([party-lobby.md](party-lobby.md), "Where each mode runs").
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
  `forge_glue::request_draft` when the game starts; Firefight holds the
  `firefight` setting on for the game, which switching to the mode sets.
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

## The Forge in-session UI

The behaviour (what each button does, the tools, the settings, the
properties by kind) is [forge-editing.md](forge-editing.md). This
section is how each piece is drawn.

### The monitor HUD

A HUD kit `Hud` named `forge`, registered by `forge_glue` and published
when the tool's state changes. One layout:

| Element | Kind | Place | Shows |
|---|---|---|---|
| `forge.reticle` (not built) | Icon, `Art::Widget` of a retail weapon reticle | centre | always in the monitor |
| `forge.prompt` | Prompt | the prompt line | the one action A does: "Press (A) to pick up Beacon", "Press (A) to place Beacon", "Press (X) for the object menu"; while manipulating, the value that is changing ("Heading 45") |
| `forge.notice` | Message | the notification list | a refusal ("The object budget is full."), Saved, Loaded, a setting changed by a button ("Fine movement on"); it fades as a pickup line does |
| `forge.teach` | Training | the training box | the first time a state is reached: hold Y to delete, hold RT to turn, Start for the menu |
| `forge.budget.pill`, `forge.budget.pill.end` | Icon, `Art::GrenadePill` | the grenade row's first place | always |
| `forge.budget.count` | Text, `Font::Lcd`, longest "9999" | on the pill | objects left to place |
| `forge.budget.meter` | Meter, `Art::StripMeter` | under the pill (`rects::below`) | used over limit; `Flash(Pulse::TimeWarning)` from 90% |
| `forge.tool` | Text, `Font::Hud`, longest "VOLUMES" | top right, where a weapon's name line sits | the tool; capitals |
| `forge.rebuild.pill`, `.pill.end`, `.count` | Icon and LCD Text | the grenade row's second place | pending Rebuild edits; hidden at 0 |
| `forge.nav.target` | Navpoint | over the held, aimed or placed thing; in the Links tool, over the aimed end | while there is one; the number is the distance in metres |
| `forge.nav.from` | Navpoint | over the link's first end | while a link is started |
| `forge.nav.monitor.1`, `.2` | Navpoint | over the nearest two other monitors | while another seat's monitor is up |
| `forge.gizmo.x`, `.y`, `.z` | world mark, `Shape::Segment` in `Style::Axis(0..3)` | from the held or aimed object's origin to its end along each axis, in world space | in the Objects tool while an object is held, placed or aimed at |

- **The retail widgets are off in the monitor, but the motion tracker.**
  A monitor has no shield, weapon or grenades. The kit's gate hook
  answers for the retail widgets of a user whose HUD is in monitor mode
  (`Handle::retail(user, false)`, slice K1), except the motion
  tracker, which stays drawn (decision 3). The grenade row's origins
  (`Origin::NextGrenadePill`) then resolve to the row's first place.
- **No reticle yet.** The kit has no reticle element until a retail
  weapon's reticle widget is chosen from the disc census (**S**): a text
  at the screen's centre is inside the retail reticle's reserved
  rectangle and fails the `retail` check, so the monitor draws none.
- **Copy** is the game's form: sentence case, the button as the font's
  glyph, one line. The count is digits only, as the LCD font has no
  slash. A prompt's button is read from the bindings table
  (`forge_controls::bindings::BINDINGS`), never written in the string.
- **No legend bar.** The full control list is the **Controls** page.
- **The feed** (`forge_hud.rs`): the editor publishes the kit's `View`
  each tick (`Editor::monitor_hud`), the tool, what the hand has, the
  budget, the rebuilds pending, and the heading while a held thing is
  turned with RT. The feed publishes the values when they change and
  sends the session's notice once, as a notification: a sentence the
  list cannot keep whole sends its first sentence, and one with no short
  first sentence none (`forge_hud::notice_line`). A hint is shown the
  first time its state is reached, for seven seconds, and not again
  (`forge_hud::Teaching`); the Links tool has none. The prompt names a
  name over 24 characters by its verb alone.
- The agent API's `forge.status` carries the same values; `hud.kit`
  reports what the gate drew.

### The screens

Each is a list class of ours on a retail screen tag
(`forge_screens::sheet::Look`): the large dialog, `error_dialog_large`
(screen 240), laid out as the value picker for the object menu and
Tools and with a body for Link and the questions; the in-game Controller
Settings (screen 200) for the list pages. Both are in `shared.map`
(`each_base_resolves_in_shared`). A screen asked for opens at the next
UI update for player one, once nothing is pending on the dialogs'
window (`forge_screens::step`): a dialog class over the screen beneath,
a list page through the game's screen opener. While a screen is up, ours
or the game's (GAME MENU, the picker, the keyboard), the monitor's
camera and tool take no input (`forge_screens::ui_up`, published to
`forge_controls::State`, `screen`).

- **Who builds what.** The session builds the sheets that need its
  document: the object menu, Properties, Link and the questions
  (`forge_screens::open`). The others are built from what the session
  publishes each tick (`forge_screens::Facts`): Tools, Tool settings,
  Forge options and Controls (`forge_screens::request`). One screen of
  ours opens at a time; a second is refused by name.
- **Choices.** A press sends a `sheet::Choice` to its screen's outbox,
  which the session drains each tick. Yes to Leave Game stays in
  `forge_screens`: it leaves as the title leaves a game
  (`host_lobby::LEAVE_SESSIONS`, then the main menu).

| Screen | Opened by | Built as | Slice |
|---|---|---|---|
| Object menu | X with nothing aimed at | `kit.picker.game` twice: groups, then the group's objects | U4 |
| Properties | X on the thing aimed at or held | a list page on the in-game Controller Settings host; rows from a table by kind | U4 |
| Tools | D-pad Right | `kit.picker.game` over the tools that are built | U4 |
| Tool settings | D-pad Down; a row of Forge options | a list page on the same host, a row per setting | U4, rows in P1 |
| Overlays | Back | a list page on the same host as Tool settings, a row per overlay | |
| Link | X in the Links tool | a large dialog with a body and two rows | U4 |
| Questions | delete with links, overwrite, leave unsaved | the same dialog with Yes and No | U4 |
| Forge menu | Start | the game's GAME MENU with rows added | U4 |
| Forge options | a row of the Forge menu | a list page on the Controller Settings host | U4 |
| Controls | a row of Forge options | a list page made from the bindings table | U4 |
| Coordinates | a row of Properties | a list page on the same host; A opens the keyboard | P1 |
| Placed objects | a row of Forge options | `kit.picker.game` over the objects placed, each flying the monitor to it | built |
| Map limits | a row of Forge options | a list page of facts | P4 |

- **Object menu.** Each picker has its six rows, scroll arrows and
  description block. The groups come from a table
  ([forge-editing.md](forge-editing.md), "The object menu"); a group
  with nothing on this map is absent. The description is the object's
  line: what it is, how many are placed, how many are left. A on an
  object closes both and holds a new one. The picker opens on the group
  and object last chosen.
- **Properties.** The object's name is the header. A value row has its
  value in the skin's value column and A opens the picker; an action row
  (**Coordinates...**, **Reset orientation**, **Duplicate**) acts. No
  row draws arrows. Coordinates and Heading are present for every object,
  Team for starting points and goals, Role and Order for goals, and
  Respawn time for item spawners (`properties(group, kind)`), so a later
  slice adds rows to the table and no screen.
- **Tool settings.** Rows Movement snap, Rotation snap and Axes, each a
  picker (`Tool::set_movement_step`, `Tool::set_rotation_step`,
  `Tool::set_axes`, `sheet::SETTINGS`). Fine movement is the stick click's
  toggle.
- **Link.** The body lists the two ends and what the link writes, with
  rows **Remove link** and **Done**: retail's two-button question.
- **Questions.** **Yes** and **No**; No has the focus.
- **Forge menu.** GAME MENU with five rows added above the retail rows:
  **Save**, **Save as**, **Load**, **Rebuild map** (with the pending
  count as its value) and **Forge options...**
  (`forge_screens::game_menu::ROWS`). The list's constructor makes its
  rows, then `put_first` adds ours and moves the action words so ours
  come first; the rows' array is grown for them at `LIST_DATA_NEW`
  (`forge_screens::game_menu_count`). Load opens the picker over this
  map's saved variants, then asks before it replaces unsaved edits;
  Save opens the keyboard for a session with no name; Save as the
  keyboard, then the replace question when the name is taken
  ([forge-map-variants-ui.md](forge-map-variants-ui.md), "Saving,
  loading and renaming"). Rebuild map is refused by name, as the session
  cannot rebuild yet. Leave Game is retail's, behind the unsaved
  question. These are the rows Halo 3's Start has (**C**); everything
  else is one screen down so that GAME MENU never scrolls.
- **Forge options.** Tool settings... ("Rotation snap and the other
  tool settings.") and Controls ("Every Forge control, with its
  button."), `sheet::OPTIONS`, each with its help and each opening
  above it so B comes back; Monitor
  look... joins when it is built, **Build AI ground**, **Test waves** and **Generate nav**
  ([forge-map-variants.md](forge-map-variants.md), "AI ground";
  [forge-nav.md](forge-nav.md); they act
  and close the page), and the rows later slices bring:
  Placed objects, Map limits (P4), Players (F5), Map settings and
  Publish (F14). The object menu's last row is **AI and objectives**,
  the ground labels (`sheet::objects_with_ai`).
- **Controls.** A row a control of the bindings table, its pad button
  as the value and its keys in the description; up and down, and undo
  and redo, share a row (`bindings::PAIRS`), so the page fits the
  in-game list's sixteen lines.
- **Coordinates.** Six rows, X to Roll. A opens `Effect::Keyboard` with
  the value; left and right nudge.

### Markers in the world

- The held, placed or aimed object carries `forge.nav.target`, the game's
  arrow and marker, with the distance in whole metres as its number.
  In the Links tool the target is the aimed end, and a link's first end
  carries `forge.nav.from` while it is started.
- Each other seat's monitor is a navpoint (`forge.nav.monitor.1` and
  `.2`, the nearest two, nearest first). The game draws a navpoint in one
  colour, so a seat is told apart by where it is, not by its colour. The
  game's list has four slots, which the four navpoints fill. A HUD that
  never shows with another (`Describe::never_with`) starts its navpoints
  at the first slot, so the monitor's four do not wait behind a biped's
  (`hud_kit::slot_bases`).
- **The axis gizmo** (decision 2) is three HUD kit world marks of
  `Shape::Segment`: a line between two world points (`Pose::at` and
  `Pose::to`), drawn by the window's world-line pass as every world mark
  is, in the three axis colours (`Style::Axis`, `world::AXIS_COLOURS`).
  `forge_glue/gizmo.rs` (`Axes::from_frame`) gives the ends each tick, in
  the world's or the object's basis as the Axes setting says, an arm a
  seventh of the object's distance from the eye, between 0.05 and 0.5
  world units. It hides with the HUD and while a Forge screen is up. No
  bounding sphere's outline is drawn: retail has none.
- `forge_hud::Points` holds the world points each tick; the editor
  leaves out the markers, the rings (other monitors) and the gizmo that
  `forge_hud::Show` turns off; the session turns them on all (the
  Overlays setting of [forge-editing.md](forge-editing.md) sets them).

## The Firefight HUD

A HUD kit `Hud` named `firefight` (`hud_kit_core::huds::firefight`), fed
by `hud::view` from the mode blob as it is ([firefight.md](firefight.md),
"The HUD"), with the kit's elements, places and checks.

| Element | Kind | Place | Shows |
|---|---|---|---|
| `ff.banner` | Banner (slice K2) | the message routine's centred line | GET READY, WAVE 3, BOSS WAVE, ROUND COMPLETE, VICTORY, GAME OVER; timed, then gone |
| `ff.lives.pill`, `.end`, `ff.lives.count` | Icon and LCD Text | the grenade row's next place | lives; `Flash(Pulse::TimeWarning)` at 1 and 0 |
| `ff.enemies.pill`, `.end`, `ff.enemies.count` | Icon and LCD Text | the place after | enemies left while a wave is on |
| `ff.note` | Message | the notification list | "Round 2 of 3", "Mythic skull on", a life lost (`huds::firefight::notes`; the skulls ride in the mode blob) |

- The round and wave are not permanent lines. They are in the banner
  when they change and in the notification list.
- The score is the game's own: kills credit Slayer's score
  ([firefight.md](firefight.md), "Guest boundary"), which the retail
  score display and scoreboard show.
- Split screen: the pills keep the grenade row's half and quarter
  offsets; the banner and the notes are the game's in every mode.
- The enemies pill is a kit pill's width and the retail panel's 3-pixel
  gap right of the lives pill (`huds::firefight::PILL_STEP`, 30); a
  count shows 99 at most.
- A pill has no icon: the fallback, never new art. A pill's icon is
  retail art by widget name, chosen from the disc census (**S**, not
  done).

## Slices

Sizes as [forge-design.md](forge-design.md), "Features": **S** under 800
lines with tests, **M** under 2,500, **L** more. Lanes write code and
tests on synthetic data. K slices extend the HUD kit and sit before
their first user. The editing slices P1 to P6 are in
[forge-design.md](forge-design.md), "Slices".

| # | Slice | Size | Needs | Proofs |
|---|---|---|---|---|
| **U2** (built) | Lobby modes: `lobby_start::Mode` (picked in the party lobby's GAME MODE picker, [party-lobby.md](party-lobby.md)), Game Setup rows by mode, Map Variant picker, the Firefight map category, variant panel lines, the monitor only in Forge mode, the remote-member refusal; delete the kit lobbies | M | nothing | `Page` tables for Game Setup in each mode (`mode_lists_only_enabled_features`, `forge_mode_lists_change_map_and_forge_map`, `leaving_the_lobby_clears_the_mode`, `forge_with_a_remote_member_is_refused`, `the_monitor_needs_forge_mode`); `h2 ui preview kit.picker.map-variant`; `lobby_options` tests for the panel lines; content checks (no colon values, no repeated spaces) |
| **U1** (built) | Main menu: six rows. Remove the FORGE and FIREFIGHT entries, place Theater after System Link, re-pitch if needed | S | U2, landed together | table tests `the_main_menu_has_six_rows_with_theater`, `five_rows_without_theater`, `pitch_keeps_six_rows_above_the_footer`; the `main-menu` opener's preview with six rows; the layout checks |
| **U4** (built) | Forge screens in the game: the bindings table and Halo 3 pad gate, object menu, Properties from a table, Tools picker, Tool settings (Rotation snap), Link, questions, GAME MENU rows, Forge options, Controls; input handover | L | nothing | `h2 ui preview forge.objects`, `forge.properties`, `forge.tools`, `forge.settings`, `forge.link`, `forge.question`, `forge.options`, `forge.controls --state all --animation all`; `Page` tables carried over from `ui_kit/forge/tests.rs` plus `x_opens_objects_or_properties_by_aim`, `no_is_focused_on_every_question`, `every_action_has_one_binding`, `the_controls_page_lists_the_bindings_table`, `game_menu_adds_five_rows`; a gate table test that RB and LB rise and fall, RT held manipulates, hold Y deletes and B cancels; `list_class` runner test that a picker opens with its pick in a game; layout and content checks in every state; a disc test that each base resolves in `shared` |
| **K1** (built) | HUD kit: monitor mode (retail widgets gated off per user but the motion tracker, grenade origins without the row) | S | the HUD kit on `master` | `hud_kit_tests`: a retail widget of a monitor user is not drawn, the motion tracker is, and another user's widgets are; `h2 hud preview forge --state all --mode all --synthetic` |
| **U3** (built) | Forge HUD on the kit: `forge` HUD, feed from `forge_glue`, copy table with buttons from the bindings table. Not in it: the reticle (`forge.reticle`), the retail widgets' gate (K1), the gizmo and markers (U6) | M | U4's bindings | `h2 hud preview forge --state all`; the kit's checks (fit, glyphs, frame, retail, own, one layout, prompt, drawn) in every state and mode; `every_tool_state_has_one_prompt`, `the_prompts_read_as_the_game_does`, `the_budget_and_the_rebuilds_follow_the_view`, `each_hint_fits_the_training_box`, `the_pills_are_the_grenade_rows_first_places` (kit); `the_buttons_are_read_from_the_table`, `every_button_has_a_glyph_and_every_row_a_cue`, `a_notice_is_sent_once_each`, `a_hint_is_shown_once_for_its_time` (`forge_hud_tests.rs`); `each_yaml_is_the_hud_its_rust_was` |
| **K2** (built) | HUD kit: the `Banner` channel (the timed centred line) | S | the HUD kit on `master` | `FlatMemory` test of the text, start and duration fields (`the_banner_is_written_as_the_game_writes_its_timed_line`, `the_banner_is_written_on_a_change_of_text`); the prompt check extended to it; preview draws it at the routine's place (`the_banner_is_drawn_centred_above_the_frames_bottom`) |
| **U5** (built) | Firefight HUD on the kit | M | K2; kit `Message` | `h2 hud preview firefight --state all --mode all` (`the_firefight_hud_draws_and_passes_on_the_synthetic_sheet`); `the_hud_reads_only_the_mode_blob` unchanged; `every_stage_has_a_banner_or_none` table; the kit's checks replace `hud::check`; `the_feed_publishes_the_view_and_sends_the_notes_a_change_makes` |
| **K3** (built) | HUD kit: several navpoints for one HUD, the number drawn | S | the kit's navpoint number | `FlatMemory` test of n slots kept and released; preview with three points |
| **K4** (built) | HUD kit: `Shape::Segment`, a line between two world points at any slope, and `Style::Axis(n)`, one of three axis colours | S | the HUD kit on `master` | `a_segment_runs_between_its_two_points_unlifted` (`world.rs`); `a_gizmo_axis_is_a_segment_in_its_colour` (`hud_kit_world.rs`); `the_preview_draws_a_gizmo_segment_in_its_axis_colour` (the colour per axis, the `world` check on a fourth axis and on an axis off a world mark) |
| **U6** (built) | Forge markers as navpoints; the gizmo as three segments in the Axes setting's basis; remove the marker and gizmo painters | S | K3, K4, U3 (built) | table test: held, aimed, linking and two monitors give the expected points and segments; `h2 hud preview forge --state holding` |
| **U7** (built) | Remove the compositor path: `ScreenSpec::hud` and the `hud` screen kind, the Forge painter in `play_cmd/forge.rs` (the window keeps only the input hand-over), `forge.hud`, `forge.palette`, `forge.menu`, the reticle page and their sheet entries. `Art::Solid` stays: Theater's playback page draws its bar with it; update [ui-builder.md](ui-builder.md) "Screen kinds" | S | U3 (built), U4, U6 | `cargo check -p cli --no-default-features --features host`; the sheet has no `forge.hud`; `docs-check` |

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
  are unchanged.

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
- The main menu has six rows at most and never scrolls. GAME MENU never
  scrolls: a Forge row beyond its five goes on Forge options.
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

- That the game draws its HUD for a user whose view is the monitor's
  private world view (`spectator_view::draw_world_view`). If it does
  not, K1 also calls the HUD draw for that view.
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
  draws over the world, and that Forge options' rows push their page
  above it on the same window and slot.
- That `ui_watch` reads no screen over a running game with nothing up,
  so the monitor takes its input (`forge_screens::ui_up`).
- Which dialog bases besides `error_dialog_large` are in `shared`.
- That five rows added to GAME MENU fit its list without scrolling, and
  that the GAME MENU rows' array is the one named `0x4593a8`
  (`game_menu::ARRAY_NAME`).
- That the axis colours (`world::AXIS_COLOURS`, red, green and blue) read
  against the level; they are ours, not the HUD's colour table.
- Everything the HUD kit's page lists as not verified, which U3, U5 and
  U6 inherit.

## Related

- [forge-map-variants-ui.md](forge-map-variants-ui.md): the readout, world marks, Properties rows by kind and the map variant screens.
- [party-lobby.md](party-lobby.md): the lobby's modes and the GAME MODE picker.
- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md): the UI kit.
- [forge-editing.md](forge-editing.md): what the Forge screens do.
- [forge-design.md](forge-design.md) "The UI", [forge.md](forge.md): Forge.
- [firefight.md](firefight.md) "The HUD", "Game Setup", "The lobby": Firefight.
- [settings-menu.md](settings-menu.md) "Pause menu", [map-select.md](map-select.md).
- [hud-editor.md](../editor/hud-editor.md), [native-post-hud.md](../graphics/native-post-hud.md), [button-icons.md](../input/button-icons.md).
- The HUD kit's page `docs/game/hud-kit.md` and the monitor look's page `docs/game/forge-monitor-appearance.md`, on the branches `sb4-integ` and `forge-look`.
