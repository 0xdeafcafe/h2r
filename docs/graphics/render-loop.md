# Render loop in Rust (stage 7a)

Stage 7a of the rasterizer front-end ([feature-engine.md](../engine/feature-engine.md),
"7. Rasterizer front-end"): Rust owns the view loop and the pass loop, as
Port-grade code that calls the recompiled pass bodies. This page holds the
read of the four orchestrators in the 1.0 XBE, what is built, and its
evidence. Evidence labels as in feature-engine.md: **Code** (read in the
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
vertical one). Not ported: the records are camera state other code reads.

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

## What is built

- `engine::render_loop` (`crates/engine/src/render_loop.rs`): `render_views`
  as a state machine. `Machine::start` / `resume` run the port to its next
  guest call and return `Step::Call { target, site, cleanup }` with every
  general register and `xmm0` as the original has them at that `call`,
  and the stack arguments pushed; `Step::Return { to }` at the end. Pure:
  works on any `GuestMemory`.
- `feature::render_loop` (`crates/feature/src/render_loop.rs`): the
  `render-loop` feature places it as the implementation of
  `RENDER_VIEWS` when `H2_FEATURES=impl.render.views=on` (or
  `impl.render=on`). Off by default. The driver makes each call with
  `call_guest` and resumes the port in the continuation, so a callee that
  blocks unwinds to the scheduler as usual.
- The layer draw `0x00044550` and its entry loop `0x0003bc30`
  (`engine::render_loop::layer`, `impl.render.layer_draw` and
  `impl.render.layer_entries`, off by default): see
  [layer-draw.md](layer-draw.md).
- `render_game_view` and `render_scene_passes`
  (`engine::render_loop::scene`, `GameView` and `ScenePasses`;
  `impl.render.game_view` and `impl.render.scene_passes`, off by default).
  Both are block machines: the state is the original's address the port
  continues at (after a call, that call's return address), and each block
  is the original's instructions in order, with the partial-register
  writes (`mov al, ..`, `movsx cx, ..`), the frame locals and the argument
  slots the original reuses. So every register at every call, including
  what the callees leave in EBX and EDI, is the original's.
  `ScenePasses::at()` names the call in progress by its original return
  address: the pass identity a `PassTag` needs. `Regs` gained `xmm1` for
  `render_scene_passes`' `0x0002afa0` call.
- The layer sort (`engine::render_loop::sort`, `impl.render.layer_sort`,
  off by default): the bookkeeping port of this stage, below.
- The shader draw's render-state list (`engine::render_loop::states`;
  `impl.render.states` exact, `impl.render.states_skip` skipping the
  redundant D3D calls; off by default), below.
- The transparents' index sort (`engine::render_loop::sort_indexed`,
  `impl.render.transparent_sort`, off by default), below.
- The one guest-visible difference: callees see the continuation token
  as their return address, not `0x0002b6b9` etc., as with every chain that
  runs something after a body. That includes `render-timing`'s per-site
  breakdown inside the scene passes, which keys on those return addresses:
  with `impl.render.scene_passes` on, the site lines are empty and the time
  stays in the stages. Read the site lines with the switch off.

### The layer sort (`0x0013da70`, `impl.render.layer_sort`)

The bookkeeping port of 7a: work the original does in guest code that
Rust does with fewer guest transfers and the same result.

The layer build `0x0003bab0` sorts each layer's entries with the engine's
qsort (Code: ECX count, EAX base, stack `(size, compare, context)`,
`ret 0xc`). It is the old CRT shape: an explicit stack of 30 partitions
in a 0x10c-byte frame, middle pivot swapped to the front, ranges of 8 or
fewer to a selection sort (`0x0013dcd0`, stdcall, six arguments), dword
swaps when base and size are multiples of 4 and byte swaps otherwise.
It is not stable, so only this algorithm leaves ties in this order, and
the order of the entries is the order of the draws: a Rust `sort_by`
would change the draw stream. The comparator is called through a pointer
(`call [esp+0x130]`), and that indirect call leaves the direct path:
every comparison is a round trip through the runner (out of the qsort,
resolve, the comparator, back into a qsort continuation). In the sampling
profile no comparator sample has the qsort on its stack.

`engine::render_loop::sort::Sort` ports it at Port grade, block by block,
and evaluates the layer build's two comparators (`0x0003bb80`, and
`0x0003bbd0` for layer 5: stdcall `(a, b, context)`, AL whether `a`
sorts after `b`, comparing the entries' key fields in turn; the first
starts at `+0x10`, `+2`, `+5`, the layer-5 one at `+5`, `+6`, `+0x10`) in Rust with their exact register
and stack effects, so a layer sort makes no guest call at all. Any other
comparator is still called (`Step::Call`), so the port is correct for
every caller of `0x0013da70`.

