# Forge map variants: editing the map's own placements, and the file that keeps the edits

Status: Partly built: slices M1 to M10, D2 to D4, V1, V2, V7 and G1 are built, and the AI ground ("AI ground") is built, with the reload of the map it builds. V3 to V6, V8 and V9 are design. The document's placement ops, its links and labels, the materialiser and the local session that this page extends are built ([forge-design.md](forge-design.md)), and so are the print, `EntryOp::{Set, Remove, Reset}`, format 3, their validator rules and `Refusal::Print` (M1), `resolve`, which lowers entry ops for `replay` (M2), the materialiser's entry ops and print check (M3), the tier, the play and session forms and the variant name rule (M4), the session's things, `Session::{things, move_thing, hold}`, its kept rows and print check, and the base calls on `World` (M5), `Session::{set_field, remove, reset}` and `forge::props` (M6), `forge_map::rows`, which reads each block's rows, typed fields and prints from the loaded scenario as the session opens (M7), `ForgeWorld`'s base calls over the call queue, `base_move`, `base_field`, `base_inert` and `base_restore` (M8), the Objects tool aiming at every thing, own or added, mark or body, by game type and `forge.show` (M9), and `forge.things`, entry ops in `forge.propose` with named refusals and the variant in `forge.status` (M10); `forge_doc::games` (D2), `forge::props::{games, games_ops}` and `Session::set_games`, the item spawner's game types, one op a changed slot (D3), and `EntryOp::Property` with the spawner's `team`, `forge_doc::teams` and the census's team for it (D4); the variant store, `forge::files::variants` (V1), and the lobby's pick and the build before Start Game, `game::map_variant` (V2); a game variant's suggested map variants, `maps` in the `.h2variant` bundle and the picker's order (V7, `variants::portable::Bundle::with_maps`, `map_variant::suggest`); `Property::Games` on scenery, crates and machines, `forge_doc::{object_games, for_game}`, `forge_build::{build_for, map_id}` and `game` in the format 2 `MapId` (G1); everything else named here as new is not.
Code: `crates/formats/forge-doc` (the document), `crates/tools/forge-build` (the materialiser), `crates/extensions/forge` (`Session`; `things.rs` for things, kept rows and the base reconcile; `tool.rs` for aiming), `crates/extensions/agent/src/forge/{local.rs, base.rs}` (`ForgeWorld`, its base calls and the agent methods), `crates/extensions/game/src/forge_map.rs`, `crates/live/live-hle/src/maps.rs` (`MapGate`). Tests: named per slice below.
Switch: `forge`, listed in [switches.md](../runtime/switches.md). No switch of its own.

Forge edits every placement a multiplayer scenario has, not only the
objects a session adds: a monitor picks up, moves, turns, deletes and
sets the fields of the map's own weapons, vehicles, scenery, crates,
machines, item spawners, starting points and goals. What a player saves
is a **map variant**: the base map named by hash and the edits, in a
file of its own, named per map. A **game variant** stays what it is, the
rules and options of a `.h2variant`
([rules-variants.md](rules-variants.md)). The custom-game lobby and a
playlist entry pick one of each, as Halo 3 does (**C**,
[forge.md](forge.md) 1.1). The rule that must hold: **an edit names the
entry it changes by identity, and a map variant that does not fit the
map in front of it is refused by name**. Nothing falls back to the
original map.

How the editor is drawn (the Properties page per kind, the world marks,
the budget, the save, load and rename screens) is
[forge-map-variants-ui.md](forge-map-variants-ui.md). The retail blocks
and fields are in [forge.md](forge.md) 1.2 and
[scenario-editor.md](../editor/scenario-editor.md) sections 1, 2 and 6.

Evidence labels are those of [docs-style.md](../meta/docs-style.md),
plus **C**: the retail games as publicly documented, and the public
Halo 2 scenario layout's field names, used as hints for where to read
and never as proof.

## How it works

### Things

A **thing** is one entry of one of the scenario blocks the editor and
Forge share (`map::scenario_layout::BlockKind`). The session lists
every thing of the loaded map, not only those its document added
(`forge::Session::things`, replacing `objects`).

