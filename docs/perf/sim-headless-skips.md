# Headless sim: work nobody consumes, and what may be skipped

Status: Design. Nothing here is built; every ms figure is supposed until lane H0 measures it.
Code: none of its own. Each lane below names its files. Tests: named per lane.
Switch: proposed `impl.sim.skip.phantom-snapshot` (off until measured); it joins [switches.md](../runtime/switches.md) when its lane lands.

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
copy-on-write copy of guest RAM, and that copy is remade every tick.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Addresses are research points in the retail XBE.

## How it works

### What a phantom host does not run

The local-user map is `[[0x4e8c20] + 0xc + 4 * user]`, −1 for an unmapped
user, and `[[0x4e8c20] + 8]` is the count of local users. A sim host
leaves all four profiles signed out
([sim.md §5](../runtime/sim.md#5-setting-up-a-match-by-script)), so the
map is four times −1 on the live side (**S**: read it in a run).
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
the guest, and `impl.sim.observer-resources` hooks a call the observer
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
(`0x1a4900`). The action is game state; the copies are not. By
[second-executor.md](../runtime/second-executor.md#2-guest-memory-view)
a copy costs 0.43 to 0.60 ms with 16 MB resident on a loaded Mac, and
afterwards the live side takes a fault of about 2.8 µs on its first
write to each 16 KB host page. Where the host cannot remap (`cow.rs`:
every host but macOS), a snapshot copies all RAM bytes. None of this is
inside `wall.bots_ms_per_tick`: `bots::think::GameThreadTimer` wraps the
brain only (**Code**), so it is counted as "the guest's tick" in the
[note](notes/sim-10x.md).

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
parts, no particles (**S**: the chain is read; the outcome needs a run).
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
block before `SIM_UPDATE_APPLY`, the phantom builder's results
(`BTAP`), the gameplay digest every tick and the arena hash every 30
ticks; it reads no camera, HUD, effect, particle, light or sound state
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

Shares are of the 3.3 ms Lockout tick ([note](notes/sim-10x.md)). No
profile of a phantom roster exists, so a share is the model's (sim-10x.md,
"The budget", scaled to 3.3 ms) or unmeasured. "Exact" means the arena
arm of `h2 sim diff` prints nothing.

| # | Candidate | Share | State | Films | Viewer | Mechanism | Lane | Expected ms |
|---|---|---|---|---|---|---|---|---:|
| 1 | Per-tick copy-on-write copies for the phantoms' action build | unmeasured; up to about 1 ms by the unit costs above | **safe** if the view's bytes at each call are unchanged: the action is the only output | **safe**: `BTAP` tapes the action, playback does not build | **safe** | a view kept across ticks and brought up to date from the live side's written pages, not remade | H0 measures, H1 builds | 0.2–1.0 (**S**) |
| 2 | Sound service and the audio clock under it | 0.33 + 0.11 (model) | **unsafe**: voice completion and the listener verdict are read by the tick ([sim-budget.md](sim-budget.md#headless-guest-dependencies)) | unsafe with it | safe | none: C1's read stands. One read left: whether a voice is admitted when no listener is active (`0x127d9e`) | none | 0 |
| 3 | Effect tick `0x176e40` and frame update `0x137ed0` | 0.22 (model, from a local roster); probably far less | **unsafe** as entries: object and damage parts, word 0, lifetimes. The local class is already suspended by the title (**S**) | unsafe with it | safe | none beyond the title's own. H0's census says what is left | H0 | 0 until the census |
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
| **H0** census | The sim report gains `wall.phantom_build_ms_per_tick` and `wall.phantom_snapshot_ms_per_tick` (game-thread time in `retail_input::gather` and in `SecondExecutor::snapshot`), the snapshot's `CopyKind`, and a `guest_census` block read once a tick by the sim's tick observer: the local-user count, whether the local visibility set `[0x4e6948] + 0x1178` is empty, and the live counts of the effect, particle-system, particle, decal and light arrays (their globals are the lane's first read, with `h2 disasm`) as mean and peak. The same block is an agent method, `sim.guest_census` | `crates/extensions/game/src/bot_glue/retail_input.rs`, `crates/extensions/game/src/sim_glue.rs`, `crates/live/sim-bench/src/metrics.rs`, [sim.md](../runtime/sim.md), [agent-api.md](../runtime/agent-api.md) | none | metrics unit tests on the JSON; a fake-guest test of the census reads |
| **H1** phantom view kept across ticks | Needs a spec first. `SecondExecutor` keeps its view and its base as private copies and, at each tick's first gather, copies in only the pages the live side stored to since the last one (a write-watch subscription on the live `Guest`), then reverts the view's own written pages as it does between seats. No remap per tick and no fault on the live side | `crates/runtime/runtime/src/executor.rs`, `guest/view.rs`, `cow.rs`, `crates/extensions/game/src/bot_glue/retail_input.rs`, [second-executor.md](../runtime/second-executor.md) | `impl.sim.skip.phantom-snapshot` | a runtime test: after N rounds of random live and view stores the kept view equals a fresh `cow_snapshot` byte for byte; `h2 sim diff` on against off, arena hashes on; two films of one seed, on and off, with equal `BTAP` and digest series |

**Order.** H0 first: it is two timers and a handful of reads, and every
row above waits on it. The coordinator's phantom-roster profile (census
M1 of sim-10x.md) sizes row 1 a second way, by the kernel's remap and
fault frames. H1 starts when row 1 measures at 0.15 ms or more. Rows 2
to 12 get no lane.

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
  need to be a boot fact at all, and H1's spec decides whether its
  switch enters `Switches::spec`.

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
| Every share and expected ms | S | lane H0, census M1 |

No game was run for this page; it is static reading, so the agent API
was not used. H0 adds the one read the API lacks.

## Not verified

- That the live local-user map is four times −1 and the local count 0
  during a phantom match.
- That the local visibility set is empty and local-class effects are
  suspended, and how many effects, particles, decals and lights a
  Lockout match then holds.
- The cost of the two copies and of the live side's faults in a sim, on
  macOS and on a Linux host, and whether a paged sim remaps or copies.
- Whether any local-class effect in the shipped tags has an object,
  damage or character part.
- Whether a voice starts with no active listener.
- The readers of the light bit, of the entity scan's masks, and the
  callers of the decal and contrail walks.
- Whether `h2 sim run` without `--upload` samples telemetry frames.

## Related

- [sim-10x.md](sim-10x.md): the levers and the budget this page refines.
- [sim-attribution.md](sim-attribution.md), [sim-budget.md](sim-budget.md): the local-roster profile and its dependency notes.
- [second-executor.md](../runtime/second-executor.md): the private view.
- [bots.md](../game/bots.md): phantoms and the action build.
- [film-format.md](../game/film-format.md), [live-telemetry.md](../live/live-telemetry.md): the other two consumers.
