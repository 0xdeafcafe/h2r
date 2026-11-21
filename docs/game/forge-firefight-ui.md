# Forge and Firefight UI: every screen and HUD piece as Halo 2 draws it

Status: Partly built. The Forge screens in the game and the bindings table (slice U4) are built; the rest is design, and the audit describes the compositor code that U3 and U7 replace.
Code: `crates/extensions/game/src/forge_screens.rs` and `forge_screens/` (the Forge screens and GAME MENU rows), `forge_controls/bindings.rs` (the bindings table), `crates/extensions/game/src/ui_kit/forge.rs` and `ui_kit/forge/` (the compositor's Forge strip and dialogs), `forge_lobby.rs`, `firefight/{hud,lobby,options}.rs`, `lobby_start.rs`, `forge_glue/gizmo.rs`, `theater_ui.rs` (`menu`, the main menu's rows), `crates/apps/cli/src/play_cmd/forge.rs` (the compositor painter). Tests: named per slice below.
Switch: none of its own. `forge` and `firefight`, listed in [switches.md](../runtime/switches.md).

Forge and Firefight are modes of the game's lobby, with a HUD each and,
for Forge, screens and markers over the world. Theater is the one row
added to the main menu. This page audits what
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
| Main menu rows THEATER, FORGE, FIREFIGHT | the retail main menu list, grown from 5 rows to 8 with one added item widget, so it shows 6 and scrolls | `theater_ui.rs:115` (`Rows::count`), `layout_glue.rs:1151` |
| FORGE lobby | a kit `List` screen with the pregame lobby's two backings behind three rows and a status line | `forge_lobby.rs:213`, `lobby_start.rs:81` |
| FIREFIGHT lobby | the same layout, other rows | `firefight/lobby.rs:168` |
| Firefight Options | a kit dialog on Quick Options over Game Setup, drawn by the game | `firefight/options.rs:209` |
| Forge HUD strip | the play window's compositor: the preview renderer paints a `ScreenSpec::hud` page into a 640 by 480 RGBA image with the disc's fonts, blended over the frame | `ui_kit/forge.rs:45`, `crates/apps/cli/src/play_cmd/forge.rs:126` |
| Forge dialogs (palette, properties, link, menu, load) | the same compositor path; the spec is a kit dialog spec, but no game widget exists | `ui_kit/forge/dialogs.rs:232`, `forge_glue.rs:55` |
| Gizmo | pixels the compositor writes into that image | `forge_glue/gizmo.rs:75` |
| Monitor marker | the same, a ring in the seat's colour | `forge_glue/marker.rs` on the branch `forge-look` |
| Firefight HUD | nothing. The view is computed and checked; no code reads `hud::current` | `firefight/hud.rs:26`, `firefight/hud.rs:347` |

### Why each reads as fake

**The main menu.**

- Eight rows in a list that shows six: System Link and Settings are below
  the fold, and a retail main menu never scrolls (`theater_ui.rs:116`).
- Our labels are capitals (`theater_ui.rs:39` to `:43`) beside the kit's
  own record of the retail rows as "Campaign", "Xbox Live"
  (`ui_kit/view.rs:96`). **S**: if the menu's font has lower case, ours
  are the only capitals in the list. The disc preview of
  `Opener::MAIN_MENU` shows which.

**The FORGE and FIREFIGHT lobbies.**

- Wrong widget. The pregame lobby has two buttons on `name_field` and
  everything under them is information
  (`mods_screen/page_layout.rs:1`). Ours stacks three list rows from
  the buttons' anchor down over `lobby_display`, where the lobby has its
  status text (`lobby_start.rs:81`: `visible: 3` on a layout made for one
  row, `mods_screen/page_layout.rs:39`).
- Missing parts. No player table, no map picture, no settings summary:
  the panel carries the two backings and no text
  (`lobby_start.rs:91`). What is left is an empty lobby frame.
- Wrong text form. Rows are `ValueStyle::Inline`, which joins label and
  value with a colon, "Map: Ascension" (`ui_kit/spec.rs:172`). No retail
  list writes a value that way.