| Kind | Block | In the world |
|---|---|---|
| Object | scenery, crates, machines, and the weapon, vehicle and equipment placement blocks | the live object made from the placement |
| Item spawner | netgame equipment | the object the spawner last made, when one is out (**E**, forge.md 1.2) |
| Starting point | player starting locations | nothing: a world mark |
| Goal | netgame flags of the flag, bomb, ball, hill and territory types | the engine's goal object where it makes one, else a world mark |
| Teleporter | netgame flags of the two teleporter types | a world mark; the map's own scenery beside it is a separate object |

Sound scenery and starting equipment are listed by the editor and are
not things: the first has no body (`forge_map::placeable`), the second
no decoded field worth a row (scenario-editor.md section 2).

A thing is an **own** entry (the base map has it) or an **added** one
(a `Placement::Add` op of the document made it). Every tool treats the
two alike. The only difference a player sees is **Restore original**,
which an own entry has and an added one does not.

**Aiming.** The Objects tool's reticle picks from every thing of
`Session::things` (`crates/extensions/forge/src/tool.rs`): the nearest
along the view, up to the wall the view meets, whose position the view
passes within its **pick radius**. An object's pick radius is its
palette item's bounding sphere (`Extent`, the default half unit for a
tag the Objects palette lacks); a starting point, a goal, a teleporter
and an item spawner are picked by their mark, `MARK_RADIUS` (0.5 world
units). A spawner's object, when it has one out, lies on the mark, so
the mark picks it too. Each radius widens by `RETICLE_SPREAD` (0.012) a
unit of distance, so a far thing stays as wide as the reticle. A thing
of another game type than the session's is passed over unless Show is
All game types ("Game types"). What is aimed at is `Tool::aimed_thing`,
an `Endpoint`; A picks it up, the drop is `Command::MoveThing` for an own
entry (`Session::move_thing`) and `Command::Move` for an added one, and
delete is `Command::Remove` (`Session::remove`) or `Command::Delete`.
While held, an own entry stays where the document has it and the tool's
ghost shows where it goes.

### Naming an entry

`forge_doc::Endpoint` already names an entry by identity, for links and
labels: `Base {block, index}` is the base map's entry at that index
wherever later edits move it, and `Added(OpId)` is the entry an add op
made (`crates/formats/forge-doc/src/links.rs`). Edits of fields use the
same name, with a **print** added to the base form:

```
Endpoint::Base  { block, index, print }   index in the base map's block, as the disc has it
Endpoint::Added (OpId)
```

- **The index is the base map's**, never a running index. The document
  names its base by SHA-256 (`ForgeDoc::base`), so the index means one
  entry on every machine that passes the base check.
- **The print** is the first eight bytes, as sixteen hex digits, of the
  SHA-256 of the canonical JSON of the entry's typed fields as
  `forge_doc::Value`s, in the layout's order, with its block's name:
  `{"block": <name>, "fields": [...]}` (`forge_doc::print(block,
  &[Value])`, which refuses a field list of another length or type). A
  tag reference enters as
  its `TagKey` and a palette index as the key of the palette entry it
  selects, so the print holds no salted ID. It is a digest, as a tag
  write's `was` is: no value of the map can be read back from it.
- The field list a print covers is the layout's typed fields
  (`scenario_layout::BlockDef::fields`) as of `builder.forge_build`. A
  layout that types another field takes the next builder number, which a
  machine with another number already refuses by name.
- **The check.** An adapter computes the print of the entry at
  `(block, index)` before its first write to it: the materialiser from
  the disc's scenario, the session from the loaded one as it stood when
  the session opened. A print that differs is the refusal
  `Refusal::Print {block, index}`, "the map's entry is not the one this
  edit was made on", and nothing is written. `forge_doc::check_print`
  is that check. The materialiser reads the disc's row
  (`forge_build::base_fields`) and stops the build as `variant_print`
  (`forge_build::Error::Print`) with the block and index.
- Links and labels written before a print existed carry none and keep
  their spelling and identity; one that carries a print makes the
  document format 3. Every op this page adds requires one.

