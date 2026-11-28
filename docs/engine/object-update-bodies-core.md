# Object update bodies: walks, functions and the post-update shell

Status: Built (native only). Slice 0 resolutions and the bodies of slices 1, 3 and 4: the walks and dispatchers, function values and change colours, and the post-update shell.
Contract page for phase 3 of the [simulation rewrite](sim-rewrite-objects.md); entries and conventions are in [object-update.md](object-update.md).

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

**The after-root slots `112dc0` and `10a390`.** C. Both read a parent
handle (unit: body `+ [body+33e] + 28`; scenery: body `+138`). When it is
not none and `badc0(eax: handle, -1)` answers an object, they compose the
matrix argument in place with the parent's root node matrix (body
`+ [body+116]`) through `142a60`. The bodies
(`engine::sim_objects::after_root`) reach `badc0` as a table body
(`OBJECT_GET_TYPED`) and run `142a60` inline through
`sim_native::compose`, as `bd090` does.

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
(`crates/runtime/recomp/src/fallback.rs`).
So a long Rust segment runs without expiring its slice, and the only
preemption points inside a walk are the true callouts. That is
deterministic. Whether to charge bodies is a decision, not a reading; a run
over the same film, compared with `h2 sim diff` against the lifted build, shows whether
thread interleaving changes.

## Slice 1 bodies

The three walks and the typed object iterator use the same bounded bitmap scan
in `sim_objects::walk::next_live`. Each visit reads through the first occupied
slot and stops. The next visit reads the high-water mark and the bitmap again,
so a callback can activate or delete a slot in the same word.

Each is **C**: read at its address with `h2 disasm`, and compared with
the lifted original by `tests/sim_objects_differential.rs`.

**Walks** (`engine::sim_objects::walks`). The pre- and post-physics
bodies are the per-function ports, with each transfer returned to
`drive` instead of the runtime. Finalize `b7150` keeps the objects array in `ebp`, read
once per pass. Its scan is one slot at a time: `test eax, eax; jl`, the
high-water mark into `esi`, `cmp eax, esi; jge`, the bitmap into `edx`,
then `ecx` the word index and `edi` the bit of each slot tried, until a
live slot or the mark. The body reads each bitmap word once and leaves
`eax`, `ecx`, `edi` and the flags as the last slot tried left them. Pass
A keeps the slot in `ebx`, the handle in `edi` and the header in `esi`;
it stores the flags with DEFER cleared (`dl`, so `edx` holds the bitmap
pointer with that byte), and for a NEW, ACTIVE, AWAKE object without
DELETE stores them again with NEW cleared, then calls `bc470` and, if
POST is set after it, `bc820`. Pass B keeps the slot in `edi` and the
handle in `esi` and calls `b83b0(handle, 1)` then `b8460(handle, 1)` for
each DELETE object. The phase byte is cleared after `pop edi`, `pop esi`
and `pop ebp`, before `pop ebx` and the jump to `bf380`.
Neither pass steps register by register through a run of slots it only
passes over (pass A: not due, so only DEFER is cleared; pass B: no
DELETE). `walks::skip_idle` reads the run once, stores the cleared
flags, and leaves the slot register one live slot before the last idle
slot, so the loop replays that slot and the one after it and every
transfer shows retail's registers. No callout happens inside the run.
The tick's finalize is then one pass over the bitmap plus the due
objects' callouts. The walk keeps no compare arm and has no refusal of
its own: the float-environment decline (`supported`) is the only one,
counted by `walk_arm::declined` when the arm is on.

**Events.** Before finalize's `b83b0` call the body appends
`ObjectDeleted { id, kind, definition }` (the handle, header `+3`, body
`+0`) to the tick's buffer (`sim_objects::events`, read through
`sim_state::events`). The buffer belongs to the thread that runs the
walks, and it is emptied at `GAME_TICK`'s entry ("Slice 8 body"). Those two reads are not retail's,
so a fault there leaves the field none instead of faulting the guest.

**Per-object updates** (`object.rs`). `bc470` and `bc820` are the ports,
as above.

**Type dispatchers** (`object.rs`). Each reads the type from body `+aa`
(`movsx`), the descriptor from `[468630 + 4 × type]`, and walks the
ancestry at descriptor `+84` (descriptor pointers, null-terminated). Per
level it loads `eax` from the level's slot and calls it when non-null;
between levels it keeps the address of the level entry in `eax`, the
entry in `ecx` and (except `108cd0`) the level number sign-extended from
16 bits in `edx`.