- Debug-looking text. The status line reads "Ascension: a new map"
  (`forge_lobby.rs:205`) and "Start: Firefight on Lockout"
  (`firefight/lobby.rs:162`), and is cut to 46 characters with "..."
  (`lobby_start.rs:99`).
- Not previewed. Neither lobby is in `ui_kit::screens::NAMES`
  (`ui_kit/screens.rs:279`), so no sheet draws them and no layout check
  runs on them.

**Firefight Options.** The nearest to retail: Quick Options' panel, skin,
help line and picture, held to the tag by
`our_settings_dialogs_are_quick_options_inside`. Two gaps: its legend
advertises D-pad left and right as "Change" (`firefight/options.rs:219`),
and from the FIREFIGHT lobby it opens over our lobby while its spec's
opener is Game Setup (`firefight/options.rs:213`).

**The Forge HUD strip.**

- Not the HUD. It is a host overlay: it ignores the HUD's safe frame and
  fit, stays up when the game hides its HUD, is absent from films and
  captures, and exists only in the desktop play window.
- Invented panels. Three flat navy boxes, `rgba [10, 19, 29, 214]`,
  across the full width at the top, the bottom and for a notice
  (`ui_kit/forge.rs:52`, `:66`, `:83`, `:159`). `Art::Solid` has no
  other user (`ui_kit/view.rs:40`). Halo 2's HUD has no opaque bars.
- Menu font and one invented colour. Every text is menu font 1 in
  `[235, 243, 252]` (`ui_kit/forge.rs:174`, `:176`). The HUD's fonts are
  the HUD font and the LCD count font, in the HUD colour.
- A text reticle: the character "+" in font 1 (`ui_kit/forge.rs:65`).
- A menu legend on a HUD. Up to six button prompts in one bar along the
  bottom (`ui_kit/forge.rs:67`, `:94`). Halo 2's HUD shows one
  contextual prompt at a time, top left.
- Debug-looking text: "Budget 12 / 80" (`ui_kit/forge.rs:63`),
  "OBJECTS   REBUILD 2" with three spaces as a separator
  (`ui_kit/forge.rs:49`).
- No motion: the image is replaced whole when the spec changes
  (`crates/apps/cli/src/play_cmd/forge.rs:121`).

**The Forge dialogs.**

- Not game screens. They never enter the game's UI: no retail open and
  close animation, no focus sounds, no key repeat of the game's, and
  rows are clicked through a hit test of ours
  (`crates/apps/cli/src/play_cmd/forge.rs:44`).
- An invented backing. A 514 by 204 navy box is pushed under the retail
  panel so rows stay legible (`ui_kit/forge/dialogs.rs:372`). Retail's
  panel is an edge glow over the world.
- A hybrid no retail screen has. Properties and Link put the settings
  list skin, moved to a new anchor, inside `error_dialog_large`'s panel
  (`ui_kit/forge/dialogs.rs:275`, `:293`).
- Information as rows. Link's From, To and Channel are unselectable list
  rows (`ui_kit/forge/dialogs.rs:288`); retail keeps facts in the
  screen's texts and rows for what A acts on.
- Three rows for a pause menu: Save, Save as, Load
  (`ui_kit/forge/dialogs.rs:306`), not the game's own GAME MENU.
- The opener in every spec is the main menu
  (`ui_kit/forge/dialogs.rs:409`, `:438`), though they open in a game.

**The gizmo and the marker.** Red, green and blue two-pixel lines
(`forge_glue/gizmo.rs:87`) and a flat ring: a modelling tool's idiom,
aliased, drawn behind the strip's pixels and in front of everything in
the world.

**The Firefight HUD.** Not drawn, so in a match the mode shows nothing.
The checked layout would not read as Halo 2 either: five lines of
"LABEL NUMBER" text ("ROUND 1 OF 3", "ENEMIES 12", "LIVES 5") stacked at
the top right at invented offsets (`firefight/hud.rs:395`), an anchor the
HUD does not have (`Anchor::TopCenter`, `firefight/hud.rs:362`), a
guessed glyph width (`firefight/hud.rs:568`) and a private copy of the
kit's types (`firefight/hud.rs:354` to `:384`). Halo 2's HUD shows counts
as an icon with an LCD number, never as a labelled line.

## Targets

