# Level lighting and atmosphere

Status: Built. Code: `crates/formats/tags/src/lighting.rs`. Tests: `crates/formats/tags/src/lighting_tests.rs`. Switch: none.

`tags::lighting` extends the canonical `tags::structure` and shared
`tags::geometry` readers. It does not introduce a second BSP, palette, bucket,
or geometry decoder: every block it follows goes through the shared
`TagView`, so it inherits that view's alias budget, and bucket sections come
from the shared loader with its framing and resource-key checks.

No asset bytes, extracted files, names, dumps or game-derived fixtures belong
in Git. Tests invent their data. Validation read the owner's disc image
directly (`--disc <disc.iso>`), without copying or linking it. Analysis
scripts and build output stayed in scratch space. See [map-format.md](map-format.md),
[structure-bsp.md](structure-bsp.md) and [bitmaps.md](bitmaps.md).

## Evidence and scope

- **E**: retail engine instructions/decompilation, with virtual addresses below.
  The names are ours. Ghidra's inferred register/stack calling conventions are
  not treated as recovered ABI declarations.
- **V**: structural observation from the supplied disc. This establishes the
  stated property, not visual or acoustic equivalence.
- **I**: interpretation of an engine operation; explicitly narrower than a
  recovered field name or a complete shader/DSP contract.
- **Opaque**: preserved without interpreting internal pointers or parameters.

No community tag definitions, XML layouts or implementation bodies were
consulted for the new layouts. Existing bitmap and BSP dependencies retain
their own provenance; this does not relabel them as independently derived.

## Lightmap storage and rendering setup

The active BSP and lightmap pointers are `0x004e0348` and `0x004e0344`.
`0x000226d0` tests lightmap availability, nonempty groups, bitmap presence and
the paired BSP checksum. Consequently a nonempty tag alone is insufficient
for a surface to receive the current lightmap.

The existing [BSP layout](structure-bsp.md) owns the root group descriptor at
`ltmp+80`, bitmap reference at group `+18`, palettes at `+08`, render-info
indices and bucket references. Group stride is `0x68`: the palette
initialisation loop advances by `add esi, 68h` at `0x00015c6b`. An earlier
`0x98` reading could not be told apart by bounds, since every retail tag has
one group; the canonical decoder now uses `0x68`, and a synthetic two-group
test distinguishes the layouts.

| Field or operation | Evidence |
|---|---|
| Group `+08/+0c`: palette count/pointer, `0x400` bytes each | E `0x00015c0a`, `0x00015c32`, `0x00015c51`; V 2,042 palettes |
| Palette: 256 A8R8G8B8 words | E palette resource initialisation at `0x00015c37`–`0x00015c46`, `0x00015c90` → `0x003f84d0` (`D3DDevice_SetPalette_4`); channel convention also covered by existing gfx conversion tests |
| Render info `+0` i16 bitmap page; `+2` i8 palette, `-1` absent | E `0x0004d3d4`–`0x0004d40a` |
| Selected palette is bound at texture stage 3 | E `0x0004d40f`–`0x0004d416`, `edi=3`, `esi=palette index` |
| Lightmap bitmap and page are selected from the active group | E `0x00018bfe`–`0x00018d06`: bitmap datum at group `+1c`, image stride `0x74`, page global `0x00485a60` |
| CPU queries use the same bitmap/palette indices | E `0x000d16d0`, `0x000d2f90`, `0x001362b0`; palette entry lookup at `0x00015cd0` |
| Packed colour word to RGB | E `0x00131bb0`: R bits 16..23, G 8..15, B 0..7, each divided by 255 |

A render info entry with a page and palette `-1` names a **colour page**:
the converted Halo 1 pages, `R5G6B5` (or `A8R8G8B8`, `X8R8G8B8`), sampled
as stored. The native world keys it `LightmapKey { palette: None }` and
decodes it direct (`native_render::load::decode_lightmap`). A P8 page with
palette `-1`, or a colour page with a palette, panics by name.

