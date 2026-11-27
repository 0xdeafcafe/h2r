# Forge: the design

Status: Local Objects editing uses the Forge document, materialiser, runtime
probes and monitor UI. Place, move, turn, delete, undo and redo apply to the
running game's objects through live-mirror M8's object calls. The Links
tool joins teleporter flags on a channel, written to the running game's
scenario, and machines into device groups, written when the map is
materialised; **Rebuild map** builds a play copy from the document and
reloads the running map onto it, and the session goes on over it. The
synchronous network mode's protocol (the log on the
network, roles, holding, join in progress) is built and tested over fake
machines; no transport or game carries it yet. The host-replicated mode
is planned.
Code: `crates/formats/forge-doc` defines the document and log.
`crates/tools/forge-build` materialises it. `crates/extensions/forge`
owns the local session and its tools ([forge-editing.md](forge-editing.md)), and its
`net` module the synchronous protocol. The host is
`crates/extensions/game/src/forge_glue.rs`, the screens are in
`crates/extensions/game/src/forge_screens.rs`, the monitor HUD's feed is
`crates/extensions/game/src/forge_hud.rs`, and the game calls and API
are in `crates/extensions/agent/src/forge`.
Switch: `forge`, listed in [switches.md](../runtime/switches.md).

Forge is an in-game editor for multiplayer maps: fly as a monitor, place
and link things, shape the level, save the result and play it with
friends. A Forge map is a **document**: a base map named by hash and an
ordered list of edits. Every machine turns the document into a playable
map from its own disc. The rule that must hold: **a document carries
only what its author made**. No byte of Halo leaves a machine through
the document, Live or this repository, and a model a user imported
reaches Live only from an account an admin has approved to upload
models.

[forge.md](forge.md) holds the retail evidence this page builds on (the
placement blocks, the netgame equipment spawner, object creation). This
page is the architecture, the features and the slice plan.
[forge-editing.md](forge-editing.md) is what a monitor can do, after
Halo 3, Reach and Infinite, and
[forge-firefight-ui.md](forge-firefight-ui.md) is how it is drawn.
[forge-map-variants.md](forge-map-variants.md) is the design for editing
the map's own placements and for the map variant, the saved form a
custom game and a playlist entry pick beside a game variant.
[forge-firefight.md](forge-firefight.md) is the design for building a
Firefight map in a session and playing it.

Evidence labels are those of [docs-style.md](../meta/docs-style.md):
**E**, **V**, **Code**, **Test**, **Run**, **S** (supposed, with what
would verify it) and **Design**.

## What the engine allows at runtime

Each kind of edit reaches a running game in one of three ways. The
classes are the ones the paired editor uses
([live-mirror.md](../editor/live-mirror.md), "How an edit applies"),
grouped.

