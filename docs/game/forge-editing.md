# Forge editing: the monitor's controls, tools and properties

Status: Partly built. The Halo 3 button table, flying with thrust and fine movement, pick up and drop, manipulating a held object, movement and rotation snap, world or local axes, Coordinates with typing and nudging, hold to delete, undo and redo, the Tools picker with the Objects and Links tools, the object menu, a Properties page with Heading and the spawn, goal and item rows, Tool settings with Movement snap, Rotation snap and Axes, the Overlays page with the Markers, Other monitors, Axes and Nav problems overlays, **Generate nav**, the placed objects browser, groups (Select, Group selection, Ungroup, Lock group; moving one member moves the group), labels (Label, Label number, Clear label) and prefabs (Save as prefab, the object menu's Prefabs row) exist. The Nav tool, the nav's sectors drawn in the game, the group pivot row, a picker of a script's declared labels, a held A to select and lights are design.
Code: `crates/extensions/forge/src/tool.rs` (`Tool`, `Action`, `Input`, `Command`), `crates/extensions/forge/src/session.rs` and `session/groups.rs` (`Session`), `crates/extensions/game/src/forge_controls.rs` and `forge_controls/bindings.rs` (pad, keyboard and agent input; the bindings table), `crates/extensions/game/src/forge_screens/sheet.rs` (the menus' tables), `crates/extensions/game/src/forge_glue/editing.rs` (the host). Tests: `crates/extensions/forge/src/tool_tests.rs`, `link_tests.rs`, `props_tests.rs`, `group_tests.rs`, `tests.rs`; `forge_controls.rs`, `bindings.rs`, `forge_screens/tests.rs` and `forge_glue/editing/tests.rs` in `crates/extensions/game/src`.
Switch: `forge`, listed in [switches.md](../runtime/switches.md). No switch of its own.

What a player can do as a monitor in a Forge session, and which button
does it. The behaviour takes after Halo 3's and Halo: Reach's Forge,
with the parts of Halo Infinite's that Halo 2's engine can carry. The
look is Halo 2's: every screen and HUD piece named here is drawn as
[forge-firefight-ui.md](forge-firefight-ui.md) says. The architecture
under it (the document, the log, the kit, the network modes) is
[forge-design.md](forge-design.md). The rule that must hold: **every
edit is one op in the session's log**, made by `forge::Session` from a
`tool::Command`, so the pad, the keyboard, the agent API and a peer all
edit through the same path and undo the same way.

Evidence labels are those of [docs-style.md](../meta/docs-style.md),
plus **C**: the retail games as publicly documented, with the source in
[notes/forge-prior-art.md](notes/forge-prior-art.md).

## What it takes after

| From | Taken | Not taken |
|---|---|---|
| Halo 3 | the monitor and its buttons; the object menu by category with a count left; a budget that is a counter; spawn and goal objects edited under a chosen game type; item properties; teleporter channels; Forge as a lobby mode with split screen, System Link and Live; save as a map that plays in custom games; the session as a film (**C**) | Run-Time Minimum and Maximum (Halo 2's spawner respawns each entry on its own, **E**, [forge.md](forge.md) 1.2); Symmetry; Spare Clips, until an entry field for it is found |
| Halo: Reach | rotation snap; Coordinates with nudging; a fine mode; Normal, Fixed and Phased as one Physics choice; a structure palette of pieces; labels that give an object its part in a game type; boundaries as volumes (**C**) | Forge World as one large map: a session edits the map the disc has |
| Halo Infinite | movement snap; world or local axes; typed numbers; duplicate; groups and prefabs; a browser of placed objects; limits shown by kind; placed lights; logic as a graph; undo and redo, which here also work with other players (**C**) | free scale; welding; an editable pivot; light probes, reflection volumes and sky settings; hiding objects; a node canvas on a pad |

### The Infinite features, judged

"Fit" is against Halo 2's engine and our session: a document of ops,
objects that are guest truth, tool state that is presentation
([forge-design.md](forge-design.md), "Who owns state").

| Feature | Verdict | Fit | Cost |
|---|---|---|---|
| Undo and redo | built | An undo is the author's inverse op in the log, so it works in a shared session, where Infinite's does not (**C**). `Session::undo`, `Session::redo` | none |
| Movement and rotation snap | built | Tool state only: the frame is rounded before the op is made (`Tool::set_movement_step`, `Tool::set_rotation_step`) | none |
| World or local axes | built | Tool state: which basis the snap, the nudge and the gizmo use (`Tool::set_axes`); Local is the pickup's basis | none |
| Typed numbers, Coordinates | built | A frame is six `f32` values kept bit-exact in the document (`forge::Frame`), so a typed number is the number saved. Halo 3's variants quantise positions (**C**, [forge.md](forge.md) 1.1); ours do not. A typed or nudged value moves the held thing live, and closing writes one op (`Session::hold`, then `move_thing`) | none |
| Duplicate | P1 | One `Placement::Add` with the source's fields, held at once | in P1 |
| Placed-object browser | built | A picker over `Session::objects`; choosing a row flies the monitor to it (`sheet::placed_rows`, `Camera::toward`) | none |
| Limits by kind | P4 | `forge.capacity` already measures the object pool, each block's count and limit, and tag headroom | in P4 |
| Selection, groups, group pivot | built, but the pivot row and a held A | A selection is tool state (`Tool::toggle_selected`). A group is a Forge-family op naming its members by endpoint (`forge_doc::Group`); moving one is a batch of entry sets in one undo step, as a delete of a linked machine is one step (`Session::move_thing`). The pivot, centre or first member, is a field of the op (`pivot_first`); the Properties page makes centre groups, and `forge.propose` writes the other | none; the document format is 4 (`FORMAT_GROUPS`) |
| Prefabs | built (P5) | A prefab is a small file: tag keys and frames relative to a pivot, no game bytes. Placing one checks every tag against this map's palette and refuses by name | sharing on Live rides F14 |
| Folders, lock | built | A group's name is the folder (the browser lists groups first); lock is a flag on the group (`Forge::GroupLock`) that the tool honours: it does not aim at a locked group's things | none |
| Placed lights | P6 | A kit piece of ours with a generated light tag; colour and size are tag field writes, which are Live ([forge-design.md](forge-design.md), "What the engine allows at runtime"). Halo 2 lights the level from a baked lightmap, so a placed light adds dynamic light only and casts no shadow (**S**: the engine's count of dynamic lights a frame, to be probed) | M, after F8's generators |
| Node graph | F7, [rules-engine.md](rules-engine.md) | The graph exists in the design as links between ports ([forge-features.md](forge-features.md), "Teleporters, buttons and triggers"), compiled to the rules IR. On a pad it is edited in the world with the Links tool, one link at a time; a canvas view belongs to the paired editor. Its caps are the validator's and the Forge step's budget of node visits a tick | L (F7), unchanged |
| Bots on an edited map | F9 | `bots::autonav` rebuilds the nav mesh off the game thread | M (F9), unchanged |
| AI ground for scripted games | built (GM3) | Labelled markers on the Objects tool's AI and objectives page, **Build AI ground** and **Test waves** on the Forge options page, and a save that builds first when the ground is stale ([forge-map-variants.md](forge-map-variants.md), "AI ground") | none |
| Test without leaving | built | D-pad Up swaps monitor and player in the same session, as in Halo 3 | none |
| Versions | F14 | A document is content-addressed, so every published save is a version | in F14 |
| Magnets | with F8, kit pieces only | Our generated pieces can carry snap points at face centres and corners. A retail object has none and gets none | S, in F8 |
| Scale | not adopted | Whether a placement's scale reaches collision and physics is not verified ([forge-design.md](forge-design.md), "Not verified"). Kit pieces come in a ladder of sizes instead | |
| Welding | not adopted | Parenting an arbitrary object to another needs an engine entry nobody has proven, and replication of the pair | |
| Editable pivot | not adopted | An object turns about its tag's origin. A group turns about its centre or its first member (P5), which covers the use | |
| Light probes, reflection volumes, sky | not adopted | The level's light is a lightmap in the map file; replacing it is the relighting work, a Rebuild, outside Forge | |
| Hiding objects | not adopted | A hidden guest object changes the sim; the browser's fly-to covers finding things | |

## Controls

One table binds every action: `forge_controls::bindings::BINDINGS`.
The pad gate, the keyboard map, the Controls page and the HUD's prompt
all read it, so none can disagree. The host pad's bumpers are the game's
Black and White buttons.

| Action | Controller | Keyboard and mouse |
|---|---|---|
| Monitor or player | D-pad Up | Tab |
| Fly | Left stick | W A S D |
| Look | Right stick | Mouse |
| Up, down | RB, LB | E, Q |
| Thrust | LT (hold) | Shift (hold) |
| Fine movement, on or off | Click the left stick | Left Ctrl |
| Pick up, drop, confirm | A | Left click |
| Manipulate a held object | hold RT: the left stick pushes it away, pulls it in and rolls it; the right stick turns it | hold the right mouse button; the wheel pushes and pulls |
| Object menu with nothing aimed at; Properties of the thing aimed at or held | X | F |
| Delete | hold Y | Delete |
| Cancel a hold or a started link; back | B | Escape |
| Tools | D-pad Right | T |
| Tool settings | D-pad Down | G |
| Overlays | Back | H |
| Undo; redo | D-pad Left; hold for redo | Cmd+Z; Shift+Cmd+Z |
| Forge menu | Start | Escape with nothing held |

- The first eleven rows are Halo 3's table (**C**), with B as cancel
  where Halo 3 has hold to drop. The D-pad's other three directions and
  the stick click are ours; Reach puts its tools on B (**C**).
- In the Links tool the same buttons mean the same things: A starts a
  link and makes it, X opens the Link question of the thing aimed at,
  hold Y removes its latest link, B cancels a started link.
- Start, and Escape with nothing to cancel, are the game's: they open
  its GAME MENU with the Forge rows in it. The table's Start row does
  nothing of the session's (`Does::Game`).
- While a screen is up, ours or the game's, the monitor takes no input:
  the pad and the keys go to the game's UI (`forge_controls::State`,
  `screen`).
- Thrust triples flying while held; manipulating, it does not apply.
- On the Controls page, up and down, and Tool settings and Overlays, share
  a row each (`bindings::PAIRS`), so the page fits the in-game list's
  sixteen lines.
- `forge.input` takes each action by the name `Action::named` gives it
  ([agent-api.md](../runtime/agent-api.md), "Local Forge session").

## Tools

A tool decides what the reticle aims at and what A does. D-pad Right
opens the Tools picker (`forge_screens::sheet::TOOLS`); the HUD names
the tool in use.

| Tool | Aims at | Slice |
|---|---|---|
| Objects | placed objects; the map's own objects, item spawners, starting points and goals ([forge-map-variants.md](forge-map-variants.md), "Things"); and, in a kit session, new spawns, goals and item entries | built for placed objects and the map's own things (M5 to M9); P2 |
| Links | teleporter flags and machines, then every port | built (F6); F7 |
| Volumes | trigger volumes, boundaries, barriers | F7, F8 |
| Emitters | emitters | F7 |
| AI | squads | F11 |
| Shape, Paint | faces, textures | F13 |
| Nav | the nav overlay and its problems | F9 |

A tool that is not built is absent from the picker.

## Overlays

An overlay is a drawing the monitor adds over the world. Back (H) opens
the Overlays page: a row for each overlay in `sheet::OVERLAYS`, showing
On or Off, A opening a picker on the two. Every overlay starts On.
Overlays are tool state, as the settings are: never in the document, the
log or the sim (`forge::tool::Overlay`, `Tool::overlays`).

| Overlay | Draws | Off |
|---|---|---|
| Markers | the game type markers: a spawn, flag, hill, teleporter, item spawner and ground label each (`forge_glue::markers_world`) | `Editor::marker_entries` is empty |
| Other monitors | a ring at each other seat's monitor (`Editor::monitor_markers`) | no rings; the seats' bodies stay |
| Axes | the axes and outline at the held or aimed object (`forge_glue::gizmo`) | no gizmo |
| Nav problems | a mark at each problem the last **Generate nav** found (`markers_world::nav_problems`, [forge-nav.md](forge-nav.md)) | no marks |

The sectors and links of the nav are in `forge.nav`'s `overlay`; the
game does not draw them. `forge.status`
carries `overlays`, a map of each overlay's name to whether it is on;
`forge.input` takes `overlays` to open the page.

## Precision

Tool settings are per player and are presentation: they are never in
the document or the log. They are settings of the profile's store
(`forge.*` in `setting_defs::ALL`, as Firefight's choices are), so they
persist and each has a picker.

| Setting | Values | Default |
|---|---|---|
| Movement snap | Off, 0.05, 0.1, 0.25, 0.5, 1, 2 world units | Off |
| Rotation snap | Off, 1, 5, 15, 30, 45, 90 degrees | 15 |
| Axes | World, Local | World |
| Fine movement | On, Off | Off |

- **Movement snap.** With World axes the held object's position is
  rounded to the grid on each world axis. With Local axes the move since
  pickup is rounded along the object's own axes as they were at pickup.
- **Rotation snap.** The turn since pickup is rounded, so picking an
  object up never turns it (`Tool`, `initial_rotation`). With Local
  axes the right stick turns about the object's axes, with World about
  the world's.
- **Fine movement** scales flying, pushing and turning to a fifth.
  **Thrust** triples flying while held.
- **Manipulate.** With RT held the left stick's forward and back change
  the hold distance (`Tool`, `distance`, at `tool::PUSH_RATE`), from
  where the object shows: never past a nearer wall, nor nearer than its
  reach plus `tool::NEAR_GAP` (`Extent::near`). Its sideways axis rolls;
  the right stick yaws and pitches.
- **Coordinates** is a screen opened from Properties: X, Y, Z, Yaw,
  Pitch and Roll, each with its value. A on a row opens the game's
  keyboard for a number. Left and right nudge the row by the snap in
  force, or by 0.01 world units and 1 degree with snap off; that is a
  shortcut, never the only way. The object moves as the values change.
  Closing the screen is the drop: one `Placement::Set` and one undo
  step, as a hold is. A number outside the map's bounds is refused by
  the validator's own rule, with its sentence as the HUD notice.
- **Reset orientation** (Properties) sets pitch and roll to zero and
  rounds yaw to the rotation snap.
- **Duplicate** (Properties) makes a copy of the object and holds it.
- **The gizmo** draws the three axes of the basis in force at the held
  or aimed object ([forge-firefight-ui.md](forge-firefight-ui.md),
  "Markers in the world").

## The object menu

X with nothing aimed at. Groups first, then the group's objects; a
group with nothing to offer on this map is absent. The groups are
`forge_screens::sheet::GROUPS`, in this table's order; every scenario
block's objects have a group (`Group::of_block`).

| Group | Lists | Made as | Slice |
|---|---|---|---|
| Weapons, Vehicles, Equipment | the map's item and vehicle tags | in a kit session a netgame equipment entry, which the game spawns and respawns (**E**, [forge.md](forge.md) 1.2); otherwise a scenario placement made once | built as placements; P2 |
| Scenery, Crates, Machines | the tags in the scenario's palettes (`forge::PaletteItem`) | a scenario placement | built |
| Teleporters | Sender, Receiver | netgame flags of the two teleporter types | P2 |
| Spawns | Starting point, by team | a player starting location | P2 |
| Goals | Hill, Territory and Siege site | one centre flag and its default area | MK7. Other goal placements are P2 |
| Structure | the kit's block-out pieces and barriers | generated kit tags | F8 |
| Lights | the kit's lights | a generated kit tag | P6 |
| Prefabs | the player's saved prefabs whose every tag this map holds | the pieces as placements, their labels, and a group | built |
| My imports | the player's converted models | import slots | F12 |

- A row's description gives what the object is, how many are placed and
  how many more fit: the smallest of the budget left, the block's free
  entries and the object pool (`Session::budget`, `forge.capacity`).
- The budget is a count of objects, as Halo 3's is a counter (**C**).
  A cost per item is a design option of
  [forge-features.md](forge-features.md), "Objects".
- **Goal areas.** Hill, Territory and Siege site are available for
  authoring. Placing a goal of another game type switches the monitor
  to show all game types so the goal can be aimed at. **Edit area** in
  Properties opens the draft radius or path
  editor ([gametype-markers.md](gametype-markers.md#forge)).
- **Kit sessions.** Spawns, teleporters and item entries are
  rows of scenario blocks that cannot grow in a loaded map. A session
  started from the lobby in Forge mode runs on the draft materialised
  with the `forge_kit` recipe, whose spare entries make these Kit edits
  ([forge-design.md](forge-design.md), "The kit"). A session on a map
  without the kit lists those groups with the count left at zero and
  the reason in the description.

## Properties

X on the thing aimed at or held. One list page whose rows come from its
doc's `props:` block by kind (`forge_screens::props::rows`,
[forge-map-variants-ui.md](forge-map-variants-ui.md), "The Properties
page"), so a slice adds rows and no screen. A value row
opens a picker; an action row acts and closes.

| Kind | Row | Writes | Class | Slice |
|---|---|---|---|---|
| any object | Coordinates... | position and rotation, `Placement::Set` | Live | P1 |
| any object | Reset orientation, Duplicate | as above; `Placement::Add` | Live | P1 |
| item entry | Respawn time | the entry's respawn time (`+0x0e`, **E**, [scenario-editor.md](../editor/scenario-editor.md) section 2) | Live, from the next spawn | P3 |
| item entry | Game types | the entry's game-type bytes (**S**: not decoded) | Live, from the next spawn | P3, after the decode |
| item entry | Place at start | the entry's respawn mode (**S**: its two values are not decoded) | Live, from the next round | P3, after the decode |
| goal, spawn | Team | the flag's or location's team | built | P2 |
| goal | Role | the netgame flag's type | built | P2 |
| goal | Order | the flag's identifier, for hills and territories | built | P2 |
| teleporter | Channel | the flag's identifier, through a link (`forge_doc::Forge::Link`) | Kit; built for the map's own flags | F6, P2 |
| scenery, crate, machine | Physics: Normal, Fixed | **S**: no placement field that pins a physical object is known | | after a probe |
| any thing | Label, Label number, Clear label | a label and number in the document ([rules-engine.md](rules-engine.md#forge-labels)): `Forge::Label` | none: scripts read it | built |
| object the session placed | Save as prefab | `<data>/forge/prefabs/<name>.h2prefab` | none | built |
| any object | Select, Group selection | tool state; the group op | none | built |
| group member | Group, Lock group, Ungroup | `Forge::GroupLock`, `Forge::Ungroup` | none | built |
| light | Colour, Size, Brightness | fields of the light's generated tag | Live | P6 |

- **The map's own entries.** Every row applies to an entry the base
  map has as it does to one the session added, through ops that name
  the entry by identity
  ([forge-map-variants.md](forge-map-variants.md), "The ops"). The rows
  by kind, with Game types, Spare clips, Weight and Restore original,
  and the doc the page is built from, are in
  [forge-map-variants-ui.md](forge-map-variants-ui.md), "The Properties
  page"; where the two tables differ on a row that is not built, that
  page is the design.
- **Role and Label are Reach's labels, split in two.** A part in a
  retail game type is the netgame flag's type and team, which the
  retail engines already read. A part in a scripted game type is the
  document's label, which costs no scenario bytes.
- **Physics.** Forge never tests a drop against collision, so an object
  that does not simulate (scenery, a machine) already stays where it is
  put and may intersect anything: Reach's Fixed and Phased at once
  (**C**). The row matters only for kinds that do simulate.
- The built page has Coordinates... and Heading for any object, Team for
  starting points and goals, Role and Order for goals, and Respawn time
  for item spawners. Heading is Coordinates' Yaw; the rotation step is the
  Rotation snap setting, on Tool settings (`sheet::SETTINGS`). Team, Role
  and Order open pickers; Respawn time opens the game's keyboard.

## Selection, groups and prefabs

- **Select.** Properties of a thing has **Select**, which adds it to the
  selection (and **Deselect** takes it out), and, with two or more
  selected, **Group selection**. B with nothing held clears the
  selection. The selection is tool state: it never reaches the document,
  the log or the sim, and a thing that is deleted or undone leaves it. A
  held A that selects without opening a page is design.
- **Group.** **Group selection** makes a Forge-family op with a name
  ("Group 1", the first free) and the members' endpoints
  (`Session::group`). A on a member picks the group up, and a drop, a
  typed Coordinates frame or a heading moves every member in one undo
  step (`Session::move_thing`): the group turns rigidly about its centre,
  the mean of its members, or about its first member when the op says so,
  by the yaw the held member turns. Pitch and roll are the held member's
  alone. **Ungroup** ends the op. **Lock group** sets the lock: a locked
  group is not aimed at. An object is in one group, and a group has 2 to
  256 members. Deleting a member leaves the others grouped, in the same
  undo step, and ends the group when one is left. The tool shows only the
  held member's ghost; the others move on the drop.
- **Label.** The **Label** row opens the game's keyboard on the label's
  text, which the session lowercases and checks as `forge_doc::Label`
  does (1 to 32 bytes of letters, digits, `_`, `-` and `.`); **Label
  number** types the number, empty taking it off; **Clear label** takes
  the label off. Each is one `Forge::Label` op (`Session::set_label`),
  and a thing has one label, so a later set replaces it. A label is
  document data with no scenario bytes. Deleting or removing a labelled
  thing clears its label in the same undo step, as it ends its links
  and its place in a group. The ground labels belong to the AI and
  objectives page: they are neither set nor replaced here. A script's
  declared labels are not offered as a picker, since a session does not
  know its script; the player types the name.
- **Prefab.** **Save as prefab** (an object the session placed) takes
  the object's group, else the selection when it holds the object with
  another, else the object alone, and asks for a name on the keyboard.
  It writes `<data>/forge/prefabs/<name>.h2prefab` (`forge_doc::Prefab`):
  canonical JSON of each piece's block, tag key, label, and position and
  rotation from the pivot, the first piece, as if the pivot faced yaw 0,
  read back with a bounded reader (128 KiB, 256 pieces). It holds nothing
  a document could not, and a name is 1 to 64 letters, digits, dashes and
  underscores. A name already saved is replaced and the notice says so. A
  map's own entries are not saved, since they name no tag of their own.
  Two or more pieces place as a group. A file that does not read, is not
  a regular file or holds another name than its own is named in a notice
  and left out.
- **Placing a prefab.** The object menu's **Prefabs** row lists the
  prefabs whose every tag this map's palette holds, each with its
  object count; choosing one places it with its pivot 3 world units in
  front of the monitor, facing as the monitor does
  (`Session::place_prefab`). It is one undo step: an add for each piece,
  a label for each piece that has one, and the group, whose pivot is the
  first piece. A prefab naming a tag the map lacks, or more objects than
  the budget has left, is refused by name (`Session::prefab_fits`) and
  nothing is placed or substituted.
- The browser (**Placed objects**, a row of the Forge options page)
  opens a picker over every object placed, named as the object menu names
  it and numbered when the map has more than one ("Beacon 2", in the
  order placed), listed by the object menu's groups and then by name.
  Each row's line says its group and position. A on a row flies the
  monitor to it: it stands 5 world units off, on the side it was on, and
  faces the object (`Camera::toward`), so the aim finds it. With nothing
  placed the row is a fact. The browser lists the groups first, each
  with its object count and lock; A on one flies to its centre, standing
  back by its spread. The monitor's view changes only: nothing reaches the
  document, the log or the sim.

## Interfaces

- **`forge::tool`**: `Tool`, `Action`, `Input`, `Command`. The settings
  are `movement_step`, `rotation_step` and `axes`; `HoldThing` carries a
  frame without an op, `MoveThing` drops it, and `SetField` writes a
  decoded property.
- **Agent API**: `forge.status` adds `movement_snap`, `rotation_snap` and
  `axes` beside `fine`, the selection and the tool
  ([agent-api.md](../runtime/agent-api.md), "Local Forge session"). No new
  method: input and state already carry them.
- **Document**: a document with a group op is format 4
  (`forge_doc::FORMAT_GROUPS`), since the format is one number and an
  older build refuses a newer document whole (`validate.rs`); one with no
  group op keeps its number. A prefab is a file with its own number
  (`forge_doc::PREFAB_FORMAT`, 1), and a placed prefab is the ordinary add,
  label and group ops, so a document that holds one needs no number of
  its own.
- **Agent API**: `forge.status` has `groups` (`{id, name, locked, pivot,
  members}`) and `selection`; a group is made, ended or locked through
  `forge.propose` with a `Forge::Group`, `Ungroup` or `GroupLock` op
  ([agent-api.md](../runtime/agent-api.md), "Local Forge session").
- **Agent API**: `forge.status` has `labels` (`{thing, text, number}`) and
  `prefabs` (`{name, pieces, grouped, fits, reason}`, `reason` the sentence
  that refuses it on this map). A label is set or cleared through
  `forge.propose` with a `Forge::Label` op, and a prefab's pieces are
  placed through it with their own ops; the prefab file is the player's
  and no method reads or writes it.
- **`forge::Session`**: `label_of`, `labels`, `set_label`,
  `capture_prefab`, `save_prefab`, `prefab_fits` and `place_prefab`;
  `forge::load_prefabs` lists the files.

## Rules and limits

- A setting, a selection, a gizmo and a held object's frame before the
  drop never reach the document, the log or the sim.
- A choice opens a picker and a number opens the keyboard. A sideways
  step is only ever a shortcut.
- A property row exists only where its field is decoded and its write
  is proven. A row whose field is **S** is not shown.
- A prefab or group names tags by key. It is refused by name on a map
  that lacks one; nothing is substituted.
- Every action has one binding in `BINDINGS` and is reachable from the
  agent API by name.

## Evidence

- **Code** What is built: `tool.rs` (`Action`, `Tool`, the movement and
  rotation steps, world or local axes, the pickup offset and the push),
  `session.rs` (place, move, hold, property sets, delete, link, undo,
  redo, save, load), `forge_screens/sheet.rs` (Coordinates and the
  property tables), `forge_controls.rs` (the gate and `sample`),
  `forge_controls/bindings.rs` (the table).
- **Test** `tool_tests.rs`, `link_tests.rs`, `props_tests.rs` and
  `tests.rs` in `crates/extensions/forge/src`; `every_action_has_one_binding`,
  `the_gate_follows_halo_3s_buttons`, `the_controls_page_lists_the_bindings_table`
  and the Coordinates, nudge and property tests in
  `crates/extensions/game/src/forge_screens` and `forge_glue/editing`.
  Labels and prefabs: `prefab_tests.rs` in `crates/extensions/forge/src`,
  `tests/prefab.rs` in `crates/formats/forge-doc/src`, and the label and
  prefab tests of `forge_screens/props_tests.rs` and `forge_glue/editing/tests.rs`.
- **E** The netgame equipment entry and its spawner: [forge.md](forge.md)
  1.2 and [scenario-editor.md](../editor/scenario-editor.md) section 2.
- **C** Every statement about Halo 3, Reach and Infinite:
  [notes/forge-prior-art.md](notes/forge-prior-art.md), which also marks
  what no fetched source confirms.
- **Design** The D-pad and stick-click bindings, the snap ladders, the
  split of Reach's labels, the prefab file.

## Not verified

- That a netgame equipment entry added in a kit's spare rows spawns in
  the round it is added, or only from the next.
- The game-type bytes and respawn modes of a netgame equipment entry.
- Which netgame flag types each retail engine reads, and that a flag
  added in a kit row is read without a new round.
- Any placement field or entry that holds a physical object still.
- How many dynamic lights the engine draws in a frame, and whether a
  generated light tag on a kit piece lights the level.
- That moving a group as a batch of object moves in one tick shows no
  tearing between members.
- That the left stick's click and both triggers reach the pad gate on
  every pad the host supports.

## Related

- [forge-design.md](forge-design.md): the document, the log, the kit, the slices.
- [forge-firefight-ui.md](forge-firefight-ui.md): how each screen and HUD piece is drawn.
- [forge-features.md](forge-features.md): each feature's mechanism.
- [forge-map-variants.md](forge-map-variants.md),
  [forge-map-variants-ui.md](forge-map-variants-ui.md): editing the
  map's own placements, map variants, and the readout, marks and
  screens for them.
- [forge.md](forge.md): what retail Halo 2 has.
- [notes/forge-prior-art.md](notes/forge-prior-art.md): the retail Forges, with sources.
- [rules-engine.md](rules-engine.md): labels and the graph's IR.
