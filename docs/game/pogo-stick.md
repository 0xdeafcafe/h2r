# Superbounce's Pogo Stick

Status: Partly built. Everything below is written and tested on synthetic
data; none of it has run in a game, and the Elite has no pose set of its
own.
Code: `crates/tools/content-kit` (the kit and the map rewrite),
`crates/extensions/game/src/pogo_loadout.rs` (the secondary weapon),
`pogo.rs` (the rider), `pogo_ride.rs`, `pogo_camera.rs`, `pogo_pose.rs`,
`pogo_pose_glue.rs`, `pogo_sound.rs`, `pogo_agent.rs`, `pogo_mode.rs` (the
features). Tests: `crates/tools/content-kit/src/tests.rs` and
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

`content/pogo/` is the `pogo` package (`package.toml`, baked into `h2`). It
holds each look as a parametric model source (`models/<look>.yaml`:
lathes, sweeps and boxes on two nodes) and a painted 512x256 atlas
(`models/<look>.png`), all original
([content/pogo/README.md](../../content/pogo/README.md)).
The looks are `unsc` (a stamped olive field spring with a coil and a
chromed piston), `covenant` (a curved shell over a plasma column) and
`forerunner` (a six-sided alloy strut over a core of light and rings).
All three share one rig (`content_kit::RIG`): the grips, the foot pegs, the
foot and the piston's travel are in the same places, so a rider is posed
the same on each.

The package's `model` item per look and its `tag` item for the weapon build
a look into the tags a map carries it as (`workspace::PackageRewrite`,
below); `crates/tools/content-kit` holds the builder's code
(`kit::install_model`) and embeds the same sources for `inject::add_pogo`,
which the package's build is tested against:

| Tag | From |
|---|---|
| `bitm` | the atlas, as a compressed mip chain (`texture::bitmap`) |
| `shad` | written from a shader template's definition the map has, with the atlas as its base map (`h1convert::shad::Library`, `kit::LAYOUTS`); no retail shader is copied |
| `mode` | the mesh (`mesh.rs`) through `h1_assets::models::mode::convert_model`: two nodes, `frame` and `piston`, and the five markers |
| `hlmt` | `h1objects::rebuild::model`, with one of the map's global materials |
| `weap` | the `tag` item `tags/pogo_<look>.weap.yaml`: the definition's defaults with the model, an empty HUD, the weapon class and type, and the first-person block `borrow`ed from the map's own weapon of that class (`workspace`'s `tagitem`; `kit::weapon` is the same record in code) |

The three looks share one physics model and one empty weapon HUD
(`kit::install_shared_at`, added by the first `model` item), so a map gains
17 tags. The weapon is inert: no
triggers, barrels, magazines, melee damage or zoom.
It is held as a weapon class both player animation graphs already have an
`idle` for, because a unit refuses a weapon whose class its graph lacks
([objects.md](../halo1/objects.md)): the magnum's, `pistol` and `hp`
(`kit::PREFERRED_CLASS`), named in each weapon's tag file. The package's
build does not read the player graphs; `inject::add_pogo` does
(`kit::player_class`), and the ignored disc test
`every_multiplayer_map_holds_the_class` says whether every map has the
magnum's class.

Held, the game reads a weapon's first-person block (its arms and weapon
model and their graph) by player character, for its first-person camera
and draw. The stick's block is a copy of the map's own weapon of the same
class (the tag file's `borrow`: the weapon of that class and type, else
the first of the class; `kit::first_person` is the same choice in code),
so the references are to tags the map already has and the stick is held
in that weapon's first-person view. A map with no weapon of the class is
refused by name, as one with no room is.

### The map rewrite

