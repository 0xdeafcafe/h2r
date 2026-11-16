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
