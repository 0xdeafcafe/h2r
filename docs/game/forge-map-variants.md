# Forge map variants: editing the map's own placements, and the file that keeps the edits

Status: Design, slices M1 to M10 built. The document's placement ops, its links and labels, the materialiser and the local session that this page extends are built ([forge-design.md](forge-design.md)), and so are the print, `EntryOp::{Set, Remove, Reset}`, format 3, their validator rules and `Refusal::Print` (M1), `resolve`, which lowers entry ops for `replay` (M2), the materialiser's entry ops and print check (M3), the tier, the play and session forms and the variant name rule (M4), the session's things, `Session::{things, move_thing, hold}`, its kept rows and print check, and the base calls on `World` (M5), `Session::{set_field, remove, reset}` and `forge::props` (M6), `forge_map::rows`, which reads each block's rows, typed fields and prints from the loaded scenario as the session opens (M7), `ForgeWorld`'s base calls over the call queue, `base_move`, `base_field`, `base_inert` and `base_restore` (M8), the Objects tool aiming at every thing, own or added, mark or body, by game type and `forge.show` (M9), and `forge.things`, entry ops in `forge.propose` with named refusals and the variant in `forge.status` (M10); everything else named here as new is not.
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
`forge_doc::Entry` is the log entry). `Set`, `Remove` and `Reset` are
built; `Property` is slice D4's:

| Op | Does | Carries |
|---|---|---|
| `EntryOp::Set {target, write}` | sets one typed, editable field of the entry | a `FieldWrite`: the field's layout name and a `Value` |
| `EntryOp::Remove {target}` | removes the entry | nothing |
| `EntryOp::Reset {target}` | drops the effect of every earlier entry op on an own entry: it is in the map again, as the disc has it. Links and labels are their own ops and stay | nothing |
| `EntryOp::Property {target, property}` | sets or clears a property that has no retail field ("Properties without a retail field") | the property's own value |

`Placement::Add` stays the one way to make an entry, and its `OpId` is
the new entry's name.

- **Replay lowers them.** `forge_doc::resolve`
  (`crates/formats/forge-doc/src/resolve.rs`) walks the ops keeping
  each identity's running index through appends and swap-removes, as
  `ForgeDoc::links` does for final indices, and gives each entry op the
  `Placement::Set` or `Placement::Remove` of that running index
  (`Resolved::lowered`); `replay` hands those to the adapter in op
  order. `forge_doc::Apply` keeps its two required methods.
  `EntryOp::Reset` lowers to nothing, and so does every entry op a
  later reset of the same entry covers, as the fold leaves out a
  retracted op. Placement ops by running index are never covered;
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
  document without them keeps its number and its identity.
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
| Item spawner | place at start | none: the reset `0x15ad30` spawns every matching entry at the start, whatever its mode | **E** (scenario-editor.md section 2) | left out |
| Item spawner | respawn trigger: what starts the countdown while the object is out | `respawn mode`: 0 the object moved from its spot or an object bit set, 1 another object bit set; the bits are not named | **E** for the paths; **S** for the bits' meaning | a row in D3 once the bits are named |
| Item spawner | game types | `game type 1` to `game type 4` (`+0x04`), read by the matcher `0x15a090` | **E** (scenario-editor.md section 2, `map::scenario_layout::game_types`) | nothing |
| Item spawner | spare clips, team | none in the entry (**C**: the public layout has neither) | **S** | a Forge property; slice D4 |
| Starting point | team | `team` | **V**, **C** | nothing |
| Starting point | game types | none read; the public layout's slots after the team (**C**) are kept, not typed | **E**: no reader of the block found reads those bytes, and the spawn chooser `0x14ef00` weighs every entry (scenario-editor.md section 2); that no other code reads them is **S** | left out |
| Starting point | weight | none in the entry (**C**) | **S** | a Forge property; slice D4 |
| Goal | role: the netgame flag's type | `type` | **V**, **C**; the roles traced are in `forge_doc::builtin_label` | nothing |
| Goal | team | `team` | **V**, **C** | nothing |
| Goal | order: the hill corner or territory number | the type for a hill corner, `identifier` otherwise | **V** for the identifier; **Code** for the corner (`builtin_label`) | nothing |
| Teleporter | channel | `identifier`, through a link | **V**; built for the map's own flags (F6) | nothing |

