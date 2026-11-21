# Forge: the features

Status: Design. Nothing here is built.
Code: `crates/extensions/forge` (the tools and the Forge step), `crates/tools/forge-build` (recipes and the kit), `crates/tools/asset-import` (glTF and FBX to tags). Tests: each crate's own, on invented data.
Switch: the `forge` feature of `H2_FEATURES`, as [forge-design.md](forge-design.md).

What each Forge feature is made of, its options and the engine points
still open. [forge-design.md](forge-design.md) holds the architecture
these rest on (the document, the apply classes **Live**, **Kit** and
**Rebuild**, the kit, `ForgeExt`, the edit log, sharing), the table of
feasibility and size, and the slices. The rule that must hold is that
page's: nothing a feature saves or shares carries a byte from any game.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## Monitor mode and bodies

The editing player toggles between their own biped and a flying editor
body. Bodies exist in Forge sessions only: a published map played as a
match gives every player their own biped, and its package is
materialised without the body recipes. Two bodies exist in the design:

- **The camera body** (slice F4). The player's biped is parked
  (invulnerable, hidden, input cut) and their view is the director's
  flying camera, driven from the pad each frame (theater.md, "Free
  camera"). No tag is needed and nothing reaches the sim but the park.
  Other players see a marker drawn in the scene at the camera.
- **A real body** (slice F10): a Sentinel, 343 Guilty Spark or 2401
  Penitent Tangent. These are flying bipeds on campaign maps of the
  user's disc (**S**: that each is a `bipd` and where; the census in
  F10 finds them by group and shape). A `transplant` recipe brings one
  into the map, and the player's unit becomes it. Other players see it,
  it collides, and it can be shot if the host allows.

Options from what the disc has (**S** until the census):

| Option | Source | Class |
|---|---|---|
| Body | the Sentinel variants and the monitor model on campaign maps | Rebuild (a recipe) |
| Guilty Spark or Penitent Tangent | the same model; the eye and glow differ by a variant, a permutation or a shader, to be read | a `variant` recipe |
| Eye and glow colour | the player's own colours, where the model takes change colours | Live |
| Voice | the lines the campaign maps hold for each character, found by reference from that map's tags; none, Spark, Tangent, Sentinel sounds | Rebuild; played by a button as an emote |
| Tool beam | the Sentinel's beam effect, drawn from the body to a held object | presentation |

Open engine points: how the player's unit is set to a body the globals
do not list (the look byte allows four models, **Code**,
[agent-api.md](../editor/agent-api.md), "What there is to build on";
routes are another entry in the globals' list or attaching the player to
a spawned unit); whether pad input drives a flying biped's height (**S**;
a recipe map in [test-maps.md](test-maps.md) with the swap made is the
test); the sound gestalt merge a transplanted voice needs
(tag-repo.md, "Per-map couplings").

## Objects

The palette is the map's own tag index by group (weapons, vehicles,
equipment, scenery, crates, machines, controls), then the kit's
block-out set, then **My imports**. Labels are tag base names read from
the user's map at run time. Placing is: aim, a ghost follows the ray
cast against collision ([collision-queries.md](../engine/collision-queries.md)),
A confirms, and the host makes the object and adds the placement. Per
object: respawn time, team, game types, placed at start, physics
(fixed, normal), snap (off, grid, surface, 15 degree turns).

Budget: the object pool, each block's limit, the tag headroom and a
Forge cost per item (**Design**; the limits are the engine's, the cost
is ours and keeps a map under them with room for a match's own
objects).

## Teleporters, buttons and triggers

Everything linkable has **ports**. A link joins an output to an input.

| Thing | Outputs | Inputs | Made of |
|---|---|---|---|
| Teleporter sender, receiver | used | enable | netgame flags and a channel; a link between two is the channel |
| Button | pressed, released | enable | a control from the map's palette in a device group, else a kit marker with an interact volume |
| Trigger volume (box, cylinder, sphere) | entered, left, occupied; filter by team and object kind | enable | `ForgeExt`; tested each tick against `SimRead::objects` |
| Timer, counter, latch, and, or, not, delay, random | fired | start, stop, reset | `ForgeExt` |
| Game event | round start, score, kill, flag taken | | `sim.events` |
| Machine, door, lift | arrived | open, close, power | a device group |
| Emitter, barrier, squad, kill zone, mover, message | | enable, fire, place | below |