The base hash already proves the whole map. The print is the second
lock: it catches a map served under the right name and hash rule but
with another scenario (a package rebuilt by another converter, an
`H2_MAP_DIR` file), a fault in index resolution, and a session whose
loaded scenario another tool has written. It is also what a later
**rebase** tool matches by, to carry a variant to another build of its
map; that tool is not designed here.

### The ops

A family beside `Placement`, `TagField` and `Forge`
(`forge_doc::Edit::Entry(EntryOp)`, in
`crates/formats/forge-doc/src/entry.rs`; the type is `EntryOp` because
`forge_doc::Entry` is the log entry). `Set`, `Remove`,
`Reset`, `Property` and `Area` are built.

| Op | Does | Carries |
|---|---|---|
| `EntryOp::Set {target, write}` | sets one typed, editable field of the entry | a `FieldWrite`: the field's layout name and a `Value` |
| `EntryOp::Remove {target}` | removes the entry | nothing |
| `EntryOp::Reset {target}` | drops the effect of every earlier entry op on an own entry: it is in the map again, as the disc has it. Links and labels are their own ops and stay | nothing |
| `EntryOp::Property {target, property}` | sets or clears a property that has no retail field ("Properties without a retail field") | the property's own value |
| `EntryOp::Area {target, area}` | sets or clears a netgame flag's [area](gametype-markers.md#in-the-document) | a radius, box, sphere or path (a sphere has no heights), or `null` |

`Placement::Add` stays the one way to make an entry, and its `OpId` is
the new entry's name.

- **Replay lowers them.** `forge_doc::resolve`
  (`crates/formats/forge-doc/src/resolve.rs`) walks the ops keeping
  each identity's running index through appends and swap-removes, as
  `ForgeDoc::links` does for final indices, and gives each entry op the
  `Placement::Set` or `Placement::Remove` of that running index
  (`Resolved::lowered`); `replay` hands those to the adapter in op
  order. `forge_doc::Apply` keeps its two required methods.
  Resets, properties and areas lower to nothing. The fold leaves out
  entry ops covered by a later reset of the same entry, as it leaves
  out a retracted op. Placement ops by running index are never covered;
  their indices are read in the document with the covered ops left
  out.
- **Replay checks the prints first.** `Resolved::prints` lists every
  base entry an entry op, a link or a label names with a print, once.
  `replay` passes them to `Apply::prints` before the first edit, and
  the adapter calls `check_print` on each against the entry as the
  base map has it. An adapter without the method refuses a document
  with a print as `Refusal::Unsupported("print")`.
- **The session writes only `EntryOp` ops and adds.** `Placement::Set`
  and `Placement::Remove` by running index stay in the format for the
  editor's scripts and `h2 forge build`, where one author writes one
  sequence. A proposal from a peer whose `base_seq` is stale can name a
  running index that has moved; an identity cannot.
- **Undo is unchanged.** An undo retracts the author's latest op and
  the fold replays the rest from the base (`forge_doc::fold_from`), so
  undoing the removal of an own entry needs no byte of it in the
  document. The map's entry is on every machine.
- **Format.** A document with an `EntryOp` op or a print has `format` 3
  (`content::forge::DOCUMENT_FORMAT_ENTRIES`).
  `ForgeDoc::required_format` is the highest number its ops need, so a
  document without them keeps its number and its identity. A group op
  needs 4 (`DOCUMENT_FORMAT_GROUPS`), and a document with a wave set is 5
  (`DOCUMENT_FORMAT_WAVES`, [forge-firefight.md](forge-firefight.md),
  "The wave set").
- **Validation without a map** (`validate.rs`, and the identity pass in
  `links.rs`): a base target carries its print; the block has the
  field and the layout marks it editable; the value's type is the
  field's; a target that an earlier op removed is refused; a removal of
  a linked or labelled entry is refused, as it is today; `Reset` names
  a base entry that an entry op has changed since its last reset; one
  entry has one print in a document. With a map, the adapter adds the
  print check (`forge_doc::check_print`), the tag behind a reference
  and the block's count and limit.
- **Undo.** `forge_doc::inverse` retracts an entry op as it does any
  other. `ForgeDoc::links` and `labels` follow entry removals, and
  leave out a removal that a later `Reset` covers.

