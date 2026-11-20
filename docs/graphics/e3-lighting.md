# E3 lighting: stencil shadows and dynamic lighting

Status: Partly built. The Lighting setting (harder, sharper retail shadows in
the view) is built. Observers, sun shadows from objects and per-pixel
dynamic lights are Design (not built).
Code: `crates/platform/d3d/src/scaled.rs` (`Lighting`, `follow_lighting`, `shadow_scale`),
`crates/formats/shaders/src/psh/shadow.rs` (hard shadows), `crates/rewrites/engine/src/render.rs`
(read-only views of the light record). Tests: `crates/platform/d3d/tests/render_scale.rs`,
`crates/formats/shaders/tests/shadow.rs`.
Switch: `H2_LIGHTING` (`retail`), listed in [switches.md](../runtime/switches.md).

An opt-in lighting mode in the spirit of the Halo 2 E3 2003 demo: hard-edged
shadows from every direct light, cast by and onto everything, and per-pixel
lighting with normal maps. [pstencil.md](pstencil.md) adds the Digsite E3 tag
set's lights. The history of the cut renderer, with sources, is in
[pstencil-history.md](pstencil-history.md).

The short version: **the evidence says there is nothing of the cut renderer to
switch back on in the retail game.** What retail kept is the vocabulary (a
light flag still called "stencil" in the official tools) and a shadow-buffer
system that took its place. So this is a new host-side renderer feature that
reads the retail scene, not a recovery. The README's word "recovering" should
be read that way.

It was written from the code and docs on master and from public sources. No
leaked build, game file or archive was downloaded or examined, and none may be
for this work.

The setting is `[graphics] lighting = "retail" | "e3"`, the Video screen's
**Lighting** row ([settings-menu.md](../game/settings-menu.md)), default
Retail and live. `H2_LIGHTING=e3` (or `retail`) locks it, for development
and for headless `h2 run`.

## Evidence labels

- **P**: a developer's own statement, with the source. **P\*** means we read
  it through a secondary quotation (a wiki, a mirror or a search excerpt)
  because the original was unreachable, so the wording is unverified.
- **C**: a community finding or a secondary characterisation.
- **R**: read from the retail image or disc by this project, in the doc
  named.
- **S**: our inference or speculation.


## Target look

What phases 4 and 5 should be judged against. Each item has its source;
the footage times are rows of the table in [pstencil-history.md](pstencil-history.md), section 12.

1. **Hard shadow edges, no visible filtering**, from the sun and from local
   lights, on large and small casters. Halopedia's "sharp rather than
   softened" (**C**); Steam commenter's "razor sharp" (**C**); footage 2:00,
   5:24, 7:12 (**S**).
2. **Characters and vehicles cast onto the world and onto themselves under
   the sun**, not only under dynamic lights. Staten (**P\***), Halopedia
   (**C**), Beyond3D (**C**).
3. **Large vehicles cast large shadows** (the Pelican). Jones (**P\***),
   footage 1:07 (**S**).
4. **Several shadows per object where several lights reach it.** Beyond3D on
   the Ghost (**C**).
5. **Lamps cast shadows and throw pools of light** that read as discrete
   shapes on the ground. Halopedia (**C**), footage 5:00, 7:12 (**S**).
6. **Vehicle headlights light the road in front** and move with the vehicle.
   Halopedia (**C**), footage 5:44-5:49 (**S**).
7. **Weapon fire and explosions light their surroundings**, including the
   firer. Perez via Halopedia (**P\***), Halopedia on the turret (**C**),
   footage 3:19 (**S**).
8. **Shadowed areas are very dark**: low ambient, high contrast, characters
   close to silhouettes in shade. Footage 2:00-2:09, 6:27 (**S**) only; no
   statement.
9. **Lightmaps give the ambient only;** direct light is dynamic. Perez
   (**P**). For us this stays the lightmap problem below; the item is met for
   objects and lights, not for the level's baked sun.
10. **Dynamic lights shade per pixel through normal maps**, so armour and
    walls show relief under a moving light. Freidin (**P**), normal maps
    everywhere (**C**), footage 6:33, 8:36 (**S**).
11. **Specular highlights from dynamic lights** on armour and visors. Staten
    on the visor (**P\***), footage 8:36-8:42 (**S**).
12. **Coloured, projected lights (gels)** where a light has one. Digsite
    (**C**).
13. **Out of scope but part of the look:** the demo's different bloom
    (Halopedia, **C**).

## What this means for the technique

Nothing public argues that the look needs stencil volumes. Every description
of the look is about properties a high-resolution, one-tap shadow map also
has: hard edges, everything casting, self-shadowing. The only public argument
for volumes is the Steam commenter's that 2003 hardware could not afford a
shadow map sharp enough, which is about the Xbox, not about us.

