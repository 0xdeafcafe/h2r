# Object update: entries, conventions and liveness

Status: Partly built. The dispatcher, the body table and the installer exist with no bodies; slices 1 to 13 of [sim-rewrite-objects.md](sim-rewrite-objects.md) add them.
Code: `crates/rewrites/sim-objects` (kernels and typed views, no `runtime` dependency), `crates/rewrites/engine/src/sim_objects` (the guest boundary: `BODIES`, `drive`, `supported`, retail frame helpers), `crates/rewrites/sim-bridge/src/sim_objects.rs` (installer). Tests: `crates/rewrites/engine/src/sim_objects/tests.rs` (the dispatcher), `crates/rewrites/sim-bridge/src/sim_objects.rs` (the installer), `crates/rewrites/engine/tests/sim_objects_differential.rs` (fused transcripts and mixed frames against the lifted original, feature `differential`), and the views in `crates/rewrites/sim-objects` and `crates/rewrites/sim-state`.
Switch: `impl.sim.native.objects` (default off), listed in [switches.md](../runtime/switches.md).

The contract page for phase 3 of the simulation rewrite: the object walks,
the per-object pre- and post-update, the type dispatchers, function values,
change colours, the post-update shell and lights. The design, its decisions
and its slices are in [sim-rewrite-objects.md](sim-rewrite-objects.md); this
page records what each entry and callout takes and returns, what the
static reading resolved, and which registers each call site relies on. The
rule every body keeps is the callout contract in that page's section 5: at
every call out and every return, `esp`, the frame slots read later and all
eight integer registers equal retail's.

## Evidence labels

This page uses the sim-rewrite scheme: **C** confirmed (read from the
retail image with `h2 disasm`, at the address given, or encoded in a
differential-tested port), **L** likely, **O** open. Addresses are guest
virtual addresses; bare numbers in tables are hex. Names are ours.

## Code paths

- `engine::sim_objects::BODIES` lists every body: its point, its entry,
  its retail return sites. Lanes append one row block per slice.
- `engine::sim_objects::drive` is the one local dispatcher: a call or
  return between two enabled bodies stays in Rust; anything else returns a
  `TransferAbi` to the runtime.
- `sim_bridge::sim_objects` registers each body's entry and sites under
  `impl.sim.native.objects`. Where the float environment check fails, the
  original runs from the same frame. A per-function port at a point a body
  owns (`sim_physics::PORTS`, `sim_native::ENTRIES`) stands down and stays
  a test oracle (`sim_bridge::sim_objects::owns`).

## Entries

"ABI" is the convention at entry and exit: argument registers, stack
arguments (`stack n` is `[esp + 4 + 4n]` at entry), the result, and the
bytes `ret` pops. All are **C**, read at the entry address and its `ret`
instructions, and cross-checked by the liveness scan below, unless marked.
A register a function leaves unwritten is still part of the contract
(rule 2): the "Reaches `ret` with unwritten" column of the liveness table
lists them.

### Slice 1: walks and dispatchers

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `b6f10` | `OBJECTS_PRE_PHYSICS` | no arguments; `ret` | segment |
| `b7060` | `OBJECTS_POST_PHYSICS` | no arguments; `ret` | segment |
| `b7150` | `OBJECTS_FINALIZE` | no arguments; restores its saved registers, then `jmp bf380` | segment |
| `bc470` | `OBJECT_PRE_UPDATE` | `(object: stack 0) -> al`; `ret 4` | segment |
| `bc820` | `OBJECT_POST_UPDATE` | `(object: stack 0)`; `ret 4` | segment |
| `108c60` | none yet | `(object: stack 0) -> al`, the OR of every slot result; `ret 4`. Slot `+40`, called `(object)` stdcall | segment (indirect callout) |
| `108cd0` | none yet | `(object: edi)`; `ret`. Slot `+44`, called `(object)`. `eax` at return is the address of the last ancestry entry, not a result | segment (indirect callout) |
| `109050` | none yet | `(object: ebx, stack 0, stack 1, stack 2)`; `ret 0c`. Slot `+6c`, called `(object, s0, s1, s2)` | segment (indirect callout) |
| `1090d0` | none yet | `(object: ebx, matrix: stack 0)`; `ret 4`. Slot `+70`, called `(object, matrix)` | segment (indirect callout) |
| `b7360` | none yet | `(object: eax)`; `ret` | leaf |
| `baf80` | none yet | `(object: ecx) -> eax` root ancestor (none in, none out); `ret`; clobbers `ecx`, `edx` | leaf |
| `bba20` | none yet | `(object: eax)`; `ret` | leaf |
| `bc7b0` | none yet | `(object: stack 0)`; `ret 4` | leaf |
| `109580` | none yet | `(mover: stack 0)`; `ret 4`. A `1004`-byte frame made through the stack probe `320560` (`eax` = size) | segment until phase 2's `bb050` is callable from Rust |

### Slice 3: functions and colours

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `bab40` | `OBJECT_FUNCTION_VALUE` | `(object: stack 0, name: stack 1, value: stack 2) -> al`; `ret 0c` | segment, then leaf |
| `ba8c0` | none yet | `(record: eax, object: stack 0, value out: stack 1) -> al`; `ret 8`; stores the `f32` at `[stack 1]` on every path. Argument names L | segment, then leaf |
| `108d90` | `OBJECT_FUNCTION_IMPORT` | `(object: stack 0, name: stack 1, value: ebx, active: edi) -> al`; `ret 8`. Stores 0 to `[ebx]` and `[edi]` first, then calls slot `+4c` per ancestry level as `(object, name, value, active)` until one returns non-zero. Clobbers `xmm0` | segment (indirect callout) |
| `bcc20` | `BASE_OBJECT_IMPORT` | `(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al`; `ret 10` | segment, then leaf |
| `c6b90`, `de3b0`, `f6d10`, `ff5f0`, `fbe70`, `106d70`, `11bd60` | none yet | the slot `+4c` shape: `(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al`; `ret 10`; no register arguments | segment or leaf |
| `be8e0` | `OBJECT_CHANGE_COLORS` | `(object: stack 0) -> al`; `ret 4` | segment, then leaf |
| `13bb90` | none yet | `(function: eax, stack 0, stack 1)`, input in `xmm0` (clamped), result in `xmm0`; `ret 8`. Calls the evaluator `13b390` | callout |

### Slice 4: post-update shell

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `bdef0` | none yet | `(object: stack 0) -> al`; `ret 4` | segment |
| `bef30` | none yet | `(object: stack 0, stack 1, stack 2, stack 3, stack 4)`; `ret 14`. `bc820` passes `(object, 1, 1, 0, 0)` | segment |
| `be1d0` | none yet | `(object: eax)`; `ret`. Calls `b8c00`, then `109140` (below) | segment |
| `bc5e0` | none yet | `(object: stack 0)`; `ret 4` | segment |
| `bd090` | `OBJECT_NODE_MATRICES` | `(object: stack 0)`; `ret 4`. Calls `1090d0` after the root node matrix (`bd507`) | segment; shared with phase 1b |
| `b8c00` | none yet | `(object: eax, count out: stack 0) -> eax` the node matrices (`body + [body+116]`); stores the node count (`[body+114]` ÷ `34`) at `[stack 0]`; `ret 4` | leaf |
| `109140` | none yet | `(object: ebx, count: stack 0, matrices: stack 1)`; `ret 8`. Slot `+74`, called `(object, count, matrices)` | segment (indirect callout) |
| `b7430` | none yet | nine stack arguments; `ret 24` | segment |
| `b7740` | none yet | `eax`, `ecx`, `edi` and one stack argument; `ret 4` | segment |
| `b9dd0`, `b9fc0` | none yet | `eax`, `esi` (`b9fc0` also `edi`); `ret` | segment |
| `ba1d0` | none yet | `eax`, `ecx` and one stack argument; `ret 4` | segment |

