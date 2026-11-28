# Render loop (the retail front-end)

Status: Built. The read of the four orchestrators is reference; of the code, only the render-state list port (`engine::render_loop::states::RenderStates`) is placed, by the native world's BSP walk.
Code: `crates/rewrites/engine/src/render_loop.rs` and `render_loop/` (addresses, record layouts, `states`, `layer`); `crates/rewrites/render-bridge/src/render_loop.rs` (`world_states`). Tests: `crates/rewrites/engine/tests/states_differential.rs`.
Switch: none. The native world runs the state list under `impl.render.native` ([switches.md](../runtime/switches.md)).

This page holds the read of the four orchestrators in the 1.0 XBE, the
render-state list, and what `engine::render_loop` keeps. The native renderer
([native-renderer.md](native-renderer.md)) reads the addresses and record
layouts here. The Rust ports of `render_views`, `render_game_view`,
`render_scene_passes` and the layer draw are not in the tree (see
[notes/not-adopted.md](notes/not-adopted.md)). Evidence labels as in
[feature-engine.md](../engine/feature-engine.md): **Code** (read in the
disassembly, `h2 disasm`), **Test**, **S** (supposed).

No game data here: addresses and offsets are research points.

## The call tree (Code)

```text
render_frame_and_present 0x00223240
  render_game_views      0x002234e0   (game/BSP ready; else 0x002232a0)
    0x00223080                         per view: camera from the observer
    render_views         0x0002b5d0   once, after the view records are built
      per view record (0x118 bytes):
        [view] != 0  -> 0x0002c790(view)            alternate view
        else         -> 0x00025820(eax=view) test; skip when it passes and
                        byte 0x005093fc is set; else
                        0x00020720(eax=[view+4]);
                        render_game_view 0x0002b790(view, 0)
      0x00013b90, 0x00013cd0                         end of drawing
  present (0x00013f10 -> Swap)
render_game_view 0x0002b790 (stdcall, ret 8)
  0x0002b720, 0x0014b360, 0x0012e5e0, 0x0002f970, 0x00154310,
  render_view_scene 0x0002ba10
render_scene_passes 0x0002bcd0 (ret 0x18)
  a straight-line pass sequence, 98 call sites, no loop: the triple
  0x00044370 / 0x00044550 / 0x000445d0 begins, draws and ends a layer
  (EDI the layer; the begin's AL says whether to draw), 0x00014bc0 /
  0x00015680 / 0x00015370 set targets and state around the base layers,
  0x0002c370 is called with EAX 0 to 6, 0x00031c80 is the dynamic-light
  pass (which calls render_light_passes 0x00031170 per light),
  0x0001e370 the sorted transparents, 0x00013630 is called on every exit
```

### `render_views` `0x0002b5d0` (Code, complete)

- **Convention.** EAX the view count (signed: `<= 0` draws none), ECX a
  frame context (kept in ESI, stored to `0x004ba030` for the duration and
  cleared on exit), stack `(arg0, arg1, views)`, `ret 0xc`. Frame:
  `push ebp; mov ebp, esp; and esp, -8; sub esp, 0x1c; push ebx/esi/edi`.
- **Entry.** If `0x0047989c` is non-zero, calls `0x00146de0` (and
  `0x00146b80` on exit); the flag is kept at `[esp+0xf]`.
- **Frame begin.** Increments `0x004ba034`; stores `arg0` to `0x004ba04c`,
  `arg1` to `0x004ba048`; zeroes `0x00487b18`, `0x004aeb1c`, `0x004aeb28`
  and the 16 dwords at `0x004c56c0` (`rep stosd`); copies the float at
  `0x0045dd78` to `0x004aeb4c` through `xmm0`; builds a 0x18-byte record
  at `[esp+0x10]` = (context, 0, the double at `0x004ba040`, 0, 0) and
  calls `0x00012a50` with EAX pointing at it. The double is copied with
  `fld`/`fstp`, so a signalling NaN arrives quiet. Then `0x0054d598 = -1`.
- **Per view** as in the tree. After `render_game_view` it copies 12
  bytes `0x004b9dac..` to `0x00485b14..` and stores -1 to `0x005234c4`.
- Registers at each call are whatever the code left: callees are LTCG
  functions with private conventions (EAX, ESI, EDI and `xmm0` all carry
  values), so a port has to reproduce every register, not only the
  arguments it knows about.

### `render_game_views` `0x002234e0` (Code, partial)