### Applying an edit in a session

`forge::World` gains the calls an own entry needs, and `ForgeWorld`
(`crates/extensions/agent/src/forge/local.rs`) makes them on the call
queue that `world.object.*` uses, as it does for added objects. The
class column is [forge-design.md](forge-design.md), "What the engine
allows at runtime".

| Kind | Move, turn | Set a field | Remove | Class |
|---|---|---|---|---|
| Object | the live object, found by its type and placement index (object `+0x1a`, **E**, scenario-editor.md section 6), through the `world.object.set` body; and the placement's fields in the loaded scenario, so a new round makes it there | the loaded scenario's field; it shows when the object is next made (**Code**, [hot-iteration.md](../graphics/hot-iteration.md), "What shows when") | the object is deleted and its row made inert | Live |
| Item spawner | the entry's position and orientation in the loaded scenario; the object it has out is deleted, and the spawner makes the next one there when it is due (**E**: the per-tick update walks the block, forge.md 1.2) | the loaded scenario's field, from the next spawn | the row made inert and its object deleted | Live for an own entry; Kit for an added one |
| Starting point | the loaded scenario's fields | the same | the row made inert | Live; Kit to add |
| Goal, teleporter | the loaded scenario's fields | the same | the row made inert | Live, from the next round (**S**: netgame flag results are cached per game, scenario-editor.md section 6); Kit to add |

- **A removal in a session is an inert row, not a swap-remove.** A
  loaded block cannot be reordered under a running game: objects keep
  their placement index, the spawner's state array is by entry, and
  peers send placement indices (scenario-editor.md section 6). The row
  takes the form the kit gives its spare rows
  (`crates/tools/forge-build/src/kit.rs`, `Spare`), which F3's kit
  probe shows the readers skip. The materialiser, which builds a file,
  swap-removes as the editor does and repoints the object names. The
  two agree because every op names an identity and each adapter keeps
  its own running indices.
- **The session keeps the map's rows.** `forge_map` reads each block of
  the loaded scenario once, when the session opens, into the session's
  own memory (`forge_map::rows::read`, handed to
  `Session::with_base` as `forge::BaseRows`): the typed fields for the
  things list and the prints, and the raw rows so an undo or a `Reset`
  can put one back exactly. A block that does not read, or whose rows
  are not the map's count, is left out by name (`Session::left_out`):
  its entries are not things and an edit naming one is refused. None
  of it reaches the document, the log, a save or the network.
- **The world follows the document.** After every fold the session
  works out each own entry's state from the document's entry ops and
  makes the base calls that take the world there, per entry: one
  `base_inert` for a removal, one `base_restore` of the kept row for a
  return, then one `base_move` for a changed position or turn and one
  `base_field` for each other changed field
  (`crates/extensions/forge/src/things.rs`, `reconcile`). A refused
  call is compensated from where it stopped, as for placed objects.
- **One path.** `Session::set_field`, `move_thing`, `remove` and
  `reset` make ops and pass them to `Session::edit`, which stamps,
  folds and reconciles as it does for a placed object. The pad, the
  keyboard, `forge.propose`, a peer's proposal and a film's entry all
  arrive there. A refusal leaves the document, the log and the world
  as they were.
- **Holding** an own entry is the same hold: moves are sent at most
  once a tick and the drop is one `EntryOp::Set` for position and one for
  rotation, in one undo step. `Session::hold` moves the thing in the
  world only, `release` puts it back where the document has it, and
  `move_thing` is the drop. An added entry's drop names it by its add
  (`Endpoint::Added`), as an own entry's names it by index and print.

### Fields by kind