Each piece copies one retail screen or HUD element, by tag.

| Piece | Copies | Built through |
|---|---|---|
| Main menu | `main_menu` (screen 6): six rows, no scroll | the retail list with one added row, Theater ("The main menu") |
| Forge and Firefight lobbies | `pregame_lobby` (screen 14) itself, with `squad_settings_dialog` (screen 23) as its Game Setup | the retail screens in a mode chosen on Game Setup's Mode row ("The lobbies") |
| Firefight Options, Forge map, Mode | `quick_options` (screen 222) and `low_level_settings` (screen 223) | `DialogBase::QuickOptions`, the value picker; built |
| Forge menu | the multiplayer pause screen, GAME MENU ([settings-menu.md](settings-menu.md), "Pause menu") | five rows added to the retail list, as `pause_menu.rs` adds Video and Voice |
| Object menu, Tools, pickers in a game | `low_level_settings`' list on `error_dialog_large` (screen 240) | `kit.picker.game` (`picker::offer_list`) |
| Properties, Tool settings, Forge options, Controls, Coordinates | the in-game Controller Settings list (screen 200) | a kit `List` screen on that host, value rows opening `kit.picker.game` |
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
| Axis gizmo | none: the owner's exception (decision 2) | HUD kit `Segment` in the HUD's colours (slice K4) |

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
  Link and an Xbox Live custom game open ("The lobbies"). This is how
  Halo 3 and Reach have it: Forge is a lobby with a Network choice, and
  Theater is a lobby of its own (**C**,
  [notes/forge-prior-art.md](notes/forge-prior-art.md)).
- Six rows is what the retail pane's item widgets plus the one added
  widget hold (`layout_glue.rs:1151`), so nothing scrolls.
  `theater_ui::menu` keeps its text and press hooks (`ROW_TEXT`,
  `ROW_CHOSEN`) with the one entry, placed after System Link;
  `Rows::count` returns 6 with Theater on and 5 with it off.
- If six rows at the skin's pitch pass -211, the rows are re-pitched as
  the Xbox Live menu's are: the list layout hook (`custom_games::repitch`
  generalised over the handler's vtable, `theater_ui::menu::VTABLE`) and
  a `pitch(rows)` with the room above.
- The label takes the case the retail rows have (the disc preview
  decides: see the audit).

## The lobbies

Forge and Firefight are modes of the retail pregame lobby
(`pregame_lobby`, screen 14), not screens shaped like it. The roster,
the map picture, Start Game, Game Setup and the variant panel are the
game's.

**The Mode row.** Game Setup (`squad_settings_dialog`) has a first row
of ours, **Mode**, with its value in the skin's value column. A opens
the value picker:

| Mode | Shown when | The session |
|---|---|---|
| Multiplayer | always; the mode a lobby opens in | retail's |
| Forge | the `forge` feature is on | a Forge session: D-pad Up enters the monitor ([forge-editing.md](forge-editing.md), "Controls") |
| Firefight | the `firefight` feature is on and a settings store holds its choices | Firefight on a kit map ([firefight.md](firefight.md)) |

- The row is in Game Setup in every lobby that has it: Split Screen,
  a System Link game being hosted, and an Xbox Live custom game. Only
  the party leader can open Game Setup, so the mode is the leader's, as
  the map is.
- The mode is `lobby_start::Mode`, held per lobby. Leaving the lobby
  clears it. With both features off the row is absent and the lobby is
  retail's.
- The monitor is offered only in a session whose mode is Forge. In a
  Multiplayer or Firefight session D-pad Up is the game's own. A game
  started with no lobby (the level boot,
  [level-boot.md](../runtime/level-boot.md)) is a Forge session when it
  is local multiplayer, so an agent can drive one.

**Game Setup's rows by mode**, each added or hidden the way the Clanker
and Firefight rows are (`firefight/options.rs`, `ROW_ACTION`):

| Mode | Rows after Mode |
|---|---|
| Multiplayer | retail's, with ours as they are |
| Forge | Change Map..., Forge Map..., the retail game type row; Clanker Options... and Firefight Options... are hidden |
| Firefight | Change Map..., Firefight Options..., Clanker Options...; the game type row is hidden |