Builds the per-player (or cinematic) view records and their cameras, one
`0x00223080` per view (the render camera from the observer camera;
[settings-menu.md](../game/settings-menu.md)), then calls `render_views` once at
`0x00223714` with `ecx = 3`, EAX from EDX (S: the view count), and the
stack arguments `(ebp, [esp+0x10], 0x0047f210)`: the view records are
the static array at `0x0047f210`. The record fill uses `rep movsd` of 0x1d dwords
and x87 (`fpatan`) for a field at `+0x28` (S: the horizontal FOV from the
vertical one). Rust builds the records and writes them once its byte arm is proven ([native-visibility.md](native-visibility.md#the-view-records)).

### `render_game_view` `0x0002b790` (Code, complete)

- **Convention.** stdcall `(view, unused)`, `ret 8`; frame `sub esp,
  0x16c` and EBX, EBP (the view), ESI, EDI pushed.
- **Entry.** Copies 16 bytes from `[0x004686d4]` into the frame, stores
  the view's `+4` (a view index) to `0x004ba050` and reads the pair at
  `0x004ba054 + 4·index` / `0x004ba064 + 4·index`, then calls
  `0x0002b720(&local)` (EDI another local). When it returns AL set, the
  pair is replaced from the locals.
- `0x0014b360` with ECX the pair's first word and the 12 bytes at
  `[0x004686cc] + 4` in the frame; its AL goes to a frame byte and EBX
  takes that dword. Then `0x0012e5e0` (stdcall, four arguments: the
  view's `+0xc`, `+0x18`, EBX, 1). If a frame flag is set, the view's
  `+0x50` is clamped down to a frame float when that float is above `+0x4c`
  (`comiss`, so NaN never clamps).
- `0x0002f970` (EAX = EDI = the view's `+0xc`). If the view's `+8` (an
  object index) is not -1, `0x00154310` with the object's cluster
  (`[0x004e8c24]+0x44` records of 0x21c bytes, `+0x24`).
- `render_view_scene` `0x0002ba10` (17 stdcall arguments) only when the
  view's `+0x34` is above the float at `0x0045dbdc`.

### `render_scene_passes` `0x0002bcd0` (Code, complete)

- **Convention.** Six stack arguments, `ret 0x18`: the view kind (kept in
  EBP; 1 and 2 draw the base layers with their targets, 5 draws nothing),
  a byte that gates the transparents, two flag bytes, the view (passed
  to the visibility and `0x0003fdf0` calls) and a mode (2 skips most of
  the late passes). The first argument's slot is reused as a byte local.
  `xmm0` carries a float that goes into a 12-byte record with the pass
  counter `0x004ba03c` (incremented) and the sixth argument; ECX points at
  it for `0x00013450`.
- **Early-outs.** `0x000289e0` and `0x00159610` (AL), then the visibility
  `0x00040e90` (EAX the fifth argument), then kind 5.
- **Base layers.** For kinds 1 and 2: layers 1, 3, 5, 8 and 20, each gated
  by its bit in `[0x004c1a50]` (8 also by the byte at `0x004ba022`): set
  the current layer, argument and list (`0x00467130..38`, the list is
  `0x004c1a48`), the target `0x00014bc0(target, arg)` (layer 1 also
  clears through `0x00014b60` unless a fog byte is set), the target mode
  `0x00015680` (EAX from `[0x00485898]`: kinds 4 to 7 take 0), the state
  `0x00015370` (EBX 1, then 0) and the layer draw. Other kinds draw layers
  1, 3, 5 and 2 with begin/draw/end and skip to the late passes.
- **Middle.** `0x0002c370(0)`, `0x000335e0(mode)`, layer 7, the dynamic
  lights `0x00031c80`, `0x0002c370(1)`, layer 2, `0x0002c370(2..6)`.
- **Late passes** (not for kind 2): `0x0003ede0` between two state calls
  when `0x0048575c` is set, `0x00025d50` and layer 16 (fog), `0x0003ba90`
  and layer 6 unless bit 4 of `0x004b9fe0` (then `0x0003c3c0`),
  `0x00025dc0`, `0x0002c340`; then for every kind `0x000c3e90`,
  `0x00036b50`, layer 15, `0x001595d0`, `0x00042b20(-13)` and the
  transparents `0x0001e370` when the second argument's byte is set.
- **After** (not kind 2): `0x00025dc0` again, and with the fog byte
  `0x00485a76` set around it, the player object chain (`0x004b9ed8`,
  `0x00155760`, `0x0014ddc0`, `0x0014de70`, `0x000cbd50`, the shader tag
  through `[0x004e3b44]`) that calls `0x0002afa0` when both floats at the
  shader's `+0x314` and `+0x318` are non-zero (`ucomiss` + `lahf`, so NaN
  counts as non-zero); `0x0003fdf0(0)`, `0x000404b0`, `0x0003fdf0(1)`,
  `0x00040870`, `0x0002dd30`, layer 17, `0x00020770`, `0x00013870`.
- `0x00013630` on every exit.
- **Registers.** `xmm1` is set before `0x0002afa0` (which passes it on
  to its own callee), so the port carries `xmm1` as well as `xmm0`.

## What `engine::render_loop` keeps

- The addresses and record layouts above, which the native renderer reads
  (`views`, `layer`, `occlusion`, the world walk and material modules).
- The render-state list (`states::RenderStates`), below. The native
  world's BSP walk runs it with redundant calls skipped
  (`feature::render_loop::world_states`).
- The layer entry loop (`layer::Entries`), unplaced: tests drive the loop
  through it ([layer-draw.md](layer-draw.md)).

### The render-state list (`0x0001a170`)

The shader draw `0x00016b90` (under layer 3) calls `0x0001a170` once per
draw to apply the shader's engine-driven render states. The time it takes
is in [notes/scene-pass-timing.md](notes/scene-pass-timing.md).

Read (Code): stdcall with one argument, the address of a list word
`count << 9 | first`. EBX is the draw context, and it returns with
`ret 4`. Each entry is 4 bytes at `[[ctx+0x20]+0x3c] + 4*(first+i)`:

- Byte 0 indexes a definition table at `0x00467020`, which holds the D3D
  state (a word at +0) and a type (at +3).
- Byte 1 is the source. Source 1 reads the float at `ctx+0x5c+4n`. Any
  other source calls the colour function `0x00131ed0` (cdecl, argument
  `ctx+12*(n+0x16)`), and the result replaces the low 24 bits of the
  state's shadow value.
- Byte 3 is the index `n`.

The type decides the conversion:

- Type 2 converts the float with `fistp`, rounding by the x87 control
  word.
- Type 1 passes the float's bits.
- Type 0 multiplies by the scale at `0x0045dc38`, at the control word's
  precision, and puts the integer in the top byte over the shadow's low
  24 bits.

The value is then applied by state:

| State | What the original does |
|---|---|
| below `0x5c` | `SetRenderState_Simple` `0x003f5cf0` (ECX the push header from `0x0043e6f0`, EDX the value), then the D3D shadow `0x00407498 + 4*state` |
| `0x5c..0x87` | memory only: the shadow, and the state's mask (`0x0043e4d0`) ORed into the dirty word `0x00407490`. D3D pushes it at the next draw |
| `0x88` | the device's `+0x790`, dirty bit `0x4000`, `0x004076b8`. The port leaves the device word out: it only fed D3D |
| `0x89..0xa5` | that state's setter (`0x003f5xxx`, stdcall, one argument) |

Nothing in it compares against the shadow, so every entry of every draw
makes its D3D call. The function has three copies of the apply code: one
for type 2, one for type 1, and one shared by the colour and type 0. Each
copy has its own call sites and keeps the value in different registers.
The port keeps all three, so every call sees the original's registers and
return address.

`engine::render_loop::states::RenderStates` ports the function at Port
grade (`Mode::Exact`). `Mode::SkipRedundant` adds one check before each
`SetRenderState_Simple`. If the shadow already holds the value, it skips
the call, and the shadow store too, since that store would write the same
value. Two things make this safe:

- **The shadow matches what was pushed.** Our D3D keeps the values it
  pushed separately from the guest shadow. For the immediate states, the
  only code that writes the shadow is the callers of
  `SetRenderState_Simple`, and they store right after the call. So a
  value in the shadow has already been pushed.
- **Shared push headers are excluded.** States `0x36` and `0x54..0x5b`
  share one push header. Pushing one of them changes what the GPU holds
  for the others, so the shadow of `0x55` can hold its value while the
  GPU register holds `0x54`'s. The port never skips these states
  (`SHARED_HEADER`). The differential asserts that this list equals the
  set of shared headers in the image's table. The skip-mode differential
  found this case: the GPU model diverged until these states were
  excluded.

The deferred states (`0x5c..0x88`) are already memory-only in the
original. The setters stay as calls, because they are recompiled guest
code that does more than update the shadow.

The native world's walk (`feature::render_loop::world_states`) runs the
list in skip mode inside its own hook, so it adds no exit of its own, and
counts the skipped calls in the native world's timeline metrics
(`StateCallsSkipped`).

## Evidence

The render-state list is checked against the lifted original by `crates/rewrites/engine/tests/states_differential.rs` (Test, needs the XBE). The measured skip rates are in [notes/render-state-skip.md](notes/render-state-skip.md).

| Part | What it covers |
|---|---|
| Inputs | 1,200 seeded states (three seeds of 400): lists of 0 to 24 entries over 64 random definitions, every type, both sources and every state range including states past `0xa5`; floats include NaN, infinities and halves; random x87 control word, shadow and dirty word |
| Stubs | `SetRenderState_Simple` is a stub on both sides that updates a model of the GPU (push header to value, seeded from the shadow). The setters and the colour function log their arguments and answer from the same generator |
| Exact mode | Equal call logs (target, eight registers, `xmm0`, stack argument), equal registers at return, all 64 MB equal |
| Skip mode | Same GPU model, same registers at return, same memory outside the dead stack below the frame; same setter and colour calls in the same order; its Simple calls are an ordered subsequence of the original's and the skip count equals the calls left out |
| Per-draw workload | `skipping_on_a_per_draw_workload`: 400 draws, a 12-entry list with eight Simple entries over 16 states. A state that appears twice in a list with different values is never redundant. The skip rate depends on draw order |

Not covered: faults inside the list. It reads and writes in a different order from the original, so a bad pointer faults at a possibly different address (the original would crash the game either way).

## Related

- [layer-draw.md](layer-draw.md): the layer draw and the shader draw begin.
- [native-renderer.md](native-renderer.md): the renderer that replaces the front-end.
- [notes/scene-pass-timing.md](notes/scene-pass-timing.md): where the scene passes' time goes.
- [notes/not-adopted.md](notes/not-adopted.md): the stage 7a ports.
