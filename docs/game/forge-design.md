# Forge: the design

Status: Design. Nothing here is built.
Code: `crates/formats/forge-doc` (the document, ops, validation), `crates/tools/forge-build` (the materialiser), `crates/extensions/forge` (the session, the tools, the edit log), `crates/tools/asset-import` (glTF and FBX to tags), screens in `crates/extensions/game/src/ui_kit/`. Tests: each crate's own, on invented data.
Switch: the `forge` feature of `H2_FEATURES` (off until slice F4 is seen in the game); it joins [switches.md](../runtime/switches.md) with the code.

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
page is the architecture, the features, the UI and the slice plan.

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
| Teleporter pair and channel | Kit | netgame flags of the two teleporter types with a shared identifier (**Code**, [bots.md](bots.md) 3.2, "Traversal evidence") |
| Button to machine | Kit | a control and a machine in one device group (**S**: the device-group readers are not disassembled, scenario-editor.md section 7) |
| Shader colours, effect and light colours, damage, rates of fire | Live | read at each draw or use (hot-iteration.md) |
| Texture pixels, same size and format | Live for the picture | the D3D write barrier re-uploads changed bytes (hot-iteration.md); the saved form is a `BitmapOp` log |
| Texture of another size or format; a new texture | Rebuild | the game's texture cache holds the old one; new raw data |
| A tag the map does not hold (a campaign biped, an imported character, a barrier of a new size) | Rebuild | the tag index and metadata grow in the file ([map-builder.md](../formats/map-builder.md), "Adding tags to a pinned map") |
| Render geometry | Live as a preview in the native renderer; Rebuild to keep | [modeling-targets.md](../editor/modeling-targets.md) write classes |
| Structure collision, portals, AI pathfinding sectors | Rebuild | compiled data: collision BSP, MOPP, `sbsp` pathfinding ([pathfinding.md](../formats/pathfinding.md)) |
| Bot nav mesh | Live | host-side Rust state, built off the game thread (`bots::autonav`) |
| Emitters, triggers, logic, barrier switches | Live | Forge's own state (below); no retail home |
| AI squads | Kit to place one; Rebuild to add a character type | squads are scenario blocks; characters are tags ([ai.md](../halo1/ai.md)) |

### The kit

A map materialised for a Forge session carries a **kit**: capacity and
tags added at build time so that most edits in a session are Live.

- Spare entries at the end of each placement block, the netgame flag,
  netgame equipment and starting location blocks, and the object name
  block, up to the engine's per-type limits (scenario-editor.md, "Add").
  A spare entry is inert: palette index -1, or a flag type no reader
  matches (**S** per block; the test is that an unedited kit map plays as
  its base).
- Spare device groups and squad slots.
- Generated tags of our own, built from nothing on the disc: barrier
  boxes in a ladder of sizes, a block-out set (box, ramp, wedge,
  cylinder), marker objects for emitters and volumes. Each is a `bloc` or
  `scen` with a `coll` and `phmo` from `crates/formats/collision-build`
  ([modeling.md](../editor/modeling.md) slice 8) and, for block-out
  pieces, a `mode` from the section encoder.
- The tags a document's recipes ask for (monitor bodies, AI characters,
  imports).

The kit costs tag memory. Multiplayer maps have 0.1 to 1.5 MiB of
headroom under the bound the writer keeps, and one map has none (**V**,
scenario-editor.md section 6). The real headroom is not measured; slice
F3 measures it. A kit that does not fit is cut in a fixed order (block-out
set, spare entries, then recipes are refused by name), so two machines
cut alike.

A published map is materialised without the unused kit: `forge-build`
drops spare capacity the document does not use, so a finished map costs
only what it holds.

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
  imports: [ImportSlot]                the author's own assets, by hash
  ops: [Op]                            ordered; ids are stable
  expect: {map_id, geometry}           what the author's build produced