The stick reaches a map through the one rewrite of the game's packages,
`workspace::PackageRewrite` (`crates/apps/boot/src/packages.rs`, the last
rewrite in boot's chain), applying the `pogo` package's items to each
multiplayer map (`maps = "multiplayer"`, by the map's header) as
`workspace::build_map` does for `h2 package build`. The same map and
package give the same bytes on every machine, and the bytes equal
`inject::add_pogo`'s less the version string (`pogo_tests.rs` in
`crates/tools/workspace`).

- **Cache.** `<data>/packages/cache/<map sha>-<key>.splice` with a `.json`
  receipt ([studio-redesign.md](../editor/studio-redesign.md), "How the
  game consumes packages"). The key covers the package's id, so a changed
  source, atlas or tag file builds anew.
- **Refusal.** A map with no room, with no shader template the model can
  be written on, or with no weapon of the class to borrow from is served
  as it is and named once on stderr (`packages: <map> is served without its
  packages: <why>`). A map that already holds the stick's tags is left
  alone.
- **Off.** The package's `features = ["pogo"]` follows the `pogo`
  feature: with it off the package is not in the set and no map gains a
  tag.
- **Placed.** The `PackageRewrite` hands each map it is asked for, as the
  game is given it, to `content_kit::served::publish`, which reads the three
  `weap` datums from the map and records them in `content_kit::placed`,
  under the map file's stem and the name in its header. A map without all
  three (not multiplayer, refused, off) has none, and then nothing below
  happens on it.
- **The kit's version.** `KIT_VERSION` (in `crates/formats/pogo`) is 2
  and is the version of `inject::add_pogo`'s output, which interns
  `pogo_kit_v<n>` in the map it builds (Studio's `model.*` with `pogo:
  true` uses it). The package's build interns no string; a changed
  package is a changed `PackageId`.

### The loadout

The Superbounce script (`content/modes/rules/superbounce.rules`)
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

While the chase camera draws, the first-person arms and weapon are left
out (a hook on `points::FIRST_PERSON_RENDER` returns as the Blind skull's
does). When the game's view does not read as a camera (no finite
position and aim, or no lens), the camera is placed from the body's eye
and aim of the last tick instead (`pogo_camera::body_eye`) and the frame
is counted in `pogo.state` (`camera.fallback_frames`), so a degenerate
first-person view is never what is drawn.

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
and the bone names with their aliases are in `content/pogo/data/poses.yaml`,
hand-keyed. A biped's bone names are read from the map file once per map
and model, off the game thread (`pogo_skeleton.rs`); a skeleton missing a
bone the pose needs is left in the game's pose, and the reason is logged
once.

The rows are written where both render paths read them: into the object
bodies, in a hook before `points::RENDER_VIEWS`, and put back in a hook
after it (`pogo_pose_glue.rs`). The simulation never sees a posed row.

### Sounds

`pogo_sound.rs` plays three clips of our own (`content/pogo/sounds/compress.wav`,
`spring.wav`, `settle.wav`, synthesised by `scripts/pogo-sounds.py`) as 2D
host buffers through the mixer, as the medals' announcer lines are: the
piston going in on a touchdown, springing out on a bounce, and easing out
when no bounce comes. They play for the local players only, up to three
at once.

- **Level.** A host buffer is mixed at its own volume alone: none of the
  sound class gain, mix-bin volumes and headroom the game's voices pass
  through. The clips peak at `PEAK` (0.7 of full scale, the script's), so
  each plays at `level_mb`: the spring -12 dB, the compress -14 dB and the
  settle -20 dB, all under the game's weapons.
- **Once.** A cue started less than `RETRIGGER_TICKS` (3) ago is dropped,
  so two local players landing on one tick give one clank.

## Interfaces

- **`h2 pogo kit MAP [--out FILE]`** builds one map's kit and prints the
  cost and the placed tags; **`h2 pogo models [--out DIR]`** writes the
  three meshes as OBJ files, with no disc (`crates/apps/cli/src/pogo_cmd.rs`).
- **`pogo.state`** and **`pogo.test`** on the agent API
  ([agent-api.md](../runtime/agent-api-world.md#pogo-superbounces-pogo-stick)):
  the options, the map's weapon tags, every rider's phase and trick and
  the camera; and a trick shown on the local player's stick.
- **Guest boundary.** One hook changes the simulation:
  `points::STARTING_WEAPON_PICK` (the feature `pogo`). The feature
  `pogo-view` hooks `points::RENDER_VIEWS` twice (the camera's private
  view; the rows written and put back) and `points::FIRST_PERSON_RENDER`
  (left out under the chase camera), and is presentation only.
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
- **Test.** The sounds: the `tests` module of `pogo_sound.rs` (each
  clip's peak, the level each plays at, a cue sounding once in three
  ticks, the oldest voice giving way).
- **Test.** The kit: `content-kit`'s `tests.rs` (the rig's markers, the
  triangle budget, outward normals, the tags read back, the first-person
  block copied from the class's weapon and the refusal without one) and
  `inject_tests.rs` (the references resolve, the same bytes twice, the
  refusal). The package: `pogo_tests.rs` in `crates/tools/workspace`
  (on `content_kit::fixture`'s map the package builds the tags and the
  bytes `kit::install_all` builds; the weapon borrows the pistol's
  first-person block; the rewrite serves it, says which weapons it holds
  and leaves a map that has the stick alone; the `sound` and `model`
  builders' refusals).
- **Test.** The loadout's decision: the `tests` module of
  `pogo_loadout.rs`; the script's options: `superbounce_scores_bounces_only_by_their_chain`
  in `crates/formats/rules/tests/engines.rs`.
- **Test.** The camera: the `tests` module of `pogo_camera.rs` (the
  record matches a host camera's for the pose, the body's eye when the
  view is not a camera, the first-person draw left out only under the
  chase camera).
- **V**, not yet run: `every_multiplayer_map_holds_the_class` (with the
  first-person graphs' idle) and
  `every_multiplayer_map_has_the_room` in `content-kit` (`#[ignore]`, read
  `$H2_DISC`).

## Not verified

- A rewritten multiplayer map loads, and peers accept each other's.
- The unit accepts the stick at spawn, and Y swaps to and from it, with
  the borrowed first-person block (the stick shows the class weapon's
  arms in first person with `-pogo-view`).
- The chase view is not black: `pogo.state`'s `camera.drawn` true and
  `camera.fallback_frames` 0 while the stick is held.
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
- The sounds' level against the game's weapons and footsteps, by ear.
- Every multiplayer map has the tag memory and the shader template.

## Related

- [superbounce.md](superbounce.md): the game type, the medal chain the
  tricks follow, and the script's options.
- [rules-engine.md](rules-engine.md): how the host reads a script's
  options.
- [game-options.md](game-options.md): the starting weapon bytes.
- [forge-design.md](forge-design.md): the host camera.
- [notes/2026-11-23-pogo-static-reads.md](notes/2026-11-23-pogo-static-reads.md).
