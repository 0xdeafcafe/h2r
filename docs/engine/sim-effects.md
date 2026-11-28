# Simulation rewrite, phase 6: effects, particles, contrails and decals

Status: ports written, native only; run
parity and timing not yet recorded.

Design for the phase is in [sim-rewrite.md](sim-rewrite.md) ("Effects
(phase 6)"). This page states what runs.

## What runs

`crates/rewrites/engine/src/sim_effects` holds one [`Port`] per retail
function or stretch. A port is an entry plus the return addresses of the
guest calls it makes. At each, it works from the registers and the retail
stack frame, so a call it declines, or a checkpoint restore between two
segments, is finished by the original from the same state. A port keeps no
Rust state past a guest call; the effect clock, the particle step, the
contrail clock and the decal clock are values gathered from the datums for
one step and written back before the next call.

| Module | Retail | What it does |
|---|---|---|
| `events.rs` | `effect_step_events` `1792b0` | one effect's event clock: a tick can cross an event's delay, run its parts, cross its duration and start its successor in one call. Each callout sees the clock in the effect datum |
| `particles.rs` | `PARTICLE_SYSTEM_UPDATE` `247140` | the emitter's particle advance: age, wrap or expire, rest test, motion and bounds. Rest queries, on-death effects and the allocator run as guest calls on the original frame |
| `particle_birth.rs` | `PARTICLE_BIRTH` `247950` | slot allocation, linking, the five local RNG draws, then the shape initialiser `249170` as a guest call |
| `contrails.rs` | `17c040`, `17c880` | the point emission clock; the release of a trail's four point chains |
| `decals.rs` | `17cc60`, `17ccd0`, `17ce00`, `17d860` | the root clock and fade, bucket link and unlink |

Not ported here: the contrail point update `17c540`, the particle view
update (`docs/graphics/native-particles.md`) and the render side of decals.
They stay guest code and see the same datums. The phase's effect walk
`176e40` and the part creation `17a8a0` are
[effects-parts.md](effects-parts.md)'s. So is a second body of `1792b0`
there; `events.rs` keeps the point and that body is
left out.

RNG draws keep retail order: birth draws from the local effects stream in
program order, and the shared stream `[0x4e7408]` is only touched by the
guest callees.

## Install and parity arm

Every port always installs through
`sim_bridge::sim_effects`, with no switch. `supported` gates each call: the float
environment, bounded chains and loops, and no armed memory trap. A call the
gate refuses is counted and runs the original.

The parity arm is the shadow registry ([shadow-exec.md](shadow-exec.md)).
Every port registers under its point's name in the group `sim.effects`,
installed or not. `shadow.enable` over the agent API
checks sampled calls against the lifted original while the port stays
authoritative.

## Tests

- `engine::sim_effects::tests`: synthetic guest checks of the decal link
  and unlink and the contrail clock, plus a disjointness check on the
  port table.
- `tests/sim_effects_differential.rs` (`differential` feature, skipped
  without the image): the decal link and the contrail clock against the
  lifted original from one synthetic state.
- `sim_bridge::sim_effects`: the installer installs every port with its
  sites, and registers all of them with the shadow registry.

The remaining ladder steps (sim-rewrite.md section 3: run parity on
Lockout and Waterworks with arena hashes, the cache-cold arm, the
cross-architecture run) are the coordinator's.