A P8 lightmap is an **index texture**. For an integer texel index `i`, its
colour is `palette[i]`; a palette is not an extra image to multiply into the
sample. Filtering belongs after palette expansion. A conventional diffuse
material can multiply its base colour by that sampled irradiance, but the
complete retail material equation depends on its shader/template and passes.
This work establishes the engine's texture/palette setup and CPU query path;
it does **not** claim every material is one universal `texture * lightmap`
combiner, nor that a raw P8 index is intensity. Shader-specific combining,
exposure, gamma, specular and fog compositing remain open.

`palette_rgba` retains alpha. The preliminary scan found alpha 255 in every
entry of all 2,042 palettes. Export performs lookup only: no guessed exposure,
gamma correction or intensity multiplier. A PNG page is not a shaded surface.

## Spatial lighting probes and directional/ambient terms

| Field | Type / extent | Evidence |
|---|---|---|
| `ltmp+34` | f32 cycling period | E `0x00022850`: positive values divide elapsed time and enable coefficient cycling |
| Group `+38/+3c` | probe block, stride `dc` | E `0x00022850`, `0x00022cf0`: count/pointer, `index * 0xdc` |
| Probe `+00` | f32 xyz position | E `0x00022cf0`: squared distances to the query position |
| Probe `+0c`, `+30`, `+54` | f32[9] per RGB channel | E `0x00022850`, `0x00022cf0`: three nine-word copies/interpolations; RGB basis generation `0x00146020` |
| Probe `+a0` | f32 rotation | E `0x00022850`: sine/cosine and three calls to coefficient rotation `0x00143600` |
| Probe `+a4` | f32[3] interpolated direction | E `0x00022cf0`: copied/interpolated alongside coefficients; its downstream application is not reconstructed |
| Probe remaining bytes | retained `raw[dc]` | Opaque, including apparent vectors/colours at `+78/+84`; no meaning assigned solely from plausible numbers |

The arrays use the engine's real spherical-harmonic basis. At
`0x00143360`, for unit `(x,y,z)` and `c=1/sqrt(pi)`, the nine terms are:

```text
0: c/2                         constant ambient
1: -sqrt(3)*c*y/2              linear directional
2:  sqrt(3)*c*z/2
3: -sqrt(3)*c*x/2
4:  sqrt(15)*c*x*y/2           quadratic directional
5: -sqrt(15)*c*y*z/2
6:  sqrt(5)*c*(3*z*z-1)/4
7: -sqrt(15)*c*x*z/2
8:  sqrt(15)*c*(x*x-y*y)/4
```

The constants at `0x0045dee0/+dee8` establish 3 and pi; `0x0045dec0/+dec8`
establish 15 and 5; `0x0045dbbc/+dbc4` and `0x0045dc20` supply 1/2, -1/2 and 1/4.
`sh_basis`, `LightProbe::ambient_rgb` and `evaluate` expose that convention.
Evaluation is the coefficient/basis dot product for each channel, without
clamping or normalisation. `0x00023ca0` also contracts nine-term arrays to
per-vertex RGB. This does not recover all precomputed-transfer, visibility,
probe selection or material-shading behaviour. `0x00022cf0` selects nearby
probes and interpolates; reproducing its full selection algorithm is deferred.

The preliminary disc scan found 147 probes in 26 groups; all stored rotations
were zero. The final Rust finite-value and bounds checks pass for all 147 probes.

## Per-vertex lighting buckets

Bucket location, flags, resource fixup and section-offset tables remain in
`tags::structure`. `load_bucket` reuses the shared section loader.
`vertex_lighting` adds the following interpretations without redefining the
geometry stream enum:

| Stream | Stride | Interpretation and evidence |
|---|---:|---|
| `2e` | 4 | Packed ARGB to normalized RGB; E `0x000d35a0` selects the four-byte variant, `0x000d3630` calls `0x00131bb0` |
| `2f` | 3 | RGB bytes in memory order, each /255; E `0x000d33a0`, `0x000d3630`, normalisation constant `0x0045dbb4` |
| `30` | 4 | Signed 11:11:10 lighting vector; E `0x000d31c0`, constants `0x0045dd38/+dd3c/+dd48/+dd4c` |
| `1f` | 4 | Lightmap UVs, existing geometry interpretation; CPU consumer E `0x000d2bf0` |
| Others | preserved | Opaque; validator counts them explicitly |

