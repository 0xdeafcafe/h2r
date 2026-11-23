# Superbounce's Pogo Stick

Status: Partly built. Everything below is written and tested on synthetic
data; none of it has run in a game, and the Elite has no pose set of its
own.
Code: `crates/tools/pogo-kit` (the kit and the map rewrite),
`crates/extensions/game/src/pogo_loadout.rs` (the secondary weapon),
`pogo.rs` (the rider), `pogo_ride.rs`, `pogo_camera.rs`, `pogo_pose.rs`,
`pogo_pose_glue.rs`, `pogo_sound.rs`, `pogo_agent.rs`, `pogo_mode.rs` (the
features). Tests: `crates/tools/pogo-kit/src/tests.rs` and
`inject_tests.rs`, `pogo_tests.rs`, `pogo_pose_tests.rs`, and the `tests`
module of each other file.
Switch: the `pogo` and `pogo-view` features (on), listed in
[switches.md](../runtime/switches.md).

A Superbounce player spawns with the magnum and a Pogo Stick as the
secondary weapon. The stick is a real weapon of our own in the map, in
one of three looks a script option picks, and it does nothing as a
weapon. Held, it puts the camera behind the player; on the ground it is
carried in the hands, and in the air the player rides it, with a trick
for each bounce of a chain. The one rule: the stick changes the game only
through the weapon a spawn is given. The pose, the camera, the tricks and
the sounds are drawn over the game and never reach the simulation, the
netcode or a film.

## How it works

### The kit

`art/pogo/` holds each look as a parametric model source (`<look>.yaml`:
lathes, sweeps and boxes on two nodes) and a painted 512x256 atlas
(`<look>.png`), all original ([art/pogo/README.md](../../art/pogo/README.md)).
The looks are `unsc` (a stamped olive field spring with a coil and a
chromed piston), `covenant` (a curved shell over a plasma column) and
`forerunner` (a six-sided alloy strut over a core of light and rings).
All three share one rig (`pogo_kit::RIG`): the grips, the foot pegs, the
foot and the piston's travel are in the same places, so a rider is posed
the same on each.

`crates/tools/pogo-kit` embeds the sources and builds a look into the
tags a map carries it as (`kit::install_from`):

| Tag | From |
|---|---|
| `bitm` | the atlas, as a compressed mip chain (`texture::bitmap`) |
| `shad` | written from a shader template's definition the map has, with the atlas as its base map (`h1convert::shad::Library`, `kit::LAYOUTS`); no retail shader is copied |
| `mode` | the mesh (`mesh.rs`) through `h1_assets::models::mode::convert_model`: two nodes, `frame` and `piston`, and the five markers |
| `hlmt` | `h1objects::rebuild::model`, with one of the map's global materials |
| `weap` | the definition's defaults with the model, an empty HUD and a weapon class (`kit::weapon`) |

The three looks share one physics model and one empty weapon HUD
(`kit::install_shared`), so a map gains 17 tags. The weapon is inert: no
triggers, barrels, magazines, melee damage, zoom or first-person model.
It is held as a weapon class both player animation graphs already have an
`idle` for, read from the map when the kit is built (`kit::player_class`),
because a unit refuses a weapon whose class its graph lacks
([objects.md](../halo1/objects.md)).

### The map rewrite

`pogo_kit::rewrite::PogoRewrite` is a `vfs::Rewrite` in the boot's chain
(`crates/apps/boot/src/lib.rs`, after the UI screens). For each map the
game opens whose header says multiplayer, it decomposes the tags, adds
the kit and builds the map again (`inject::add_pogo`). The same map and
kit give the same bytes on every machine.

- **Cache.** `<data>/cache/pogo/<map>.map` with a `<map>.json` receipt,
  keyed by the SHA-256 of the map's name, `KIT_VERSION`, the embedded
  sources and atlases, and the map's bytes. A boot after the first reads
  the file back.
- **Cost and refusal.** `inject::Costs` is the tag memory the kit takes
  against what the map type allows (`map::writer::max_metadata_size`). A
  map with no room, with no shader template the kit can write on, or
  larger than a rewrite reads (`vfs::MAX_REWRITE_BYTES`) is served as it
  is and named once on stderr (`pogo: <map> is served without the pogo
  stick: <why>`).
- **Placed.** The three `weap` datums of each rewritten map are recorded
  in `pogo_kit::placed`, under the map file's stem and the name in its
  header. A map served without the kit has none, and then nothing below
  happens on it.
- **The kit's mark.** A rewritten map interns `pogo_kit_v<n>`, so the
  kit's version changes the map's bytes.

### The loadout

The Superbounce script (`crates/formats/rules/src/builtin/superbounce.rules`)
declares two options after the HUD's: `pogo` (a bool, on) and
`pogo_style` (`unsc`, `covenant` or `forerunner`; default `unsc`). Its
`secondary_weapon` stays `none`.

