# Layer draw (`0x00044550`, `0x0003bc30`)

The scene passes draw layer by layer ([e3-lighting.md](e3-lighting.md),
"Pass taxonomy: the layers"): `0x00044370` begins a layer, `0x00044550`
draws the current list's entries for it, `0x000445d0` ends it. On Old
Mombasa at 4x the single most expensive call in the scene passes is the
layer draw at `0x0002bed0` (6.25 ms a frame, 3.34 guest and 2.90 D3D).
This page is the read of the draw and how `render-timing` breaks it down.
Evidence labels as in feature-engine.md; no game data.

## The read (Code, `h2 disasm`)

`0x00044550` (no arguments, `ret`):

- `[0x004858b0] = [0x00467130]` (the current layer), byte
  `[0x004670bc] = 1`, then `0x00016b10` with ESI = `0x00485b48` (the
  layer setup).
- Draws only if bit `layer` of the mask `0x00bffdee` is set: layers 1, 2,
  3, 5, 6, 7, 8, 10, 11, 12, 13, 15, 16, 17, 18, 19, 20, 21, 23. Layer 16
  (planar fog) draws only when `[0x0048574c] == 3` or byte `0x00485a75` or
  `0x00485a76` is set.
- Then `0x0003bc30` with EDI = `[0x00467138]` (the list), EBX = the layer,
  and `[0x00467134]` pushed.
- `[0x004858b0] = -1` on the way out.

`0x0003bc30` (EDI list, EBX layer, one stack argument, `ret 4`):

- `_chkstk` (`0x00320560`) for a 0xf004-byte frame: a local array of up to
  2,560 entries of 0x18 bytes.
- `n = 0x0003bab0(list, arg, layer, &array)` (stdcall, 16 bytes): builds
  the layer's entries into the array and returns their count.
- For each entry `e` (0x18 bytes): the entry type is the word at `e+0`; its
  handler record is `[list+0x14] + type * 0x18`. Skips the entry when
  `[[e+0x10]+0x100] == -1`; otherwise calls the record's function
  `[rec+4]` (indirect) as stdcall with seven arguments: `[rec+8]`, the
  stack argument, byte `[rec+0x15]`, the layer, byte `[rec+0x14]`,
  `[rec+0xc]`, and `e`.

So the 3.3 ms of guest code in that draw is in three places: building the
entries (`0x0003bab0`), the handlers (one indirect call per entry, a
different function per entry type), and what they call: shader draw begin
`0x0001bbf0` and geometry submission `0x00041040` per draw.

`0x0003bab0` (stdcall `(list, arg, layer, buffer)`, `ret 0x10`; Code):

- The list holds items `[list+0xc] .. [list+4]` in the 0x18-byte records
  at `[list+0x14]`: the same records as the handler table above, so an
  entry's type is its item's index, and every item has two functions,
  `[item+0]` (build) and `[item+4]` (draw).
- For each item whose mask `[item+0x10]` has bit `layer`, it calls the
  item's build function (stdcall, seven arguments: `[item+8]`, the
  argument, byte `[item+0x15]`, the layer, byte `[item+0x14]`,
  `[item+0xc]`, the entry slot), with the slot's word 0 the item index and
  word 1 `[item+8]`. AL zero takes the entry back.
- Then it sorts the entries: `0x0013da70` (a qsort; ECX the count, EAX
  the buffer, 0x18-byte elements) with comparator `0x0003bb80`, or
  `0x0003bbd0` for layer 5.

**Waste to look at (S, from the read; not measured yet).**

- Every layer draw walks the whole item list again, calls every item's
  build function again, and sorts again. A view draws about 15 layers
  (`0x00044550` is called from 15 places in the scene passes), so the
  same items are filtered and sorted about 15 times a view. Building each
  item's entries once per view for all its layers (one pass, bucketed by
  layer) and sorting once is the first batch opportunity, if the build
  functions have no side effects that depend on the layer order (to
  read: the build functions of the common item types).
- The draw loop then makes one indirect call per entry and each handler
  begins its own shader draw and submits its own geometry; the loop has
  no notion of "same shader as the previous entry". The sort's
  comparator (`0x0003bb80`) says what order the entries come in; if it
  groups by shader, a Rust loop can skip repeated shader begins between
  neighbours. `layer_build`, `shader_begin` and `geometry` in
  `render-timing` size each part.

## The port (Code, Test)

`engine::render_loop::layer` ports both functions as state machines like
`render_views` (docs/render-loop.md): `Draw` for `0x00044550` and
`Entries` for `0x0003bc30`, which does `_chkstk`'s probes and stack
writes itself. The `render-loop` feature places them with
`H2_FEATURES=impl.render.layer_draw=on` and
`impl.render.layer_entries=on` (or `impl.render=on`); both are off by
default. The entry loop in Rust is where batching goes: it reads each
entry, its item record and the handler before calling it.