`map::scenario_layout` is the one table of field names, offsets and
types; a field joins it with its evidence, and scenario-editor.md
section 2 is where the page for it is.

`forge::props` (`crates/extensions/forge/src/props.rs`) is the rows
built: position and rotation (the block's rotation or facing) on every
kind, object, item, respawn time, return time, team, role and order,
each with the kinds it is on, how it reads a thing and the one
`EntryOp::Set` it writes. Game types joins it in D3, and the rows a
Forge property backs in D4 and D5. A teleporter's channel is its link's
(the Links tool), not a row. `Session::set_field` takes a row's name
and refuses one the thing's kind does not have.

### Properties without a retail field

Spare clips, a spawner's team and a starting point's weight are in
Halo 3's variants (**C**, forge.md 1.1) and have no field in Halo 2's
entries (**C**, **S**). They are properties of the document
(`EntryOp::Property`), in the labels' manner: document data with no
scenario bytes (`forge_doc::Labels`).

| Property | On | Reaches the game by | State |
|---|---|---|---|
| `spare_clips` | item spawner of a weapon | a write to the new weapon's reserve after the spawner makes it. The spawner returns the object's ID and stores the entry's index on it (**E**, forge.md 1.2); the reserve's field on a weapon object is not decoded | not shown until slice D5 proves the write |
| `team` | item spawner | nothing in a retail game type. A rules script reads it, as it reads a label ([rules-engine.md](rules-engine.md#forge-labels)) | shown with D4 |
| `weight` | starting point | an answer to the spawn chooser (`PLAYER_SPAWN_LOCATION`, whose free-spot test `spawn_areas` answers, [rules-engine.md](rules-engine.md)). How the routine weighs locations is not traced | not shown until slice D5 |
| `games` | object | the materialiser leaves the placement out of a map built for another game type ("Game types") | slice G1 |

A property that reaches the game through a hook is host state in
`ForgeExt` and follows [forge-design.md](forge-design.md), "Who owns
state".

### Game types

Which things a game type has is the **Game types** field: a set of
engines, or all of them.

| Kind | Where the set lives | Who filters in a match |
|---|---|---|
| Item spawner | the entry's game-type slots | the engine, as retail: the tick skips an entry the matcher `0x15a090` does not match (**E**) |
| Starting point | nowhere: no retail reader of a game type was found (**E**, **S**, as in "Fields by kind") | not filtered |
| Goal, teleporter | the flag's type: a flag is its game type's by what it is | the engine: each game engine walks the types it reads |
| Object | the `games` property | the materialiser, at build time |

- **The set and the slots.** An item spawner has four slots, each an
  engine's code or a group value: 12 every engine, 13 and 14 every
  engine but CTF and Assault, which match each other's code; King of
  the Hill's code is 0 (**E**, `map::scenario_layout::game_types`).
  `forge_doc::games::encode(set)` (new) gives the slots for a set: a group value when one equals the
  set, else one slot an engine. A set that needs more than four slots
  is refused with "An item can name four game types at most."
  `decode(slots)` gives the set back, and an entry whose slots hold a
  value the table does not know is shown as it is and not edited.
- **In a session.** The session runs the lobby's game type
  ([forge-firefight-ui.md](forge-firefight-ui.md), "The lobbies"), so
  the engine spawns that game type's items and reads its goals, and
  the Goals group offers its goals. A tool setting, **Show** (`This
  game type`, `All game types`; `forge.show`, default the first),
  decides what else is drawn: with All, a thing of another game type
  has its world mark in the `Inactive` style and its game types in its
  label, and can be aimed at and edited. Its field writes are Live in
  the loaded scenario like any other; nothing spawns from it until a
  game of its type.
- **Which game type a thing is in**, for aiming (`forge::tool::in_game`,
  given the variant record's engine index, `+0x44`, which the host reads
  each tick): an item spawner when one of its slots matches the engine
  by the engine's own matcher (`game_types::slot_matches`; King of the
  Hill's index 4 is its code 0); a goal by its flag type, CTF's flag
  spawn and return (0, 1), Assault's bomb spawn and return (2, 3),
  Oddball's ball spawn (4), a territory (10) and a hill corner (11 to
  18), as forge-doc's `builtin_label` traces them (**S**: which engines
  read types 0 to 3 beyond their own); every other thing, and a flag of
  an untraced type, in every game type. With no engine index read,
  every thing is the session's. The Tool settings screen's Show row is
  not built: the tool's `Show` is `This game type` until it is.
- **A new thing's set.** A goal's is its role's. An item spawner or a
  starting point made in a session starts as all game types, as the
  map's own mostly are (**S**: the ignored test
  `disc_item_game_types_and_respawn_modes_census` prints how the retail
  maps use the slots).
