# Simulation review: how the Halo 2 sim works and how to make it ours

Status: independent review, 2026-11-20. Nothing here is built. It was
written in three steps, in this order: read the retail image and the
Waterworks profile, write a recommendation, and only then read
[sim-rewrite.md](sim-rewrite.md), [sim-budget.md](../perf/sim-budget.md),
[sim.md](../runtime/sim.md) and the ports in `crates/engine/src/sim_native`
and `sim_physics`. Section 5 is the critique that came out of the third
step.

No game was run and nothing was built for this page. The evidence is:

- `h2 disasm` over the owner's `default.xbe` (a linear dump of `.text` in
  scratch, never committed);
- the generated C of the current image, read in scratch;
- the main-thread profile of a nine-thousand-tick Waterworks match with
  four local bots (84,358 samples at 1,000 Hz, the same capture
  sim-budget.md uses) and the A/B reports beside it;
- the repo docs named in each section.

Labels: **Code** means read from the disassembly at the address given.
**Profile** means counted from the folded stacks. **Estimate** means
arithmetic on top of those, to be checked before anyone builds on it.
Addresses are guest virtual addresses. No game data is on this page.

## 1. The short version

1. **The sim is slow because of how we translate it, not because of what
   it does.** The profile is flat: 1,982 guest functions were sampled, the
   hottest is 1.4% of the thread, the top 25 together are 13%, the top 100
   are 30% and it takes 300 to reach half. The image is 383 MB of host
   code for about 4 MB of x86. A four-bot Waterworks tick costs about
   14 ms, which is roughly what the same tick cost on the 733 MHz console
   (**Estimate**, section 3.2). A modern core has thirty to fifty times
   that headroom and the translation contract spends all of it.
2. **So porting by profile rank cannot get there.** Rewriting the 100
   hottest functions perfectly, at zero cost each, removes at most 30% of
   the tick. The 32 per-function ports (8,620 lines of Rust) are off by
   default and have no recorded timing. The current plan aims at 7.5 ms
   after five phases that end with a bit-exact structural port of Havok.
   That is a 1.9 times gain for the most expensive work on the list.
3. **Change the contract for the sim instead.** Recompile the tick's
   closure under a *sim tier*: guest registers and the x87 stack become
   SSA values, calls become native calls with recovered signatures,
   memory is one unchecked base plus offset, and there is no per-instruction
   fault, flag, precision or scheduling bookkeeping. Float results stay
   exact because the tick runs at 53-bit precision with round to nearest,
   which is plain IEEE `f64` and `f32` arithmetic. The rare cases where
   that is not true are caught by the host's sticky exception flags, and
   the tick is rolled back and re-run on today's translation. Today's
   image is the oracle, so parity is a per-tick hash comparison with
   automatic bisection, not a judgement.
4. **Keep the arena.** The checkpoint arena (one fixed-address block that
   holds all persistent sim state) is the best design decision in the
   engine. It makes snapshot, rollback, keyframes, hashing and
   multi-match hosting cheap. Rust should own the *meaning* of that state
   (one schema crate, typed views, events) and new state (an extension
   arena), not move it. The current plan's rule here is right.
5. **Hand-write Rust where it buys a feature, not where the profile is
   warm.** With the tier in place, the order of Rust rewrites is set by
   what theater, stats, Live, bots and the Halo 1 campaign need: the tick
   driver, the state schema and events, the presentation snapshot,
   animation sampling, HaloScript. Havok gets its speed from the tier and
   is never ported by hand to the exact bar.

## 2. How the Halo 2 sim works

### 2.1 The frame and the tick

The main loop iteration `0x0012b450` runs pending map changes, input, the
network pre-update `0x0008df50`, the tick budget `0x00146660`, the tick
loop `0x00137f70`, the network post-update `0x0008dfc0`, the frame-time
camera work (`0x001554b0`, `0x0016f280`), the renderer `0x00223240` and
the sound service `0x00125b40`
([engine-map.md](engine-map.md), "Frame phases").

`game_tick` `0x00137fe0` is short enough to read whole. **Code**:

1. It sets the x87 control word through the CRT (`0x003212d6`): 53-bit
   precision, round to nearest, every exception masked. It does this on
   every tick, which says other code (the renderer is the likely one)
   runs with a different word. That last part is an inference.
2. It builds the tick's *simulation update* on its stack, a `0x4048`-byte
   record. `0x000682e0` clears it and `0x000683a0` fills it from the
   players and the session. The record carries the tick number at
   `+0x4040` and the game random word at `+0x4044`; `0x000682e0` compares
   both with the live values and latches a flag in the simulation globals
   (`[0x004cf77c] + 0x2c`) when they differ. The engine ships with its
   own desync detector.
