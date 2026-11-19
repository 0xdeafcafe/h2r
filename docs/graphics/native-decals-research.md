# Native decals

Addresses and offsets below are research points from the guest code (**Code**,
`h2 disasm`), not game data. No runtime draw comparison has run yet.

## The sorted-list handlers are not decals

The sorted transparent list calls the handler in each entry at `+0x20` with
`entry+0x24`, in the order produced by its sort (`0x0001e370`). The two
previously unclassified handlers replay object/model scene layers; neither is
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

## The dynamic decal path

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
and writes `[0x004ba038]`'s low word after admitting the record. A native
replacement must prepare the complete callback before making any of those
writes, then reproduce them only after native admission succeeds.

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
  slot. A native snapshot reads the program there, rather than assuming a
  fixed projection.
- Record `+0x2c` is a bitmap datum and signed byte `+0x29` its image selector.
  `0x000144f0` takes the selector modulo the bitmap's image count and accepts
  only a 2D image, otherwise it falls back to the engine's default bitmap.
  `0x00014390` resolves the live texture header through `0x00012310`. Native
  admission requires the selected image and its resident texture to resolve;
  fallback and streaming cases stay on D3D.

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
resolved disabled stencil state for now; enabled or unreadable stencil stays
on D3D.

The native hook is on `0x00044040`, after `0x00043a40` has established this
state and before any record is processed. It does not skip `0x0002c370`, so
the guest's target setup, shader restore, texture-cache conventions and final
solid-offset reset still run. One immutable snapshot covers the complete
callback: state, combiner, vertex program, ordered records, geometry and
textures. Any unresolved item refuses the whole callback and changes no frame
markers. The requested external pass is the small admission-seam assumption:
the coordinator's handoff can rebind it to synchronous encode confirmation;
until then the existing `external_passes_dropped` counter is also required in
an A/B.

## Comparison and remaining runtime evidence

`H2_NATIVE_DECALS_COMPARE` follows the other native stages' compare style:
leave the selected callback on D3D while recording its candidate native plan,
and log paired/missing/duplicate draw identities by view, mode, outer-list
entry and record link. A separate tally reports admissions and refusals by
reason, with `external_passes_dropped` checked for accepted callbacks. Runtime
A/B still needs to prove actual replacement, exact depth/bias and blend,
resident texture identity, and mixed native/D3D order. No runtime evidence is
claimed by this static classification.