- **Objects.** A scenery, crate or machine placement has no game-type
  field (**C**). With a `games` property the materialiser builds the
  map for the session's engine and leaves out the placements whose set
  lacks it (`forge_build`, slice G1). A document with such a property
  has the engine in its package's identity: `MapId` format 2 gains a
  `game` member, present only then, so every other document keeps its
  id ([maps.md](maps.md), "Identity: `MapId`"). Each machine knows the
  engine from the session's variant, so each builds the same file.
- **The start gate.** A map variant that lacks what the game variant
  needs blocks the start by name: a script's labels through
  [rules-start-gate.md](rules-start-gate.md), and a retail engine's
  goals through the same census (`forge_doc::labelled`, which reads
  the document's flags with the map's).

### The map variant file

A map variant is a Forge document, in the form a game is played from.

- **Wire.** The bytes are `ForgeDoc::canonical_json`: canonical JSON,
  floats as their bit patterns, at most 8 MiB
  ([forge-design.md](forge-design.md), "The document"). There is no
  second format and no envelope: the name is the document's `title`,
  the author its `author`, the base its `base {map, sha256}`.
- **What it may hold** (`ForgeDoc::tier() -> Tier`,
  `Tier::Variant` or `Tier::Map`, in
  `crates/formats/forge-doc/src/variant.rs`). A variant holds
  `Placement::Add`, `EntryOp`, link, unlink and label ops, and nothing
  else: no recipe but the kit's, no import slot, no source map, no
  placement set or removal by running index, no tag-field, mesh or
  bitmap op. So a variant
  changes the scenario's placement blocks and no tag, its build is a
  scenario patch, and it fits every map the kit fits. A document with
  more is a **Forge map**: it is picked as a map
  ([forge-design.md](forge-design.md), slice F14), keeps `.h2forge`,
  and is never offered as a variant.
- **Play form and session form.** A session's document has the
  `forge_kit` recipe (`forge_doc::KIT_RECIPE`) and a saved variant does
  not ([forge-design.md](forge-design.md), "The kit").
  `forge_doc::play_form(&doc)` drops it and `session_form(&doc)`
  appends it after any other recipe, so the play form of the session
  form is the document. A form that changes the recipes changes the
  identity, so it drops `expect`. Save writes the play form and Load
  opens the session form at a fresh log head. The tier is judged on
  the play form.
- **The name** is 1 to 16 characters (`forge_doc::MAX_VARIANT_NAME`),
  the length of the game's own variant names, so it fits every retail
  text that shows one; it is not blank and has no control character
  (`forge_doc::variant_name`). `ForgeDoc::check_variant` holds a
  document to the variant tier (`Refusal::Tier`) and its title to
  that rule. Two variants of one base map may not share a name,
  compared without case: the store's check.
- **Identity.** A variant's id is the document's
  (`ForgeDoc::id`). The map it makes has the `MapId` of format 2, the
  hash of the base's hash and the document's
  ([maps.md](maps.md), "Identity: `MapId`"). That one value names "this
  variant of this map" everywhere a map is named.
- **On disk.** `<data>/map-variants/<base map>/<file>.h2mapvariant`,
  one directory a base map. `<file>` is the name made safe for a file
  system; the name of record is the `title` inside. `live/<id>` under
  the same directory holds the ones fetched from Live, as the
  game-variant store keeps its Live copies
  (`crates/extensions/game/src/game_variants/store.rs`).
  `forge::files` owns the store: `list(map)`, `save`, `load`,
  `rename`, `duplicate`, `delete`. Files are user output and stay out
  of Git checkouts.
- **No game bytes.** The document rule holds unchanged: typed values an
  author set, tag keys, digests. A print and a `was` are digests. An
  own entry's position is never written unless the author moved it,
  and then the value is the author's.

