# Halo 1 object conversion: game-run observations

Evidence for [../objects.md](../objects.md). Each entry is a run of the
game on a converted Blood Gulch through the agent API (`render.frame`,
`havok.bodies`, `sim.state.set`, `h2 agent edit`), read at the commit named.

## 2026-11-19: the whitebark's canopies (cut-out leaf shaders)

Commit: `78d2887c5`. Converted Blood Gulch, converter version 91: the
whitebark tree's canopies drew as flat near-black diamonds, the clear part
of each card showing the dark colour behind the depth, not the cliff. The
leaf shader's slot 0 (the bump map) was `default_vector`, opaque, and the
colour stem's depth pass (`shadow\shadow_generate`) samples slot 0 with
alpha kill. Pointing slot 0 at the leaf bitmap itself (`h2 agent edit set`)
cut the canopies into the leaf's star shape at once. Retail's leaf bump
(`tree_cliff_leaf_bump`) is P8 bump with index 255 exactly where the colour
map is clear: 24,367 of its 24,396 texels with colour alpha below 128 are
255, none above 159.

## 2026-11-19: the Blood Gulch ghost

Commit: `2375dcb02` (converter version 94). A report on converted Blood
Gulch (versions 90 and 91) that the ghost's physics were wrong and it looked
like its damage model; one ghost lay nose down at pitch -10.8, partly in the
ground. Checked in a game run and against the discs:

- Model: Halo 1's `ghost` model has one region `__unnamed` with `__base`
  (geometries 3, 3, 2, 1, 0 from super low to super high, shaders 0 to 2)
  and `~damaged` (3, 7, 6, 5, 4, shaders 3 to 5). The game drew 1,082, 28
  and 109 vertex parts (`render.frame` draws): geometry 0, `__base` at super
  high, with the `ghost hull metal`, `ghost self illuminated` and `ghost
  hull diffuse shine` shaders. Next to a spawned retail ghost on flat ground
  the two look alike; the "fragment" was the ghost lying on a slope.
- At rest: its `phmo` is Halo 1's collision (lowest point 0.134 below the
  origin; the retail ghost's 0.019) with Halo 1's `phys` mass 2000 (retail
  907), centre of mass (0.376, 0, 0.133) from the pieces (Halo 1's own
  0.362, 0, 0.108; retail 0.287, 0, 0.177). Spawned on flat ground the
  converted ghost's origin was 0.13 above it at pitch -2.8, the retail
  one's 0.01 at -2.3. The nose-down ghost lay on a slope falling 7 degrees
  forward with its hull on the ground.
- Driven: the converted ghost rode at pitch +19.9, its tail on the ground;
  the retail ghost rides at -7.9, 0.39 above it. The back hover point
  (`hull back seat`, strength 3.75 on the retail back point) sat on Halo 1's
  seat node; eight times its strength changed nothing, and named to a
  node-0 marker it lifted. Moved live to node 0 at the same place, the ghost
  rode at -7.7, 0.33 above the ground, and drove. The hover points match the
  retail ghost's within 0.06 (Halo 1's back point (-0.25, 0, 0.08), retail
  (-0.27, 0, 0.10); the front pair (0.62, +-0.4, 0.06) and (0.68, +-0.4,
  0.10)); the skeletons are the same five nodes at the same rest pose.

## 2026-11-19: the scorpion's treads are solid, the boulders are not

Commit: `3368ffedb`. Converted Blood Gulch, the scorpion's `phmo` 18,786
bytes with its 42 pieces, as converter version 90 builds it; the scorpion at
[40.36, -64.01, 1.0], forward [-0.03, -0.99, -0.10]. Its pieces reach 1.18
to each side over the back half of the tread and 1.06 over the front half
(retail's scorpion: 1.13; its hull 0.53), from 0.10 to 0.53 above the origin.
Walking at x 41.45 or 41.59 toward -y, the player stopped at y -62.4, the
treads' back face, was pushed out to x 41.79 and slid along the side at 1.40
from the centre line (the tread's edge plus the capsule); from x 41.59
angled a little inward it slid inward along the back face and stopped at
[40.92, -62.37]. Put inside the tread (`sim.state.set`), it was pushed out
or stood on top (z 1.61). A path just outside the front half (narrower by
0.12) clears it. An earlier report of walking through the treads at 1.2 to
the side was not reproduced.

The boulders: the player put inside `boulder_doublewide` or `boulder_crouch`
drops to the ground through it.

## 2026-11-19: converted scenery has no Havok body

Commit: `3dfd6e41b` (converter version 83). The player walked straight
through `boulder` (placement at (37.93, -88.69, 0.09); the path passed 0.23
from its centre, its collision box half 0.67 by 0.60) with no Havok contact
but the level's. The boulder's `coll` has its BSP physics slot, and the
engine had written the slot's vtables at load (`0x00456194` at `+0`,
`0x00455540` at `+0x40`, `0x00455f58` at `+0x50`), yet `havok.bodies` lists
no body owned by any of Blood Gulch's 50 scenery placements (only the two
fixed bodies with no owner), and a10 holds none either. So the slots alone
do not put converted scenery in the Havok world. Retail Coagulation, Blood
Gulch's base, gives every scenery with collision a `phmo` (6 of 6, V:
`h2 object coagulation ...`; the trees' rigid bodies on node 0 or -1).
Giving converted scenery a built `phmo` (versions 85 to 91) did not help:
the engine never builds scenery a body from it. Moving each placement's
collision into the level's collision (version 96) is the design on the page.

## 2026-11-19: a converted weapon plays Halo 1's effects and sounds

Commits: `7074e4c63` (fire and reload), `690ab1fed` (ready, overheat,
pickup, zoom, chamber). Method: `weapon_rebuild_disc` against the discs.

Before converter version 97 the counterpart's were kept: drawing the
assault rifle played the needler's ready effect (the SMG's), picking it
up the SMG's ammunition sound.

## 2026-11-19: the assault rifle's reserve

Commit: `9dc0cb45b` (converter version 76; the live check ran on the
version 77 build). Method: game run on converted Blood Gulch, the HUD's
reserve count.

With the magazine's reserve cap (`+0xc`) written 0 there is no reserve
and the HUD count reads 0. Live: the assault rifle spawns with 180 in
reserve, and emptying the magazine and reloading leaves 120.

## 2026-11-17: b40's shared BSP blocks

Commits: `fefbf343f` (the BSP physics slots, converter version 18),
`108e8f932` (shared blocks stay shared). Method:
`level_objects_select_collision_with_bsp_physics` prints the bytes the
slots add; converting b40 at each lean step.

The slots alone add 37,280 bytes on b40 (38 slots; a10 13,152 for 15),
which put b40 0x8000 over the tag memory bound even at the last lean
step. With sharing b40 converts at lean 1 (with the empty squads
dropped) with tag memory `0xc22000` of `0xc24000`.

## 2026-11-17: lifepods and the pelican's engine loops

Commits: `0daf3e54a` (converter version 17: lifepods drop the loops),
`495e87ca8` (lifepods on the pelican template), `d4528bee4` (converter
version 49: every vehicle from the `vehi` definition, the quiet
stand-in gone). Method: the campaign disc test comparing converted
vehicles' ordered loop attachments with their source.

On the pelican template the pelican's attachments (`obje +0x94`, `0x18`
each) loop six sounds, engine, hover and banking on each side, scaled
by its `engine_audio`, `hover_audio` and `banking_audio` functions; on
a lifepod they hummed the pelican's engine at every placement, all
level (a10 on a converter version 14 build: "repeated wurrrring").
