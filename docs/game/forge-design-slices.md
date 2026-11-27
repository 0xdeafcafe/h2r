# Forge design: slices, decisions and evidence

The build slices, decisions, evidence and open items. Part of [forge-design.md](forge-design.md).

## Slices

Lanes write code and tests on invented data; the coordinator runs them
and the game. Sizes as under Features.

| # | Slice | Size | Needs | Acceptance |
|---|---|---|---|---|
| **F1** | `forge-doc`: the document, placement and tag-field ops, canonical form and hash, validator with caps, fold of a log into a document, inverse ops | M | nothing | golden vectors for the hash; fold then unfold through inverses gives the start; every validator rule has a refusing case; a seeded fuzz of 10,000 random documents never panics; applying placement ops to a `scenario_edit::fixture` equals the same edits made through `Session` |
| **F2** | `forge-build`: document to package, the `[[forge]]` layer, `MapId` format 2, `h2 forge build` and `check`, the kit's spare entries | M | F1 | two builds of one document are byte-equal; an empty document on a kit map differs from its base only in the kit's blocks; a stale base hash is a named error; `MapId` golden vectors, format 1 ids unchanged; the package resolves through `content::resolve` to the built map |
| **F3** | Probes: `forge.capacity`, `forge.probe` (replication of a host-made object by group, the synchronous protocol on a multiplayer map, the AI flag in multiplayer, kit entries inert) | S | nothing | each method answers over a fake game with a test; the coordinator's runs go in `docs/game/notes/forge-probes.md` |
| **F4** | Local session, its edits applied to the live objects through M8's calls: the camera body, the Objects tool, the object menu and Properties, save and load. **Built**, with its screens in the game (U4) and its HUD on the HUD kit (U3) | L | F1, F2, live-mirror M8 | `Page` tables for each dialog; the `Hud` view passes the kit's checks in every state; over a fake game, place, move and delete produce the expected proposals and engine calls, undo restores, a stale object is a named refusal that leaves the document alone, a saved file folds to the same document |
| **F5** | Synchronous mode: the log on the network, roles, holding, join in progress, the film record | M | F3's results, F4 | two fake peers and a host converge to one head under reordered proposals; a second hold is refused; a late joiner's document equals the host's; a film with entries replays to the same `ForgeExt` hash. **Built** (below), except the `ForgeExt` hash, which waits for F7; the co-op link offers its lanes (`game/src/forge_glue/link.rs`) and the game's run of it is unproven; the lobby still refuses Start Game with a remote member (`lobby_mode::REMOTE_REFUSAL`) until that run is proved ([notes/forge-probes.md](notes/forge-probes.md), "The synchronous Forge link in a real game") |
| **F6** | Teleporters and retail links: the Links tool, channels, device groups | M | F4 | a linked pair writes two flags with one identifier on a fixture; the bots' generator finds the link; unlink restores the bytes. **Built**, for the map's own flags and the flags a kit session adds (`link_tests`, `kit_tests`); the generator half is `bots::autonav::teleporter_flags` and `link_teleporters`, checked over a linked pair in `crates/tools/forge-build/tests/links.rs` |
| **F7** | `ForgeExt`: volumes, the graph, emitters. Not needed for a Firefight map | L | F4; sim-rewrite's blob hooks | the blob round-trips and enters the hash; a graph fixture's outputs per tick match a table; one, two and eight workers give the same bits; an emitter fires on the scheduled ticks and stops at its limit; a graph's squad sink makes the director's `SquadOp` |
| **F8** | Barriers and block-out: pieces from a definition in the document, the natural set, the dev textures, sizing on a grid, slots, the Structure and Natural groups. Its slices are BO1 to BO12 of [forge-blockout.md](forge-blockout.md) | L | F2, modeling slice 8; BO12's enable input needs F7 | each generated `coll` passes `collision_build::verify::check` and random segment tests against brute force; the same definition gives the same bytes twice; a barrier's enable input switches it over a fake game; the rest in that page's slice table |
| **F9** | Nav: Forge objects as generator input, **Generate nav**, the overlay, problems | M | F4 | a fixture with a block-out ramp gains the sectors and link that make a ledge reachable; a spawn off the mesh is reported. **Built**: the Nav tool draws the sectors and links in the game, and the bots take the generated mesh in a match ([forge-nav.md](forge-nav.md)) |
| **F10** | Transplant recipes and monitor bodies. Enemy classes do not wait for it: they are the AI ground's (FA6) | L | F2, F3's headroom | on invented maps, a transplanted closure reads back and references resolve; a recipe over the headroom is refused with its cost; the census and the body runs go in notes |
| **F11** | AI: Firefight maps. Markers and the ground build are **built**; the rest is slices FA1 to FA8 of [forge-firefight.md](forge-firefight.md): the lobby's pick of a map variant with ground, the wave set in the document, a wave test in the session, Map Waves, the Waves screens, enemy classes, the AI tool, other machines | L | F4; `firefight-kit`, the Firefight runtime | a variant with markers and a recipe materialises to the kit's build of the same labels (`crates/tools/forge-build/src/ground.rs`); each FA slice's named tests |
| **F12** | `asset-import`: glTF props, then skins on a chassis, then FBX | L | modeling slices 2, 6, 8 | invented glTF files in, tags out that `tags` decodes to the same mesh within quantisation; a mapping that drops a chassis node is refused; a missing bundle materialises with aliases and equal tag indices |
| **F13** | Shape and Paint in game: instance moves, face push and pull, strokes, previews | L | F4, modeling slices 1 to 3, 5 to 7, 11 | a face pull over a fake game is one Mesh op and one preview; replaying the document gives the editor's bytes for the same ops |
| **F14** | Live: `ForgePublished`, the gate, Game Setup's **Players can edit** row and the published maps under **Change Map > Forge** ([map-select.md](map-select.md)), prefabs as a shared kind, both homes | M | F2 | the server refuses a non-document and an oversize one; a joiner without the base is refused before load; the parity suite passes in both homes |
| **F15** | Host-replicated mode: the definition and world split, mirrors, shared schedules, actor puppets (the actor channel of [ai-networking.md](ai-networking.md), slices AN2 to AN5), the `ForgeExt` definition hash, the per-map capability mark | L | F5, F7, F11, F3's replication results, AN2 to AN5 | a fake host and two fake clients: a mirror follows its host object within one correction; an emitter's shots fall on the same ticks on all three; a client's `ForgeExt` definition hash equals the host's after reordered delivery; a puppet's pose matches the host actor's; a map with AI is refused in this mode while puppets are off |
| **F16** | Full vertex editing in game: vert, edge and face modes, loop and ring select, loop cut, merge and weld, bevel and inset, texture coordinates, on a pad and with mouse and keyboard | L | F13, modeling slice 4 | every pad gesture is one `MeshOp` equal to the editor's for the same edit; `Page` tables for the radial menu and each tool's options; a session's ops replay to the editor's bytes |
| **F17** | Custom models on Live: the `model_uploads` flag and request, the `model-bundle` kind, the upload and publish checks in both homes, reports with `third_party_asset`, the Models queue in Halo Admin, the gate's bundle download, **Show other players' models** | M | F12, F14 | each refusal in the check's table, in both homes through the parity suite; approval and revocation replay from the log; three reports hide a bundle; a taken-down bundle makes a joiner draw the chassis; an unapproved account's publish naming a bundle is refused |