### Slice 5: lights

| Entry | Point | ABI | Kind |
|---|---|---|---|
| `c1400` | none yet | no arguments; `ret` | segment |
| `c0c80` | `LIGHT_RECORD_UPDATE` | `(light: stack 0)`; `ret 4` | segment |
| `c0b00` | none yet | `(state: eax, functions: ecx, stack 0..3)`; `ret 10`. `c0c80` passes the light datum's `+d4` block in `eax` and the definition's `+c0` block in `ecx`. Names L | segment |
| `c2d00` | none yet | `(stack 0)`; `ret 4` | segment |
| `c3220` | none yet | `(light: eax)`, light array `[4e030c]`, stride `110`; `ret` | segment |
| `c3260` | none yet | `(stack 0, stack 1)`; `ret 8` | segment |

Slices 6 to 11 (unit, biped, damage, weapon, vehicle, small types) add
one block each here, in the same shape.

### Callouts that stay guest in this phase

| Callout | Point | ABI |
|---|---|---|
| `b7b40` object_new | none yet | `(stack 0)`; `ret 4` |
| `108a90` new dispatch | none yet | `(object: ebx, stack 0, stack 1) -> al`, the AND of the slot results, stopping at the first zero; `ret 8`. Slot `+2c`, called `(object, s0, s1)`. Called from `b7b40` at `b8166` |
| `b83b0`, `b8460` delete | none yet | `(object: stack 0, stack 1)`; `ret 8`. `b83b0` calls `OBJECT_DELETE` `b8540`, then eight function pointers from `468664`, each `(object)` |
| `b8540` | `OBJECT_DELETE` | `(object: stack 0)`; `ret 4` |
| `fd910` | none yet | no arguments; `ret` |
| `bf380` | none yet | no arguments; `ret` |
| `109660` | none yet | `(index: ecx, mover: eax)`; `ret` |
| `a9570` | none yet | `(object: eax)`; `ret` |
| `1cd4e0` | none yet | `(animation block: esi) -> al`; `ret` |
| `1ced00` | none yet | `(component record: edi)`; `ret` |
| `1d3920`, `1d44f0` | none yet | three stack arguments; `ret 0c` |
| `bb050` | `OBJECT_SPATIAL_VISIT` | seven stack arguments; `ret 1c` |
| `b87b0` | none yet | `(object: stack 0)`; `ret 4` |
| `b8600` | none yet | `(object: stack 0, stack 1)`; `ret 8` |
| `d5de0` | `OBJECT_DAMAGE_UPDATE` | `(object: stack 0) -> al`; `ret 4` |
| `baeb0` | none yet | `(iterator: stack 0) -> eax` the next body or 0; `ret 4` |
| `fd980` | none yet | `(weapon: stack 0)`; `ret 4` |
| `320560` | none yet | stack probe: `(size: eax)` |

## Resolved in slice 0

Each item of section 13 "Not resolved" of the design page.

**DEFER (`20`) setter.** C. The only store found is in the attach function
`b93b0` `(parent: stack 0, child: stack 1, node: stack 2)`, `ret 0c`: it
links the child under the parent (child body `+14`, `+18`, parent `+10`,
child `+0c`), sets CHILD (`b9532`), and after the transform and attachment
refresh sets DEFER on the child's header (`b97df`, stored at `b97e4`). So an
object attached during a tick is skipped by `bc470` (returns 1) for the
rest of that tick's walks, and finalize clears DEFER. Method: every
`or byte ptr [r+2],20h` and `or r8,20h` followed by a flags store in
`.text`; the other hits address other arrays (`b465e` on `[4cf78c]`,
`6c067` and the `6c...` run, `21fd33` on a `+3` byte).

**NEW (`08`) at creation.** C. `object_new` `b7b40` sets NEW on the new
header (`b7c07`, stored at `b7c0e`) right after taking the slot, before it
stores the type at `+3`. `109580` sets it on woken weapon, equipment,
garbage and projectile objects (`109638`).

**The list at `[5107f0]+1d00`.** C: early movers. Appended by `109390`
`(object: edx)` from `object_new` (`b8332`) when the definition's flags
byte `+2` has bit 6 set and the count is below 32; it sets body `+c0`
bit 3. Removed by `109400` `(object: stack 0)`, `ret 4`, from
`OBJECT_DELETE` (`b85d9`): swap with the last entry, clear `+c0` bit 3,
then every live object woken by this mover (body `+b8` equal, `+c0` bit 1
or 2) is passed to `b9b90` and woken again (`b7360`); bipeds go through
`e4c10` first. `109360` zeroes the whole block (`762` dwords) and sets the
byte `+1d84`; `109380` clears that byte. Bit 6 of definition flags is the
"early mover" flag of the community tag layouts (L for the name only).

**What `109660` writes.** C. A record per list index at
`[5107f0] + index × e8` (the 32 records fill `+0..+1d00`, the handle list
follows). From the mover's physics component (body `+b4`) it picks the
first rigid body whose shape is flagged, then: copies the current block to
the previous block (`+7c..+af` to `+48..+7b`, and three vectors `+0c`,
`+24`, `+3c` to `+00`, `+18`, `+30`) when `+e5` is set, and sets `+e4`;
writes the current rigid-body transform at `+7c` (`1d08e0`), two velocity
vectors at `+24`, `+3c` and a vector at `+0c`; sets `+e5`; and when a
previous transform exists, stores the delta transform previous⁻¹ × current
at `+b0` (`141590`, `142a60`) and re-orthonormalises its basis (`30bf0`).
Without a component or a flagged body it clears `+e4` and `+e5`.

**Who reads it, and when.** C for the call chains, L for the purpose
(riders move with what woke them). The readers find the record through
body `+b8` (the waking mover):

