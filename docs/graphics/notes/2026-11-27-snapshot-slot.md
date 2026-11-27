# Where the view snapshot and layers 18, 19 and 21 draw

Evidence for [framescene.md](../framescene.md) (4.2.1, the view's slots) and [retire-d3d-r5.md](../retire-d3d-r5.md).

## 2026-11-27: research item R-a

Read at `6107fe5fa` with `h2 disasm` on the owner's retail Xbox executable, and a scan of its executable sections for `call` rel32 targets. Reproduce with:

```sh
for addr in 0x1e370 0x41cc0 0x423c0 0x352e0 0x2c2b9; do
    h2 disasm disc/default.xbe "$addr" 80
done
```

| Fact | Where |
|---|---|
| The sorted list `0x0001e370` sorts its 0x70-byte entries (`0x0013df80`), then calls each entry's handler `[e+0x20]` with `e+0x24` | `0x0001e3b2`, `0x0001e3f1` |
| Its only callers: slot 9 (`0x0002c19a`) and the first person's sorted pass `0x00040870` (slot 10, called at `0x0002c2cb`) | call scan |
| The camouflage handler `0x00041cc0` and the hologram handler `0x000423c0` are entry handlers, stored by the record builders (`0x000420a0`, `0x00042760`) | [native-water-fog.md](../native-water-fog.md) |
| Each handler calls the colour copy `0x000352e0(-1, 1)` at its entry, before its own list and layer draws | `0x00041d31`, `0x000423f1` |
| `0x000352e0(target, four)`: target `-1` is 19 (`0x13`); one stage, or four when the flag byte is set | `0x00035331`, `0x000352f1` |
| Its other callers: the water's `0x0003bea0` (`0x0003bed4`), `0x00025820` (`0x00025923`), `0x0001c0d0` (`0x0001c116`) | call scan |

Decision: there is no snapshot slot. Layers 18, 19 and 21 draw at their sorted entry inside slot 9, or slot 10 for the first person (`ViewSlot::sorted_layer`). The snapshot is taken at that entry and holds the view's colour as drawn up to it, the entries sorted before it included. One snapshot type serves every caller of `0x000352e0` (`native_render::snapshot_gpu`).

Not read: which pass of `0x00025820` and `0x0001c0d0` the copy serves.
