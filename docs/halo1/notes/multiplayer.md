# Multiplayer maps: measurements and game runs

Evidence for [../multiplayer.md](../multiplayer.md).

The numbers come from the ignored tests named in the entries, run
against the owner's discs, and from game runs by the owner.

## 2026-11-19: the tag memory of a pinned Halo 1 sandbox

Commit: `968f2d157` (Halo 1 weapon effects and sounds merged, converter
version 97). Method: `sandbox_disc::pinned_bloodgulch_fits_its_tag_memory`,
which prints the pinned map's tag memory, the largest base tags kept in
the metadata with what names each, and checks no kept tag names a
dropped one. Reproduce by converting Blood Gulch with `--sandbox h1
--pinned`.

With the weapons taking Halo 1's effects and sounds, Blood Gulch's
Halo 1 sandbox was 0x634000 bytes of tags against the multiplayer
0x62c000 (`map::Error::TagMemory` names the groups); the pinned map
held all of Coagulation's retail tags (546 effects, 708 bitmaps)
beside the conversion's. On the fresh map Blood Gulch drops 510 of
Coagulation's tags (0x74698 bytes, most of it three `jmad`s and retail
shaders and bitmaps).

## 2026-11-19: scenery collision sizes

Commits: `31156269f` (the merge, which as first written copied the
piece's tree into every level leaf the box reached), `f1fb8b182` (one
copy of each piece's tree). Method: `h1objects`
`tests/scenery_collision_disc.rs`, which prints each map's node growth
and collision BSP bytes before and after; Boarding Action is merged
again piece by piece and its growth itemised. Reproduce by running the
test with `H1_MAP_DIR` and `disc/halo2.iso` present.

- As first written, Blood Gulch's 12,071 collision nodes became 51,007
  for 25 placements, past what the pinned build's BSP space holds.
  Merged once, the merge grows the level by one tree and six nodes per
  level leaf.
- Boarding Action's 21 placements with collision take its collision BSP
  from 311,492 bytes to 537,388, and its structure region past Beaver
  Creek's BSP space (its `sbsp` and `ltmp` are 655,344 bytes on the
  disc); a `--pinned` Boarding Action is refused with "BSP space
  outgrew the pinned metadata base".

## 2026-11-19: game runs behind the shader rebuilds

Commits: `c8248825d` (detail tiling, converter version 84),
`78d2887c5` (cut-out depth pass), `5d1fe496d` (chicago chains and
plasma, converter version 90), `6b33f0e49` and `2852e09a8` (the plasma
rebuild's brightness). Method: live frames and detail-vector edits
through the agent API, and the ignored tests
`bloodgulch_leaves_cut_out_and_details_tile_by_their_scale`,
`bloodgulch_sniper_metal_reflects_its_cube_map` and
`teleporter_receiver_brightness_against_halo1`.

- Detail tiling, proven live by setting the Ghost's hull detail vector
  to 8 and the Warthog's to 16: each turned the flat panels into Halo
  1's fine corroded metal. Tiled once over the base, the corroded metal
  lay over each hull as a few large dark smears.
- The broadleaf's leaves drew as dark green cards on the opaque
  `tex_bump` copy before the cut-out rebuild.
- The teleporter cone before the chain rebuild: a static grey speckled
  shell at full brightness (the lone grey dust peaks at 0.61 in every
  channel), back faces culled, its silhouette hard and none of Halo
  1's green. The shield before the plasma rebuild drew its least tiled
  map alone on `one_add_illum`: `energy plasma`, white, added over the
  base as a blown-out white square (capture `before-receiver-close.png`,
  peak 1.0 in every channel).
- The first plasma rebuild (unreleased) drew the picture's mean colour
  masked by its alpha (0.10 to 0.13, nearly flat): peak (0.0005,
  0.027, 0.004), 37 times too dim, shapeless and with no glow.

## 2026-11-19: a10's cryo bay under sky 0

Commit: `61f49ff15` (converter version 81). Method: the agent API on a
running game: the camera's cluster's `+0x6e` set to `0xff` while the
frame was read.

With every cluster no sky reaches naming sky 0 (the state before
converter version 81), a10's cryo bay (`a10a`, 33 clusters, none with a
sky or a portal to one) drew `sky\space\space` as a flat grey slab over
two thirds of the view; the camera's cluster's `+0x6e` set to `0xff`,
the bay drew whole and the sky's draw left the frame.

## 2026-11-18: the player's markers on Blood Gulch

Commit: `9bb0d14ed` (spawns name built collections, the player's camera
and hands name its model's markers; converter version 50). Method: the
owner's report on a Blood Gulch game (the first Halo 1 sandbox builds).

With the rebuilt biped's camera and weapon markers left empty, the
cyborg's camera sat at its origin — the feet in view, the view low —
and its weapons had no marker to attach to, so the player held nothing
(and those builds had no item to pick up either: the spawns named
their `weap` and `vehi` directly, so nothing spawned).

## 2026-11-18: cluster flood and culled surfaces

Commits: `65fe71050` (portals kept, converter version 40), `ab5d0d7ca`
(converter version 65). Method: `h1convert` `tests/portal_flood.rs`
(`bloodgulch_surfaces_in_two_clusters`) and the owner's Blood Gulch
runs.

- Without portals, every collision leaf in cluster 0 and only cluster 0
  drawn: Blood Gulch's capture had one structure draw, cluster 0's
  cliffs, and no ground, and sky showed where the level should be.
- With a surface that crosses a portal listed by its first cluster
  only (the state before converter version 65), looking from another of
  its clusters without seeing the first, it vanished — the culling the
  owner saw on Blood Gulch at converter version 60. The test proves it
  from every spawn turning through 360°: of the surfaces seen (centre
  in the frustum, the sight line clear of solid), drawn with the first
  cluster 189 went missing in 157 of 2556 views; drawn with each, none.

## 2026-11-17: the dark a10 lightmap

Commit: `b1c933d3e`. Method: a capture of a10 at converter version 14
(frame 2806) through the agent API, and the `check_images` disc tests.

a10 at version 14 drew without light: the structure's lighting pass
(pixel shader `r0 = t3 · (1 + v1.a)`, then `× t0 × 2`, `t0` the screen
copy of the albedo pass) had nothing bound at stage 3, and no R5G6B5
texture was created in the whole run. The binding itself was right
(group, checksum and page all pass `0x00018bfe`); the texture cache
returned null, because the converted images lacked the `0x200` cache
flag every retail image has (all 1,686 of Coagulation's and 2,231 of
Delta Approach's: `0x203` DXT, `0x1209` swizzled, `0x290` linear), so
every converted texture drew whatever cache slot 0 held: the level's
one flat colour.

## 2026-11-17: the converted maps in the game

Commit: `87d3e4b78`. Method: game runs by the owner.

The converted multiplayer maps load and play: the engine loads the
converted `sbsp`/`ltmp`, the player stands on the converted collision,
spawn points, flags and weapons land where the Halo 1 map had them,
and the maps are listed and start from the multiplayer lobby.