```

An `Op` is one of the editor's own commands or a Forge one:

| Family | Ops | Source of the vocabulary |
|---|---|---|
| Placement | set, add, remove on a scenario block | `scenario_edit::Command` (scenario-editor.md section 3) |
| Tag field | set a field by tag key and path | `edit.batch`'s write, with `was` |
| Mesh | a `MeshOp` on a target | modeling.md, "Operations" |
| Bitmap | a `BitmapOp` (strokes, fills, adjusts) | modeling.md, "Texture editing" |
| Forge | emitter, volume, barrier, link, logic node, squad, nav setting, body | this page |

Tags are named by group and path, resolved through the map's tag index
when the document is applied. A Mesh op holds coordinates the author
made and ids into the source mesh. A Bitmap op holds strokes, never
pixels of a disc texture. So a document replayed on a machine without
the disc produces nothing, and on a machine with it produces the map.

The file is canonical JSON (`content::canonical`, the integer-only copy
of RFC 8785; floats are written as their `f32` bit patterns in hex, so
the form stays integer-only). Its SHA-256 is its identity. The saved
file is `<data>/forge/<name>.h2forge`.

A **recipe** brings tags into the map by reference:

| Recipe | Does | Carries |
|---|---|---|
| `transplant` | copies a tag and its closure from a source map on the user's disc into the map, through `tags::repo` (`MapSource`, `closure`) and `TagSet::add_tag` ([tag-repo.md](../formats/tag-repo.md)) | the tag key and the source map's hash |
| `variant` | a copy of a tag the map holds with listed fields changed (a `bipd` with another `hlmt`) | the key and the field writes |
| `generate` | a kit tag made by our code | the generator's name and parameters |
| `import` | binds an import slot to tags converted on this machine | the slot id |

### Materialising

`forge-build` turns a document and the user's disc into a map package
([maps.md](maps.md) section 1) under `<data>/maps/forge-<first 16 hex of
the id>/`. It is the editor's save path run headless: placement ops
through `scenario_edit::Session`, mesh and bitmap ops through
`geometry-edit` and `bitmap-edit`, recipes through `map::build::TagSet`,
then pathfinding (`pathfind::generate`) for any BSP whose collision
changed, then the package writer (`scenario_edit::package`).

- **It is deterministic.** The same document, disc hashes and builder
  versions give the same bytes, on arm64 and x86-64. The `builder` field
  pins the versions; a machine with another version refuses the document
  with the versions named. `expect` lets a joiner check its own result
  before the lobby does.
- **The package is local.** It holds bytes derived from the disc
  (transplanted tags, patched geometry), so it is never uploaded. Its
  manifest has one `[[forge]]` layer naming the document's hash, and
  `content::MapId` gains a `forge` list in its canonical form (manifest
  format 2). Two machines have the same map when they have the same
  base, sources, document and builder.
- **Rebuild in a session.** The host announces a rebuild in the log.
  Every machine materialises the document as of that log position, the
  session saves each player's position and body, the map loads again
  from the new package, and play resumes. The existing reload path is
  `map.reload` (live-mirror.md slice M9).

### Who owns state

The sim rewrite's rule holds ([sim-rewrite.md](../engine/sim-rewrite.md)
section 2): guest memory is the persistent truth for everything retail
has a home for. Forge follows it.

| State | Kind | Where |
|---|---|---|
| Objects, placements, tag fields, netgame flags, device groups, squads | guest truth | the arena and the tag data, written through engine entries and `hot-tags` |
| Emitter timers, logic node state, barrier switches, volume occupancy, the object-to-op link, who holds what, budgets | **Extension** | `ForgeExt` in the `SimExt` blob: saved with checkpoints and snapshots, carried in films, folded into the sim hash |
| The document and the log | session state, outside the sim | the Forge extension, on every machine |
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
proposal:  {author, base_seq, op, was?}        peer -> host
entry:     {seq, tick, author, op, prev_hash}  host -> every peer
```

The shared `host-log` crate handles entries, the hash chain and the head
([ai-networking.md](ai-networking.md), "How it works"). The multiplayer
integration uses `host_log::Entry<Op>` with the authenticated author set.
Its canonical bytes and SHA-256 match the authored F1 entry.

- The host validates a proposal (schema, permission, budget, bounds,
  `was` against what it holds), assigns the next `seq` and an apply
  `tick` a few ticks ahead, and sends the entry to every machine on a
  reliable ordered channel. A refused proposal is answered to its author
  with the reason and never enters the log.