| Reader | Called from | Walk | Record it sees |
|---|---|---|---|
| `109a00` `(object: stack 0, ..)`, `ret 0c` | projectile update `f8de0` (`f8e52`); item update `10b4d0` → `10b850` | pre-physics | the previous tick's |
| `109e00` (root ancestor's mover), `ret 0c` | biped update `dd360` → `e3700` | pre-physics | the previous tick's |
| `109e00` | biped post-physics `dd990` → `e4680` → `e4770` | post-physics slot pass | this tick's (the early-mover pass ran first) |
| `109e00` | barrel fire `104150` (from `fd910` → `fd980`), players `14e200`, AI `1e0fe0`, `1ed340`, `1a68a0`, `e7aa6` | post-physics step 3 for weapons; others outside the walks | this tick's |
| `109fd0` `(mover: stack 0, out: edi)`, `ret 4`: copies record `+24` | AI `1eb6f0`, `1f2250` | AI, before the walks | the previous tick's |

So riders read `109660`'s record in the same tick from the post-physics
walk (biped post-update, deferred weapon fire) and in the next tick from
the pre-physics walk.

**`bc470` step 3.** The byte `[[4e6948]+c]` is the game's network mode,
written at game start (`0x6ec80`; [bots.md](../game/bots.md) has the
derivation): 1 local, 2 synchronous client, 3 synchronous server, 4
distributed client, 5 distributed server. C (writers `67fd6..68054`,
`6af09`, `6afd4`; readers across the object, unit and damage code compare
it with 4). `a9570` `(object: eax)` applies a pending replicated transform:
when body `+d8` bit 0 is set it gathers state (`a9820`), applies it
(`a9a70`, `a9b40`, `a99f0`) and clears the bit; a parented object goes
through `a9640` instead. The bit is set by `a96d0` and `a9770`, which take
the body's position (`+64`) and an orientation vector from the
distributed-simulation entity update `aab40`; body `+d4` is the entity
index (reset by `a7ab0`). C for the stores, L for "replicated transform".

**`fd910`.** C. No arguments, `ret`. Builds an object iterator on its
stack (type mask `4`, weapons; header mask `02`, AWAKE; slot cursor; the
handle out) and steps it with `baeb0`. For each weapon whose body `+16c`
bit 0 is set it calls `fd980(weapon)`, which fires every barrel
(`104150(weapon, barrel)`) whose state byte `+1a8 + 34 × i` has bit 1
set, clears that bit, and finally clears `+16c` bit 0. It is the deferred
weapon fire. `baeb0` advances the cursor at iterator `+6`, matches the
header flags against the mask byte at `+4` and the type against the mask
at `+0`, stores the handle at `+8` and returns the body.

**`bf380` (finalize tail).** C. Object memory collection. It allocates a
`2a04`-byte frame through `320560`. The mode comes from the object
globals `[4de2f4]` bytes `+1`/`+2` when a request is pending, otherwise
from pool pressure: the free space of the pool at `[4de2ec]` and the free
object slots (`800` − `[4e0300]+3c`), gated by the word `[4de2f4]+4`.
`bf1c0(mode: eax, out: esi)` decides whether to run. It then walks the
table at `440578` (`10`-byte entries: a type mask at `-8`, a flag at `-4`,
an optional prepare function at `+0`, a collector at `+4`; the walk ends
at a null collector). The two indirect calls are `call eax` with the entry
`+0` value `(mode, buffer, 2800)` once per run of entries, and
`call [edi+4]` `(mode, buffer, 2800, out, done, 200)`. It clears the
request bytes `+1`, `+2` on exit.

**`be1d0` → `b8c00`, `109140`.** C. When the object's definition has a
model (definition `+38`) whose two references at `+4` and `+14` are set,
`be1d0` gets the node matrices and count from `b8c00` and calls the
slot `+74` dispatcher `109140(object, count, matrices)`. Only `unit`
(`4679b0`) sets slot `+74` (`1130f0`, unit animation).

**Register conventions.** C, in "Entries" above. Corrections to the design
page's section 5 table: `bc7b0` takes its object on the stack (`ret 4`),
not in a register; `108cd0` takes it in `edi`; `109050`, `1090d0`,
`108a90` and `109140` take it in `ebx`.

**The slot `1090d0` dispatches.** C. Descriptor slot `+70`, set by `unit`
(`112dc0`) and `scen` (`10a390`). `bd090` calls it once per object at
`bd507`, after the root node matrix is composed (`bdc40`, `1421f0`,
`142a60`) and before the loop over the other nodes, with the root matrix as
its argument: it is the "after root" hook. The full slot map read from the
thirteen descriptors also has `+2c` (new) for unit, biped, vehicle, item,
weapon, garbage, projectile, scenery, device, machine, sound scenery, crate
and creature; `+74` for unit; `+78` for biped (`de4c0`) and vehicle
(`ee9b0`); `+7c` for unit (`114710`).

**RNG in function values.** C, for direct calls. No function reachable
from `ba8c0`, `bab40`, `13bb90`, `bcc20`, the seven import handlers or
`d1210`/`d1410` through direct calls (the whole closure, at most 36
functions) references the RNG global `[4e7408]`. The RNG is inlined, so a
draw is a reference to that global. The only indirect calls in the
closure are `108d90`'s slot calls (whose targets are the import handlers
checked here) and the import thunks reached from `bcc20` (`33198b`,
`3317d9`, `3315cf`). `13bb90` is a tag-function lookup with no state. So
function values and change colours are a pure kernel of their inputs;
section 7's wording holds.

**Unit offsets.** C. Actor handle `+12c` (`unit_new` `c4410` stores none;
`c6810` tests it against none at `c682c`), team `+138` `i16` (`c684a`),
current weapon index `+212` `i8` and weapon handles `+218` `[u32; 4]`
(`c8755`, `c86ad`). They are named in `crates/hooking/guest-layout/src/unit.rs`
(`ACTOR`, `TEAM`, `WEAPON_INDEX`, `WEAPONS`) and in
[live-gamestate.md](../live/live-gamestate.md), "AI bipeds, teams and
vehicles". The actor array is `[4f55f0]` (that page, L here).

**Not answerable statically.** Nodes posed per tick needs a count at
`bd090` over a Waterworks match and a co-op campaign level: slice 12's
step 0. The logical clock: today a Rust port is charged nothing for its own
guest accesses. The slice is counted in guest memory accessor calls
(`h2abi_v1_*`) made by generated code (`crates/runtime/runtime/src/preempt.rs`,
`spend_access`); a port reads and writes through the memory guard
(`ExecCtx::memory`, `runtime::Guest`), which does not spend; only a few hooks
charge explicitly with `ExecCtx::charge_accesses`
(`crates/rewrites/render-bridge/src/pipeline_render.rs`,
`crates/rewrites/render-bridge/src/pipeline_scene.rs`, `crates/apps/boot/src/fallback.rs`).
So a long Rust segment runs without expiring its slice, and the only
preemption points inside a walk are the true callouts. That is
deterministic. Whether to charge bodies is a decision, not a reading; a run
with `impl.sim.native.objects` on and off over the same film shows whether
thread interleaving changes.

## Register liveness

`scripts/sim-liveness.py` answers two questions for every entry above
and every direct callee of their bodies:

- **Entry:** which registers and arithmetic flags a function reads before
  writing them, which it saves and restores (pushed unwritten, popped back
  into the same register on some path), and which it leaves unwritten at
  some `ret`.
- **Site:** for every call in the slice 1 to 5 bodies, which registers and
  flags the caller reads after the return before writing them. That is
  what the caller takes from the callee, and what a body must hand back
  when it replaces the callee.

Rerun it from the worktree (it runs `h2 disasm` in the main checkout,
where `disc/default.xbe` is, and prints only register names and
addresses):

