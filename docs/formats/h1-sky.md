# Halo 1 skies and their conversion to Halo 2

`crates/h1sky` reads Halo 1 `sky ` tags out of Xbox cache files
([h1-map-format.md](h1-map-format.md)), decodes Halo 2 `sky ` tags
retail-side, and converts one into the other as a `map::build` block tree
(the `Converted { group, root, raw }` pattern of [h1-models.md](h1-models.md),
with the render model supplied by `h1models::model_from_map`). This page
documents both layouts and the mapping. It is not an on-disk Rust struct
ABI.

Evidence labels:

- **V — verified on disc:** read off the owner's retail discs by this
  crate's ignored tests — Halo 1: all 31 skies on the 24 maps
  (`real_h1_disc_every_sky_reads`); Halo 2: all 91 identities across 28
  maps (`real_h2_disc_every_sky_decodes`). Structural facts: offsets,
  sizes, value ranges.
- **E — engine:** the field's meaning was reviewed against the retail Halo 2
  engine in [lighting.md](lighting.md) (the `sky ` rows of its atmosphere
  table); the engine addresses there are the evidence, not this page.
- **Design:** a converter choice.

Field **names** are ours. The disc fixes where things sit; only the E rows
carry engine-proven meanings, and no community tag definitions were
consulted.

No sky bytes, tag dumps or extracted assets are in the repository. The
disc tests read the ISOs at run time (`$H1_ISO`, `$H2_ISO`).

## The Halo 1 `sky ` tag

The root struct is `0xd0` bytes; its two blocks' elements follow in order.
Every tag on the disc adds up exactly:
`size = 0xd0 + functions × 0x24 + lights × 0x74`. **V**

| Offset | Field | Evidence |
|---|---|---|
| `000` | Render model reference (`mode`); null on 6 of 31 skies | V: every non-null reference resolves to a `mode` tag |
| `010` | Animation graph reference (`antr`); non-null only on the two `sky\sky_x10\space_x10` skies | V |
| `020`–`047` | Zeros on all 31 skies | V |
| `048` | 20 lighting scalars (`f32`), mapped below | V: values |
| `098` | Fog reference (`fog `); **null on all 31 skies** | V |
| `0a8`–`0b7` | Zeros | V |
| `0b8` | Shader functions block (count, address, unused) | V: two skies carry one `0x24`-byte element, stored uninterpreted |
| `0c4` | Sky lights block; elements `0x74` bytes, 0–7 per sky | V |

### The lighting scalars (`+0x48`, `f32 × 20`)

Three colour-sized triples, fractions in 0..1 and distances, at fixed
positions in one run. The run is kept whole in `h1sky::h1::Sky`; the
positions (ours, from the values — **V** for the ranges):

| Index | Seen as |
|---|---|
| `0–2` | RGB, set on every sky (the **first ambient colour**) |
| `3` | fraction, 0–0.55 |
| `4–11` | **fog A** (outdoor): RGB `4–6`, two zero words `7, 8`, density `9` (0–1), start `10` (0–5), opaque distance `11` (8–10,000; 0 on five skies, below) |
| `12–19` | **fog B** (indoor): RGB `12–14`, two zero words `15, 16`, density `17` (0.1–1), start `18` (0–40), opaque distance `19` (18–120, `120` on most skies) |

The two fog runs have the shape of Halo 2's sky fog blocks (`+0x48` and
`+0x50`: colour, density, start and opaque distance; below), and on every
retail sky their padding is zero, the density a fraction and the start no
farther than the opaque distance (`real_h1_disc_every_sky_reads`). **V**
for the shape; the names are ours. Indices `4–6` are also what
`Sky::ambient_second_rgb` returns, from before the fog runs were read.

#### Fog A is the outdoor fog, fog B the indoor one

A Halo 1 structure cluster names a sky at `+0x00` (`i16`, `-1` for none;
clusters `0x68` bytes at `sbsp +0x134`). Which clusters name one, by map
(V, every BSP on the disc), against the skies' runs:

