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
  (planar fog) draws only when `[0x0048574c] != 3` or byte `0x00485a75` or
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

**Waste to look at (S, from the shape only).** The loop re-derives the
handler record and calls the handler once per entry, and each handler then
begins its shader draw and submits geometry; nothing in this loop groups
entries by shader or skips a state set that equals the previous one. Whether
`0x0003bab0` already sorts entries by shader is the next thing to read: if
it does, consecutive entries share a shader and a Rust loop can skip the
repeated begin; if it does not, sorting there is the batch opportunity.

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