| Body | Keeps | At the slot call |
|---|---|---|
| `108c60` | object `ebp`, descriptor `edi`, level `esi`, result `bl` | `push ebp`; after it `or bl, al` |
| `108cd0` | address of the level entry `esi` (pushed only when the list is not empty) | `push edi` |
| `109050` | `s2` `ebp`, descriptor `esi`, level `edi` | `edx` = `s1`, `ecx` = `s0` from the frame, then `push ebp, edx, ecx, ebx` |
| `1090d0` | matrix `ebp`, descriptor `edi`, level `esi` | `push ebp, ebx` |

The slots resolve per type through the ancestry, so a type without a slot
of its own gets its parent's: weapons, equipment and garbage get the item
update `10b4d0`, machines, controls and light fixtures the device update
`106790` (and the device pose slot `107000`), bipeds and vehicles the
unit update `c6b30` before their own. Sound scenery has no update slot at
any level.

**Wake leaves** (`sim_objects::wake`, bodies in `engine::sim_objects::wake`).
Plain functions over guest memory; the body puts their exit in `eax`,
`ecx`, `edx` and the flags. They save or never write `ebx`, `ebp`, `esi`
and `edi`. The header address is `(handle & ffff) × c` (the stride is
hard-coded, not the array's element size).

| Leaf | `eax` | `ecx` | `edx` | Flags |
|---|---|---|---|---|
| `b7360` | the object when it was already AWAKE; else none | the header storage, re-read after each store | `(object & ffff) × 3` | `test` of AWAKE (all clear) when it was AWAKE; else `cmp` with none (ZF, PF) |
| `baf80` | the root ancestor; none for none | none | the header storage; the caller's for none | `cmp` with none |
| `bc7b0` | for a leaf, the object or `b7360`'s `eax`; else the last child's body | none | for a leaf, `1 << type` or `b7360`'s `edx`; else the last child's `edx` | `cmp` with none |
| `bba20` | `bc7b0`'s | `bc7b0`'s | `bc7b0`'s | `bc7b0`'s |

`bc7b0` reads each child's next sibling after the child's subtree,
through the child's header again. The body keeps an explicit stack of
open frames instead of recursing, and reports a fault past `2^16` levels
(a cycle in the child links, where retail recurses until the guest stack
runs out).

The leaves leave the stack below their entry `esp` as retail does
(`sim_objects::wake::Stack`): `b7360` pushes `ebx` and `esi`, `bba20`
pushes `ebx`, `esi` and `edi` and calls `baf80`, `b7360` and `bc7b0`
(returning to `bba49`, `bba63`, `bba6d`), and frame `d` of `bc7b0` is
entered `c × d` below the first and pushes `esi`, then for `b7360` its
return `bc7e6`, then for each child the handle and the return `bc80c`.
Those slots are dead when the leaf returns, but the caller's next deeper
frame overlays them, and a local it never writes holds what the leaf left
there.

**Early-mover wake** (`wake.rs`). `109580` sets `eax` = `1004` and calls
the stack probe `320560`, which leaves the caller's `ecx` at the top of
the new frame and `esp` `1004` lower. After its pushes (`ebx`, `ebp`,
then `esi`, `edi` once the count is positive) the frame holds the count
at `[esp+10]`, the result buffer at `[esp+14]` and the mover at
`[esp+1018]`. `ebx` keeps the objects array from before the visit across
every call. The visit is `bb050(0, 3f, location, centre, radius bits,
buffer, 400)`; its `ax` is sign-extended. Per result: `or` body `+c0`
with 4, then store the mover at `+b8`, then `b7360`; for types 2 to 5,
`bba20` and `or` the header with NEW.

## Slice 3 bodies

Each is **C**: read at its address with `h2 disasm`, and compared with
the lifted original by `tests/sim_objects_differential.rs`. All are
segments in `engine::sim_objects::functions`. They leave the arithmetic
flags undefined at every call and return (the liveness tables below show
no reader at any slice 3 entry or site) and keep `xmm0` to `xmm4` and the
x87 state as retail leaves them. Function names compared by `cmp` or
`sub` are read from those instructions' immediates in the loaded image.

The `sim_objects::functions::ExportCursor` scans consecutive misses in
one Rust loop. A match returns to the evaluator with the record, index
and byte offset in the guest frame. On resume the body advances that
cursor and reads the current count, table address and requested name.
This keeps duplicate exports in definition order even if a callback
replaces the table.

**Function value** `bab40` (`functions/value.rs`). Saves `ecx`, `ebx`,
`ebp`, `esi`, `edi`; stores 0 through the value pointer first. The
result byte is the top byte of the saved `ecx` slot, so `ecx` at return
carries it. Order: the scripted override `10aac0` when body `+c0` bit 9
is set (`ebx` the object, `(name, value, &[stack 2])`); the built-in
names (none or "one" store 1.0, "zero" stores 0); for a valid handle the
import `108d90` (`ebx` the value pointer, `edi` the result byte); then
every export of the definition (`+64` count, `+68` records of `20`
bytes, the name at `+8`) through `ba8c0`, each match storing its result
byte, all matches evaluated; then, when body flags bit 26 is set and
the parent (`+14`) is valid, the parent's value (`bab40`, tail-like:
its registers are returned). `esi` holds the header offset until the
exports, then the export index; `ebx` the record offset; `edi` the
definition. Before the exports the value pointer's argument slot is
overwritten with the body.

