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
| [hs_vm.md](hs_vm.md) | HaloScript interpreter `0x207c20–0x20a490`, scheduler `0x209310` | native Rust VM in `crates/hs` (`hs::vm`); not yet wired into the game |

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

The `differential` feature lists `ir` and `lift` as dependencies
(`crates/engine/Cargo.toml`). The run below used a copy of the lift crates
from before they landed, so it predates that wiring. Recorded on
2026-11-15, seeds 1, 7, 99, 2026, 31337 at scale 1: 7.66 M original
instructions interpreted, 93,291 calls, 5 matching fault outcomes,
zero disagreements across all 22 adapters. After the argument-order
fixes, with the unmapped-argument fixtures, the same five seeds: 7.66 M
instructions, 93,304 calls, 17 matching fault outcomes, zero
disagreements. Reverting `fill` to an
all-or-nothing write makes the first fault fixture fail; reading all of
`data_initialize`'s arguments up front, reading the contiguous variant's
name before its allocator, reading the locale count before the loaded
test, or storing `data_last_datum`'s slot only at the end each make an
unmapped-argument or mid-walk fixture fail (9 disagreements together). Earlier
injected bugs (salt wrap, grandparent acceptance, locale bound) were each
caught by the seeded runs.

The same feature also builds the stage 7a proofs, `render_loop_differential.rs`
(`render_views`) and `layer_differential.rs` (the layer draw), which stub
every callee on both sides and compare the call logs, the registers at
return and all of memory ([render-loop.md](../graphics/render-loop.md),
[layer-draw.md](../graphics/layer-draw.md)).

Agreement here tests the Rust against the lifted original. It shares the
lifter's instruction semantics, so it is not independent evidence about the
CPU (`docs/lifter/LIFTER.md` §8).

## Registering the overrides

`engine::abi::OVERRIDES` lists each address with its adapter, output
register and clobber set. With the `runtime-glue` feature,
`engine::runtime_glue::register_all(&mut dispatch)` registers them all
through `Dispatch::register_override`; each handler takes one
`ctx.memory()` guard for the whole call and returns `TransferAbi::ret`.
The feature builds against master's `runtime`; nothing calls
`register_all`. The game installs the same adapters through the `feature`
crate's `engine-leaves` feature (`crates/feature/src/leaves.rs`, added in
`crates/boot/src/lib.rs`): only the locale group is on by default.
