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