The spawn's weapon pass maps the variant's primary and secondary weapon
bytes to tags through one routine (`points::STARTING_WEAPON_PICK`), and a
byte can only name a slot of the multiplayer globals' list. The hook on
that routine (`pogo_loadout::install_hooks`) answers its calls for the
secondary, told apart by return address
(`pogo_loadout::SECONDARY_RETURNS`), with the stick's tag when the game's
script is Superbounce's, `pogo` is on and the map has the kit
(`pogo_loadout::secondary`). The pass then makes the object and adds it
to the unit as it does any weapon. Whatever the secondary byte says, the
stick takes the slot.

The pick is the simulation's, so it is read from guest memory (the
variant's extension block and the script's option bits, which a guest
takes from its host) and from the map's bytes, and from nothing else
(`pogo_loadout::decide`). The routine has no player argument: every spawn
of a game gets the same look.

### Phases and tricks

`pogo::Rider` is one player's state, stepped once a tick; `pogo::Riders`
keeps one per slot. `superbounce_mode::on_tick` steps them
(`pogo_ride::on_tick`) after the medal chain, with the players the chain
read and the awards it filed on the tick, plus one read more of each
player: the weapon held, and which of the map's sticks it is
(`pogo_loadout::held_style`). It runs for all sixteen players on every
machine, so everyone sees everyone's tricks.

| Phase | When | Next |
|---|---|---|
| `carried` | on the ground | `mount` after `AIR_TICKS` (3) ticks airborne in a row |
| `mount` | getting on, over `MOUNT_TICKS` (6) | `ride`; a touchdown or a set-down as below |
| `ride` | in the air, knees tucked | `compress` on a touchdown (`superbounce_charge::touchdown`); `dismount` when set down with no fall |
| `compress` | the piston in, the verdict not in yet | `spring` on the chain's bounce; after `VERDICT_TICKS` (4) with none, `dismount` on the ground or `ride` still in the air |
| `spring` | the piston out over `SPRING_TICKS` (3), the flare fading | `ride` |
| `dismount` | getting off, over `DISMOUNT_TICKS` (8) | `carried`; `mount` on a takeoff |

A player who puts the stick away, or dies, has no rider. The chain files
a bounce when its launch window closes, three ticks after the touchdown,
which is why the piston waits in.

The trick of a flight is the bounce's chain count (`pogo::trick_for`):

| Chain | Trick |
|---|---|
| 1 | No-Hander |
| 2 | Can-Can |
| 3 | Bar Spin |
| 4 | Superman |
| 5 | Backflip |
| 6 and over | Double Backflip |
| a Monster Bounce, any count | Front Flip |

It is timed from the vertical speed (`pogo::trick_progress`): half way at
the apex, taking `TRICK_SHARE` (0.7) of the hang time up to the trick's
longest (`Trick::longest`, 0.9 to 1.5 s, since a high bounce hangs for
seconds). A flight under `MIN_HANG` (0.9 s) gets the tuck alone. There
are no points and no button for a trick.

### The camera

While the first local player holds a stick, the world is drawn from a
chase camera (`pogo_camera.rs`): the player's own first-person camera,
read from the view the game built, moved back along the aim by `BOOM`
(0.95 units) and up by `RISE` (0.22). It is the host camera Forge and
Theater use (`spectator_view::draw_world_view`): the renderer gets a
private, playerless copy of the view, the director stays in first person,
and the aim, the bots and the netcode read what they always do. Putting
the stick away is first person again on the next frame.

Each tick `pogo_camera::on_tick` finds how much of the boom the level
leaves room for (`pull_in`: line-of-sight tests against the collision the
superbounce scan read, a halving search when the full boom is blocked).
Each frame the boom eases toward it (`Boom`): in at once, back out at
`EASE_OUT` a second. The camera stands down in split screen and under
another host camera (Forge's monitor, Theater, the agent's spawn view).

### The pose

`pogo_pose::pose` is a pure function from a biped's skeleton, its world
node rows, the stick's two rows and a rider's `Sample` to posed rows for
both. Carried with the piston out, the rows are the game's own (the
weapon class's carry animation). Mounted:

- the body frame is the biped's root row, upright, at its yaw;
- the stick stands under the rider on the body's centre line, its
  handlebar across the body;
- the pelvis sits over the pegs, and the spine, neck and head take keyed
  rotations over the rest pose;
- each leg and each arm is a two-bone solve, so the feet are on the pegs
  and the hands on the grips whatever the skeleton's proportions;
- `compress` slides the piston into the frame by up to `RIG.travel`: the
  foot stays on the ground and the frame and the rider come down;
- `mounted` blends every row between the game's and the mounted one, so
  mounting and dismounting are transitions;
- a trick is drawn over the mounted pose by its progress, eased in and
  out so it leaves from and returns to the mounted pose. The flips turn
  the rider and the stick together about the pelvis by whole turns.

The numbers (heights, lean angles, the hands-off and feet-off targets)
and the bone names with their aliases are in `art/pogo/poses.yaml`,
hand-keyed. A biped's bone names are read from the map file once per map
and model, off the game thread (`pogo_skeleton.rs`); a skeleton missing a
bone the pose needs is left in the game's pose, and the reason is logged
once.

The rows are written where both render paths read them: into the object
bodies, in a hook before `points::RENDER_VIEWS`, and put back in a hook
after it (`pogo_pose_glue.rs`). The simulation never sees a posed row.