**Expected saving.** In the 2026-11-17 Old Mombasa profile (142.37 s at
500 Hz, about 4,260 frames, menus included) the qsort holds 188 samples
inclusive and the comparators 192: 380 samples, 0.76 s, about 0.18 ms a
frame over the run. The runner round trips per comparison are charged to
neither (they show as `runtime::` frames), so that is a lower bound, and
gameplay frames sort more entries than the average. The cost of turning it
on is one hooked call per layer build (about 15 a view).

### The render-state list (`0x0001a170`, `impl.render.states`, `impl.render.states_skip`)

The first target ranked below. The shader draw `0x00016b90` (under layer
3: 8.26 ms a frame and 1,882 D3D calls in the census run) calls
`0x0001a170` once per draw to apply the shader's engine-driven render
states.

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
| `0x88` | the device's `+0x790`, dirty bit `0x4000`, `0x004076b8` |
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

**Cost model.** With either switch on, `0x0001a170` becomes a hooked
call. The shader draw leaves the direct path once per draw, and the
colour and setter calls become `call_guest` transfers where they were
direct calls before. In return, every `SetRenderState_Simple` call is
made from Rust. It is a Rust HLE override, so it was already a runner
round trip, but it no longer unwinds the generated frames above it. The
redundant calls are not made at all. Whether this is a net win needs the
in-game measurement below. `impl.render.states` alone measures the
hook's cost, and the counters show how many calls were skipped.

**Counters.** Every 100,000 lists the feature logs
`render-states: lists L entries E simple S skipped K (P%) deferred D setters T`.
These are running totals: `simple` counts the calls made and `skipped`
the calls not made.

### The transparents' index sort (`0x0013df80`, `impl.render.transparent_sort`)

The transparents (`0x0001e370`) are sorted by `0x0013df80`. This is the
engine's second qsort, and it sorts u16 indices. EAX holds the count and
ECX the base. The stack holds `(unused, compare, context)`, and it
returns with `ret 0xc`. Ranges shorter than 9 go to a short sort at
`0x0013e140`. Pending ranges are kept on a 30-deep stack in the frame.

The comparator `0x0001e700` reads 0x70-byte records at
`context + 0x70*index`:

1. It orders by the dword at +0.
2. On a tie, if either record has its flag at +0xc set, it orders by
   which side of the record's plane (+0x10..+0x1c) the other record's
   point (+0x64..+0x6c) and the camera (`0x004b9da0`) fall on.
3. Otherwise it orders by the float at +4, then by the dword at +8.

`engine::render_loop::sort_indexed::IndexSort` runs the sort, the short
sort and this comparator in Rust. It reproduces SSE's NaN quieting and
the registers the comparator leaves (EAX, ECX, EDX, `xmm0`, `xmm1`). Any
other comparator is called in the guest. The original comparator also
changes `xmm2`, but the port does not model it, since `Regs` has no
`xmm2`. Callers do not read a caller-saved register after a call.

**Expected saving: small.** In the 2026-11-17 Old Mombasa profile, the
sort, the short sort and the comparator hold 30 samples between them (13,
7 and 10) over 142 s. That is about 0.01 ms a frame. The port is worth
having for coverage, at the cost of one hooked call per frame, not for
the time it saves.

## Evidence

- **Differential against the lifted original (Test, needs the XBE).**
  `crates/engine/tests/render_loop_differential.rs` (feature
  `differential`). For each seeded state (globals, 0 to 5 view records
  of random bytes with `[view]` zero two times in three, a negative count
  in some, the debug hook in a third, a signalling-NaN frame time in
  some, random registers and stack alignment), it runs the original on
  the IR interpreter and the port, with every callee a stub on both sides
  that logs the call and answers from the same generator (EAX, ECX, EDX,
  `xmm0`, the camera block `render_game_view` leaves). It requires equal
  call logs (target, all eight registers, `xmm0`, stack arguments, and
  the frame record EAX points at), equal registers at return, and all 64
  MB equal. The call stream into the pass bodies is what decides the draw
  stream, so equal call streams with equal memory mean equal draws: this
  is the draw-stream comparison at the orchestrator's boundary, without
  running a frame.
- **The runtime driver (Test, no game code).**
  `crates/feature/tests/render_loop.rs`: the plan places the Rust
  `render_views`; the nine callees are Rust overrides at their addresses
  that log and `ret n`. The runtime's call stream (targets and seven
  registers) and return state equal the port's driven directly, through
  the debug hook, a drawn view, two skipped views and an alternate view.