The graph is data, evaluated by the Forge step with a fixed budget of
node visits a tick; what does not fit runs next tick, in id order.
`random` draws from a generator seeded in `ForgeExt`, never the game's.
A sink acts through an engine entry on the host (teleport, device group
power, make, delete), so its result is ordinary game state. Where a
retail mechanism is the whole behaviour (a teleporter pair, a button
wired to one door) the link is written as retail data and the graph
holds no node for it: that link works in any session and costs nothing
a tick.

## Projectile emitters

An emitter is a `ForgeExt` record: the tag (a projectile, a grenade, any
object the map holds), position, direction with a cone, speed, period,
burst count, phase, a limit on how many of its objects are alive, a team
for kill credit, and an enable input. The Forge step makes its objects
on schedule from the tick count, so two machines with the same record
fire on the same tick.

Limits: a per-emitter and a per-map rate, and a share of the object pool
it cannot exceed; an emitter over its limit skips a shot and shows a
warning in the editor.

Open: which placement fields seed a projectile's velocity and owner when
it is made outside a weapon's fire (**S**; the fire path is hooked for
telemetry, [film-format.md](film-format.md), "Weapon fire, damage and
projectile endpoints", and is the place to read). In the
host-replicated mode every machine fires an emitter from the shared
schedule and the host decides damage ([forge-design.md](forge-design.md),
"The edit log and multiplayer"), so nothing rests on the engine
replicating a projectile made outside weapon fire.

## Invisible barriers

- **New barriers** are kit objects: a box with collision and physics and
  no render model. They are placed, moved and turned like any object
  (Live), drawn only in the editor as a tinted volume, and take an
  enable input. Options: blocks players, vehicles, projectiles
  (**S**: which collision groups each maps to, to be read in the `phmo`
  and `coll` material flags). Sizes come from the kit's ladder; an exact
  size is a `generate` recipe and a Rebuild. Whether a placement's scale
  reaches collision is **S**, and would remove the ladder.
- **The map's own barriers** are collision surfaces with no drawn
  counterpart. The editor overlay shows them (collision surfaces whose
  material is not in any render part near them, **Design**). Removing or
  moving one is a structure collision write (modeling slice 11 or 13)
  and a Rebuild.
- **Kill zones** are trigger volumes with a kill sink; the scenario's own
  kill triggers show in the same overlay.

A later option for switchable structure collision: the engine skips a
breakable surface once its bit is cleared (**E**,
[collision.md](../formats/collision.md), "Breakable mask"), so a barrier
compiled into the level as a breakable surface could be switched Live.

## Nav mesh

Two consumers, two meshes.

- **Bots.** `bots::autonav::build_map` builds a mesh for each BSP from
  collision and the scenario, cached by map fingerprint, with teleporter
  and lift links read from the netgame flags and placements (bots.md
  3.2). A materialised Forge map has its own fingerprint, so the
  existing boot scan (`bots_nav_auto.rs`) builds it. In a session,
  **Generate nav** runs the same build off the game thread over the
  current document, with one new input: the collision of fixed Forge
  objects (block-out pieces, barriers that block players, fixed
  scenery), merged as walkable surfaces and obstacles. Teleporters and
  lifts placed in Forge are in the scenario, so they route with no new
  code. The mesh is host-side state; bot actions are inputs, so no
  machine needs the same mesh.
- **AI.** Actors walk the sectors in `sbsp` ([pathfinding.md](../formats/pathfinding.md)).
  `pathfind::generate` writes them at materialise time when the map has
  squads. This is sim data: the generator's version is in `builder`.

The overlay draws sectors, links by kind, and problems: a spawn off the
mesh, an island no link reaches, an objective a team cannot path to.
Dynamic things (doors, movers, switched barriers) have no timed edges;
a bot recovers by replanning, as on the retail maps with moving parts.

## Spawning AI

