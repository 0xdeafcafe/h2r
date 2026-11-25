# Rebuilt objects: game runs on converted Blood Gulch

Evidence for [../rebuild.md](../rebuild.md). Each entry is a run of the
converted Blood Gulch (the multiplayer map on the Coagulation base) in a
local game, read through the agent API (`h2 agent`: `havok.bodies`,
`sim.state.set`, `edit.get`, `edit.tags`, live edits of tag fields). The
runs predate the fixes that the design page now states; each entry names
the converter version of the image it ran on and the commit that
introduced the fix. To repeat one, convert Blood Gulch at the commit
before the fix and drive the same object.

## 2026-11-19: scenery has no Havok body

Image: converter version 90, commit `5d1fe496d`. `havok.bodies` lists
the eight vehicles, the player and the level, and no body for any of the
50 scenery placements, though the map holds the boulders' `phmo` tags
(`edit.tags phmo`). The player put (`sim.state.set`) at the middle of
`boulder_doublewide` and of `boulder_crouch` drops through each to the
ground under its origin (z 0.32 and 0.10, the boulders' origins 0.31 and
0.09). So the scenery `phmo` built from version 85 to 91 was inert; the
engine reading that explains it is on the design page.

## 2026-11-19: vehicle runs on converted Blood Gulch

Images: converter versions 75 to 93, fixes at the commits named below.

### Hover points on another node

Version 93 (fixed in version 94, `2375dcb02`). A hover point whose marker
is on another node gives no lift. With a driver the ghost rode nose up at
19.9 degrees on its tail, its origin 0.17 above the ground. The back
point at eight times its strength changed nothing, while zeroing the
front pair dropped the nose, and naming any node-0 marker for the back
point lifted the tail. The same marker moved live to node 0 at the same
place in the object (node 0 translation (-0.643, 0, -0.091)) held the
ghost at pitch -7.7, 0.33 above the ground, as the retail ghost rides
(pitch -7.9, 0.39), and it drove.

### Marker rotation

The tire and tread markers, edited live to node 0 and facing out along
their axles before the warthog spawned, left its path from the spawn
unchanged to the hundredth. A `phmo` centre-of-mass edit did the same, so
the engine likely reads both at load and the run decides neither.

### Friction point order

Version 76 (fixed in version 77, `7c743c355`). Halo 1 lists the warthog's
back tires first and the retail phantom spheres run front first, so each
front marker took a back sphere's compression: as the nose sank the back
spheres rose and the front springs let go, and the converted warthog
pitched onto its nose at spawn (forward z -0.93). Paired by position both
Blood Gulch warthogs stood level (forward z 0.08 and 0.14 on the slopes)
and slept within 70 ticks.

### Control field

Version 75. Written 0 (the role took 4-byte spans only), the converted
scorpion crept at 0.05 units a second with the stick held forward; set to
2 live it drove at 2.5. Fixed with `rebuild::VEHI_CONTROL`.

### Wheels and treads

Versions 77 to 85 left the wheels and treads out of the body, and the
owner walked through the converted scorpion's treads though its `coll`
carried them (fixed in version 86, `ea511ed12`). Before version 77, with
the tires whole in the body, their lowest point 0.018 below the origin
sank up to 0.144 into the ground at spawn and the depenetration threw the
warthog about 4.8 units.

## 2026-11-19: weapon firing effects

Image: converter version 83, commit `d278af342`. `h2 agent edit get`:
each of the eight converted hand weapons' `firing_effects` elements was
datum for datum a retail weapon's, and the three converted vehicle guns
had none (fixed in version 97: each barrel's effects are Halo 1's). The
shot counts (`+0` of both elements): every retail element, 33 weapons'
worth, holds i16 1, 1.

## 2026-11-19: weapon sounds and tag memory