### How a pick names a map variant

A pick is a map and, optionally, a variant of it: the map as the game
names it, and the format 2 `MapId`.

| Who picks | Holds the variant as | Where |
|---|---|---|
| The custom-game lobby | `lobby_start::map_variant() -> Option<MapId>`, beside the lobby's map | `crates/extensions/game/src/lobby_start.rs` |
| A Live session | `NewSession.map_id`, which exists; `map` stays the base map's name | `crates/live/live-proto/src/session.rs` |
| A playlist entry | `map_variant: <MapId>` on a record of the hopper JSON. The retail record has no room for a hash, so the server keeps it in a table by hopper and record position, re-keyed by the patch's `from` as the other bytes are ([live-admin.md](../live/live-admin.md), "`PUT /v1/titles/{title}/hopper`"), published as `HopperVariantsSet` and served with the pins | `crates/live/live-server/src/admin.rs` |
| A game variant | `maps: [{base, variant, name}]` in the `.h2variant` bundle: map variants its author made it for. The `0x130` record is untouched | `variants::portable::Bundle` |

- **A playlist entry is a pair.** The record's level id must be the
  variant's base map, and the server checks it at publish, with the
  document published, not hidden and of the variant tier. Publishing
  an entry with a variant pins that `MapId` on its hopper, which is
  what lets a ranked session take it
  ([maps.md](maps.md) section 5). Two records of one hopper with the
  same level id and the same variant record must name the same map
  variant, so the host can tell which record matchmaking drew from the
  map and variant it was handed (**S**: the drawn record's index is
  not read at any named point).
- **A game variant only suggests.** Picking a game variant never
  changes the lobby's map or map variant. When the lobby's map has a
  suggested variant on this machine, the Map Variant picker opens on
  it and its description says which game variant it was made for. What
  a game variant *requires* of a map is its script's labels, which the
  start gate enforces.

### The lobby

Game Setup has **Map Variant...** under **Change Map...** in the
CUSTOM GAMES and FORGE modes (`lobby_mode::arrange`); in FORGE it takes
the place of Forge Map....

1. **Change Map...** picks the map as today
   ([map-select.md](map-select.md)). A new map clears the variant.
2. **Map Variant...** opens a picker dialog on top of Game Setup
   (`kit.picker.map-variant`, in place of `kit.picker.forge-map`):
   **Original**, then this map's variants by name. The description
   gives the author, the counts by kind and the game types it has
   goals for. A variant that cannot be used is listed with its reason
   as its description and cannot be chosen.
3. The lobby's panel shows the variant's name on the line under the
   map's (`lobby_mode::panel_lines`).
4. In FORGE the pick is what the session opens: Original is a new
   variant of the map.
5. **Start Game** materialises first when the package is not cached
   and is refused, with the reason as the lobby's status line
   (`lobby_status.rs`), when any check of "Rules and limits" fails for
   the host or a member.

The row is absent when the `forge` feature is off. Only the party
leader picks, as for the map.

### Materialising for a match

Every machine builds the map from its own disc; no built byte travels
([forge-design.md](forge-design.md), "Materialising").

- **The host** builds when the leader picks, off the game thread, into
  `<data>/maps/forge-<first 16 hex of the id>/`, and reuses a package
  whose receipt matches.
- **A member** learns the pick from the session's `map_id`.
  `live_hle::maps::MapGate` treats a format 2 id as a package to make
  ready: get the document, check it, build it, compare with `expect`.
  A member is ready when its package resolves.
- **Getting the document.** From this machine's store by id; else from
  Live's content store by hash. A host in a Live lobby whose pick is
  not on Live uploads the document unlisted when it picks (a blob with
  no `ForgePublished` event, so it is in no list). In a System Link
  game without Live a member must have the file; a member who does not
  is named in the host's refusal. The Forge session channel of slice
  F5 carries a document to a joiner and replaces that refusal.
- **At load.** The lobby's map stays the base's level id. The session
  selects the package for that map's path
  (`content::Library::resolve_package`, which Forge packages need an
  explicit selection for), so the game loads the built file where it
  would load the disc's. The selection ends with the session.
- **A dedicated host** does the same from its host data, and refuses a
  session whose variant it cannot build before it accepts players
  ([dedicated-host.md](../live/dedicated-host.md)).