3. If the record says the tick runs, it calls 29 phases in a fixed order,
   with no arguments beyond the record:

   | Order | Entry | What (where known) |
   |---:|---|---|
   | 1 | `0x0014d3a0` | players, before |
   | 2 | `0x00138db0` | |
   | 3 | `0x001c83c0` | AI |
   | 4-5 | `0x001fb510`, `0x0020b660` | |
   | 6 | `0x0015c150` | game engine (mode rules) |
   | 7 | `0x00195ed0` | |
   | 8 | `0x00209310` | HaloScript scheduler |
   | 9 | `0x00138c10` | |
   | 10 | `0x000b6f10` | objects, before physics |
   | 11 | `0x001c3d30` | Havok step |
   | 12 | `0x000b7060` | objects, after physics |
   | 13 | `0x000b7150` | objects, final (deletions) |
   | 14 | `0x002268e0` | |
   | 15 | `0x00176e40` | effects |
   | 16 | `0x000c1400` | lights |
   | 17 | `0x0015ca10` | |
   | 18 | `0x0014d8b0` | players, after |
   | 19-25 | `0x001df500`, `0x00138e40`, `0x00138eb0`, `0x00226050`, `0x0013c680`, `0x00123430`, `0x00225730` | |
   | 26 | `0x00165dc1` | first person |
   | 27-29 | `0x00153950`, `0x001917b2`, `0x0016f200` | |

4. It runs `0x000684e0` on the record and increments the tick count at
   game time `+8`.

So a tick is a function of two things: the persistent state and one
update record. That record is what a film stores and what a peer needs.
Everything in section 4 leans on this.

### 2.2 State: one arena, salted handles

- **The arena.** `0x00123b30` takes one block of `0x3fe000` bytes at a
  fixed address and every system bump-allocates its persistent state out
  of it in a fixed order at start-up (`0x00123dd0`). A checkpoint is a
  copy of the block (`0x00123e20`), a revert is a copy back
  (`0x00123f60`), and a save file is the block plus a header
  ([checkpoints.md](../runtime/checkpoints.md)). Because the address is
  fixed, pointers inside the block survive a restore. The Havok world is
  in there too.
- **Datum arrays.** Named arrays with a fixed capacity, an occupancy
  bitmap, and a 16-bit salt per slot; a handle is `salt << 16 | index`
  ([datum_new.md](datum_new.md)). Stale handles fail the salt check
  instead of aliasing a new occupant.
- **Objects.** 2,048 twelve-byte headers (salt, flags, type, a pointer to
  the body) and a one-mebibyte pool of variable-size bodies
  (`0x000b67c0`). Behaviour is by type descriptor: each type lists its
  ancestry (object, unit, biped) and the update walks it, calling each
  level's slot (`+0x40` update, `+0x44` after physics; `0x00108c60`,
  `0x00108cd0`).
- **Tags are read in place.** Nearly every function turns a tag handle
  into a pointer by indexing the entry table at `[0x004e3b44]`; 995
  functions reference it and 1,387 reference the object table
  (engine-map.md, "Size, coupling").

### 2.3 The object passes

**Code**, `0x000b6f10`, `0x000b7060`, `0x000b7150`:

- The pre-physics pass first walks a short list of object handles kept in
  the game globals (`[0x005107f0] + 0x1d00`, count at `+0x1d80`), then
  walks every header through the occupancy bitmap. It reads the header's
  flag byte and updates an object only when it is active and connected
  and not being deleted. Objects at rest are skipped by a flag test, so
  the engine already has an activation scheme.
- `0x000bc470` is the per-object update (damage, functions, the type
  ancestry call). The post-physics pass calls `0x000bc820`, which reaches
  `0x000bd090`: animation state to node orientations to node matrices.
  The matrix composition `0x00142a60` (61 call sites) and `0x001421f0`
  are short packed-SSE leaves and the two hottest pieces of pose math in
  the profile.
- Animation is sampled per node through a codec table; the evaluation
  wrappers and the codecs are at `0x00279d80`..`0x0028d560`
  ([animation.md](../formats/animation.md)).

### 2.4 Physics and collision

- **The Havok step** `0x001c3d30` (**Code**): reads the tick length from
  game time, masks the SSE exceptions in MXCSR, calls the world step
  `0x00311100`, clears the sticky flags, restores the x87 control word,
  then runs Halo's post-step work (`0x00278f00`, `0x001c4260`). It does
  not change the rounding mode or turn on flush-to-zero, so Havok's SSE
  is IEEE single precision.