- **Fog A opaque at 0.** Five skies carry fog A as black, density 1,
  start and opaque distance 0: a10's `space` and `space_x10`, d40's
  `space_x10`, c20's `interior atmosphere` and the menu's `sky_ui`. Every
  cluster of `ui.map`'s one BSP names `sky_ui`, and the one cluster of
  a10's and d40's space BSPs names `space_x10`; the menu and the x10 and
  d40 space shots are not black. So fog A opaque at its start is no fog,
  and Halo 1 draws fog A where the cluster names the sky. Halo 2 would
  draw it as total black fog: `0x001301c0` raises the opaque distance to
  the start plus `0.0001` (`0x45DBDC`) and a density above `0.9999`
  (`0x45DC78`) to 1 (**E**), which is what a10 at version 60 showed.
- **Fog B is not drawn outdoors.** Most multiplayer skies carry the
  same untuned fog B (purple `0.196, 0.173, 0.329` or black, density 0.9 to 1,
  opaque at 120), and on those maps every cluster names the sky: Blood
  Gulch's 30 clusters all name sky 0, and the level is wider than 120
  units (`level_reach` 194). Halo 1 draws no purple wall there. c20's
  every cluster names no sky, and its only real fog is B (blue,
  density 0.1, 10 to 30 units): the library's. So B is the fog of
  skyless clusters.

The converter writes fog A as `fog_a` when its density is above zero and
its opaque distance past its start (`Sky::outdoor_fog`), and writes no
`fog_b`. Halo 2 blends `fog_b` from the same skies as `fog_a`
(`0x00131030`), so it would lay Halo 1's indoor fog over the outdoors,
as it did on Blood Gulch at version 60.

#### Indoor fog