```sh
python3 -I scripts/sim-liveness.py                 # these tables, markdown
python3 -I scripts/sim-liveness.py --text          # with "reaches call X with R unwritten"
python3 -I scripts/sim-liveness.py 0xbc470 0xbdef0 # chosen entries and their sites
python3 -I scripts/sim-liveness.py --h2 ~/.cargo/bin/h2 --repo <checkout>
```

The walk follows both arms of every branch and stops a path at `ret`,
`int3`, an indirect `jmp`, a `jmp` out of the function (below the entry,
to a listed entry, or past `int3` padding), when everything is written, or
after 400 instructions; a visited set keys on the address and the written
state. Sub-registers count by byte (`al` is not `eax`). `xor r,r`,
`sub r,r`, `or r,-1` and `and r,0` are writes only. A `call` is not
entered: the walk records which registers were unwritten there, then
continues as if the callee wrote no register (over-reporting reads after a
call, never under-reporting) and as if it wrote the flags.

Limitations:

- Indirect jumps (jump tables) end the path; rows with one say so. `bcbb0`,
  `1588b0`, `f78e0`, `13b390` and `13bc00` have one.
- Only integer registers and EFLAGS. `xmm0` is an argument of `13bb90` and
  a result of it and of `ba8c0`'s callees; x87 state is not tracked.
- Saves are recognised by tracking `esp` through pushes, pops, `add`/`sub
  esp`, and callees' `ret N`; an indirect call is assumed to pop the pushes
  that immediately precede it (stdcall). Where `esp` is lost, a
  straight-line look-ahead decides.
- "Reads before writing" after a call includes registers the callee may
  have written (the walk does not enter callees). `1cd4e0`'s `al` is such
  a case: it is the result of its own callee `1d9320`.
- `baeb0`'s `edi[31:16]`: it increments `edi` after loading only `di`, so
  the caller's upper half is read and discarded.
- Rows marked "truncated" hit the state bound (`b7b40`, `1d96d0`,
  `16a8e0`); their sets are a lower bound.
- `esp` is always live and is left out.

### Entries and callees

DF in "Flags read" comes from string instructions (`rep movsd`,
`rep stosd`): the direction flag must be clear on entry, as the ABI
requires.