For `30`, sign-extend each component to `s` and compute
`(2*s+1)/(2^bits-1)`. This is **not** the geometry normal decoder's
`s/(2^(bits-1)-1)`, and the result must not be normalized to unit length.
The engine interpolates these vectors for CPU surface queries. The meanings
of their individual channels beyond that use remain open.

The GPU reads the same words another way: the stream format table
(`0x0043f408`) declares type `30`'s one attribute (semantic 8) with format
code 16, `NORMPACKED3`, so a vertex program gets `s/1023`, `s/1023`,
`s/511`, clamped at -1 (the geometry normal decoder). A renderer standing
in for the game's draws takes that decode
(`native_render::load::program_lighting_vector`).

Bucket flag bit 0 gates a four-byte-per-vertex offset patch at
`0x0004d326` and `0x0004d396`; bit 1 gates the colour stream at
`0x0004dc95`/`0x000d35a0`. A second descriptor of type `2e` uses offset
`first_vertex*4`; the byte-colour variant uses `first_vertex*3`.

## Atmosphere groups

All seven groups have distinct typed variants. Only the following reviewed
fields are interpreted. `opaque`/`raw` retains the complete source payload or
record, including fields also represented by typed values; it is not an
editing API that automatically patches the retained bytes.

| Group / field | Interpretation | Provenance |
|---|---|---|
| `sky +00` | `mode` reference | E `0x0003ea60`/`0x0003ede0` resolve `+4`; V concrete class |
| `sky +14` | uniform model scale | E `0x0003ebd0` assigns transform scale |
| `sky +18` | camera translation scale | E `0x0003ebd0`: multiplies camera xyz |
| `sky +34` | fallback object-lighting RGB | E `0x000d1a30`: used without sky lights or after occlusion |
| `sky +48/+50/+58` | two fog blocks (stride `18`: RGB, density, start, opaque distance) and a third of stride `10` (RGB, scalar); element 0 of each, weighted by view coverage, blended into the view's atmosphere state | E `0x00131030` (from `0x0012f673`), `0x0012fb1b`; docs/halo1/sky.md |
| `sky +78/+7c` | lights block, stride `34` | E `0x0003eec0`, `0x000d1a30` |
| `sky +80` | rotation angle | E `0x0003ebd0`/`0x0003eec0`: sine/cosine |
| Sky light `+0c/+10` | yaw/pitch angles | E `0x0003eec0`, `0x000d1a30`: spherical direction construction |
| Sky light `+2c` | Parameter block, stride `28`. The census reports the first parameter’s raw flags at `+00`, or absence for an empty block. | V; the bit meanings are unknown |
| Sky light `+14` | lens-flare reference | E `0x0003eec0` tests datum `+18` before flare submission; V `lens` class |
| `fog +38/+40/+50/+58` | `lsnd`, `snde`, `snd!`, `snd!` references | V recognizable class words and datums; sound use not traced |
| `fpch +14` | `bitm` reference | V recognizable class word/datum; reference resolution checked across all maps; optical formula unreviewed |
| `MGS2 +00/+04` | camera-distance fade interval `[near, far]` (`distance_fade`) | E `0x0004f061`–`0x0004f098` form camera minus volume position; `0x0004f09e` calls `0x00030bf0`, which returns `sqrt(x*x+y*y+z*z)`; `0x0004f0a3`–`0x0004f115` compute `clamp((far-d)/(far-near), 0, 1)` when `far > near`, else keep 1.0 (`0x0045dbc0`) |
| `MGS2 +08/+0c` | layers block, stride `98` | E `0x0004f010` loop |
| Volume layer `+00` | raw flags | E `0x0004f010`, `0x00179880`: bit tests; full flag labels deferred |
| Volume layer `+0c` | sample count | E `0x0004f010`: loop count and reciprocal of count minus one |
| Volume layer `+04` | bitmap reference | V class/datum; rendering function blobs remain opaque |
| `snde +08..+34` | twelve environment floats | E `0x0021ece0`, detailed below |
| `gldf` | whole payload | Opaque; function tables and all fields unreviewed |
| `weat` | whole payload | Opaque; simulation/rendering blocks unreviewed |

