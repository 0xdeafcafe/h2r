# Halo 1 sky conversion: measurements

Evidence for [../sky.md](../sky.md).

## 2026-11-19: a10's grey slab under `sky\space\space`

Commit: `90a5be1fd`. Method: a converted a10 with the cryo bay's 33
clusters forced to sky 0 (`+0x6e` = 0), `render.frame` on the running game
(per-draw depth state, vertex program and constants), and pass 11's
slab-grey pixel count with the vertex programs overridden. The slab is the
sky drawn over walls 5 to 10 units away. Reproduce with a build of that
commit and `render.frame` on the forced map.

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
- With `unclipped_depth` on clamped draws, the slab is unchanged (138,803
  pixels). With every program overridden to clip z = `k * w - e`, k = -1 or
  e = 1e30 changes nothing, so the depth really is unclipped. Yet z still
  changes coverage: e from 5,000 to 5,000,000 leaves about 88,400 slab
  pixels, and e of 5e7 or more the full 138,803, though no vertex has w
  between 1e4 and 1e8.

Open hypothesis: the host rasterizer mishandles the parts of these
triangles that lie far off screen near w = 0 (huge x / w), a guard-band
problem rather than a depth-range one. Guard-band clip planes for clamped
draws, clip distances `K w +- x` and `K w +- y` (wgpu `CLIP_DISTANCES`),
would bound x / w and y / w without culling anything on screen, the fan
included. Untested: it needs a build, not an override.
