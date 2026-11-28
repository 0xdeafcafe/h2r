# Headless sim: work nobody consumes, and what may be skipped

Status: Partly built. Lane H0's measurements are built (the sim report's `wall.phantom_*` fields and `guest_census` block, [sim.md §6](../runtime/sim.md#6-metrics)); no skip is. Row 1 and the census are measured; every other ms figure is supposed.
Code: H0's, `crates/extensions/game/src/sim_census.rs` and `bot_glue/retail_input.rs`; each other lane below names its files. Tests: named per lane.
Switch: none here. Row 1's is proposed in [sim-phantom-build.md](sim-phantom-build.md).

A headless bot match ([sim.md](../runtime/sim.md)) has no screen, no
speaker and no local player. This page asks which per-tick work it still
does for those three, and which of that can be skipped with the match
left bit for bit the same. The rule: a skip is **safe** only if it is
safe for all three consumers of a sim: the game state
(`state_hashes_v1` equal with `impl.sim.arena-hash` on, by
`h2 sim diff`), films, and the Uplink viewer. It is one lever family of
[sim-10x.md](sim-10x.md) (its lever 8).

The answer in one paragraph: **the title already skips nearly all of it
by itself.** The four bots are phantom peers, remote players with no
local user, and retail gates the director, the observer cameras, first
person, the per-user HUD, the sound listeners' BSP queries and the
local class of effects on a local user being mapped. Nothing is
replicated to a phantom either. What is left in the guest is small,
unmeasured, or read back by the tick. The one large candidate is ours:
each tick the host builds the phantoms' actions in a private
copy-on-write copy of guest RAM. That build is 22% of the tick, and
[sim-phantom-build.md](sim-phantom-build.md) is its design.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Addresses are research points in the retail XBE.

## How it works

### What a phantom host does not run

The local-user map is `[[0x4e8c20] + 0xc + 4 * user]`, −1 for an unmapped
user, and `[[0x4e8c20] + 8]` is the count of local users. A sim host
leaves all four profiles signed out
([sim.md §5](../runtime/sim.md#5-setting-up-a-match-by-script)), so the
map is four times −1 on the live side (**Run**: the census counts 0
local users on every tick, [note](notes/sim-10x.md)).
With that, by retail's own tests (**E**):

| Work | Entry | Gate that skips it |
|---|---|---|
| Director, per user | `0x1554b0` (per frame, from the main loop `0x12b450`) | `0x155507` |
| Observer cameras: camera placement `0x170fd0`, its collision `0x171830`, the listener leaf, predicted resources `0x3f450` | `0x16f280` (per frame) | `0x16f2c1` |
| First-person weapon and its animation (`0x166d75`, `0x16760c`) | `0x165dc1` (tick) | `0x165de2` |
| Player control and the local action builder, with its aim-assist search | `0x1870a0` (per frame) | `0x1870c4` |
| Per-user HUD and weapon-target state | loops at `0x1e7015`, `0x1e73d0`; `0x1a264e` under `0x1917b2` | the same map test |
| Screen-shake and player effects, per user | `0x153950` (tick) | `0x153a0e` |
| Sound listeners: unit, leaf and cluster of each | `0x127400` | `0x127468`; the record gets −1 and no BSP query runs |
| Particle level of detail from the observer's scale | `0x248970` | loop bound is the local count, `0x2489d0` |
| Effects whose definition has flag bit 2 clear (the local class) | `0x179730` under the effect update `0x176e40` | see "Effects" below |

So lever 8 of sim-10x.md ("phantom seat residue") has nothing to take in
the guest, and the seats' prediction walk answers a call the observer
never makes on a phantom-only host (its other callers are not read).
Making the phantoms "remote-like" is what the roster already is. The
opposite, local seats, brings back the 16% of the thread that
[sim-attribution.md](sim-attribution.md) measured for four local users.

Nothing goes to a phantom over the network: Rust writes the peer and its
player into the session with no channel and no simulation view
(`crates/live/bots/src/phantom/session.rs`,
[bots.md §5.1](../game/bots.md)), so the per-view loops of the tick's
record close `0x684e0` and of the entity manager `0x8a090` run zero
times (**E**). The fixed scans stay.

### What it still runs

One frame is one tick, so per-frame work costs the same as tick work.
Running several ticks a frame would thin it out, but the phantoms' pads
are gathered once per frame and the sound service keeps its own cadence,
so that is a different match ([sim.md](../runtime/sim.md#per-tick-work)).

**The phantoms' action build (ours).** At the remote gather
(`SIMULATION_SLOT_ACTION`, `crates/extensions/game/src/bot_glue.rs`) each
phantom's pad goes through retail's own local action builder in a private
view where the phantom is local user zero
(`bot_glue/retail_input.rs`, `Builder::action`). Per tick the first seat
calls `SecondExecutor::snapshot`: `Guest::cow_refresh` and
`Guest::freeze_ram`, two copy-on-write copies of the 64 MiB RAM block
(`crates/runtime/runtime/src/executor.rs`, `guest/view.rs`). Each later
seat reverts the pages the view wrote. Every seat then makes six guest
calls: bind `0x1872d0`, profile to look rates `0x190c34`, the camera
origin `0x1557c0`, the input read `0x124c00`, the remap `0x217c40` and
the builder `0x186ab0`, which runs the aim-assist search
(`0x1a4900`). The action is game state; the copies are not. Measured
on Lockout with four phantoms (**Run**, [note](notes/sim-10x.md)): the
build is 1.15 ms of a 5.19 ms tick, and about 1.5 of 7.0 with eight
matches at once. The two copies are 0.12 to 0.17 ms of it, one snapshot
a tick, remapped. The rest is the guest calls, the reverts and what a
mapping made each tick costs in kernel faults; the live side also takes
a fault of about 2.8 µs on its first write to each 16 KB host page while
the copies stand
([second-executor.md](../runtime/second-executor.md#2-guest-memory-view)),
which no timer covers. Where the host cannot remap (`cow.rs`: every host
but macOS), a snapshot copies all RAM bytes. None of this is inside
`wall.bots_ms_per_tick`: `bots::think::GameThreadTimer` wraps the brain
only (**Code**); the report has it as `wall.phantom_build_ms_per_tick`.
[sim-phantom-build.md](sim-phantom-build.md) reads the build call by
call.

**Sound.** The service `0x125b40` runs every frame. Lane C1's read found
no clean no-sound path ([note](notes/sim-10x.md#2026-11-22-lane-c1s-read-the-titles-no-sound-path)).
New here: with no local user the listener records hold −1 and ask no BSP
query, so the "sound asks 19% of collision" row of the attribution is a
local-roster figure.

**Effects.** `0x176e40` (tick) walks the effect array; `0x137ed0` (per
frame, unconditional but for a zero `dt`) runs the contrail, decal and
particle frame updates (`0x17bc40`, `0x17d690`, `0x176f60` and five
more). Retail has two classes of effect, chosen by definition flag bit 2
(**E**, `0x177c20`): the sim class draws from word 0 of the shared seed
`[0x4e7408]` and is culled against the all-players visibility set; the
local class draws from word +4 and is culled against the local-players
set (`0x179730`). The local set is built from players with a local user
only (`0x150891`), so on a phantom host it is empty and a local-class
effect is suspended at its first update (`0x1797c7`): no events, no
parts, no particles (**Run**: the census finds the local set empty on
every tick, particles and decals 0, a mean of 8 effects and 30 lights,
[note](notes/sim-10x.md)).
Particle setup (`0x249170`, `0x247950`) draws from word +4 only.
Effect parts are a switch at `0x17a8a0`: object creation draws word 0,
damage and character parts change the world, decals, lights and sounds
do not draw word 0. Particle retirement feeds the effect's lifetime
(`+0xfc`, tested at `0x176ebe`), and effects update in index order, so
a skip that changes when a cosmetic datum is freed can reorder word 0
draws.

**Lights.** `0x0c1400` walks the light records each tick and sets a bit
on the owner object (`0x0c3308`); the reader of that bit is not found.

**Small fixed work.** UI update `0x1479c3` (it also runs the lobby),
rumble timers `0x222a70`, the HUD tick `0x1917b2` (for each unmapped user
`0x22beb5` walks a sound block and may start a sound), the 1,024-entity
scan `0x8a090`, the sixteen player records of `0x69a00` and `0x69b50`.

### The other two consumers

**Films.** A film records inputs and checks, and playback re-simulates
([film-format.md](../game/film-format.md)). The recorder reads the action
block before `SIM_UPDATE_APPLY` and the gameplay digest every tick; it
reads no camera, HUD, effect, particle, light or sound state
(`crates/extensions/game/src/theater/actions.rs`, `record.rs`). Three
consequences (**Code**):

- The player refuses a film whose switch spec differs from its own:
  `h.features == switches.spec()` or "film boot facts differ"
  (`crates/extensions/game/src/theater.rs`). A film recorded with a skip
  switch on plays only with it on. With it off it does not start; if it
  did, an arena-exact skip would verify and any other would stop at the
  next arena check.
- A sim film plays back in sim mode, through the host observer, which
  draws the BSP and posed models and no HUD, effects or first person
  ([theater.md](../game/theater.md)). No skipped presentation is ever
  shown from one.
- The 36-byte shot-assist block at action `+0x34` is recorded and written
  back, not recomputed ([input-film.md](../game/input-film.md)). Aim
  assist is film state.

**The viewer.** Uplink draws a sim from telemetry frames
(`crates/live/live-proto/src/game.rs`, `Frame`). Every field comes from
the object table, the player array, engine globals, or one of three
hooks: `WEAPON_FIRE`, `DAMAGE_REPORT`, `PLAYER_KILLED`
(`crates/live/live-telemetry/src/memory.rs`, `hooks.rs`). Projectile
trails are projectile objects joined in the browser; tracers and muzzle
flashes are drawn from `fire`, `aim` and `weapon`; hits and death markers
come from the damage and kill hooks. No field reads an effect, particle,
decal, contrail, light, camera, HUD or sound record, and
[viewer-effects.md](../live/viewer-effects.md) plans none. So no
candidate below can break the viewer; only skipping the object update or
the fire and damage paths would.

## Candidates, ranked

Row 1 is measured, as a share of the 5.19 ms tick of the run that
measured it ([note](notes/sim-10x.md)). The other shares are of the
3.3 ms Lockout tick: no profile of a phantom roster exists, so each is
the model's (sim-10x.md, "The budget", scaled to 3.3 ms) or unmeasured.
"Exact" means the arena arm of `h2 sim diff` prints nothing.

| # | Candidate | Share | State | Films | Viewer | Mechanism | Lane | Expected ms |
|---|---|---|---|---|---|---|---|---:|
| 1 | The phantoms' action build: the private view, its faults and reverts, around six guest calls a seat | 1.15 ms, 22% (**Run**); the copies alone 0.12 | **safe** if every action's bytes are unchanged: the action is the only output | **safe**: `BTAP` tapes the action, playback does not build | **safe** | the executor runs on live RAM with an undo journal, no mapping per tick ([sim-phantom-build.md](sim-phantom-build.md)) | P0 splits, P1 builds | 0.8–0.95 (**S**), plus the live side's faults |
| 2 | Sound service and the audio clock under it | 0.33 + 0.11 (model) | **unsafe**: voice completion and the listener verdict are read by the tick ([sim-budget.md](sim-budget.md#headless-guest-dependencies)) | unsafe with it | safe | none: C1's read stands. One read left: whether a voice is admitted when no listener is active (`0x127d9e`) | none | 0 |
| 3 | Effect tick `0x176e40` and frame update `0x137ed0` | 0.22 (model, from a local roster); far less with a mean of 8 effects and no particles (**Run**) | **unsafe** as entries: object and damage parts, word 0, lifetimes. The local class is already suspended by the title | unsafe with it | safe | none beyond the title's own | none | 0 |
| 4 | Lights `0x0c1400` | about 0.04 (**S**) | **unknown**: the read of object `+4` bit `0x40`; the records are in the arena | not exact | safe | none | none | 0 |
| 5 | Network record and entity scans (`0x684e0`, `0x8a090`, `0x8df50`, `0x8dfc0`) | 0.06 (model) | **unknown**: the pending masks the scan clears, and the session state the pumps keep alive | unknown | safe | none: the per-peer part is already zero | none | 0 |
| 6 | Decals `0x17e670` (a collision trace per placement) | unmeasured, small | digest: **S** safe (word +4 only, no object); arena: **unsafe** (decal array, `0x17d271`) | plays with the switch; not exact against a film without it | safe | the title's own enable byte `[0x46dd48]`, tested at `0x17e67a` | none: not exact | ≤ 0.02 |
| 7 | HUD tick for unmapped users (`0x1917b2`, `0x22beb5`) | unmeasured, small | **unknown**: whether `0x22beb5` reaches the sound start `0x126360` | unknown | safe | none | none | ≤ 0.01 |
| 8 | Rumble timers `0x222a70` | unmeasured, tiny | **S** safe (rumble state only) | safe if exact | safe | not worth a switch | none | < 0.01 |
| 9 | UI update `0x1479c3` | under 0.1% of the Waterworks thread | **unsafe**: it runs the lobby and the menu layer's virtual calls | unsafe | safe | none | none | 0 |
| 10 | Aim assist | inside row 1's builder | **unsafe**: the search fills action `+0x34`, which shot aiming `0x1a4c00` bends by | **unsafe**: recorded in the action | safe (`aim` is the unit's) | none | none | 0 |
| 11 | Cameras, first person, per-user HUD, listeners' BSP, replication to peers | 0 on a phantom host | already skipped by the title | n/a | n/a | none needed | none | 0 |
| 12 | Several ticks per frame | all per-frame work | **unsafe**: pads are gathered per frame | unsafe | safe | none | none | 0 |

## Lanes

| Lane | Scope | Files | Switch | Proof |
|---|---|---|---|---|
| **H0** census (built) | The sim report has `wall.phantom_build_ms_per_tick` and `wall.phantom_snapshot_ms_per_tick` (game-thread time in `retail_input::gather` and in `SecondExecutor::snapshot`), the snapshot's `CopyKind`, and a `guest_census` block read once a tick by the sim's tick observer: the local-user count, whether the local visibility set `[0x4e6948] + 0x1178` is empty, and the live counts of the effect, particle-system, particle, decal and light arrays (their globals, read with `h2 disasm`, are in sim.md §6) as mean and peak. The same block is an agent method, `sim.guest_census` | `crates/extensions/game/src/bot_glue/retail_input.rs`, `crates/extensions/game/src/sim_glue.rs`, `crates/live/sim-bench/src/metrics.rs`, [sim.md](../runtime/sim.md), [agent-api.md](../runtime/agent-api.md) | none | metrics unit tests on the JSON; a fake-guest test of the census reads |
| **P0**, **P1**, **P2** the action build | Row 1: the split of the build's cost, then the alias view with its undo journal, then the small calls. The design, files and proofs are in [sim-phantom-build.md](sim-phantom-build.md#interfaces) | there | `impl.sim.phantom-alias` (proposed) | there |

**Order.** H0 measured row 1 at 1.15 ms, so its lanes run: P0 and P1
together, P2 after P0's figures. The coordinator's phantom-roster
profile (census M1 of sim-10x.md) sizes row 1 a second way, by the
kernel's remap and fault frames. Rows 2 to 12 get no lane.

## Rules and limits

- **Three consumers.** A lane's proof names all three: the arena arm of
  `h2 sim diff`, a film recorded on and off, and a frame stream compared
  field by field (`Frame` is plain data).
- **No stubbed entries.** A skip goes through a test the title already
  makes, or it replaces our own host work. None of rows 2 to 9 has such
  a test that is also exact.
- **Digest-only equality is not safe.** A skip that keeps the `gameplay`
  digest and changes the arena (row 6) changes every film check and
  needs the user's sign-off, as sim-10x.md's "Output-only work" says.
- **A film carries its switches.** A skip switch that defaults on makes
  every earlier sim film refuse to play. Nothing is published, so that
  costs nothing today; it is the reason a skip that is exact should not
  need to be a boot fact at all. `Switches::spec` holds only the
  switches a launch names, so a switch left at its default is not one.

## Evidence

| Fact | Label | Source |
|---|---|---|
| The tick's call order; the main loop's in-game branch | E | `0x137fe0`, `0x12b450` |
| Observer gate on the local-user map | E | `0x16f2c1`..`0x16f2ca` |
| Seed chooser by definition flag bit 2 | E | `0x177c20`..`0x177c40` |
| The local visibility pass skips players with no local user | E | `0x150891` |
| An effect outside its set is flagged and stopped | E | `0x1797c7`..`0x1797e3` |
| Decal placement tests an enable byte | E | `0x17e67a` |
| `0x137ed0` calls its eight updates behind a `dt` test only | E | `0x137ee2`..`0x137f48` |
| The other gates of the first table; the part switch; particle draws from word +4; the network loops | E | the addresses given; read once for this page, not rechecked |
| Two copies per tick, one revert per later seat, six guest calls per seat | Code | `crates/extensions/game/src/bot_glue/retail_input.rs` (`Builder::action`), `crates/runtime/runtime/src/executor.rs` (`snapshot`) |
| The bots timer does not cover the build | Code | `crates/live/bots/src/think.rs` (`GameThreadTimer`), started only in `think.rs` and `phantom/action.rs` |
| Copy and fault unit costs | Run | [second-executor.md](../runtime/second-executor.md#2-guest-memory-view) |
| A film's switches must equal the player's | Code | `crates/extensions/game/src/theater.rs` ("film boot facts differ") |
| Frame fields and their sources | Code | `crates/live/live-telemetry/src/memory.rs`, `hooks.rs`, `session.rs` |
| 3.3 ms a tick, bots 0.24 ms | Run | [notes/sim-10x.md](notes/sim-10x.md) |
| The build 1.15 of 5.19 ms, snapshots 0.12, one a tick, remapped; local users 0, the local set empty, the array counts | Run | [notes/sim-10x.md](notes/sim-10x.md), lab job 0060 |
| Every other share and expected ms | S | lanes P0 and P1, census M1 |

No game was run to write this page; it is static reading, so the agent
API was not used. H0's census is the one read the API lacked
(`sim.guest_census`).

## Not verified

- The cost of the live side's faults in a sim while the copies stand,
  and of a snapshot on a Linux host, which copies.
- Whether any local-class effect in the shipped tags has an object,
  damage or character part.
- Whether a voice starts with no active listener.
- The readers of the light bit, of the entity scan's masks, and the
  callers of the decal and contrail walks.
- Whether `h2 sim run` without `--upload` samples telemetry frames.

## Related

- [sim-10x.md](sim-10x.md): the levers and the budget this page refines.
- [sim-attribution.md](sim-attribution.md), [sim-budget.md](sim-budget.md): the local-roster profile and its dependency notes.
- [sim-phantom-build.md](sim-phantom-build.md): row 1's design.
- [second-executor.md](../runtime/second-executor.md): the private view.
- [bots.md](../game/bots.md): phantoms and the action build.
- [film-format.md](../game/film-format.md), [live-telemetry.md](../live/live-telemetry.md): the other two consumers.