Commits: `7074e4c63` (firing and reloading effects from Halo 1's
triggers), `9d5e588a9` (a firing effect named as a bare `snd!`),
`e869cbca7` (the tag-memory map). Method: the live check below;
`tests/weapon_sounds_disc.rs`; h1campaign's `every_campaign_level_converts`
(the sweep's tag-memory and sample-byte figures).

- Live (`h2 agent` `edit get`, converted Blood Gulch, converter version
  83, before the firing effects were taken from Halo 1's triggers): each
  of the eight converted hand weapons' `firing_effects` elements was
  datum for datum a retail weapon's, and the three converted vehicle guns
  had none.
- Live (same run): every retail `firing_effects` element on the map, 33
  weapons' worth, holds i16 1, 1 at the shot counts (`+0` of both
  elements).
- The campaign sweep installs the objects' sounds with the gestalt and
  builds all ten levels fresh at lean `Everything` with 0 dangling
  references: 0 to 1 effect sounds and 0 to 11 looping sounds per level,
  at most 0x1f8ad4 sample bytes (b40), no file size retry; tag memory at
  most 0x942000 of 0xc24000 (b40; 0x918000 before the effects lane) and
  at most 0x923000 with rebuilt bipeds alone.

## 2026-11-19: scenery solid as level collision

Commits: `48b444773` (a `phmo` built from scenery collision), `8c7222add`
(each body's size class by its bounding radius), `3368ffedb` (the
walk-through boulders recorded), `31156269f` (the `phmo` dropped, the
collision merged into the structure). Method: `h2 agent` `havok.bodies`,
`sim.state.set`, `edit.tags phmo` on converted Blood Gulch; the retail
censuses over coagulation, beavercreek and 05a_deltaapproach.

- `havok.bodies` (converter version 90) lists the eight vehicles, the
  player and the level, and no body for any of the 50 scenery
  placements, though the map holds the boulders' `phmo` tags
  (`edit.tags phmo`); the player put (`sim.state.set`) at the middle of
  `boulder_doublewide` and of `boulder_crouch` drops through each to the
  ground under its origin (z 0.32 and 0.10, the boulders' origins 0.31
  and 0.09).
- Retail holds both forms: 05a has 11 scenery with collision and no
  `phmo`, 5 with one; beavercreek 6 and 3; coagulation 0 and 6.
- The scenery bodies' words on coagulation: all six bodies flags 0,
  motion 2 (`+0x18`, `+0x1a`), and each body's size class `+0x1e` by
  its bounding radius `+0x14`: 3 from 0.30 to 0.70, 4 from 0.73 to
  1.23, 5 from 1.39 to 1.96, 6 from 2.13 to 9.73 (every rigid body of
  every `phmo` on the three maps).

## 2026-11-18: scenery from definitions on the disc

Commit: `c1a8c1fa2` (the scenery template path removed). Method:
`tests/rebuild_disc.rs`, the commands in [../rebuild.md](../rebuild.md);
the template comparison was made at the commit that removed the path.

- `every_scenery_rebuilds_from_its_definition`: 237 scenery objects (the
  ten levels on 05a, the multiplayer maps on coagulation and
  beavercreek) convert; every `coll` (109) has a BSP physics slot per
  BSP; every model is predicted first, 545 bitmaps after.
- `retail_scenery_has_bsp_physics_and_predicts_its_model`: `hlmt`
  `block_58` and `block_84` empty on 26 and 29 of 05a's 56 scenery.
- The scenery words of the base maps (the spans the role words later
  dropped): on 05a (56 scenery) the object header's `0x6c..0x8c` (nine
  floats, the same words as the turret role's `0x6c..0x74`) and the
  `hlmt`'s `0x34..0x44` (five decreasing floats); coagulation (12) and
  beavercreek (17) give the same object words (beavercreek also `0x1c`)
  and no `hlmt` majority.
- The template path (Delta Halo's `statue_shard_large`) against the
  rebuild, 187 objects with the template on their base (the 50 on the
  multiplayer bases had none, since neither carries the statue): every
  field agreed but these, all the statue's own.

| Field | Template path | Rebuilt |
|---|---|---|
| `scen.object.predicted_resources` | the statue's 8, pinned | the object's model and bitmaps |
| `hlmt.block_58`, `hlmt.block_84` | the statue's one element each, pinned | empty (retail-valid) |
| `scen.object.unknown_18`, `scen.pathfinding_policy`, `hlmt.unknown_28`, `unknown_2c`, `unknown_a4`, `unknown_e4` | the statue's values | zero (no retail majority) |
| null references (`bloc`, `shad`, `effe`, `foot`, `phys`, `udlg`) | group word kept, datum null | `0xffffffff` both |

  The `coll` was the same tag on both paths.

## 2026-11-18: devices placed from their definitions

Commit: `fd2d75d70`. Method: `every_device_placement_lands_with_its_device_groups`
(h1campaign `disc.rs`) and the device sweep over the ten levels.

- On every level, every Halo 1 machine, control and fixture placement
  lands (1,596 machines, 840 fixtures, 77 controls on the disc), and all
  496 Halo 1 names of devices name their placement.
- Counted per level, 86 of the ten levels' 156 device tags get a `phmo`
  from their collision; the rest have no collision or none with solid
  space (most light fixtures).

## 2026-11-18: the solid space of the collision BSPs

Commit: `afd0651b1`. Method: `collision_disc.rs`
`halo1_solid_space_becomes_physics_pieces`, every collision model of the
ten levels.

16,189 cells. Just outside a surface Halo 1's own tree finds open space
51,729 times and solid 179; just inside solid 50,591 times and open
1,317 (parts thinner than the probe step, open meshes). At 142,528
points through the BSP boxes the tree and the cells agree on all but
117 (points on a face).