- **Havok keeps its own temporal caches**
  ([havok-runtime.md](havok-runtime.md)): sleeping islands, per-pair
  agents that remember the last query box and reuse the candidate list,
  cached separating axes and feature ids per convex pair. The structure
  is a MOPP tree, a bytecode bounding-volume tree walked by small
  recursive interpreters (`0x002ff8b0` integer box walk, `0x003009e0`
  float walk; [mopp.md](../formats/mopp.md)).
- **Halo's own collision** is separate: the vector test `0x001697c0`
  (51 direct callers: weapons, camera, AI, sound, decals), the BSP
  segment walk `0x001de630`/`0x001de6d0`, point to leaf `0x0014a280`.

### 2.5 Randomness: two streams

`[0x004e7408]` points at two words. **Code**: the word at `+0` is the one
the simulation record carries and checks; weapons, AI and game code draw
from it. The word at `+4` is drawn by particles (`0x0024....`), effects
(`0x0017....`) and render-side code. Of 215 references in `.text`, the
effects and particle neighbourhoods use `+4` almost exclusively. The
engine already separates "random that must agree between peers" from
"random that only changes how sparks look".

### 2.6 Floating point, counted

A census of the linear disassembly (**Code**, counts are of instructions
in `.text`, including a little data decoded as code):

| | Count |
|---|---:|
| Instructions with an XMM operand | 125,723 |
| x87 instructions | 38,686 |
| `fsqrt` | 551 |
| `fsin`, `fcos`, `fptan`, `fpatan` | 244, 267, 19, 89 |
| `fyl2x`, `f2xm1`, `fscale` | 47, 23, 29 |
| `rsqrtss`, `rsqrtps`, `rcpss` | 118, 14, 9 |
| `fldcw`, `ldmxcsr` | 10, 23 |
| `minps`, `maxps` | 48, 41 |

Three things follow:

- Most float work is scalar SSE (`movss`, `mulss`, `addss`), which is
  IEEE `f32` on any host.
- The x87 work runs at 53-bit precision, so add, subtract, multiply,
  divide and square root are IEEE `f64` operations, and a store to a
  `float` is the usual second rounding. The exceptions are values that
  leave the `f64` exponent range (x87 keeps a wider exponent), 64-bit
  constants such as `fldpi`, and the transcendentals, which need the
  `x87` crate's exact routines.
- The reciprocal estimates are 141 sites and need the table model the
  runtime already has.

### 2.7 AI, scripts, networking

- `ai_update` `0x001c83c0` and the script scheduler `0x00209310` are
  phases 3 and 8. In the multiplayer profile they are 0.06% of guest
  time, so nothing here measures them. Scripts are cooperative threads
  that wake on a tick number ([hs_vm.md](hs_vm.md)).
- The simulation layer (`0x00067e30`..`0x000684e0`) wraps the tick. In
  multiplayer the host is authoritative and replicates state; campaign
  co-op runs the same tick on every peer from the same records. The
  sim-rewrite plan keeps replication in the guest, and so does this
  review.

### 2.8 How the renderer reads the sim

Directly. The render walk reads object headers, bodies and the node
matrices in guest memory, and it also *writes* tick-visible state:
particle visibility stamps, the effects random word, animation cache use
stamps, visit marks ([pipelined-render.md](../graphics/pipelined-render.md),
"Tick ordering and hash parity"). There is no presentation copy in the
retail design; render and tick alternate on one thread.

### 2.9 What is good and should be kept

- One fixed-address arena for all persistent state.
- Salted handles everywhere instead of pointers.
- A tick that is a pure function of the arena and one record, with a
  fixed phase order and a built-in consistency check.
- Two random streams.
- Activation flags on objects and sleeping islands in Havok.
- Fixed 30 Hz step and 53-bit precision set at the top of the tick, which
  is what makes exact cross-architecture results possible at all.

## 3. What is slow and why

### 3.1 The measurements

**Profile** (Waterworks, four local bots, 9,000 ticks, unpaced, no
rendering; 13.6 ms a tick in the profiled run, 14.12 unprofiled):

| | Share of the thread |
|---|---:|
| Samples whose innermost guest frame is generated code or a service it called | 78.8% |
| of which: inside generated code itself | 70.8% |
| Our bot code (`bots::Brain`, line-of-sight walls, perception) | 6.9% |
| Runner and dispatch (`Runner::run_from`, `handler::execute` own time) | about 11% |
| DirectSound clock, input sync, kernel file calls | about 4% |