| Class | Reaches the game by | Delay |
|---|---|---|
| **Live** | an engine entry at a tick boundary (`object_new`, the engine's move, delete, teleport), or a tag field write through `crates/extensions/hot-tags` | next tick or next use |
| **Kit** | the same, but only because the map was materialised with spare capacity for it (below) | next tick |
| **Rebuild** | materialising the document again and loading the map again; the session, its players and the log survive | seconds; a hold for everyone |

| Edit | Class | Why |
|---|---|---|
| Move, turn, delete an object that exists | Live | `world.object.set`, `.delete` (live-mirror.md, "The game's agent API") |
| Make an object from a tag the map holds | Live | `sim.spawn` path: placement data, then `object_new` (**Code**, [agent-api.md](../runtime/agent-api.md), "What `sim.state.set` writes"). The pool is 2,048 objects (**E**, forge.md 1.2) |
| Change a placement's fields; retarget a netgame equipment entry | Live, shows when the object is next made | read once at creation (**Code**, [hot-iteration.md](../graphics/hot-iteration.md), "What shows when") |
| Add a placement, spawn point, netgame flag or equipment entry | Kit, else Rebuild | a block cannot grow in guest memory; the count can rise into spare entries the kit reserved (**S**: each reader takes the count from the root pair on every walk; [scenario-editor.md](../editor/scenario-editor.md) section 6 lists the readers) |
| Teleporter pair and channel | Kit | netgame flags of the two teleporter types with a shared identifier (**Code**, [bots.md](bots.md) 3.2, "Traversal evidence"). A channel change on an existing flag is a field write (F6) |
| Button to machine | Kit | a control and a machine in one device group (**S**: the device-group readers are not disassembled, scenario-editor.md section 7) |
| Shader colours, effect and light colours, damage, rates of fire | Live | read at each draw or use (hot-iteration.md) |
| Texture pixels, same size and format | Live for the picture | the D3D write barrier re-uploads changed bytes (hot-iteration.md); the saved form is a `BitmapOp` log |
| Texture of another size or format; a new texture | Rebuild | the game's texture cache holds the old one; new raw data |
| A tag the map does not hold (a campaign biped, an imported character, a barrier of a new size) | Rebuild | the tag index and metadata grow in the file ([map-builder.md](../formats/map-builder.md), "Adding tags to a pinned map") |
| Render geometry | Live as a preview in the native renderer; Rebuild to keep | [modeling-targets.md](../editor/modeling-targets.md) write classes |
| Structure collision, portals, AI pathfinding sectors | Rebuild | compiled data: collision BSP, MOPP, `sbsp` pathfinding ([pathfinding.md](../formats/pathfinding.md)) |
| Bot nav mesh | Live | host-side Rust state, built off the game thread (`bots::autonav`) |
| Emitters, triggers, logic, barrier switches | Live | Forge's own state (below); no retail home |
| AI drop zones, bases and cover | Live to place the marker; Rebuild for the ground they are built into | a marker is a netgame flag with a label; sectors, zones and squad slots are compiled by `firefight_kit::ground::build` ([forge-firefight.md](forge-firefight.md)) |
| A wave set | Live | session state beside the document; the director reads it when a test or a match starts |
| An enemy class | Rebuild | a `char` tag and its closure, brought in by the ground recipe |

### The kit

A map materialised for a Forge session carries a **kit**: capacity and
tags added at build time so that most edits in a session are Live.

- Spare entries at the end of each placement block, the netgame flag,
  netgame equipment and starting location blocks, and the object name
  block, up to the engine's per-type limits (scenario-editor.md, "Add").
  F2 leaves the root count at the active entries. Where the format has
  a sentinel, the reserved rows carry it (for example palette index
  -1). F3 probes whether engine readers take the count from the root
  on each walk.
- Spare device groups follow the same active-count rule. Squad slots
  are not the kit's: the AI ground has sixteen, which the director
  fills ([forge-firefight.md](forge-firefight.md)).
- Generated tags of our own, built from nothing on the disc: the
  block-out set ([forge-blockout.md](forge-blockout.md): the dev
  textures, a starter set of pieces, spare slots a session rewrites to
  a new size, and every definition the document's pieces use) and
  marker objects for emitters and volumes. A piece is a `bloc` with a
  `coll` from the `collision-build` crate
  ([modeling.md](../editor/modeling.md) slice 8), a `phmo` with one
  fixed body and a `mode`; a barrier is a piece with no `mode`.
- The tags a document's recipes ask for (monitor bodies, AI characters,
  imports). Enemy classes come with the AI ground, not the kit.

The kit costs tag memory. Multiplayer maps have 0.1 to 1.5 MiB of
headroom under the bound the writer keeps, and one map has none (**V**,
scenario-editor.md section 6). The real headroom is not measured; slice
F3 measures it. A kit that does not fit is cut in a fixed order (block-out
set, in the order of forge-blockout.md, "Reaching the game"; spare
entries; then recipes are refused by name), so two machines cut alike.

A session document requests the kit with the `forge_kit` generate
recipe. F2 reserves up to eight slots per block, in scenario root
offset order, and reduces a reservation to fit the writer's bounds.
The recipe is part of the hashed document. A published play document
omits the kit recipe.

## How it works

### The document

`forge-doc` is a pure crate: no guest, no GPU, no disc.

```
ForgeDoc
  format, builder: {forge-build version, pathfind generator, bots generator}
  base: {map, sha256}                  the disc's map, never its bytes
  sources: [{map, sha256}]             other disc maps a recipe reads
  title, author, description           player text
  recipes: [Recipe]                    tags to bring in, by reference
  waves: firefight::Def?               one Firefight wave set (design)
  imports: [ImportSlot]                the author's own assets, by hash
  ops: [Op]                            ordered; ids are stable
  expect: {map_id, geometry}           optional build result
```

An `Op` is one of the editor's own commands or a Forge one:

| Family | Ops | Source of the vocabulary |
|---|---|---|
| Placement | set, add, remove on a scenario block | `scenario_edit::Command` (scenario-editor.md section 3) |
| Entry (design) | set a field of, remove or reset an entry named by identity: a base entry by its index in the base map with a print of its fields, or an added one by its op | [forge-map-variants.md](forge-map-variants.md), "Naming an entry" and "The ops" |
| Tag field | set a field by tag key and path | `edit.batch`'s write, with a digest of the prior typed value in `was` |
| Mesh | a `MeshOp` on a target | modeling.md, "Operations" |
| Bitmap | a `BitmapOp` (strokes, fills, adjusts) | modeling.md, "Texture editing" |
| Forge | emitter, volume, barrier, link, label, logic node, nav setting, body | this page; labels in [rules-engine.md](rules-engine.md) "Forge labels" |

Tags are named by group and path, resolved through the map's tag index
when the document is applied. A Mesh op holds coordinates the author
made and ids into the source mesh. A Bitmap op holds strokes, never
pixels of a disc texture. So a document replayed on a machine without
the disc produces nothing, and on a machine with it produces the map.

The file is canonical JSON through `content::canonicalize`, the
integer-only implementation of RFC 8785. Floats are eight lowercase
hexadecimal digits of their `f32` bits. The document's identity is the
SHA-256 of its canonical form with the entire `expect` block omitted.
Use `ForgeDoc::from_bytes` to read the 8 MiB
bounded format and `ForgeDoc::canonical_json` to write it. The editor saves
map variants in the variant store ([forge-map-variants.md](forge-map-variants.md),
"The map variant file"); `h2 forge build` reads a `.h2forge` path. A draft can omit `expect`.
The `format` is 1, or 2 when an op sets or clears a label; validation
refuses the other number, so a document has one spelling and every
document without labels keeps its identity.

A **recipe** brings tags into the map by reference:

| Recipe | Does | Carries |
|---|---|---|
| `transplant` | copies a tag and its closure from a source map on the user's disc into the map, through `tags::repo` (`MapSource`, `closure`) and `TagSet::add_tag` ([tag-repo.md](../formats/tag-repo.md)) | the tag key and the source map's hash |
| `variant` | a copy of a tag the map holds with listed fields changed (a `bipd` with another `hlmt`) | the key and the field writes |
| `generate` | a kit tag made by our code, or the AI ground (`ai_ground`) | the generator's name and parameters |
| `import` | binds an import slot to tags converted on this machine | the slot id |

### Materialising

`forge-build` turns a document and the user's disc into a map package
([maps.md](maps.md) section 1) under `<data>/maps/forge-<first 16 hex of
the id>/`. F2 runs placement commands through
`scenario_edit::Session`, applies named typed tag-field writes, and
saves with the editor's map writer. It verifies the pinned versions
and each named source hash. The later geometry and paint slices use
`geometry-edit` and `bitmap-edit`. Later recipes use
`map::build::TagSet`, and collision edits regenerate pathfinding with
`pathfind::generate`.

- **It is deterministic.** The same document, disc hashes and builder
  versions give the same bytes, on arm64 and x86-64. The `builder` field
  pins the versions; a machine with another version refuses the document
  with the versions named. `expect` lets a joiner check its own result
  before the lobby does.
- **The package is local.** It holds bytes derived from the disc
  (transplanted tags, patched geometry), so it is never uploaded. Its
  manifest has one `[[forge]]` layer naming the document's hash, and
  `content::MapId` gains a `forge` list in its canonical form (manifest
  format 2). Format 2 hashes the base map's SHA-256 and the Forge
  document's identity. Its canonical JSON is
  `{"base":"<base hash>","forge":["<document hash>"],"format":2}`, with
  a `game` member for a document whose objects have game types
  ([maps.md](maps.md), "Identity: `MapId`").
  Build and load compare `expect.map_id`, when present, with that
  computed ID and report `forge_expect_map_id` on a mismatch. The
  resolver verifies the packaged document, disc base and built map
  hashes before serving the file.
- **Rebuild in a session.** The host announces a rebuild in the log.
  Every machine materialises the document as of that log position, the
  session saves each player's position and body, the map loads again
  from the new package, and play resumes. The existing reload path is
  `map.reload` (live-mirror.md slice M9).

### Rebuild map

In a local session, **Rebuild map** on the Forge menu builds a play copy of
the map from the document and reloads the running map onto it; the
session goes on over the copy with the same document, log, undo and
redo. It is how a Rebuild edit (a device link, an add past the kit's
spare rows) reaches the game.

- **The copy.** The editor builds the session's live form
  (`forge::live_form`, `Session::rebuild_form`) off the game thread
  with the composition root's builder
  (`forge_glue::reload::install_builder`, `boot::ground_builder::AppCopy`:
  `forge_build` over the disc's maps) and writes it as
  `<data>/cache/forge-play/<cache name>.map`
  (`Editor::start_rebuild`). The live form keeps every row where the
  session has it: a loaded block is never reordered under a running
  game, so the removal of an own object entry becomes a write of an
  empty palette index, which makes no object, and the removal of an
  own entry of any other block is left out. The n-th add to a block is
  row `base + n` in the copy as in the running game.
- **The reload** is `map.reload`'s, one function for both
  (`agent::sim::MapReloader`, `Sim::start_reload`): the driver hands
  it the copy's file and the game queues the reload onto it with the
  options it is running. It needs what `map.reload {file}` needs, the
  agent socket and the editor map mount; without them the row says
  `This game cannot reload the map.`
- **The session goes on.** The map's activation would end the session,
  so the driver carries the editor across a reload of its own
  (`forge_glue::reload::Carry`) and, when the session opens on a map
  with the copy's checksum, reopens it over the copy
  (`Editor::reopen`, `Session::adopt_copy`). The copy's rows are what
  the world holds: each added object is the live object the game made
  at its row (`agent::forge::base::live_object`), and the kit's room is
  the copy's spare rows plus the rows the document adds. The session
  then brings the world to the document as after any edit, so a
  removal the live form left out is made inert again and an added
  object the game did not make is placed. Nothing the copy holds is
  placed twice.
- **What is kept.** The document, the log, undo and redo, the monitor,
  its place and the tool settings. The selection and anything held are
  let go; a rebuild is refused while the monitor holds something
  (`Drop what the monitor holds first.`). Undo after a rebuild goes
  back over it on the copy's rows: an add the copy made is deleted, a
  removal is restored.
- **When it has nothing to do.** The session keeps the digest of the
  form the running map was built from (`Session::rebuild_needed`); with
  no edit since, the row says `The map already has every edit.` and
  nothing is built. The device links in that form are baked, and the
  pending count on the row is the active device links the running map
  does not have yet (`Session::pending_rebuild`).
- **The notices** are `Rebuilding the map...`, then `The map is
  rebuilt.` when the session has gone on over the copy. A reload the
  game refuses leaves the session on the map it has, with the reason.
  A copy that does not build says the build's sentence.

### Who owns state

The sim rewrite's rule holds ([sim-rewrite.md](../engine/sim-rewrite.md)
section 2): guest memory is the persistent truth for everything retail
has a home for. Forge follows it.

| State | Kind | Where |
|---|---|---|
| Objects, placements, tag fields, netgame flags, device groups, squads and actors | guest truth | the arena and the tag data, written through engine entries and `hot-tags` |
| Emitter timers, logic node state, barrier switches, volume occupancy, the object-to-op link, who holds what, budgets | **Extension** | `ForgeExt` in the `SimExt` blob: saved with checkpoints and snapshots, carried in films, folded into the sim hash |
| The document and the log; the wave set and the ground recipe beside them | session state, outside the sim | the Forge extension, on every machine |
| A wave test's director | **Extension** | the rules VM's `RulesExt`, as in a Firefight match ([firefight.md](firefight.md), "The director") |
| Bot nav mesh, collision BVHs, the preview meshes and textures | **Derived** or **Static** | rebuilt on an epoch bump; a Rebuild is an epoch bump |
| The monitor's camera, gizmos, ghosts, link lines, overlays | presentation | appended to the `FrameScene` after extraction ([framescene.md](../graphics/framescene.md) section 4); never read by the tick |

`ForgeExt` is written only at a tick boundary, by the Forge step, which
runs before the game's tick on the game thread. It uses `sim-core`'s
float rules and iterates in op-id order. A ghost (an object being
placed, not yet confirmed) is a scene instance, not a guest object, so
aiming a placement costs the sim nothing.

### The edit log and multiplayer

The host owns the log. Every edit, the host's included, is a proposal
until the host accepts it.

```
up:      propose {nonce, proposal: {author, base_seq, op, was?}}   machine -> host
         hold {nonce, target} | carry {target, frame} | release
reply:   accepted {nonce, n} | refused {nonce, refusal}            host -> author
stream:  {n, tick, item}                                           host -> every machine
item:    entry {seq, tick, author, op, prev_hash} | hold | carry
         | release | member | left | through
```

`forge::net` (`crates/extensions/forge/src/net.rs`) is the protocol. It
is pure: no clock, socket or guest. `net::Host` is the one authority.
Every machine, the host's own included, runs a `net::Peer` fed by the
host's stream. An entry is F1's `forge_doc::Entry`; its canonical bytes
and SHA-256 are those of `host_log::Entry<Op>` with the author set
([ai-networking.md](ai-networking.md), "How it works"). Only entries form
the hash chain and reach the document. Holds, carries, releases, members
and the watermark ride the same stream and never do.

- **The stream.** The host numbers every item it writes from 1 (`n`)
  and gives it an apply tick at least `APPLY_DELAY` (4) ticks ahead;
  the ticks of all items but the watermark never go backwards. Once a
  tick (`Host::tick`) it writes `through`, the watermark: no item at or
  before its tick follows. A peer buffers items by `n`, whatever order
  they arrive in, ignores a copy, and hands its game the items due at a
  tick in `n` order (`Peer::due`). It holds the tick (`PeerError::Wait`)
  until the watermark reaches it, so every machine applies every item at
  the same tick. An item for a tick a machine has already run ends that
  machine's session (`PeerError::Late`).
- **Admitting an edit** (`Host::propose`). The author must be a member
  whose role edits. The host writes the seat's name as the author, so a
  machine cannot speak for another. A proposal from a later position than
  the host's is refused (`ahead`). One from an earlier position is
  rebased onto the head when it names its entry by identity (an entry
  op, a link, a label, an add without `like`, an undo); one that names an
  entry by its running index (a placement set or remove, an add with
  `like`) is refused as `stale`, since an add or remove since may have
  moved the index. `was` is checked against what the host holds now,
  through the game's `net::Admit`. The host stamps the entry at the head
  (`LogHead::stamp`), folds it, checks that every entry op still names an
  entry (`forge_doc::resolve`), and asks `Admit::admit` about the
  document it leaves (palette, budget, bounds). A refused proposal is
  answered to its author with the reason (`net::NetRefusal`) and never
  enters the log.
- **The whole-log rules.** Folding one entry at a time forgets an undone
  op and the author of an op already folded. `net::State` keeps every op
  the log has named with its author, so the host admits exactly what one
  fold of the whole log admits: an undo by another author is
  `undo_author`, and an undone op's ID taken by a different op is
  `duplicate_op`. Re-proposing the same op is a redo.
- **Op IDs.** Seat `s` makes new ops with IDs `s + 1`, `s + 17`,
  `s + 33` and so on (`net::lane`, `Peer::op_ids`), above every ID the
  session has named, so two machines never pick the same ID. An ID at or
  below the starting document's largest belongs to no seat. A new op
  outside its author's lane is refused (`lane`).
- The first entry names the starting document's hash. Later entries
  name their predecessor's hash. `validate_chain` checks these links,
  consecutive sequences, nondecreasing ticks and an expected head when
  supplied. Machines report their log head and stream position in the
  session's keep-alive (`net::Beat`). `Host::check` compares the head
  with its own at that sequence; a head that differs ends that machine's
  session with `head_differs`.
- Each machine applies an entry at the start of `tick`, in `seq` order.
- **Holding.** Picking a thing up is a `hold` item; the host refuses a
  second hold of the thing (`held`, naming the holder), and a seat's new
  hold replaces its old one. While a thing is held, the host refuses
  another seat's edit of it, and of any entry of its block named by
  index. Moves of a held thing are `carry` items, at most one for each
  apply tick, inside the world bounds; they move the world and never reach
  the document. The drop is the holder's edit, then a `release`. An entry
  that removes or retracts the held thing ends the hold, and so does a
  role that cannot edit and leaving. A Rebuild waits until no one holds
  anything (`Host::may_rebuild`).
- **Undo** is the inverse op proposed by the same author. The log only
  grows. F1 folds an inverse by retracting the latest active op by ID
  and replays the remaining edits from the base. The map's existing
  placements and their intact fields are available locally on replay.
- **Join in progress** (`Host::join`, `net::Welcome`). The newcomer
  gets the document the session's map was materialised from, the entries
  after it up to what every machine has applied at the host's tick, the
  head they end at, the members and holds at that point, and the items
  still to apply. `Peer::join` folds the entries one at a time and in one
  fold, refuses a log that does not reach the host's head, and goes on
  from there. The newcomer materialises, then joins.
- **Roles.** The host gives each player a role (`net::Role`): editor,
  player or spectator. An editor edits, holds and carries; a player and a
  spectator do none of these. A role change is a `member` item.

Forge has two network modes, built in this order.

**Synchronous (first, slice F5).** The session runs the engine's
synchronous protocol ([live-coop.md](../live/live-coop.md) 1.1), the one
co-op over Live runs. Every machine applies every entry identically and
simulates everything, AI included. The sim hash proves it, as in co-op.
Players: `coop.max_players` (2 to 16). The protocol is chosen when a
session has no variant or a variant of an engine mapped to it (**E**,
live-coop.md 1.1); whether it holds a multiplayer map with a game engine
running is **S**, and slice F3 probes it. Every Forge feature works in
this mode, so it is the mode for editing with friends and for any map
until the second exists.

**Host-replicated (second, slice F15).** The session runs the
distributed protocol, as retail multiplayer does: the host owns the
world and clients predict. It gives Forge maps ordinary multiplayer
netcode and 16 players. Entries split in two:

| Entry | Applied by | Clients get it from |
|---|---|---|
| Definition: placements, tag fields, `ForgeExt` records | every machine, at the entry's tick | the log |
| World: make, move, delete a live object | the host | the engine's own replication, for the object groups it carries (**S**, by group; F3 measures) |
| World, for a group the engine does not carry | the host | Forge's replication, below |

Forge's replication fills what the engine's does not:

- **Mirrors.** A client makes its own copy of a host object from the
  entry (tag, transform, the host's id) and keeps a host-id to local-id
  map in `ForgeExt`. The host sends each mirror's transform and velocity
  on Forge's channel when it moves, in the `objects` layer's delta form
  (`agent_proto::live`); the client moves its copy through the engine's
  move. A mirror takes no damage on a client; the host decides.
- **Schedules.** Emitters and the logic graph are definition records.
  Each machine fires an emitter from the shared tick count, so the
  projectiles are local predictions of the same shots; hits and damage
  are the host's. A sink that changes game state (teleport, kill, device
  power, squad place) runs on the host only.
- **Actors as puppets.** AI runs on the host; a client never runs it
  (**E**, [ai-networking.md](ai-networking.md), "What retail's
  replication carries"). Each actor's body reaches clients as a unit
  entity of the engine's own replication, and posture and dialogue on
  the actor channel, so a client draws and can shoot it; the shot is
  judged by the host. The layer is ai-networking.md's, shared with
  Firefight.

Proof in this mode is the log head, each definition entry's `was`, and
a hash of `ForgeExt`'s definition part in the keep-alive. A map marks
what it needs: one with AI, emitters or mirrors is offered in
host-replicated sessions only once F15 covers that part, and is
synchronous until then.

A finished map with none of those (placements, links written as retail
data, geometry, textures) needs neither mode's machinery: every machine
materialised the same package before the map loaded, and it plays in
retail's distributed protocol as any map package does.

### Films and determinism

- A film of a Forge map names the document's hash and builder in `HEAD`;
  playback materialises it first. Map content is already part of a
  film's match ([film-format.md](film-format.md), "Determinism and
  verification").
- A film of an editing session records the session's welcome at its
  start and each stream item at the tick the machine applied it, as the
  film file's `FRGE` extension records
  ([film-format.md](film-format.md#the-film-file)). The payload is the
  item's bytes (`net::Stamped::to_bytes`). Playback hands them to the
  session before that tick and applies them as the session did. `simulation_hooks` covers Forge's
  hooks, so a film records whether Forge was installed. The film's header
  decides, on both sides: a session's default recording and its replay
  install Forge, a film named with `--film` installs it on neither
  (`registration::local_forge`,
  [film-format.md](film-format.md#recording-every-game)).
- `ForgeExt` is in the keyframes and the sim hash through `SimExt`
  (sim-rewrite.md section 7).
- Theater never edits: writes are refused in playback, as for the
  paired editor (`SessionKind::writes`).
- Presentation-only differences are allowed between machines: a viewer
  without an import sees its chassis
  ([forge-features.md](forge-features.md), "Character import"). They never reach the hash.

### Saving and sharing on Live

What a machine sends to Live is the document, a preview picture and,
from an approved account only, the author's own converted models.

| | Shared through Live | Never leaves the machine through us |
|---|---|---|
| The document: ops, recipes, player text | yes | |
| A preview picture (PNG or JPEG, at most 512 KB, as map packages have) | yes | |
| The materialised package, patches made from it, nav caches | | derived from the disc |
| Transplanted tags (monitor bodies, AI characters, any disc tag) | named by key and source hash | their bytes |
| Painted textures | the strokes | the pixels |
| Edited geometry | the mesh ops | the result |
| Imported models, textures, animations and sounds, and tags converted from them | a slot: name, chassis, content hash; the bundle itself only from an account approved for model uploads | every byte, from any other account |

- **Store.** A document is a content-store blob by SHA-256
  (`crates/live/live-server/src/content.rs`), announced by a signed
  `ForgePublished` event beside `MapPublished`. The server parses it with
  `forge-doc`'s validator and refuses anything else under that kind: a
  blob that is not a valid document, a document over 8 MiB, an op with a
  payload type the schema does not have. There is no field that can hold
  a file.
- **Joining.** `live_hle::maps::MapGate` treats a Forge `MapId` as a
  package to make ready: fetch the document, check `base` and every
  `sources` entry against the disc's own hashes, materialise, compare
  with `expect`. Refusals name the reason before any map load: a map the
  disc lacks (the campaign maps, for a monitor body), another builder,
  a result that differs.
- **Imports.** A slot names its bundle by content hash. A hosted bundle
  is downloaded with the document; a bundle that is not hosted, or was
  taken down, leaves a skin drawn as its chassis and refuses a join that
  needs a prop's collision. [forge-features.md](forge-features.md), "Custom models on Live", is
  the upload rule.
- **Ranked.** Hopper and ranked sessions take only retail or
  admin-pinned ids (maps.md section 5); a Forge id is pinned like any
  other.
- **Moderation.** As map packages: hide, delete, block by hash, reports
  ([live-admin.md](../live/live-admin.md)). Player text is moderated as
  gamertags are.
- **Both homes.** The store and the event exist in the local server and
  the Worker ([live-worker.md](../live/live-worker.md)).

## Pages

| Page | What |
|---|---|
| [Forge design: features, UI, interfaces and rules](forge-design-features.md) | The Forge feature list, its UI, interfaces, rules and risks. |
| [Forge design: slices, decisions and evidence](forge-design-slices.md) | The build slices, decisions, evidence and open items. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Features

See [Forge design: features, UI, interfaces and rules](forge-design-features.md#features).

## The UI

See [Forge design: features, UI, interfaces and rules](forge-design-features.md#the-ui).

## Interfaces

See [Forge design: features, UI, interfaces and rules](forge-design-features.md#interfaces).

## Rules and limits

See [Forge design: features, UI, interfaces and rules](forge-design-features.md#rules-and-limits).

## Risks

See [Forge design: features, UI, interfaces and rules](forge-design-features.md#risks).

## Slices

See [Forge design: slices, decisions and evidence](forge-design-slices.md#slices).

## Decisions

See [Forge design: slices, decisions and evidence](forge-design-slices.md#decisions).

## Evidence

See [Forge design: slices, decisions and evidence](forge-design-slices.md#evidence).

## Not verified

See [Forge design: slices, decisions and evidence](forge-design-slices.md#not-verified).

## Related

See [Forge design: slices, decisions and evidence](forge-design-slices.md#related).