- Each entry hashes its predecessor. Machines report their log head in
  the session's keep-alive; a head that differs ends that machine's
  session with a named error.
- Each machine applies an entry at the start of `tick`, in `seq` order.
- **Holding.** Picking an object up is a `hold` entry; the host refuses a
  second hold. Moves of a held object are sent at most once a tick and
  only the last one before the drop is kept in the document.
- **Undo** is the inverse op proposed by the same author. The log only
  grows; the document is the log folded.
- **Join in progress.** The newcomer gets the document as of a log
  position and the entries after it, materialises, then joins.
- **Roles.** The host gives each player a role: editor, player or
  spectator. Tools can be allowed per role.

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
- A film of an editing session records each log entry as a tagged record
  at its tick (a new tag, "Forge entry"), beside the inputs. Playback
  applies them as the session did. `simulation_hooks` covers Forge's
  hooks, so a film records whether Forge was installed.
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

## Features

Size is the code a slice needs with its tests: **S** under 800 lines,
**M** under 2,500, **L** more. "Game runs" counts the things only a run
can settle.

| Feature | Feasibility | Size | Rests on | Game runs |
|---|---|---|---|---|
| Monitor mode, objects, save | high | L | live-mirror M8, the director fly camera ([theater.md](theater.md)) | few |
| Teleporters, buttons, triggers | high for teleporters and triggers; medium for device groups | M | the kit; `ForgeExt` | device groups |
| Projectile emitters | high alone and synchronous; host-replicated by shared schedule in F15 | M | `SimExt`; a trace of projectile creation | replication |
| Invisible barriers | high for new ones; low for retail ones before collision writes | M, then L | `collision-build` | scale, collision groups |
| Nav mesh | high for bots; medium for AI sectors | M | `bots::autonav`, `pathfind` | AI on generated sectors in multiplayer |
| Monitor bodies (Forge sessions only) | medium | L | transplant, tag headroom, player control of a flying biped | several |
| Spawn AI | medium synchronous; host-replicated as puppets in F15 | L | transplant, `h1campaign`'s squad writers, the synchronous probe | several |
| Geometry in game | block-out high; face push and pull medium (F13); full vertex editing on a pad medium, after it (F16) | L, then L | modeling slices 1 to 4, 7, 8, 11 | every write class |
| Textures in game | medium | M | modeling slices 5, 6 | cache behaviour |
| Character import | medium for a skin on a chassis; low for own animation | L | the `mode` writer, `assets::encode` | every imported kind |
| Sharing on Live | high | M | the content store, `MapGate` | one deploy check |
| Custom models on Live | high | M | the content store, Live Admin | one deploy check |
| Host-replicated mode | medium; mirrors high, puppets medium | L | F3's results, `agent_proto::live` | several |

Each feature's mechanism, options and open engine points are in
[forge-features.md](forge-features.md).

## The UI

Menus are native screens from the UI builder ([ui-builder.md](ui-builder.md)):
each a `Page` with table tests, drawn offline by `h2 ui preview`, held
to the kit's checks. A filter or choice shows its value in the second
column and A opens a picker over it; no row steps sideways through
values.

**Getting in.** **FORGE** on the main menu, below **THEATER**
(`layout_glue.rs`). It opens the Forge lobby: the pregame lobby's skin
with rows **Map**, **Forge map** (New, or a saved or downloaded one),
**Players can edit** and **Start**. Published maps also appear under
**Change Map > Forge** in Game Setup ([map-select.md](map-select.md)).

**In a session** there are three layers.

1. *The HUD strip*, always up while a monitor. A new builder kind,
   `Hud`: the same `View` widgets (`Text`, `Art`, `Legend`), no list and
   no focus, drawn by the compositor from the preview's own renderer
   with the map's fonts. It shows the tool, what is aimed at or held,
   the budget, and a legend that changes with the state.
2. *Dialogs*, `Dialog` kind, `Over`: the palette, an object's
   properties, a link's ports, the Forge menu. The world stays visible.
3. *The scene*: ghost, gizmo, link lines, volumes, nav and barrier
   overlays.