- **`render_game_view` and `render_scene_passes` against the lifted
  originals (Test, needs the XBE).** `crates/engine/tests/scene_differential.rs`,
  the same method: every callee a stub on both sides that logs target,
  eight registers, `xmm0`, `xmm1` and stack arguments, and answers EAX,
  ECX, EDX, `xmm0`, `xmm1`, and in one call in five a changed EBX or EDI
  (the port must use what the callee left). Predicates answer AL 0 or 1,
  the early-outs rarely fire, the object lookups answer small indices or
  -1 into synthetic object, cluster and shader tables, whose floats include
  both zeros and NaN. Globals (the layer mask, target kind, fog bytes,
  flags, the player), the six arguments, and the stack under the frame
  are random per state, so the branches on frame bytes the callees would
  fill go both ways. Call logs, registers at return and all 64 MB must be
  equal. Every callee is reached.
- **The layer sort against the lifted original (Test, needs the XBE).**
  `crates/engine/tests/sort_differential.rs`: 120 seeded states with
  0 to 2529 entries and tie-heavy keys, both layer comparators run for real
  on the interpreter (natively in the port), a foreign comparator stubbed
  to cover the guest-call path, aligned and byte-swap element sizes. The
  sorted buffer (the layer's draw order), the foreign comparator's call
  log, the registers and all 64 MB are equal.
- **The runtime driver for the new ports (Test, no game code).**
  `crates/feature/tests/render_loop.rs` places `render_game_view` and
  `render_scene_passes` and checks the runtime's calls against the port
  driven directly, through the targeted base layers, the light pass and
  the transparents.
- **The render-state list against the lifted original (Test, needs the
  XBE).** `crates/engine/tests/states_differential.rs`.
  - Inputs: 1,200 seeded states (three seeds of 400). Each has a list of
    0 to 24 entries over 64 random definitions, covering every type, both
    sources and every state range, including states past `0xa5`. Floats
    include NaN, infinities and halves. The x87 control word is random
    across precision and rounding, and the shadow and dirty word are
    random.
  - Stubs: `SetRenderState_Simple` is a stub on both sides that updates a
    model of the GPU, mapping push header to value, seeded from the
    shadow. The setters and the colour function log their arguments and
    answer from the same generator.
  - Exact mode must give equal call logs (target, eight registers,
    `xmm0`, stack argument), equal registers at return and all 64 MB
    equal.
  - Skip mode must give the same GPU model, the same registers at return,
    and the same memory (shadow, dirty word, everything) outside the dead
    stack below the frame. It must make the same setter and colour calls
    in the same order with the same arguments. Its Simple calls must be
    an ordered subsequence of the original's, and the skip count must
    equal the number of calls left out.

  ```
  render states: 1200 states, 10583 calls equal (exact); skip mode: 574 of 5100 Simple calls skipped, same GPU state, shadow and other calls; targets 31
  ```

  A synthetic per-draw workload (`skipping_on_a_per_draw_workload`) runs
  400 draws, each applying a 12-entry list with eight Simple entries over
  16 states and fixed parameters per shader. Within a list, a state that
  appears twice with different values is never redundant.

  ```
  one shader: 400 draws, 3200 Simple calls in the original, 1596 skipped (49.9%)
  four shaders, sorted: 400 draws, 3200 Simple calls in the original, 1683 skipped (52.6%)
  four shaders, interleaved: 400 draws, 3200 Simple calls in the original, 396 skipped (12.4%)
  ```

  The skip rate depends on draw order. The game's real share is in the
  counters' line.
- **The index sort against the lifted original (Test, needs the XBE).**
  `crates/engine/tests/transparent_sort_differential.rs` runs 180 seeded
  states. 135 use the transparent comparator, and the rest use a foreign
  comparator, stubbed. Records include NaNs and infinities, with up to
  599 entries. The draw order, the foreign comparator's calls, the
  registers (`xmm0` and `xmm1` included) and all 64 MB are equal.
- **The render-state driver (Test, no game code).**
  `crates/feature/tests/render_loop.rs` places `0x0001a170` with each
  switch, over a list holding every kind of entry. The runtime's calls
  equal the port's driven directly: setters pop their argument, the
  colour function keeps ECX, and the runtime's x87 control word is used.
  The skipping switch makes two fewer Simple calls, one for a repeated
  entry and one for a value the shadow already holds.
- **Not covered.** A `RecordingBackend` capture of a real frame with the
  switch on and off (needs a game run; see the check below); faults inside
  the port (it reads and writes in a different order from the original,
  so a bad pointer faults at a possibly different address; the original
  would crash the game either way).

## Checking it in game

No image rebuild: the ports are Rust in the `render-loop` feature, placed
on points the runtime already resolves. Rebuild the `h2` binary only. All
switches are off by default; `impl.render=on` turns on every
`impl.render.*` port at once.

```sh
# The three new ports.
H2_FEATURES=impl.render.game_view=on,impl.render.scene_passes=on,impl.render.layer_sort=on

# The render-state list skipping redundant Simple calls (logs the
# render-states counter line), and the transparents' sort.
H2_FRAME_STATS=1 H2_FEATURES=impl.render.states_skip=on,impl.render.transparent_sort=on
# The list port without skipping: the hook's own cost.
H2_FRAME_STATS=1 H2_FEATURES=impl.render.states=on

# Frame stats and the stage split (the site lines need scene_passes off).
H2_FRAME_STATS=1 H2_FEATURES=render-timing,impl.render.game_view=on,impl.render.scene_passes=on,impl.render.layer_sort=on

# Draw streams: the same frame of a run with and without, then diff.
H2_BOOT_DRAWLOG=$R/on.txt H2_BOOT_DRAWLOG_FRAMES=600 H2_FEATURES=impl.render=on
H2_BOOT_DRAWLOG=$R/off.txt H2_BOOT_DRAWLOG_FRAMES=600
```

What to expect: the orchestrator ports change no draw and save no time on
their own (each adds a runner round trip per call they make, about 100 a
view for the scene passes; compare `render` with and without). The layer
sort should take `render` down by the sort's share (above); `render-timing`
shows it in `scene_passes`, where the layer draws run, and the site line
`0x0002bed0` (layer 3) most. Draw logs from two runs line up only when the
two runs render the same frame (same input and timing, or a replay).

For the render-state list, compare `render` across three runs:

- no switch;
- `impl.render.states`, which shows the hook's cost alone (one exit from
  the direct path per draw);