How concentrated the guest time is:

| Hottest N guest functions | Share of the thread | Share of guest time |
|---:|---:|---:|
| 10 | 7.3% | 9.2% |
| 25 | 13.3% | 16.9% |
| 100 | 30.0% | 38.1% |
| 300 | 51.3% | 65.1% |
| 1,000 | 73.5% | 93.3% |
| 1,982 (all sampled) | 78.8% | 100% |

By address neighbourhood (innermost guest frame, services included):
objects, units and weapons 17.9%; Havok and the CRT 11.4%; collision,
effects and data arrays (`0x16b000`..`0x1a6000`) 10.0%; physics glue and
animation management 8.5%; animation codecs and particles 7.6%; math
6.2%; sound 2.9% plus its share of the scenario neighbourhood.

The services generated code calls most: the slow memory path 4.8%, x87
helpers about 4.5%, direct-call entry and return 2.9%, the SSE guard and
lanes about 3%, exit, charge, budget and gate about 2.9%, dirty marking
0.7%.

### 3.2 The translation contract is the cost

Reading the generated C for a hot leaf next to its disassembly
(**Code**, scratch only) shows what one guest instruction becomes:

- Every load and store is a macro that carries the instruction's address
  so that a fault can exit with the exact architectural state.
- Every SSE arithmetic instruction samples the host float mode, checks
  each operand's range, and can raise; packed registers live as 128-bit
  integers in a CPU structure.
- Every x87 instruction works on an 80-bit register file with tags, the
  stack top, the precision-exception flag and the rounding-direction bit
  kept correct ([x87-native.md](../lifter/x87-native.md)).
- Flags are kept as deferred records and materialised by per-function
  helper switches.
- Functions take the CPU structure and an entry selector, begin with a
  gate, count their accesses for the scheduler, and return a transfer
  record to a runner or a direct-call protocol.
- Stores mark dirty bits.

The result is 382.7 MB of `__text` for 3.57 MB of `.text` plus the
libraries: about ninety times. No sampled function shows the
instruction-cache and branch-predictor cost of that, but every function
pays it.

**Estimate.** The console ran this tick, the renderer and audio in
33 ms on a 733 MHz Pentium III. If the sim was a third of that, a tick
was about 10 ms of 733 MHz time. A current Apple core does thirty to
fifty times the work per second. So the same tick compiled natively
would cost 0.2 to 0.4 ms, and we measure about 10.7 ms in guest code and
its services. The translated sim runs at about the speed of the original
hardware. Whatever the precise ratio, the size of the gap is not in
doubt, and it is the whole opportunity.

The work in [architecture-review.md](../perf/architecture-review.md) and
[guest-memory.md](../perf/guest-memory.md) (inline loads, register homes,
direct calls, gate at entry, inline charge, guarded native float) has
largely landed. The tick is still about 14 ms. Polishing inside the
contract has reached its plateau.

### 3.3 Hand ports inherit the contract

`crates/engine/src/sim_native/pose.rs` is representative. It reads its
arguments from `CpuAbi`, performs each SSE operation through `op` and
`vector` so guest MXCSR flags come out right, and writes the dead values
the original left in `xmm0`..`xmm7` back into the CPU structure. It is a
correct port of the *machine*, which is what its differential test
demands (all memory, and every register outside the clobber list). It
cannot be much faster than the code it replaces, because it does the
same modelled operations through calls instead of inline. The 32
switches default off (`crates/feature/src/sim_native.rs`) and no doc
records a timing for them. The one A/B on file, Waterworks 14.96 to
14.12 ms a tick and Lockout 9.07 to 8.49, measures commit `36b417561`,
a change to our own bot navigation code, not a guest port. That is
worth noting on its own: the only measured tick gain so far came from
our Rust, not the guest's.

### 3.4 Work in the tick that is not the sim

In a match with no renderer and no speakers the profile still shows:

- **Our own Rust**, 6.9%: `bots::los::Walls::clear` alone is 2.1%,
  perception 1.7%, with `Vec::retain` and seat-list collection visible.
  This is the cheapest millisecond on the page.
