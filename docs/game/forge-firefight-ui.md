# Forge and Firefight UI: every screen and HUD piece as Halo 2 draws it

Status: Design. Nothing here is built; the audit describes the code on `master`.
Code: `crates/extensions/game/src/ui_kit/forge.rs` and `ui_kit/forge/` (the Forge strip and dialogs), `forge_lobby.rs`, `firefight/{hud,lobby,options}.rs`, `lobby_start.rs`, `forge_glue/gizmo.rs`, `theater_ui.rs` (`menu`, the main menu's rows), `crates/apps/cli/src/play_cmd/forge.rs` (the compositor painter). Tests: named per slice below.
Switch: none of its own. `forge` and `firefight`, listed in [switches.md](../runtime/switches.md).

Forge and Firefight add a main menu row each, a lobby each, a HUD each
and, for Forge, dialogs and markers over the world. This page audits what
draws each one, names the retail screen or HUD element each must copy, and
orders the work. The one rule: **every pixel of Forge and Firefight UI
comes out of a retail draw routine reading retail tags.** Menus are kit
screens the game's own widgets draw ([ui-builder.md](ui-builder.md)); HUD
pieces are HUD kit elements the game's own HUD draws (the HUD kit, below).
The compositor draws no Forge or Firefight UI.

Evidence labels are those of [docs-style.md](../meta/docs-style.md), plus
**C**: public knowledge of retail Halo 3, as [forge.md](forge.md) uses it.

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
| Main menu | `main_menu` (screen 6): six rows, no scroll | the retail list; a sub-menu on a copy of the same tag ("The main menu") |
| FORGE and FIREFIGHT lobbies | `pregame_lobby` (screen 14) itself, with `squad_settings_dialog` (screen 23) as its Game Setup | the retail screens in a mode ("The lobbies") |
| Firefight Options, Forge map | `quick_options` (screen 222) and `low_level_settings` (screen 223) | `DialogBase::QuickOptions`, the value picker; built |
| Forge menu | the multiplayer pause screen, GAME MENU ([settings-menu.md](settings-menu.md), "Pause menu") | rows added to the retail list, as `pause_menu.rs` adds Video and Voice |
| Object menu, pickers in a game | `low_level_settings`' list on `error_dialog_large` (screen 240) | `kit.picker.game` (`picker::offer_list`) |
| Properties | the in-game Controller Settings list (screen 200) | a kit `List` screen on that host, value rows opening `kit.picker.game` |
| Link, delete, overwrite, leave | `error_dialog_ok_cancel`: body text and two rows | `DialogBase::Large` with a body |
| Save as | the game's keyboard | `Effect::Keyboard` |
| Contextual prompt | the pickup prompt line, top left | HUD kit `Prompt` |
| Refusals and confirmations | the pickup notification list | HUD kit `Message` |
| First-use teaching | the training box | HUD kit `Training` |
| Budget, lives, enemies left | a grenade pill with its LCD count | HUD kit `Icon` (`Art::GrenadePill`) and `Text` (`Font::Lcd`) |
| Budget fill | the energy sword's battery strip | HUD kit `Meter` (`Art::StripMeter`) |
| Reticle | a retail weapon's reticle widget | HUD kit `Icon` (`Art::Widget`) |
| Wave and round banner | the timed centred line of the message routine | a HUD kit channel, `Banner` (slice K2) |
| Held object, other monitors, link ends | the game's navpoint: arrow, waypoint marker, number | HUD kit `Navpoint` |

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

Eight rows do not fit at any pitch that looks retail, and the logo
leaves no room to move the list up. So the fix is not a re-pitch.

**The design: six rows and a sub-menu.**

```text
 main menu                         EXTRAS (a copy of main_menu's tag)
   Campaign                          Theater
   Xbox Live                         Forge
   Split Screen                      Firefight
   System Link
   Extras
   Settings
```

- The main menu has one row of ours, **Extras**, above Settings. Six
  rows is what the retail pane's item widgets plus the one added widget
  hold (`layout_glue.rs:1151`), so nothing scrolls. `theater_ui::menu`
  keeps its text and press hooks (`ROW_TEXT`, `ROW_CHOSEN`) with one
  entry; `Rows::count` returns 6 whichever features are on, and the row
  is absent when none is.
- If six rows at the skin's pitch pass -211, the rows are re-pitched as
  the Xbox Live menu's are: the list layout hook (`custom_games::repitch`
  generalised over the handler's vtable, `theater_ui::menu::VTABLE`) and
  a `pitch(rows)` with the room above.
- **Extras** opens a screen that is the main menu again: a tag copy of
  `main_menu` (root flags, pane 0 with the logo and track art, the list
  block with its skin, anchor and animation), with our rows. It is a new
  base beside the dialog bases, `ScreenBase::MainMenu`, written by the
  recipe `dialog_tag::write_base_tag` uses and served by the screen-tag
  hook (`list_class::served`). B returns to the main menu on the Extras
  row.
- Labels take the case the retail rows have (the disc preview decides:
  see the audit). Rows of features that are off are absent, never
  greyed.

**Screen tag work.**

1. `ScreenBase::MainMenu` in `ui_kit::dialog`: geometry from the disc
   (`Disc::dialog_geometry_for`'s pattern) and a disc-free fallback the
   disc test holds equal.
2. `write_base_tag` for a base with no header layout and a list that is
   not moved.
3. The class `main.extras` (`KitScreen`), its string family, and its
   place in `ui_kit::screens::NAMES`.
4. `theater_ui::menu`: one entry, the fixed count, and the optional
   re-pitch hook on `custom_games::LIST_LAYOUT`.
5. `Opener::MAIN_MENU.rows` follows the six rows.

**The alternative not taken**: a tag extension that grows the list block
to eight visible rows (`speaker_row::grow_visible` on a copy of the
`wgit` tag) and re-pitches to 20. It keeps everything one press away and
does not look retail.

## The lobbies

FORGE and FIREFIGHT open the retail pregame lobby, not a screen shaped
like it. The lobby is the one the Split Screen row opens: the roster,
the map picture, Start Game, Game Setup and the variant panel are the
game's.

- **Mode.** The Extras rows set a lobby mode (`lobby_start::Mode`:
  `Forge`, `Firefight`) and call the Split Screen row's own handler
  through the retail row path (`retail_rows::retail_handle`). Leaving the
  lobby clears the mode. With no mode the lobby is retail's.
- **Game Setup** (`squad_settings_dialog`) in a mode lists what the mode
  uses, each row added or hidden the way the Clanker and Firefight rows
  are (`firefight/options.rs`, `ROW_ACTION`):

  | Mode | Rows |
  |---|---|
  | Forge | Change Map..., Forge Map..., Clanker Options... is hidden |
  | Firefight | Change Map..., Firefight Options..., Clanker Options... |

  **Forge Map...** opens the value picker with New and the saved maps
  (`forge::saved_names`); its help line is the description the lobby
  page has. **Change Map...** opens the map select on the mode's
  category: the original maps for Forge, a **Firefight** category of kit
  maps for Firefight ([map-select.md](map-select.md),
  `lobby_start::kit_maps`).
- **The variant panel** shows the mode's choices in the lobby's own five
  option lines through `lobby_options.rs`: "Forge map" and its value for
  Forge; wave set, difficulty and starting lives for Firefight.
- **The header** reads FORGE or FIREFIGHT, set on the lobby's header
  widget after its pane is built, as kit classes set theirs.
- **Start Game** is retail's. Forge takes the chosen draft through
  `forge_glue::request_draft` when the game starts; Firefight in a mode
  holds the `firefight` setting on. A refusal is the game's own dialog
  (`error_dialog_ok`), with our sentence.
- `forge_lobby.rs`, `firefight/lobby.rs` and the lobby half of
  `lobby_start.rs` are deleted; their table tests move to the Game Setup
  rows and the mode.

A second local player can join the lobby as in Split Screen, so Forge
for two on one console needs no screen of its own.

## The Forge in-session UI

### Halo 3's model on Halo 2's widgets

| Halo 3 Forge (**C**) | Here |
|---|---|
| D-pad Up swaps player and monitor | the same |
| A picks up and drops | A; the prompt line says which |
| X opens the object menu; on an object, its options | X: the object menu with nothing aimed at, Properties with an object aimed at or held |
| Y deletes | hold Y |
| A held object floats in front of the monitor, moved and turned with the sticks | the real object, held at a distance along the aim; the triggers move it nearer and farther |
| A budget counts down as objects are placed | a pill and count in the HUD's grenade row |
| Button prompts on the HUD | one contextual line in the pickup prompt, and the training box once per action |

B cancels a hold and backs out of a screen. Tool switching stays on the
Black and White buttons ([forge-design.md](forge-design.md), "The UI").

### The monitor HUD

A HUD kit `Hud` named `forge`, registered by `forge_glue` and published
when the tool's state changes. One layout:

| Element | Kind | Place | Shows |
|---|---|---|---|
| `forge.reticle` | Icon, `Art::Widget` of a retail weapon reticle | centre | always in the monitor |
| `forge.prompt` | Prompt | the prompt line | the one action A does: "Press (A) to pick up Beacon", "Press (A) to place Beacon", "Press (X) for the object menu" |
| `forge.notice` | Message | the notification list | a refusal ("The object budget is full."), Saved, Loaded; it fades as a pickup line does |
| `forge.teach` | Training | the training box | the first time a state is reached: hold Y to delete, Black to turn, Start for the menu |
| `forge.budget.pill`, `forge.budget.pill.end` | Icon, `Art::GrenadePill` | the grenade row's first place | always |
| `forge.budget.count` | Text, `Font::Lcd`, longest "9999" | on the pill | objects left to place |
| `forge.budget.meter` | Meter, `Art::StripMeter` | under the pill (`rects::below`) | used over limit; `Flash(Pulse::TimeWarning)` from 90% |
| `forge.tool` | Text, `Font::Hud`, longest "VOLUMES" | top right, where a weapon's name line sits | the tool; capitals |
| `forge.rebuild.pill`, `.count` | Icon and LCD Text | the grenade row's second place | pending Rebuild edits; hidden at 0 |

- **The retail widgets are off in the monitor.** A monitor has no
  shield, weapon or grenades. The kit's gate hook answers for the retail
  widgets of a user whose HUD is in monitor mode
  (`Handle::retail(false)`, slice K1), so only the motion tracker's
  place is empty and ours remain. The grenade row's origins
  (`Origin::NextGrenadePill`) then resolve to the row's first place.
- **Copy** is the game's form: sentence case, the button as the font's
  glyph, one line. The count is digits only, as the LCD font has no
  slash.
- **No legend bar.** The full control list is a row in the Forge menu
  (**Controls**), a kit list page.
- The agent API's `forge.status` carries the same values; `hud.kit`
  reports what the gate drew.

### The object menu, properties and prompts

All are kit screens opened in the game with `list_class::open`, on the
dialogs' window, over the running world. While one is up the monitor's
camera and tool take no input (`forge_controls` reads the kit's open
state).

- **Object menu** (X). The value picker in a game, twice: a list of
  groups, then the group's objects, each `kit.picker.game` with its six
  rows, scroll arrows and description block. The description is the
  object's line: what it is, how many are placed, how many are left. A
  on an object closes both and holds a new one. The picker opens on the
  group and object last chosen.
- **Properties** (X on an object). A list page on the in-game Controller
  Settings host: rows Heading, Rotation step, and per kind Team, Channel
  or Respawn, each with its value in the skin's value column. A opens
  the picker. No row steps sideways and none draws arrows. The object's
  name is the header.
- **Link.** A large dialog whose body lists the two ends and what the
  link writes, with rows **Remove link** and **Done**: retail's
  two-button question.
- **Questions.** Delete with links attached, overwrite on Save as, and
  leaving with unsaved edits are the same dialog with **Yes** and
  **No**; No has the focus.
- **Forge menu** (Start). The game's GAME MENU with rows added above
  Leave Game: **Save**, **Save as**, **Load**, **Rebuild map** (with the
  pending count as its value), **Monitor look**, **Controls**. Load
  opens the picker; Save as the keyboard. Leave Game is retail's, behind
  the unsaved question.

### Markers in the world

- The held or aimed object carries the kit's navpoint: the game's arrow
  and marker, with the distance as its number.
- Each other monitor is a navpoint in its seat's colour, and a link's two
  ends are navpoints while the Links tool is up (slice K3 gives a HUD
  several points).
- The axis lines are removed. Heading is read on the prompt line while
  turning ("Heading 45").

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
tests on synthetic data. Ordered by what the owner sees most; K slices
extend the HUD kit and sit before their first user.

| # | Slice | Size | Needs | Proofs |
|---|---|---|---|---|
| **U1** | Main menu: six rows, Extras, `ScreenBase::MainMenu`, `main.extras` | M | nothing | `h2 ui preview main.extras --state all` beside the `main-menu` opener; table tests `the_main_menu_never_has_more_than_six_rows`, `extras_lists_only_enabled_features`, `pitch_keeps_six_rows_above_the_footer`; `the_extras_menu_is_the_main_menu_with_other_rows` (every non-row widget at the opener's rectangle, art and font); the layout checks in every state; the disc test holds the fallback geometry |
| **U2** | Lobbies: the mode, Game Setup rows, Forge Map picker, the Firefight map category, variant panel lines, header; delete the kit lobbies | M | U1 | `Page` tables for Game Setup in each mode (`forge_mode_lists_change_map_and_forge_map`, `firefight_mode_holds_the_setting_on`, `leaving_the_lobby_clears_the_mode`); `h2 ui preview kit.picker` in state `forge-map`; `lobby_options` tests for the panel lines; content checks (no colon values, no repeated spaces) |
| **K1** | HUD kit: monitor mode (retail widgets gated off per user, grenade origins without the row) | S | the HUD kit on `master` | `hud_kit_tests`: a retail widget of a monitor user is not drawn and another user's is; `h2 hud preview forge --state all --mode all --synthetic` |
| **U3** | Forge HUD on the kit: `forge` HUD, feed from `forge_glue`, copy table | M | K1 | `h2 hud preview forge --state all`; the kit's checks (fit, glyphs, frame, retail, own, one layout, prompt, drawn) in every state and mode; table test `every_tool_state_has_one_prompt` over the states of `ui_kit/forge/pages.rs` |
| **U4** | Forge screens in the game: object menu, Properties, Link, questions, GAME MENU rows, Controls page; input handover | L | nothing | `h2 ui preview forge.objects`, `forge.properties`, `forge.link`, `forge.question`, `forge.controls --state all --animation all`; `Page` tables carried over from `ui_kit/forge/tests.rs` plus `x_opens_objects_or_properties_by_aim`, `no_is_focused_on_every_question`; `list_class` runner test that a picker opens with its pick in a game; layout and content checks in every state; a disc test that each base resolves in `shared` |
| **K2** | HUD kit: the `Banner` channel (the timed centred line) | S | the HUD kit on `master` | `FlatMemory` test of the text, start and duration fields; the prompt check extended to it; preview draws it at the routine's place |
| **U5** | Firefight HUD on the kit | M | K2; kit `Message` | `h2 hud preview firefight --state all --mode all`; `the_hud_reads_only_the_mode_blob` unchanged; `every_stage_has_a_banner_or_none` table; the kit's checks replace `hud::check` |
| **K3** | HUD kit: several navpoints for one HUD, the number drawn | S | the kit's navpoint number | `FlatMemory` test of n slots kept and released; preview with three points |
| **U6** | Forge markers as navpoints; remove the gizmo and marker painters | S | K3, U3 | table test: held, aimed, linking and two monitors give the expected points; `h2 hud preview forge --state holding` |
| **U7** | Remove the compositor path: `ScreenSpec::hud`, `Art::Solid`, the Forge painter in `play_cmd/forge.rs`, `forge.hud` and its sheet entries; update [ui-builder.md](ui-builder.md) "Screen kinds" and [forge-design.md](forge-design.md) "The UI" | S | U3, U4, U6 | `cargo check -p cli --no-default-features --features host`; the sheet has no `forge.hud`; `docs-check` |

U1, U2 and U4 need neither kit change and can start at once. U3 and U5
wait for the HUD kit to land from `sb4-integ`; K1 and K2 can be written
against that branch.

## Interfaces

- **HUD kit**: `hud_kit::register`, `Handle::publish`,
  `Handle::navpoint`; added here `Handle::retail(bool)` (K1), the
  `Banner` kind (K2) and `Handle::navpoints(&[Point])` (K3).
- **UI kit**: `ScreenBase::MainMenu`; `list_class::open` for screens in
  a game; `picker::offer_list`; `Effect::Keyboard`.
- **Guest**: no new entry. The Split Screen row's handler and the lobby
  header widget are reached through `retail_rows` and `TEXT_SET`; the
  banner's fields are the message state's timed text (`+0x454`, `+0x4d8`,
  `+0x4dc`, **E** in the HUD kit's page).
- **Agent API**: no new method. `ui.screen` and `ui.press` see the
  Forge screens once they are game screens, which the compositor pages
  never were; `hud.kit` (the HUD kit's build plan) reads the HUDs;
  `forge.status` and `firefight.state` are unchanged.

## Rules and limits

- No Forge or Firefight pixel is drawn by the compositor, and no colour,
  rectangle or font in their UI is a constant of ours: each is a tag
  field or a retail address read at run time.
- A choice opens a picker. No row draws arrows or steps through values
  as its only way to change.
- Copy: capitals in titles and HUD text widgets, sentence case in rows
  and prompts, single spaces, no colon between a label and its value, no
  "..." cut of a sentence. `ui_kit::content::check` holds the menus to
  it; the HUD kit's `prompt` and `fit` checks hold the HUD.
- One layout per screen and per HUD.
- A screen or element with no retail counterpart is not built: it is
  changed until it has one, or dropped (the axis lines).
- A map whose interface lacks a base (Quick Options in a game) uses the
  base that is there (`error_dialog_large`, the Controller Settings
  host), never art of ours.

## Evidence

- **Code** Every audit row: the path and line beside it.
- **V** The main menu's logo, list anchor and first row, the dialog
  tags' contents, Quick Options' and `low_level_settings`' parts, the
  safe area: [ui-builder.md](ui-builder.md).
- **E** The HUD's parts, the prompt, notification, training and navpoint
  routines and the timed centred line: the HUD kit's page on `sb4-integ`.
- **C** Halo 3 Forge's controls and HUD, from the retail game as
  publicly documented; [forge.md](forge.md) §1.1 has the sources for the
  model. The button for each action is checked against them when U3 is
  built.
- **S** The main menu skin's item height; that the menu's font has lower
  case; which retail reticle and pill icons are used; that the retail
  score display shows a Firefight score.

## Not verified

- That the game draws its HUD for a user whose view is the monitor's
  private world view (`spectator_view::draw_world_view`). If it does
  not, K1 also calls the HUD draw for that view.
- That the Split Screen row's handler can be called for a row on another
  screen, and that the lobby keeps a header set after its pane is built.
- That a kit screen on the dialogs' window opens over a running
  multiplayer game without pausing it, takes the pad from the player and
  gives it back on close.
- Which dialog bases besides `error_dialog_large` are in `shared`.
- That rows added to GAME MENU fit its list without scrolling.
- That a copy of `main_menu`'s tag draws its logo and plays its
  transition when it opens over the main menu.
- Everything the HUD kit's page lists as not verified, which U3, U5 and
  U6 inherit.

## Related

- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md): the UI kit.
- [forge-design.md](forge-design.md) "The UI", [forge.md](forge.md): Forge.
- [firefight.md](firefight.md) "The HUD", "Game Setup", "The lobby": Firefight.
- [settings-menu.md](settings-menu.md) "Pause menu", [map-select.md](map-select.md).
- [hud-editor.md](../editor/hud-editor.md), [native-post-hud.md](../graphics/native-post-hud.md), [button-icons.md](../input/button-icons.md).
- The HUD kit's page `docs/game/hud-kit.md` and the monitor look's page `docs/game/forge-monitor-appearance.md`, on the branches `sb4-integ` and `forge-look`.