Evidence: `crates/engine/tests/layer_differential.rs` runs each against
the lifted original from seeded states (any layer 0 to 31, the fog
cases, 0 to 40 entries or a negative count, six item types with stub
handlers, skipped targets); call logs, registers and all 64 MB must be
equal. `crates/feature/tests/render_loop.rs` runs the entry loop on the
runtime with stub build and handlers and checks the calls equal the
port's.

## `render-timing` breakdown

With `H2_FRAME_STATS=1 H2_FEATURES=render-timing` the stage line gains six
stages, each exclusive like the others and each as guest + D3D:

| Stage | Function | Holds |
|---|---|---|
| `layer_draw` | `0x00044550` | its own few instructions |
| `layer_setup` | `0x00016b10` | the layer setup call |
| `layer_entries` | `0x0003bc30` | its loop and the entry handlers' own code |
| `layer_build` | `0x0003bab0` | building the entries |
| `shader_begin` | `0x0001bbf0` | shader draw begin, wherever called |
| `geometry` | `0x00041040` | geometry submission, wherever called |

`shader_begin` and `geometry` are counted wherever they run (they move out
of `light_passes` and the scene-pass sites too). Under the site lines, one
line per layer drawn, most expensive first:

```text
render-timing:   layer 1: 6.25 ms (guest 3.34 d3d 2.90) 1.0 draws 812.0 entries
```

(illustrative). A layer's time is inclusive: all of its `0x00044550`
calls, with everything they called. `entries` is the sum of
`0x0003bab0`'s counts for it. The layer draw is also a scene-pass call, so
its call site still appears in the site list, now holding only the few
instructions of the scene passes after it.

**Overhead.** `0x0001bbf0` and `0x00041040` run once per draw and now
take an observer pair with a shadow call each; that cost is charged to
`shader_begin` and `geometry` themselves (and their callers' exclusive
time is not inflated). Read their absolute numbers as an upper bound.

## Shader draw begin (`0x0001bbf0`, `0x00016b90`)

The first 4x numbers (Old Mombasa) put `shader_begin` at 7.36 ms over 481
calls, about 15 µs each (an upper bound), against 0.88 ms for
`layer_build`. So the shader draw is the target, not the build.

**`0x0001bbf0`** is small. On the first draw after a layer draw sets
byte `0x004670bc` it runs the layer setup `0x00016b10`, sets a fixed
handful of render states directly (LogicOp, ZEnable, Simple `0x40354`,
`0x40300`, `0x40340`, ZBias, SampleAlpha, CullMode, FrontFace, through the
D3D entries) and clears the byte. Every call then copies four words
from the shader tag (`[0x004e3b44]` tag table, group `shad`) to
`0x00485a64..0x00485a70` and tail-calls the shader draw `0x00016b90` with
the state cache `0x00485b48` and five of its own stack arguments.

**`0x00016b90`** already has a state cache. The context at `0x00485b48`
holds the last shader (`+0x00`), template and pass (`+0x20`, `+0x24`), and
dirty masks (`+0x30..+0x40`, `+0x58`). It compares the new pass and
template with the cached ones and the shader with the last. Only on a
change does it rebuild the parameter blocks (`0x00016f60`, `0x00017000`)
and switch passes (`0x00018900`, `0x000189a0`). The rest runs on every
call, changed or not:

| Callee | What it does (read so far) |
|---|---|
| `0x0001ae70` | per-draw parameter evaluation; no D3D entry |
| `0x0001ba00` or `rep movsd` | the pass's 0xf0-byte block into `+0x1424`, only on a change or dirty bit |
| `0x00017170` | only on a change or dirty bit |
| `0x000174d0`, `0x00018a90`, `0x00019be0`, `0x00019ca0` | not read yet; no D3D entry directly |
| `0x0001a170` | the pass's list of per-draw render states: one `SetRenderState_*` per list entry, every call, with no comparison against D3D's shadow (`0x00407498`) |
| `0x0001ab50` | the same for texture stage states (`SetTextureState_*`) |
| `0x00017420` | four more setters (`0x000176a0`, `0x00017960`, `0x00017b60`, `0x00018560`); `0x00017b60` has the same render-state switch |
| `0x00018d70` | pixel-shader constants: only the dirty ones (mask `+0x1730`), written straight into the push buffer |
| `0x00018e80` | vertex-shader constants: only the dirty ones (mask `+0x1734`), `SetVertexShaderConstant1Fast` |
| `0x0001b0a0` | not read yet |
| `0x0001cf50` | `SetTexture` for the stages whose texture changed (cache at `0x0051f3c8`, current at `0x0051f3d8`) |
| `0x0001b050` | `SetPixelShader`, only when `+0x1514` says the pass or a dirty bit changed it |
| `0x00018ee0` | otherwise, a render-state list like `0x0001a170` |