### Sounds

`pogo_sound.rs` plays three clips of our own (`art/pogo/compress.wav`,
`spring.wav`, `settle.wav`, synthesised by `scripts/pogo-sounds.py`) as 2D
host buffers through the mixer, as the medals' announcer lines are: the
piston going in on a touchdown, springing out on a bounce, and easing out
when no bounce comes. They play for the local players only, up to three
at once.

## Interfaces

- **`h2 pogo kit MAP [--out FILE]`** builds one map's kit and prints the
  cost and the placed tags; **`h2 pogo models [--out DIR]`** writes the
  three meshes as OBJ files, with no disc (`crates/apps/cli/src/pogo_cmd.rs`).
- **`pogo.state`** and **`pogo.test`** on the agent API
  ([agent-api.md](../runtime/agent-api.md#pogo-superbounces-pogo-stick)):
  the options, the map's weapon tags, every rider's phase and trick and
  the camera; and a trick shown on the local player's stick.
- **Guest boundary.** One hook changes the simulation:
  `points::STARTING_WEAPON_PICK` (the feature `pogo`). The feature
  `pogo-view` hooks `points::RENDER_VIEWS` twice (the camera's private
  view; the rows written and put back) and is presentation only.
- **From the superbounce mode.** `superbounce_mode::on_tick` calls
  `pogo_ride::on_tick`, `pogo_sound::play` and `pogo_camera::on_tick`;
  its resets call `pogo_ride::reset`. With the `superbounce` feature off
  the stick is still given and carried, with no pose, camera or sound.

## Rules and limits

- **Every machine or none.** The rewritten map and the loadout hook are
  the simulation's. A machine running `-pogo` in a game whose host gives
  sticks does not hold the weapon's tag. Nothing checks this at join
  time.
- **A film** made with the stick plays back only with the `pogo` feature
  on and the same kit version: the stick's tag is in the rewritten map.
- **One look a game**, the script's; not per player, species or team.
- **Split screen** has no chase camera; the pose is still drawn.
- **Not in a snapshot.** After a state load or a Theater seek a rider
  starts from the next tick: a trick in flight is dropped.
- **The flare** at the foot on a bounce is in the rider's `Sample` and in
  `pogo.state`, and nothing draws it.
- **The Elite** is posed with the Spartan's keys through the bone
  aliases; a skeleton they do not fit stays in the game's carry pose.
- **A model the D3D path draws** is posed too: both paths read the body's
  rows.
- **The chase camera's wall test** runs up to seven line-of-sight tests a
  tick, only while a local player holds a stick.

## Evidence

The image reads behind the loadout, and the code reads behind the camera
route and the row override, are in
[notes/2026-11-23-pogo-static-reads.md](notes/2026-11-23-pogo-static-reads.md).

- **Test.** The rider: `pogo_tests.rs` (the phases on lists of ticks, the
  trick of each count, the apex timing, the short bounce).
- **Test.** The pose: `pogo_pose_tests.rs` (feet on pegs and hands on
  grips, bone lengths kept, the piston's travel, each trick leaving from
  and returning to the mounted pose, the flips rigid).
- **Test.** The rows put back byte for byte: the `tests` module of
  `pogo_pose_glue.rs`.
- **Test.** The kit and the rewrite: `pogo-kit`'s `tests.rs` (the rig's
  markers, the triangle budget, outward normals, the tags read back) and
  `inject_tests.rs` (the references resolve, the same bytes twice, the
  refusal, the cache).
- **Test.** The loadout's decision: the `tests` module of
  `pogo_loadout.rs`; the script's options: `superbounce_scores_bounces_only_by_their_chain`
  in `crates/formats/rules/tests/engines.rs`.
- **V**, not yet run: `every_multiplayer_map_holds_the_class` and
  `every_multiplayer_map_has_the_room` in `pogo-kit` (`#[ignore]`, read
  `$H2_DISC`).

## Not verified

- A rewritten multiplayer map loads, and peers accept each other's.
- The unit accepts the stick at spawn, and Y swaps to and from it; a
  weapon with no first-person model does not fault on ready or swap.
- The hook fires on both secondary calls in the recompiled image.
- A guest and a film's playback make the same pick as the host.
- The stick dropped on death, and picked up.
- The carry pose with the chosen weapon class, and the stick's lighting.
- The chase view draws the player's own body and the HUD, and where the
  reticle sits against the aim.
- The real Spartan's and Elite's bone names resolve through the aliases.
- The riding pose and each trick as they look; nothing has been seen.
- The posed body is not culled by its bounding sphere, and nothing in
  render setup poses the body again after the hook.
- The sounds' level against the game's mix.
- Every multiplayer map has the tag memory and the shader template.

## Related

- [superbounce.md](superbounce.md): the game type, the medal chain the
  tricks follow, and the script's options.
- [rules-engine.md](rules-engine.md): how the host reads a script's
  options.
- [game-options.md](game-options.md): the starting weapon bytes.
- [forge-design.md](forge-design.md): the host camera.
- [notes/2026-11-23-pogo-static-reads.md](notes/2026-11-23-pogo-static-reads.md).
