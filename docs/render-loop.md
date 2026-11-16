# Render loop in Rust (stage 7a)

Stage 7a of the rasterizer front-end ([feature-engine.md](feature-engine.md),
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
  a straight-line pass sequence, about 120 calls, no loop: the triple
  0x00044370 / 0x00044550 / 0x000445d0 brackets each pass (S: a pass
  marker, begin/end), 0x00014bc0 / 0x00015680 / 0x00015370 set targets
  and state around passes, 0x0002c370 is called for six layer passes,
  0x00031c80 is the dynamic-light pass (which calls render_light_passes
  0x00031170 per light), 0x00013630 is called on every exit
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
[settings-menu.md](settings-menu.md)), then calls `render_views` once at
`0x00223714` with `ecx = 3`, EAX from EDX (S: the view count), and the
stack arguments `(ebp, [esp+0x10], 0x0047f210)`: the view records are
the static array at `0x0047f210`. The record fill uses `rep movsd` of 0x1d dwords
and x87 (`fpatan`) for a field at `+0x28` (S: the horizontal FOV from the
vertical one). Not ported: the records are camera state other code reads.

### `render_scene_passes` `0x0002bcd0` (Code, shape only)

Straight-line, not a loop: early-outs on `0x000289e0`, `0x00159610`,
`0x00040e90`, then the pass sequence. The layer order recorded in
[e3-lighting.md](e3-lighting.md) (base layers 1, 3, 5, 8, 20, lights from
`0x0002c03e`) is this sequence. Porting it is one long Port with ~120
call sites; each call site's full register state has to be read as was
done for `render_views`.

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
- The one guest-visible difference: callees see the continuation token
  as their return address, not `0x0002b6b9` etc., as with every chain that
  runs something after a body.

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
- **Not covered.** A `RecordingBackend` capture of a real frame with the
  switch on and off (needs a game run); faults inside the port (it reads
  and writes in a different order from the original, so a bad pointer
  faults at a possibly different address; the original would crash the
  game either way).

## Next

- **7a, rest.** `render_game_view` (6 calls, read above) and
  `render_scene_passes` the same way: read each call site's registers,
  port as a state machine, extend the differential with their callees as
  stubs. With `render_scene_passes` ported, the pass identity of every
  draw is known in Rust: set the device's `PassTag` before each pass call
  ([feature-engine.md](feature-engine.md), "Rendering").
  `render_game_views` stays recompiled (camera records are contract).
- **7b.** Replace the `0x00031c80` call site (dynamic lights) in the
  ported pass sequence with a Rust lighting pass; E3 lighting becomes a
  pass rather than a draw rewrite.
- **7c.** Targets and post-processing: the `0x00014bc0` / `0x00015680` /
  `0x00015370` target and state calls in the pass sequence are where a
  Rust pass loop picks view-resolution targets.
- **7d.** Geometry submission and materials (`0x00041040`, `0x0001bbf0`),
  after the pass loop is Rust.
- **Measure first.** `render-timing` ([frame-budget.md](frame-budget.md),
  "Render front-end versus simulation") says how many ms the stages
  hold; the port itself saves no time until a pass body moves.