- **Forge Map...** opens the value picker with New and the saved maps
  (`forge::saved_names`); its help line is the map's description.
- **Change Map...** opens the map select on the mode's category: the
  original maps for Forge, a **Firefight** category of kit maps for
  Firefight ([map-select.md](map-select.md), `lobby_start::kit_maps`).
- **The game type row** in Forge mode picks the engine the session
  runs, and so which goals the object menu offers
  ([forge-editing.md](forge-editing.md), "The object menu"). It is
  Halo 3's Change game type (**C**). It opens on Slayer.
- **The variant panel** shows the mode's choices in the lobby's own five
  option lines through `lobby_options.rs`: "Forge map" and its value for
  Forge; wave set, difficulty and starting lives for Firefight.
- **The header** reads FORGE or FIREFIGHT in those modes, set on the
  lobby's header widget after its pane is built, as kit classes set
  theirs. In Multiplayer it is retail's.
- **Start Game** is retail's. Forge takes the chosen draft through
  `forge_glue::request_draft` when the game starts; Firefight holds the
  `firefight` setting on for the game. A refusal is the game's own
  dialog (`error_dialog_ok`), with our sentence.
- **With players on other machines.** Firefight in a hosted lobby is the
  custom game it is with the setting on ([firefight.md](firefight.md),
  "Live"). Forge with a remote member needs the session log on the
  network (slice F5); without it Start Game is refused with "Forge with
  players on other consoles is not available." A second local player
  joins as in Split Screen and plays as a player; a monitor for each
  local seat comes with F5's holds.
- `forge_lobby.rs`, `firefight/lobby.rs` and the lobby half of
  `lobby_start.rs` are deleted; their table tests move to the Game Setup
  rows and the mode.

## The Forge in-session UI

The behaviour (what each button does, the tools, the settings, the
properties by kind) is [forge-editing.md](forge-editing.md). This
section is how each piece is drawn.

### The monitor HUD

A HUD kit `Hud` named `forge`, registered by `forge_glue` and published
when the tool's state changes. One layout:

| Element | Kind | Place | Shows |
|---|---|---|---|
| `forge.reticle` | Icon, `Art::Widget` of a retail weapon reticle | centre | always in the monitor |
| `forge.prompt` | Prompt | the prompt line | the one action A does: "Press (A) to pick up Beacon", "Press (A) to place Beacon", "Press (X) for the object menu"; while manipulating, the value that is changing ("Heading 45") |
| `forge.notice` | Message | the notification list | a refusal ("The object budget is full."), Saved, Loaded, a setting changed by a button ("Fine movement on"); it fades as a pickup line does |
| `forge.teach` | Training | the training box | the first time a state is reached: hold Y to delete, hold RT to turn, Start for the menu |
| `forge.budget.pill`, `forge.budget.pill.end` | Icon, `Art::GrenadePill` | the grenade row's first place | always |
| `forge.budget.count` | Text, `Font::Lcd`, longest "9999" | on the pill | objects left to place |
| `forge.budget.meter` | Meter, `Art::StripMeter` | under the pill (`rects::below`) | used over limit; `Flash(Pulse::TimeWarning)` from 90% |
| `forge.tool` | Text, `Font::Hud`, longest "VOLUMES" | top right, where a weapon's name line sits | the tool; capitals |
| `forge.rebuild.pill`, `.count` | Icon and LCD Text | the grenade row's second place | pending Rebuild edits; hidden at 0 |
| `forge.gizmo.x`, `.y`, `.z` | Segment (slice K4) | from the held or aimed object's origin along each axis, projected | while an object is held or aimed at |

- **The retail widgets are off in the monitor, but the motion tracker.**
  A monitor has no shield, weapon or grenades. The kit's gate hook
  answers for the retail widgets of a user whose HUD is in monitor mode
  (`Handle::retail(false)`, slice K1), except the motion tracker, which
  stays drawn (decision 3). The grenade row's origins
  (`Origin::NextGrenadePill`) then resolve to the row's first place.
- **Copy** is the game's form: sentence case, the button as the font's
  glyph, one line. The count is digits only, as the LCD font has no
  slash. A prompt's button is read from the bindings table
  (`forge_controls::bindings::BINDINGS`), never written in the string.