The public reasons Bungie left volumes are the ones the shadow prototype
measured ([notes/e3-lighting.md](notes/e3-lighting.md)).
Perez: "geometry/lighting fidelity compromises" (**P**), that is, meshes
simplified and closed so volumes work. Griesemer: the shadows did not work
at all in exteriors (**P\***, Halopedia's paraphrase of his X post). Jones: the Pelican's volumes filled
the screen (**P\***), which is fill cost. Butcher: the engine could not
support the environments a Halo game needs (**P\***). Retail and Halo 3 both
went to shadow maps. Shadow maps are the faithful choice for the look and the
unfaithful one only for the mechanism, which the non-goals already exclude.
What volumes would still buy, exact edges at grazing angles and no bias, is
what items 1 and 2 test, so the shadow-map path should be judged against them
at the resolutions we ship.

One community claim to discount: that retail still has true stencil lights
(Zanzibar's swinging lights, Foundation's fan room; ResetEra, 2018, **C**).
The retail reading ("Phase 0 findings") found no shadow-volume path (**R**); those are lights
without the "no stencil shadow" flag, drawn with shadow buffers.


## The Lighting setting

Retail shadows are depth textures: the shadow-generate layer (10) draws the
casters into a depth level, and the shadow-apply layer (11) samples it as
`LIN_D24S8` with the NV2A's hardware comparison and a 2x2 percentage-closer
filter ("Phase 0 findings"). E3 lighting makes that edge hard and sharp in
the view only. It runs on the scaled backend ([high-resolution.md](high-resolution.md)):
a 1x reference answers every read the guest makes and a scaled view draws what
is shown, and in-place depth sampling (`Backend::depth_level_texture`,
`gfx::depth_bits`; [d3d.md](d3d.md) "Lazy depth levels") means a depth level
is sampled with no read-back. The shadow comparison indexes texels by the
bound texture's real size (`textureDimensions`) and takes the coordinate in
the guest's size (`tex_scale`), so a larger map needs no change there.

- **Scaled shadow maps in the view** (`d3d::scaled`). Every `D24S8` depth
  surface not of the frame's size is a shadow level. With E3 lighting the
  view keeps a copy of it at `shadow_scale` times its size, made at first
  use, with a colour surface of that size to draw through. A draw that writes
  the level into a guest-size target (layer 10) is drawn again into the copy
  with colour writes off (`ScaleStats::shadow_draws`); a clear of it is
  repeated; a draw that samples it (layer 11) gets a texture of the copy
  (`shadow_textures`) instead of the reference's 1x one. What the view cannot
  repeat (guest bytes, a copy, a fill from the alias mirror, a draw it cannot
  make) marks the copy stale per aspect; a stale copy is filled from the
  reference (each texel repeated, `shadow_refills`) at its next draw, or
  skipped for sampling (the 1x texture stands in), unless a clear makes it
  fresh first. Halo's shadow pass clears the map before it draws, so the
  mirror refill at each binding costs nothing in the view.
- **The factor** is tied to the render scale, at least 2
  (`d3d::scaled::shadow_scale`: 2, 2, 3, 4, 5, 6 for scales 1 to 6; 1 with
  retail lighting). At the render scale a shadow texel covers no more host
  pixels than it covers guest pixels at 1x, so a one-tap edge steps no coarser
  on screen than retail's filtered one; the minimum of 2 makes E3 sharper than
  retail at 640x480 too. One setting and one rescale point.
- **One comparison tap** (`shaders::psh::PixelOptions::hard_shadow`, set by
  `Nv2aShaders` only when the view is told `set_hard_shadows`). A hard stage
  reads the nearest texel and compares once: no 2x2 filter. If a pass samples
  one map on several stages (taps the combiners average), each compares at
  the mean of their coordinates, so the average is one tap too. Whether
  retail's layer 11 does that is not known (S); the code covers both.
  Retail's translation is unchanged (`retail_shadow_lookups_are_unchanged`
  compares against a golden file).
- **At render scale 1** the view sleeps with retail lighting. With E3 it is
  awake: the frame at 1x, the shadow maps at 2x.
- **Live**: `ScaledBackend::follow_lighting` reads the row at every present
  and changes there, as the scale does. Copies at another factor are dropped
  at that boundary and made again at their next use (`lighting_changes_at_a_frame_boundary`).
- **The guest sees retail**: the reference keeps every level at 1x and answers
  every read (`read_depth`, `read_surface`, counts, the judge), so game
  memory, read-backs and the simulation are the 1x run's.

