# Retail lens-flare draws

Evidence for the [native renderer](../native-renderer.md) and the [D3D retirement inventory](../retire-d3d.md).

## 2026-11-24 — visibility, reflections and the filtered fan sequence

Read at `ac1f1c67e` with `h2 disasm` on the owner's retail Xbox executable.
From a checkout containing the extracted executable, reproduce the reads with:

```sh
for addr in 0x2dd30 0x2df00 0x20bb0 0x20770 0x2e540 0x2e9e0 0x480a0 0x46ea0 0x46230 0x467c0; do
    h2 disasm disc/default.xbe "$addr" 900
done
```

### Visibility

`0x0002dd30` disables colour and depth writes. It hands the current view, a class mask and callback `0x0002e540` to the walker at `0x0002def4`. The walker (`0x00020bb0`) tests the active frame and view before walking the list at `0x0051f40c` in 0x20-byte steps. It filters each entry by flags, view and class. On the query arm it polls the slot before reuse at `0x00020cbb`, publishes the begin-count commands, calls the callback with the entry and its payload at `entry+8`, then closes the ticket at `0x00020d0f`.

The callback resolves the tag and projects the test rectangle through `0x00048b00`. Projection failure, a nonpositive area or an excessive area takes its zero-return exit. On success it emits four position-register writes as a triangle fan, closes it at `0x0002e9b6` and returns the integer area. The walker stores the bounded expected area at `entry+6`. The reader polls at `0x00020840` and divides the completed sample count by that area. The visibility history can therefore change while a flare is hidden. Replacing its test with a visible colour quad would lose that feedback.

### Visible reflections

`0x0002df00` enables colour blending, binds the view's flare resources, resets the late list and calls the same walker at `0x0002e1a3`. This arm invokes `0x0002e9e0` at `0x00020d5a` without starting a query. The callback resolves the lens datum, reads its reflection count and address at `+0x40/+0x44` and walks 0x30-byte records. Depending on flags it calls `0x00020d80`, which reads the visibility byte at `entry+3` and can combine it with a linked entry's visibility. The branch at `0x0002eaa3` culls zero visibility. The callback applies per-reflection brightness, radius, tint and position, sets up the bitmap and calls the billboard at `0x0002f247`. The saved return distinguishes these fans from light volumes in the same helper.

If bit zero at `lens+0x28` is set and the late list has room, `0x0002f260` appends the tag datum and three position components to a four-entry array at `0x004c19c4`. Back in the parent, `0x0002e1c5` calls `0x00046ea0` for each 0x10-byte entry. That helper projects the position and rejects a rectangle outside the viewport before issuing geometry.

### Late draws in submission order

Each row describes a four-vertex triangle fan. The register set describes writes between Begin and End. Other register values and shader constants come from draw-time state.

| Producer | End call | Registers | Ordering evidence |
|---|---|---|---|
| Reflection billboard, `0x000480a0` | `0x00048497` | 0, 3, 9 | Saved return `0x0002f24c`. The light-volume caller returns to `0x0004fddd`. |
| Mask rectangle, `0x00046ea0` | `0x00047306` | 0 | Emits a position-only rectangle, then selects the target named at `0x004858b8`. |
| Textured rectangle, same helper | `0x00047512` | 0, 3 | Immediately precedes the two filter-source calls. |
| Filter inputs, `0x00046230` | `0x0004679b` | 0, 3 | Called twice, returning to `0x00047523` and `0x0004752f`, with different destination targets. |
| Filter, `0x000467c0` | `0x00046e44` | 0, 3 | Returns to `0x0004753a`. The loop at `0x00046af1` alternates source and destination for four iterations. |
| Composite, `0x00046ea0` | `0x00047854` | 0, 3, 9 | Selects the view target again, binds the filter result and draws sixteen fans. The loop changes the corners and the weight in register 9. |

At the End call, the billboard's return address is at `esp+0x24`. For the three fans inside `0x00046ea0`, `esp+0x54` holds the return to `0x0002e1ca`. The filter-source helper has its immediate return at `esp+0x90` and the outer return at `esp+0xf0`. The filter helper has those returns at `esp+0x9c` and `esp+0x100`. Checking both returns keeps other users of the filters outside the flare stage.

Register 0 emits the position. The textured fans set register 3 before each corner. The billboard and composite set register 9 once per fan. The register-9 value persists across all four vertices. This trace establishes the order and the producers' inputs. Pixel parity and the frequency with which the optional sequence runs require a frame capture.