| Entry | Reads before writing | Flags read | Saved and restored | Reaches `ret` with unwritten | Notes |
|---|---|---|---|---|---|
| `b6f10` OBJECTS_PRE_PHYSICS | none | none | ecx ebx ebp esi edi | edx |  |
| `b7060` OBJECTS_POST_PHYSICS | none | none | ebx ebp esi edi | ecx |  |
| `b7150` OBJECTS_FINALIZE | none | none | ebx ebp esi edi | none | tail `bf380` |
| `bc470` OBJECT_PRE_UPDATE | none | none | ebx ebp esi edi | edx ebp |  |
| `bc820` OBJECT_POST_UPDATE | none | DF | ebx ebp esi edi | ebx |  |
| `108c60` type update dispatch | none | none | ebx ebp esi edi | none |  |
| `108cd0` type post dispatch | edi | none | esi | ebx ebp esi edi |  |
| `109050` type pose dispatch | ebx | none | ebp esi edi | ebx |  |
| `1090d0` type after-root dispatch | ebx | none | ebp esi edi | ebx |  |
| `108d90` OBJECT_FUNCTION_IMPORT | ebx edi | none | ebp esi | ebx edi |  |
| `108a90` type new dispatch | ebx | none | ebp esi edi | ebx |  |
| `109140` type node-matrix dispatch | ebx | none | ebp esi edi | ebx |  |
| `b7360` wake | eax | none | ebx esi | eax ebp edi |  |
| `baf80` root ancestor | ecx | none | none | ecx edx ebx ebp esi edi |  |
| `bba20` wake for post | eax | none | ebx esi edi | edx ebp |  |
| `bc7b0` wake children | none | none | esi | ebx ebp edi |  |
| `109580` early-mover wake | none | none | ebx ebp esi edi | esi edi |  |
| `bab40` OBJECT_FUNCTION_VALUE | none | none | ecx ebx ebp esi edi | none |  |
| `ba8c0` function evaluate | eax | none | ebx ebp esi edi | edi |  |
| `bcc20` BASE_OBJECT_IMPORT | none | none | ecx ebx esi | ebp edi |  |
| `be8e0` OBJECT_CHANGE_COLORS | none | none | ebx ebp esi edi | ebx ebp edi |  |
| `c6b90` unit import | none | none | ebx esi | ebp esi edi |  |
| `de3b0` biped import | none | none | esi | ebx ebp edi |  |
| `f6d10` vehicle import | none | none | ebx ebp esi edi | none |  |
| `ff5f0` weapon import | none | none | ecx ebx ebp esi edi | none |  |
| `fbe70` projectile import | none | none | ebx | ebp esi edi |  |
| `106d70` device import | none | none | ebx | ebp esi edi |  |
| `11bd60` machine and crate import | none | none | ebx esi | ebp esi edi |  |
| `bdef0` bounds | none | none | ebx ebp esi edi | none |  |
| `bef30` attachment refresh | none | none | ebx ebp esi edi | none |  |
| `be1d0` node-matrix notify | eax | none | ecx ebx | ebp esi edi |  |
| `bc5e0` component to object | none | none | ebx ebp esi edi | ebp esi |  |
| `bd090` OBJECT_NODE_MATRICES | none | DF | ebx ebp esi edi | none |  |
| `c1400` lights walk | none | none | ebx ebp esi edi | ebp edi |  |
| `c0c80` LIGHT_RECORD_UPDATE | none | none | ebx ebp esi edi | none |  |
| `c0b00` light record | eax ecx | none | ebx ebp esi edi | ecx edx |  |
| `c2d00` light list | none | none | ebx ebp esi edi | esi |  |
| `c3220` light list | eax | none | esi edi | eax ebx ebp edi |  |
| `c3260` light list | none | none | ebx ebp esi edi | none |  |
| `b7b40` object_new | none | DF | ebx ebp esi edi | ecx edx esi edi | truncated |
| `b83b0` delete | none | none | ebx esi edi | ebp |  |
| `b8460` delete | none | none | ebx ebp esi edi | none |  |
| `b7430` transform set | none | none | ecx ebx ebp esi edi | edx |  |
| `b7740` transform set | eax ecx edi | none | ebx ebp esi | ecx ebp edi |  |
| `fd910` deferred weapon fire | none | none | none | ecx edx ebx ebp esi edi |  |
| `bf380` object memory collection | none | none | ebx ebp esi edi | ecx edx edi |  |
| `109660` early-mover record | eax ecx | DF | ebx ebp esi edi | none |  |
| `a9570` network transform apply | eax | none | ebx esi edi | ebx ebp |  |
| `1cd4e0` animation | al esi | none | ebx | eax edx ebp esi edi |  |
| `1ced00` component | edi | none | ebx ebp esi | ecx edx ebx ebp esi edi |  |
| `1d3920` component nodes | none | none | ebx ebp esi edi | ecx edx |  |
| `1d44f0` component sphere | none | none | ebx ebp esi edi | eax ecx edx |  |
| `bb050` OBJECT_SPATIAL_VISIT | none | none | ebx ebp esi edi | ecx |  |
| `d5de0` OBJECT_DAMAGE_UPDATE | none | DF | ebx ebp esi edi | edi |  |
| `b87b0` partition disconnect | none | none | ebx ebp esi edi | none |  |
| `b8600` partition connect | none | DF | ebx ebp esi edi | none |  |
| `b8c00` node matrices of | eax | none | none | ebx ebp esi edi |  |
| `13bb90` function source | eax | none | edi | ecx edx ebx ebp esi |  |
| `baeb0` object iterator next | edi[31:16] | none | ebx ebp esi edi | none |  |
| `fd980` weapon fire triggers | none | none | ebx ebp esi edi | esi |  |
| `320560` | eax | none | ecx | eax ecx edx ebx ebp esi edi |  |
| `10aac0` | ebx | none | ebp esi edi | eax edx ebx esi edi |  |
| `b8bd0` | eax | none | none | ebx ebp esi edi |  |
| `bff60` | none | none | none | eax ecx edx ebx ebp esi edi |  |
| `bca70` | none | none | none | edx ebx ebp esi edi |  |
| `bcbb0` | eax | none | ecx | edx ebx ebp esi edi | 1 indirect jmp |
| `bcad0` | none | none | esi | ebx ebp edi |  |
| `bcb60` | none | none | esi | ecx edx ebx ebp edi |  |
| `bcb10` | none | none | esi | ecx edx ebx ebp edi |  |
| `1588b0` | ecx | none | esi | ebx ebp edi | 1 indirect jmp |
| `c18e0` | none | none | esi | edx ebx ebp edi |  |
| `bc970` | eax ecx | none | ebx esi | eax ecx ebp esi edi |  |
| `131c20` | eax ecx esi edi | none | ebx | ecx edx ebp esi edi |  |
| `d1210` | eax | none | none | ebx ebp esi edi |  |
| `d1410` | eax | none | esi | eax edx ebx ebp esi edi |  |
| `f5d70` | eax | none | none | ebx ebp esi edi |  |
| `e6fe0` | eax ecx | none | esi edi | ebx ebp |  |
| `f78e0` | eax | none | ecx | none | 1 indirect jmp |
| `f78b0` | none | none | none | eax ecx edx ebx ebp esi edi |  |
| `24590` | eax | none | none | eax ecx edx ebx ebp esi edi |  |
| `f8070` | cl | none | none | ebx ebp esi edi |  |
| `11d580` | eax ecx edx esi | none | none | eax ecx edx ebx ebp esi edi |  |
| `102070` | eax | none | esi | ebx ebp edi |  |
| `10cf50` | ecx | none | none | eax ebx ebp esi edi |  |
| `162b10` | ecx | none | esi | eax ecx ebx ebp edi |  |
| `162b70` | none | none | none | ecx edx ebx ebp esi edi |  |
| `1d3550` | ecx edx esi | none | none | eax edx ebx ebp esi edi |  |
| `b9ce0` | ecx | none | esi | ebx ebp edi |  |
| `c1720` | none | none | ebx ebp esi edi | ecx edx |  |
| `1d1c10` | esi | none | ecx ebx ebp edi | edx esi edi |  |
| `1d1ca0` | edi | none | esi | eax edx ebx ebp esi edi |  |
| `1d4360` | edi | none | esi | ebx ebp esi edi |  |
| `b91d0` | eax esi edi | none | ebx | ecx edx ebp esi edi | tail `11d090` |
| `1d09d0` | eax ecx edx | none | none | ecx ebx ebp esi edi |  |
| `1d0ad0` | eax ecx edx | none | none | ecx ebx ebp esi edi |  |
| `1d96d0` | eax | none | ebx ebp esi edi | edx ebp edi | truncated |
| `e1670` | eax | none | none | ebx ebp esi edi |  |
| `16d940` | none | none | ebx ebp esi edi | edx ebx ebp edi |  |
| `bf5a0` | eax | none | none | ebx ebp esi edi |  |
| `bfc90` | ecx | none | none | eax ecx edx ebx ebp esi edi |  |
| `bff10` | ecx | none | none | ecx ebx ebp esi edi |  |
| `bfe80` | eax | none | none | eax ebx ebp esi edi |  |
| `bfed0` | eax esi | DF | ebx edi | ecx ebp esi edi |  |
| `bdb60` | eax ecx edx | none | ebx ebp esi edi | eax ecx edx |  |
| `bfe20` | eax cl dl | none | none | eax ecx ebx ebp esi edi |  |
| `b6780` | eax | none | none | ebx ebp esi edi |  |
| `1c6440` | ecx | none | none | ecx edx ebx ebp esi edi |  |
| `1421f0` | none | none | ebp | edx ebx esi edi |  |
| `a98b0` | ecx esi | none | ebx | ecx ebp esi edi |  |
| `a9940` | ecx esi edi | none | ebx | ecx ebp esi edi |  |
| `bdc40` | eax ecx bl | DF | ebp esi edi | ebx |  |
| `142a60` | ecx edx | none | none | ecx edx ebx ebp esi edi |  |
| `16bc00` | ecx edx | none | ebx esi edi | ecx edx ebx ebp esi edi |  |
| `1cae40` | edi | none | ebx ebp esi | edx ebx esi edi |  |
| `bad50` | eax esi edi | none | ebx | ebp esi edi |  |
| `131d60` | ecx ebx | none | ebp esi edi | edx ebx |  |
| `17ca10` | ax | none | esi | eax ecx edx ebx ebp esi edi |  |
| `b9d20` | eax | none | esi | ebx ebp edi |  |
| `13b390` | edi | none | ebx ebp esi | eax ecx edx edi | 1 indirect jmp |
| `13bc00` | ecx | none | ebp esi | ecx edx ebx edi | 1 indirect jmp |
| `c0840` | eax esi | none | ebx ebp edi | eax ecx edx ebx ebp esi edi |  |
| `b8c40` | ax ecx | none | esi | ebx ebp edi |  |
| `16640f` | eax | DF | ebx ebp esi edi | ecx edx edi |  |
| `b8d30` | cl ebx | DF | ebp esi edi | ebx |  |
| `142570` | eax edx | none | none | edx ebx ebp esi edi |  |
| `c28b0` | eax | none | ebx ebp esi edi | none |  |
| `16a8e0` | none | none | ebx ebp esi edi | ebx esi | truncated |
| `11bed0` | esi | none | edi | eax ecx edx ebx ebp esi |  |
| `ba300` | eax ecx | none | esi edi | eax ebx ebp esi |  |
| `1cac60` | eax | DF | ebx ebp esi edi | esi |  |
| `16ba40` | eax edx | DF | ebx ebp esi edi | edx ebp |  |

### Call sites

"Call" is the call instruction, "Returns to" the retail return site a
body lists in `Body::sites`. "Reaches `ret` with unwritten" names the
registers whose value at the caller's own return may be the callee's.

