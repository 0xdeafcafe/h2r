# Skies: measurements

Evidence for [../sky.md](../sky.md).

All numbers come from the ignored tests named in the entries, run against
the owner's Halo 1 and Halo 2 discs.

## 2026-11-19: the a10 grey slab

Commit: `90a5be1fd`. Method: `render.frame` on a10 with the cryo bay's
clusters (no sky) forced to sky 0 (`+0x6e` = 0 on all 33), per-draw depth
state, vertex program and constants from the agent API, and a WGSL override
of the clip z for every program. Reproduce by forcing the clusters and
reading pass 11's draws.

- The sky's draws are W-buffered and depth-clamped, LessEqual with no depth
  write, after the level. Their constants match the walls'.
- At the slab's pixels the sky's fragments store w under 1,000 depth units
  (about 0.06 world units at the level's 16,384 per unit). They come from
  triangles with a vertex behind the eye (w < 0). Every vertex is over
  10,000 units out, so a correct clip would leave no on-screen fragment
  nearer than thousands of units.
- With the depth clipped (before `unclipped_depth`), a clamped draw's clip
  z is `w / 2` (`xvs_fixup`), so the host's near plane is w = 0. Moving it
  out with a WGSL override, to z = (w - n) / 2, thinned the slab at
  n = 256 and 10,000 and removed it at n = 1,000,000 (3,716 slab-grey
  pixels left in pass 11, from 138,803).
- A fixed n is no fix: the full-screen clamped fan drawn after the sky
  (`inline Begin/End`, a pass-through program) sits at w of about 16,000
  (1 unit) and would be culled.
- On a build with `unclipped_depth` for clamped draws the slab is unchanged
  (138,803 pixels). With every program overridden to clip z = `k * w - e`,
  k = -1 or e = 1e30 changes nothing, so the depth really is unclipped. Yet
  z still changes coverage: e from 5,000 to 5,000,000 leaves about 88,400
  slab pixels, and e of 5e7 or more the full 138,803, though no vertex has
  w between 1e4 and 1e8.

Open hypothesis (untested, needs a build): the host rasterizer mishandles
the parts of these triangles near w = 0 that lie far off screen (a
guard-band problem), which clip distances `K w +- x` and `K w +- y`
(wgpu `CLIP_DISTANCES`) would bound.

## 2026-11-19: sky pictures, scale and edge-on triangles

Commits: `15948e853` (strips), `90a5be1fd` (opaque pictures, edge-on
triangles), `29035bdfb` (model scale). Method: the ignored tests
`real_h1_disc_every_sky_stays_behind_its_level` and `sky_pictures_disc` on
the Halo 1 disc.

- Model scale: sky models are 5,600 to 98,000 units out on most skies.
  Nearest surfaces: a30's distant ground 68 units below the camera on
  `skydusk1`, d20's 100, d40's 138, Damnation's 54. 10 of the 25 skies with
  a model are scaled: a30's two (7.5 and 12.2), a50's `skynight1` (3.0),
  d20's two (3.0, 4.0), d40's four (17.9) and Damnation's (1.5). The
  largest sky after scaling reaches about a million units; retail Halo 2
  skies reach 2.8 million (Delta Approach's, 14,200 units at scale 200).
- Edge-on triangles dropped: a50's `skynight0` has 12, b30's clear
  afternoon 20 and Damnation's dusk 1. a10's two skies have none: every
  surface is at least 6,829 units out.
- Blood Gulch's and b30's `sky clear blue` picture is A8R8G8B8 with alpha 0
  over most of it (mean 53 of 255).

## 2026-11-18: the indoor fog block on the Halo 2 discs

Commit: `02debdc8b`. Method: `real_h2_disc_atmosphere_fog_reads_where_the_converter_writes`
on the Halo 2 disc.

10 maps fill the scenario's atmosphere block, 34 elements; clusters index
it on 9 of them, always inside it. 21 of the 34 elements have a fade
distance of 0. 03a's first element is a brown fog, density 0.6, 5 to 200
units.

## 2026-11-17: the sky census

Commit: `144cc4ca8`. Method: `real_h1_disc_every_sky_reads` and
`real_h2_disc_every_sky_decodes`.

Halo 1: 31 skies on 22 of the 24 maps (the other two have none): 25 with a
model (6 without), 2 with an animation graph, 0 with fog, 2
shader-function elements, 48 sky lights (every one falloff `50.0`), one
unused duplicate (`c40` carries `b40`'s sky unreferenced). Root sizes seen:
`0xd0` (no lights), `0x144` (one), `0x1b8` (two), `0x1dc` (a function and
two), `0x3fc` (seven).

Halo 2: 91 identities on 28 maps, 71 lights (67 skies with one, two with
two, 22 with none), 70 with a bitmap layer, 68 with fog A, 13 with fog B,
rotation 0.0 on 90 of 91. Light flags 15 on 49 of the 71 lights.