For `snde`, `0x00220790` copies a `0x48`-byte record per environment;
`0x0021ee80` calls `0x0021ece0` to construct the effect parameters and send
them through `XAudioSetEffectData`. The twelve floats, in offset order, are:

| Offset | API field | Engine operation / confidence |
|---|---|---|
| `08` | `room_db` | level clamped to [-64,0], converted to DSP units; E, conventional name I |
| `0c` | `room_hf_db` | same clamp/conversion; E, conventional name I |
| `10` | `room_rolloff` | copied; E, conventional name I |
| `14` | `decay_seconds` | copied; E, conventional name/unit I |
| `18` | `decay_hf_ratio` | copied; E, conventional name I |
| `1c` | `reflections_db` | level clamped to [-64,10], converted; E, conventional name I |
| `20` | `reflections_delay_seconds` | copied; E, conventional name/unit I |
| `24` | `reverb_db` | level clamped to [-64,20], converted; E, conventional name I |
| `28` | `reverb_delay_seconds` | copied; E, conventional name/unit I |
| `2c` | `diffusion` | multiplied before effect output; E, conventional name I |
| `30` | `density` | same scaling; E, conventional name I |
| `34` | `hf_reference_hz` | copied; E, conventional name/unit I |

`LightVolume::distance_fade_factor` reproduces that fade for a given
displacement. Example: interval `[0, 10]` and displacement `(6, 0, 0)` give
`0.4`; only the length matters, so `(0, 3, 4)` gives `0.5`. The engine then
multiplies the factor by a per-instance value (`[ebx+30h]`) that is not part
of the tag.

These conventional effect names describe the parameter mapping, not a
recovered tag-editor schema. The first eight and last sixteen bytes remain
opaque. No acoustic validation or DSP/reverb implementation is supplied.

## CLI, validation and limits

```sh
cargo h2 lighting <map> --bsp 0 --disc <disc.iso>
cargo h2 lighting --all --disc <disc.iso>
cargo h2 lighting <map> --all --disc <disc.iso>
cargo h2 lighting <map> --bsp 0 --page 0 --palette 0 --out "$OUTPUT_DIR" --disc <disc.iso>
```

A BSP number uses scenario descriptor order, including descriptors with no
lightmap. It is not the order of `ltmp` tags. `--group N` selects a group for
export (default zero). If every render reference to a page selects one
palette, export can infer it; zero or multiple choices require `--palette N`.
Export requires a 2D P8 bitmap and full-resolution LOD 0. Names are generated
from numeric identifiers, not untrusted asset paths. Files go through
`safe_out::OutDir::new_strict`, which walks every component of the `--out`
root as well as below it. A symlink anywhere in the root path is refused, as
is `..`. The only exceptions are the macOS system aliases `/var`, `/tmp` and
`/etc`, each accepted only when root-owned with exactly its `private/...`
target; the walk then continues strictly on the resolved path. Existing files are refused unless `--force` replaces
a regular file; a symlink at the final name is never followed or replaced.
Export does not create anything without `--page` and `--out`.

`--all` without a map checks every disc map, including shared caches. With a
map it checks that map. It opens raw-resource siblings through the existing
bitmap router. It checks reviewed tag bounds/alignment, full salted references
and classes, finite reviewed floats, palette/page indices, bitmap layouts and
pixel decoding, bucket framing and stream conversions. It reports absent
local payloads separately and counts opaque blocks/streams. `gldf` and `weat`
are inventoried. `--all` exits nonzero if a map, tag or BSP census fails.

For each scenario BSP, `crates/apps/cli/src/lighting/census.rs` counts clusters with a resolved
sky and the lights from those distinct skies. A cluster's signed sky index
at `+6e` selects a scenario sky. The sentinel -1 selects the first sky only
when scenario byte `+12` bit 1 is set. Empty sky slots and out-of-range
indices select no sky. The report identifies BSPs without a local lightmap
and gives the page sizes of group zero even when its BSP checksum differs.
A mismatch leaves the geometry measurements unavailable. For a matching
group, including one with no page bitmap, the report gives:

- A histogram of full-resolution bitmap page sizes.
- Effective linear texels per world unit, the square root of summed texel
  area divided by summed world area. Both sums use the same opaque
  triangles with a page, UVs and nonzero area in both spaces. Each placed
  mesh contributes with its own lightmap UVs and the rigid transform used
  by the draw.