A squad is a scenario block entry with a character, a count, a start
position and orders. Characters and their bipeds, weapons and dialogue
are tags on campaign maps, brought in by `transplant` recipes. The
blocks are written by the code that converts Halo 1's encounters
(`crates/tools/h1campaign`, [ai.md](../halo1/ai.md), "Halo 2: the
scenario's AI blocks and `char`"). In a session the kit's spare squad
slots make "place a squad" a Kit edit; a character type the map lacks is
a Rebuild. A squad takes a place input, so a trigger or a timer can
spawn waves.

What must hold: AI updates only when the AI flag is set and the game
engine allows it (**E**, bots.md 2.4); multiplayer scenarios carry no AI
data, so the kit adds all of it; the session is synchronous, or
host-replicated with the actors as puppets once slice F15 is built. Costs to
measure: a character's closure against the tag headroom, the sound
gestalt merge, and the tick time with actors (the phase 0 census of
sim-rewrite.md).

## Geometry, textures and models

The kernels are the editor's: `mesh-edit`, `geometry-edit`,
`bitmap-edit` and `collision-build` are crates with no window
(modeling.md), and Forge links them into the game.

| Tier | In game with a pad | Class |
|---|---|---|
| Block-out | kit pieces placed, scaled by swapping within the ladder, given a material | Live |
| Instances and models | move, turn and delete instanced geometry; swap an object's material or permutation | Live preview, Rebuild to keep (instances); Live (objects) |
| Faces | pick a face, push or pull it (`ExtrudeFaces`, `Transform`), delete, set material, planar texture coordinates | Live preview, Rebuild for collision |
| Paint | brush, fill, colour pick on the texture under the reticle | Live picture; saved as strokes |
| Vertices, edges, loops, welds, texture coordinates (slice F16, after the tiers above) | the full `MeshOp` set, below | Live preview, Rebuild for collision |

The order is block-out (F8), then instances, faces and paint (F13),
then full vertex editing (F16).

**Vertex editing in game.** Edit mode is entered on the aimed target
(a cluster, an instance definition, a model section) and shows its
wireframe.

- *Modes*: vert, edge, face, switched on the D-pad; the reticle picks
  the nearest element within a screen radius, A toggles it, holding A
  and sweeping paints a selection, X held draws a screen box.
- *Select more*: loop and ring from the aimed edge, linked, more, less,
  by material.
- *Move*: with a selection, the left stick moves it on the view plane
  and the triggers along the view axis; D-pad Left and Right lock an
  axis or the face normal; a grid step and vertex snapping are on by
  default, and Y opens the exact value in a picker.
- *Tools*, a radial menu on LB: extrude, loop cut (with a count), merge,
  weld by distance, dissolve, fill, bevel, inset, flip, set material,
  project texture coordinates.
- Each gesture is one `MeshOp` proposed to the log, the same op the
  editor makes, so undo, replay and the saved document are the editor's.
- Mouse and keyboard use the editor's Blender-style keys.

The paired editor stays available. `h2-editor` paired with a game in
a Forge session proposes its `Action`s to the host's log like any
player, so one friend can fly as a monitor while another works with a
mouse. This replaces the paired editor's own write rule for Forge
sessions only: outside one, live-mirror.md's "Networked games" table
stands.

A preview is the edited mesh or texture handed to the native renderer
under the target's id; the tick never sees it. Until the Rebuild, the
edited faces are drawn with a pending tint and still collide as before.
Lightmaps of changed faces are stale until relit (modeling slice 12);
block-out pieces are lit as objects.

## Character import

The user converts their own files to Halo 2 tags on their own machine.
Input is an interchange file: glTF 2.0 (`.gltf`, `.glb`) first, FBX
second. There is no reader for any game's archive or native format, and
no tool here extracts from another game.

The import runs in `h2-editor` (an Import tool) and as `h2 import-asset`;
bone mapping and file dialogs need a mouse. The in-game palette lists
the results.

| Kind | Result | Notes |
|---|---|---|
| **Skin on a chassis** (default for a character) | a `mode` and its `bitm` and `shad` tags, and a `variant` of a retail biped whose `hlmt` draws them | the retail biped supplies skeleton, animations, collision, physics and hitboxes |
| **Prop** | a `scen` or `bloc` with `mode`, `coll` and `phmo` | collision from the mesh or its convex hull |
| **Own animation** | `jmad` from glTF clips | later; a prop's idle loop first |

Steps for a skin: normalise (units, Z up, triangles, at most 65,535
vertices a section, split past it); map source bones to the chassis's
nodes by name, then by hand; re-bind the mesh to the chassis's bind pose;
reduce each vertex to the two strongest nodes with weights summing to
255 (the layout the `mode` writer emits, **Code**,
`crates/tools/h1-assets/src/models/mode.rs`); encode textures
(`assets::encode`, modeling slice 6); make each material a `shad` from a
short list of retail templates (opaque, alpha test, emissive) with its
base map replaced ([shader-instances.md](../formats/shader-instances.md)).

The output is a **bundle** under `<data>/imports/<sha256>/`: loose tags
in the repository's file format and a manifest (source file hash,
chassis, the user's note on where the file came from). Bundles are
outside Git. A bundle reaches Live only from an account an admin has
approved for model uploads, through the upload check, and can be
reported and taken down ("Custom models on Live", below).

Why a chassis: the sim reads the biped, its collision and its animation
graph, all retail and on every disc. The skin is presentation. So a
machine without the bundle runs the same sim and draws the chassis, a
film replays anywhere, and an imported character can be an AI character
or a monitor body by choosing that chassis. The slot plan keeps tag
indices equal on every machine: a missing bundle's `mode`, `bitm` and
`shad` slots are filled with aliases of the chassis's own (**S**: that
no sim path reads a render model; the node-matrix and marker paths read
the `mode`'s nodes and markers, so an import keeps the chassis's nodes
and marker groups exactly, and the importer refuses a mapping that drops
one).

