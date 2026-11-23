# Forge map variants: the editor's HUD, world marks and screens

Status: Design. The Forge screens in the game (slice U4), the HUD kit, its world marks and the superbounce readout this page copies are built; nothing named here as new is.
Code: `crates/extensions/game/src/forge_screens/` (the screens), `assets/ui/screens/forge.*.yaml` (their docs), `crates/rewrites/hud-kit-core/src/{world.rs, huds/}` (the HUD kit's model), `crates/extensions/game/src/hud_kit_world.rs` (the world-line source). Tests: named per slice below.
Switch: `forge`, listed in [switches.md](../runtime/switches.md). No switch of its own.

How a monitor sees and edits the things of
[forge-map-variants.md](forge-map-variants.md): a readout by the motion
tracker for the thing aimed at, marks on the level for what has no
body, a Properties page whose rows depend on the kind, the budget, and
the screens that save, load and rename a map variant. Every piece is
built with a kit: a screen is a YAML doc that becomes a tag in the
map, and a HUD piece is a HUD kit element the game's own HUD draws.
The rules that must hold are
[forge-firefight-ui.md](forge-firefight-ui.md)'s: **every pixel comes
out of a retail draw routine reading retail tags, a choice opens a
picker on top, and one screen or HUD has one layout.**

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### What a screen is made of

- **The doc.** Each screen is `assets/ui/screens/<name>.yaml`: kind,
  layout, backing, states, widgets and rows
  ([ui-builder.md](ui-builder.md), "Screens in YAML"). A name in
  `ui_kit::screens::NAMES` gets a `wgit` tag in the maps the game reads
  its screens from (`ui_kit::inject::plan`,
  [ui-in-maps.md](ui-in-maps.md)).
- **No Rust per screen.** A screen's rows, labels, help and values are
  in its doc. Rust holds what is shared: the binder that reads a
  property and makes its op (`forge::props`,
  [forge-map-variants.md](forge-map-variants.md), "Interfaces"), the
  list and dialog classes (`list_class`, `picker`), and the store. A
  new property is a row in a doc and a line in `forge::props`.
- **Bases.** A list page is on the in-game Controller Settings screen,
  a picker and a question on the large dialog, and in the lobby a
  picker is over Game Setup, as `kit.picker.forge-map` is
  ([forge-firefight-ui.md](forge-firefight-ui.md), "The screens"). No
  screen here needs a base those do not give.
- **The checks.** `ui_kit::content::check` (capitals in titles,
  sentence case in rows, single spaces, no colon between a label and
  its value), the layout checks, and
  `h2 ui preview <screen> --state all --animation all` for every doc
  below. `h2 ui inject` builds the tags and runs the tag checks.

### The Properties page

X on the thing aimed at or held opens `forge.properties`: the thing's
name as the header, then the rows its kind has. The rows are the doc's
(`assets/ui/screens/forge.properties.yaml`), in place of the
`sheet::PROPERTIES` table:

```yaml
rows:
  row.4:
    kind: value
    text: "Respawn time"
    help: "How long after it is taken before the item comes back."
    on: [spawner]
    prop: respawn_time
    open: picker
    values: [[0, "Map default"], [5, "5 seconds"], [10, "10 seconds"]]
```

`on` is the kinds the row is for, `prop` a name in `forge::props`,
`open` what A does (`picker`, `keyboard`, a screen's name, or `act`),
and `values` a picker's choices with the number written. A doc that
names a property `forge::props` lacks is a `DataError`. A property
whose field is not decoded is not in `forge::props`' answer for the
map, and its row is then absent, not greyed.

A value row shows its value in the skin's value column. No row has
arrows; left and right on a number row are the Coordinates page's
nudge and nothing else.

| Kind | Row | A opens | Writes | Slice |
|---|---|---|---|---|
| any | Coordinates... | the Coordinates page ([forge-editing.md](forge-editing.md), "Precision") | position, rotation | P1 |
| any with a rotation | Reset orientation | acts | rotation | P1 |
| any | Duplicate | acts; holds the copy | `Placement::Add` with the source's fields | P1; P2 for a kind that needs the kit |
| any | Delete | the question when it has links, else acts | `Entry::Remove` | M6 |
| own, changed | Restore original | the question | `Entry::Reset` | M6 |
| Object | Object... | a picker of its group's palette | the palette entry | M6 |
| Object | Game types... | the Game types dialog | the `games` property | G1 |
| Object | Label... | a picker of the labels the session's script declares, then a number | the label ([rules-engine.md](rules-engine.md#forge-labels)) | P5 |
| Item spawner | Item... | the object menu's item groups, as a picker | `item` | M6 |
| Item spawner | Respawn time | a picker: Map default, then seconds | `respawn time` | M6 |
| Item spawner | Return time | a picker of seconds | `abandon limit` | M6 |
| Item spawner | Place at start | a picker: Yes, No | `respawn mode` | D3 |
| Item spawner | Game types... | the Game types dialog | the game-type slots | D3 |
| Item spawner | Team | a picker of teams | the `team` property | D4 |
| Item spawner, a weapon | Spare clips | a picker of counts | the `spare_clips` property | D5 |
| Starting point | Team | a picker of teams | `team` | M6 |
| Starting point | Game types... | the Game types dialog | the game-type slots | D3 |
| Starting point | Weight | a picker: Low, Normal, High | the `weight` property | D5 |
| Goal | Role | a picker of the roles the session's game type reads | the flag's `type` | M6 |
| Goal | Team | a picker of teams | `team` | M6 |
| Goal | Order | a picker of numbers | the hill corner or the `identifier` | M6 |
| Teleporter | Role | a picker: Sender, Receiver | the flag's `type` | M6 |
| Teleporter | Channel | a picker of channels | a link ([forge-design.md](forge-design.md), "F6 in detail") | built for the map's own flags |

- **The header** is the thing's name: the tag's base name for an
  object and a spawner (`forge_map`'s palette names), and the role
  for a starting point, a goal or a teleporter ("Starting point",
  "Hill 2 corner 3").
- **Teams** in a picker are the game's own team names, read from the
  map's strings (**S**: which string list holds them), with the row
  the engine reads as no team.
- **A picker's values** are in the doc, so a designer changes the
  ladder of seconds without code. A value the map already has that is
  not in the ladder is listed first, as it is.
- **Game types dialog** (`forge.games`, a large dialog laid out as a
  list). **All game types** first, then a row an engine. A on a row
  ticks or unticks it; the tick is the row's value column. **Done**
  writes the set as one undo step. A set the entry cannot hold is
  refused when Done is pressed, with the document's sentence as the
  dialog's status line
  ([forge-map-variants.md](forge-map-variants.md), "Game types").
- The built page's Heading row goes when Coordinates lands; Heading is
  its Yaw.

### The readout

A HUD kit `Hud` named `forge-readout`
(`crates/rewrites/hud-kit-core/src/huds/forge_readout.rs` (new)),
beside the monitor's `forge` HUD
([forge-firefight-ui.md](forge-firefight-ui.md), "The monitor HUD").
It copies the superbounce readout's place and build: three rows of
font 6 right of the motion tracker, which stays drawn in the monitor,
and marks on the geometry, with nothing in the middle of the screen
([hud-kit.md](hud-kit.md), "The superbounce readout"). It is a HUD of
its own for the same reason that one is: its rows fit only a
fullscreen frame, and the `modes` check wants the widgets of one HUD
in the same screen modes.

```text
 x 47     130   149
    .---.
   /     \      Rocket Launcher                            fgr.name    y 376
  | track |     Item spawner  Respawn 30 s  Slayer, CTF    fgr.facts   y 397
   \     /      From the map, moved                        fgr.guide   y 418
    '---'
```

| Element | Kind | Shows |
|---|---|---|
| `fgr.name` | text, font 6 | the name of the thing aimed at or held. Hidden with nothing aimed at |
| `fgr.facts` | text, font 6 | the kind, then its two most useful facts: a spawner's respawn time and game types, a starting point's team and game types, a goal's role, team and order, a teleporter's role and channel, an object's group. While a thing is held: its position and heading, in the Coordinates page's units and rounding |
| `fgr.guide` | text, font 6 | aimed: where the thing is from (`From the map`, `From the map, moved`, `From the map, changed`, `Added`), or `Not in <game type>` for a thing of another game type. Held: the placement guide's line, an arrow and the height above the ground under it, then the snap and axes in force. For four seconds after a refusal: its sentence, in place of either |
| `fgr.sel`, `fgr.ring`, `fgr.face`, `fgr.under`, `fgr.from`, `fgr.near.1` to `.12`, `fgr.zone.1` to `.8` | world mark | "World marks" |

- The rows are at the superbounce readout's three places
  (`superbounce_readout`'s `SEGMENTS_GAP` and row pitch), and their
  `longest` texts end inside the frame. A fact that would overrun is
  dropped whole, last first; no text is cut with "...".
- **The two readouts never show together.** A monitor has no bounce to
  read, and the superbounce readout is for a player's biped. The kit
  takes the declaration (`Describe::never_with`, slice K5): the `own`
  check does not compare the two HUDs, and the runtime hides the one
  whose user is not in its mode.
- **Values** come from one view, `forge_readout::View`, made from
  what the session publishes each tick (`forge_screens::Facts`, with
  the aimed thing added), so `forge.status` and the HUD cannot
  disagree. The texts are built by `values(&View)` in the kit crate,
  where the preview and the tests reach them.

### World marks

The marks are the kit's ([hud-kit.md](hud-kit.md), "World marks"):
lines on the geometry, drawn under the HUD by the window's world-line
pass, in the preview and the checks like any element. A mark's pose is
set each tick (`Handle::world_mark`).

| Element | Shape | Pose | Shows |
|---|---|---|---|
| `fgr.sel` | `Bracket` | on the ground under the thing aimed at or held, heading its facing; half its bounding radius, at least 0.25 | the selection. `Normal` aimed; `Go` held where a drop is allowed; `Inactive` held where it is not, with the reason on `fgr.guide`. Its label is the name |
| `fgr.ring` | `Pipper` | at the thing's point, the ring at the radius the engine uses for the kind | a starting point's body radius, a teleporter's and a territory's reach (**S**: where the engine keeps each; a kind with no known radius has no ring) |
| `fgr.face` | `Arrow` | from the thing's point along its facing | starting points, goals and teleporters, which have a facing and no body to show it |
| `fgr.under` | `Pipper` | on the ground straight under a held thing | where it comes down; hidden when the thing is within 0.05 of the ground |
| `fgr.from` | `Bracket` | where a held thing was picked up, at its size | `Dim`: where B puts it back |
| `fgr.near.1` to `fgr.near.12` | `Pipper` | the nearest things that have no body: starting points, goals, teleporters, spawners with nothing out; within 12 world units, nearest first | each in its team's colour (`Style::Team`), `Inactive` when not in the session's game type. Its label is short: `Spawn`, `Flag`, `Hill 2`, the item's name |
| `fgr.zone.1` to `fgr.zone.8` | `Line` | between a hill's corners in order, or a territory's | the outline of the hill or territory the aimed goal belongs to |

- **Team colour.** A mark is in the user's HUD colour today. K5 adds
  `Style::Team(n)` for world marks: the colour the game itself gives
  team `n`, read at run time from where the engine reads it (**S**:
  the source is not traced; hud-kit.md, "Colours", takes its row).
  Until then a mark's team is in its label. No colour is a constant of
  ours.
- **A pool, not a mark a thing.** One layout means a fixed set of
  elements: twelve near marks and eight zone lines. The runtime gives
  the nearest things the poses and leaves the rest without one; a
  mark without a pose costs nothing.
- **Fullscreen only**, as every world mark is. In split screen the
  monitor has its `forge` HUD and navpoints.
- **Navpoints** stay what they are: the held or aimed thing's marker
  and distance, other monitors, a link's two ends
  ([forge-firefight-ui.md](forge-firefight-ui.md), "Markers in the
  world"). A mark is for what is near and on the ground; a navpoint is
  for what is far.
- **The gizmo**'s three axes are that page's `Segment`s and are not
  repeated here.

### The budget

One number has one source: `Session::budget_for(kind)` (new), which
gives what is left, the limit and which limit binds: the session's
object budget, the kind's block (its count against the engine's
limit, or the kit rows left for a kind that needs the kit), or the
object pool (`forge.capacity`).

| Where | Shows |
|---|---|
| `forge.budget.count`, `.meter` (the monitor HUD's pill) | with nothing held, the session's objects left and the share used; holding or aiming, the same for that thing's kind. It flashes from 90% (`Pulse::TimeWarning`) |
| `fgr.guide` | when a hold starts with a tenth or less left: the binding limit by name, "3 item spawners left" |
| The object menu's description | placed and left, by the object's kind ([forge-editing.md](forge-editing.md), "The object menu") |
| **Map limits** (Forge options, slice P4) | a row a limit with used and limit in the value column: Objects, Scenery, Crates, Machines, Item spawners, Starting points, Goals |
| A refused edit | `forge.notice` with the limit named: "Item spawners are full (100)." |

The budget is a count, as Halo 3's is a counter
([forge-editing.md](forge-editing.md), "The object menu"). Moving,
turning and setting a field of an own entry costs nothing; removing
one gives its row back to the kind.

### Saving, loading and renaming

GAME MENU keeps its five Forge rows
([forge-firefight-ui.md](forge-firefight-ui.md), "The screens"); they
act on the variant store (`forge::files`).

| Screen | Doc | Opened by | Is |
|---|---|---|---|
| Name | the game's keyboard (`Effect::Keyboard`) | Save as; Rename; Save on a variant with no name | titled NAME MAP VARIANT, 16 characters |
| Map variant picker | `kit.picker.map-variant` | Load; the lobby's Map Variant... | a picker: Original, then this map's variants by name; the description block has the author, the counts by kind and the game types with goals |
| Map variants | `forge.variants` | **Map variants...** on Forge options | a list page: a row a variant of this map, with where it is (`Saved`, `Xbox Live`) or why it cannot be used in the value column |
| A variant's actions | `forge.variant` | A on a row of Map variants | a dialog on top: **Load**, **Rename**, **Duplicate**, **Upload to Xbox Live**, **Delete** |
| Questions | `forge.question` | below | Yes and No, No focused |

- **Save** writes the open variant under its name and shows "Saved
  <name>" as a notice. A variant opened from Original has no name, so
  Save is Save as.
- **Save as** opens the keyboard on the current name. A name another
  variant of this map has asks "Replace <name>?".
- **Load** asks "Load without saving?" when there are unsaved edits,
  then opens the picker. The pick opens at a fresh log head, and the
  world is reconciled to it: every own entry the old variant touched
  is restored first.
- **Rename** opens the keyboard. A name that is taken is refused with
  the store's sentence as the dialog's status line; nothing is
  replaced by a rename. The name is part of the document, so a renamed
  variant has a new id and a copy already on Live keeps the old name.
- **Delete** asks "Delete <name>?". The open variant cannot be
  deleted; its row says so.
- **Upload to Xbox Live** is present for a signed-in player on a
  variant Live does not have. It ends in a notice with Live's answer.
- **Unsaved edits** show as the Save row's value on GAME MENU and in
  `forge.status`. Leave Game asks before it leaves.
- A variant that fails a check
  ([forge-map-variants.md](forge-map-variants.md), "Rules and limits")
  is listed with its sentence and can be renamed and deleted, not
  loaded.

**In the lobby** the same picker opens over Game Setup from
**Map Variant...**, and the panel's second line is the variant's name
([forge-map-variants.md](forge-map-variants.md), "The lobby").

### Previews

Every piece has a preview and passes its checks before a game run.

```sh
h2 ui preview forge.properties forge.games forge.variants forge.variant kit.picker.map-variant --state all --animation all --out DIR
h2 hud preview forge-readout --state all --out DIR
h2 hud preview forge-all --out DIR
```

- **`forge.properties`** has a state a kind: `object`, `spawner`,
  `spawner-weapon`, `starting-point`, `goal`, `teleporter`,
  `own-changed`.
- **`forge-readout`**'s states: `idle`, `aim-object`, `aim-spawner`,
  `aim-starting-point-red`, `aim-goal-hill`, `aim-teleporter`,
  `aim-other-game-type`, `hold-allowed`, `hold-refused`,
  `hold-snapped`, `refused`, `budget-low`, `near-full` (all twelve
  near marks) and `zone` (all eight lines). Each has sample poses
  (`Describe::world`).
- **`forge-all`** draws the `forge` HUD and the readout in one frame,
  as `superbounce-all` does for its two
  (`huds::forge_all::MOMENTS`): the moments of one edit, from flying
  with nothing aimed at, through aim, pick up, a snapped hold, a
  refused drop, a drop, a field change and a save.
- The HUD kit's checks hold in every state: fit, glyphs, frame,
  retail, own, one layout, prompt, modes, drawn and world
  ([hud-kit.md](hud-kit.md), "The preview and the checks").

## Interfaces

- **HUD kit**: `hud_kit::register` and `Handle::{publish, world_mark,
  navpoint_at}` as they are; added by K5 `Style::Team(u8)`,
  `Shape::Line` and `Describe::never_with`.
- **UI kit**: a doc's rows gain `on`, `prop`, `open` and `values`
  (`ui_kit::data`); `list_class::open`, `picker::offer_list` and
  `Effect::Keyboard` as they are. No new screen base.
- **Session**: `forge_screens::Facts` gains the aimed thing, its
  kind, fields and budget; `sheet::Choice` gains the property choices
  by name.
- **Agent API**: no method of its own. `ui.screen` and `ui.press`
  drive the screens, `hud.kit` reads both HUDs, and `forge.status`
  carries what the readout shows.
- **Guest**: no new entry.

## Rules and limits

- No pixel is the compositor's and no colour, rectangle or font is a
  constant of ours.
- A choice opens a picker or a dialog on top; a number opens the
  keyboard. No row draws arrows.
- A row, a mark or a fact that is not built, or whose field is not
  decoded, is absent.
- One layout a screen and a HUD: a state changes texts, styles, poses
  and what shows.
- The readout and the marks are presentation. They read the session
  and never write it, and nothing of them is in the document, the log
  or the sim.
- GAME MENU does not scroll: everything past its five Forge rows is on
  Forge options.
- A screen with no retail counterpart is not built
  ([forge-firefight-ui.md](forge-firefight-ui.md), "Rules and limits").

## Slices

One lane each, one to two hours, code and tests on synthetic data.
The document, session and Live slices (M, D, V, G) are in
[forge-map-variants.md](forge-map-variants.md), "Slices"; U and K1 to
K4 are in [forge-firefight-ui.md](forge-firefight-ui.md); P1 to P6 in
[forge-design.md](forge-design.md). Paths are under `crates/` unless
they start with `assets/` or `docs/`.

| # | Slice | Owns | Tests | Done when |
|---|---|---|---|---|
| **K5** | World marks: `Shape::Line`, `Style::Team`, `Describe::never_with`. The team colour's source is traced first and written into hud-kit.md, "Colours" | `rewrites/hud-kit-core/src/{world.rs, model.rs, tests.rs}`, `tools/hud-kit/src/{check.rs, preview.rs, tests.rs}`, `extensions/game/src/hud_kit_world.rs`, `docs/game/hud-kit.md` | `world::edges` of a line is one segment; the world check refuses `Team` off a world mark; two HUDs that declare each other are not compared by `own`; the preview draws both | the synthetic sheet passes |
| **H1** | The readout's description: elements, `View`, `values`, the states and their sample poses | `rewrites/hud-kit-core/src/huds/{forge_readout.rs (new), mod.rs}`, `rewrites/hud-kit-core/src/tests.rs` | every state passes every check on the synthetic source; a table of views and their three texts; no state's text is wider than its `longest`; an overrun drops the last fact whole | `h2 hud preview forge-readout --state all --synthetic` writes every state |
| **H2** | `forge-all`: the moments of an edit, both HUDs in one frame | `rewrites/hud-kit-core/src/huds/{forge_all.rs (new), mod.rs}` | each moment names a state of each HUD that exists; the sheet passes | `h2 hud preview forge-all --synthetic` writes the moments. Needs U3 |
| **H3** | The runtime: register after the `forge` HUD, feed the view, choose the near pool, set the poses | `extensions/game/src/forge_readout.rs` (new), `extensions/game/src/forge_glue.rs` | a table from `Facts` to `View`; the twelve nearest of thirty things, by distance then identity; a held thing's `under` and `from` poses; with the monitor off every element is hidden | tests pass. Needs H1, M5, M7 |
| **S1** | Properties from its doc: the row keys, the binder, the page's states, `sheet::PROPERTIES` removed | `assets/ui/screens/forge.properties.yaml`, `extensions/game/src/ui_kit/data.rs`, `extensions/game/src/forge_screens/{props.rs (new), sheet.rs, pages.rs, tests.rs}` | the doc loads and round-trips through `data::to_yaml`; an unknown `prop` is a `DataError` naming the row; a `Page` table a kind; a property the map lacks has no row; the content check passes in every state | `h2 ui preview forge.properties --state all` writes a state a kind. Needs M6 |
| **S2** | The Game types dialog | `assets/ui/screens/forge.games.yaml`, `extensions/game/src/forge_screens/games.rs` (new), one line of `ui_kit/screens.rs` | a `Page` table: ticks, All game types, Done writes one step, five engines show the refusal as the status line and write nothing | preview in every state. Needs D2, S1 |
| **S3** | The map variant picker and the lobby's row | `assets/ui/screens/kit.picker.map-variant.yaml` (in place of `kit.picker.forge-map.yaml`), `extensions/game/src/{lobby_mode.rs, lobby_options.rs}`, one line of `ui_kit/screens.rs` | `Page` tables for Game Setup in CUSTOM GAMES and FORGE; Original is first; an unusable variant shows its sentence and cannot be chosen; a map change clears the panel line | `h2 ui preview kit.picker.map-variant`. Needs V1, V2 |
| **S4** | Save, Save as and Load on the store, with their questions and notices | `extensions/game/src/forge_screens/{variants.rs (new), game_menu.rs}`, `extensions/game/src/forge_glue/editing.rs`, `assets/ui/screens/forge.question.yaml` | a table of flows: unnamed Save opens the keyboard; a taken name asks; No is focused; Load with edits asks first; each ends in its notice | tests pass. Needs V1 |
| **S5** | Map variants and a variant's actions | `assets/ui/screens/{forge.variants.yaml, forge.variant.yaml}`, `extensions/game/src/forge_screens/variants.rs`, `forge_screens/sheet.rs` (`OPTIONS`), two lines of `ui_kit/screens.rs` | `Page` tables; rename to a taken name writes nothing; the open variant's Delete is absent; Upload is absent when signed out | previews in every state. Needs S4 |
| **S6** | The budget by kind: `Session::budget_for`, the pill's feed, the guide line, the refusal's sentence | `extensions/forge/src/session.rs`, `extensions/forge/src/tests.rs`, `extensions/game/src/forge_readout.rs` | a table of counts and which limit binds; the sentence names it; an own entry's move costs nothing | tests pass. Needs M5, H3 |

K5 and H1 need nothing built here and can start at once. S2, S3 and
S5 each add a name to `ui_kit::screens::NAMES`, which shifts later
screen ids ([ui-in-maps.md](ui-in-maps.md), "Rules and limits"), so
they land one after another.

## Evidence

- **Code**: the screens' docs and the baked data path,
  `crates/extensions/game/src/ui_kit/data.rs`; the injected tags,
  `ui_kit/inject/`; the Forge sheets,
  `forge_screens/sheet.rs`.
- **Code**: world marks, their shapes, styles and check,
  `crates/rewrites/hud-kit-core/src/world.rs` and
  `crates/tools/hud-kit/src/check.rs`; the superbounce readout,
  `huds/superbounce_readout.rs` and `huds/superbounce_guide.rs`.
- **Design**: the readout's rows, the mark pool, the row keys, the
  save and rename flows.

## Not verified

- The engine's source for a team's colour, and that a world mark in it
  reads against every level's floor.
- The radius the engine uses for a starting point, a teleporter and a
  territory.
- That three rows of font 6 right of the tracker stay clear of the
  monitor HUD's own elements on every map's HUD art (the `retail` and
  `own` checks say so offline; a run confirms it).
- That twelve near marks and eight lines stay inside the world-line
  pass's cost on a busy map.
- That the game's keyboard takes a 16-character limit and a title of
  ours from an in-game screen.
- That a list page and a dialog of ours open over GAME MENU in a game
  in the order the flows need.

## Related

- [forge-map-variants.md](forge-map-variants.md): things, ops, the
  file, the lobby and Live.
- [forge-firefight-ui.md](forge-firefight-ui.md): the monitor HUD, the
  Forge screens and the rules for both.
- [forge-editing.md](forge-editing.md): the controls and what each
  row does.
- [hud-kit.md](hud-kit.md): elements, world marks, the preview and
  the checks.
- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md),
  [ui-in-maps.md](ui-in-maps.md): screens as docs and as tags.
- [superbounce.md](superbounce.md): the approach guide the marks take
  after.