**Export evaluator** `ba8c0`. A `10`-byte frame below the saved `ebx`,
`ebp`, `esi` (`edi` pushed after the override): the value at `E-0c`,
the time input at `E-8`, the hash at `E-4`, "input is time" (the byte
the record's `+18` points at is 3) at `E-0d`, "not a range" (record
flags bit 1 clear) at `E-0e`; the low byte of the object's argument
slot is the active flag. Steps: the override `10aac0`; the input import
`108d90(object, record +4)`; the function `13bb90` (record `+14`, input
inverted when flags bit 0) or, for a time input, game time
(`[510c54]`: ticks × seconds per tick) plus, with flags bit 3, a hash of
the handle through the x87, then `13bb90` scaled by the value; a scale
`bab40(object, record +1c)`; a threshold (record `+10`, when positive);
flags bit 2 forces active; a gate import `108d90(object, record +c)`;
the result clamped to `[0, 1]`, or 0 when inactive.

**Import dispatcher** `108d90`. Zeroes `[ebx]` and `[edi]`, keeps the
object in `ebp` and the ancestry entry in `esi` (saved), and calls slot
`+4c` of each level as `(object, name, ebx, edi)` until `al` is
non-zero; at the list's end with no answer it zeroes both again. `eax`
starts as the body with `al` cleared; `ecx` is the slot, `edx` the name.

**Import handlers.** The slot `+4c` shape, `ret 10`. An unknown name
returns `al` 0 and stores nothing; an answered one stores the `f32`
value and the byte "value > 0" (1 where noted).