- **No legend bar.** The full control list is the **Controls** page.
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
| Link | X in the Links tool | a large dialog with a body and two rows | U4 |
| Questions | delete with links, overwrite, leave unsaved | the same dialog with Yes and No | U4 |
| Forge menu | Start | the game's GAME MENU with rows added | U4 |
| Forge options | a row of the Forge menu | a list page on the Controller Settings host | U4 |
| Controls | a row of Forge options | a list page made from the bindings table | U4 |
| Coordinates | a row of Properties | a list page on the same host; A opens the keyboard | P1 |
| Placed objects, Map limits | rows of Forge options | `kit.picker.game`; a list page of facts | P4 |

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
  row draws arrows. The rows are `properties(kind)`'s, so a later slice
  adds rows to the table and no screen.
- **Tool settings.** Rows Movement snap, Rotation snap, Axes, Fine
  movement, each a picker. Until P1 it has the one row that is built,
  Rotation snap (`Tool::set_rotation_step`, `sheet::SETTINGS`). Fine
  movement is the stick click's toggle until then.
- **Link.** The body lists the two ends and what the link writes, with
  rows **Remove link** and **Done**: retail's two-button question.
- **Questions.** **Yes** and **No**; No has the focus.
- **Forge menu.** GAME MENU with five rows added above the retail rows:
  **Save**, **Save as**, **Load**, **Rebuild map** (with the pending
  count as its value) and **Forge options...**
  (`forge_screens::game_menu::ROWS`). The list's constructor makes its
  rows, then `put_first` adds ours and moves the action words so ours
  come first; the rows' array is grown for them at `LIST_DATA_NEW`
  (`forge_screens::game_menu_count`). Load opens the picker over the
  saved maps; Save as the keyboard, then the overwrite question when
  the name is saved. Rebuild map is refused by name, as the session
  cannot rebuild yet. Leave Game is retail's, behind the unsaved
  question. These are the rows Halo 3's Start has (**C**); everything
  else is one screen down so that GAME MENU never scrolls.
- **Forge options.** Tool settings... and Controls
  (`sheet::OPTIONS`), each opening above it so B comes back; Monitor
  look... joins when it is built, and the rows later slices bring:
  Placed objects, Map limits (P4), Generate nav (F9), Players (F5), Map
  settings and Publish (F14).
- **Controls.** A row a control of the bindings table, its pad button
  as the value and its keys in the description; up and down, and undo
  and redo, share a row (`bindings::PAIRS`), so the page fits the
  in-game list's sixteen lines.
- **Coordinates.** Six rows, X to Roll. A opens `Effect::Keyboard` with
  the value; left and right nudge.

### Markers in the world

- The held or aimed object carries the kit's navpoint: the game's arrow
  and marker, with the distance as its number.
- Each other monitor is a navpoint in its seat's colour, and a link's two
  ends are navpoints while the Links tool is up (slice K3 gives a HUD
  several points).
- **The axis gizmo** (decision 2) is three HUD kit `Segment` elements:
  a line between two screen points, drawn by the game's HUD with the
  HUD's own line shader and three colours of the HUD's colour table.
  `forge_glue/gizmo.rs` (`Axes::from_frame`) projects the ends each
  frame, in the world's or the object's basis as the Axes setting says.
  It hides with the HUD and is in films and captures, as every kit
  element is. Until K4 the compositor draws it.

## The Firefight HUD

A HUD kit `Hud` named `firefight`, fed by `hud::view` from the mode blob
as it is ([firefight.md](firefight.md), "The HUD"). `hud::LAYOUT`, the
private `Element`, `Place` and `Anchor` types and `hud::check` are
replaced by the kit's.

| Element | Kind | Place | Shows |
|---|---|---|---|
| `ff.banner` | Banner (slice K2) | the message routine's centred line | GET READY, WAVE 3, BOSS WAVE, ROUND COMPLETE, VICTORY, GAME OVER; timed, then gone |
| `ff.lives.pill`, `.end`, `ff.lives.count` | Icon and LCD Text | the grenade row's next place | lives; `Flash(Pulse::TimeWarning)` at 1 and 0 |
| `ff.enemies.pill`, `.end`, `ff.enemies.count` | Icon and LCD Text | the place after | enemies left while a wave is on |
| `ff.note` | Message | the notification list | "Round 2 of 3", a skull switched on, a life lost |