### Live

- **Store.** The document is a content-store blob by SHA-256, parsed by
  `forge-doc`'s validator, announced by `ForgePublished`
  ([forge-design.md](forge-design.md), "Saving and sharing on Live").
  The event's projection keeps the tier, the base's map name and hash,
  the name, the author and the counts by kind, so
  `GET /v1/map-variants?base=<sha256>` lists a map's variants and
  `GET /v1/map-variants/{id}` gives one. A preview picture is a second
  blob under the map packages' rule (512 KB, PNG or JPEG).
- **Sharing.** The Live site has a page a map with its variants and a
  page a variant; **Download** puts it in the signed-in player's list,
  which the game fetches into `live/` at sign-in as it does game
  variants. In the game, **Upload to Xbox Live** is a row of a
  variant's actions ([forge-map-variants-ui.md](forge-map-variants-ui.md)).
- **Limits.** A variant is refused over 1 MiB and over the document's
  op cap; the account quota and moderation are the map packages'
  ([live-admin.md](../live/live-admin.md), section 7). The name and
  description are moderated as gamertags are.
- **Both homes.** Routes, the event and the checks are in the local
  server and the Worker, and the parity suite covers them
  ([live-worker.md](../live/live-worker.md)).

## Interfaces

- **`forge-doc`**: `Endpoint::Base.print`, `print`, `check_print`, `Edit::Entry`,
  `EntryOp`, `Property`, `resolve`, `Resolved`, `Printed`,
  `Apply::prints`, `Refusal::Print`, `games::{GameSet,
  encode, decode}`, `Tier`, `ForgeDoc::{tier, check_variant}`,
  `Refusal::Tier`, `play_form`, `session_form`, `KIT_RECIPE`,
  `variant_name`.
- **`forge`**: `Thing {id: Endpoint, block, kind, own, frame, fields,
  changed}`, `ThingKind`, `BaseRows`,
  `Session::{with_base, left_out, things, thing, move_thing, hold,
  release, set_field, remove, reset}`, and on `World`: `base_move`,
  `base_field`, `base_inert`, `base_restore`. The tool's
  `Tool::{aimed_thing, held_thing, game, show}`, `Show`, `in_game`,
  `MARK_RADIUS` and `Command::{MoveThing, Remove}`.
  `forge::props` is the table of properties by name: the kinds
  each is on, how it reads a thing and the op it writes. `forge::files`
  is the variant store.
- **Agent API** (game), in [agent-api.md](../runtime/agent-api.md),
  "Local Forge session". `forge.things {kind?, changed?}` (read) lists
  the things with their endpoints, fields and prints: every test and
  lane that edits the map's entries needs to name one, and
  `forge.status` holds only the aimed and held one. `forge.propose`
  takes `EntryOp` ops, and a refused edit carries its name
  (`forge_print`, `forge_was`, `forge_left_out`, ...) in the error's
  `data.code`. `forge.status` reports the variant's name, id, tier and
  whether it has unsaved edits (`variant`). `forge.variants {map?}`
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
| `variant_tier` | a file offered as a variant holds more than one may | "<name> is a Forge map, not a map variant." |
| `variant_expect` | the built map's id is not `expect.map_id` (`forge_expect_map_id`) | "<name> did not build to the map its author saved." |
| `variant_missing` | the document is on neither this machine nor Live, or is hidden or blocked | "<name> is not available." |
| `variant_game` | the start gate: a goal or a label the game variant needs is absent | the gate's sentence, naming what is missing |
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

Each is one lane of one to two hours: code and tests on invented data,
no game run. The coordinator builds and runs. The UI slices (K5, H1 to
H3, S1 to S6) are in
[forge-map-variants-ui.md](forge-map-variants-ui.md), "Slices". Paths
are under `crates/` unless they start with `docs/`, `assets/` or `web/`.