- The count and fraction of incident vectors within 15 degrees of any sky
  light selected in the BSP. Each vertex referenced by opaque triangles
  counts once per cluster or placement. Stream `30` is decoded as the GPU
  reads it. On a rigid placement the vertex program rotates its tangent
  frame and dots the world-space incident vector against it. Sky directions
  use the object-lighting yaw and pitch convention. A zero vector is an
  unmatched observation. A missing vector is counted separately. An empty
  denominator or an unmeasurable density prints `n/a`.

All block reads, in the shared linkage decoder and here (probes, sky lights,
volume layers, scenario BSP list), are charged to the one `TagView` budget,
equal to the payload length. Each logical visit is charged even when spans
alias, so the public `Lightmap::decode` and `LightmapLighting::decode` both
reject, for example, 128 groups naming one 256-palette table. Sky light
records read from another sky's payload are charged to the same budget.
Bounded aliases are permitted; block order and non-overlap are not assumed.
Raw geometry blocks are capped by the shared `MAX_BLOCK_SIZE`, and decoded
streams borrow that one block instead of copying. Duplicate block-resource
keys are rejected: the engine fixup (`0x00223c70`) applies resources in order,
so a repeated key would let the last one win, and the decoder does not pick
either. The bitmap reader supplies its own caps. Empty descriptors never
dereference stale pointers. Unreviewed descriptors are never followed. Typed
payload and alias reads also use the map reader's 64 MiB range cap. These are
design limits, not format facts.

## Validation results

The final `h2 lighting --all` run covers all 30 maps, including shared caches.
Counts below include repeated identities across maps; distinct paths are an
inventory measure, not a claim that payload contents are byte-identical.

| Group | Instances | Distinct paths |
|---|---:|---:|
| `ltmp` | 78 | 75 |
| `sky ` | 91 | 56 |
| `fog ` | 46 | 35 |
| `fpch` | 22 | 11 |
| `gldf` | 34 | 4 |
| `MGS2` | 379 | 41 |
| `weat` | 12 | 9 |
| `snde` | 124 | 21 |

The lightmaps contain 78 groups, 2,042 palettes, 147 spatial probes, 13,658
bitmap pages and 1,201 loaded buckets. The bucket conversions cover 1,748,301
colour vertices and 5,196,110 lighting-vector vertices. There are 63 populated
unreviewed group blocks (`+58`) and no unrecognized loaded bucket stream IDs.

Three sky light blocks alias records inside another sky tag's declared
payload. One fog identity has zero index size at another fog tag's exact
metadata address. These explain all four failures of an initial
own-payload-only run. The lighting reader resolves only an actual same-class
owning payload in the identical address space, charges the referenced span,
and reads only that child range. Zero-sized root aliases require an exact
same-class metadata address and a nonempty canonical payload. Unclaimed map
bytes, wrong classes, bad ranges, misalignment and excess alias work are
rejected. `Atmosphere::decode` with a standalone `TagView` continues to require
its children inside that view; cross-tag aliases require `Atmosphere::read`.

The earlier payload-only inventory counted 45 fogs because it excluded the
zero-sized identity. The final reader counts and validates all 46. A real P8
page was exported with both explicit and inferred palettes and inspected:
atlas boundaries and lighting gradients are coherent. This is a decoding
sanity check, not a comparison against a retail rendered frame. Images remain
outside the repository.

Synthetic tests cover multiple groups at the `0x68` stride, probe arrays,
every SH term against independent values at mixed-axis directions, probe
evaluation per term and channel, sky scalar offsets with neighbouring
sentinels, the MGS2 distance fade, RGB/ARGB and vector quantisation,
duplicate bucket resources, truncation, malformed pointers, non-finite
reviewed fields, opaque-field preservation, the shared alias budget, palette
ambiguity, PNG channel order, symlinked output roots, no-clobber output, and
the `--all` exit status on an invented malformed map. Remaining format work:
optical fog and fog-patch parameters, `gldf`, weather internals, probe tail
fields, group `+10/+58`, complete probe selection and shader-specific material
combining. These remain opaque or explicitly deferred, without invented
semantic field names.