- The round and wave are not permanent lines. They are in the banner
  when they change and in the notification list.
- The score is the game's own: kills credit Slayer's score
  ([firefight.md](firefight.md), "Guest boundary"), which the retail
  score display and scoreboard show.
- Split screen: the pills keep the grenade row's half and quarter
  offsets; the banner is the game's in every mode.
- A pill's icon is retail art by widget name, chosen from the disc
  census when the slice is built (**S**); a pill with no icon is the
  fallback, never new art.

## Slices

Sizes as [forge-design.md](forge-design.md), "Features": **S** under 800
lines with tests, **M** under 2,500, **L** more. Lanes write code and
tests on synthetic data. K slices extend the HUD kit and sit before
their first user. The editing slices P1 to P6 are in
[forge-design.md](forge-design.md), "Slices".

| # | Slice | Size | Needs | Proofs |
|---|---|---|---|---|
| **U2** | Lobby modes: `lobby_start::Mode`, the Mode row and its picker, Game Setup rows by mode, Forge Map picker, the Firefight map category, variant panel lines, header, the monitor only in Forge mode, the remote-member refusal; delete the kit lobbies | M | nothing | `Page` tables for Game Setup in each mode (`mode_lists_only_enabled_features`, `forge_mode_lists_change_map_and_forge_map`, `firefight_mode_holds_the_setting_on`, `leaving_the_lobby_clears_the_mode`, `forge_with_a_remote_member_is_refused`, `the_monitor_needs_forge_mode`); `h2 ui preview kit.picker` in states `mode` and `forge-map`; `lobby_options` tests for the panel lines; content checks (no colon values, no repeated spaces) |
| **U1** | Main menu: six rows. Remove the FORGE and FIREFIGHT entries, place Theater after System Link, re-pitch if needed | S | U2, landed together | table tests `the_main_menu_has_six_rows_with_theater`, `five_rows_without_theater`, `pitch_keeps_six_rows_above_the_footer`; the `main-menu` opener's preview with six rows; the layout checks |
| **U4** | Forge screens in the game: the bindings table and Halo 3 pad gate, object menu, Properties from a table, Tools picker, Tool settings (Rotation snap), Link, questions, GAME MENU rows, Forge options, Controls; input handover | L | nothing | `h2 ui preview forge.objects`, `forge.properties`, `forge.tools`, `forge.settings`, `forge.link`, `forge.question`, `forge.options`, `forge.controls --state all --animation all`; `Page` tables carried over from `ui_kit/forge/tests.rs` plus `x_opens_objects_or_properties_by_aim`, `no_is_focused_on_every_question`, `every_action_has_one_binding`, `the_controls_page_lists_the_bindings_table`, `game_menu_adds_five_rows`; a gate table test that RB and LB rise and fall, RT held manipulates, hold Y deletes and B cancels; `list_class` runner test that a picker opens with its pick in a game; layout and content checks in every state; a disc test that each base resolves in `shared` |
| **K1** | HUD kit: monitor mode (retail widgets gated off per user but the motion tracker, grenade origins without the row) | S | the HUD kit on `master` | `hud_kit_tests`: a retail widget of a monitor user is not drawn, the motion tracker is, and another user's widgets are; `h2 hud preview forge --state all --mode all --synthetic` |
| **U3** | Forge HUD on the kit: `forge` HUD, feed from `forge_glue`, copy table with buttons from the bindings table | M | K1, U4's bindings | `h2 hud preview forge --state all`; the kit's checks (fit, glyphs, frame, retail, own, one layout, prompt, drawn) in every state and mode; table test `every_tool_state_has_one_prompt` over the states of `ui_kit/forge/pages.rs` |
| **K2** | HUD kit: the `Banner` channel (the timed centred line) | S | the HUD kit on `master` | `FlatMemory` test of the text, start and duration fields; the prompt check extended to it; preview draws it at the routine's place |
| **U5** | Firefight HUD on the kit | M | K2; kit `Message` | `h2 hud preview firefight --state all --mode all`; `the_hud_reads_only_the_mode_blob` unchanged; `every_stage_has_a_banner_or_none` table; the kit's checks replace `hud::check` |
| **K3** | HUD kit: several navpoints for one HUD, the number drawn | S | the kit's navpoint number | `FlatMemory` test of n slots kept and released; preview with three points |
| **K4** | HUD kit: the `Segment` kind, a line between two screen points in a HUD colour | S | the HUD kit on `master` | `FlatMemory` test of the record's ends and colour; the frame check clips a segment to the safe frame; preview draws three |
| **U6** | Forge markers as navpoints; the gizmo as three segments in the Axes setting's basis; remove the marker and gizmo painters | S | K3, K4, U3 | table test: held, aimed, linking and two monitors give the expected points and segments; `h2 hud preview forge --state holding` |
| **U7** | Remove the compositor path: `ScreenSpec::hud`, `Art::Solid`, the Forge painter in `play_cmd/forge.rs`, `forge.hud` and its sheet entries; update [ui-builder.md](ui-builder.md) "Screen kinds" | S | U3, U4, U6 | `cargo check -p cli --no-default-features --features host`; the sheet has no `forge.hud`; `docs-check` |