**Halo 1 (E).** `0x000531b0` takes the camera's leaf (`sbsp +0xe4`,
cluster at `+0x08`) and its cluster's `+0x00` sky, and `0x000581a0`
picks the fog from it: a sky index takes that scenario sky's fog A (the
run at `sky +0x58`); `-1` takes the **first** scenario sky's fog B (`sky
+0x78`), whichever sky the level's other clusters name. So a level has
one indoor fog, sky 0's B. Every fog B on the disc with a density reaches
opacity past its start (V).

**Halo 2 (E).** `0x0012efb0` reads cluster `+0x6f`: not `0xff`, it takes
element `[+0x6f]` of the scenario's atmosphere block (`scnr +0x340`,
`0xf4` bytes each) in place of the cluster's sky fog. The element's
fields feed the accumulators the sky's fog elements do
(`h1sky::atmosphere` has the offsets and addresses):

| Element | Meaning | Sky counterpart |
|---|---|---|
| `+0x00` | name (string id) | |
| `+0x04` RGB | first fog colour | `fog_a` `+0x00` |
| `+0x10` | fade distance: the weight is 1 − distance / this, clamped to 10; 0 or less, the engine default | sky `+0x44` |
| `+0x18`, `+0x1c`, `+0x20` | density, start, opaque distance | `fog_a` `+0x0c`..`+0x14` |
| `+0x24` RGB, `+0x34`..`+0x3c` | second fog | `fog_b` |
| `+0x44`..`+0x58` | a third group, accumulated apart | |
| `+0x88`..`+0xd4` | patchy fog (`fpch` at `+0xd0`), when `+0xa4` > 0 | sky `+0x60`, `+0x64` |
| `+0xe0`..`+0xec` | when `+0xe0` > 0 | sky `+0x68`..`+0x74` |
| `+0xf0` (u16) | copied out for the camera's cluster | |

V (every Halo 2 map): 10 maps fill the block, 34 elements; on 9 of them
clusters index it, always inside it. 03a's first element is a brown fog,
density 0.6, 5 to 200 units (`real_h2_disc_atmosphere_fog_reads_where_the_converter_writes`).

**Conversion.** A campaign level whose first sky has a fog B gets one
atmosphere element, `h1_indoor`: fog B as its first fog, fade distance 0
(as 21 of the 34 retail elements), nothing else but the null `fpch`
reference. Every cluster naming no Halo 1 sky has `+0x6f` 0; the rest
keep `0xff` and their sky's fog. Multiplayer levels keep the base's
scenario, which has no atmosphere block, so their clusters name none
(the multiplayer skies' fog B is the untuned purple, and Blood Gulch has
no skyless cluster).

### Halo 2's sky fog (`+0x48`, `+0x50`, `+0x58`)

`0x00131030` (called from `0x0012f673`) takes a sky datum and three
accumulators. For each sky it is given, weighted by how much of the view
it covers, it adds element 0 of `+0x48` (`fog_a`), `+0x50` (`fog_b`) and
`+0x58` into them: colour times the weight, each of the three scalars
times the weight, the weight itself, and one minus the weight into a
product. `0x0012fb1b` divides by the summed weight and writes the
results to the view's atmosphere state: `fog_a` to `+0x1c..+0x30`,
`fog_b` to `+0x34..+0x48`, `+0x58` to `+0x4c..+0x58`. **E** for the
layout: `0x18`-byte elements, an RGB at `+0`, scalars at `+0xc`, `+0x10`
and `+0x14`; `+0x58`'s are `0x10` bytes, an RGB and one scalar. Then
`0x001301c0` checks each: when the blended `+0xc` is above a threshold it
clamps the colour to 0..1, raises `+0x14` to at least `+0x10` plus a
constant, and clamps `+0xc` to at most 1; otherwise it zeroes `+0xc`.
Hence the names: `+0xc` the density (zero: no fog), `+0x10` the start
and `+0x14` the opaque distance. **E** for those roles; the shader that
consumes the view state has not been traced.

### The sky light record (`0x74` bytes)

| Offset | Field | Evidence |
|---|---|---|
| `00` | Lens flare reference (`lens`), 16-byte Halo 1 reference; often null | V: resolves to `lens` tags |
| `10` | `u32`: `0x006e7500` on every light with a flare set, `0` otherwise; not interpreted | V: constant across all maps |
| `14–4b` | Zeros on every light | V |
| `4c` | `u32` flags: 1 or 3 | V |
| `50` | Colour (RGB) | V: values |
| `5c` | Power (0.3–1.8 seen) | V |
| `60` | Falloff distance (`50.0` on all 48 retail lights) | V |
| `64` | Zero on every light | V |
| `68` | Direction yaw, radians | V: values in ±π |
| `6c` | Direction pitch, radians | V: values in ±π |
| `70` | Cutoff angle, radians (0.0035–0.35 seen: 0.2°–20°) | V |

### Census (the whole disc)

31 skies on 22 of the 24 maps (the other two maps have none): 25 with a
model, 2 with an animation graph, 0 with fog, 2 shader-function elements,
48 sky lights, one unused duplicate (`c40` carries `b40`'s sky
unreferenced). Root sizes seen: `0xd0` (no lights), `0x144` (one), `0x1b8`
(two), `0x1dc` (a function + two), `0x3fc` (seven).

## The Halo 2 `sky ` tag

The root struct is `0xac` bytes; the declared payload additionally covers
the block elements, laid out after it. Root fields this crate reads
(`h1sky::h2`), with the E rows from [lighting.md](lighting.md):

| Offset | Field | Evidence |
|---|---|---|
| `00` | Render model reference (`mode`), 8 bytes: group word + datum | E `0x0003ea60`; V datum resolves |
| `08` | A reference, null on every retail sky; its group is unknown | V |
| `10` | `u32` varying with the sky (0, 2, 6, 13, 18, 34, 35, 38 seen); not interpreted | V |
| `14` | Uniform model scale | E `0x0003ebd0`; V: 400.0, 1000.0 |
| `18` | Camera translation scale | E `0x0003ebd0`; V: -1.0, 1.0 |
| `1c` | Bitmap layers block; elements `0xc`: a `bitm` reference plus a scalar (1.0, 1.5, 0.0 seen) | V: 70 of 91 have one |
| `24–33` | Zeros on retail skies | V |
| `34` | Fallback object-lighting colour (RGB) | E `0x000d1a30`; V values |
| `44` | Scalar, set by one retail sky (5.0) | V |
| `48` | Fog block A (`fog_a`); elements `0x18`: RGB, density, start and opaque distances (0, 200, 700 seen) | E `0x00131030` (above); V: 68 of 91 have one |
| `50` | Fog block B (`fog_b`), the same element | E `0x00131030`; V: 13 of 91 have one |
| `58` | Block of `0x10` elements: RGB and one scalar | E `0x00131030` |
| `60–77` | Zeros on retail skies | V |
| `78` | Sky lights block; elements `0x34` | E `0x0003eec0`, `0x000d1a30`; V |
| `80` | Rotation angle, radians | E `0x0003ebd0`/`0x0003eec0`; V: 0.0 on 90 of 91 |

Three retail skies point their lights block into another `sky ` tag's
payload; `h1sky::h2` reads elements by address through the map rather than
out of the payload, so those decode like any other (the same aliasing the
atmosphere reader resolves, and a zero-sized identity resolves to a
same-class payload at the same address).

### The sky light record (`0x34` bytes)

| Offset | Field | Evidence |
|---|---|---|
| `00–0b` | Three zero `u32`s on retail lights | V |
| `0c` | Direction yaw, radians | E `0x0003eec0` |
| `10` | Direction pitch, radians | E `0x0003eec0` |
| `14` | Lens flare reference (`lens`), 8 bytes | E `0x0003eec0`; V datum |
| `1c–2b` | Four zero `u32`s on retail lights | V |
| `2c` | Parameters block; one element of `0x28` bytes | V |

### The light parameters record (`0x28` bytes)

| Offset | Field | Evidence |
|---|---|---|
| `00` | `u32` flags: 15 common (49 of 71 lights), 12, 13 and 0 also seen | V |
| `04` | Zero `u32` on retail | V |
| `08` | Colour (RGB) | V |
| `14` | Power | V |
| `18` | Falloff distance (50.0, 80.0 seen) | V |
| `1c, 20` | Zero scalars on retail | V |
| `24` | Cutoff angle, radians (0.035–0.35 seen: 2°–20°) | V |

Census (the whole disc): 91 identities on 28 maps, 71 lights (67 skies
with one, two with two, 22 with none), 70 with a bitmap layer, 13 with the
second fog block.

## The conversion

`h1sky::convert` builds the Halo 2 root above as a block tree. What maps,
what is synthesised, and what has no Halo 2 home:

| Halo 1 | Halo 2 | Rule |
|---|---|---|
| model | `+00` reference | the output of `h1models::model_from_map`, installed in the same set; null stays null |
| first ambient colour (`+48`) | `+34` fallback colour | copied |
| light yaw/pitch (`+68/+6c`) | light `+0c/+10` | copied (which of the two angles is yaw follows Halo 2's engine order; the pairing itself is **Design**) |
| light colour/power/falloff/cutoff | params `+08/+14/+18/+24` | copied |
| — | params flags `+00` | **Design:** `15`, the retail-common value; Halo 1's flags (1/3) have a different, unknown meaning |
| — | model scale `+14` | **Design:** the least scale, at least 1, that puts every surface of the model as far from the camera as the level is wide (below) |
| — | camera translation scale `+18` | **Design:** `1.0`; the Halo 1 sky box stays centred on the camera |
| fog A (scalars `4–11`) | `+48` `fog_a` | colour, density, start and opaque distance copied, one element, when the density is above zero and the opaque distance past the start |
| fog B (scalars `12–19`) | scenario `+0x340` atmosphere element 0 | the first sky's only, when its density is above zero: colour, density, start and opaque distance as the element's first fog; every skyless cluster's `+0x6f` names it ("Indoor fog", above) |
| — | `+10` word, bitmap layers, `+58`, `+44`, rotation | **Design:** `0`, empty blocks, `0`, `0.0` — retail zeroes |

### The model scale

Halo 1 sky models are authored full size (5,600 to 98,000 units out on
most skies), but some carry surfaces near the camera: a30's distant ground
lies 68 units below it on `skydusk1`, d20's 100, d40's 138, Damnation's 54.
Halo 1 draws the sky first and the level over it, so those never show in
front of the level. Halo 2 draws the sky after the opaque layers (the
scene loop calls the sky draw `0x0003ede0` at `0x0002c0bc`, after layers 1
to 6), depth-tested, centred on the camera when the camera translation
scale is 1 (E `0x0003ebd0`: the sky's translation is that scale times the
camera position `0x004b9da0`). At scale 1, a30's ground cut through the
level wherever the level was more than 68 units away.

Scaling a sky about the camera leaves its picture unchanged and only moves
it back in depth. `h1sky::convert::model_scale` takes the nearest surface
(`nearest_surface`: the least distance from the model's origin to any
triangle, except triangles whose plane passes within one unit of the
origin, which the camera sees edge-on as a line) and the level's reach
(`level_reach`: the diagonal of the box around every structure BSP's world
bounds) and returns `reach / nearest`, or 1 when the sky is already that
far. Then no level surface is ever behind the sky from a camera inside the
level. On the retail disc 10 of the 25 skies with a model are scaled:
a30's two (7.5 and 12.2), a50's `skynight1` (3.0), d20's two (3.0, 4.0),
d40's four (17.9) and Damnation's (1.5). The largest sky after scaling
reaches about a million units; retail Halo 2 skies reach 2.8 million
(Delta Approach's, 14,200 units at scale 200).
`real_h1_disc_every_sky_stays_behind_its_level` checks every sky.

### The model's shaders

The converted model's material `k` is Halo 1's model shader `k`.
`h1convert::sky_shaders` gives each a copy of one of the base's own sky
shaders, named for the Halo 1 shader, with Halo 1's bitmaps in its bitmap
parameters and Halo 1's map scale in its vectors. A material whose shader
doesn't convert names the base sky's first shader, never NONE.

Halo 1's sky shaders are `schi` (transparent chicago), `sotr` (transparent
generic) and `swat` (water). Their fields, read on every sky of the disc:

| Field | `schi` | `sotr` |
|---|---|---|
| framebuffer blend, u16 (0 alpha blend, 3 add) | `+0x2c` | `+0x2c` |
| maps block (count, address) | `+0x54` | `+0x54` |
| map element size | `0xdc` | `0x64` |
| map's bitmap reference | `+0x6c` | `+0x1c` |
| map's u and v scale, f32 | `+0x54`, `+0x58` | `+0x04`, `+0x08` |

A `swat` sky shader is its base map (`+0x4c`) at scale 1.

Which template each becomes, all three drawn by the base's sky
(Coagulation's sky shaders):

| Halo 1 shader | Halo 2 template | Bitmaps |
|---|---|---|
| additive (galaxy, x10's space, d20's canyons) | `one_add_illum_detail` | the least tiled map on stage 1, `color_white` on stage 2, tint white |
| `schi`, two maps, a 1×1 mask then a tiled one (the cloud layers) | `two_alpha_clouds` | the mask on stage 1, the tile on stage 2 |
| anything else | `sky_one_alpha_env` | the least tiled map on stage 3 |

The least tiled map is the sky's picture: every multi-map sky shader on
the disc has one map at 1×1 (the dusk, night and clear-day domes, the
horizon land) and the rest tile detail over it (stars at 8 to 18×,
detail ground at 12 to 30×), or a single map. Halo 1's `A8` cloud masks
sample white with their alpha, so `two_alpha_clouds`' `rgb = t1 × t2`
keeps the tile's colour.

The vectors come in pairs, `[su, 0, 0, 0]` then `[0, sv, 0, 0]`, and pair
`p` scales bitmap parameter `p`: Coagulation's cloud cover tiles
parameter 0 eight times with pair 0, and its sun volume parameter 1 three
times with pair 1. The copies set each swapped parameter's pair to Halo
1's scale; `sky_one_alpha_env`'s three pairs all take the picture's (its
other stages hold uniform default bitmaps). The masked star layers of the
night and dusk domes, Halo 1's per-map colour functions and `schi`
animation are not carried over.

Fields with **no Halo 2 home** (kept readable on the Halo 1 side, listed
here for the next pass):

- the animation graph (`antr`): Halo 2's `+08` slot is null on every
  retail sky, so its group is unknown and nothing is written there;
- the fog reference (`fog `): null on every retail Halo 1 sky anyway;
- fog B of every sky but the first (Halo 1 never draws it, "Indoor fog", above);
- the shader-function elements;
- the first ambient colour's fraction (scalar `3`);
- each light's `0x006e7500` constant and flags;
- lens flares: the Halo 1 `lens` tags have no converter yet, so converted
  lights reference none.

## Commands

There is no CLI here; the crate is a library step of the campaign import
([campaigns.md](../game/campaigns.md) §3 step 3). Checks:

```
cargo test -p h1sky                                   # invented bytes
H1_ISO=<iso> cargo test -p h1sky -- --ignored --nocapture real_h1
H2_ISO=<iso> cargo test -p h1sky -- --ignored --nocapture real_h2
```

`real_h1_disc_a30_bloodgulch_round_trip` converts both skies with their
models, builds each into a v8 map, and decodes it back through `h1sky::h2`:
every field Halo 2 has comes back equal to its source.