| In | Call | Returns to | Callee | After return, reads | Flags | Reaches `ret` with unwritten |
|---|---|---|---|---|---|---|
| `b6f10` | `b6f69` | `b6f6e` | `bc470` | ebx ebp esi edi | none | edx |
| `b6f10` | `b6f7a` | `b6f7f` | `109580` | ebx ebp | none | edx |
| `b6f10` | `b7031` | `b7036` | `bc470` | none | none | none |
| `b7060` | `b70a9` | `b70ae` | `bc820` | ebx ebp esi edi | none | none |
| `b7060` | `b70b2` | `b70b7` | `109660` | ebx ebp edi | none | ecx |
| `b7060` | `b70f1` | `b70f6` | `fd910` | none | none | eax ecx |
| `b7060` | `b713d` | `b7142` | `bc820` | none | none | none |
| `b7150` | `b719b` | `b71a0` | `fd910` | none | none | none |
| `b7150` | `b723b` | `b7240` | `bc470` | ebx ebp esi edi | none | none |
| `b7150` | `b724b` | `b7250` | `bc820` | none | none | none |
| `b7150` | `b727d` | `b7282` | `b83b0` | esi | none | none |
| `b7150` | `b7285` | `b728a` | `b8460` | none | none | none |
| `bc470` | `bc4c5` | `bc4ca` | `bba20` | bl ebp esi edi | none | none |
| `bc470` | `bc4e8` | `bc4ed` | `a9570` | bl ebp esi edi | none | none |
| `bc470` | `bc517` | `bc51c` | `1cd4e0` | al ebp edi | none | ecx edx |
| `bc470` | `bc51f` | `bc524` | `108c60` | al bl ebp edi | none | ecx edx |
| `bc470` | `bc527` | `bc52c` | `d5de0` | al bl ebp edi | none | ecx edx |
| `bc470` | `bc52f` | `bc534` | `be8e0` | al bl ebp | none | ecx edx |
| `bc470` | `bc541` | `bc546` | `bc470` | al bl ebp esi | none | none |
| `bc470` | `bc595` | `bc59a` | `1ced00` | bl ebp | none | eax ecx edx |
| `bc820` | `bc84e` | `bc853` | `bc5e0` | ebp esi edi | DF | ecx edx ebx |
| `bc820` | `bc862` | `bc867` | `bd090` | al ebp esi edi | none | ecx edx ebx |
| `bc820` | `bc8dc` | `bc8e1` | `1d3920` | al ebp esi | none | ecx edx |
| `bc820` | `bc8e6` | `bc8eb` | `108cd0` | al ebp esi edi | none | ecx edx ebx |
| `bc820` | `bc911` | `bc916` | `bdef0` | al ebp esi edi | none | ecx edx ebx |
| `bc820` | `bc929` | `bc92e` | `bef30` | esi edi | none | ecx edx ebx |
| `bc820` | `bc937` | `bc93c` | `bc820` | esi edi | none | ebx |
| `bc820` | `bc95c` | `bc961` | `be1d0` | none | none | eax ecx edx ebx |
| `108c60` | `108cab` | `108cad` | indirect | al bl ebp esi edi | none | none |
| `108cd0` | `108d11` | `108d13` | indirect | esi edi | none | edx ebx ebp edi |
| `109050` | `1090a5` | `1090a7` | indirect | ebx ebp esi edi | none | ebx |
| `1090d0` | `10911b` | `10911d` | indirect | ebx ebp esi edi | none | ebx |
| `108d90` | `108de5` | `108de7` | indirect | al ebx ebp esi edi | none | eax ecx edx ebx edi |
| `108a90` | `108ae4` | `108ae6` | indirect | al ebx ebp esi edi | none | eax ebx |
| `109140` | `109190` | `109192` | indirect | ebx ebp esi edi | none | edx ebx |
| `bba20` | `bba44` | `bba49` | `baf80` | eax ebx | none | ecx ebp |
| `bba20` | `bba5e` | `bba63` | `b7360` | esi edi | none | eax ecx edx ebp |
| `bba20` | `bba68` | `bba6d` | `bc7b0` | none | none | eax ecx edx ebp |
| `bc7b0` | `bc7e1` | `bc7e6` | `b7360` | esi | none | eax edx ebx ebp edi |
| `bc7b0` | `bc807` | `bc80c` | `bc7b0` | esi | none | edx ebx ebp edi |
| `109580` | `109585` | `10958a` | `320560` | none | none | esi edi |
| `109580` | `1095c2` | `1095c7` | `bb050` | ax ebx | none | ecx edx esi edi |
| `109580` | `10961d` | `109622` | `b7360` | ebx ebp esi edi | none | ecx |
| `109580` | `109633` | `109638` | `bba20` | ebx ebp esi | none | ecx edx |
| `bab40` | `bab8e` | `bab93` | `10aac0` | al ebx ebp esi edi | none | eax edx |
| `bab40` | `babdd` | `babe2` | `108d90` | al ebp esi | none | eax edx |
| `bab40` | `bac37` | `bac3c` | `ba8c0` | al ebx ebp esi edi | none | eax edx |
| `bab40` | `bac6f` | `bac74` | `bab40` | none | none | eax edx |
| `ba8c0` | `ba927` | `ba92c` | `10aac0` | al ebp esi | none | eax ecx edx edi |
| `ba8c0` | `ba949` | `ba94e` | `108d90` | ebp esi | none | eax edx |
| `ba8c0` | `ba98c` | `ba991` | `13bb90` | ebp esi | none | eax edx |
| `ba8c0` | `baa22` | `baa27` | `13bb90` | ebp esi | none | eax edx |
| `ba8c0` | `baa45` | `baa4a` | `bab40` | al ebp esi | none | eax edx |
| `ba8c0` | `baac7` | `baacc` | `108d90` | none | none | eax edx |
| `bcc20` | `bccbf` | `bccc4` | `b8bd0` | eax bl | none | eax edx ebp edi |
| `bcc20` | `bccf5` | `bccfa` | `bff60` | bl | none | eax edx ebp edi |
| `bcc20` | `bcd2b` | `bcd30` | `bca70` | bl | none | eax edx ebp edi |
| `bcc20` | `bcd35` | `bcd3a` | `bcbb0` | bl | none | eax edx ebp edi |
| `bcc20` | `bcda5` | `bcdaa` | `bcad0` | bl | none | eax edx ebp edi |
| `bcc20` | `bcdd4` | `bcdd9` | `bcb60` | bl | none | eax edx ebp edi |
| `bcc20` | `bcde1` | `bcde6` | `bcb10` | bl | none | eax edx ebp edi |
| `bcc20` | `bcec6` | `bcecb` | `1588b0` | bl esi | none | eax edx ebp edi |
| `bcc20` | `bcef6` | `bcefb` | `c18e0` | bl | none | eax edx ebp edi |
| `bcc20` | `bcfa7` | `bcfac` | `bc970` | al | none | eax edx ebp edi |
| `be8e0` | `be9bc` | `be9c1` | `bab40` | al ebx ebp edi | none | none |
| `be8e0` | `be9df` | `be9e4` | `131c20` | ebx ebp | none | none |
| `be8e0` | `bea27` | `bea2c` | `bab40` | al ebx ebp esi edi | none | edx |
| `c6b90` | `c6c21` | `c6c26` | `d1210` | bl | none | ebp esi edi |
| `c6b90` | `c6c28` | `c6c2d` | `d1410` | bl | none | ebp esi edi |
| `c6b90` | `c6ca6` | `c6cab` | `d1410` | none | none | none |
| `c6b90` | `c6dc8` | `c6dcd` | `ff5f0` | none | none | eax ecx edx ebp edi |
| `f6d10` | `f6dc0` | `f6dc5` | `d1210` | bl | none | none |
| `f6d10` | `f6dcc` | `f6dd1` | `f5d70` | al bl esi | none | none |
| `f6d10` | `f6e1c` | `f6e21` | `e6fe0` | al bl | none | none |
| `f6d10` | `f6f28` | `f6f2d` | `f78e0` | bl esi | none | none |
| `f6d10` | `f6fff` | `f7004` | `f78b0` | bl ebp esi | none | none |
| `f6d10` | `f7015` | `f701a` | `f78b0` | bl | none | none |
| `f6d10` | `f7079` | `f707e` | `24590` | ecx bl | none | none |
| `f6d10` | `f7082` | `f7087` | `f78b0` | bl | none | none |
| `f6d10` | `f70c1` | `f70c6` | `f8070` | al ebx esi edi | none | none |
| `f6d10` | `f70dc` | `f70e1` | `f8070` | al ebp esi edi | none | none |
| `f6d10` | `f7105` | `f710a` | `f78e0` | ebx ebp esi edi | none | none |
| `f6d10` | `f712e` | `f7133` | `f78b0` | ebx esi edi | none | none |
| `f6d10` | `f713a` | `f713f` | `f78e0` | esi edi | none | none |
| `f6d10` | `f7151` | `f7156` | `f78b0` | esi edi | none | none |
| `f6d10` | `f71b8` | `f71bd` | `f78b0` | esi edi | none | none |
| `f6d10` | `f7205` | `f720a` | `f78e0` | bl | none | none |
| `f6d10` | `f7283` | `f7288` | `f78e0` | bl esi | none | none |
| `f6d10` | `f748c` | `f7491` | `24590` | ecx bl | none | none |
| `f6d10` | `f7495` | `f749a` | `f78b0` | bl | none | none |
| `f6d10` | `f7554` | `f7559` | `24590` | ecx bl | none | none |
| `f6d10` | `f755d` | `f7562` | `f78b0` | bl | none | none |
| `f6d10` | `f75f6` | `f75fb` | `11d580` | ecx bl esi | none | none |
| `f6d10` | `f7602` | `f7607` | `24590` | ecx bl | none | none |
| `f6d10` | `f760b` | `f7610` | `f78b0` | bl | none | none |
| `f6d10` | `f76a9` | `f76ae` | `f78b0` | eax ecx bl esi | none | none |
| `f6d10` | `f7705` | `f770a` | `f78b0` | bl | none | none |
| `f6d10` | `f771c` | `f7721` | `f78e0` | bl esi | none | none |
| `f6d10` | `f7755` | `f775a` | `f78e0` | bl esi | none | none |
| `f6d10` | `f77b7` | `f77bc` | `f78e0` | ecx bl esi | none | none |
| `f6d10` | `f77c8` | `f77cd` | `24590` | ecx bl esi | none | none |
| `f6d10` | `f77d1` | `f77d6` | `f78b0` | ecx bl esi | none | none |
| `f6d10` | `f77ec` | `f77f1` | `f78b0` | bl | none | none |
| `f6d10` | `f7839` | `f783e` | `f78e0` | bl esi | none | none |
| `f6d10` | `f7852` | `f7857` | `f78b0` | bl | none | none |
| `ff5f0` | `ff745` | `ff74a` | `102070` | ebx ebp esi edi | none | none |
| `ff5f0` | `ff7ea` | `ff7ef` | `10cf50` | al | none | none |
| `ff5f0` | `ff887` | `ff88c` | `10cf50` | al ebp | none | none |
| `ff5f0` | `ff940` | `ff945` | `162b10` | al esi edi | none | none |
| `ff5f0` | `ff94e` | `ff953` | `162b70` | esi | none | none |
| `ff5f0` | `ffa65` | `ffa6a` | `162b10` | al edi | none | none |
| `ff5f0` | `ffa73` | `ffa78` | `162b70` | none | none | none |
| `ff5f0` | `ffa9c` | `ffaa1` | `102070` | none | none | none |
| `11bd60` | `11bdaa` | `11bdaf` | `1d3550` | al bl | none | eax ecx edx ebp edi |
| `bdef0` | `bdf7f` | `bdf84` | `1d44f0` | al ebx ebp esi edi | none | edx |
| `bdef0` | `be189` | `be18e` | `b87b0` | ebx ebp esi edi | none | ecx edx |
| `bdef0` | `be1b3` | `be1b8` | `b8600` | none | none | eax ecx edx |
| `bef30` | `bef80` | `bef85` | `b9ce0` | al esi edi | none | eax ecx edx |
| `bef30` | `befcf` | `befd4` | `c3220` | bl ebp esi edi | none | ecx |
| `bef30` | `befdf` | `befe4` | `c2d00` | bl ebp esi edi | none | ecx |
| `bef30` | `bf004` | `bf009` | `c1720` | dh+edx[31:16] bl edi | none | ecx edx |
| `bef30` | `bf024` | `bf029` | `bef30` | edi | none | eax ecx edx |
| `be1d0` | `be225` | `be22a` | `b8c00` | eax | none | eax edx ebp esi edi |
| `be1d0` | `be230` | `be235` | `109140` | none | none | eax edx ebp esi edi |
| `bc5e0` | `bc634` | `bc639` | `1d1c10` | al ebx ebp edi | none | eax ecx |
| `bc5e0` | `bc673` | `bc678` | `1d1ca0` | al ebx ebp | none | none |
| `bc5e0` | `bc685` | `bc68a` | `1d4360` | ebp | none | none |
| `bc5e0` | `bc6b2` | `bc6b7` | `b91d0` | ebx ebp | none | none |
| `bc5e0` | `bc6d1` | `bc6d6` | `b7430` | ebp esi | none | none |
| `bc5e0` | `bc6e8` | `bc6ed` | `1d09d0` | ebp esi | none | none |
| `bc5e0` | `bc6ff` | `bc704` | `1d0ad0` | ecx esi | none | none |
| `bc5e0` | `bc71e` | `bc723` | `b7740` | none | none | eax ecx |
| `bc5e0` | `bc755` | `bc75a` | `1d96d0` | bl edi | none | eax ecx edx ebp esi |
| `bd090` | `bd18c` | `bd191` | `e1670` | ebx esi edi | DF | none |
| `bd090` | `bd1a7` | `bd1ac` | `1d3920` | ebx esi | DF | none |
| `bd090` | `bd247` | `bd24c` | `b8bd0` | eax ebx ebp esi edi | DF | none |
| `bd090` | `bd2a1` | `bd2a6` | `16d940` | ebx ebp | DF | ecx edx |
| `bd090` | `bd2af` | `bd2b4` | `bf5a0` | al ebx ebp esi | DF | ecx edx |
| `bd090` | `bd2be` | `bd2c3` | `bfc90` | al ebx ebp | DF | ecx edx |
| `bd090` | `bd307` | `bd30c` | `bff10` | al ebx ebp esi edi | DF | none |
| `bd090` | `bd323` | `bd328` | `bfe80` | ebx ebp | DF | none |
| `bd090` | `bd331` | `bd336` | `bfed0` | ebx ebp | DF | none |
| `bd090` | `bd353` | `bd358` | `bdb60` | ebx ebp esi | DF | edx |
| `bd090` | `bd365` | `bd36a` | `bfe20` | ebx ebp | DF | ecx edx |
| `bd090` | `bd372` | `bd377` | `b6780` | eax ebx ebp | DF | ecx edx |
| `bd090` | `bd37d` | `bd382` | `1c6440` | eax ebx | DF | ecx edx |
| `bd090` | `bd3c6` | `bd3cb` | `1421f0` | ebp | none | ecx edx |
| `bd090` | `bd432` | `bd437` | `a98b0` | al ebx ebp | none | none |
| `bd090` | `bd462` | `bd467` | `a9940` | al ebx ebp | none | none |
| `bd090` | `bd4cc` | `bd4d1` | `bdc40` | ebp | none | none |
| `bd090` | `bd4de` | `bd4e3` | `1421f0` | ebp | none | none |
| `bd090` | `bd4f6` | `bd4fb` | `142a60` | ebp | none | ecx edx |
| `bd090` | `bd507` | `bd50c` | `1090d0` | ebp | none | ecx edx |
| `bd090` | `bd559` | `bd55e` | `1421f0` | edx ebx ebp esi edi | none | none |
| `bd090` | `bd56c` | `bd571` | `142a60` | edx ebx ebp esi | none | none |
| `bd090` | `bd5f0` | `bd5f5` | `bdc40` | none | none | eax ecx edx |
| `c1400` | `c141b` | `c1420` | `16bc00` | eax esi | none | eax ecx edx ebp edi |
| `c1400` | `c1484` | `c1489` | `c0c80` | ebx ebp esi edi | none | ecx edx |
| `c1400` | `c14d9` | `c14de` | `1cae40` | ebx ebp esi | none | ecx edx |
| `c1400` | `c14e3` | `c14e8` | `c2d00` | ebx esi | none | ecx edx |
| `c1400` | `c1521` | `c1526` | `c3260` | ebx ebp | none | ecx edx |
| `c0c80` | `c0dc4` | `c0dc9` | `bab40` | ebx ebp | none | none |
| `c0c80` | `c0dd5` | `c0dda` | `bad50` | ebx ebp esi | none | none |
| `c0c80` | `c0df7` | `c0dfc` | `131d60` | ebp esi | none | none |
| `c0c80` | `c0e17` | `c0e1c` | `131d60` | ebp esi | none | none |
| `c0c80` | `c0e87` | `c0e8c` | `17ca10` | ebx ebp | none | none |
| `c0c80` | `c0eba` | `c0ebf` | `131c20` | ebx ebp | none | none |
| `c0c80` | `c0ed6` | `c0edb` | `131c20` | ebx ebp | none | none |
| `c0c80` | `c0f70` | `c0f75` | `c0b00` | ecx[31:16] ebx ebp esi edi | none | ecx edx |
| `c0c80` | `c0f90` | `c0f95` | `baf80` | eax ebx ebp esi edi | none | none |
| `c0c80` | `c1270` | `c1275` | `b9ce0` | al esi | none | eax ecx edx |
| `c0c80` | `c127b` | `c1280` | `b9d20` | al | none | eax ecx edx |
| `c0c80` | `c1297` | `c129c` | `c2d00` | none | none | eax ecx edx |
| `c0c80` | `c12b4` | `c12b9` | `c3220` | ebp | none | eax ecx edx |
| `c0b00` | `c0b63` | `c0b68` | `13b390` | ebx ebp esi edi | none | ecx edx |
| `c0b00` | `c0bc6` | `c0bcb` | `13b390` | eax ebx ebp esi edi | none | none |
| `c0b00` | `c0bd3` | `c0bd8` | `13bc00` | eax ebx ebp esi | none | none |
| `c0b00` | `c0c5d` | `c0c62` | `13bb90` | ebx ebp esi | none | ecx edx |
| `c0b00` | `c0c6f` | `c0c74` | `13bb90` | edi | none | eax ecx edx |
| `c2d00` | `c2d6c` | `c2d71` | `c0840` | ebx ebp edi | none | none |
| `c2d00` | `c2d86` | `c2d8b` | `b8c40` | eax ebx ebp edi | none | none |
| `c2d00` | `c2dac` | `c2db1` | `16640f` | al ebx esi edi | none | none |
| `c2d00` | `c2e02` | `c2e07` | `b8d30` | edi | none | none |
| `c2d00` | `c2ed8` | `c2edd` | `142570` | edx ebx ebp esi edi | none | none |
| `c2d00` | `c3040` | `c3045` | `c28b0` | ebx ebp edi | none | none |
| `c2d00` | `c307c` | `c3081` | `16a8e0` | ebx ebp esi edi | none | none |
| `c2d00` | `c309a` | `c309f` | `11bed0` | ebx ebp esi edi | none | none |
| `c2d00` | `c30bb` | `c30c0` | `ba300` | ebp esi edi | none | none |
| `c2d00` | `c30cd` | `c30d2` | `11bed0` | ebp esi edi | none | none |
| `c2d00` | `c30f0` | `c30f5` | `1cac60` | edi | none | eax ecx edx |
| `c3220` | `c324e` | `c3253` | `1cae40` | esi | none | eax ecx edx ebx ebp |
| `c3260` | `c3296` | `c329b` | `1cae40` | ebx ebp esi | none | ecx |
| `c3260` | `c32d5` | `c32da` | `16bc00` | eax bl ebp esi edi | none | none |
| `c3260` | `c3312` | `c3317` | `16ba40` | none | none | eax ecx edx |
| `c3260` | `c3328` | `c332d` | `16ba40` | none | none | eax ecx edx |