| **P1** | Precision in `forge::tool`: the settings (movement snap, rotation snap, axes, fine movement) in the profile's store, thrust, Halo 3's manipulate (push, pull, roll), Coordinates with typed numbers and nudges, Reset orientation, Duplicate; their rows and the Coordinates screen; `forge.input` and `forge.status` carry them | M | F4, U4 | `tool_tests` tables: a held frame rounds to each grid in World and Local axes; the turn since pickup rounds and pickup never turns an object; a Coordinates visit is one `Placement::Set` and one undo step; a typed number outside the bounds is refused; Duplicate is one `Placement::Add`; `Page` tables for Tool settings and Coordinates |
| **P2** | Kit sessions (**built**: `forge::kit`, `forge::session::entries`, `map_variant::pick_kit`): a Forge-mode start materialises the draft with `forge_kit` and loads the package; netgame equipment entries, starting locations, netgame flags and teleporter flags as Kit edits; the Weapons, Vehicles, Equipment, Teleporters, Spawns and Goals groups; Team, Role and Order rows; goals listed by the session's game type | L | F2, F3's kit and capacity results, U2 | over a fake game on a kit fixture: an added entry raises the root count and writes the reserved row; a goal of a type the game type does not read is not offered; undo restores the count and the bytes; a map without the kit refuses the add with the reason; the materialised start resolves to the package |
| **P3** | Item properties: Respawn time; Game types and Place at start once their fields are decoded in [scenario-editor.md](../editor/scenario-editor.md) | S | P2 | a table of `properties(kind)`; each row's write is one typed `FieldWrite` with its `was`; a row whose field is not decoded is absent |
| **P4** (built) | Forge options opens Placed objects over `Session::objects` and Map limits over the live counts shared with `forge.capacity` | S | U4 | `Page` tables; browser group order and fly-to; a synthetic guest supplies pool and block counts to the read-only limits page |
| **P5** (built) | Selection (Properties and Hold A), groups with a local pivot picker and a lock, prefabs as local files, labels ([rules-engine.md](rules-engine.md#forge-labels)): one step of `forge_doc::FORMAT` for groups and labels, and a prefab file with its own number | M | P1 | golden vectors for the group ops and the prefab file; a pivot change or group move is one undo step; a member's removal is refused while grouped; a prefab naming a tag the map lacks is refused by name; the seeded fuzz covers the ops; a document of the older format still reads; gate, tool and picker tests cover the selection gesture and pivot choice |
| **P6** | Lights: a generated light piece in the kit, the Lights group, Colour, Size and Brightness as tag field writes | M | F8, P2 | the generated tags decode through `tags`; each property is one `TagWrite`; a light over the per-map cap is refused |

F2 and F3 can proceed independently of each other. F4 needs F2 and
live-mirror M8. F8's generators, F12's glTF props and F14's server half
can run beside F4. F11 needs neither F7 nor F10, and its first slices
(FA1, FA2, FA3, FA6) start at once. The two network modes are F5 then F15. Geometry is
F13 then F16. F17 follows F12 and F14. P1 and P4 follow the screens of
slice U4 and can run beside each other; P2 waits for F3's measurements;
P5 is the only slice that changes the document's format before F7. The
UI slices (U1 to U7, K1 to K4) are in
[forge-firefight-ui.md](forge-firefight-ui.md), "Slices". The slices
that edit the map's own entries and build map variants (M1 to M10, D1
to D5, V1 to V9, G1) are in
[forge-map-variants.md](forge-map-variants.md), "Slices", and their UI
(K5, H1 to H3, S1 to S6) in
[forge-map-variants-ui.md](forge-map-variants-ui.md), "Slices". F8's
slices (BO1 to BO12) are in [forge-blockout.md](forge-blockout.md),
"Slices", and the markers' icons and play forms (MK10 to MK16) in
[gametype-marker-forms.md](gametype-marker-forms.md), "Slices".

**F1 in detail.** `crates/formats/forge-doc` defines `ForgeDoc`, `Op`,
`OpId`, `Recipe`, `ImportSlot`, `Entry`, `Proposal` and `Refusal`.
It uses `content` for canonical JSON and hashing, and
`map::scenario_layout` for the editor's placement schema.
`ForgeDoc::id`, `validate(&ForgeDoc, &Limits)`, `fold(&ForgeDoc, &[Entry])`
and `inverse(&Op, &Before)` operate on portable edits. `Apply` is the
boundary for resolving tags, checking actual field types and enforcing
the map's budget. The materialiser implements it over
`scenario_edit::Session` and the runtime over the guest. The editor
is linked by the fixture parity test. Mesh, Bitmap and Forge-family
variants are reserved for later slices. The
[crate README](../../crates/formats/forge-doc/README.md) describes the
wire caps and API.

**F2 in detail.** `forge-build` materialises the placement and
tag-field ops and the session kit. It uses format 2 identity derived
from the base map hash and Forge document hash. Build and load check
`expect.map_id` against that identity. Forge packages contain the
canonical document and the locally built map. They resolve through
`content::Library::resolve_package` and require explicit selection. The
[crate README](../../crates/tools/forge-build/README.md) lists the CLI
and recipe contract.

**F3 in detail.** Methods in `crates/extensions/agent` with their
`agent-proto` types and fake-game tests. The [Forge probes](../runtime/agent-api.md#forge-probes) read the arena
cursors, object pool, block counts and candidate kit tails.
`forge.probe {what: "replication", group}` makes one object on an
`mp_host` so the client can check for it through `sim.objects`.
`{what: "sync"}` reports the protocol and tick hashes of a session
started on a multiplayer map with no variant. `{what: "ai"}` reports
AI flags and the actor count. `{what: "kit"}` gives a runtime census
alongside the block counts for comparison with the base map. The
[measurement procedure](notes/forge-probes.md) covers all four probes.
The replication and AI questions can also be read with `net.replication`
and `world.ai` ([ai-networking.md](ai-networking.md), probes P1 and P2).

**F4 in detail.** `forge_map` reads the active cache, validates the
base map and constructs the palette, placement counts, object budget,
BSP bounds and collision queries from that map. The palette takes a
scenario palette entry only when its tag is in the map, of the entry's
group, and an object the player can place and see: its definition's
object type is its group's, its model reference names an `hlmt` in the
map, and its bounding radius and offset are finite and at most 1024
units (`forge_map::placeable`). Sound scenery is left out: it has no body,
and the game adapter refuses its block. Each entry left out is logged as
`forge: the Objects palette leaves out <path>: <reason>` and listed in
`forge.status` as `palette_refused`. Names in one group that share a
base name carry the nearest folder that differs (`boulder (rocks)`), and
the full path only when that is shared too.

The Objects tool edits Forge placements. A held object is carried as
Halo 3's Forge carries one: in front of the camera, with the world around
it (`forge::tool::Extent`). A new object's bounding-sphere centre sits two
and a half of its reaches away (radius plus offset length, at least one
unit), pulled in to a nearer wall; a picked-up one stays at the distance
it was taken from. Neither comes nearer than its reach plus a quarter
unit, so the camera is never inside it and the near plane never cuts it.
The held offset is kept along the camera's forward, right and up, so the
object turns with the view and the camera can look all round. Holding
RT or the right mouse button manipulates it: the right stick turns it,
and the left stick's forward and back push it out from that distance
and pull it in (`forge::tool::PUSH_RATE`), from where it shows, so a
push stops at a nearer wall and a pull at the same near limit. In the native
renderer the gizmo shows the proposed frame's three axes
(`forge_glue::gizmo`), drawn as the HUD kit's `forge.gizmo` world marks
(forge-firefight-ui.md, "The monitor HUD"): the renderer has no
translucent preview of an object, and
the object in the world moves only when the player confirms. A drop that
changes position and rotation is one undo step. Properties exposes
heading and the rotation increment.

The monitor parks the local biped at the game tick
(`forge_controls::render::park`: invulnerable while parked) and hides it
from the main view by the "not drawn" exit at `OBJECT_RENDER_SETUP`
([occlusion-culling.md](../graphics/occlusion-culling.md), "Where to
skip"). The hook is ordered first, ahead of the native scene lists'
setup, so the biped has no layer-3 items and the native objects stage,
which draws only the walk of those items, never meets it. The biped stays
in the visibility list, so its shadow is still chosen as a caster.
The monitor's view is a playerless world view
(`spectator_view::replace_world_view` puts none in its player word, so
`VIEW_PLAYER` `0x004b9ed8` is none while it draws). The HUD draw calls the
game engine's black overlay (`HUD_BLACK_OVERLAY` `0x00159880`, from
`0x001396a0`), which draws a local user's spawn fade
(`[GAME_ENGINE] + 0xe4 + 4 * user`) and a view with no user at 1.0, opaque
black. The monitor keeps the HUD for its own, so the `private-view-overlay`
feature returns from the overlay while a private view draws and the view
has no user (`spectator_view::skips_black_overlay`). Every view drawn
through `spectator_view::replace_world_view` is one: the monitor, the
agent camera, the Pogo chase camera and Theater's cameras. A player's
own view keeps its index in `VIEW_PLAYER`, so the retail fade for a dead
or respawning player stays.
`forge.status` gives the parked unit as `parked`. Start Game in a Forge lobby opens the monitor with its HUD and tools.
A local level boot starts in the biped. D-pad Up or Tab switches between
the biped and the monitor.

The session's first monitor camera stands at
the map's first player start, 0.64 above it along its facing
(`agent_spawn_view::Starts`, as `render.spawn_view` places its eye): the
view the game draws as a Forge game opens is from before the biped
spawns, and a camera taken from it shows none of the map. Each later
switch to the monitor takes the player's own view
(`forge_glue::editing::monitor_camera`).

`forge::Session` validates edits against the loaded map and folds their
entries with F1. Its `World` interface resolves each placement to a
local object handle. The game adapter (`ForgeWorld` in
`crates/extensions/agent/src/forge/local.rs`) stages an edit's engine
work on a private copy of the session and queues it as one
`Call::Forge` for the next tick, on the call queue that `world.object.*`
uses. Before the first guest call the queue rechecks the session guard
and resolves every live object by kind and salt, as `world.object.set`
and `.delete` do. It then makes new objects with the ownerless spawn,
moves with the `world.object.set` body and deletes with the
`world.object.delete` body (`crates/extensions/agent/src/sim/calls.rs`).
A move, a delete, an own entry's removal and its restore make the same
guest calls as `world.object.set`, `.delete` and `.respawn`; the restore
also writes the entry's placement index, so its object is linked
(`crates/extensions/agent/src/sim/calls/tests/same_calls.rs`). The
session guard takes the place of those calls' session checks. Forge
opens only in an offline local multiplayer game
(`forge_map::admitted`), where `confirm_peers` and the Live refusals
never apply, and the guard's map generation and scenario do the work of
`epoch` and the cache identity. Edits in a network game go through F5's
log.
Undo and redo fold F1's inverse ops the same way: undoing a delete makes
a new object at the stored frame and binds its handle to the same
`OpId`. The adapter commits the candidate document and log only after
the engine answers. A refusal leaves both unchanged and shows its reason
as the HUD notice. A stale handle is refused as `forge_object_gone`, and
a block whose objects `world.object` cannot resolve (sound scenery) is
refused as `forge_object_kind` before anything is queued. Loading a
draft validates its base and builder and opens its folded document at a
fresh log head. The local session ends on a map change. Multiplayer
proposals, roles and film entries belong to F5.

**F5 in detail.** The protocol is `forge::net`, as "The edit log and
multiplayer" describes, with its tests in
`crates/extensions/forge/src/net/tests.rs`: a host and two editing
machines propose against whatever each has applied, the host takes the
proposals in a seeded shuffled order and each machine gets the stream in
its own shuffled order and at its own pace, over 24 seeds; every machine
and one fold of the whole log end at the host's head, document, members
and holds, and some proposals were rebased. Other tests cover a second
hold, an edit of a held thing, roles, a stale edit by index, a lane, an
undo by another author, `was` after a rebase, the map's admit, a late
joiner with items in flight, a changed welcome, a head that differs, the
watermark, a late item, and a recorded stream replayed through a fresh
peer to the same state. F3's measurements in
[notes/forge-probes.md](notes/forge-probes.md) are still to be taken, so
whether the synchronous protocol holds a multiplayer map is **S**.

The session's reliable ordered channel is the co-op datagram's Forge
lane (`coop::Peer::forge_send` and `forge_recv`): bytes in order, once,
repeated until acknowledged. A datagram carries as many of them as fit
beside its pads and hashes (`coop::wire::forge_room`, at most
`coop::wire::MAX_FORGE`, never past the wire cap), and one flush sends a
window of them (`coop::peer::FORGE_WINDOW`), so a welcome of a few
kilobytes crosses in a few round trips (`coop`'s forge lane tests bound
them), not one a round trip. Every datagram also
carries the beat, the 48-byte log head and stream position
(`forge::net::Beat`), as the keep-alive; `Host::check` ends the seat
whose head differs. `forge::net::Node` runs a machine over a
`forge::net::Lane`: the host and its `Host`, or a guest, with the
messages `ToHost` and `Down` framed by `frame` and `Framer`. A node
sends proposals, holds, carries and releases, settles each by nonce with
the entry or the host's named refusal, and gives the game each tick's
items. Its tests are in `crates/extensions/forge/src/net/node_tests.rs`,
over a fake lane that reorders and chops what it carries and a fake
world: edits made on every machine end at one document, a late joiner
and a film replay equal the live state, a head that differs ends one
seat, and a full lane is refused.

The co-op link that offers the lanes is `game/src/forge_glue/link.rs`:
in a synchronous network game (a co-op host or guest, by the simulation
world's mode) whose lobby is in Forge mode, it runs one `coop::Peer` per
other machine over the co-op channel - the XNet base port + 4, served
from one bound socket (`coop::link::UdpPort`) whose peers the Live
transport names (`Hle::coop_endpoints`; a guest's host from its joined
session) - pumps each peer every tick, and offers the lanes
(`forge_glue::net::offer_host`, one per guest seat, or `offer_guest`),
re-made each tick so a session that opens finds them. A seat that joins
after the offer is added (`net::add_seat`) and one that leaves is
dropped (`net::drop_seat`), the running session taking the changes each
tick; the link closing ends the session (`net::end`). The Forge lane is
the datagram's own section, so beside it the channel carries nothing of
the game's. Beside the direct channel, each lane carries the Live
session's relay (`coop::link::Fallback` around `coop::relay::RelayLink`,
its URL and `sessions.relay` token from `Hle::coop_relay`) for two
machines that cannot reach each other directly: the host's
advertisements say the session runs while it does (`flags: "forge"`,
`Hle::set_forge`), which is what the relay admits, and a lane's relay
connects on a thread of its own so the game never waits on it.
`forge_glue/link.rs`'s tests run the whole of it over a
fake co-op link: the session opens on both sides, a placement on the
host reaches the guest, a late guest gets a lane and a leaving one
loses it, a closing link ends the session, and a dead direct path
opens it over the relay.

A shared `forge::Session` (`Session::share`) queues each edit as a
proposal (`Session::take_outgoing`) rather than applying it, stamps ids
in its seat's lane (seat s uses s+1, s+17, and so on, as the host does),
and applies the stream's items with `apply_item` after `sync_to` took
the welcome's state. `Session::admit` gives the host the game's
checks (palette, budget, bounds). In the game
(`crates/extensions/game/src/forge_glue/editing/net.rs`) the editor
sends what the session queued, runs the node, and stages the items the
node releases through the same `ForgeWorld` path a local edit takes. A
shared session proposes one op per edit: a command that makes several
(a move that unlinks) is as many proposals, and another seat's edit may
fall between them. `forge.status`, `forge.propose`, `forge.log`,
`forge.doc` and `forge.things` answer from the node's state
([agent-api.md](../runtime/agent-api.md), "Shared Forge session").

The film's `FRGE` extension records open with the welcome and then
carry each stream item the machine applied, at the tick it applied it
(`forge::net::film`, [film-format.md](film-format.md)). Watermarks are
not recorded. The editor leaves each record with
`forge_glue::net::record_edit`, and the recorder writes them with
`forge_glue::net::take_edits` before the game tick's first film tick
(`theater::extension`). In playback the player hands each record over
with `forge_glue::net::deliver_played` when the tick before it ends, and
the editor's tick applies them in order (`Editor::film_tick`): the welcome
gives the session its state, each item changes the document and the world.
The monitor looks and poses stay in the stats container's Forge entry
record (tag 7) through `forge_glue::net::take_film`.

**F6 in detail.** `forge_doc::Forge` holds Forge-family edits: a
`Link` and an `Unlink` naming the link op it ends. A teleporter link
names a sender and a receiver netgame flag and a channel; a device link
names two machines, lower endpoint first. An endpoint is a base entry by
its base index or the entry an Add op made, so it survives later appends
and swap-removes. The validator refuses a link to a missing or removed
entry, the removal of a linked entry, a flag in two roles or on two
channels, and a repeated link. `ForgeDoc::links` resolves the active
links to final indices from the base counts.

`forge-build` writes links after every placement edit. A teleporter
link sets both flags' identifier (`+0x14`) to the channel and is refused
by name when the sender is not type 7 or the receiver not type 8; an end may
be a flag the document adds as well as one of the map's. Each
group of linked machines becomes a new device group after the map's own
(`scnr+0xa0`, stride `0x28`), and each member's position group (`+0x36`)
names it. An unlinked entry keeps the bytes it would have had without the
link (`crates/tools/forge-build/tests/links.rs`). The bots' generator
pairs teleporters from those same bytes: type at `+0x10`, identifier at
`+0x14` of `scnr+0x118`. `bots::autonav::teleporter_flags` is that
reader, public, and `link_teleporters` the linker; the fixture test reads
a linked pair through them.

In a local session the Links tool aims at the map's teleporter flags,
at the teleporter flags the session added and at placed machines. A
starts a link on the first and makes it on
the second; the delete action removes the aimed thing's latest link, and
the Properties action opens the Link question with **Remove link**
(the buttons: [forge-editing.md](forge-editing.md), "Controls"). A teleporter pair takes the
channel either flag already has through a link, the sender's first, else
the sender's own, else the lowest no teleporter uses. The session writes
each changed identifier of the map's own flags into the loaded scenario
through `ForgeStep::Channel`, after the batch's objects, and undo writes
the previous one. An added flag is a row of the kit: `scene::project` gives
the marker the link's channel as its identifier, so the kit's `base_field`
writes the row, an unlink or undo gives the add's own channel back, and a
link that names an added flag of another type than its role is refused
(`Session::check_added_roles`). A session with no kit cannot add flags;
a document's added flags take their channel at the Rebuild, where
`forge-build` writes the same identifier. Device links have no live write: the
engine's device group setter is unproven, so they count as pending
Rebuild edits in the HUD title and `forge.status`. Deleting a linked
machine or flag ends its links in the same undo step.

**F7 in detail.** `ForgeExt` is the `FORGE` record of the `SimExt` blob
([sim-rewrite.md](../engine/sim-rewrite.md), "The `SimExt` blob"),
which registers the kind and holds no record until this slice. It
carries what Forge adds that retail has no home for: emitter timers,
logic node state, barrier switches and volume occupancy ("Who owns
state"). The Forge step writes it at the tick boundary, in op-id order.
Nothing a Firefight map needs is in it: drop zones are scenario data
the ground build compiles, the director's state is the rules VM's
`RulesExt`, and a wave is the director's command. When the graph is
built, its squad sink issues `actor_net::squads::SquadOp` through the
entry Firefight uses, so a trigger can place a squad on a map that has
the ground.

**F10 in detail.** A `transplant` recipe copies a tag and its closure
from a source map on the player's disc (`tags::schema::transplant`,
`TagSet::add_tag`), names the source by hash, and is refused with its
cost when the map has no room. Its first users are monitor bodies. The
one transplant a Firefight map needs is narrower and does not use the
recipe: the AI ground's class list
([forge-firefight.md](forge-firefight.md), "Enemy classes") goes
through `firefight_kit::Classes`, which has the cost report, the cut
order and the sound gestalt merge, and whose donors are the kit's
retail class table.

**F11 in detail.** Forge adds no AI machinery of its own.
[forge-firefight.md](forge-firefight.md) is the design and the slice
table. In brief:

- **Where squads arrive, and what they walk on.** The author places
  `ff_drop`, `ff_base` and `ff_cover` markers; **Build AI ground**
  hands them to `firefight_kit::ground::build`, which makes the
  sectors, zones, firing positions, sixteen squad slots and hunt
  orders a kit map has ([forge-map-variants.md](forge-map-variants.md),
  "AI ground"). This is built.
- **What arrives.** The document carries one wave set, a
  `firefight::Def` as the built-in sets are written. The Waves screens
  edit it; a squad is a row of a wave with a class, a count and a drop.
- **Who runs it.** Firefight's director and host runtime, unchanged: in
  the session as a test (**Test waves**), and from the lobby as a
  Firefight game on the saved map variant, with the map's own set
  (Map Waves) or a built-in one.
- **Enemy classes** past the Elite come with the ground recipe, from
  the player's campaign maps.
- **Networking** is Firefight's: the distributed model with host actors
  ([ai-networking.md](ai-networking.md)). No synchronous session is
  needed.

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
5. **forge.md** is the retail reference only. The design is here.
6. **Forge identity.** The document hash omits the entire `expect`
   block. Format 2 uses the base map hash and document hash.
   `expect.map_id` checks the resulting ID at build and load.
7. **The model is Halo 3's and Reach's Forge**, with the advanced
   features of Halo Infinite's that fit Halo 2's engine
   ([forge-editing.md](forge-editing.md), "What it takes after"). The
   look stays Halo 2's.
8. **Forge is a lobby mode**, not a main menu row. It is a mode of the
   party lobby in Split Screen, System Link and Xbox Live custom games
   ([party-lobby.md](party-lobby.md)); Theater is the main menu's sixth
   row.
9. **Halo 3's buttons**, the axis gizmo and the motion tracker in the
   monitor ([forge-firefight-ui.md](forge-firefight-ui.md),
   "Decisions").

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
- **Code**: `forge-doc` defines the portable document, recipes, and log
  validation and folding.
- **Code**: `forge-build` materialises the document, including its kit
  allocation. `content::Library::resolve_package` loads the local map.
- **Test**: the synchronous protocol (`forge::net`) converges over fake
  machines with reordered proposals and delivery
  (`crates/extensions/forge/src/net/tests.rs`).
- **Design**: the two network modes, `ForgeExt`, the chassis
  rule, the sharing table, and the model upload check.

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
- Device groups driving machines in a multiplayer session, and whether
  the position group is the field that makes a linked machine follow.
- Whether the teleporter reader takes a channel written into the loaded
  scenario at once, or only from the next round (netgame flag results are
  cached per game, scenario-editor.md section 6).
- Mirrors and puppets in a distributed session: that a client-made
  object the host did not replicate is left alone by the engine's
  replication, and the bandwidth of corrections and poses.
- Whether a placement's scale reaches collision and physics.
- Materialise and nav build times on real maps.
- Everything [forge-editing.md](forge-editing.md) and
  [forge-firefight-ui.md](forge-firefight-ui.md) list as not verified.

## Related

- [forge-features.md](forge-features.md): each feature in detail.
- [forge-firefight.md](forge-firefight.md): Firefight maps built in a
  session: the wave set, the test, the lobby, slices FA1 to FA8.
- [forge-editing.md](forge-editing.md): the monitor's controls, tools
  and properties.
- [forge-firefight-ui.md](forge-firefight-ui.md): the screens and HUD.
- [forge-map-variants.md](forge-map-variants.md),
  [forge-map-variants-ui.md](forge-map-variants-ui.md): editing the
  map's own placements, the map variant file, the lobby and Live, and
  the editor's readout, world marks and screens.
- [notes/forge-prior-art.md](notes/forge-prior-art.md): Halo 3, Reach
  and Infinite Forge, with sources.
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
