# The Havok collide phase

Status: Built as a frame replay. Its callees (pending operations, the island updates and pair passes, the listeners, merges and the queue) run lifted.
Code: `crates/rewrites/engine/src/sim_physics/world.rs` (`collide`), `crates/hooking/guest-layout/src/points.rs` (`HAVOK_COLLIDE_PHASE`). Tests: `the_collide_phase_walks_the_islands_and_world_passes_in_retail_order` (`world.rs`), `havok_collide_phase` (`crates/rewrites/engine/tests/sim_physics_differential.rs`).
Always installed: the physics entries have no switch.

The first of the world step's two phases. The step tree, the censuses
and the persistent writes are [havok-step.md](havok-step.md)'s.

## The phase

`0x0030fcb0` (thiscall world, `&{dt, 1/dt}`, `ret 4`) is a frame replay in
Rust: `world::collide` in `crates/rewrites/engine/src/sim_physics/world.rs`,
installed with the physics entries at `HAVOK_COLLIDE_PHASE`. Every call
under it stays a guest call, resumed in the phase's own frame (registers
and stack as retail's), so the scheduler can stop in any of them. **E**

| Order | Does | Resume |
|---|---|---|
| 1 | step copy `W+0x150`, the 16 bytes at `[W+0xcc]` to `W+0x140`, the four terms below | |
| 2 | `W+0x12c = 1`; `0x0030f6b0` when `W+0x24 > 0` (signed) | `0x0030fd8b` |
| 3 | `W+0x12d = 1`; for `i` while `i < W+0x0c` (read again after each island): `0x00317d30(island[i], W+0xc4, W+0x140)` | `0x0030fdb3` |
| 4 | when `W+0xac != 0`: `W+0x12d = 0`, cdecl `0x003188c0(W, island[i], step)`, `W+0x12d = 1` | `0x0030fdfc` |
| 5 | `W+0x12d = 0`, `W+0x12c = 0`; `0x0030f6b0` when `W+0x24 > 0` | `0x0030fe59` |
| 6 | `0x0030fc30` when `W+0xf4 != 0`; `0x0030cda0` when `W+0x100 != 0` | `0x0030fe6a`, `0x0030fe7b` |
| 7 | `0x0030f6b0` when `W+0x24 > 0`; cdecl `0x003187a0(W, step)` | `0x0030fe89`, `0x0030feba` |

The terms are x87 under the caller's control word, through the `x87`
crate's ABI with the status word, tags and last-instruction pointers kept
as retail leaves them: `W+0x170 = dt * W+0x180`, `W+0x174 = fild(W+0x17c) *
1/dt`, the copies `W+0x198`/`+0x19c`, `W+0x1a0 = fild(W+0x17c) * W+0x170`,
`W+0x1a4 = W+0x180 * W+0x174`, each `fstp dword`. The iteration count is a
32-bit integer, so its product can round twice under a 64-bit precision
control; the crate models that.

Registers at the return: ebx, ebp, esi and edi as on entry, ecx the slot
the entry pushed (the world), eax `[0x00480158]`, the flags from the last
monitor compare. The port's entry condition holds the monitor stream off;
a callee that turns it on inside the phase panics by name, since its
records carry `rdtsc`. Tests:
`the_collide_phase_walks_the_islands_and_world_passes_in_retail_order`
(`world.rs`) and `havok_collide_phase` (`sim_physics_differential.rs`,
against the lifted phase with seeded callees that change the island count,
the pending count, merges and the queue).

## Related

- [havok-step.md](havok-step.md): the step tree and its contracts.
- [havok-integrate.md](havok-integrate.md): the phase after this one.
- [sim-rewrite-havok.md](sim-rewrite-havok.md): the phase 5 plan.