## Not verified

- Nodes posed per tick on Waterworks and in co-op campaign (slice 12,
  step 0: a count at `bd090` through the agent API).
- That no other DEFER setter exists outside the `or` forms scanned (a flags
  byte copied whole from another header, or a computed store, would not
  match). A differential run of the walks with DEFER watched would show it.
- The purpose of the definition flags bit 6 and of body `+d8` bit 0 (L),
  and whether the replicated transform `a9570` applies ever runs outside
  distributed modes.
- Whether the type callbacks behind slots `+70`, `+74`, `+78` and `+7c`
  draw from the RNG (not followed here; slices 4 and 6 read them).
- Liveness through the indirect jumps listed above, and of `xmm0` and x87
  state at every site.
- Whether charging Rust bodies to the logical clock changes thread
  interleaving in a film (on/off run parity with `impl.sim.arena-hash`).

## Related

- [sim-rewrite-objects.md](sim-rewrite-objects.md): the design, decisions
  and slices this page serves.
- [sim-rewrite.md](sim-rewrite.md): the plan.
- [collision-queries.md](collision-queries.md): the phase 2 contract page
  (`bb050`, `b8600`, `b87b0` move there).
- [anim-sampling.md](anim-sampling.md): phase 1, which shares `bd090`.
- [live-gamestate.md](../live/live-gamestate.md): unit and weapon offsets.
- [bots.md](../game/bots.md): the network mode byte.
- [README.md](README.md): the engine contract format and fault behaviour.
