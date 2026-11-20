# Scene pass timing

Evidence for [render-loop.md](../render-loop.md) and [layer-draw.md](../layer-draw.md). Nothing here states what the code does today. Entries are newest first.

## 2026-11-17: where the scene passes' time goes

Commit: not recorded. Method: the `render-timing` feature (`crates/hooking/hooks/src/render_timing.rs`), Old Mombasa census run, headless. Not re-run.

`render-timing` site lines from the 2026-11-17 Old Mombasa census run,
weighted over the 100 one-second windows with `render` above 15 ms (2,607
frames, mean `render` 24.9 ms with the observers on). A site's time is
its callee's, inclusive, minus the stages carved out (the per-light pass
`0x00031170` is `light_passes`). ms per frame:

| Site | Callee | ms | guest | D3D | D3D calls | What |
|---|---|---|---|---|---|---|
| `0x0002bed0` | `0x00044550` | 8.26 | 5.51 | 2.75 | 1,882 | layer 3, the opaque scene ([layer-draw.md](../layer-draw.md)) |
| `0x0002c19a` | `0x0001e370` | 2.58 | 2.29 | 0.29 | 1,010 | sorted transparents |
| `0x0002c03e` | `0x00031c80` | 1.88 | 1.88 | 0.00 | 0 | dynamic lights, without the per-light passes |
| `0x0002c021` | `0x000335e0` | 1.24 | 1.05 | 0.19 | 182 | |
| `0x0002bd2d` | `0x00040e90` | 0.91 | 0.91 | 0.00 | 0 | visibility |
| `0x0002c2b3` | `0x0003fdf0` | 0.84 | 0.69 | 0.15 | 404 | |
| `0x0002c322` | `0x00013870` | 0.76 | 0.64 | 0.12 | 164 | |
| `0x0002c2b9` | `0x000404b0` | 0.39 | 0.32 | 0.08 | 173 | |
| `0x0002be45` | `0x00044550` | 0.32 | 0.24 | 0.08 | 56 | layer 1 |
| `0x0002c05b` | `0x00044550` | 0.29 | 0.22 | 0.07 | 120 | layer 2 |

Everything else is under 0.3 ms. The pass sequence's own code is nothing:
the cost is in the pass bodies, and two thirds of it in the layer draws.

## 2026-11-17: the layer draw and the shader draw begin

Commit: not recorded. Method: `H2_FRAME_STATS=1 H2_FEATURES=render-timing` (per-draw stages need `render-timing.deep`), Old Mombasa at 4x, headless. Not re-run. The per-draw rows rank the hooks as much as the work; see "Hooked calls leave the direct path" in [layer-draw.md](../layer-draw.md).

- The layer draw at `0x0002bed0` was 6.25 ms a frame (3.34 guest, 2.90 D3D).
- `shader_begin` was 7.36 ms over 481 calls, about 15 us each (an upper bound), against 0.88 ms for `layer_build`.
- The pure-guest callees of `0x00016b90`, per frame, each about 482 calls: `0x000174d0` 2.17 ms, `0x00018a90` 1.94, `0x0001b0a0` 1.36, `0x00019ca0` 0.57.
- Host cost of the soft-float helpers (`crates/recompiler/x87`, release, one core): `fyl2x` about 870 ns for a non-exact logarithm and 120 ns for a power of two, `fprem` about 23 ns, `fmul` about 12 ns.

### The `render-timing` stages

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
its call site still appears in the site list, holding only the few
instructions of the scene passes after it.

**Overhead.** `0x0001bbf0` and `0x00041040` run once per draw and now
take an observer pair with a shadow call each; that cost is charged to
`shader_begin` and `geometry` themselves (and their callers' exclusive
time is not inflated). Read their absolute numbers as an upper bound.


### Breakdown by call

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


