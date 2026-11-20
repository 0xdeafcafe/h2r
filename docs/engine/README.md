# Engine function contracts

One page per rewritten function family, for the Rust in `crates/engine`.
Each records addresses, the register/stack convention, every memory effect,
failure paths, quirks that are reproduced on purpose, and the differential
evidence. No disassembly listings or game data.

| Page | Functions | Status |
|---|---|---|
| [tag_get_checked.md](tag_get_checked.md) | `0x122c10`, `0x122d00` (+ bsearch `0x13ddd0`) | Rust + adapters |
| [datum_new.md](datum_new.md) | datum arrays, `0x16b570–0x16bd10` | Rust + adapters for every leaf, `datum_initialize` included; `data_new`/`data_dispose` stay generated (they call guest allocators) |
| [locale_string_id_find.md](locale_string_id_find.md) | `0x1a02d0`, `0x1a0310` | Rust + adapters |
| [memory_pool.md](memory_pool.md) | `0x18e1f0–0x18e700` | contract notes only |
| [sim-rewrite.md](sim-rewrite.md) | animation, collision, objects, Havok, campaign subsystems | plan: Rust subsystem rewrites with guest-layout state |
| [sim-rewrite-havok.md](sim-rewrite-havok.md) | the Havok step `0x311100` and everything under it | design: phase 4 of the plan, bit-exact structural port |
| [sim-rewrite-collision.md](sim-rewrite-collision.md) | BSP and MOPP queries, collision query, cluster flood, object visit | design: phase 2 collision world, entries, data layouts, slices |
| [collision-queries.md](collision-queries.md) | BSP point to leaf `0x14a280`, location `0x11bed0`, vector test `0x1de630`, `0x1de6d0` | Rust (`sim-collision`) + adapters behind `impl.sim.native.collision`, default off |
| [sim-rewrite-objects.md](sim-rewrite-objects.md) | object walks `0xb6f10`, `0xb7060`, `0xb7150`, per-object update `0xbc470`, `0xbc820`, type update trees, lights `0xc1400` | design: phase 3 of the sim rewrite |
| [hs_vm.md](hs_vm.md) | HaloScript interpreter `0x207c20–0x20a490`, scheduler `0x209310` | native Rust VM in `crates/hs` (`hs::vm`); not yet wired into the game |
| [sim-tier-experiment.md](sim-tier-experiment.md) | compose `0x142a60`, pose `0x1421f0`, MOPP box `0x2ff8b0`, BSP segment `0x1de6d0`, solver `0x31c940` | experiment: `emit::sim` against the reference tier and the hand ports |
| [sim-tier-writes.md](sim-tier-writes.md) | everything `game_tick` `0x137fe0` reaches | method: static census of the tick's stores |
| [anim-sampling.md](anim-sampling.md) | animation sampling `0x2798a0` and everything below it | Rust (`sim-anim`) + adapter behind `impl.sim.native.anim`, default off |

## Fault behaviour

A fault ends guest flow, but the bytes written before it stay written, and
they must match. Fills follow the instructions the originals use (`rep
stosd` of `len >> 2` dwords, then `rep stosb` of the rest): every store
before the faulting dword or byte lands, and the faulting store writes
nothing (`GuestMemory::fill`). The name copy is the CRT `strncpy` at
`0x3203e0` ported store for store (`memory::strncpy`). Reads that a
function performs only on corrupt state (the element below the data
pointer, the "group record" at address 0) are performed too.

Stack arguments are read where the original reads them, interleaved with
its stores: `data_initialize` clears the header before it reads the name
and reads the bitmap before the allocator; `data_initialize_contiguous`
reads buffer, allocator, name; `locale_string_id_exists` reads its count
only when the table is loaded; `data_last_datum` stores its cursor in its
argument slot at each step. The adapters do not model the originals'
pushes of saved registers into the dead stack below ESP, so a stack that is
unmapped *below* the entry ESP faults later than the original would.

With `runtime-glue`, `invoke` reports the runtime fault the Rust body
returned: the latest one recorded, since `fill` retries a faulting bulk
store one store at a time.

## How the contracts were checked

Contracts come from `h2 disasm` and a linear scan of `.text` for call sites.
Behaviour is checked differentially by `crates/engine/tests/differential.rs`
(feature `differential`): the original function, lifted by `crates/lift` and
run on the IR interpreter (`ir::interp`) over the loaded image, against the
`crates/engine` adapter, from the same 64 MB guest state with the same
registers and a planted return address. The masks and the run command are
in that file's header:

- both return: same return target, ESP, EAX (unless the adapter has no
  output) and every register outside the adapter's clobber list;
- both fault: same fault address and direction;
- either way, all of memory except the 8 KiB of dead stack below the entry
  ESP.

Fixed fault fixtures (an element running off the end of RAM through the
dword and byte parts of a fill, a name copy faulting mid-string, the
element-below-data read wrapping, a group table past RAM, a tag entry at
address 0, `data_last_datum` faulting mid-walk) run before the seeded
scenarios, followed by every stack-argument adapter called with its stack
ending at the end of RAM so that only the first k arguments are mapped,
for every k. The test pins the image by
SHA-256 (`03215919…935d`) and fails if any adapter goes unexercised.

The harness runs every call three ways (the original, the adapter, and
`engine::runtime_glue::invoke` on a `runtime::Guest`, the body the game
installs) and also checks the typed views `Datums` and `Tags` against the
original's answers. Its results and the bugs it caught are in
[notes/feature-engine.md](notes/feature-engine.md).

The same feature builds the other differential tests in
`crates/engine/tests`: `layer_differential.rs` (the layer entry loop) and
`states_differential.rs` (the render-state list), which stub every callee
on both sides and compare the call logs, the registers at return and all
of memory ([render-loop.md](../graphics/render-loop.md),
[layer-draw.md](../graphics/layer-draw.md)); and one per other port
(`colour_pass`, `glow`, `light_constants`, `light_passes`,
`shadow_constants`, `screen_effects`, `screen_grid`, `sim_native`,
`sim_physics`).

Agreement here tests the Rust against the lifted original. It shares the
lifter's instruction semantics, so it is not independent evidence about the
CPU (`docs/lifter/LIFTER.md` §8).

## Registering the overrides

`engine::abi::OVERRIDES` lists each address with its adapter, output
register and clobber set. With the `runtime-glue` feature,
`engine::runtime_glue::register_all(&mut dispatch)` registers them all
through `Dispatch::register_override`; each handler takes one
`ctx.memory()` guard for the whole call and returns `TransferAbi::ret`.
Nothing calls `register_all`. The game installs the same adapters
through the `feature` crate's `engine-leaves` feature
(`crates/feature/src/leaves.rs`): only the locale group is on by default
([feature-engine.md](feature-engine.md)).