```text
 OBJECTS                                              BUDGET 412 / 1000
 Barrier, 4 x 4 x 1                                       OBJECTS 96 / 640



                                 +


 (A) Place   (X) Palette   (Y) Properties   (B) Delete   (LB)(RB) Tool
```

```text
 +----------------------------------------------------------+
 | LINK                                                     |
 | From        Button 3              pressed                |
 | To          Teleporter B          enable                 |
 | Behaviour   Toggle                                       |
 | Delay       0.0 seconds                                  |
 |                                                          |
 | (A) Change   (X) Remove link   (B) Done                  |
 +----------------------------------------------------------+
```

| Action | Controller | Keyboard and mouse |
|---|---|---|
| Monitor or player | D-pad Up | Tab |
| Fly; up, down | Left stick; RT, LT | W A S D; E, Q |
| Look | Right stick | Mouse |
| Pick up, drop, confirm | A | Left click |
| Palette | X | F |
| Properties of the aimed thing | Y | R |
| Delete (hold); cancel; back | B | Delete; Escape |
| Turn a held object | hold RB and move the sticks | hold right mouse button |
| Tool: Objects, Links, Volumes, Emitters, AI, Shape, Paint, Nav | LB, RB | 1 to 8 |
| Start a link from the aimed thing | D-pad Right | L |
| Undo, redo | D-pad Left; hold for redo | Cmd+Z, Shift+Cmd+Z |
| Overlays | Back | H |
| Forge menu | Start | Escape at the top |

The Forge menu: **Save**, **Save as**, **Rebuild map** (with the count
of pending edits), **Generate nav**, **Players** (roles), **Map
settings** (title, description, budget), **Publish**, **End session**.

Rules: every pending Rebuild edit shows the same badge in the HUD and in
its dialog; a refused proposal shows its reason in the HUD strip for a
few seconds; nothing in a dialog needs the keyboard except names; copy
is sentence case in rows and capitals in titles, as retail.

## Interfaces

**Agent API** (game). New namespace `forge`, in
[agent-api.md](../runtime/agent-api.md) when built. It is added because
every Forge test, lane and later tool drives a session through it.

| Method | Access | Does |
|---|---|---|
| `forge.status` | read | session, role, log head, pending Rebuild count, budget |
| `forge.doc` | read | the document as of the log head |
| `forge.propose {op, was?}` | act | one proposal; the entry or the refusal |
| `forge.log {since?}` | read | entries after a position |
| `forge.rebuild` | act | the host's rebuild; `{map_id, seconds}` |
| `forge.capacity` | read | tag headroom, object pool use, each block's count and limit, kit pools left |
| `forge.nav {generate?}` | act | build state, counts, problems |
| `forge.probe {what}` | act | one of the F3 measurements |

**Editor API.** `mirror.edit` in a Forge session routes the editor's
actions to `forge.propose`. `import.*` (list, convert, check, remove)
for bundles, a provider in the studio's `api`.

**CLI.** `h2 forge build <doc>` (materialise), `h2 forge check <doc>`
(validate), `h2 import-asset <file> --chassis <key>`.

**Live.** `ForgePublished` and its routes, in [live-api.md](../live/live-api.md)
when built; `NewSession.map_id` is unchanged.

**Engine entries** Forge needs beyond those the agent API has, each
proven by a differential test before use: the device group setter, the
squad place entry, the unit swap for a body, the damage entry for a kill
sink.

## Rules and limits

- A document holds no bytes from any game and no imported asset. The
  validator has no type that could carry them. An imported model is a
  separate `model-bundle` blob under its own check.
- Recipes name disc tags by key and source hash. A machine whose disc
  lacks a source is refused by name before the map loads.
- An op is validated before any guest write: finite floats, positions
  inside the BSP's world bounds, a tag that exists with a group the
  block accepts, counts inside engine limits and the budget. A proposal
  that fails changes nothing.
- No edit outside a Forge session; no Forge session on a hopper or
  ranked session; no edit in Theater.
- The Forge step has a time budget a tick. Emitters and the graph skip
  work past it and say so.
- Forge files, bundles and materialised packages are user output and
  stay out of Git checkouts.
