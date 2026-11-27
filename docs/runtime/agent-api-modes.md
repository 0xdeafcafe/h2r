# Agent API: Forge, Firefight, rules, variants and Theater

Status: Built. Part of the [agent API](agent-api.md), which holds the wire, the method index and the rules.

## Local Forge session

The local Forge feature registers these methods in
`crates/extensions/agent/src/forge/local.rs`. The controller and the agent
edit the same document. The methods appear in `session.methods` when the
feature is enabled.

| Method and access | Parameters | Result |
|---|---|---|
| `forge.status` (read) | `{}` or omitted | The local session, role, log head, budget, monitor camera, aimed and held IDs, screen and focus, pending call or file work, and last notice. Tool settings: `rotation_snap` (degrees), `movement_snap` (world units, 0 for off) and `axes` (`world` or `local`). `overlays`: each overlay (`markers`, `monitors`, `gizmo`, `nav`, `blockout`) to whether it is on. Block-out: `block_grid` (units), `dev_view`, and `resize` (`{target, def, size}` while a Resize draft is in hand, else `null`). Links fields: `tool` (`objects`, `links` or `nav`), `aimed_link` and `link_from` (`{teleporter: flag}`, `{flag: id}` for a teleporter the session added, or `{object: id}`), `teleporters` (`{flag, sender, channel, position}` with the channel the document gives), `added_teleporters` (`{id, sender, channel, position}`, the teleporter flags the session added, with the channel a link gives), `links` (`{id, kind:"teleporter", sender, receiver, channel}` or `{id, kind:"device", machines, applies:"rebuild"}`, final indices) and `pending_rebuild`, the active device links. Palette fields: `palette` (the Objects palette's item count) and `palette_refused` (each scenario palette entry left out, as `"path: reason"`). `parked`: the hex handle of the biped kept while the player is in a Forge body, undamageable and hidden, or `null` while none is. Body fields: `body` (`{kind, unit}` while the player is in one: `kind` the body's key, `monitor`, `sentinel` or `prophet`, and `unit` its hex handle; else `null`), `body_choice` (the profile's `forge.body` key) and `bodies` (`{kind, name, refused}` for each body, `refused` the sentence that names why the kit left it out, or `null`). Groups fields: `groups` (`{id, name, locked, pivot, members}`, `pivot` `centre` or `first`, `members` the things' endpoints) and `selection` (the endpoints chosen to group). Labels and prefabs: `labels` (`{thing, text, number}`, the active labels) and `prefabs` (`{name, pieces, grouped, fits, reason}`, the saved prefabs, and why one is refused on this map). Things fields: `aimed_thing` and `held_thing`, the thing's endpoint as `forge.things` gives it (an own entry's or an added one's; `aimed` and `held` are an added object's op ID only), `game` (the variant record's engine index the tool reads, or `null`) and `show` (`this_game_type` or `all_game_types`). `variant`: `{name, title, id, tier, dirty, waves}`, the session's document as a map variant saves it: `name` the file it was saved or loaded as (`null` before), `id` the play form's identity (`null`, with `id_refused`, when it does not compute), `tier` `variant` or `map`, `dirty` edits since the last save or load, and `waves` the name of the session's own wave set, or `null` when it has none and a save writes none (`forge.waves` has the rest). `ground`: `{state, building, available, loaded, markers, problems, zones?, squads?}`, the AI ground in brief (`forge.ground` has the rest): `state` is `not_built`, `current` or `stale`, `available` whether this game has a builder, `loaded` whether the last build's map is the running one, `markers` how many labels are placed and `problems` how many labels the last build could not honour. `nav`: `{state, available, generating, problems, sectors?, out_of_map?}`, the last **Generate nav** in brief (`forge.nav` has the rest). An inactive session returns `{session:"inactive", reason}` |
| `forge.ground` (read) | `{}` or omitted | The AI ground (docs/game/forge-map-variants.md, "AI ground"): `{state, building, available, loaded, recipe, counts, markers, problems, report}`. `state` is `not_built`, `current` or `stale`; `building` is true while a build runs (edits wait for it); `available` is whether this game has a builder; `loaded` is whether the running map was reloaded onto a build of the ground the document asks for now (false after an edit makes it stale, and while the game cannot reload the map). `recipe` is `{kit_version, home, probe, classes}` or null before the first build, with `classes` the enemy classes past the Elite as `{name, donor, sha256}`, each donor the campaign map the class is copied from (docs/game/forge-firefight.md, "Enemy classes"). `classes` (top level) is the class names the session asks for, in keep order: the recipe's, or a list `forge.ground.build {classes}` gave that the next build gives donors. `counts` maps each label (`ff_drop`, `ff_base`, `ff_cover`, `siege_site`, `siege_goal`, `siege_line`, `siege_barrier`) to how many are placed (a `siege_barrier` is a label on a placed object, not a flag, so it is counted and is not in `markers`), and `markers` lists the flags as `{id, label, number, position, facing, team}`. `problems` is each label the last build could not honour, `{label, position, reason}`. `report` is null before a build, else `{zones, squads, problems, receipt, classes, cut, costs}`, with `receipt` `{base, labels, kit_version}` as hex digests of the inputs the build covers, `classes` the classes the ground has in palette order (the Elite first), `cut` the classes cut to fit the map's tag headroom (first cut first) and `costs` each kept class past the Elite as `{name, tags, bytes}` |
| `forge.ground.build` (act) | `{if_stale?, classes?}` or omitted | **Build AI ground**, or with `if_stale: true` **Test waves** (build only when the ground is stale or was never built). `classes` asks for those enemy classes past the Elite first (`grunt`, `jackal`, `drone`, `brute`, `hunter`, in any order; `[]` for none): a list other than the recipe's marks the ground stale, and the build takes each class's donor from this game's disc and cuts classes from the end of the list when the map is short of headroom. Answers when the build has ended with `forge.ground`'s reply plus `built`, whether it built. The build runs off the game thread over a copy of the session, and edits wait meanwhile. Refused as `forge_refused` when this game has no builder (`This game cannot build AI ground.`), in a shared session, while another edit or file job waits, with the build's own sentence when it fails (a map with no room for the ground names its cost and its biggest part; a class no donor on the disc carries, or a donor of another hash, is named), and for a class name the table lacks; a failed build changes nothing |
| `forge.waves.test` (act) | `{stop?}` or omitted | **Test waves**, or with `stop: true` **Stop waves** ([forge-firefight.md](../game/forge-firefight.md), "Test waves"), at a local multiplayer tick; registered in `crates/extensions/agent/src/forge/test.rs`. A start whose ground the running map holds answers once the Firefight runtime has taken it, `{testing: true, set, id}` with the set's name and `Def::id` (the session's set, else Holdout's built-in set). A start whose ground is stale or not loaded builds it and answers at once with `{testing: false, building: true}`; the test starts after the reload, and `firefight.state`'s `test` says when. A stop answers `{testing: false, stopped}`, the test's squads erased. Refused as `forge_refused` with the row's sentence: while another edit or file job waits, while a test runs (`A wave test is running.`), with no test to stop (`No wave test is running.`), when this game cannot run Firefight, when the build leaves the ground unloaded, and with the runtime's refusal (a match running, a seat other than the host's, a map with no AI ground, a film's playback) |
| `forge.nav` (act) | `{generate?}` or omitted | **Generate nav** ([forge-nav.md](../game/forge-nav.md)): `{state, available, generating, counts, problems, overlay}`. `state` is `not_built`, `current` or `stale`; `available` is whether this game has a generator; `generating` is true while one runs (edits wait for it). `counts` is null before a generation, else `{sectors, links, out_of_map, solids}`, `links` mapping each link kind (`walk`, `drop`, `jump`, `crouch_jump`, `teleporter`, `lift`, `elevator`) to its count. `problems` lists `{kind, what, position, reason}`, `kind` one of `start_off_mesh`, `teleporter_off_mesh` and `solid_left_out`. `overlay` lists `{kind, a, b}` lines: each sector's sides (`sector`, or `out_of_map` where no start reaches) and each link but a walk, by its kind. With `generate: true` (a local multiplayer tick) it generates first and answers when it has ended; refused as `forge_refused` when this game has no generator (`This game cannot generate nav.`), in a shared session, while another edit or file job waits, and with the generator's sentence when it fails; a failed generation changes nothing |
| `forge.waves` (act) | `{set?}` or omitted | The session's wave set ([forge-firefight.md](../game/forge-firefight.md), "The wave set"): `{name, id, default, set, problems, drops}`. `set` is the `firefight::Def` the session plays, its own or, with `default: true`, Holdout; `id` is its `Def::id` as hex. `problems` lists `{round, wave, squad, what, sentence}`, one-based, `what` `class_not_built` (a class the AI ground does not build) or `zone_not_placed` (a preferred zone no drop gives); neither is a refusal. `drops` lists the session's `ff_drop` markers in zone order as `{zone, id, position, facing}`: zone `NN` is the `NN`th by position, then facing, as the kit numbers them, and a squad's `prefer_zone` follows its marker when a drop is added, moved or removed. With `set` it first replaces the set with that definition, or with `null` clears it, and answers the new reply. The set is beside the document, as the ground recipe is: a save writes it into the map variant (`waves`, format 5) and a load takes it out; it is not an op, so `forge.log` and undo do not carry it. Refused as `forge_waves`, changing nothing: a set over 64 KiB (checked before it is decoded), one that is not a definition, the validator's sentence (a class other than `elite`, `grunt`, `jackal`, `drone`, `brute` and `hunter`, a wave over the enemy limit at some player count, a round without five waves), a seat other than the host's in a shared session, and while an edit or file job waits |
| `forge.pieces` (read) | `{}` or omitted | The session's block-out pieces ([forge-blockout.md](../game/forge-blockout.md)): `{pieces, slots, pending}`, registered in `crates/extensions/agent/src/forge/pieces.rs`. `pieces` lists each piece in the order of the crates block as `{target, def, name, state, position, rotation, bounds}`: `target` is the endpoint a `Forge::Piece` op names (`{added: op}`), `def` the definition as the document writes it (`{shape, size, param, materials}`, sizes in block-out units), `name` its canonical name (`forge\blockout\<name>` is its `bloc`), `state` `live` (an object of its `bloc` stands), `slot` (an object of a spare definition, from slice BO10; the piece then also has `slot`) or `pending` (in the document with no object until **Rebuild map**), and `bounds` the box it fills, `{min, max}` in world units (its size turned by `rotation` as yaw about z, pitch about y and roll about x). `slots` lists the spare definitions in use and is empty until BO10; `pending` counts the pieces with no object. A piece is also a thing of kind `piece` in `forge.things`, and `forge.propose` takes the op `{"id", "edit":{"forge":{"piece":{target, def}}}}` that changes the definition of a piece the document already has; a placement is the add and its definition together, which `forge.propose` takes one op at a time and so cannot make |
| `forge.things` (read) | `{kind?, changed?}` or omitted | `{things, left_out}`. `things` is every thing of the session (the map's own entries in block and index order, then the added ones), or those of one `kind` (`object`, `item_spawner`, `starting_point`, `goal`, `teleporter`, `piece`), or with `changed` those an entry op changes (`true`, which includes every added one) or not (`false`). Each is `{id, block, kind, own, changed, position, rotation, fields}`: `id` is the endpoint an entry op's `target` names, `{base:{block, index, print}}` or `{added: op}`; `fields` maps each layout field name to its value as the document writes one. Each netgame flag also has `area` in the document spelling, or `null` for an unshaped flag, and `show`, whether its play form shows in a match (a flag the document added unless an op turns it off, a flag of the map only when an op turns it on). An active area or a `show` op marks its flag as changed. A removed entry is not listed. `left_out` is each block of the map that did not read, as `"block: reason"`; its entries are not things |
| `forge.doc` (read) | `{}` or omitted | `{document}`, the folded Forge document including confirmed edits |
| `forge.log` (read) | `{since?}` | `{entries}` in sequence order, starting strictly after `since` (default zero) |
| `forge.input` (act) | `{action}` or `{movement?, look?, roll?, rotating?}` | `{queued:true}` when the current local session accepts one monitor input |
| `forge.propose` (act) | `{op, was?}` | `{entry}` on acceptance, or a refusal: its sentence as the message and `{code, message}` as the error's `data`. `op` may be an entry op (`{"entry":{"set":{target, write}}}`, `remove`, `reset`, `{"entry":{"area":{target, area}}}` or `{"entry":{"show":{target, "on": bool}}}`, which only a netgame flag takes) naming a thing by its `forge.things` `id` |
| `forge.look` (read) | `{seat?}` | The monitor look: `{session, look, drawn_as, saved, catalogue, locks}`, and `notice`, `catalogue_note` and `problems` when there are any. `look` is the session's look, or with no session the saved one as this machine can use it. `drawn_as` is what other players see: `{as: "model" \| "marker" \| "hidden", reason?}`. `saved` is `{look, source}` as the profile holds it. `catalogue` is each model of the census: `{key, name, source, sha256, variants, colours, light, nodes, triangles, available, refusal?}`. `locks` maps each locked field to its reason. A `seat` other than 0 is refused: offline there is one seat |
| `forge.look.set` (act) | `{reset?, save?, model?, variant?, primary?, secondary?, tint?, light?, light_colour?}` | `{look, changed, saved, drawn_as}`: the look as accepted. `reset: true` is the default look (the marker in the seat's colour) and takes no field. `variant: null` clears the variant. `save: true` also writes the profile (`not_saved` says why when the file was not written). A refusal is an error whose `data` is `{code, message, ...fields}`: `monitor_look_invalid` (`field`, `reason`), `monitor_look_locked` (`field`, `reason`) or `monitor_look_no_session` outside a Forge session; the look stays as it was |
| `forge.status.monitors` (read) | `{}` or omitted | `{monitors: [{seat, name, monitor, pose, look, drawn_as, peers_see, seen_ticks_ago}]}`. Offline it holds seat 0 once a session is open and is empty before. `pose` is `{position, yaw, pitch}` while the monitor camera exists. `drawn_as` is how the seat looks in its own view, always `hidden` with the reason; `peers_see` is what another machine draws |

The input queue accepts `toggle`, `accept`, `select`, `back`, `delete`, `palette`,
`properties`, `tools`, `settings`, `overlays`, `undo`, `redo` and `link`. `palette`
opens the object menu and `properties` the aimed or held thing's
Properties, as X does by what is aimed at. In the Objects tool `accept`
picks up the thing aimed at, own or added, mark or body, and drops it.
With empty hands `select` toggles selection of the ungrouped thing under
the reticle, like holding A. `delete` removes the thing; `tools` opens the Tools
picker, `settings` the Tool settings and `overlays` the Overlays page. `link` switches to the Links
tool; there `accept` starts a link on the aimed teleporter flag or
placed machine and makes it on the second, `delete` removes the aimed
thing's latest link, `properties` opens its Link dialog, and `link` or
`back` cancels a started link. `link` with nothing aimed returns to the
Objects tool. `back` with nothing to cancel does nothing. In player mode
only `toggle` enters the monitor. The screens are the game's: while one
is up (ours or the game's), `forge.input` is refused with "A screen is
up; drive it with ui.press." and the screen is driven with `ui.press`.
`menu` opens the Forge menu, as Back does, and is refused in player mode
with "The Forge menu opens while you fly the monitor.". An analog request supplies one 1/30-second sample
with at least one axis. Movement is [right, forward, up], look is [yaw,
pitch] and roll turns a preview with `rotating:true`. Each axis is in
[-1, 1]. Read `forge.status` for the resulting monitor and document
state: it adds `fine` (fine movement on), `ui_up` (a screen is up) and
`dirty` (edits not saved), and `screen` is the Forge screen of ours that
is up, with its rows and focus.

Choose **Edit area** in the Properties of a goal with an area (a hill,
a territory, a site, a flag return, a bomb goal or a siege goal) before
sending area input. While a draft is active, `forge.status.area_edit`
gives `{target, shape, below, above, radius?, half?, points?}` with
numeric world units: `shape` is `radius`, `box` (with `half`, the half
length and half width), `sphere` (with `radius`; `below` and `above`
are 0) or `path`. It is `null` outside the editor. `forge.things` shows
the committed area.

| Input in the draft | Radius | Box | Sphere | Path |
|---|---|---|---|---|
| `accept` | Save | Save | Save | Drop a point, or close at the first point |
| `properties` | Open the typed dimension rows | The same | The same (Radius only) | Remove the last point |
| `area_shape` | Switch to a box of the radius each way | Switch to an equal-floor sphere | Switch to an empty path | Switch to an equal-floor radius |
| `area_close` | | | | Close and save the path |
| `area_grab`, `area_release` | | | | Press or release A for a drag |
| `back` | Cancel | Cancel | Cancel | Cancel |

In a radius or sphere draft, the forward axis steps the radius; in a box
draft, which lies on the marker's facing, each axis steps the box's
axis nearer to it as the monitor looks: forward the half length and
right the half width while the monitor's yaw is within 45 degrees of the
marker's facing (or its back), the other way round otherwise; one step a
lean and again every 0.18 s while held. In a path
draft, use movement and look to aim at the floor.
`forge.input` acknowledges the queued input. Read `forge.status.notice`
for geometry refusals and `forge.log` for the area op written on save.

A proposal uses the [Forge document's](../game/forge-design.md#the-document)
portable op and optional prior-value digest. The local host supplies the
author and sequence. Placement waits for the engine's creation result.
A saved document can create several objects in one tick. If the engine refuses
a creation, the call releases objects it made for that load. A refusal leaves
the log head in place. Moves, deletes and restores, including those of
undo and redo, run at the next tick through the `world.object.set` and
`world.object.delete` bodies and the ownerless spawn. A live object that
no longer resolves by kind and salt is refused as `forge_object_gone`
before any guest call. A block with no `world.object` kind (sound
scenery) is refused as `forge_object_kind`.

An entry op edits one of the map's own things
([forge-map-variants.md](../game/forge-map-variants.md), "Applying an
edit in a session"): read its `id` from `forge.things`, whose print the
session checks against the row it kept when it opened. A set of its
position or rotation moves it, a removal makes its row inert and a reset
puts the kept row back, at the next tick. A refused input or proposal
carries its name in `data.code` (`agent_proto::forge::refusal`):

| Code | Refused because |
|---|---|
| `forge_print` | the entry's print is not the one the op carries: the map's entry is not the one the edit was made on |
| `forge_was` | the target's value is not the one `was` names |
| `forge_left_out` | the entry's block did not read when the session opened (`forge.things` `left_out`) |
| `forge_object_gone`, `forge_object_kind`, `forge_object_held` | the game call's: the live object no longer resolves, its block has no live objects, or the own entry's object is held or attached |
| `forge_no_kit` | an add or a drop of a starting location, netgame flag or netgame equipment row, on a map whose next row is not in the kit's spare form: the map has no kit (or its spare rows are used) |
| `forge_waves` | `forge.waves {set}` was refused: the message is the validator's sentence or says why |
| `forge_refused` | any other: the message says why |

A kit session adds those rows: the document's `Add` of such an entry is
one `Add` base step, which writes the block's next spare row and raises
its root count, and its removal or undo is one `Drop` step, which writes
the spare form back and lowers the count (`agent::forge::base`). A batch
is checked in order; a refusal changes nothing.

An area op names a netgame flag, own or added, and sets its
[document area](../game/gametype-markers.md#in-the-document). Send
`"area": null` to clear it. Radius, offsets and heights use the
document's eight-digit lowercase hexadecimal float spelling. An area
edit changes the document and can be undone through the edit log.
`forge_refused` carries the area validator's sentence, including when
a path crosses itself, a flag has no area to clear, or a hill already
has corners. The `was` precondition applies to field sets.

A teleporter link or unlink, or the undo of one, writes each changed
flag's identifier into the loaded scenario at the same tick, after the
batch's objects. The call checks the flag index against the scenario's
count and its type (7 or 8) before any guest write. A device link reaches
the game only through a rebuild and counts in `pending_rebuild`. Deleting
a linked machine ends its links in the same undo step.

## Shared Forge session

In a synchronous network game whose co-op link carries the Forge lanes,
the local Forge methods answer for the shared session
(`crates/extensions/agent/src/forge/net.rs`; the design is
[forge-design.md](../game/forge-design.md), "F5 in detail").

- `forge.status` has `session: "shared"`, the seat's `role`, and `net`:
  `host`, `joined`, `seat`, `name`, `members` (`seat`, `name`, `role`),
  `holds` (`seat`, `target`, `carried`), `head`, `n` (the last stream
  item applied), `through` (the host's watermark), `in_flight`
  (proposals and holds not yet settled) and `ended` (why the session
  ended, else null).
- `forge.propose` sends the op to the host and answers when the stream
  carries the entry (the entry, as locally) or the host refuses it. It
  waits up to 240 ticks, not 30. A refusal's message begins with its
  name and a colon: `forge_not_joined`, `forge_role`, `forge_held`,
  `forge_not_holding`, `forge_stale`, `forge_ahead`, `forge_lane`,
  `forge_was`, `forge_print`, `forge_refused`, `forge_map` (the game's
  palette, budget or bounds), `forge_seat`, `forge_name`,
  `forge_head_differs` and `forge_clock`. A proposal waiting when the
  session ends is refused `forge_not_joined`.
- `forge.log`, `forge.doc` and `forge.things` read what this machine has
  applied, so the same call on two machines at the same `through`
  returns the same entries.

A session that has ended keeps answering `forge.status` with `ended` set
and refuses every proposal.

## Forge probes

The running game's agent registers these methods through
`crates/extensions/agent/src/forge.rs`. The wire types are in
`crates/extensions/agent-proto/src/forge.rs`. Results include the normal
`at` stamp. See the [measurement procedure](../game/notes/forge-probes.md).

| Method and access | Parameters | Result |
|---|---|---|
| `forge.capacity` (read) | `{}` or omitted | `tag_arena {low, high, headroom_bytes}`, `object_pool {used, limit}`, and `blocks` keyed by scenario block name |
| `forge.probe` (act) | `{what:"replication", group, tag?, pos?}` | `{what, session, tag:{tag, group, name}, object, pos}` after the next tick |
| `forge.probe` (act) | `{what:"sync", from_tick?, max?}` | `{what, session, scenario_type, variant_engine, desync, hashes}` |
| `forge.probe` (act) | `{what:"ai"}` | `{what, enabled, active, actors}` |
| `forge.probe` (act) | `{what:"kit"}` | `{what, blocks, objects_by_group, actors}` |

**Capacity and kit.** `headroom_bytes` is the difference between the
two physical arena cursors at the stamp's tick. It includes space
that cache pools may need. Each block reports `count`, the known
`limit` or `null`, and `kit_left`. The candidate spare count is
the contiguous marked tail of a block. The markers are palette index
`-1` for placements, placement index `-1` for object names, player
type `-1` for starting locations, flag type `0xffff` for netgame flags
and a null item datum for netgame equipment. A block without a known
marker reports `null`. The kit probe adds a runtime object census by
group and the live actor count for base-versus-kit comparisons. The
candidates' behaviour is measured in the game.

**Replication.** The host must be in a ticking `mp_host` session. The
group must be one that [`sim.spawn`](agent-api-sim.md#the-games-own-functions-teleport-give-spawn)
accepts. A tag may be a datum or name in that group. With the group alone,
the probe uses its first valid loaded tag in index order. With `pos`
omitted, it uses one world unit above the first eligible scenario
starting location. Pass an explicit `pos` to control the spawn location.
The probe queues a single `sim.spawn` engine call. The returned
`object` is the host's salted handle in the same form as `sim.objects`.
The client can enumerate that group through `sim.objects` immediately
afterward. A refused or missing spawn leaves replication unmeasured.

**Synchronous protocol.** `session` uses the same reader as
`sim.session`. `scenario_type` is the loaded tag's kind (multiplayer
is 1). `variant_engine` shows the engine in the active game options
(none is 0). `desync` is the simulation world's desync latch. Fields
whose backing globals are absent are `null`. The probe supplies the
current multiplayer map's [`sim.hashes`](agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes)
rows up to the sampled tick. `from_tick` is inclusive. `max` defaults
to 4096 and accepts 1 to 4096. During pregame, or when the hash observer
has no samples for the current map, `rows` is empty. Compare matching
ticks across peers and check the latch on both.

**AI.** `enabled` is the flag the engine's `ai` builtin sets and
`ai_update` reads. `active` is the adjoining activity flag used by
the campaign AI extension. A missing AI-global pointer produces
`null` for both, and a missing actor pool produces zero actors.
Capacity, AI and kit reads require a gameplay tick.

## Firefight

The [Firefight](../game/firefight.md) runtime of the loaded map, read and
driven at a tick boundary. `crates/extensions/agent/src/firefight.rs`
checks the params and calls the game's runtime
(`game::firefight::feature::AgentSource`, over
`crates/extensions/game/src/firefight/runtime.rs`); the wire types are in
`crates/extensions/agent-proto/src/firefight.rs`. The methods are always
registered; with the `firefight` feature off
([switches.md](switches.md)) `firefight.state` says so and
`firefight.command` refuses.

| Method and access | Parameters | Result |
|---|---|---|
| `firefight.state` (read) | `{}` or omitted | `{running, test, refused?, map?, role?, team?, director?, log?, errors, placements_unseen, pods?}` |
| `firefight.command` (act) | `{command?, director?: "hold" \| "run"}`, at least one | `{appended?: {seq, tick}, held, log}` |
| `firefight.kit` (read) | `{}` or omitted | `{kit, slots, team?, zones: [{index, point, starts}], classes, bases, characters, hunt_orders}` |

**State.** `running` is false outside a multiplayer game, on a map
without the kit, and with the feature off; `refused` names which (a map
without the kit: "the map does not carry the Firefight kit"). `test` is
true while what runs is a Forge session's wave test (`forge.waves.test`),
whose lives and respawns are not applied and whose finish ends the test
only; a test reports no score or result and publishes no log head. `role` is
`host` (the distributed server, a synchronous host, or a game with no
session), `replica` (a synchronous peer, or a film's playback) or
`client` (a distributed client). `team` is the actors' team, never a
player team (with two player teams it is a third). `director`, on the
host only: `phase` (`warmup`, `spawning`, `fighting`, `cleanup`,
`intermission`, `bonus`, `done`) with `phase_tick` for the timed ones (a
bonus round: the tick it ends at), `set` (zero-based; always 0 in a finite
definition), `round` (of the set) and `wave` (zero-based), `reserve` (actors of the wave not yet placed),
`teams` (the player teams, 1 or 2), `pools` and `scores` (each team's
shared lives and points, one entry per team), `live_actors` (enemies)
and `live_allies` (the round's allied squads, which also count toward
`cap`) against `cap`, `players`, `skulls` and `held`.
`log` is the head (`seq`, `tick`, `hash` in hex) and `applied`, the last
entry applied here. `errors` holds the latest 16 entries the engine
refused, each with its `seq` (none for the AI switch), `tick` and
`error`. `pods` is the players' drop pods: `on`, `hidden` (why the kit has none), `open_starts`, `pod_spawns` and `pod_fallbacks` by reason, and the squads' pods: `squad_falls`, `squad_fallbacks` by reason (`no_fall`, `no_free_pod`) and `squad_reparks` (docs/game/firefight.md, "Drop pods"). `placements_unseen` counts the squad placements applied here whose
slot showed no actor within 90 ticks (the director's slot timeout), each
also one row in `errors` ("slot N: C placed at tick T, none seen"); a wave
that clears at once with this above 0 means the game never made, or the
runtime never counted, its actors. A placement the engine answers by adding
no actor to the squad is an `errors` row of its own ("the engine placed no
actor for slot N: class row R at zone Z").

**Command.** `command` is one director command in the host log's shape
(externally tagged, snake_case): `place_squad {slot, class, count, zone,
weapon_tier}`, `drop_squad {slot, class, count, zone, weapon_tier}` (by
drop ship where the kit has a ship and a path to the zone, else on foot),
`ship_hover {on}`, `ship_unload` and `ship_leave {zone}`, `target {slot, team}`, `hunt {slot}`, `erase_squad
{slot}`, `set {index}`, `round {index}`, `wave {index}`, `bonus {on,
ticks}`, `skull {id, on}`, `lives
{player?, pool}`, `team_lives {team, pool}`, `score {player, points,
reason: kill | assist | player_kill}`, `respawn {player}` and `finish
{outcome: won | lost | {team_won: {team}} | draw}`. `target` picks the
player team a slot's squad goes for, `respawn` is refused while a `bonus`
round is on (the engine error is in `errors`: no respawn in the bonus
round), and its next `hunt` heads for that
team's base (team 0's until one is set); `team_lives` sets one team's
shared pool and `lives` the one-team pool or a player's own lives, in
the director, so its next respawns spend from the new count (a team at 0
waits for the wave to end). It is appended at the tick after the last one
the runtime stepped, with the author `agent`, and applied before that
tick like the director's own. `director: "hold"` stops the director
stepping, so only appended commands reach the game; `"run"` starts it
again. A director change applies before the command. Refusals:
`UNAVAILABLE` when the mode does not run, `REFUSED` (11) on a client or
a replica (only the host writes the log), `INVALID_PARAMS` for a command
of another shape, a class or zone the kit lacks, a `target` team the
kit has no base for, a `team_lives` team above 1, and params with
neither field. An engine refusal comes later, in `firefight.state`'s `errors`,
as does a `lives` or `team_lives` the match keeps no such count for (a
player's own lives with shared lives, a shared pool with lives each
player's own, a player the director has not seen).

**Kit.** Read from the loaded scenario on any machine: `slots` gives
each of the sixteen slots' scenario squad index (`null` where the kit
reserves none), `zones` each drop zone's scenario index, point and
firing position count, `classes` each kit class's character palette row,
`characters` the palette's rows, `bases` each player team's base point
(base 0 the home base, from the `ff_base_NN` zones) and `hunt_orders`
the scenario order for each base's hunt (`ff_hunt_00`, `ff_hunt_01`;
`null` where the kit has none), and `hold_orders` the same for the allies'
hold (`ff_hold_00`, `ff_hold_01`; left out when the kit has none).
A map without the kit answers `{"kit": false, ...}` with empty lists.
The kit's version string is not read: the game has no reader for the
loaded map's string IDs.

```sh
h2 agent firefight.kit
h2 agent firefight.command '{"director":"hold"}'
h2 agent firefight.command '{"command":{"erase_squad":{"slot":0}}}'
h2 agent firefight.state
```

## Rules scripts

The [rules script](../game/rules-engine.md#the-host-runtime) of the game
running now, read at a tick boundary. `crates/extensions/agent/src/rules.rs`
checks the params and calls the game's runtime
(`game::rules::feature::AgentSource`, over
`crates/extensions/game/src/rules/runtime.rs`); the wire types are in
`crates/extensions/agent-proto/src/rules.rs`. The methods are registered in
builds with the `rules` cargo feature; with the `rules` switch off
([switches.md](switches.md)) it says so.

| Method and access | Parameters | Result |
|---|---|---|
| `rules.state` (read) | `{}` or omitted | `{running, refused?, script?, version?, hash?, source?, carrier?, scoring?, options?, authority?, stopped?, budget?, pending, errors, last_reload?, blocked?, labelled?}` |
| `rules.zones` (read) | `{}` or omitted | `{zones: [{id, label, number?, shape: "area" \| "corners" \| "default", area, at, reports}]}` |
| `rules.reload` (act) | `{ir_b64 \| source, mode?: auto \| in_place \| restart}` | `{mode: in_place \| restarted, hash, reasons, tick}` |

`running` is false with the switch off, outside a multiplayer game, for
a record that names no script, on a machine that is not the game
engine's authority, after the game's end, and for a refused script;
`refused` says which, and for a refusal names what the script needs
("the script calls set_team, whose engine entry is untraced", "no
built-in script has hash prefix ..., and fetching a blob from Live is
not built"). `hash` is the blob's SHA-256 in hex, or the record's
16-digit prefix when no blob was found. While a script runs: `script`
and `version` are its header's, `source` is `builtin` or `live`,
`carrier` the active variant record's physical index (6 on a script-index
record), `scoring` who scores the game,
`options` the record's option bits, and `authority` what
`GAME_ENGINE_AUTHORITATIVE` answered at the first tick boundary (absent
before it). `scoring` is `script`: the running script's VM scores. A
script with a `uses` line has `scoring` `carrier`; its game
([rules-variants.md](../game/rules-variants.md#a-script-that-keeps-its-carriers-rules))
runs the used type's built-in script, so `script`, `hash`, `source` and
`options` are the built-in's. `stopped` names the script error that
stopped the VM. `budget` is the VM's tick (`tick`), the ops it has used
(`ops`) against `limit` (20,000), and the verifier's `worst_tick`
estimate. `pending` counts the calls waiting for the next tick boundary.
`errors` holds the latest 16 calls the engine refused, each with its
`tick`, `call` (the entry's name) and `error`. `last_reload` is the
last `rules.reload` on this run: its `mode` (`in_place`, `restarted` or
`refused`), `hash`, `tick` and `reasons` (for a refusal, why).

`blocked` lists every problem the host's start gate found with the map
([rules-start-gate.md](../game/rules-start-gate.md)), in the check's
order, while the block holds: the game was ended in its first tick (or,
in a Forge session, the script does not run), until the next game
starts. In the lobby, with no script running and no block of the
host's, `blocked` lists the problems of the lobby's own check
(`rules::gate`, the lobby's map file against the variant's script), and
`refused` says Start Game is held (in Forge mode, that the script will
not run); the lobby's check fills no `labelled`. `labelled` is the map's labelled things as the host counted
them, for a blocked script or a running one that declares labels: each
with its `label`, `class` (`object`, `flag`, `volume` or `spawn`),
`number`, `team` (the placement's designator), `id` (the placement id
`zone.id` reads) and `declared` (the script declares that label). To
check a map against a script, choose both in the lobby as its leader,
or start a game with them, and read `rules.state`; there is no separate
census method, since the host's census is the loaded game's.

`rules.zones` lists the zones the host built for the running script, in
`id` order (empty when no script runs): each netgame flag's `id` (the
placement id `zone.id` reads), `label` and `number`, `shape` (the step
of the zone order that made it: the flag's own `area`, a hill's
`corners`, or the `default` cylinder), `area` in plain numbers
(`{radius, below, above}`, `{box: [half length, half width], below,
above, yaw}`, `{sphere, below, above}` or `{path: [[x, y], ...], below,
above}`, a path in offsets from `at` on the world's axes, a box turned
`yaw` radians with its flag, which is how it scores), `at` (the centre marker in world units) and
`reports` (this zone reports its enters and leaves; a hill's other
corners name the same zone and report none)
([gametype-markers.md](../game/gametype-markers.md), "In a script";
`Runtime::zones` in `crates/extensions/game/src/rules/runtime.rs`).

`rules.reload` puts a new script into the running game
([rules-engine.md](../game/rules-engine.md#reloading-a-running-script);
`crates/extensions/game/src/rules/reload.rs`). Give the blob
(`ir_b64`, the IR's bytes in base64, as Studio sends it) or the
`source` to compile; exactly one. With `mode` `auto` (the default) a
script that keeps the running one's state shape goes in place between
ticks, and keeps every variable's value; any other restarts the game on
the same map and variant with the new script, and `reasons` says why it
could not go in place. `in_place` refuses rather than restart;
`restart` always restarts. A restart answers once the game has taken
the queued options (up to 30 ticks later), and needs the agent's guest
calls (the `agent-calls` feature). It is refused (`REFUSED`, with
`data.why`) for a Theater film, a matchmade Live game (ranked or not),
a guest, outside a multiplayer game, for a blob that does not verify or
that this host cannot run; a Live custom game reloads in place only.

```sh
h2 agent rules.state
h2 agent rules.zones
h2 agent rules.reload '{"source":"rules \"Mine\" 1\ntype slayer\nuses slayer\n..."}'
h2 agent rules.reload '{"ir_b64":"...","mode":"in_place"}'
```

## Game variants

The lobby variant's Quick Options as its rules script marks them
([rules-variants.md](../game/rules-variants.md#quick-options-in-the-lobby)),
read at a tick boundary, and one row changed through the lobby variant
setter as the Quick Options dialog changes it.
`crates/extensions/agent/src/variants.rs` checks the params, holds the
wire types and calls the game's rows and changes
(`game::variant_quick::AgentSource`, over
`crates/extensions/game/src/variant_quick.rs` and
`variant_quick/page.rs`).

| Method and access | Parameters | Result |
|---|---|---|
| `variants.quick` (read) | `{}` or omitted | `{variant, engine, script?: {name, hash}, shown, rows: [{slot, kind: setting \| option, name, label, value, text, line, help?, values: [{value, text}]}]}` |
| `variants.quick.set` (act) | `{slot, value}` | `variants.quick`'s result, read after the change |

The variant is the lobby's (the squad session's record, else the game's
own copy); with neither it is `UNAVAILABLE`. `script` is the script the
record's block names, when this machine has it (a store variant's
bundle block counts, as at the start gate), and `hash` its SHA-256 in
hex; absent for a variant with no script, whose panel keeps retail's
rows. `shown` is true when the panel shows these rows: the
`variant-quick` switch is on ([switches.md](switches.md)) and the script
was found. `rows` are the script's `quick` marks in its order, at most
five, empty for a script with no marks: `slot` is the row's place, `kind`
a record setting of `rules::settings::TABLE` or one of the script's
options, `value` the record's value (an option's from the block's option
bits), `text` its text and `line` the panel's "Label: value". `help` is
an option's description. `values` are what the row may take: for a
setting, the values the editor offered at the last variant list load
(none before one); for an option, its range. The panel's lines
themselves are in [`ui.lobby_options`](agent-api-host.md#ui-what-the-menus-show).

`variants.quick.set` sets row `slot` of the lobby variant to `value`,
one of that row's `values`: a record setting, or the option's bits in
the block in effect (the block's CRC made again). The record is checked
with `record::refused`, then queued; the next UI update writes it and
calls the lobby variant setter (`LOBBY_SET_VARIANT`), and the answer
waits for that (`TIMEOUT` past the deferred limit). It answers with the
rows as they are then. A variant with no script this machine has is
`UNAVAILABLE`, as is a record the setter refuses ("the game refused the
variant"); a slot past the script's marks, a value the row does not
offer and a record `record::refused` refuses are `INVALID_PARAMS`, with
the reason. It changes the lobby's variant, not a save, as the dialog
does ([rules-variants.md](../game/rules-variants.md#quick-options-in-the-lobby)).

```sh
h2 agent variants.quick
h2 agent variants.quick.set '{"slot":0,"value":10}'
```

## Sim matches

Host methods while sim mode is on (`H2_SIM=<match file>`, `h2 sim run
<match> --socket PATH`; `boot::sim_glue::Sim::add_methods`, [sim.md](sim.md)).
Sim mode is a whole headless match with bots and no rendering, ending in a
report; these let a client watch it, read its numbers so far and end it.
The `sim.*` methods above are a different thing: simulated players added to
an ordinary run. `sim.guest_census` is the one `sim.*` method of a sim
match.

| Method | Access | What |
|---|---|---|
| `match.status` | read | `match` (the file), `map`, `game_type`, `bots`, `phase` (`waiting` until the match's game starts, `binding` until every distributed seat has a player and unit, `playing`, `over`), `lobby` (startup progress, below), `bound_seats` (logical seat to player index for a distributed match), `ticks` (game ticks counted since the start), `max_ticks`, `game_seconds`, `wall_s` (since the boot), `match_s` (since the first tick), `speedup` (game over match wall time), `frames` (`skipped` world frames, `rendered_world` by `render_every`, `other` menu and loading frames, `stub_faults`), `end` (`game_over`, `tick_limit`, `wall_limit`, `stopped`; `null` while it plays), `drops` (a [seam drops](sim.md#seam-drops) match's `planned` and `done` drops, its `control` (`off`, `running`, `passed`, `failed`) and its `error`; `null` without a `[drops]` table), `bounces` (a [bounce search](bounce-search.md)'s `planned` and `done` trials, `sites`, `batches`, where it is (`phase`, `batch`, `variant`), its forks (`captures`, `restores`, `waves`, `busy`, and `last_busy`, what the last busy answer named) and its `error`; `null` without a `[bounces]` table) |
| `match.report` | read | the report as it would be written now (`h2-sim-report/1`, sim.md section 6): players, kills so far, a note while the match has not ended (its `result.end` is then `aborted`) |
| `match.stop` | act | end the match: `end` becomes `stopped` (unless it had ended), the boot loop stops at the next slice and the report is written. `{stopping, end, already_over}` |
| `sim.guest_census` | read | no parameters (any other is `INVALID_PARAMS`). `{now, match}`: `now` is the guest census read at this boundary, `{local_users, local_visibility_empty, arrays: {effect, particle_system, particles, decals, lights}}`, each `null` where the read failed (`null` as a whole with no guest); `match` is the report's `guest_census` block so far (`ticks`, `local_users`, `local_visibility`, `arrays` with `samples`, `mean`, `peak`), `null` before the match's first counted tick. sim.md section 6 names every address (`game::sim_census`) |

`match.status.lobby.step` advances through `Create`, `SquadCreated`,
`SessionStarted`, `Start` and `Loading`. Its `wait` field gives the current
setup blocker or `null`. The numeric `session_type` (retail lifecycle)
and `squad_state` come from the same setup tick. A missing reading is
`null`. An allocated idle squad reads 0, and a hosted pregame squad reads
5 with session type 1 or 2. These fields retain the last setup observation
during play. Use `phase` and `bound_seats` for gameplay progress.

```sh
h2 sim run docs/runtime/sim-examples/aim-net-2v2.toml --socket /tmp/sim.sock &
H2_AGENT=/tmp/sim.sock h2 agent match.status
H2_AGENT=/tmp/sim.sock h2 agent match.report
H2_AGENT=/tmp/sim.sock h2 agent sim.guest_census
H2_AGENT=/tmp/sim.sock h2 agent match.stop
```

`session.pause` and `session.step N` step a match.

## Theater transport

The game agent lists these host methods in `session.methods`
(`crates/extensions/agent-proto/src/theater.rs`, `crates/extensions/game/src/theater/controls.rs`).
From a signed-in local multiplayer lobby, `python3 scripts/theater-e2e.py` starts and ends a game, opens its Recent film from Game Setup, exercises both cameras and the transport, and closes the player. Pass the map’s display name if it differs from the recorded map filename.

A game that records its own games plays a film in itself. `theater.watch`
queues the film's game options as the next game, as `game.load` queues a
level, and the game loads the film's map and plays the film's updates; no
process is started. The transport methods below then answer from that game
until `theater.end`, and they answer with an unavailable error until the
film's game has activated. Only one film is watched at a time. The
session's recording is parked while the film plays and resumes at the next
game, so a film being watched is not recorded. The film's simulation-scope
settings are applied when it is queued and the viewer's put back when it
ends; the viewer leaves any lobby, so a lobby's mode (Forge) does not
reach the film's game. A film recorded with other simulation hooks than
the game runs is refused before anything is queued ("This film was
recorded by another build."). The film holds its last tick when it ends;
`theater.end` queues the main menu and the game returns to it. A co-op or
networked session refuses `theater.watch`, as it refuses `game.load`.

In any other CLI-hosted game, `theater.play` with a library ID starts an
isolated playback process; subsequent transport requests through that
game agent go to its player. During startup the caller can retry after a
temporary unavailable response. The playback process also accepts calls
directly on its own `--socket` (`H2_AGENT`, else the cache's `theater.sock`).

| Method | Parameters | Result |
|---|---|---|
| `theater.watch` | `{"id":"film-id"}` or `{"path":"/films/a.h2film"}`; optional `tick`, `seat`, `camera` | on a game that records its games: `{id, length_ticks, queued}` once the film's options are queued. The film must be playable (`theater.compat`) and have a game start. `tick` plays unrendered to that tick, `seat` and `camera` choose the first view. Refused while a film is already watched |
| `theater.end` | `{}` | on a game that watches a film: queues the main menu and leaves the film; the film page opens when the menu loads. Refused with no film |
| `theater.exit` | `{}` on the library agent | Closes the active playback window. Wait for `theater.state` to return `active:false` on the original game. |
| `theater.list` | `{}` on the library agent | Local films, newest first, as an array of `{id, map, variant, started_ms, length_ticks, saved}`. Pass the ID to `theater.play`. |
| `theater.open` | `{"url":"h2recomp://theater/<film>?server=<encoded-origin>"}` | Opens the film detail page in the signed-in game and returns its `ui.screen`. Watch starts playback when pressed. Optional `clip`, `t`, `seat` and `camera` select the initial view. |
| `theater.share` | `{"id":"film-id","visibility":"link"}`, optional `clip` is a clip ID or two tick bounds | Returns `{film,url}` after publication. Visibility accepts `private`, `link` or `public`. A participant shares the existing host film by link. |
| `theater.play` | `{"id":"film-id","range":[300,900]}` to open, `{}` to resume; omit `range` for the whole film | `{id, starting:true}` on launch, otherwise transport state |
| `theater.state` | `{}` | transport state, or `{id:null,active:false}` with no player |
| `theater.pause` | `{}` | holds playback and reports state. During a seek it arranges a hold at the destination |
| `theater.compat` | `{"id":"film-id"}` | `{state, reasons, film: {xbe, contract, hooks, abi}, installed: [{stamp, state}], monitor_looks?: [{seat, map, reason}]}` |
| `theater.speed` | `{"speed":0.5}` | sets host pacing from 0.25 to 4 times realtime |
| `theater.seek` | `{"tick":900}`, optionally `"clip_marks":[300,null]` and `"source":"boot"` | when the session ring holds an entry at or before the target (and, for a target ahead, past the tick shown), the entry loads at the next frame's start and the film runs forward from it to the target (`seek.source` is `ring`; `seek.restore_ms` is the load's time once it has loaded). Otherwise a tick ahead of the one shown runs forward from it (`seek.source` is `forward`), and a tick behind, or `"source":"boot"`, queues the film's options again in the same game, and the film plays from its game's first update to the target (`seek.source` is `boot`). An entry that fails its hash, or whose digests differ before the target, turns the ring off for the film and the jump goes on by the start route (`source` becomes `boot`). The camera and timeline state carry across ([session ring](../game/theater-design.md#the-session-ring)) |
| `theater.seek` | `{"cancel":true}` | returns to the tick the active seek left and holds there. An optional `tick` is ignored. If no seek is active, returns the current state |
| `theater.keyframes` | `{}` | the session ring of the film playing: `{ring, ring_bytes, ring_cap, off?}`, the film ticks it holds an entry for (one for each stretch of 150 ticks passed), their stored bytes, the cap (`H2_THEATER_RING_MIB`), and why it is off for the film (`H2_THEATER_RING=0`, an entry that failed its check, or a player that keeps none) |
| `theater.step` | `{"ticks":1}` | accepts a positive number of ticks while paused and outside a seek. Returns the current state, runs the requested ticks, and holds at the new tick. Read state again to get the completed tick |
| `theater.camera` | `{"mode":"orbit","seat":3,"look":[0.2,-0.1],"zoom":-1,"fov":85}` | selects the view and returns transport state. An absolute film seat is 0 to 15 |
| `theater.hud` | `{"game":false,"bar":true}` | changes game HUD and Theater bar visibility and returns transport state. Omitted fields keep their values |
| `theater.start_point.capture` | `{"path":"/tmp/a.sp","key":"text","ids":"0a0b"}`; `ids` is optional hex, at most 64 bytes | on the game agent, at a tick boundary: writes the arena and the extension records as a tagged start point and returns `{path, tick, arena_hash, arena_bytes, ext_bytes, stored_bytes}`. Refused with no arena or no tick |
| `theater.start_point.load` | `{"path":"/tmp/a.sp","key":"text","film_hash":"<16 hex>"}`; `film_hash` is optional | on the game agent: checks the tag, digest, size and hash before touching the game, writes the arena and records back, then returns `{tick, arena_hash, loaded_hash, matches, error?}`. `matches` is `StartPoint::check_loaded`; a file that loads but hashes differently reports `matches:false` and the game keeps the loaded state. A refused file changes nothing |
| `theater.status` | `{}` | `{default_recording, recording, in_game, films, last_discard, notes, failure, watch_failure}` on any game or playback agent: whether the session records every game by default, the film file being written (`null` outside a game), whether a game is in play, the films sealed so far (film file paths, oldest first), why the last game went without a film, the outside changes the recording carried on through, and the failure that stopped it. `watch_failure` is why the last film watched in this game stopped before the viewer left it (a start or digest divergence): the film is left like `theater.end`, the game goes on. A start that does not reproduce names the arena pages that differ and the data arrays that own them (`… 104 arena page(s) differ, at offsets 0x0 0x1000 …; in object ×40, players ×3`), and says first when the film was recorded by another build |
| `theater.measurements` | `{}` | timings and page counts from the instrumented recorder or playback child |
| `theater.verify` | `{"from":1,"to":900}`, both optional | plays the film from its game start in an isolated, unpaced player and returns `{ticks, digests_checked, first_divergence}` |
| `theater.screenshot` | `{"scale":2,"path":"/path/view.png"}`. Both fields are optional | `{path, width, height, tick}` for a clean PNG |
| `theater.clip.mark` | `{"in":300}`, `{"out":900}`, or `{"clear":true}` | `{in, out}` with null for each unset mark |
| `theater.clip.export` | `{"from":300,"to":900,"fps":60,"format":"mp4"}`. Optional `path`. Use 30 or 60 fps and `png` or `mp4` | `{job}` once the offline player starts |
| `theater.clip.status` | `{"job":"<job>"}` | `{frames_done, frames_total, path, error, finished}` |

The start point methods are in `crates/extensions/agent-proto/src/theater.rs`
and `crates/extensions/game/src/theater/start_point_agent.rs`, over the
envelope in `theater/start_point.rs` and the state read and write in
`theater/start_point_state.rs`. They serve the experiments of slice 15 in
[theater-design.md](../game/theater-design.md): capture at tick N, run on,
load, and compare the `sim.hashes` rows that follow with the first run's.
Use them on a local game; a load writes the arena of a running match, so a
co-op or Live session would desync. Without `film_hash`, load takes the
hash the file declares, so only the tag and digest are checked.

Verification counts the ticks and the gameplay digests checked within the
inclusive tick bounds. The verifier plays from the film's game start and
reports the first divergence even when it prevents reaching the requested
range. The active player's transport remains usable. A clean run returns
`first_divergence: null`. A mismatch returns its tick and reason.

Screenshots use scale 1 by default and save the latest published game picture.
Scales 2 through 4 render the view at a larger internal raster. The independent
observer can redraw from its current state. When the redraw is
unavailable, the method reports why. The default output is a new PNG in the
film library's `screenshots` directory.

An export job copies the current camera rig into a windowless replay. The
range is half-open, in ticks from boot. The tick at `to` is excluded. Output
uses a 640×480 canvas and contains exactly `(to - from) * fps / 30` frames.
At 60 fps, a tick with one presented frame uses it twice. The timeline holds
the previous picture through skipped draws. MP4 includes the available
48 kHz stereo game mix. Host recordings with clock-only audio export with
silence.

The default destination is under the film library's `exports` directory.
A PNG export creates a directory of numbered frames starting at
`frame-000000.png`. MP4 uses the user's `ffmpeg` on `PATH`. When ffmpeg is
missing, the job reports the same reason as `h2 movies` (exit 3) and PNG
export remains available. Use a fresh destination path for either format.
`finished` becomes true after the output is published or the job fails.
`error` is null for a successful job. The job ID remains valid across seeks.
These files stay on the user's machine.

`theater.compat` is a read method on the library agent. It accepts a local
film ID or `live:<game-id>` for a cached host film. States are `playable` and
`unavailable`. Reasons are strings, and `contract` and `hooks` are null
when the recording supplies no digest. The preflight checks the XBE, the
simulation ABI and that the film has a game start; any installed image of
the XBE plays a film that passes. The replay verifies installed hooks
before entering the guest.

The rendered player starts at the clip's first tick and loops at its end.
Its overlay uses the player's agent socket for transport, camera and HUD
controls. HUD and camera changes apply to the next rendered frame.

Both picture paths use the same rig. Rendered local playback starts in
`default` with the whole retail frame. Host playback starts in `free`;
`default` on a playerless film uses the first available player's eye.
Use `orbit`, `follow` or `first-person` with a player name or film seat
to select a subject. On rendered films, first person selects the recorded
viewport. If the subject has no viewport, state reports `camera: "follow"`
and `camera_fallback: "no-recorded-viewport"` until one is drawn.
On playerless films, first person uses the unit's eye.
A Forge monitor seat that the film recorded can be named by its seat number
in `follow`, `orbit` and `first-person` even when the roster does not list
it; first person looks from the monitor's recorded pose. `theater.state`
and `theater.compat` list `monitor_looks` (`{seat, map, reason:
"monitor_look_source"}`) the machine draws as the marker; they never change
`state`.

`look` adjusts yaw and pitch in radians in free and orbit modes. In free
mode, `movement` adjusts strafe, forward and rise in world units. In orbit
mode, `zoom` adjusts the radius in world units. `fov` sets the horizontal
field of view at 4:3 in degrees (20 to 140). Omitting it keeps the current
lens. Headless playback (`--render-every 0`) accepts only `default`.
Playerless playback has no game HUD.

Transport state carries `id`, `tick`, `length_ticks`, `paused`, `speed`, `speed_reached`,
`seeking`, `compatibility`, effective `camera`, optional `camera_player` and `camera_seat`,
`hud`, `bar` and `camera_pose`. The pose has `position`, `yaw`, `pitch`,
`fov` and `radius` once there is a view. State also carries discovered kill
marker ticks in `kills` and the usual `at` stamp.
Tick zero is boot. `speed` is the requested multiplier. `speed_reached`
is completed ticks per wall second divided by the film's tick rate and
capped at the requested speed. It is null before a half-second sample
or during a seek or step, and zero while held. A paused seek holds at
its target until play. Fast-forward checks the film's gameplay digests
([theater.md](../game/theater.md)).

The desktop playback socket also exposes these host page methods. They
work while the guest is paused.

| Method | Parameters | Result |
|---|---|---|
| `ui.screen` | `{}` | `screen` names `theater.playback` or `theater.pause-menu`, with its `focus` and the UI-builder `page`. `dialog` names the open picker and its focus, or is null. `native` is false for these host-drawn pages |
| `ui.input` | `{"input":"start"}` | Applies one named page input and returns the updated screen. Inputs are `a`, `b`, `x`, `y`, `start`, `back`, `up`, `down`, `left` and `right` |

The `seek` object records `target`, `from_tick`, `source`, `requested`,
`remaining_ticks` and `restore_ms`. `seeking` is true while the film's
game is queued again, a ring entry waits to load, or the player is
running forward. A replacement target keeps the original departure tick.
The seek object remains available at the target with zero ticks left.
Linear playback starts with `seek: null`. `source` is `forward` for a run
on from the tick shown, `ring` for one from a session ring entry and
`boot` for one from the film's game start; a ring jump whose entry fails
its check becomes `boot`. `restore_ms` is the time a ring jump's entry
took to load, in milliseconds rounded up, present once it has loaded.
`requested` is the route the caller named in
`theater.seek`'s `source` (`boot`), and is absent when the player chose
the route.

`theater.measurements` is a read on the recorder's or playback child's
own socket when the run uses the Theater measurement switch
([switches.md](switches.md#tools)). Its fields are:

- `child_to_first_slice_ms` from the playback child entry to its first
  guest scheduler slice.
- `boot_to_match_start_ms` from configured boot to the first recorded
  match tick, and `match_start_tick`. These stay null when the run
  does not pass through that tick.
- `fast_forward_ms` and `fast_forward_ticks` from the first scheduler
  slice after boot to the seek target. Both are null until a forward
  replay reaches its target.
- `page_windows` with `{seconds, from_tick, to_tick, pages}` counts
  distinct physical guest pages written in each 5 s and 30 s window of
  match ticks. `page_windows_omitted` counts entries older than the 256
  retained.

```sh
h2 agent theater.state '{}' --socket "$film_run/play.sock"
h2 agent theater.pause '{}' --socket "$film_run/play.sock"
h2 agent theater.seek '{"tick":30,"source":"boot"}' --socket "$film_run/play.sock"
h2 agent theater.step '{"ticks":1}' --socket "$film_run/play.sock"
h2 agent theater.play '{}' --socket "$film_run/play.sock"
```