A row of Properties exists only where its field is decoded and its
write is proven ([forge-editing.md](forge-editing.md), "Rules and
limits"). This is every field the design gives a row, with what stands
between it and a row.

| Kind | Field | Home | Evidence | Needs |
|---|---|---|---|---|
| any | position, rotation or facing | the block's fields | **E**, **V** (scenario-editor.md section 2) | nothing |
| Object | object: the palette entry | palette index | **V**, **C** | nothing |
| Item spawner | item: a collection or an object tag | `item` | **E** | nothing |
| Item spawner | respawn time; 0 is the collection's own | `respawn time` | **E** | nothing |
| Item spawner | return time: how long an object may be away from its spot | `abandon limit` | **E** | nothing |
| Item spawner | place at start | none: the reset `0x15ad30` spawns every matching entry at the start, whatever its mode (**E**, [2026-11-27-forge-spawner-rows.md](../engine/notes/2026-11-27-forge-spawner-rows.md)) | **E** (scenario-editor.md section 2) | the property `place_at_start` ("Properties without a retail field") |
| Item spawner | respawn trigger: what starts the countdown while the object is out | `respawn mode`: 0 the object moved from its spot or an object bit set, 1 another object bit set; the bits are not named (where each is written is in [notes/2026-11-26-forge-loose-ends.md](notes/2026-11-26-forge-loose-ends.md)) | **E** for the paths; **S** for the bits' meaning | a row in D3 once the bits are named |
| Item spawner | game types | `game type 1` to `game type 4` (`+0x04`), read by the matcher `0x15a090` | **E** (scenario-editor.md section 2, `map::scenario_layout::game_types`) | nothing |
| Item spawner | spare clips, team | none in the entry (**C**: the public layout has neither) | **S** | Forge properties, both built (D4, D5) |
| Starting point | team | `team` | **V**, **C** | nothing |
| Starting point | game types | none read; the public layout's slots after the team (**C**) are kept, not typed | **E**: no reader of the block found reads those bytes, and the spawn chooser `0x14ef00` weighs every entry (scenario-editor.md section 2); that no other code reads them is **S** | left out |
| Starting point | weight | none in the entry (**C**); the engine's weight is its position's, with a random term, so a property is a bias added to it | **E** for the routine (notes/2026-11-26-forge-loose-ends.md) | a Forge property, built (D5) |
| Goal | role: the netgame flag's type | `type` | **V**, **C**; the roles traced are in `forge_doc::builtin_label` | nothing |
| Goal | team | `team` | **V**, **C** | nothing |
| Goal | order: the hill corner or territory number | the type for a hill corner, `identifier` otherwise | **V** for the identifier; **Code** for the corner (`builtin_label`) | nothing |
| Teleporter | channel | `identifier`, through a link | **V**; built for the map's own flags and the flags a kit session adds (F6) | nothing |

`map::scenario_layout` is the one table of field names, offsets and
types; a field joins it with its evidence, and scenario-editor.md
section 2 is where the page for it is.

`forge::props` (`crates/extensions/forge/src/props.rs`) is the rows
built: position and rotation (the block's rotation or facing) on every
kind, object, item, respawn time, return time, team, role and order,
each with the kinds it is on, how it reads a thing and the one
`EntryOp::Set` it writes. The game types of an item spawner are not a
row of that table, which is one field a row: `props::games` reads the
four slots as a set and `Session::set_games` writes the slots that
change in one undo step. The rows a Forge property backs are Team (D4), Spare clips, Weight and Place at start (D5). A teleporter's channel is its link's
(the Links tool), not a row. `Session::set_field` takes a row's name
and refuses one the thing's kind does not have.

### Properties without a retail field

Spare clips, place at start, a spawner's team and a starting point's
weight are in Halo 3's variants (**C**, forge.md 1.1) and have no field in Halo 2's
entries (**C**, **S**). They are properties of the document
(`EntryOp::Property`), in the labels' manner: document data with no
scenario bytes (`forge_doc::Labels`).

| Property | On | Reaches the game by | State |
|---|---|---|---|
| `spare_clips` | item spawner whose collection holds a weapon | a write to the new weapon's reserve after the spawner makes it: an observer after `NETGAME_ITEM_SPAWN` (`0x15aad0`) reads the entry's index from the popped arguments and its weapon from the spawner's state (`[GAME_ENGINE]+0x7dc`, 8 bytes an entry), and sets the first magazine's reserve (`+0x22a`) to the clips times the definition magazine's clip (`+0x0a`), capped at its `+0x0c` (**E**, [2026-11-27-forge-spawner-rows.md](../engine/notes/2026-11-27-forge-spawner-rows.md)). The clips are host state by entry (`agent::forge::spare_clips::TABLE`), set by the session's `World::spare_clips` (`ForgeStep::SpareClips`, which also fills the weapon the entry has out) and emptied at `GAME_ACTIVATE` (`game::forge_spare_clips`). A match on a built variant reads them from the Forge record | `Session::{spare_clips, set_spare_clips}`; the Properties row Spare clips |
| `team` | item spawner | nothing in a retail game type. A rules script reads it, as it reads a label ([rules-engine.md](rules-engine.md#forge-labels)) | `Session::{team, set_team}`; `forge_doc::labelled` reports it for a labelled spawner |
| `weight` | starting point | a bias added to the weight the chooser (`PLAYER_SPAWN_LOCATION`) gives the location. The weight is the position's sum of influence spheres plus a random term (`0x23ba90`, its one caller `0x14efb1`, EAX the location, XMM0 the weight; `0x23b060`; **E**, notes/2026-11-26-forge-loose-ends.md); a hook after `0x23ba90` (`SPAWN_LOCATION_WEIGHT`) adds -0.5 for Low and +0.5 for High. The random term is uniform over 0 to 1 (the multiplayer globals' scale is 1.0) and an enemy's sphere weighs -0.75 to -1, so High wins a tie of influence seven times in eight and still loses to a location clear of enemies (**E**, [2026-11-27-forge-spawner-rows.md](../engine/notes/2026-11-27-forge-spawner-rows.md)). The weights are host state by location (`agent::forge::spawn_weight::TABLE`), set by `World::spawn_weight` and emptied at `GAME_ACTIVATE` (`game::forge_spawn_weight`). A match on a built variant reads them from the Forge record | `Session::{spawn_weight, set_spawn_weight}`; the Properties row Weight |
| `place_at_start` | item spawner; `false` is No, none the retail Yes, `true` is refused (a clear) | a write after the round reset: an observer after `ENGINE_ROUND_RESET` sets the countdown (`+4`) of each held entry the reset left with no object and a countdown of 1 to 0, so the spawner arms the respawn time and the item first comes after it (**E**, [2026-11-27-forge-spawner-rows.md](../engine/notes/2026-11-27-forge-spawner-rows.md), "Place at start"). The held entries are host state (`agent::forge::place_at_start::TABLE`), set by `World::place_at_start` (`ForgeStep::PlaceAtStart`) or the Forge record and emptied at `GAME_ACTIVATE` (`game::forge_place_at_start`). In a session a change takes effect at the next round's start | `Session::{place_at_start, set_place_at_start}`; the Properties row Place at start |
| `games` | object | the materialiser leaves the placement out of a map built for another game type ("Game types") | `Property::Games`, `forge_doc::object_games`; no session call or tool row sets it |

A property that reaches the game through a hook is host state in
`ForgeExt` and follows [forge-design.md](forge-design.md), "Who owns
state".

### The Forge record

A match on a built variant has no document. The build writes what the
hooks above need into a record at the map's tail, before the areas
record (`forge_doc::record::ForgeRecord`, written in `forge_build::build`).
At each `GAME_ACTIVATE`, before the call and after the three features
empty their tables, `game::forge_record` reads the active map's record
and fills the tables. The first round's reset runs inside
`GAME_ACTIVATE`, so place at start holds from the first round.

```text
record  = "H2FR" version:u8 count:u16le row*count crc:u32le
row     = field:u8 index:u16le value:u8
trailer = length:u32le "H2FE"
```

| Field | Index | Value | Table filled |
|---|---|---|---|
| 0 spare clips | the item spawner's row in the built map | clips, 0 to 10 | `agent::forge::spare_clips::TABLE` |
| 1 weight | the starting point's row | 0 Low, 1 High | `agent::forge::spawn_weight::TABLE` |
| 2 held back | the item spawner's row | 0 | `agent::forge::place_at_start::TABLE` |
| 3 body refused | the Forge body's code (0 Monitor, 1 Sentinel, 2 Prophet) | why the kit left it out: 0 no donor, 1 cannot copy, 2 no room | `forge_map::kit_bodies` ([forge-design-slices.md](forge-design-slices.md), "The monitor body") |

`crc` is the FNV-1a of the bytes before it. Rows name entries by their
index in the built map, which `ForgeRecord::of` resolves from the
document (`forge_doc::resolve`); a removed entry has no row. A kit
build writes a field 3 row for each body it left out. A build with no
such property and no refused body writes no record.

A reader walks the tail's
records from the end (`game_markers::record::{tail_record, TAIL_ENDS}`),
so the areas record and the Forge record are found in either order, and
Studio strips both when it opens a map. A record that does not read is
named and none is applied.

## Where the sections live

| Page | Covers |
|---|---|
| [forge-map-variants-games-ai.md](forge-map-variants-games-ai.md) | game types and the AI ground |
| [forge-map-variants-files.md](forge-map-variants-files.md) | the file, packages, picks, the lobby, materialising and Live |
| [forge-map-variants-status.md](forge-map-variants-status.md) | slices, open decisions, evidence and what is not verified |

Each heading below is cited from code and moved to the page named after it.

### Game types

Moved to [forge-map-variants-games-ai.md](forge-map-variants-games-ai.md).

### AI ground

Moved to [forge-map-variants-games-ai.md](forge-map-variants-games-ai.md).

### The map variant file

Moved to [forge-map-variants-files.md](forge-map-variants-files.md).

### A map variant in a package

Moved to [forge-map-variants-files.md](forge-map-variants-files.md).

### How a pick names a map variant

Moved to [forge-map-variants-files.md](forge-map-variants-files.md).

### The lobby

Moved to [forge-map-variants-files.md](forge-map-variants-files.md).

### Materialising for a match

Moved to [forge-map-variants-files.md](forge-map-variants-files.md).

### Live

Moved to [forge-map-variants-files.md](forge-map-variants-files.md).

## Interfaces

- **`forge-doc`**: `Endpoint::Base.print`, `print`, `check_print`, `Edit::Entry`,
  `EntryOp`, `Property`, `teams`, `object_games`, `for_game`,
  `OBJECT_BLOCKS`, `resolve`, `Resolved`, `Printed`,
  `Apply::prints`, `Refusal::Print`, `games::{GameSet,
  GamesError, ENGINES, encode, decode, flag_games}`, `Tier`, `ForgeDoc::{tier, check_variant}`,
  `Refusal::Tier`, `play_form`, `session_form`, `KIT_RECIPE`,
  `variant_name`, `GroundLabel`, `GroundRecipe`, `GROUND_RECIPE`,
  `ForgeDoc::{ground_recipe, with_ground, ground_marks, ground_stamp}`.
- **`forge`**: `Thing {id: Endpoint, block, kind, own, frame, fields,
  changed}`, `ThingKind`, `BaseRows`,
  `Session::{with_base, left_out, things, thing, move_thing, hold,
  release, set_field, remove, reset}`, and on `World`: `base_move`,
  `base_field`, `base_inert`, `base_restore`. The tool's
  `Session::{place_marker, place_marker_next, move_marker, remove_marker,
  markers, ground_state, build_ground, ensure_ground, save_building}`,
  `GroundBuilder`,
  `Tool::{aimed_thing, held_thing, game, show}`, `Show`, `in_game`,
  `MARK_RADIUS` and `Command::{MoveThing, Remove}`.
  `forge::props` is the table of properties by name: the kinds
  each is on, how it reads a thing and the op it writes. `forge::files::variants`
  is the variant store.
- **Agent API** (game), in [agent-api.md](../runtime/agent-api.md),
  "Local Forge session". `forge.things {kind?, changed?}` (read) lists
  the things with their endpoints, fields and prints: every test and
  lane that edits the map's entries needs to name one, and
  `forge.status` holds only the aimed and held one. `forge.propose`
  takes `EntryOp` ops, and a refused edit carries its name
  (`forge_print`, `forge_was`, `forge_left_out`, ...) in the error's
  `data.code`. `forge.status` reports the variant's name, id, tier and
  whether it has unsaved edits (`variant`), and the ground in brief
  (`ground`). `forge.ground` (read) and `forge.ground.build` (act) give
  the ground's state, markers, problems and receipt, and build it
  (`crates/extensions/agent/src/forge/ground.rs`). `forge.variants {map?}`
  (read, new) and `forge.variant.pick {map_id?}` (act, new) list the
  store and set the lobby's pick, for lobby and start-gate checks; they
  are V1 and V2's.
- **CLI**: `h2 forge check` and `build` read a `.h2mapvariant` as they
  read a `.h2forge`, and print each print they compared
  (`print <block> <index> <print>: matches`) and the tier
  (`Tier variant` or `Tier map`).
- **Live**: the two `map-variants` routes; `map_variant` on a hopper
  record; `HopperVariantsSet`; in [live-api.md](../live/live-api.md)
  when built. `NewSession` is unchanged.
- **Editor**: the playlist builder's entry gains a map variant picker
  beside its game variant picker
  ([editor.md](../editor/editor.md)); the scenario tool opens a
  `.h2mapvariant` as a list of commands on its map.

## Rules and limits

Every refusal is named, shown where the player is, and stops before a
map loads. None is answered by loading the original map.

| Name | When | Says |
|---|---|---|
| `variant_base_missing` | this machine has no map of the base's name | "<name> needs <map>, which is not installed." |
| `variant_base_hash` | the installed map's SHA-256 is not the base's | "<name> was made for another version of <map>." |
| `variant_builder` | `builder` differs from this build's | "<name> was made with another version of Forge." with both versions in the log |
| `variant_format` | `format` is one this build does not read | the same sentence |
| `variant_print` | an entry's print differs | "<name> does not match this map (<block> <index>)." |
| `variant_unreadable` | the file does not parse, or is in another map's directory | "<name> cannot be read." |
| `variant_tier` | a file offered as a variant holds more than one may | "<name> is a Forge map, not a map variant." |
| `variant_expect` | the built map's id is not `expect.map_id` (`forge_expect_map_id`) | "<name> did not build to the map its author saved." |
| `variant_missing` | the document is on neither this machine nor Live, or is hidden or blocked | "<name> is not available." |
| `variant_game` | the start gate: a goal or a label the game variant needs is absent | the gate's sentence, naming what is missing |
| `variant_build` | the build failed for a reason the rows above do not name; the log has the error | "<name> could not be built." |
| `variant_member` | a member failed any of the above | "<gamertag>: " and that member's sentence |

- A base map is named by hash. A name match with another hash is
  `variant_base_hash`, never a near enough.
- An edit names an identity, and a print guards every own entry it
  touches.
- A session removal is an inert row; a built map has no such row.
- A variant holds scenario edits only. The tier is computed, never
  declared.
- No map variant in a ranked session unless an admin published it on
  that playlist's entry.
- A property row is absent until its field is decoded and its write
  proven. Nothing is written to a byte the layout does not type.
- Theater never edits; a film names the document's hash and
  materialises first ([forge-design.md](forge-design.md), "Films and
  determinism").

## Slices

Moved to [forge-map-variants-status.md](forge-map-variants-status.md).

## Open decisions

Moved to [forge-map-variants-status.md](forge-map-variants-status.md).

## Evidence

Moved to [forge-map-variants-status.md](forge-map-variants-status.md).

## Not verified

Moved to [forge-map-variants-status.md](forge-map-variants-status.md).

## Related

- [forge-map-variants-ui.md](forge-map-variants-ui.md): the HUD, the
  world marks and the screens.
- [forge-design.md](forge-design.md): the document, the log, the kit,
  materialising, Live.
- [forge-editing.md](forge-editing.md): the controls, the tools, the
  object menu and Properties.
- [forge.md](forge.md), [scenario-editor.md](../editor/scenario-editor.md):
  the blocks and fields.
- [rules-variants.md](rules-variants.md), [variants.md](../editor/variants.md):
  the game variant.
- [rules-start-gate.md](rules-start-gate.md): what a game variant
  requires of a map.
- [maps.md](maps.md), [map-select.md](map-select.md),
  [party-lobby.md](party-lobby.md): packages, `MapId`, Change Map and
  the lobby.
- [live-admin.md](../live/live-admin.md): the playlist builder.