- A Rebuild is refused while any player holds an object.
- A monitor body exists only in a Forge session. A published map played
  as a match gives every player their own biped; the recipe for a body
  is left out of a map materialised for play.
- No bundle is uploaded from an account without `model_uploads`, in
  either home.

## Risks

| Risk | Effect | Answer |
|---|---|---|
| Tag headroom is small on multiplayer maps | monitor bodies and AI may not fit; one map has none | F3 measures it; the cut order; a body recipe names its cost up front |
| Host-made objects may not replicate, by group | more of F15 falls to Forge's mirrors | F3 measures; synchronous is built first and needs none of it |
| The synchronous protocol may not hold a multiplayer map | F5 cannot ship; F15 moves ahead of it and AI waits for puppets | F3, before F5 is built |
| `SimExt` is not built | no emitters, triggers or logic | F7 builds the blob with its first user |
| The modeling crates are design | geometry and paint cannot start | block-out needs only `collision-build`; the rest follows modeling's own slices |
| A game that loads a map with added tags is unproven on retail bases | every recipe | map-builder.md, "Not verified"; F10's first run is one transplanted scenery tag |
| Materialise time grows with a document | long holds on Rebuild | cache by op-prefix hash; measure in F2 |
| A shared document is hostile input | crashes, resource exhaustion | the validator's caps; fuzzing in F1; the Forge step's budget |
| Users import assets they have no right to | legal exposure for Live | uploads only from admin-approved accounts, with a statement; reports, the Models queue, takedown by hash, revocation; no extractor ships |
| Mirrors and puppets drift or cost bandwidth | rubber-banding, a low actor cap | corrections only on movement; a per-map cap on mirrored objects and actors, measured in F15 |
| Vertex editing on a pad is slow to use | F16 disappoints | the face tier ships first and covers most shaping; F16 is designed around loops, snapping and a radial menu, and the paired editor stays available |

## Slices

Lanes write code and tests on invented data; the coordinator runs them
and the game. Sizes as under Features.