| Body | Keeps | Answers |
|---|---|---|
| `bcc20` base (`base.rs`) | `esi` the body and `bl` across every call; the value at `E-4` (the saved `ecx` slot), the result byte at `E+4` | constants, body `+ec`, `+f0`, `+f4`, `+f8`, `+f0 − 1`, the `+10a` bit 2 flag, the model variant fraction, a hash of the handle, a facing angle (`b8bd0`, x87 `fpatan`, `bff60`), the world's `[4de2f4]+70..7c`, and callouts `bca70`, `bcbb0`, `bcad0`, `bcb10`, `bcb60`, `c18e0`, `1588b0`; the component names go to `bc970`, and its answer in `bl` makes active 1. Most values clamp to `[xmm2, 1]`; after `bca70`, `1588b0` and `bc970` retail reads `xmm2` as the callee left it |
| `c6b90` unit (`unit.rs`) | `ebx` saved, `bl` 1 across `d1210`, `d1410` | body fields and flags of the unit, `d1210`, `d1410` and `1 − d1410`; two names ask the held weapon (`+212` index into `+218`) through `ff5f0` with a weapon name in `esi` (saved), returning its `al` |
| `d1210` (`unit.rs`) | a leaf, no stack | 0 unless body `+334 > 0`; with `+338` zero, `+334 /` definition `+1d4` (1.0 when that is not above); else `(+334 − +338) /` definition `+1dc`, or 0 |
| `d1410` (`unit.rs`) | `esi` the animation state (saved) | 1.0 for none; by the state's kind `+7c`: `1c6e30(esi)`, or `1 −` it, or 0 or 1 when the state is empty |
| `de3b0` biped (`biped.rs`) | `esi` the name (saved) | a state byte `+3dc` equal to 2, a flag byte `+39d`, and the length of body `+88` (x87 `fsqrt`) over definition `+2dc` |
| `f6d10` vehicle (`vehicle.rs`) | `esi` the body, `ebp` the definition (the tag of body `+0`), `edi` the name (a binary-search tree of signed compares; `sub edi` chains leave it zero), `bl` set when the name is active at any value; scratch floats at `E-18`..`E-1` and in the object and name argument slots once read | ratios `f78b0 (x, y) -> xmm0` of a speed-like quantity (`24590`, a vector length, x87 dot products of body vectors, `11d580` differences) over a definition limit (`f78e0` by index, or a field); flag tests `f5d70`, `e6fe0`, `f8070`; `d1210`. Clamped to `[0, 1]` at `f785a`; an unknown name returns `al` 0 and stores nothing |
| `ff5f0` weapon (`weapon.rs`) | `ebp` the body, `edi` the object, `esi` the name or a trigger cursor, `ebx` the definition or a state cursor; the definition over the saved `ecx` slot | barrel, trigger, magazine and flag values; `102070`, `10cf50`, `162b10`, `162b70`. Answered names end at `ffc28` with `eax` 1 |
| `fbe70` projectile (`projectile.rs`) | `bl` 1 | flags `+12c` bits 1, 3, 8, body `+158`, `+168` over definition `+e0`, and `(+168 −` definition `+18c) ×` definition `+194` clamped (1 with flag bit 8, or definition `+17c` or `+180` zero) |
| `106d70` device (`device.rs`) | `bl` 1, `xmm1` 0, `xmm2` 1 | the power and position values (`+134`, `+140`), a timer over definition `+10c`, the powered test through the device groups at `[4e0328]`, and two x87 ratios |
| `11bd60` machine and crate (`machine.rs`) | `ebx` saved, `bl` 0; `esi` the component record | the component's import `1d3550 (record: esi, name: ecx, value: edx, active: stack 0) -> al`, `ret 4`; `al` 1 when it answered |

**Change colours** `be8e0` (`colors.rs`): the per-function port, as a
body. Frame and registers as `sim_physics::colors`; `bab40` for each
permutation and scale function, `131c20` for each permutation.

**Callees.** Slice 3's guest callouts and their cleanup: `10aac0`
`ret 0c`; `13bb90`, `f78b0` `ret 8`; `b8bd0`, `bff60`, `1588b0`,
`bc970`, `102070`, `162b70`, `1d3550`, `f78e0`, `f8070` `ret 4`;
`131c20`, `bca70`, `bcbb0`, `bcad0`, `bcb60`, `bcb10`, `c18e0`,
`10cf50`, `162b10`, `1c6e30`, `f5d70`, `e6fe0`, `24590`, `11d580` `ret`.

**Not yet a leaf.** The evaluator becomes a leaf kernel (section 5 of
the design) only when every callee it can reach is Rust. Function
lookups `13bb90`, the scripted override `10aac0`, the colour
interpolation `131c20`, and the type handlers' callees above are still
guest code, so every slice 3 body is a segment.
## Slice 4 bodies

Each is **C**: read at its address with `h2 disasm`, and compared with
the lifted original by `tests/sim_objects_differential.rs` (each entry
as a root over a pool dressed with a definition, node matrices on every
body, a physics component and an attachment block, and `bc820` over the
same pool). Each body file's header records its frame and the order of
its calls; this section records what a caller or a later slice relies
on.

