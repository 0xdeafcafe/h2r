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
| `4–6` | RGB, zero on some skies (the **second ambient colour**) |
| `7, 8` | zero on all 31 skies |
| `9` | fraction, 0–1 |
| `10` | small scalar, 0.0001–5 |
| `11` | distance, 8–500 |
| `12–14` | RGB (many maps share `(0.196, 0.173, 0.329)`) |
| `15, 16` | zero |
| `17` | fraction, 0.1–1 |
| `18` | fraction, 0–40 |
| `19` | distance, 18–120 (`120` on most skies) |

Which ambient lights what, and whether the distance pairs delimit fog, has
not been established against the engine; the names above are positional.

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
| `48` | Gradient block; elements `0x18`: RGB, fraction, two scalars (0, 200, 700 seen). Meaning not established | V: 68 of 91 have one |
| `50` | A second gradient block, same element shape | V: 13 of 91 have one |
| `58–77` | Zeros on retail skies | V |
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
second gradient.

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
| — | model scale `+14` | **Design:** `1.0`; Halo 1 sky models are authored full size |
| — | camera translation scale `+18` | **Design:** `1.0`; the Halo 1 sky box stays centred on the camera |
| — | `+10` word, bitmap layers, gradients, `+44`, rotation | **Design:** `0`, empty blocks, `0`, `0.0` — retail zeroes |

Fields with **no Halo 2 home** (kept readable on the Halo 1 side, listed
here for the next pass):

- the animation graph (`antr`): Halo 2's `+08` slot is null on every
  retail sky, so its group is unknown and nothing is written there;
- the fog reference (`fog `): null on every retail Halo 1 sky anyway;
- the shader-function elements;
- the second ambient colour and the fraction, and the rest of the 20
  lighting scalars — Halo 2's gradient blocks (`+48/+50`) hold an RGB, a
  fraction and two scalars and are the plausible home, but that is
  unproven, so nothing is written there yet;
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
