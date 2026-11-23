# Objects: measurements

Evidence for [../objects.md](../objects.md).

All numbers come from the ignored tests named in the entries and from
game runs on converted Blood Gulch driven through the agent API, against
the owner's discs.

## 2026-11-19: the scorpion's treads are solid, the boulders are not

Commit: `3368ffedb` (the record; the walk's build is converter version
90). Method: game run on converted Blood Gulch — walking the player,
placing it with `sim.state.set`, listing `havok.bodies` — and disc reads
of both games' tags.

The scorpion's `phmo` is 18,786 bytes with its 42 pieces; the tank at
[40.36, -64.01, 1.0], forward [-0.03, -0.99, -0.10]. Its pieces reach
1.18 to each side over the back half of the tread and 1.06 over the
front half (retail's scorpion: 1.13; its hull 0.53), from 0.10 to 0.53
above the origin. Walking at x 41.45 or 41.59 (1.1 and 1.2 to the
tank's left) toward -y, the player stopped at y -62.4, the treads' back
face, was pushed out to x 41.79 and slid along the side at 1.40 from
the centre line, the tread's edge plus the capsule; walking from x 41.59
angled a little inward, it slid inward along the back face instead and
stopped at [40.92, -62.37], where it stops on the centre line. Put
inside the tread, it was pushed out of it or stood on top (z 1.61). A
walking biped cannot enter the converted treads; an earlier report of
walking through them at 1.2 to the side was not reproduced.

The boulders: the player walked straight through `boulder` (placement
at (37.93, -88.69, 0.09); the path passed 0.23 from its centre, its
collision box half 0.67 by 0.60) with no Havok contact but the level's.
The boulder's `coll` had its BSP physics slot, and the engine had
written the slot's vtables at load (`0x00456194` at `+0`,
`0x00455540` at `+0x40`, `0x00455f58` at `+0x50`), yet `havok.bodies`
listed no body owned by any of Blood Gulch's 50 scenery placements
(only the two fixed bodies with no owner), and a10 held none either.
The player put inside
`boulder_doublewide` or `boulder_crouch` drops to the ground through
them. Retail Coagulation, Blood Gulch's base, gives every scenery with
collision a `phmo` (6 of 6; the trees' rigid bodies on node 0 or -1).
Scenery body-building was tried over converter versions 85 to 91
(`48b444773`) and replaced by structure collision in 96 (`27a2c8cd2`).

## 2026-11-19: the Blood Gulch ghost report

Commit: `2375dcb02` (converter version 94). Method: game run on
converted Blood Gulch (the report was against versions 90/91; `render`
frame draws, marker edits through the agent API) and disc reads of both
games' tags.

The report: "physics totally fucked and looks like using damage state
model"; one ghost lay nose down at pitch -10.8, partly in the ground.

- Model: in the game the ghost drew 1,082, 28 and 109 vertex parts
  (`render.frame` draws): geometry 0, `__base` at super high, with the
  `ghost hull metal`, `ghost self illuminated` and `ghost hull diffuse
  shine` shaders in its materials 0 to 2. Side by side with a spawned
  retail ghost on flat ground the two look alike. The "fragment" was the
  ghost lying on a slope falling 7 degrees forward, half hidden by it.
- At rest: spawned on flat ground the converted ghost's origin was 0.13
  above it at pitch -2.8, the retail one's 0.01 at -2.3 (its model
  dipping into the ground). The nose-down ghost lay on the slope with
  its hull on the ground, as Halo 1's hull would.
- Driven: the converted ghost rode at pitch +19.9, its tail on the
  ground; the retail ghost rides at -7.9, 0.39 above it. The back hover
  point (`hull back seat`, the retail back point's strength 3.75) sat
  on Halo 1's seat node; live, eight times its strength changed nothing,
  and named to a node-0 marker it lifted. Moved live to node 0 at the
  same place, the ghost rode at -7.7, 0.33 above the ground, and drove.

## 2026-11-19: a converted weapon plays Halo 1's effects and sounds

Commits: `7074e4c63` (fire and reload), `690ab1fed` (ready, overheat,
pickup, zoom, chamber). Method: `weapon_rebuild_disc` against the discs.

Before converter version 97 the counterpart's were kept: drawing the
assault rifle played the needler's ready effect (the SMG's), picking it
up the SMG's ammunition sound.

## 2026-11-19: the whitebark canopies' depth cut

Commits: `78d2887c5` (the cut-out bump map), `5a5b9de70` (merged as
converter version 92). Method: live converted Blood Gulch (version 91)
and `h2 agent edit set` on the leaf shader's slot 0.

With `default_vector` in slot 0 (opaque), each card's depth covered the
whole card: the whitebark's canopies drew as flat near-black diamonds,
the clear part showing the dark colour behind the depth, not the cliff.
Pointing slot 0 at the leaf bitmap itself cut the canopies into the
leaf's star shape at once.

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