- `impl.render.states_skip`, which shows that cost minus the skipped
  calls.

With `impl.render.states_skip`, the draw log should show the same draws,
and the D3D call log should be shorter by the `skipped` count.

## Where the scene passes' time goes (measured)

`render-timing` site lines from the 2026-11-17 Old Mombasa census run,
weighted over the 100 one-second windows with `render` above 15 ms (2,607
frames, mean `render` 24.9 ms with the observers on). A site's time is
its callee's, inclusive, minus the stages carved out (the per-light pass
`0x00031170` is `light_passes`). ms per frame:

| Site | Callee | ms | guest | D3D | D3D calls | What |
|---|---|---|---|---|---|---|
| `0x0002bed0` | `0x00044550` | 8.26 | 5.51 | 2.75 | 1,882 | layer 3, the opaque scene ([layer-draw.md](layer-draw.md)) |
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

## Next port targets, ranked by measured ms

A port only saves time when it removes guest work or guest transfers
without adding them: a Rust function that calls back into guest code pays
a runner round trip per call ([layer-draw.md](layer-draw.md), "Hooked
calls leave the direct path"). So the targets are whole subtrees or
leaves.

1. **The shader draw under layer 3 (`0x0001bbf0` / `0x00016b90`, most of
   8.26 ms).** The render-state list `0x0001a170` is built (above).
   Still left in this subtree:
   - The second list, `0x0001ab50`. It is not read yet.
   - The parameter evaluation.
   - The bigger step: porting `0x00016b90` whole, so that its calls to
     the lists, the setters and the HLE entries stop leaving the direct
     path. That would also make the list port's hook free.

   [layer-draw.md](layer-draw.md) records that the original already
   skips unchanged textures, constants and pixel shaders.
2. **The transparents (`0x0001e370`, 2.58 ms).** The sort is built
   (above), and saves about 0.01 ms. The rest of the time is in the
   per-entry handlers, which are called through a pointer per entry.
3. **The dynamic lights' bookkeeping (`0x00031c80`, 1.88 ms, no D3D).**
   Per light: collection (`0x00031910`), object and cluster gathering
   (`0x000c1d90`), `0x001320f0`, `0x00031660`. This is also 7b's seam.
4. **`0x000335e0` (1.24 ms) and the visibility `0x00040e90` (0.91 ms,
   no D3D).** Guest-only subtrees; read them before choosing.
5. **Pass tags.** With `render_scene_passes` in Rust, set the device's
   `PassTag` from `ScenePasses::at()` before each layer call
   ([feature-engine.md](../engine/feature-engine.md), "Rendering").
   `render_game_views` stays recompiled (camera records are contract).

## Later stages

- **7b.** Replace the `0x00031c80` call site (dynamic lights) in the
  ported pass sequence with a Rust lighting pass; E3 lighting becomes a
  pass rather than a draw rewrite.
- **7c.** Targets and post-processing: the `0x00014bc0` / `0x00015680` /
  `0x00015370` target and state calls in the pass sequence are where a
  Rust pass loop picks view-resolution targets.
- **7d.** Geometry submission and materials (`0x00041040`, `0x0001bbf0`),
  after the pass loop is Rust.
- **Measure first.** `render-timing` ([frame-budget.md](../perf/frame-budget.md),
  "Render front-end versus simulation") says how many ms the stages
  hold; the port itself saves no time until a pass body moves.