Tests (no game run): `crates/formats/shaders/tests/shadow.rs` (retail and E3 golden
WGSL; one tap on the interpreter and the GPU; the mean of two stages over one
map) and the E3 section of `crates/platform/d3d/tests/render_scale.rs`:
`e3_shadow_maps_scale_in_the_view_and_the_guest_reads_retail_bytes`,
`retail_lighting_leaves_shadow_maps_to_the_reference`,
`e3_shadows_are_one_tap_and_follow_the_caster_closer`,
`lighting_changes_at_a_frame_boundary`,
`e3_shadow_maps_survive_halos_render_target_pool` (the order Halo uses: shadow
pass, a colour level over the map's bytes, swap),
`the_lighting_ab_draws_retail_and_e3_from_one_run` and
`a_frame_covered_by_a_small_level_shows_no_lighting_difference`.

**Diagnostics.** The run report's `render scale` line ends with `shadow maps
at Kx: D draws, T textures, R refills`. In a level with shadows, `T` above 0
is the scaled map being sampled; `D` above 0 with `T` at 0 means the level went
through guest memory and the view sampled the 1x upload, still one tap. The
draw diagnostics list frames that drew with a depth texture bound
(`shadow-map frames K  a-b, c, ...`) and `H2_BOOT_GFX_SHADOWS=N` writes the
first N lit ones. Two headless runs are not the same run this deep (polled
visibility tests answer by host timing), so the comparison is made inside one
run: `H2_BOOT_GFX_LIGHTING_AB=1` (`d3d::LightingAb`) puts a second view over
the scaled backend; the inner view draws retail and the outer draws E3 from
the same calls, and each captured frame is written as `frame-N-retail.png`
and `frame-N-e3.png`. The report ends with `lighting A/B: K of M shadow-map
frames differ between the views (first: ...)`. Results of such runs are in
[notes/e3-lighting.md](notes/e3-lighting.md).

Not built: a separate shadow-map factor; float depth levels (`F24S8`, `F16`)
and `D16` maps stay 1x (their texture comes through guest memory); a per-axis
scale. The setting does not touch the lightmaps (layer 3), so the outdoor
shadows baked into a level are unchanged; sun shadows from objects are phase 4.

To compare by eye, take the same frame with `H2_LIGHTING=retail` and `=e3` at
`H2_RENDER_SCALE=2` (and once at 1 to see E3 at 640x480), in a spot where a
dynamic light shades a Marine or a crate, and step the Lighting row in
`h2 play`: the edge goes hard at the next frame.

## Phase 0 findings

Static reading of the retail XBE (`h2 disasm`, a linear disassembly of
`.text` searched for call sites, and the Ghidra function list) and of the
disc's tags (`h2 shader`, `h2 effect`), done without building or running the
game. Labels are this page's: **R** is read from engine code or
from the disc by this project, **S** is our inference. Names are ours unless
said otherwise. Read-only views of the structures below are in
`crates/rewrites/engine/src/render.rs`, tested on invented records.

The short version: **the light data is found, and it is a plain salted datum
array the render code reads at fixed points.** The guest's dynamic lights are
additive per-light passes over a shadow buffer, on numbered layers that the
shader templates and the engine share, so draws can be told apart by layer.
Retail stencil does masking only. Nothing in it is a shadow volume.

### Stencil: every use, named

| Site | Use | Label |
|---|---|---|
| `0x00015370` `stencil_mode_set`, mode in BX, stored at `0x00467010` | The only engine code that configures a stencil test. Seven modes (`stencil::for_mode`): 0 off; 1 and 2 write 0 or `0xff` wherever a pixel is drawn (ALWAYS, REPLACE on pass); 3, 4 and 6 draw only where bit 0 is clear, set, or bits 0 and 1 are both clear (EQUAL/NOTEQUAL, KEEP); 5 sets bit 1 where bit 0 is clear. Ops are only KEEP and REPLACE: no increment, decrement or z-fail counting, which shadow volumes need. | R |
| 42 direct calls of `0x00015370` | Every caller passes 0, 1 or 2. Mode 1 is set before the base-depth (1) and lightmap (3) layers in the scene loop and the structure draw `0x0004d720`; mode 2 marks the sky (`0x0002c0b2`, before `0x0003ede0`) and structure draws with a section flag (`0x0004d6d8`, flag `0x4000`). No direct caller passes 3 to 6. | R; what reads the marks is not traced (S: modes 3 to 6 are for that, or unused) |
| `0x0003bf20` | Colour and depth writes off, stencil ALWAYS/REPLACE with reference 0, then a four-vertex quad from world points at `0x004c1b08`: resets the stencil under a quad. Called through a pointer (no direct caller). | R; purpose S |
| 24 other `SetRenderState_StencilEnable` sites, one per function (`0x0001e9cd`, `0x0002524b`, ..., `0x000314e3`, ..., `0x00052e6d`) | `StencilEnable(FALSE)`, mostly just after a `CullMode` set: state resets around a draw family. 23 pass a literal 0 or a zeroed register; `0x00026998` was not resolved. | R |
| `0x00017b60`, `0x00018ee0`, `0x0001a170`, `0x000222d0` | Generic render-state-by-index setters: every state setter is called from each of them, stencil included. They apply a tag pass's render-state list and the engine's own lists. | R |
| Tag passes | No pass of any `shad` on the disc sets a stencil state: across all 30 maps the resolved passes set only `ZFUNC`, `COLORWRITEENABLE`, `ALPHABLENDENABLE`, `ZWRITEENABLE`, blend, `ZENABLE`, `BLENDCOLOR`, alpha test, `CULLMODE`, `SAMPLEALPHA` and `FRONTFACE`. The decoder names stencil states, so their absence is real. | R |
| `ShadowFunc` | Set to `GREATER` (`0x204`) by `0x000496f0` at the start of the shadow-apply layer (`0x00031328`), back to `NEVER` (`0x200`) by `0x00049740`, which the layer end's table reaches (`0x00044649`). | R; which layer reaches `0x00049740` S |

Engine-driven render states (table `0x00467020` via `0x0001a170`) were not
enumerated. If any of them sets stencil it would be per template; nothing
above suggests it.

### The light record

| Fact | Evidence | Label |
|---|---|---|
| Lights live in a datum array: 350 records of `0x110` bytes, allocated from game state by `0x000bffa0`, header pointer at `0x004e030c`. | `0x000bffac`..`0x000bfffb` (array name at `0x0045323c`) | R |
| Created by `0x000c0350` from the effect part switch (`0x0017ab2e` → `0x0017ac70`), either attached to an object and marker or free in the world; and by `0x000c0520` from a placement record. Both call the update `0x000c0c80` once. | call sites | R |
| `+0x02` u16 flags: 1 live; 4 placed; `0x10` timed (definition `+0xb0` above zero); `0x20` copied from a placement bit. | `0x000c03b8`..`0x000c03f4`, `0x000c05cd`..`0x000c05ff` | R |
| `+0x04` `ligh` datum; `+0x14` creation tick (from the game-time global at `0x00510c54`). | constructors; age in `0x000c0e5d` | R |
| `+0x18` f32[3] and `+0x24` f32: a bounding sphere. The visible-light pass weights each light by radius over distance to the view. | `0x000319f3`..`0x00031a86` | R (centre/radius S from the formula) |
| `+0x4c`/`+0x50`/`+0x54`/`+0x56`: an object whose functions drive the light's intensity (`0x000bab40` into `+0xd0`). `+0x58` parent object, `+0x5c` marker; `+0x60`/`+0x6c`/`+0x78` frame relative to the marker. | `0x000c0d15`..`0x000c0dc4`, `0x000c048c`..`0x000c04e7` | R |
| `+0x84` position, `+0xac` forward, `+0xb8` up, `+0x90` = position + forward. Attached lights are re-posed from the marker in the render pass itself (`0x00031d4b`..`0x00031dac`), not in the tick. | constructors, `0x00031d6f` | R |
| `+0xcc` base scale; `+0xd0` intensity (fades `1 - shape(age / duration)` with definition `+0xb0` duration and `+0xb6` shape for timed lights). | `0x000c0e53`..`0x000c0ea3` | R |
| `+0xd4` and `+0xe0`: two RGB colours, each interpolated by intensity between a pair in the definition (`+0x58`/`+0x64` and `+0x40`/`+0x4c`, mode at `+0x34`), then clamped to `[0, 1]`. `+0xec` a scalar from definition `+0x70`/`+0x74`. `+0xf0`.. computed by `0x000c0b00` from definition `+0xc0`. | `0x000c0dc9`..`0x000c11d7` | R; what each colour is for S |
| `+0xc8` the parent's fade: for a biped or vehicle parent, both colours are scaled by `1 - object +0x2b0`, unless definition flag bit 7. | `0x000c0f8c`..`0x000c108f` | R (S: active camouflage) |
| `+0xf8` f32: must be above zero for the light to cast a shadow. | `0x00031e14` | R |

**Stability.** Records are salted and fixed-size; a handle stays valid until
the light is deleted. The frame of an attached light is rewritten during the
render pass, so an observer must read it there (at the per-light pass
`0x00031170` or after `0x00031dac`), not at tick end. `light::LightRecord::check`
states what an observer can rely on (live, finite, colours in range, target
equal to position plus forward) and refuses anything else.

### The light render path

`0x00031c80` (`render_dynamic_lights`, called by the scene-pass function
`0x0002bcd0` at `0x0002c03e`) does, per frame:

1. `0x00031910` builds the visible-light list from the frame's candidate list
   at `[0x00547f98]` (at most 128): per light the datum, a weight from the
   bounding sphere, and four bytes, two of them definition flag bits 5 and 20.
   `0x0004a7c0`/`0x0004ac30` then sort and trim it (arguments 3 and 0.01; S:
   keep the strongest few).
2. For each light with weight above a threshold: re-pose attached lights,
   record up to eight per frame at `0x004c6b00` (stride `0x408`), decide
   **shadow casting** (`0x00031e02`): definition flag bit 3 clear, the global
   at `0x004ba020` set, record `+0xf8` above zero, and a word returned by
   `0x000c17f0` zero (S: a cluster or view test). Gather the objects it touches (`0x000c1d90`, `0x001320f0`,
   `0x00031660`), then draw it with `0x00031170`.
3. `0x00031170` (`render_light_passes`) computes per-light flags at
   `0x004ba014` (bit 0 from a word in the caller's context, bit 1 when the colour
   at `0x00485950` is above a threshold, bit 2 shadow, bit 3 and bit 5 from
   further tests; bit 4 is cleared), then draws layers
   on the light's list (`0x004c1a60`): shadow generate (10) when bit 2,
   shadow apply (11), layer 14, light diffuse (12), and light specular (13)
   when bits 0 and 1. It ends by turning stencil off and clearing the scissor
   (`0x000314e3`, `0x000314eb`).

**Shadow depth levels.** Layer 10's begin (`0x00044408`..`0x0004449f`) picks
render target 10, cleared to colour `0xffffff`, when per-light flag bit 4 is
set and bit 5 clear, and otherwise target 9, cleared to depth 1.0. Since
`0x00031170` clears bit 4, the per-light path always takes target 9. `0x00048ee0` builds the light's shadow
projection (stores `0x24` dwords at `0x004c9798`, uploaded to vertex constants `c[40]`..`c[48]`) before the layer draws.
Shadow apply (11) samples it with `ShadowFunc` `GREATER`. The render target
table is at `0x004b4b58`, stride `0x98`; target 9 is S as the 512x512 depth
level and the other as the 128x128 target d3d.md reports.

### Definition flags read off the code

`ligh +0` is a u32. The bits the render path tests (`light::definition_flags`),
with counts over the 1,596 `ligh` records on the disc (shared tags counted per
map):

| Bit | Engine use | Set on | Label |
|---|---|---|---|
| 3 | Set: no shadow (`0x00031e02`). This is the flag the MCC tools call "no stencil shadow". | 1,275 (321 can cast) | R; the tools name P (343), the match S |
| 5 | With bit 18 clear, the frame is refreshed from the marker each frame (`0x00031d3a`); forces the shadow-apply layer (`0x000312d4`); copied to the visible list. | 98 | R; meaning S |
| 7 | Parent fade ignored (`0x000c0fe7`). | 378 | R |
| 18 | Suppresses the marker refresh with bit 5. | 27 | R |
| 20 | Copied to the visible list (`0x00031ad5`); consumer not traced. | 1 | R |
| 22 | An extra per-light call (`0x00031c20`) and a wider object gather. | 217 | R |

Most lights have shadows switched off by bit 3, which agrees with the
community claim about the E3 level's light objects ([pstencil-history.md](pstencil-history.md), section 12), but the disc
cannot say which lights those are and we did not try. The same bit is the
no-shadow bit in the Digsite E3 tag set's paired light tags
([pstencil.md](pstencil.md)). `h2 lights <map>` prints each light's shape and
this bit, with its placements, attachments and effect parts.

### Pass taxonomy: the layers

The layer index a draw carries (`0x00016b90`'s `layer`) is one numbering for
both sides: shader templates list passes per layer, and the engine draws
layer by layer, holding the current one at `0x00467130` (`layer::CURRENT_LAYER`).
`0x00044370` begins a layer (descriptor table at `0x0043fa1c`, `0x24` bytes
per layer: target, clear, list, multisample; `layer::Descriptor`),
`0x00044550` draws the current list's entries for it through `0x0003bc30`,
`0x000445d0` ends it. Roles below come from the passes the disc's 30 maps put
on each layer (their blend and depth state, and what they bind) and from the
engine's order; the numbers of passes are resolved `(lod, layer)` lists.

| Layer | Role (`layer::Role`) | Passes | List | Notes |
|---:|---|---:|---|---|
| 1 | Base depth: opaque base colour, depth write | 4,244 | scene | stencil mode 1 |
| 2 | Base overlay: blended detail | 339 | scene | gated by `0x004ba023` |
| 3 | Lightmap and per-vertex static lighting | 16,622 | scene | stencil mode 1; palette reset at end |
| 5 | Transfer (precomputed) lighting | 126 | scene | |
| 6 | Water edge blend | 2 | scene | |
| 7 | Self-illumination, additive | 3,417 | scene | |
| 8 | Environment map, additive | 977 | scene | gated by `0x004ba022` |
| 10 | Shadow generate | 10,006 | shadow casters | per light |
| 11 | Shadow apply | 9,989 | light | per light |
| 12 | Dynamic light diffuse, additive, projected or spherical | 10,137 | light | per light |
| 13 | Dynamic light specular, additive | 10,137 | light | per light |
| 14 | Opaque, depth write, on the light's list | 538 | light | role unknown |
| 15 | Transparents | 2,898 | scene | |
| 16 | Planar fog | 16,668 | scene | special case in `0x00044550` |
| 17 | Bloom | 250 | scene | gated by `0x004ba024` |
| 18, 19 | Active camouflage, two layers | 111, 30 | scene | |
| 21 | Hologram | 6 | scene | |
| 0, 4, 9, 20, 22, 23 | no passes on the disc | 0 | | 9 is on the shadow list; 20 is drawn by the scene loop |

So the order in a view (`0x0002bcd0`) is: base layers 1, 3, 5, 8, 20 on the
scene list, then the dynamic lights (10 to 14 per light, `0x00031c80`), then
sky, transparents, fog and the rest. The view can tag every draw with
`[0x00467130]` at the shader draw and composite on that.

### What is still unknown

- What reads the stencil marks of modes 1 and 2, and whether modes 3 to 6 are
  reached at all (indirect calls were not searched).
- Layers 14 and 20, and the per-light flag bits 3 to 5 at `0x004ba014`.
- Which definition fields the light passes' vertex constants come from.
  The fields themselves are named since: kind at `+0x04` (0 sphere, 2
  frustum), sphere radius `+0x18`, cone width, stretch, angle, falloff and
  cutoff `+0x20` to `+0x30`, colours `+0x40` to `+0x6f`, brightness `+0x70`
  ([effects.md](../formats/effects.md) "ligh"). `+0x14` was not confirmed as
  anything; it is zero on the study tags. These are
  in `0x00031170`'s callees (`0x0004a1e0` takes 1024.0 and the light, S: the
  scissor) and the engine vector inputs ([shader-instances.md](../formats/shader-instances.md)).
- The candidate list at `0x00547f98` and the cluster test (`0x000c17f0`).
- Engine-driven render states (`0x00467020`).

### What is left of phase 0

The light data is located, its layout is fixed and salted, and the moment it
is settled is known. The layer number is the pass tag, and the dynamic-light
layers are 10 to 13 (plus 14). Still to trace before per-pixel lights
(phase 5): the light passes' vertex constants (cone, range, falloff, gel) and
layer 14. Phase 2 starts with two observers only: the current layer
(`0x00467130`) attached to each draw at `0x0001bbf0`, and the light records
read at `0x00031170`'s entry (handle in the caller's list), proved uncharged
and digest-neutral with the off/on test.

## Goals and non-goals

Goals:

- hard, per-pixel shadows from the sun and from dynamic lights, including
  self-shadowing on characters and vehicles;
- per-pixel diffuse and specular from dynamic lights using the materials'
  normal maps;
- off by default, and incapable of changing the simulation.

Non-goals:

- reproducing the E3 demo's level, scripts or assets. Only the retail disc's
  content is in scope;
- matching the original renderer's maths, which is not public;
- re-lighting the environment's baked lighting in early phases (see "The
  lightmap problem").

## Design

### Where it runs

The device (`d3d::Device`) and everything the guest can observe stay as they
are. [high-resolution.md](high-resolution.md) splits the backend in two: a
1x *reference* that answers every read the guest makes, and a *view* that
draws what is shown. E3 lighting lives only in the view. That one decision is
what keeps the simulation untouched: the guest's D3D calls, their results,
the write-backs and the pixel counts all come from a backend that does not
know this mode exists.

This design depends on that scaled backend.

### What the view needs to know

The view sees `ResolvedDraw`s: streams, state, textures and vertex-program
constants. A lighting pass needs more than that, and it comes from
**observers**: read-only hooks at guest addresses, run on the runner thread
at the moment the guest reaches them.

| Need | Source | State |
|---|---|---|
| Camera and projection for the view being drawn | view records built by `0x002234e0` and used by `0x0002b790` ([engine-map.md](../engine/engine-map.md)) | Functions located; record layout not decoded |
| Which material and pass each draw belongs to | `0x0001bbf0` begins a shader draw and passes (shader, LOD, layer, pass) to `0x00016b90` ([shader-instances.md](../formats/shader-instances.md)) | Decoded |
| Which geometry each draw belongs to | `0x00041040` branches between structure clusters and render models ([engine-map.md](../engine/engine-map.md)) | Located, marked "likely" |
| The sun's direction and colour | `sky` lights ([lighting.md](../formats/lighting.md)) | Angles decoded; colour and the rest not |
| Dynamic lights: position, direction, cone, colour, range | the light datum array at `[0x004e030c]`, read at the per-light pass `0x00031170` ("Phase 0 findings"); the record's `ligh` datum gives the definition | Located: frame, colours, intensity, bounds, shadow decision. The definition's shape, cone, range, falloff, colours and brightness are decoded (`tags::effects::light`, [effects.md](../formats/effects.md) "ligh"); which of them feed the light passes' constants is not traced |
| Object poses and skinning matrices for shadow casters | the guest's object and render-cache data ([objects.md](../formats/objects.md), `0x0003d270`) | Not decoded for this use |
| Level and model geometry, including what the camera cannot see | tags: `tags::structure`, `tags::geometry`, `tags::model`; collision in [collision.md](../formats/collision.md) | Decoders exist |
| Which bitmap parameter of a material is its normal map | template and pass bindings ([shader-instances.md](../formats/shader-instances.md)) | Bindings decoded; meanings not named |

Observer rules:

- An observer reads guest memory and writes nothing: no guest memory, no CPU
  state, no device state.
- Its reads must not be charged. The runtime counts guest accesses and turns
  them into logical time (`H2_ACCESS_COUNT`, [menu-budget.md](../perf/notes/menu-budget.md)),
  so a charged read would shift the schedule. Observers need a read path
  outside that accounting.
- It runs at a point where the data is settled: the guest's own call, not a
  host timer.
- What it produces is a host-side scene description attached to the draw
  stream as side data. The reference backend ignores it.
- An observer that cannot make sense of what it reads disables the mode for
  that frame and counts it. It never stops the guest.

### Shadows: the choice of technique

The view needs, for each direct light, a per-pixel answer to "is this point
lit?". Three ways to get it:

| Technique | For | Against |
|---|---|---|
| **Stencil shadow volumes** (depth-fail), as the original | The authentic look: exact, hard edges at any resolution. No bias tuning. | Needs closed, well-formed caster meshes, and render meshes are often not. Alpha-tested casters (grates, foliage) cast solid shadows. Fill cost grows with volume size and with resolution. Silhouette extraction per light per caster, on skinned meshes too. |
| **Host shadow maps** at high resolution with a one-tap comparison | Works with any mesh and with alpha test. Reuses the vertex programs. This is how the community restoration gets its "stencil-like" mode. | Bias and resolution artefacts. Not exact at grazing angles. Arguably not the feature the README names. |
| **Ray queries** against a host acceleration structure | Exact and simple once geometry is on the host. | wgpu's ray-query support is not something to depend on yet. Needs the same host geometry as volumes. |

The prototype `shadowlab` (`crates/tools/viewer/src/bin/shadowlab`) renders one
scene with volumes and with shadow maps from tag geometry, with no game run,
and times them. Its results are in [notes/e3-lighting.md](notes/e3-lighting.md).

**Decided: shadow maps, one tap.** Volumes cost as much as a one-tap map or
more at every scale measured, Halo 2's casters are mostly open meshes (37% of
caster triangles closed, the level's clusters none), and the two agree on all
but a fraction of a percent of lit pixels. A study note that recommended
depth-fail volumes, and why it is not followed, is in
[pstencil.md](pstencil.md) ("The technique: why shadow maps"). Volumes for
closed object casters stay the fallback if phase 4's pictures show a case a
map cannot meet. The volume notes below are the reasoning that fallback uses.

For volumes, the caster geometry is the open design point:

- **Level.** The collision BSP is closed geometry and suits depth-fail
  volumes, but it is coarser than what is drawn, so a shadow can part from
  the surface that casts it. The render geometry matches what is seen and is
  not guaranteed closed. A level also mostly shadows itself through its
  lightmap already (below). Start with the level as a receiver only.
- **Objects.** Render models, posed with the guest's matrices. This is where
  the look comes from: characters and vehicles casting onto the world and
  themselves. Open edges need the usual treatment (per-triangle volumes or
  capping), at more fill.
- **Extrusion** on the GPU from a prebuilt edge mesh, so the CPU does not
  touch vertices per frame.

Fill cost killed this on the NV2A. A current GPU has far more fill to spare,
and the volumes can be rasterised at guest resolution even when the view is
scaled, then used as a mask. That is an estimate to be tested in the viewer,
not a result.

### Lighting: per-pixel, from the lights

Hard shadows alone are a mask. The second half is replacing how dynamic
lights shade surfaces:

1. The view builds a host-only buffer of world position (from its depth
   buffer and the observed camera) and per-pixel normal. The normal needs each
   opaque draw's normal map and tangent frame, so it needs the material
   tagging above.
2. For each dynamic light, the view accumulates diffuse and specular under the
   shadow mask.
3. The guest still issues its own dynamic-light passes. The view skips those
   draws, identified by their pass tags, and substitutes its result at the
   same point in the frame. The reference draws them as always.

Step 3 needs the pass taxonomy. Phase 0 recovered most of it: the layer index
is shared by templates and the engine, and the dynamic-light layers are 10 to
13 ("Pass taxonomy: the layers"). Layer 14 and a few empty layers are
unnamed. Compositing at the wrong point lights fog, transparents and the HUD,
so this is still the part most likely to take longer than planned.

### The lightmap problem

The E3 renderer drew direct light dynamically and kept lightmaps for ambient.
Retail lightmaps hold direct sunlight and its shadows together with bounce.
Adding a dynamic sun on top double-lights sunlit areas, and adding its shadow
mask darkens areas the lightmap already darkened.

There is no clean fix inside the retail data. Options, from cheap to costly:

- **Objects only.** The sun's hard shadows are cast by objects onto the
  level, and onto objects by objects and the level. The level's own shadows
  stay baked. This gives the most recognisable part of the look and avoids
  the double count, apart from where an object's shadow falls on an area
  already in baked shadow (mask by the lightmap's value there).
- **Estimate the direct term** from the per-vertex lighting vector and the
  probes, and subtract it. Speculative: the vector's meaning is not fully
  recovered.
- **Re-bake** the ambient term on the host from level geometry. A lightmapper
  is a project of its own.

This design commits to the first, and treats the others as research.

### The "re-enable" option

Separate from all of the above, and smaller: make retail's own shadow buffers
look harder.

- **Host-side.** In the view only, scale the shadow buffers and force a
  one-tap comparison ("The Lighting setting"). No guest change.
- **Guest-side.** Clear a "no shadow" flag on lights that have it. This edits
  tag data the guest reads, so the guest draws more. It is a mod, not a
  renderer option. It is outside the guarantee below until someone shows
  that light flags are read only by rendering code, and it would need its
  own flag, off by default, excluded from deterministic replays.

## Opt-in, and the simulation untouched

- **Off by default.** With the mode off there is no observer, no side data
  and no view-side pass. The code path is retail's.
- **One-way data flow.** Guest to observers to view. Nothing returns.
- **The guest-visible values are the reference's.** With the mode on, every
  D3D result, write-back and pixel count is what it is with the mode off.
- **Failure is local.** A frame the view cannot light is shown with retail
  lighting.
- **No game data in the repo.** Observers and passes are tested on invented
  scenes. Viewer prototypes read the owner's disc at run time and write
  nothing into Git.

The acceptance test is the one in high-resolution.md ("Determinism"): the same input timeline
with the mode off and on gives the same guest memory digest and the same D3D
return trace. A second test proves the observers are uncharged: the access
count and logical time per frame are identical off and on.

## Phases, effort and risks

Effort figures are guesses for one person who knows the codebase. They are
here to show relative size.

| Phase | Work | Effort | Needs |
|---|---|---|---|
| 0 | **Done** ("Phase 0 findings"). Static research, no runtime change: trace the stencil call sites and name each use; find the light creation and render path from the `ligh` part case (`0x0017ab2e`); read the light flags off engine code; find who draws the shadow depth levels; name the layers in the scene-pass dispatch. Deliverables are docs, decoders and invented-data tests. | 2-4 weeks | Nothing |
| 1 | **Built** ("The Lighting setting"). Harder retail shadows in the view: scaled shadow buffers, one-tap comparison; the Lighting row. | 1-2 weeks | The scaled backend and in-place depth sampling |
| 2 | Observers and the scene description: camera, draw tagging, lights, object poses. Uncharged reads. Tests on invented guest memory. The light definitions are ready: `tags::effects::light` reads shape, cone, range, colour and the no-shadow flag, and `h2 lights <map>` lists a map's lights and where they come from. | 3-5 weeks | Phase 0 |
| 3 | **Done** (decided: shadow maps, one tap; [notes/e3-lighting.md](notes/e3-lighting.md)). Viewer prototype: volumes against shadow maps on tag geometry, with timings. | 2-3 weeks | Nothing; can run beside phase 2 |
| 4 | Sun shadows from objects in the view, composited as a mask. | 6-10 weeks | Phases 2, 3 |
| 5 | Per-pixel dynamic lights with normal maps and specular, replacing the guest's light passes in the view. | 8-12 weeks or more | Phase 4, the pass taxonomy |
| 6 | The environment's direct light. Research; may never ship. | Unknown | |

Risks, largest first:

1. **The light data is not found or not stable.** Retired by phase 0: a
   salted datum array, read at a known point in the render pass. What is
   left is the cone, range and falloff inputs to the light passes.
2. **Pass taxonomy.** Mostly retired by phase 0: the layer number is the
   pass tag. Layer 14 and the stencil marks' readers are open.
3. **Material coverage.** There are 165 templates and 147 passes in retail
   ([shader-instances.md](../formats/shader-instances.md)). Finding the normal map for
   each needs the engine texture sources and inputs named, which are open
   items there. Expect a long tail, with a fallback to the vertex normal.
4. **The lightmap double count** limits how far the look can go without
   phase 6.
5. **Off-screen casters.** The guest culls to the frustum, so the draw stream
   lacks casters behind the camera. Shadows need tag geometry plus guest
   poses, which is more observer surface than the draw stream alone.
6. **Geometry quality** for volumes: open meshes, alpha-tested casters,
   level-of-detail switches changing silhouettes.
7. **Cost.** The view already costs a second backend. Volumes add fill, and
   the buffers add memory at 4K. The runtime is CPU-bound, so
   the cost is measured against the frame budget first.
8. **Expectation.** The result will be a new renderer's idea of the E3 look
   on retail content. It should be described that way to users.