So the original already skips unchanged textures, constants and pixel
shaders. What runs on every call regardless is the parameter evaluation
and the render-state lists, whose D3D entries are unconditional. A
render state set to the value D3D's shadow already holds is the obvious
redundant call. Skipping a whole shader begin would also need the
parameter evaluation's inputs to be unchanged, and the read has not
bounded them yet.

### Breakdown by call (`render-timing`)

The 21 direct callees of `0x00016b90` are now `shader_begin`'s call
sites, split the same way as the scene passes' (a call's time runs to the
next call, so `0x00016b90`'s own code between calls goes to the call
before it). Every stage and site also counts the D3D entries it made
(`timeline::phase::scope_entries(Phase::D3d)`):

```text
render-timing: ... ; d3d calls/frame shader_begin 9000.0 geometry 400.0 ...; foreign 2000
render-timing:   shader site 0x00016ef9 -> 0x0001a170 2.10 ms (guest 1.20 d3d 0.90) 481.0 calls 3500.0 d3d calls
render-timing:   shader shader_begin own code 0.80 ms (guest 0.80 d3d 0.00) 40.0 d3d calls
```

(illustrative). `d3d calls / calls` per site is how many D3D entries one
shader draw makes there. The 21 extra observers run on every shader draw
and are charged to the sites; `foreign` now also counts their calls from
elsewhere (`0x00018900` is called by the layer setup too).

### What the four hot callees call (and why they are not leaves)

The second 4x run (shader sites, inflated by the observers but ranked)
put the time in pure guest code with no D3D calls: `0x000174d0` 2.17 ms,
`0x00018a90` 1.94, `0x0001b0a0` 1.36, `0x00019ca0` 0.57 per frame, each
about 482 calls. Read with `h2 disasm`, they are loops over the pass's
parameters and texture bindings that call shared helpers per element:

- `0x000174d0` checks bounds and calls `0x00017550`. That calls
  `SHADER_PARAMETER` (`0x0001b230`) once per animated parameter and
  stores the float result at `ctx+0x338+…`. `0x0001b230` reads up to two
  inputs through `[0x00485a58]`'s function table (two indirect calls
  returning in ST0). It divides with `fdivr qword`, takes the CRT `fmod`
  (`0x00373b6a`) when ESI is zero and the function's type byte is not
  3, and then calls
  `FUNCTION_EVALUATE` (`0x0013b390`).
- `0x00018a90` copies 64 bytes per texture binding (`rep movsd`) and, for
  binding source `0x0b` (the lightmap), recomputes
  `log2(max(width, height))` with `fyl2x` whenever the lightmap page
  differs from the cached one. Other sources go through `TEXTURE_SOURCE`
  (`0x00019530`, a jump table). It marks each binding dirty in
  `ctx+0x1420`.
- `0x0001b0a0` walks those dirty bits. Per binding it takes the CRT `fmod`
  of the animation frame by the bitmap's frame count, floors it with SSE,
  and calls `BITMAP_FRAME` (`0x000143c0`) when the frame changed.
- `0x00016b90`'s own code also runs one `fyl2x` per call (`ln` of its
  stack argument at `+0x30` into `ctx+0x2c`).

The CRT `fmod` is not `fprem` alone. It saves the control word, loads a
new one, classifies both operands with `fxam` and `fnstsw` and `xlat`,
dispatches through `0x004800ac`, runs the `fprem` loop (`0x00373b74`),
and on the way out stores a double, checks error state and restores the
control word. That is about thirty x87 and environment instructions on
the soft-float x87, with `fwait` checks in between.

Host cost of the soft-float helpers (`crates/x87`, release, one core):
`fyl2x` about 870 ns for a non-exact logarithm and 120 ns for a power of
two, `fprem` about 23 ns, `fmul` about 12 ns.

So the per-draw guest time is probably in these shared helpers rather
than in the four loops themselves, and a port of the loops alone would
keep calling them. `render-timing` now times the helpers as stages of
their own, but only when they run inside the render front-end (the
simulation calls `fmod` too): `parameter` (`0x0001b230`), `function`
(`0x0013b390`), `fmod` (`0x00373b6a`), `texture_source` (`0x00019530`)
and `bitmap_frame` (`0x000143c0`). The shader sites then hold only the
loops' own code.