| # | Slice | Owns | Tests | Done when |
|---|---|---|---|---|
| **M1** | Prints and `EntryOp` ops in the document: `Endpoint::Base.print`, `print`, `EntryOp::{Set, Remove, Reset}`, format 3, the validator's rules, `Refusal::Print` | `formats/forge-doc/src/{entry.rs (new), document.rs, links.rs, validate.rs, lib.rs}`, `formats/forge-doc/src/tests/entry.rs` (new), `formats/content/src/forge.rs`, the crate README | golden vectors for a print and a format 3 id; a document without entry ops keeps its id; each validator rule has a refusing case; the seeded fuzz covers the new ops | `cargo xtask test -p forge-doc -p content` passes |
| **M2** | `resolve`: identities to running indices, lowering to `Placement` ops, the fold over `Reset` | `formats/forge-doc/src/{resolve.rs (new), log.rs, lib.rs}`, `tests/resolve.rs` (new) | a table of add, remove and set orders gives the indices a hand replay gives; lowered ops on a `scenario_edit::fixture` equal the same edits through `scenario_edit::Session`; fold then undo gives the start; `Reset` leaves the entry's bytes as the fixture's | the parity test passes with entry ops |
| **M3** | Materialise entry ops with the print check | `tools/forge-build/src/{entry.rs (new), placement.rs, lib.rs}`, `tools/forge-build/tests/entry.rs` (new), `apps/h2-tools/src/forge_cmd.rs` | moving own entry 2 changes its bytes and the checksum only; a removal swap-removes and repoints names as the editor does; a wrong print is `variant_print` with the block and index and no file; two builds are byte-equal | `h2 forge check` on a fixture document prints each print compared |
| **M4** | Tier, forms and the name rule: `Tier`, `ForgeDoc::tier`, `play_form`, `session_form`, the 16-character name | `formats/forge-doc/src/{variant.rs (new), lib.rs}`, `tests/variant.rs` (new) | a table of documents and their tiers; `play_form(session_form(d))` is `d`; a recipe other than the kit's makes `Tier::Map`; a 17-character name is refused for a variant | tests pass |
| **M5** | Things in the session: `Thing`, `Session::things`, `move_thing` on own and added entries, the hold | `extensions/forge/src/{things.rs (new), scene.rs, session.rs, lib.rs}`, `extensions/forge/src/base_tests.rs` (new) | over a fake `World`: a drop of an own entry is two `EntryOp::Set` in one undo step and one `base_move`; undo calls `base_move` with the first frame; a print the fake changes is refused and the document is the same bytes | existing `tests.rs`, `tool_tests.rs`, `link_tests.rs` pass unchanged |
| **M6** | Field edits, removal, reset: `Session::{set_field, remove, reset}`, `forge::props` for the fields that need nothing | `extensions/forge/src/{props.rs (new), session.rs}`, `extensions/forge/src/props_tests.rs` (new) | a table of `props` by kind; each row's write is one `EntryOp::Set`; remove calls `base_inert`, undo `base_restore`; removing a linked entry ends its links in the same step; a property not in the table is absent | tests pass |
| **M7** | The loaded map's things: `forge_map` reads every block's rows, typed fields and prints | `extensions/game/src/forge_map.rs`, its tests | on invented guest memory: each kind's things are listed with the layout's fields; a print equals `forge-build`'s for the same rows; a block that does not read is left out by name | tests pass |
| **M8** | `ForgeWorld`'s base calls: object by type and placement index, scenario field writes, the inert row, restore | `extensions/agent/src/forge/{base.rs (new), local.rs, tests.rs}` | over the fake game: `base_move` moves the object and writes the placement; an object that is gone is `forge_object_gone` with nothing written; the inert row equals the kit's spare form; restore writes the kept row | tests pass; `cargo check -p cli --no-default-features --features host` |
| **M9** | Aim at any thing: the reticle picks marks of starting points, goals and spawners with nothing out, and the map's own objects | `extensions/forge/src/{tool.rs, tool_tests.rs}`, `extensions/game/src/forge_glue/editing.rs` | a ray table: the nearest thing within its pick radius wins; a thing of another game type is aimed at only with Show at All | tests pass |
| **M10** | Agent API: `forge.things`, entry ops in `forge.propose`, the variant in `forge.status` | `extensions/agent-proto/src/forge.rs`, `extensions/agent/src/forge.rs`, `extensions/agent/src/forge/tests.rs`, `docs/runtime/agent-api.md` | each method answers over the fake game; a refusal carries its name | `session.methods` lists them |
| **D1** | Decode: the item spawner's and starting point's game-type slots, the respawn mode's two values, a census of how the disc's maps use them. Research with `h2 disasm`; names from the public layout are hints | `formats/map/src/scenario_layout.rs`, `tools/scenario-edit/src/tests.rs`, `docs/editor/scenario-editor.md` | the ignored disc test checks every value against the decoded range | each field has an **E** or is left out, and section 7 of the editor's page loses its line |
| **D2** | `forge_doc::games`: the set, `encode`, `decode`, the flag type's game type | `formats/forge-doc/src/{games.rs (new), lib.rs}`, `tests/games.rs` (new) | a table of sets and slots both ways; five single engines are refused; an unknown slot value decodes to "not editable" | tests pass |
| **D3** | Rows for D1's fields in `forge::props`: Game types, Place at start | `extensions/forge/src/{props.rs, props_tests.rs}` | the table gains the rows; a set change is one op a changed slot | tests pass |
| **D4** | `EntryOp::Property` and the `team` property | `formats/forge-doc/src/{entry.rs, validate.rs}`, `tests/entry.rs`, `extensions/forge/src/props.rs` | golden vector; a property on a kind that has none is refused; `forge_doc::labelled` reports the team | tests pass |
| **D5** | Spare clips and weight, each after its trace: the weapon reserve's field, the spawn chooser's rule | `docs/game/notes/forge-probes.md`, then `extensions/agent/src/forge/base.rs`, `extensions/forge/src/props.rs` | a differential test of the write on the fake game | the row appears; until then it is absent |
| **V1** | The variant store: per-map directories, `.h2mapvariant`, list, save, load, rename, duplicate, delete, the named errors | `extensions/forge/src/{files.rs, lib.rs}`, `extensions/forge/src/file_tests.rs` (new) | in a temporary directory: a save reads back to the same id; a name taken is refused without case; a file of the map tier is `variant_tier`; a base of another hash lists with `variant_base_hash` | tests pass |
| **V2** | The lobby's pick and the build before Start: `lobby_start::map_variant`, the checks, the package selection | `extensions/game/src/{map_variant.rs (new), lobby_start.rs, lobby_status.rs}` | over a fake library: a pick builds once and reuses the receipt; each refusal of the table gives its sentence and holds the start state; a map change clears the pick | tests pass |
| **V3** | `MapGate` for a variant: the document by id, the checks, the build callback, the member's sentence | `live/live-hle/src/maps.rs`, its tests | a joiner without the base is refused before load; a hidden document is `variant_missing`; a ranked session takes only a pinned id | tests pass |
| **V4** | Live: the projection and the two routes, the unlisted upload, the 1 MiB cap, in the local server | `live/live-proto/src/map_variants.rs` (new), `live/live-server/src/map_variants.rs` (new), `live/live-server/src/content.rs`, `docs/live/live-api.md` | a document of the map tier is not listed; an oversize one is refused; the list is by base hash, newest first | tests pass |
| **V5** | The same in the Worker, and the parity suite | `live/live-worker`, the parity suite's cases | each V4 case in both homes | the parity suite passes |
| **V6** | Playlist entries: `map_variant` on a record, `HopperVariantsSet`, the publish checks, the pin | `live/live-proto/src/hopper`, `live/live-server/src/admin.rs`, `docs/live/live-admin.md` | a variant of another map than the record's is refused by name; a reorder with `from` carries the variant; two like records with two variants are refused | tests pass in both homes |
| **V7** | A game variant's suggestions: `maps` in the bundle, the picker's order | `formats/variants/src/portable.rs`, `extensions/game/src/map_variant.rs`, `docs/editor/variants.md` | a bundle without `maps` keeps its bytes; more than eight are refused; the picker opens on a suggested variant | tests pass |
| **V8** | The Live site: a map's variants and a variant's page, Download | `web/` (the Live site's pages) | the pages' own tests against the routes' fixtures | the pages render from fixtures |
| **V9** | The editor: a map variant on a playlist entry; open a `.h2mapvariant` in the scenario tool | `apps/studio/src/ui/playlists.rs`, `apps/studio/src/api`, `docs/editor/agent-api.md` | the API's tests: an entry round-trips with its variant | tests pass |
| **G1** | Objects by game type: the `games` property, the build for an engine, `game` in `MapId` format 2 | `formats/forge-doc/src/entry.rs`, `tools/forge-build/src/entry.rs`, `formats/content/src/id.rs`, `docs/game/maps.md` | golden vectors: an id without `game` is unchanged; a build for Slayer lacks a CTF-only crate and has the rest | tests pass |

M1 to M4 depend only on each other, in order. M5 and M6 need M2; M7
and M8 need M1 and can run beside them; M9 and M10 need M5 to M8. D1
is research and can start at once; D2 to D4 follow it. V1 needs M4; V2
needs V1 and M3; V3 to V5 need V1; V6 and V7 need V4. G1 is last. P2 of
[forge-design.md](forge-design.md) (kit sessions) is what lets a
session *add* a spawner, a starting point or a goal; every slice here
that edits an own entry needs no kit.

## Open decisions

The owner's to make. Each has a recommendation, which the sections
above follow.

1. **One saved form or two.** A scenario-only document is a map variant
   (`.h2mapvariant`, listed per map); a document with geometry, recipes
   or tag writes is a Forge map (`.h2forge`, picked as a map).
   Recommended as written: a variant then builds in well under a
   Rebuild, fits every map and can go on a playlist entry. The other
   way is one file type for everything, with the picker listing heavy
   documents as variants.
2. **Does a game variant choose a map variant.** Recommended: it only
   suggests, and its script's labels are what it requires. The other
   way is that picking the game variant sets the lobby's map variant,
   which a leader can then undo.
3. **Variant names.** Recommended: 16 characters, as the game's own
   variant names. A longer cap needs every text that shows one checked
   for fit.
4. **Spare clips, a spawner's team and a starting point's weight.**
   Halo 2's entries have no field for them. Recommended: build them as
   Forge properties, each shown only once its hook is proven (D5), and
   ship the editor without them. The other way is to leave them out of
   the design.
5. **Ranked.** Recommended: a map variant plays ranked only as an
   admin's playlist entry. The other way is never.
6. **A variant not on Live in a Live lobby.** Recommended: the host
   uploads it unlisted when it picks, so members can fetch it. The
   other way is to refuse the pick until the author uploads it.
7. **`.h2forge` drafts already saved** under `<data>/forge`.
   Recommended: V1 reads none of them; nothing is published and the
   files are test saves.

## Evidence

- **E**, **V**: blocks, strides, typed fields, what refers to an entry
  by index, the spawner and its tick: forge.md 1.2, scenario-editor.md
  sections 1, 2 and 6.
- **Code**: `Endpoint`, `ForgeDoc::links` and the label census in
  `crates/formats/forge-doc/src/{links.rs, labels.rs}`; the fold in
  `log.rs`; the session's edit path in
  `crates/extensions/forge/src/session.rs`; the kit's spare rows in
  `crates/tools/forge-build/src/kit.rs`.
- **Code**: `NewSession.map_id`, hopper pins and the ranked rule in
  `crates/live/live-hle/src/maps.rs` (`MapGate::check`).
- **C**: Halo 3's map variants, their fields and the separate game
  variant: forge.md 1.1 and its sources.
- **E**: the item spawner's game-type slots, the matcher and the respawn
  modes: scenario-editor.md section 2 and
  [notes/2026-11-23-item-game-types.md](notes/2026-11-23-item-game-types.md).
- **C**: the absence of a team, a clip count and a weight: field names of
  the public Halo 2 scenario layout.
- **Design**: the print, the `EntryOp` family, the inert row in a
  session, the tier, the pick's forms, the refusals.

## Not verified

- That an inert row of each block is skipped by every reader in a
  running round (F3's kit probe, [notes/forge-probes.md](notes/forge-probes.md)).
- That a live object's placement index finds the map's own objects of
  every type, and that the spawner's state array gives the object an
  entry has out.
- That a starting point's or a goal's write in the loaded scenario is
  read in the round it is made, or only from the next.
- How the retail maps use the game-type slots and respawn modes: the
  census `disc_item_game_types_and_respawn_modes_census` has not been run.
- What the object bits that arm respawn modes 0 and 1 mean.
- A weapon's reserve field and the spawn chooser's rule (D5).
- How long a variant takes to build on a real map, and that the lobby
  can wait for it without a hold a player notices.
- That the host can tell which record of a hopper matchmaking drew.
- That a session's package selection survives the game's own map load
  path in every lobby kind (split screen, System Link, Live).

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
