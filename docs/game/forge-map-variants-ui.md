# Forge map variants: the editor's HUD, world marks and screens

Status: Design, slices K5, H1, H3, S1, S2, S3, S4, S5 and S6 built (S3's picker description lacks the game types with goals, and S5's dialog has no Upload to Xbox Live: the game has no Live client for it). The Forge screens in the game (slice U4), the HUD kit, its world marks and the superbounce readout this page copies are built, and so are `Shape::Line`, `Style::Team` and `Describe::never_with` (K5) and the readout's description with its fourteen states (H1), including the `fgr.zone` marks, the readout's `never_with` declaration and the team-coloured near marks. The runtime (H3, `forge_readout.rs`) registers the HUD, feeds it each tick from the session's aimed or held thing and its bodiless things, chooses the near pool and poses the marks; it has no ring feed yet (below), and its guide row says a refusal's sentence and, while an added thing is held with a tenth or less left, the binding limit (S6). The Properties page's rows are the doc's `props:` block (S1), with **Game types...** and **Team** (the document's `team` property, which a rules script reads) and **Spare clips** (the document's `spare_clips` property, on a spawner whose item collection holds a weapon, which the game writes to each weapon the spawner makes; `Session::set_spare_clips`, `game::forge_spare_clips`) and **Place at start** (the document's `place_at_start` property: Yes, No; `Session::set_place_at_start`, `game::forge_place_at_start`) on an item spawner, **Weight** (the document's `weight` property: Low, Normal, High; `Session::set_spawn_weight`, `game::forge_spawn_weight`) on a starting point, which opens the Game types dialog (`forge.games`, S2; `forge_screens/games.rs`), and the acts **Reset orientation**, **Duplicate**, **Delete** and **Restore original** (P1; Delete asks first when the thing has links, and Restore original, on an own thing the document changes, always asks; `Session::duplicate` holds the copy, and in a kit session also copies a spawner, a starting point, a goal and a teleporter through `Session::duplicate_kit`, the row's `Extras::kit`); the rows of the other slices are not built. `Session::budget_for` (S6; `extensions/forge/src/{budget.rs, session/budget.rs}`) is the budget's one source. The Map limits page (P4) lists the object pool and scenario counts on Forge options.
Code: `crates/extensions/game/src/forge_screens/` (the screens), `content/ui/screens/forge.*.yaml` (their docs), `crates/rewrites/hud-kit-core/src/{world.rs, views/}` (the HUD kit's model), `crates/extensions/game/src/hud_kit_world.rs` (the world-line source), `crates/extensions/game/src/forge_readout.rs` (the readout's runtime), `crates/extensions/game/src/forge_screens/props.rs` (the Properties rows). Tests: named per slice below.
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

- **The doc.** Each screen is `content/ui/screens/<name>.yaml`: kind,
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
  picker is over Game Setup, as `kit.picker.map-variant` is
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
`props:` block (`content/ui/screens/forge.properties.yaml`), bound to the
thing by `forge_screens::props::rows` and used by
`sheet::properties_sheet`:

```yaml
props:
  prop.6:
    text: "Respawn time"
    help: "Seconds before the item comes back."
    on: [spawner]
    prop: "respawn time"
    open: keyboard
```

`on` is the kinds the row is for (`any` when none: `object`, `spawner`,
`starting-point`, `goal`, `teleporter`), `prop` a name in
`forge::props` (or `coordinates` and `heading`, which are not fields),
`open` what A does (`picker`, `keyboard`, or a Forge screen's name), and
`values` a picker's choices, in order: the choice sent is the index. A
doc that names a property `forge::props` lacks, a kind or an `open` it
does not know, or a picker without values is a `DataError` naming the
row's field (`ScreenDoc::parse`). A property the thing does not have
(its block lacks the field, or the layout does not let it be edited) has
no row, and neither does one the binder cannot make, not greyed. The
rows read their value through `forge::props`; a write is still the
page's `Choice::Set` and typed number, which the editor turns into the
op.

A value row shows its value in the skin's value column. No row has
arrows; left and right on a number row are the Coordinates page's
nudge and nothing else.

| Kind | Row | A opens | Writes | Slice |
|---|---|---|---|---|
| any | Coordinates... | the Coordinates page ([forge-editing.md](forge-editing.md), "Precision") | position, rotation | P1 |
| any with a rotation | Reset orientation | acts | rotation | P1 |
| any | Duplicate | acts; holds the copy | `Placement::Add` with the source's fields | P1; a kit-session kind: built (P2) |
| any | Delete | the question when it has links, else acts | `EntryOp::Remove` | built (M6) |
| own, changed | Restore original | the question (`Ask::Restore`, `forge.question` state `restore`) | `EntryOp::Reset` | built (M6) |
| Object | Object... | a picker of its group's palette | the palette entry | M6 |
| Object | Game types... | the Game types dialog | the `games` property | G1 |
| Any thing | Label, Label number, Clear label | the keyboard on the text, the keyboard on the number, and an act; `forge.properties` rows `label`, `label number` and `clear label` (`open: keyboard` and `open: act`) | the label ([rules-engine.md](rules-engine.md#forge-labels)) | built; a picker of a script's declared labels is design |
| Group member in a local session | Pivot | a picker: Centre, First member | replaces the group op in one undo step | built (P5) |
| Object the session placed | Save as prefab | the keyboard on the prefab's name; the row `prefab` | `<data>/forge/prefabs/<name>.h2prefab` ([forge-editing.md](forge-editing.md), "Selection, groups and prefabs") | built |
| Item spawner | Item... | the object menu's item groups, as a picker | `item` | M6 |
| Item spawner | Respawn time | a picker: Map default, then seconds | `respawn time` | M6 |
| Item spawner | Return time | the keyboard on a whole number, as Respawn time (`return time`, `open: keyboard`) | `abandon limit` | built (P3) |
| Item spawner | Place at start | a picker: Yes, No | no field: a countdown write after the round reset, which no document property reaches ([2026-11-27-forge-spawner-rows.md](../engine/notes/2026-11-27-forge-spawner-rows.md)) | not built |
| Item spawner | Game types... | the Game types dialog | the game-type slots | D3 |
| Item spawner | Team | a picker of teams, None first (`forge.properties` row `spawner team`, `Session::set_team`) | the `team` property: one `EntryOp::Property`, nothing written to the world | built (D4) |
| Item spawner whose collection holds a weapon (`forge_map::holds_weapon`; a mixed collection does) | Spare clips | a picker: Weapon's own, then 0 to 10 (`forge.properties` row `spare clips`) | the `spare_clips` property: one `EntryOp::Property`, and `World::spare_clips` on the entry's row | built (D5) |
| Starting point | Team | a picker of teams | `team` | M6 |
| Starting point | Game types... | the Game types dialog | the game-type slots | D3 |
| Starting point | Weight | a picker: Low, Normal, High (`forge.properties` row `weight`) | the `weight` property: one `EntryOp::Property`, and `World::spawn_weight` on the location's row; the game adds -0.5 or +0.5 to the chooser's weight ([2026-11-27-forge-spawner-rows.md](../engine/notes/2026-11-27-forge-spawner-rows.md)) | built (D5) |
| Goal | Role | a picker of the roles the session's game type reads | the flag's `type` | M6 |
| Goal | Team | a picker of teams | `team` | M6 |
| Goal | Order | a picker of numbers | the hill corner or the `identifier` | M6 |
| Teleporter | Role | a picker: Sender, Receiver | the flag's `type` | M6 |
| Teleporter | Channel | a picker of channels | a link ([forge-design.md](forge-design.md), "F6 in detail") | built, for the map's own flags and the session's |

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
  ticks or unticks it; the tick is the row's value column (On, Off), and
  A on **All game types** ticks every engine, or clears them when all
  are ticked. **Done**
  writes the set as one undo step. A set the entry cannot hold is
  refused when Done is pressed, with the document's sentence as the
  dialog's status line
  (`forge_screens/games.rs`; a tick row is `Act::Tick`, Done is `Act::Done`
  and sends `Choice::Games`, which the session writes with
  `Session::set_games`)
  ([forge-map-variants.md](forge-map-variants.md), "Game types").
- The built page's Heading row goes when Coordinates lands; Heading is
  its Yaw.

### The readout

A HUD kit `Hud` named `forge-readout`
(`crates/rewrites/hud-kit-core/src/views/forge_readout.rs` for its states,
`content/forge/huds/forge-readout.hud.yaml` for its elements),
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

- **Team colour.** A mark is in the user's HUD colour unless it has
  `Style::Team(n)` (K5): the colour the game itself gives team `n`,
  read at run time from the loaded map's `mulg`
  ([hud-kit.md](hud-kit.md), "Colours", "A team's colour"). Where the
  map has none the mark is in the HUD colour, so a mark's team stays in
  its label. No colour is a constant of ours but the preview's
  stand-ins.
- **The runtime** (`forge_readout.rs`): `Editor::readout_scene` builds a
  `Scene` each tick (`forge_glue/editing.rs`), the `forge` feature's tick
  hook turns it into the view's values and the poses (`frame`), and
  nothing is published again while neither changed. With the monitor
  off every element is hidden and no mark has a pose. The twelve near
  marks are the nearest things with no body within 12 world units, by
  distance then identity, without the thing aimed at (`choose_near`).
  A held thing's `under` is on the ground its height below it, its
  `from` where it was picked up. The zone is the aimed hill corner's hill: the goals of type 11 to 18
  sharing its identifier, in corner order, closed (`zone_of`). The hold
  line's height is a ray down from the carried thing to the structure
  (`Editor::hold_height`), absent with none in reach. Not fed yet, so
  absent: a ring (no kind's radius is traced). The guide row's refusal is
  the editor's last refused edit (`Editor::refuse`), kept for
  `REFUSAL_TICKS`; its budget is `Session::budget_for` of a held added
  thing's block, given by `budget_of` from a tenth left.
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

One number has one source: `Session::budget_for(block)`
(`extensions/forge/src/session/budget.rs`), which gives what is left, the
limit and which limit binds: the session's object budget (for a block
whose entries make objects) or the block's count against the engine's
limit (`map::scenario_layout`), whichever has less left; a tie binds the
block, and a block with neither has no budget. A removal gives its row
back, and a move or a field of an own entry costs nothing. Kit additions
also check their reserved rows through `Session::kit_left`. The live object
pool is measured separately through `forge.capacity`.

`Session::place` refuses an add to a full block with the budget's
sentence (`Budget::full`: "Item spawners are full (100).").

| Where | Shows |
|---|---|
| `forge.budget.count`, `.meter` (the monitor HUD's pill) | with nothing held, the session's objects left and the share used; holding or aiming, the same for that thing's kind. It flashes from 90% (`Pulse::TimeWarning`) |
| `fgr.guide` | when a hold starts with a tenth or less left: the binding limit by name, "3 item spawners left" |
| The object menu's description | placed and left, by the object's kind ([forge-editing.md](forge-editing.md), "The object menu") |
| **Map limits** (the Forge menu's Limits tab, P4) | Objects, Scenery, Crates, Machines, Item spawners, Starting points and Goals, with each count and its engine limit where known |
| A refused edit | `forge.notice` with the limit named: "Item spawners are full (100)." |

**Map limits** (`forge.map-limits`, `forge_screens/map_limits.rs`)
is a read-only tab of the Forge menu. It shares the live object pool
and scenario block readers with `forge.capacity`. Objects counts live
datums across the game. The scenario rows count entries in their respective
blocks, with Goals counting the netgame flags block. Starting points and
Goals show their counts alone. A failed read shows **Unavailable** in its
row. Each opening takes the latest published counts.

The budget is a count, as Halo 3's is a counter
([forge-editing.md](forge-editing.md), "The object menu"). Moving,
turning and setting a field of an own entry costs nothing; removing
one gives its row back to the kind.

### Saving, loading and renaming

**Save**, **Save as** and **Load** are rows of the Forge menu's Map
variants tab ([forge-firefight-ui.md](forge-firefight-ui.md), "The
screens"); they act on the variant store (`forge::files::variants`).

| Screen | Doc | Opened by | Is |
|---|---|---|---|
| Name | the game's keyboard (`Effect::Keyboard`) | Save as; Rename; Save on a variant with no name | 16 characters (the title NAME MAP VARIANT is not set: the keyboard's open call takes none yet) |
| Map variant picker | `kit.picker.map-variant` | the lobby's Map Variant... | a picker: Original, then this map's saved variants by name; the description block has the author and the counts by kind of what a variant adds (`forge_screens::variants::detail`), or the sentence of one that cannot be used, which A does not choose (`Pick::blocked`) |
| Map variants | `forge.variants` | **Map variants...** on the Forge menu's Map variants tab | a dialog (`forge_screens::variants::list_sheet`) on the large dialog, so each row's line shows beside it: a row a variant of this map, with where it is (`Saved`, `Xbox Live`) in the value column, or `Unusable` and its sentence as the row's line |
| A variant's actions | `forge.variant` | A on a row of Map variants | a dialog on top (`variant_sheet`): **Load**, **Rename**, **Duplicate** and **Delete**, each only where it can act. A row that is only a fact (the page's "No map variants", the open variant's "Open now") takes no A: while it is focused the legend is Back alone: the footer is set as the screen opens and again on each focus change (`ForgeScreen::prompts_at`, `focused_footer`; `spec` takes the same legend) |
| Questions | `forge.question` | below | Yes and No, No focused |

- **Save** writes the open variant under its name
  (`Session::save_variant`, which is `forge::files::variants::save` of
  the session's play form) and shows "Saved <name>." as a notice. A
  variant opened from Original has no name, so Save opens the keyboard,
  as Save as does.
- **Save as** opens the keyboard on the current name, 16 characters
  (`forge_screens::NAME_CHARS`). The open variant's own name, with any
  case, saves over it. A name another variant of this map has comes
  back from the store as `NameTaken` and asks "Replace <name>?", No
  focused; Yes writes over that file, No ends in "Not saved.". A name
  the store refuses (blank, over 16 characters) is the notice and
  writes nothing.
- **Load** picks among this map's saved variants that pass the store's
  checks (a Live copy or a variant with a problem is listed on Map
  variants, not offered here), then, when there are unsaved edits, asks
  "Load without saving?" before it loads; No ends in "Not loaded.". The
  load opens at a fresh log head, and the world is reconciled to it
  (`Session::load_variant`: the document is checked against the
  session's limits, and the kit put back, before the game is asked to
  change): every own entry the old variant touched is restored first.
  Each flow's decisions are `forge_screens::variants`; the editor
  (`forge_glue::editing`) runs the store calls off the game thread.
- **Rename** opens the keyboard (`NumberField::RenameVariant`; the
  screens hold the dialog's file stem, `forge_screens::target_variant`).
  A name that is taken is refused with the store's sentence as the
  notice; nothing is replaced by a rename. Renaming the open variant
  renames the session. **Duplicate** types the copy's name the same way
  and copies the file (`variants::duplicate`). The name is part of the document, so a renamed
  variant has a new id and a copy already on Live keeps the old name.
- **Delete** asks "Delete <name>?" (`Ask::DeleteVariant`, No focused);
  Load and Delete close Map variants first, so the question opens over
  the world. The open variant cannot be deleted: an info row says so in
  place of Delete.
- **Upload to Xbox Live** joins the dialog for a signed-in player on a
  variant Live does not have, when the game has a Live client for it. It
  is not built.
- **Unsaved edits** show in `forge.status`. GAME MENU's Leave Game
  asks before it leaves.
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
  refused drop, a drop, a field change and a save. It is the
  monitor's and fullscreen only, as the readout is; the save's notice
  is "Saved <name>.".
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
- GAME MENU has no Forge row: Save, Save as and Load are on the Forge
  menu.
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
| **H1** | The readout's description: elements, `View`, `values`, the states and their sample poses | `rewrites/hud-kit-core/src/views/{forge_readout.rs (new), mod.rs}`, `rewrites/hud-kit-core/src/tests.rs` | every state passes every check on the synthetic source; a table of views and their three texts; no state's text is wider than its `longest`; an overrun drops the last fact whole | `h2 hud preview forge-readout --state all --synthetic` writes every state |
| **H2** (built) | `forge-all`: the moments of an edit, both HUDs in one frame | `rewrites/hud-kit-core/src/views/{forge_all.rs (new), mod.rs}` | each moment names a state of each HUD that exists; the sheet passes | `h2 hud preview forge-all --synthetic` writes the moments. Needs U3 |
| **H3** | The runtime: register after the `forge` HUD, feed the view, choose the near pool, set the poses | `extensions/game/src/forge_readout.rs` (new), `extensions/game/src/forge_glue.rs` | a table from `Facts` to `View`; the twelve nearest of thirty things, by distance then identity; a held thing's `under` and `from` poses; with the monitor off every element is hidden | tests pass. Needs H1, M5, M7 |
| **S1** | Properties from its doc: the row keys, the binder, the page's states, `sheet::PROPERTIES` removed | `content/ui/screens/forge.properties.yaml`, `extensions/game/src/ui_kit/data.rs`, `extensions/game/src/forge_screens/{props.rs (new), sheet.rs, pages.rs, tests.rs}` | the doc loads and round-trips through `data::to_yaml`; an unknown `prop` is a `DataError` naming the row; a `Page` table a kind; a property the map lacks has no row; the content check passes in every state | `h2 ui preview forge.properties --state all` writes a state a kind. Needs M6 |
| **S2** (built) | The Game types dialog | `content/ui/screens/forge.games.yaml`, `extensions/game/src/forge_screens/games.rs` (new), one line of `ui_kit/screens.rs` | a `Page` table: ticks, All game types, Done writes one step, five engines show the refusal as the status line and write nothing | preview in every state. Needs D2, S1 |
| **S3** (built) | The map variant picker and the lobby's row | `content/ui/screens/kit.picker.map-variant.yaml` (in place of `kit.picker.forge-map.yaml`), `extensions/game/src/{lobby_mode.rs, lobby_options.rs}`, one line of `ui_kit/screens.rs` | `Page` tables for Game Setup in CUSTOM GAMES and FORGE; Original is first; an unusable variant shows its sentence and cannot be chosen; a map change clears the panel line | `h2 ui preview kit.picker.map-variant`. Needs V1, V2 |
| **S4** | Save, Save as and Load on the store, with their questions and notices | `extensions/game/src/forge_screens/{variants.rs (new), menu.rs}`, `extensions/game/src/forge_glue/editing.rs`, `content/ui/screens/forge.question.yaml` | a table of flows: unnamed Save opens the keyboard; a taken name asks; No is focused; Load with edits asks first; each ends in its notice | tests pass. Needs V1 |
| **S5** (built) | Map variants and a variant's actions | `content/ui/screens/{forge.variants.yaml, forge.variant.yaml}`, `extensions/game/src/forge_screens/variants.rs`, `forge_screens/sheet.rs` (`OPTIONS`), two lines of `ui_kit/screens.rs` | `Page` tables; rename to a taken name writes nothing; the open variant's Delete is absent; Upload is absent when signed out | previews in every state. Needs S4 |
| **S6** (built) | The budget by kind: `Session::budget_for`, the pill's feed, the guide line, the refusal's sentence | `extensions/forge/src/session.rs`, `extensions/forge/src/tests.rs`, `extensions/game/src/forge_readout.rs` | a table of counts and which limit binds; the sentence names it; an own entry's move costs nothing | tests pass. Needs M5, H3 |

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
  `views/superbounce_readout.rs` and `views/superbounce_guide.rs`.
- **Design**: the readout's rows, the mark pool, the row keys, the
  save and rename flows.

## Not verified

- That a world mark in a team's colour reads against every level's
  floor.
- The radius the engine uses for a starting point, a teleporter and a
  territory.
- That three rows of font 6 right of the tracker stay clear of the
  monitor HUD's own elements on every map's HUD art (the `retail` and
  `own` checks say so offline; a run confirms it).
- That twelve near marks and eight lines stay inside the world-line
  pass's cost on a busy map.
- That the game's keyboard takes a 16-character limit and a title of
  ours from an in-game screen.
- That the game's keyboard and a question of ours open over a game as
  the Forge menu closes, in the order the flows need.

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