| # | Slice | Size | Needs | Acceptance |
|---|---|---|---|---|
| **F1** | `forge-doc`: the document, placement and tag-field ops, canonical form and hash, validator with caps, fold of a log into a document, inverse ops | M | nothing | golden vectors for the hash; fold then unfold through inverses gives the start; every validator rule has a refusing case; a seeded fuzz of 10,000 random documents never panics; applying placement ops to a `scenario_edit::fixture` equals the same edits made through `Session` |
| **F2** | `forge-build`: document to package, the `[[forge]]` layer, `MapId` format 2, `h2 forge build` and `check`, the kit's spare entries | M | F1 | two builds of one document are byte-equal; an empty document on a kit map differs from its base only in the kit's blocks; a stale base hash is a named error; `MapId` golden vectors, format 1 ids unchanged; the package resolves through `content::resolve` to the built map |
| **F3** | Probes: `forge.capacity`, `forge.probe` (replication of a host-made object by group, the synchronous protocol on a multiplayer map, the AI flag in multiplayer, kit entries inert) | S | nothing | each method answers over a fake game with a test; the coordinator's runs go in `docs/game/notes/forge-probes.md` |
| **F4** | The session, local: the camera body, the `Hud` kind, Objects tool, palette and properties dialogs, save and load | L | F1, F2, live-mirror M8 | `Page` tables for each dialog; the `Hud` view passes the kit's checks in every state; over a fake game, place, move and delete produce the expected proposals and engine calls, undo restores, a saved file folds to the same document |
| **F5** | Synchronous mode: the log on the network, roles, holding, join in progress, the film record | M | F3's results, F4 | two fake peers and a host converge to one head under reordered proposals; a second hold is refused; a late joiner's document equals the host's; a film with entries replays to the same `ForgeExt` hash |
| **F6** | Teleporters and retail links: the Links tool, channels, device groups | M | F4 | a linked pair writes two flags with one identifier on a fixture; the bots' generator finds the link; unlink restores the bytes |
| **F7** | `SimExt` and `ForgeExt`: volumes, the graph, emitters | L | F4; sim-rewrite's blob hooks | the blob round-trips and enters the hash; a graph fixture's outputs per tick match a table; one, two and eight workers give the same bits; an emitter fires on the scheduled ticks and stops at its limit |
| **F8** | Barriers and block-out: the kit's generated tags, the Volumes and Shape tools' object tier | M | F2, modeling slice 8 | each generated `coll` passes `Bsp::validate` and random segment tests against brute force; a barrier's enable input switches it over a fake game |
| **F9** | Nav: Forge objects as generator input, **Generate nav**, the overlay, problems | M | F4 | a fixture with a block-out ramp gains the sectors and link that make a ledge reachable; a spawn off the mesh is reported |
| **F10** | Transplant recipes and monitor bodies | L | F2, F3's headroom | on invented maps, a transplanted closure reads back and references resolve; a recipe over the headroom is refused with its cost; the census and the body runs go in notes |
| **F11** | AI: squads, the AI tool, sectors at materialise | L | F7, F10, a synchronous session from F5 | squad blocks on a fixture read back through `h1campaign`'s decoder; a wave graph places squads on the table's ticks |
| **F12** | `asset-import`: glTF props, then skins on a chassis, then FBX | L | modeling slices 2, 6, 8 | invented glTF files in, tags out that `tags` decodes to the same mesh within quantisation; a mapping that drops a chassis node is refused; a missing bundle materialises with aliases and equal tag indices |
| **F13** | Shape and Paint in game: instance moves, face push and pull, strokes, previews | L | F4, modeling slices 1 to 3, 5 to 7, 11 | a face pull over a fake game is one Mesh op and one preview; replaying the document gives the editor's bytes for the same ops |
| **F14** | Live: `ForgePublished`, the gate, the Forge lobby and browse rows, both homes | M | F2 | the server refuses a non-document and an oversize one; a joiner without the base is refused before load; the parity suite passes in both homes |
| **F15** | Host-replicated mode: the definition and world split, mirrors, shared schedules, actor puppets (the actor channel of [ai-networking.md](ai-networking.md), slices AN2 to AN5), the `ForgeExt` definition hash, the per-map capability mark | L | F5, F7, F11, F3's replication results, AN2 to AN5 | a fake host and two fake clients: a mirror follows its host object within one correction; an emitter's shots fall on the same ticks on all three; a client's `ForgeExt` definition hash equals the host's after reordered delivery; a puppet's pose matches the host actor's; a map with AI is refused in this mode while puppets are off |
| **F16** | Full vertex editing in game: vert, edge and face modes, loop and ring select, loop cut, merge and weld, bevel and inset, texture coordinates, on a pad and with mouse and keyboard | L | F13, modeling slice 4 | every pad gesture is one `MeshOp` equal to the editor's for the same edit; `Page` tables for the radial menu and each tool's options; a session's ops replay to the editor's bytes |
| **F17** | Custom models on Live: the `model_uploads` flag and request, the `model-bundle` kind, the upload and publish checks in both homes, reports with `third_party_asset`, the Models queue in Halo Admin, the gate's bundle download, **Show other players' models** | M | F12, F14 | each refusal in the check's table, in both homes through the parity suite; approval and revocation replay from the log; three reports hide a bundle; a taken-down bundle makes a joiner draw the chassis; an unapproved account's publish naming a bundle is refused |

F1, F2 and F3 are ready to hand out and touch no existing behaviour.
F1 and F3 start together; F2 follows F1. F4 waits on live-mirror M8.
F8's generators, F12's glTF props and F14's server half can run beside
F4. The two network modes are F5 then F15; geometry is F13 then F16;
F17 follows F12 and F14.

**F1 in detail.** A new crate `crates/formats/forge-doc`, depending on
`content` (canonical JSON, hashing) and `serde`. Types: `ForgeDoc`,
`Op`, `OpId`, `Recipe`, `ImportSlot`, `Entry`, `Proposal`, `Refusal`.
Functions: `ForgeDoc::id`, `validate(&ForgeDoc, &Limits)`,
`fold(&ForgeDoc, &[Entry])`, `inverse(&Op, &Before)`. No dependency on
`scenario-edit`: the apply side is a trait, `Apply`, that
`forge-build` implements over `scenario_edit::Session` and the runtime
over the guest. Out of scope: mesh, bitmap and Forge-family ops beyond
their type names, which later slices fill.