**Shared rules.** The bodies keep all eight integer registers, the `xmm`
registers and MXCSR as retail leaves them at every call and return
(scalar SSE through `sim_objects::sse`). The arithmetic flags are
reproduced at every return from the last instruction that sets them
(`add esp` at the end of `bdef0` and `bc5e0`, a `test` or `cmp`
elsewhere, the callee's after a call that ends the function): `b7430`,
`b7740`, `b9dd0`, `b9fc0` and `ba1d0` are called from all over the
image. `push`/`pop` pairs around a lookup (`b9dd0`'s `edi`, `b7740`'s
`ebp`) are made, so the dead stack holds what retail leaves there.

**Node-matrix rows** (`xform.rs`). `bdef0`, `b9dd0` and `b9fc0` inline
one sequence: each component `(m_z·z + m_y·y) + m_x·x (+ t)` through a
`34`-byte node matrix (`+0` scale, `+4` forward, `+10` left, `+1c` up,
`+28` translation), `x`, `y`, `z` in `xmm0` to `xmm2`, the row in `xmm3`
and `xmm4`. A point is first multiplied by the matrix scale when
`ucomiss scale, [45dbc0]` (the image's 1.0) is not equal; `lahf` then
leaves `ah` changed in `eax` where retail's does. `sim_objects::bounds`
is the same arithmetic as a pure kernel (exact or declining, through
`sim_core::det::Fp`), for a speculated pass.

**Bounds `bdef0`** (`bounds.rs`). Stores the first sphere (`+30`,
`+3c`) and the second (`+50`, `+5c`) on every path except a crate's
component answer, which supplies the first; then the registered sphere
(`+40`, `+4c`) only when it stores (not in the partition, or moved).
Moved is `comiss` of the squared distance against `[45e02c]` (`ja`); a
changed radius is `ucomiss` (unordered counts as changed). Callees:
`1d44f0` (`ret 0c`), `b87b0` (`ret 4`), `b8600` (`ret 8`).

**Node matrices `bd090`** (`pose.rs`). The port
`sim_physics::node_matrices` run as a body (design section 6: phase 1
owns the arithmetic, this phase the orchestration). Its sixteen sites
are the body's; the after-root dispatcher `1090d0` fuses. The pose
matrix `1421f0`, the composition `142a60` and the root transform
`bdc40` run inline through `sim_native`, so they are no callouts: the
harness steps them on the lifted side as part of the tree (`INLINED`).
The skeleton walk from node 1 is phase 1b's: `sim_anim::build_matrices`
through `engine::sim_anim::skeleton`,
with the port's per-node loop as the fallback when it declines
([skeleton-walk.md](skeleton-walk.md)). The frame and the callouts here
are the same either way.

**Attachment refresh `bef30`** (`post.rs`). Loops over siblings by
storing the next sibling over its own object argument (`[esp+14]`) and
jumping back, and recurses into the first child through its own entry
(fused). Callees: `b9ce0` (`ecx` object, `ret`), `c3220` (`eax` light,
`ret`), `c2d00` (`ret 4`), `c1720` (`ret 0c`).

**Node-matrix notify `be1d0`** (`post.rs`). `push ecx` makes the count
local `b8c00` fills; `109140` gets `(count, matrices)` with the object
in `ebx`.

**Component transform `bc5e0`** (`post.rs`). Callees: `1d1c10` (`esi`
record, `ret`), `1d1ca0` (`edi` record, `ret`), `1d4360` (`edi` record,
`ret 4`), `b91d0` (`eax`, `esi`, `edi`; `ret`), `b7430` and `b7740`
(bodies, fused), `1d09d0` and `1d0ad0` (`eax` 0, `edx` record, `ecx`
out, kept: `bc5e0` passes it on to `b7740`; `ret`), `1d96d0` (`eax` handle, `ret`). The network mode test is a
byte `cmp al, 4`.

**Setters and getters** (`transform.rs`). `b7430` keeps a one-dword
local (`push ecx`) whose byte 3 is the result; its position check is six
`comiss` against `[45e4ac]` and `[45dffc]`. Callees: `b87b0`, `b58c0`
(`ret 8`), `1c4a80` (`ret 8`), `bd020` (`ret 4`), `b8600`. `b7740`
calls `b58c0`; `ba1d0` calls `1d09d0` and `1d0ad0`. `b9dd0` and `b9fc0`
call nothing.

### Not verified for slice 4

- Captures and run parity (the slice's testable result): registers and
  RAM at the n-th call of `bc820` on Waterworks and a campaign level,
  compared with the lifted build. The harness covers
  synthetic pools.
- The harness compares `xmm` registers at callouts for the slice 4 roots
  except `bd090`, whose port was differential-tested without them
  (`tests/sim_physics_differential.rs`).
- Ret sizes of `b9ce0`, `c3220`, `1d1ca0`, `b91d0`, `1d09d0`, `1d0ad0`,
  `1d96d0`, `e1670`, `bf5a0`, `bfc90`, `bff10`, `bfe80`, `bfed0`,
  `bfe20`, `b6780`, `1c6440`, `a98b0`, `a9940` are read from their
  first `ret`, the rest likewise and checked against their arguments.


## Related

- [object-update.md](object-update.md): entries, callouts and code paths.
- [sim-rewrite-objects.md](sim-rewrite-objects.md): the design and slices.