A prop is different: its collision and physics are its own, so the sim
needs the bundle. A document with a prop slot is joinable only by a
machine that holds the bundle or can download it; otherwise the join is
refused by name before the map loads.

## Custom models on Live

Only an account an admin has approved may upload content that carries
custom models. Everyone else's imports stay on their machine.

**The flag.** `model_uploads` on the account, off by default. An admin
sets it with `PUT /v1/admin/model-uploads/{gamertag}` `{approved, note?}`
(token `lxm` `content.moderate`), a server-signed `ModelUploadsSet` event
on the account's stream, beside `ContentUploadsSet`
([live-admin.md](../live/live-admin.md) section 7), so it replays from
the log. A player asks for it from the dashboard
(`POST /v1/account/model-uploads/request` `{statement}`, a
`ModelUploadsRequested` event); the request appears in the admin's
queue. Turning the general upload switch off also stops model uploads.

**What is uploaded.** A bundle is one blob of kind `model-bundle`: the
loose tags the importer wrote (`mode`, `bitm`, `shad`, `coll`, `phmo`,
`jmad`) and its manifest, with a preview picture. It never holds a tag
copied from a disc: a skin's biped is a `variant` recipe in the
document.

**The upload check**, the same in both homes. It lives in one function
of `live-proto` that the local server and the Worker both call, over the
store trait each home implements, and the parity suite runs every case
below against both.

| Step | Refused with |
|---|---|
| The uploader's `model_uploads` is on, read from the log at upload time | 403 `model_uploads_not_approved` |
| The blob parses as a bundle: only the listed groups, every tag decodes, counts inside the importer's limits, the store's size cap | 422 `not_a_bundle` |
| Its hash is not blocked and not on the retail list | 410 |
| The upload carries the author's statement that the work is theirs or licensed to share | 422 `statement_missing` |

`ForgePublished` and `MapPublished` are checked again at publish: a
document or package that names a hosted bundle is refused unless the
bundle is in the store, is not taken down, and its uploader is approved
at that moment. A `model-bundle` is the only way
tags reach the store outside a patch layer; a package may list bundles
in its manifest (`[[bundle]]`), and one that does goes through this
check. A patch layer cannot be inspected for a model, so mods shipped
as patches stay under the existing moderation alone.

**Report and review.** Any account reports a bundle, or a map that uses
one, with the existing `POST /v1/content/{sha256}/report` and
`/v1/maps/{id}/report`, with the reason kind `third_party_asset`. In
game the row is **Report model** on a map's page.

- The Halo Admin's **Custom content** tab gains a **Models** queue: each
  bundle with its preview, uploader, statement, the maps that use it,
  its reports, and a download for inspection in `h2-editor`.
- Actions are the existing ones (`hide`, `delete`, `block`) on the
  bundle's hash; a taken-down bundle answers 410 and every map that
  names it draws the chassis from the next join. **Revoke approval**
  turns the uploader's flag off and offers "hide everything this account
  uploaded".
- Three reports from different accounts hide a bundle until an admin
  reviews it (`ContentModerated` with `by` the server).
- The queue also lists pending `ModelUploadsRequested` entries with
  Approve and Decline.

**The player's side.** A setting, **Show other players' models** (on),
draws every skin as its chassis when off. It changes nothing in the sim.

## Not verified

The list is in [forge-design.md](forge-design.md), "Not verified"; every
**S** on this page is on it.

## Related

- [forge-design.md](forge-design.md): architecture, UI, risks, slices.
- [forge.md](forge.md): retail evidence.
- [modeling.md](../editor/modeling.md), [bots.md](bots.md),
  [ai.md](../halo1/ai.md), [models.md](../halo1/models.md): what the
  features reuse.