**F3 in detail.** Methods in `crates/extensions/agent` with their
`agent-proto` types and fake-game tests. `forge.capacity` reads the two
arena cursors scenario-editor.md section 6 names and the block counts.
`forge.probe {what: "replication", group}` makes one object of the
group on an `mp_host` and returns its id for the client's `sim.objects`
to be asked for. `{what: "sync"}` reports the protocol and the hash
series of a session started on a multiplayer map with no variant.
`{what: "ai"}` reports the AI flag and the actor count. The
replication and AI questions can also be read with `net.replication`
and `world.ai` ([ai-networking.md](ai-networking.md), probes P1 and P2).

## Decisions

The owner's choices, which the sections above follow.

1. **Networking.** Synchronous first, then a host-replicated mode as
   well. Both are planned: F5, then F15.
2. **Imported models.** They may be shared on Live, but only by accounts
   an admin has approved, with a report path and review in Live Admin
   ([forge-features.md](forge-features.md), "Custom models on Live").
3. **Geometry on a pad.** Block-out with face push and pull first (F8,
   F13), then full vertex editing in game (F16).
4. **Monitor bodies.** Forge sessions only. They are not player models
   in ordinary matches.
5. **forge.md** is the retail reference only; the design is here.

## Evidence

- **E**, **V**: the placement blocks, strides, per-type limits, the
  netgame equipment spawner and the object pool are in forge.md 1.2 and
  scenario-editor.md sections 1 and 6.
- **E**: the protocol choice by variant engine, live-coop.md 1.1.
- **E**: AI update gating and the absence of AI data in multiplayer
  scenarios, bots.md 2.4.
- **Code**: the session rules for writes, `SessionKind::writes` in
  `crates/extensions/agent-proto/src/sim/mod.rs`.
- **Code**: `bots::autonav::build_map` (`crates/live/bots/src/autonav.rs`)
  and the generator version `cache::GENERATOR`.
- **Code**: `Converted::install` puts a built tag into a `TagSet`
  ([models.md](../halo1/models.md), "API"); converted Halo 1 maps built
  this way load and play ([map-builder.md](../formats/map-builder.md)).
- **Design**: the document, recipes, the kit, the log, the two network
  modes, `ForgeExt`, the chassis rule, the sharing table, the model
  upload check.

## Not verified

- Everything marked **S** here and in
  [forge-features.md](forge-features.md). The first to settle, in F3: tag
  headroom on each multiplayer map; which object groups replicate from a
  host; the synchronous protocol on a multiplayer map; the AI flag in
  multiplayer; that kit entries are inert.
- That the game loads a retail-based map with added tags and a moved
  tag index.
- That a player can be given a flying biped and steer it.
- Projectile creation outside weapon fire: velocity, owner, kill credit.
- Device groups driving machines in a multiplayer session.
- Mirrors and puppets in a distributed session: that a client-made
  object the host did not replicate is left alone by the engine's
  replication, and the bandwidth of corrections and poses.
- Whether a placement's scale reaches collision and physics.
- Materialise and nav build times on real maps.
- Every in-game screen and the `Hud` kind against the game's own
  drawing.

## Related

- [forge-features.md](forge-features.md): each feature in detail.
- [forge.md](forge.md): retail evidence.
- [scenario-editor.md](../editor/scenario-editor.md),
  [modeling.md](../editor/modeling.md),
  [modeling-targets.md](../editor/modeling-targets.md),
  [live-mirror.md](../editor/live-mirror.md): the editor's commands,
  writes and live apply.
- [maps.md](maps.md): packages, `MapId`, sharing.
- [sim-rewrite.md](../engine/sim-rewrite.md): state kinds and `SimExt`.
- [film-format.md](film-format.md), [theater.md](theater.md): films and
  the director camera.
- [bots.md](bots.md), [pathfinding.md](../formats/pathfinding.md): the
  two nav meshes.
- [ui-builder.md](ui-builder.md), [ui-screens.md](ui-screens.md): the
  screens.
- [live-coop.md](../live/live-coop.md): the synchronous protocol.
