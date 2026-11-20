# Native decals

Status: Built. All seven decal modes are admitted through `d3d::DrawAdmission`; pixel parity with D3D is not verified.
Code: `crates/hooking/feature/src/native_decals.rs`, `crates/apps/boot/src/native_decals_glue.rs`, `crates/rewrites/native-render/src/decal.rs`, `decal_gpu.rs`. Tests: `crates/hooking/feature/tests/native_defaults.rs` (switch), `crates/rewrites/native-render/tests/decals_gpu.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

The guest keeps the decal array, the per-record texture binds, the duplicate markers and the wrapper's state. The stage replaces only the resolved `D3DDevice_DrawVertices` packet of each record. Addresses and offsets below are research points from the guest code (**Code**, `h2 disasm`), not game data. Part of [native-renderer.md](native-renderer.md).

## How it works

### The sorted-list handlers are not decals

The sorted transparent list calls the handler in each entry at `+0x20` with
`entry+0x24`, in the order produced by its sort (`0x0001e370`). The two
handlers below replay object/model scene layers; neither is
a decal boundary, and neither may be suppressed by a decal implementation.

- `0x000420a0` adds class-3 entries with handler `0x00041cc0` for records
  whose `+0x1c` is 1. Its payload is a count followed by layer IDs. The
  handler calls `0x00044c90` for each of those layers, then replays layers
  **1, 3, 8, 20, 7, 2, 16, 18**, conditionally **19**, then **15**.
- `0x00042760` adds class-3 entries with handler `0x000423c0` for records
  whose `+0x1c` is 3. Its payload is `(layer index, caller argument)`. The
  handler calls `0x00044c90` for that layer, then replays **1, 3, 8, 7, 2,
  16, 15, 21**.

Their list builders are called by the normal visibility pass (`0x00040e90`,
at `0x00040f2f`/`0x00040f46` and `0x00040f35`/`0x00040f4c`). The records at
`0x004c152c` come from `0x0002cbf0`: the BSP and instanced-geometry producers
store `-1` in the field tested as `+0x1c`; the object/model producer at
`0x001341d0` stores `[part+0x1c4]`. `0x00044c90` then walks the object render
records in record and increasing part order through `0x00045ce0` and
`0x00041040`. Consequently these handlers have no single geometry format,
depth state, or blend state. The replayed layer setup uses `0x00015370(1)`
for layers 1 and 3 (stencil `ALWAYS`, reference and masks `0xff`, pass
`REPLACE`, fail/z-fail `KEEP`) and `0x00015370(0)` for the others (stencil
disabled); the earlier direct `0x00044c90` calls inherit their state.

### The dynamic decal path

The runtime decal array is initialized by `0x0017d220`, through `0x00043890`.
The construction at `0x00180f87` obtains a dynamic geometry address from
`0x000439d0`. The renderer is `0x00044040`, not either sorted-list handler.

`0x0002c370` is the only caller of the state wrapper `0x00043a40` and supplies
`0x00044040` as the callback to `0x00163cd0`. Its seven call sites in
`render_scene_passes`, all for view kinds 1 and 2 only, place the modes here:

```text
layer 20
mode 0
0x000335e0, layer 7, dynamic lights
mode 1
layer 2
modes 2, 3, 4, 5, 6
late passes / layer 15
sorted transparents
first-person/player chain
```

There is no additional late or first-person call site. `0x00163cd0` invokes
the callback once per entry of `[0x00547f94]`, in stored order, with the list
value as the callback's first stack argument and `0xffff` as the unused
second. `0x00044040` selects the head at
`[[0x004ea94c] + 4*(mode*512 + first_argument)]` and follows each record's
`+0x34` link without sorting. The native path must preserve both orders.

Each record is `0x40` bytes in the array at `[[0x004ea950]+0x44]`. The linked
record at `+0x38` has a frame marker at its `+8`; the handler skips duplicates
and writes `[0x004ba038]`'s low word after admitting the record. Native
replacement leaves the handler and these writes in place, replacing only the
resolved draw packets after their recording has succeeded.

### Geometry and programs

- `0x00044189` passes primitive type **8**, `D3DPT_QUADLIST` (not quad strip),
  to `D3DDevice_DrawVertices`. Record `+0x2a` is a signed quad count; the
  vertex count is `4 * count`, with no indices. The native expansion is the
  existing quad-list rule: `[0,1,2, 0,2,3]` per quad.
- Record `+0x4` selects the geometry allocation. The byte offset is the
  allocation's `+8` value shifted left by `[[0x509448]+0x34]`; the guest
  vertex address is `([[0x509444]+0x4] | 0x80000000) + offset`, and the start
  vertex is `offset >> 4`. The declaration at `0x0043f7b9` is 16 bytes per vertex:
  float3 position at `+0`, packed D3DCOLOR at `+12` (vertex registers `v0`
  and `v3`).
- Record `+0x24` supplies vertex default `v10`; `v11` is zero. Any other
  unsupplied vertex input is a refusal.
- The vertex program is engine slot 3 selected at `0x00043fe8`. The table at
  `[[0x00485a80]+0x5c]` gives its shader datum; the resolved shader record's
  `+0x10` is the microcode and the signed shift of `+0x0c` by 4 its start
  slot. Native admission consumes D3D's resolved copy of that program and its
  complete constants rather than assuming a fixed projection.
- Record `+0x2c` is a bitmap datum and signed byte `+0x29` its image selector.
  `0x000144f0` takes the selector modulo the bitmap's image count and accepts
  only a 2D image, otherwise it falls back to the engine's default bitmap.
  `0x00014390` resolves the live texture header through `0x00012310`. Native
  admission consumes the corresponding resolved stage view; an unresolved
  image, fallback or streaming case stays on D3D.

### Effective render state

`0x00043a40` sets RGB-only colour writes (`0x00010101`), disables alpha test,
sets `ZBIAS=0`, and then sets the following per mode. The polygon-offset
constant for modes 0 through 5 is loaded from `[0x0045e48c]`; it is not a
copied constant here. Mode 6 takes the no-offset branch. The wrapper disables
solid offset again at `0x0002c38d`.

| Mode | Depth and polygon offset | Blend |
|---|---|---|
| 0, 1 | `ZENABLE=D3DZB_USEW`, final `ZFUNC=ALWAYS`, `ZWRITEENABLE=1`; solid offset enabled, slope 0, constant from `[0x0045e48c]` | enabled, `ONE / INVSRCALPHA`, `ADD` |
| 2 | same | enabled, `DESTCOLOR / SRCCOLOR`, `ADD` |
| 3 | same | enabled, `DESTCOLOR / ZERO`, `ADD` |
| 4 | same | enabled, `MAX` (factors `ONE / ONE` do not affect `MAX`) |
| 5 | same | enabled, `ONE / INVSRCALPHA`, `ADD` |
| 6 | `ZENABLE=0` (the final `ZWRITEENABLE=1` has no effect); offset 0 | disabled explicitly |

The mode's combiner block at `0x00484f68` is part of the snapshot and is
translated with the vertex program; it is not replaced with a generic
"textured quad" approximation. Modes 0/1, 2/3, 4, 5 and 6 have their own
blocks, set at `0x00043d23`, `0x00043db9`, `0x00043e7d`, `0x00043f0f` and
`0x00043c88` respectively.

**Stencil is inherited, not established statically.** `0x00043a40` does not
set it. Its call to `0x00016b10` reaches `0x00018900`, which can apply the
previous shader implementation's arbitrary restore-state list. The effective
stencil function, reference, masks and operations therefore must be read from
the device's ordered state at the callback. The native path admits only a
resolved disabled stencil state; enabled or unreadable stencil stays
on D3D.

The observers scope the current mode and outer-list entry. With the walk
disabled, the replacement point is the ordered
`d3d::DrawAdmission` at each resolved `D3DDevice_DrawVertices`. That snapshot
contains the exact declaration, streams, shader inputs, state, textures,
viewport and scissors. The guest callback still binds textures, writes every
duplicate marker and runs the wrapper's final solid-offset reset.

### Admission

The implementation is split into `native-render/src/decal.rs` (shared mode, vertex
and quad rules), `native-render/src/decal_gpu.rs` (translation and encoding),
`feature/src/native_decals.rs` (callback scope and tallies), and
`boot/src/native_decals_glue.rs` (resolved-draw admission).

All seven mode positions and their depth, bias and blend configurations are
implemented. The actual vertex program, all 192 vertex constants and all 16
attribute defaults, and the live 60-word combiner (including `PSTEXTUREMODES`
and the final-combiner latch) come from the resolved draw and are translated
with the shared NV2A compiler. Records and quad lists retain their order.
Each target copy gets its own immutable uniforms.

Admission requires resolved disabled stencil, the mode's exact
depth/bias/blend/colour state, and an explicit final combiner that does not
consume fog, the eye or a bump environment. Border samplers, two-sided
lighting, polygon stipple, non-zero `ZBIAS`, additional supplied vertex inputs,
and non-2D or depth-format stage-0 textures are refused. Only unsupplied `v10`
and `v11` defaults are accepted beyond the position and diffuse streams.

The handoff supplies the resident stage views directly; there is no custom
bitmap decoder or map cache. Recording returns one range only after the draw is
encoded on the required copy. Two-copy recording is transactional: partial
recordings are discarded and D3D remains the owner until both recordings are
sealed, so a late refusal cannot leave the decal undrawn. The wgpu viewport is
target-wide, as in D3D; the resolved clip and disjoint scissor rectangles do
the clipping at each copy's scale.

### Per-record scene walk

After duplicate marking and bitmap and default-register setup, the hook
at `0x00044161` reads the record's live allocation and quad count. It
resolves the same array and native material and reserves that record on
all copies. Success resumes at `0x00044195`, immediately after the guest's
array-draw entry. Refusal runs the original submission for that record.
The callback owns the traversal and changes to the next texture in both
cases. It clears its admission scope at `0x000441aa`.

## Not verified

- The per-copy decal range and invariant counters in `H2_FRAME_STATS` show actual recording and suppression. A runtime A/B still has to show actual replacement, exact depth, bias and blend, live texture identity, mixed native and D3D order, and zero unexplained invariants or pixel differences.

## Related

[native-renderer.md](native-renderer.md), [native-particles.md](native-particles.md), [native-transparents.md](native-transparents.md).