U2 and U4 need neither kit change and can start at once. U1 lands with
U2, so that Forge and Firefight are never without a way in. U3 and U5
wait for the HUD kit to land from `sb4-integ`; K1, K2 and K4 can be
written against that branch.

## Interfaces

- **HUD kit**: `hud_kit::register`, `Handle::publish`,
  `Handle::navpoint`; added here `Handle::retail(bool)` (K1), the
  `Banner` kind (K2), `Handle::navpoints(&[Point])` (K3) and the
  `Segment` kind (K4).
- **UI kit**: `list_class::open` for screens in a game;
  `picker::offer_list`; `Effect::Keyboard`. No new screen base.
- **Guest**: no new entry. The lobby header widget is reached through
  `TEXT_SET`, Game Setup's rows through `ROW_ACTION`; the banner's
  fields are the message state's timed text (`+0x454`, `+0x4d8`,
  `+0x4dc`, **E** in the HUD kit's page).
- **Agent API**: no new method. `ui.screen` and `ui.press` see the
  Forge screens once they are game screens, which the compositor pages
  never were, and Game Setup's Mode row as any row; `hud.kit` (the HUD
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
  absent, never greyed.
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
   Forge. The main menu has six rows, with Theater as the one of ours.
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
  draw the `Segment` kind can use.

## Not verified

- That the game draws its HUD for a user whose view is the monitor's
  private world view (`spectator_view::draw_world_view`). If it does
  not, K1 also calls the HUD draw for that view.
- That a row added to Game Setup shows in every lobby that opens it
  (Split Screen, a hosted System Link game, an Xbox Live custom game),
  and that the lobby keeps a header set after its pane is built.
- That hiding the game type row leaves Game Setup's other rows working.
- What a client's lobby shows for a host in Forge or Firefight mode. The
  mode reaches other machines only as the session's own data does:
  Firefight's definition, and Forge's first log entry in F5.
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
- That the HUD has a line or thin quad draw the kit can compile a
  `Segment` to. If it has none, the gizmo is a row of the HUD's dot
  icons along each axis.
- Everything the HUD kit's page lists as not verified, which U3, U5 and
  U6 inherit.

## Related

- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md): the UI kit.
- [forge-editing.md](forge-editing.md): what the Forge screens do.
- [forge-design.md](forge-design.md) "The UI", [forge.md](forge.md): Forge.
- [firefight.md](firefight.md) "The HUD", "Game Setup", "The lobby": Firefight.
- [settings-menu.md](settings-menu.md) "Pause menu", [map-select.md](map-select.md).
- [hud-editor.md](../editor/hud-editor.md), [native-post-hud.md](../graphics/native-post-hud.md), [button-icons.md](../input/button-icons.md).
- The HUD kit's page `docs/game/hud-kit.md` and the monitor look's page `docs/game/forge-monitor-appearance.md`, on the branches `sb4-integ` and `forge-look`.