- **Sound**: about 6% of guest time (sim-budget.md's census) plus 1.8%
  in the DirectSound clock. Whether any of its state is tick-visible is
  established for the listener and voice completion (sim-budget.md,
  "Headless guest dependencies") and should be re-tested against the
  arena hash once phases can be timed and skipped one at a time.
- **Camera and first person**: 4.8% of guest time, kept because the
  effects stream and visit marks depend on them.
- **Host services per tick**: controller sync 0.9%, kernel file calls
  0.7% (with a path string rebuilt per read in `vfs::host::record_path`).

### 3.5 What is inefficient in Halo 2's own design

Real, but second order once the code runs natively:

- Per-node codec dispatch in animation, through a table, one node at a
  time, decoding compressed keys on every sample.
- The MOPP tree is interpreted bytecode, recursive, one node per call.
- Object behaviour is a walk up a type ancestry with an indirect call
  per level, per object, per pass.
- A tag handle is resolved to a pointer again in almost every function.
- Spatial queries mark and clear per-object visit stamps.
- Bodies are arrays of structures in a variable-size pool. At 2,048
  objects and one mebibyte this fits in the second-level cache, so it
  matters far less than it would at modern scale.

## 4. Recommended approach

### 4.1 Principle: separate "fast" from "owned"

The current plan asks one piece of work, a hand-written Rust subsystem,
to deliver speed, parity and ownership at once. Those have different
best tools:

- **Speed and parity** are a property of translation. Solve them once,
  mechanically, for the whole closure of the tick.
- **Ownership** is a property of interfaces and of who writes new state.
  Solve it with a schema and an extension arena now, and with
  hand-written subsystems where a feature needs one.

### 4.2 The sim tier

A second way to compile the same lifted IR, used for the functions the
tick reaches. The existing image stays as the *reference tier*.

**Contract.**

- **Signatures.** A per-function analysis recovers which registers are
  live in and out, the stack arguments and who pops them, and the x87
  stack depth at entry and exit. The link-time-optimised image uses
  custom conventions, so this is a dataflow pass, not a pattern match.
  Functions it cannot settle stay on the reference tier.
- **Values.** Registers, flags, XMM registers and the x87 stack are
  locals. The backend deletes dead flags and dead register writes. The
  x87 stack depth is static in compiled code, so it becomes named `f64`
  values.
- **Calls.** Direct guest calls are native calls. Indirect calls go
  through a table from guest entry address to native function. A thunk
  at each tier boundary moves values between locals and `CpuAbi`.
- **Memory.** Guest RAM is one contiguous host mapping; an access is base
  plus a 32-bit offset with no check. The base is a parameter, not a
  global (section 4.5 uses that). A host fault is treated like any other
  rare event below.
- **No bookkeeping.** No instruction addresses, no access counting, no
  gate, no dirty marking. The tick runs to completion as one scheduler
  slice. A blocking kernel call inside the tick (a cache read that
  yields, `0x00122d60`) is a rare event.
- **Floats.** `f32` and `f64` add, subtract, multiply, divide and square
  root, compiled without contraction. Transcendentals and reciprocal
  estimates call the existing exact models. `minps`/`maxps` are written
  as the x86 select, not the host's min.

**Rare events: run optimistically, verify with sticky flags, roll back.**

The host's cumulative exception flags (FPSR on arm64, MXCSR on x86-64)
are cleared at the start of the tick and read once at the end. If
invalid, denormal-input, overflow or underflow is set, then somewhere in
the tick a result could differ from the retail one: a NaN was produced
(the default NaN differs between architectures), an `f64` result went
subnormal where x87's wider exponent would not have, or a value
overflowed. Inexact is expected and ignored. In that case, and for a
host fault, an unmodelled control-word change (the 10 `fldcw` and 23
`ldmxcsr` sites need classifying; most are the CRT's float-to-int) or a
blocking call, the tick is abandoned: the arena is restored from the
copy taken at tick start, and the tick is re-run on the reference tier.

That is affordable because of section 2.2: taking the copy is a 4 MiB
`memcpy` (well under a millisecond; a dirty-page scheme can cut it
further), and state outside the arena that a tick can write has to be
enumerated once and included. That enumeration is the main unknown in
this design and is the first thing step 3 in section 6 has to settle.

What this buys: no per-operation guards at all, and exactness that does
not depend on proving ranges. The fast path either produced the retail
bits or announced that it might not have.

**Proof.**

- The reference tier is the oracle. Run the same film on both and
  compare the arena hash after every tick (`lockstep::hash_sim` exists).
- With the tick driver in Rust (section 4.4) the hash can be taken after
  each of the 29 phases, which names the diverging phase in one run.
- Tier choice is per function at build time, so a bisect over the
  function list finds the diverging function without a person. That is a
  test-harness manifest, not a user switch.
- Cross-architecture: the same film on arm64 and x86-64 must give the
  same hash series on the sim tier, and each must equal the reference
  tier.

**Expected gain** (**Estimate**): five to fifteen times on guest code,
so a Waterworks tick of 1.5 to 3 ms before any hand-written Rust, and a
code size of a few times the x86 instead of ninety. Step 2 in section 6
is a one-week experiment that replaces this estimate with a number
before the infrastructure is built.

**What it is not.** It is not readable Rust and it owns no state. It is
the thing that makes the frame rate and host density goals independent
of how fast people can port functions.

### 4.3 State ownership

Agree with sim-rewrite.md's option (c) and its one rule: guest-layout
memory is the persistent truth at every tick boundary; Rust state is
static, derived, transient or extension. Additions:

- **One schema crate** (`sim-state`), with the layouts written once as
  typed zero-copy views (object header, body by type, player, actor,
  game time, the simulation record). Bots, telemetry, the agent API,
  theater and stats read through it. Today each has its own offsets and
  its own `GuestMemory::read` calls (1.4% of the thread in this
  profile).
- **Events at the source.** Kill, damage, spawn, pickup and score are
  emitted from hooks on the guest functions that apply them now, and
  from the Rust subsystem when one exists. Consumers never scrape.
- **Do not build gather and scatter machinery ahead of need.** At 2,048
  objects the structure-of-arrays working set is a wash against copying
  in and out. Add it for a specific kernel that measures better with it.

### 4.4 Rust owns the tick driver first

`game_tick` is 29 calls. Replace its body with Rust that calls the same
29 guest entries in the same order. It changes nothing in the arena and
gives, for a day or two of work:

- inclusive time per phase, which the profile cannot give (stacks show
  one guest frame) and the static census cannot either (sim-budget.md
  leaves 37% unreached and 14% ambiguous);
- a hash per phase for the proof ladder;
- the place where a host skips or reorders a phase once a hash test says
  it may;
- the seam for the sim thread and the presentation snapshot.

### 4.5 Threading

- **Not inside the tick.** At one to three milliseconds a tick, a job
  pool inside it adds determinism risk for no visible gain. Keep the
  fixed-order rules from sim-rewrite.md for when a batch kernel does
  want lanes, and stop there.
- **Tick against render.** The tick runs on its own thread; at the end
  it publishes a presentation snapshot (a copy-on-write copy of the
  arena, or an extracted frame state: transforms, node matrices, effect
  and light lists, HUD fields). The renderer interpolates between the
  last two. The pipelined renderer already takes copy-on-write views;
  the snapshot makes that the only interface, and the render-side writes
  listed in section 2.8 become explicit messages back to the tick.
- **Many matches in one host process.** With the guest base as a
  parameter, one process can hold many arenas and share the code and the
  read-only map data. sim.md budgets about 1 GiB of resident memory per
  match process today; most of that is shareable.

### 4.6 Hand-written Rust, ordered by what it unlocks

| Order | Subsystem | Why this one |
|---:|---|---|
| 1 | Tick driver, `sim-state` schema, events | Everything else stands on them; no parity risk |
| 2 | Presentation snapshot and interpolation inputs | Locked frame rate at 4x needs render off the tick's thread |
| 3 | Keyframes and rollback (arena copies in films, `KEYF`) | Theater seek; the rollback the sim tier needs anyway |
| 4 | Animation sampling and a clip store | Theater, the web viewer and Halo 1 models all want poses without the guest. The phase 1a design in sim-rewrite.md is good; build it as that library first |
| 5 | HaloScript VM (`crates/hs`, already written) | Halo 1 campaign scripts; campaign census decides its urgency |
| 6 | Collision world and queries | Bots and Halo 1 need queries from Rust; the MOPP and BSP readers exist |
| never by hand at the exact bar | Havok | The tier gives it its speed with no semantic risk. Replace it only if the user later relaxes the bar |

Each of these takes over guest entries only when the per-tick hash
against the reference tier is clean, as the plan's ladder says.

**Assisted ports.** The sim tier's recovered signatures and typed
values are most of a decompilation. Emitting a Rust skeleton per
function from them, then refactoring it under an equivalence check
against the tier on recorded calls, is a far cheaper route to a
readable Rust subsystem than reading disassembly and rewriting. The
recorded calls are game state and stay in local scratch.

### 4.7 Other ideas, in order of expected value

1. **Rollback for co-op.** Once a tick is a millisecond or two and the
   arena restores in well under one, re-simulating several ticks inside
   a frame is affordable. Campaign co-op could predict remote input and
   correct, instead of waiting for it. This is a feature the retail
   design could not afford and ours can.
2. **Per-phase skipping on hosts**, each one admitted by a hash test:
   candidates are first person, camera, sound and effects that draw only
   from the effects stream. Section 2.5 says the engine drew that line
   itself; sim-budget.md's dependency notes say where it leaks.
3. **Ahead-of-time, per map**: the MOPP program flattened to a native
   tree with the same visit order, decoded animation clips, flattened
   collision BSP; content-addressed in the user cache and mapped
   read-only by every match (sim-rewrite.md §7 has this right).
4. **Memoising pure work across ticks.** Only where a counter shows
   inputs repeat, and only with a key that covers every input read:
   static-light already does this; candidates are a pose whose channel
   state did not change, and line-of-sight rays against static geometry
   between two points that did not move. The plan's cache-cold test arm
   is the right guard. After the tier this is about host density, not
   frame rate.
5. **SIMD across items**: node matrices in hierarchy waves, particle
   aging. Exact as long as each lane keeps the scalar operation order.
   Small after the tier.
6. **A sim-only image for hosts**: link the tick's closure and nothing
   else. Start-up and memory, not tick time.

## 5. Critique of the current plan

### 5.1 What it gets right

- The state rule: arena as truth, four kinds of Rust state, epoch bumps
  on revert and restore, and a test that cold caches equal warm ones.
- The float rules, fixed-order reductions and random draws in retail
  order.
- The verification ladder, especially run parity on arena hashes, the
  cache-cold arm and the cross-architecture step.
- Keeping replication in the guest and not running guest threads in
  parallel.
- The `sim-state` surface for theater, film, stats and Live, and the
  extension arena for new gameplay.
- Static caches shared between match processes.
- Honesty about its census: "lower bounds", 37% unreached.
- The 1a animation design is concrete and well bounded.

### 5.2 What it gets wrong or misses

1. **The target is an order of magnitude too modest.** 7.5 ms a tick is
   1.9 times. The gap to native is thirty to fifty times. A plan that
   ends at 7.5 ms has accepted the translation overhead as fixed for
   everything it does not rewrite, which will always be most of the
   image.
2. **It does not account for the flat profile.** The wins column assumes
   each family runs four times faster once rewritten and sums to roughly
   4.3 to 6.6 ms. The named top rows it rests on total 12.4% of the
   thread. No port has a measured gain yet: the 32 existing ones are off
   by default and untimed, so the four-times figure is an assumption.
3. **Bar E is over-specified at the function level.** The differential
   step compares all memory and all registers outside a clobber list,
   and fault ordering. None of that is observable at a tick boundary.
   The ports therefore reproduce dead register writes and flag state,
   which is why they are slow to write and slow to run. Parity should be
   defined where it is observed: the arena and extension blob after each
   phase.
4. **Phase 4 is the wrong way to make Havok fast.** A structural port of
   a closed-source physics library to the exact bar is the largest and
   riskiest item, and the user has since ruled that physics stays exact,
   which removes the relaxed bar that made it attractive. The sim tier
   gives Havok the same multiple as everything else, by construction.
5. **The order follows profile share.** With the tier, share stops
   mattering and feature value should set the order (section 4.6). The
   plan's own first phase, animation, is first on both lists, which is
   why 1a survives.
6. **Gather, compute, scatter is assumed to pay.** It is not measured,
   and at this object count it likely does not (section 4.3).
7. **Threads inside the tick** (`sim-core::Jobs`) solve a problem the
   tier removes, at a cost in determinism testing. The threading that
   matters, tick against render and matches per process, is not in the
   plan.
8. **It builds on a census that cannot see half the time.** Phase 0
   proposes a deeper attribution of "Unreached" by more static work. The
   tick driver gives inclusive phase times directly.
9. **Our own code in the tick is not on the list.** 6.9% in bot code and
   about 4% in host services are cheaper than any guest port.
10. **It does not use the arena for anything new.** Rollback, optimistic
    execution, in-film keyframes as plain copies and multi-match
    processes all fall out of the design it already commits to keeping.

### 5.3 What I would change

- Keep sections 2, 3 (float rules and ladder), 7 and 8 of
  sim-rewrite.md as they are.
- Replace the roadmap's spine: tick driver and our own hot code first,
  then the sim-tier experiment, then the tier, with phase 1a proceeding
  in parallel as a library.
- Redefine bar E at phase boundaries and drop register comparison from
  the gate for anything above a leaf.
- Delete phase 4 as written. Havok is covered by the tier.
- Move the campaign census (phase 0) onto the tick driver's phase
  timers.
- Set the Waterworks goal at 3 ms a tick after the tier and 1 ms as the
  stretch, and derive host density from that.

## 6. Next steps

Lane-sized, in order. Lanes write code; the coordinator builds and
measures. No lane adds a switch: steps 1 and 4 are unconditional once
their hashes match, and tier choice is a build manifest.

| # | Lane | Deliverable | Gate |
|---:|---|---|---|
| 1 | **Tick driver** | `game_tick`'s body in Rust calling the 29 guest phases; per-phase inclusive timers in the sim report; per-phase arena hash under the existing arena-hash switch | Arena hash series identical to today on Lockout and Waterworks; a phase table for Waterworks and one co-op campaign level replaces the census |
| 2 | **Our code in the tick** | `bots::los::Walls::clear` and perception allocations, `vfs::host::record_path`, per-tick input sync on hosts | Same gameplay digests; about 1 ms a tick back (**Estimate**) |
| 3 | **Sim-tier experiment** (one week, no game run) | For four leaf families that already have ports to compare with (`0x00142a60`/`0x001421f0`, `0x002ff8b0`, `0x001de6d0`, the solver leaf `0x0031c940`): emit from the existing IR under the section 4.2 contract; bench nanoseconds per call against the reference tier and the hand port in the differential harness; list every write a tick can make outside the arena | At least five times over the reference tier, bit-identical memory effects. If it fails, this review's main recommendation fails with it and the current plan stands |
| 4 | **Sim-tier infrastructure** | Signature recovery pass; unchecked contiguous memory with a base parameter; tier-boundary thunks; sticky-flag check; tick-start arena copy and reference re-run; the bisect tool | A leaf-only tier build passes the film corpus on both architectures; forced fallbacks (injected NaN, injected fault) reproduce the reference hash |
| 5 | **Tier roll-out** | Closure by neighbourhood: math and collision leaves, animation codecs, Havok, objects and units, players and weapons, effects | Per neighbourhood: hash-clean corpus, fallback rate reported per thousand ticks, ms a tick from three medians |
| 6 | **`sim-state` schema and events** (parallel with 3 to 5) | Typed views, `SimRead`, event hooks; bots, telemetry and the agent API moved onto it | No raw offsets left in those crates; same reports |
| 7 | **Presentation snapshot** | Tick on its own thread; snapshot at tick end; the renderer and interpolation read only the snapshot; render-side writes returned as messages | Arena hashes unchanged with rendering on and off; frame time independent of tick time in a capture |
| 8 | **Keyframes in films** | `KEYF` as arena plus extension copies; seek by keyframe and replay | Seek in a ten-minute film within a second; replay hash-clean from every keyframe |
| 9 | **Animation library** (sim-rewrite.md phase 1a as designed) | `sim-core`, `sim-anim`, the clip store | The plan's own ladder; theater poses without the guest |
| 10 | **Rollback experiment for co-op** | Predict remote input, re-simulate on correction, on top of 4 and 8 | Hash-identical end state against lockstep on a recorded session |
| 11 | **Host density** | Many arenas per process; shared map data; sim-only image | Matches per core and resident memory per match, measured |
| 12 | **Per-map caches and memo counters** | Flattened MOPP and BSP, repeat-rate counters for pose and ray memo candidates | Cache-cold arm identical; build a memo only where the counter says so |

## 7. What this review does not establish

- **The tier's multiple.** Five to fifteen times is an estimate from the
  shape of the generated code and the console comparison. Step 3
  measures it.
- **State a tick writes outside the arena.** Rollback needs the full
  list (resource caches, sound records, render-side stamps). Step 3
  produces it; if it is large or unbounded, fallback needs a different
  mechanism (for example, checkpointing with write-protected pages).
- **Fallback frequency.** Denormals and NaNs may be common in some
  system (a physics solver warm-starting from tiny impulses is the
  obvious suspect). If a tick falls back often, that family needs
  targeted exact handling instead of rollback.
- **Signature recovery coverage** on a link-time-optimised image.
  Unrecovered functions stay on the reference tier behind thunks; how
  many and how hot is unknown.
- **Per-phase costs.** This review could not attribute time to phases:
  profile stacks carry one guest frame and static reachability collapses
  into one component. Step 1 fixes that.
- **Campaign.** Everything measured here is multiplayer with no actors
  and no scripts.
- **The control word outside the tick.** That the renderer runs at a
  different precision is inferred from the tick resetting it.
