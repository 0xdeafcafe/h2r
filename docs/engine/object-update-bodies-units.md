# Object update bodies: units, weapons, vehicles and bipeds

Status: Built (native only). The bodies of slices 6a, 9, 10, 6c, 7a and 7b: the unit update, the weapon, the vehicle and the biped. Contract page for [sim-rewrite-objects.md](sim-rewrite-objects.md) phase 3; entries are in [object-update.md](object-update.md).

## Slice 6a bodies

On the look path of `cba50`, `sim_objects::unit::look_up` completes
`frame_up × aim` and evaluates `aim × left` in packed binary32 lanes.
The adapter reconstructs the six XMM outputs at the normaliser call.
The scalar SSE path handles exceptional operands, out-of-range
intermediates and float environments outside the packed path’s proof.

Each is **C** for its control flow and accesses: read at its address
with `h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (the four roots below, and the type
dispatchers and walks, which reach `c6b30` through the unit types' `+40`
slot). Field names are ours and **L**. `unit.rs`'s header records each
body's frame and the registers it keeps across calls.

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR
and the x87 state are retail's at every call and return. Flags are left
undefined at every transfer of `c60c0`, `cba50` and `c85c0`: no caller
and no callee reads them before writing them (`scripts/sim-liveness.py`).
`c6b30` and `c7840` reproduce them, as the ports they come from do. x87
sequences (the `_ftol2` arguments, the six "moved" tests, the clamp's
`fpatan`, `fsqrt`, `fsin` and `fcos`) run through `x87::abi` on the
guest's x87 state, with the last instruction and data pointers retail's
last x87 instruction leaves.

**Shell `c6b30`.** Calls `c49b0`, `c58f0` and `c60c0` with the unit on
the stack, then `c5eb0`, `c6740`, `114c60`, `c6990`, `c6810` and `c50a0`
with it in `eax`; `al` is the OR of the first five answers. `c60c0`,
`c49b0`, `c58f0` and `c5eb0` ("Slice 6b bodies") and slice 6c's `c6740`,
`114c60`, `c6990`, `c6810` and `c50a0` are bodies (fused). The port it
comes from (`sim_physics::objects::unit`) is an oracle.

**Movement `c60c0`.** Returns 0 at once when definition `+bc` bit 10 or
body `+10a` bit 2 is set. Otherwise it moves the aim (body `+168`)
toward the desired aim (`+15c`) and the look (`+18c`) toward the desired
look (`+180`). With both of a pair's rates zero (definition `+144`,
`+148`; `+150`, `+154`; times `+14c` when body `+1bc` is 1) the
direction is copied and clamped (`cba50`, fused; for the aim only when
`c4970` answers) and its velocity (`+174`, `+198`) set from `[4687a4]`.
Otherwise `c7840` turns it in the unit's frame (`b9fc0`) when
the limits record's (body `+ [body+33e]`) bit 5 (aim) or 6 (look) is
set, or `11f0d0` turns it in the world. The aim's two rate fractions
(`11ce20`, `1201a0`, clamped to 0..1) become the bytes at `+243` and
`+1f8` through `_ftol2` (`322b0c`), which pops the x87 stack. When any
of the six components moved by more than `[45dbdc]` the unit is woken
for the post walk (`bba20`, fused) and `al` is 1.

**Clamp `cba50`.** Returns 0 when the limits record's bit for the
direction (5, or 6 for the look) is clear. Otherwise it takes the unit's
frame (`b9fc0`; for the look re-based on the aim, normalised by `30bf0`
and `11d000`), builds a matrix from it (`1420f0`), takes the direction's
yaw and pitch in it and clamps them to the record's ranges (`+4..+10`,
or `+14..+20` for the look). When either clamped it writes the direction
back from the clamped angles. The x87 compare of the yaw against its
minimum leaves the x87 status and pointers that a later x87 user sees.

**Idle test `c85c0`.** 0 when the unit has a parent (body `+14`), when
bit 26 or 27 of its record word (body `+ [body+346] + 4`) is set, or when
any of seven `e6fe0(unit, n)` tests answers; then 0 when a held weapon
(the two index bytes at body `+212` into the handles at `+218`) answers
`100880`. Else 1. The result is also stored in the top byte of the
caller's `ecx`, saved and restored in the frame, so `ecx` returns with
it.

**Aim `c7840`.** `sim_physics::unit_aim::step` computes joint-space
direction and velocity with scalar SSE and x87. It returns a call or
return to its driver. The object update can fuse the step when it owns
aim. With the animation subsystem selected, the object update emits the
call and `sim_anim::unit_aim` plans and commits the segment. Both
drivers use the guest frame across all nine return sites.

### Not verified for slice 6a

- Captures from Waterworks and a campaign level: registers and RAM at
  the n-th call of `c6b30`, `c60c0`, `cba50`, `c85c0` and `c7840`,
  compared with the lifted build. The harness covers
  synthetic pools only.
- The harness compares the eight integer registers, the flags, memory,
  `xmm` and the x87 state at callouts. `_ftol2`'s stub pops the x87
  stack without storing the truncation, so the `+243` and `+1f8` bytes
  are checked only against stub answers.
- That the x87 crate's transcendentals do not depend on the rounding
  mode is read from its source, not checked in a test.

## Slice 9 body

**C** for its control flow and accesses: read at `fdfb0` with
`h2 disasm` (`fdfb0..ff576`, 77 calls, five jump tables), and compared with the lifted original by
`tests/sim_objects_differential.rs` (root `WEAPON_UPDATE` over the
slice 11 dressing plus a weapon definition, two triggers and two
barrels in every state the tables reach; and inside the type
dispatcher and the walks), which compare the float state at every
callout like every root. Field names are ours and **L**.

**One body, three files** (`engine::sim_objects::weapon`). A body is
one row in `BODIES`; its arms are split by state group and the
dispatcher in `weapon/mod.rs` picks the file by retail address:
`state.rs` (`fdfb0..fe5ff`: the frame, the owner, the animation block,
the timers, the charge toward body `+190`, the heat at `+180..+188`,
the effect at `+244`, the definition's `+2c0` entries), `triggers.rs`
(`fe5ff..fefee`: the trigger loop, states 0 to 7 through `ff57c`,
`ff59c`, `ff5b4`, `ff5c4`) and `barrels.rs` (`fefee..ff576`: the barrel
loop through `ff5d4`, heat, spin and the return). Each file's header
records its fields; `weapon/mod.rs` records the frame and the registers
kept across calls. The jump tables are read as retail-address arrays
(their targets, not their bytes).

**Frame.** `sub esp, 38`, then `ebx`, `ebp`, `esi`, `edi`. With `S` the
`esp` after the pushes: `[S+13]` the result, `[S+24]` the header offset,
`[S+28]` the trigger or barrel index, `[S+2c]` the definition, `[S+30]`
the owner's handle, `[S+34]` the body pointer as read at entry,
`[S+38]` `d1490`'s answer, the object at `[S+4c]`. The barrel loop
indexes the body through `[S+34]` even after a callee moved the body,
as retail does; the state group and the trigger loop re-read it through
the header (`[S+24]`) where retail does.

**Floating point.** Scalar SSE through `sim_objects::sse`; no XMM value
is live across a call. Two x87 sequences run on the guest's x87 state
through `x87::abi`: the heat value `fld; fadd; fmul` left in `st(0)`
for `_ftol2` `322b0c` (FIP `ff404`), and the spin clamps `fld qword;
fld dword; fcomip; fstp st(0); fld; fstp dword` (the stores at
`ff46b` and `ff508`, into `[esi+20]`). `fcomip` sets ZF,
PF and CF and clears OF, SF and AF, as the `jbe` after it reads.

**Callees**, every one a callout, with the bytes `ret` pops and the
register arguments retail passes: `101690` (4), `d1490` (0; `eax`,
`esi`, answers in `xmm0`), `101fb0` (0; `eax`), `badc0` (4; `eax`),
`105e80` (4), `1daea0` (4; `edx`), `1cb710` (`14`), `105740` (0; `eax`),
`105800` (0; `ecx`), `10d4e0` (0; `edx`), `105840` (0; `ecx`), `1039a0`
(`c`; `ecx`), `101f20` (0; `eax`), `c86e0` (4), `105fa0` (4), `1685a6`
(8; `ecx`), `103a60` (0; `eax`), `177610` (0; `eax`), `177260` (4;
`edi`), `fdd50` (8); in the trigger loop `102540` (`c`), `102f70` (0;
`eax`, `cx`), `106280` (8; `eax`), `103c40` (4; `eax`), `103a90` (4;
`eax`), `103ce0` (4; `ecx`), `1027b0` (`c`; `ecx`), `103b10`, `103bd0`
and `103b70` (4; `eax`), `102d60` (0; `eax`, `ecx`), `102e10` and
`102f10` (0; `eax`, `edx`), `1061c0` (8; `eax`); in the barrel loop
`1023d0` (4), `103e60` (4; `eax`), `102100` (`c`; `eax`), `102fb0` (8),
`103f60` (8), `103dd0` (4; `eax`), `ba7f0` (`10`) and `322b0c` (0;
`st(0)`, answers in `edx:eax`). None is a body yet.

### Not verified for slice 9

- Captures (the slice's testable result): registers and RAM at the n-th
  call of `WEAPON_UPDATE` from a Waterworks match (charging and
  overheating weapons, dual wielding, a vehicle turret) and from a
  campaign level, compared with the lifted build. The
  harness covers synthetic pools only.
- The x87 state at callouts: the harness compares integer registers,
  flags, XMM and memory, not `st(0)`. `_ftol2` takes its argument
  there, so a wrong heat value shows only in what `322b0c` answers into
  body `+175` in a real run.
- Callee `ret` sizes are read from the pushes at each call site and the
  callee's first `ret`.
- The register-liveness tables below do not cover this body's sites
  (`scripts/sim-liveness.py` lists slices 1 to 5); the body keeps all
  eight registers exact, so it does not rely on them.

## Slice 10 body

**C** for the control flow and accesses: read at `efde0`, `f1320` and
`f06c0` with `h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (root `VEHICLE_UPDATE`, and the type
update dispatcher over a vehicle), which compare the float state at
every callout like every root. Field names are ours and **L**. The
body file's header (`engine::sim_objects::vehicle`) records the frame,
the registers kept across calls and the order of the calls; this
section records what a caller or a later lane relies on.

**Frame.** The stack probe is a callout, as in `109580`; the body
resumes at `efdea` with `esp` 1984 bytes down, then pushes `ebx`,
`ebp`, `esi`, `edi`. All 39 return sites are at that `esp`. The object
argument is read back from the frame (`[esp+1998]`) where `ebp` was
reused for the matg record (`[[4e034c]+144]`), so the lifted code and
the body can take over from each other at any site.

**Flags and floats.** The body reproduces the arithmetic flags at every
transfer from the last instruction that sets them (rule 3), as the
slice 11 bodies do, through `sse.rs` and `alu.rs`. The scalar SSE work
(the component's rate, the mean of the contact records' floats into
body `+3fc`, the drag and pull toward the structure's point, the timer
at body `+2c4`) runs on the guest's `xmm` registers, keeping the upper
lanes and MXCSR as retail does. Three x87 sequences run on the guest's
x87 state with their last-instruction pointers: the two loads before
`372d48` (`fld qword [45e610]`, `fld dword [game time +4]`; the callee
returns in `st(0)`, stored by `fstp` at `f01f2`), `fsqrt` at `f02f5`,
and the `fabs`/`fcomip` test at `f04e0`..`f04f2`.

**The type physics.** Definition `+1f0` (`i16`) selects, through the
seven-entry table at `f069c` (read from the image), `f1cd0`
(`VEHICLE_TREAD_UPDATE`, `ebx` the object), `f1f80`
(`VEHICLE_WHEEL_UPDATE`), nothing, `f2360`, `f3010` (with `[S+14]`),
`f4800` or `2056e0` (`VEHICLE_CONTACT_SOLVE`); above 6 none. These stay
callouts, so the Halo 1 drive model's hooks on them run as before.

**Callees**, every one a callout except the leaves, `bba20`, the unit
action update `e6830` and the unit animation state `111650` (slice 6c, fused), with the
bytes `ret` pops: `320560` the probe; `1d2460`, `efc30`, `ef870`,
`efb60`, `205510`, `1cfb90`, `ba3d0` (a tail jump into `b7360`),
`f06c0`, `f6010`, `cc380`, `372d48`, `113e40`, `b9ef0` (0); `b9b90`,
`ba160`, `ba1d0`, `f1cd0`, `f6bd0`, `b77d0`, `d6660`, `e6830` (4);
`1d24a0`, `ef4f0`, `f6670`, `205be0`, `f1f80`, `f2360`, `f4800` (8); `f3010` (`c`); `ba7f0` (`10`); `2056e0` (`14`); `d7b80`
(`18`).

**Not ported.** The rest of the tree stays guest: the type physics
`f1cd0`, `f1f80`, `f2360`, `f3010` (162 samples on Waterworks),
`f4800`, `2056e0`; the contact gather `205be0`; `ef4f0`, `ef870`,
`efb60`, `f6010`, `f6670`, `f6bd0`; and `eec90` (175 samples), which
`efde0` does not call directly. Each is a later lane's body; the
segment keeps every frame slot and register they read.

### Not verified for slice 10

- Captures from a match with vehicles (the slice's testable result):
  registers and RAM at the n-th call of `VEHICLE_UPDATE`, one per
  vehicle type the table dispatches (tread, wheel, the flier types,
  turrets), on the ground and woken by an early mover, compared with the lifted build. The harness covers synthetic pools,
  with stubbed callees.
- The harness does not compare x87 state at callouts. `372d48` takes
  its two operands on the x87 stack and returns in `st(0)`; the body's
  x87 sequences are checked only through what they store.
- Callee `ret` sizes are read from each callee's first `ret` (`ba3d0`
  from the `ret` of `b7360`, which it tail-jumps to).

## Slice 6c bodies

Each is **C** for its control flow and accesses: read at its address
with `h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (a root per body over the unit
dressing, the shell `c6b30` fusing its five slice 6c children, and the
vehicle roots, which reach `111650`). Field names are ours and **L**.
The headers of `unit_rest.rs` and `unit_anim.rs` record each body's
frame, the registers it keeps across calls and the order of its calls;
this section records what a caller or a later lane relies on.

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR
and the x87 state are retail's at every call and return, and every
store retail makes is made, stack slots included. Flags are left
undefined at every transfer: no entry and no call site of these bodies
reads them (`python3 -I scripts/sim-liveness.py 0xc6740 0x114c60 0xc6990
0xd0620 0xd0690 0xc6810 0xc50a0 0x111650`). Scalar SSE runs through
`sse.rs` on the guest's registers. The x87 work is three `fld`/`fistp`
pairs (`114cce`, `c5214`, `c5281`) under the guest's control word, with
the last-instruction pointers the `fistp` leaves. `111650` touches no
x87 state.

**Timers `c6740`.** Counts down body `+2e8` (zeroing `+2e4` at 0) and
`+2ca` (at 0, the deferred injury notice `1c9c80(unit, +2c8, +2cc, 0,
1)`, then `+2c8` = 0, `+2d0` = none, `+2cc` = 0); either running makes
`al` 1. Then the byte `+1f4`: at 0, `cffc0` (`edi` the unit); while it
runs `al` is 1.

**Dialogue `114c60`.** The dialogue block is body `+ [body+342]`. Its
timer `+3c` is refilled from the tick rate (game time `+2`) through
`fld`/`fistp` when `+3e` is left; `+42`, `+40`, `+4e`, `+4c` (clearing
the handle `+54`) and `+50` count down. While a line plays (`+c`
positive) it starts once (`+48`: `20b140` without a sound `+1c`,
`1891d0` with the record `[E+c..18]` for the handle `+54`, `20d220`),
calls `20d570` once before and once after (`+49`), `20fd50` with a
sound, and clears `+c`. Last, body `+208` moves toward zero by a step
clamped between two constants. The Halo 1 search dialogue hooks this
point (`before`), so the body is guarded under Halo 1.

**Team `c6990`, seats `d0620` and `d0690`.** Unless body `+10a` bit 2,
the team word `+138` is the player's (`+13c`; `i8` at `+c0` of its
`21c`-byte record), else the actor's (`+12c`; `+24` of its `888`-byte
record), else for a vehicle (`+aa` 1) the seated units' teams through
the seat iterator in the frame (`E+4..1c`): a unit whose seat record
(vehicle definition `+1cc`, `b0` bytes, bit 11) is flagged gives one
candidate, the others another; the team is that one when they agree or
the flagged one is none, else none. `d0620` climbs from the unit to its
top biped or vehicle ancestor (while body `+4` bit 26 is set), storing
the 1 it shifts over its saved `ecx`; `d0690` walks that ancestor's
children (siblings `+c`, children `+10`, entering a biped or vehicle
with bit 26) and stores the next biped or vehicle (`+10`), its seat
(`+14`, body `+1fc`) and its parent (`+c`). Its found byte is the top
byte of its saved `ecx`, so `ecx` returns it. Both are leaves: no call.

**Camouflage `c6810`.** In the campaign with the Assassins skull, an AI
unit whose team is none, outside `0..10`, or not friendly to the local
player's (`bfe60`, `al`) gets body `+134` bit 3. Outside the campaign the
body passes the mode test only where `engine::skull_gate` answers it for
Assassins (a Firefight host, [skulls.md](../game/skulls-modes.md#the-campaign-gate)),
as the lifted code does with the hook at its `jne` (`c683f`). The level `+2b0` moves
by `+2b8` times the tick toward 1 (bit 3) or 0, clamped; reaching 1 with
an effect at `+d4` calls `b58c0(+d4, mask)`. The level `+2b4` moves by
the tick times a constant toward 1 (bit 4) or 0, clamped.

**Light `c50a0`.** For a player's unit in the campaign only. Without
the player record's flag `+88` and without the Envy skull, the toggle
(body `+148` bit 2) switches `+134` bit 29 with `c5890(4)` or
`c5890(5)` (the latter starting the timer `+268`); the timer counts
down by the tick, and bit 29 clears when the unit has a parent, is
dying (`+10a` bit 2), or the timer ran out while the game runs
(`[[5107e8]]`). The level `+264` then moves toward 1 or 0 by one tick
over a tick-rate-scaled time. With the flag or Envy: the camouflage
toggle through `c5740(unit, 1)` and `+134` bit 15, and the timer `+2be`
ending in `c5890(3)` (`+2bc` 2, then cleared) or `c5740(unit, 0)`
(`+2bc` 1). The Halo 1 flashlight and scope hook this point
(`before`), so the body is guarded under Halo 1.

**Animation state `111650`.** It picks two animation names: A (against
the animation block's `+70`) and B (against `+7c`), from the record
argument (`+0`, `+4`, each unless none or 0; with B its byte `+8`),
else from defaults: with no parent, a name `10eef0` gives (`dl` the
record's byte `+9`), fixed names for a dying unit (`+10a` bit 2) or one
whose byte `+1f4` runs, and the limits block's (`body + [body+33e]`)
`+2c` when set. When the animation block (`body + [body+12a]`) plays
(`+0`, `+6` not none) it calls `1cb710`, `e6a20` and `1c6440` (whose
answer's word `+14` must exceed 1), then `e6b00` for the names, and a
signed `cmp` ladder on B before chooses the seat vector (`bfc90`,
`1c7380`, `10eea0`, `10ee70`, `13e420`, `ba160`, `1c73a0`, `142570`,
`b8bd0`, then `e5840`), the three channel marks (`or byte [ebp+11|31|51],
1`), `e4300`, `11b710`, or the kill test (`10f5f0`, `a7670`, `d6bc0`).
Then `111010` and, with a parent, `c7070` and the parent's seat flags.
A change of names restarts them (`1103a0` unless given, `10f430`,
`1ccba0`, `10ee20`); otherwise once in 16 ticks per unit (the counter
`[55e70c]`) `1ce180` and `1060a0` for each held weapon (`+218..224`).
The limits block's three channels (`+dc`, `+9c`, `+bc`) each get
`1c68c0`, `e6a20`, `11b710`, `e6b00`, `110fc0`, `1c63f0`, `114330` and
`ba350` as their flags ask. `al` is the OR of `113910`'s, `10dc70`'s and
`10eaf0`'s answers and a channel playing. The string IDs it compares and
stores, and the `imm32` arguments it pushes, are read from the
instructions that carry them.

**Callees**, every one a callout, with the bytes `ret` pops: `cffc0`,
`c5890`, `20fd50`, `113910`, `1c6440`, `bfc90`, `1c7380`, `10eea0`,
`10ee70`, `e4300`, `10f5f0`, `a7670`, `111010`, `c7070`, `1ccba0`,
`10ee20`, `1060a0`, `1c63f0`, `114330`, `110fc0`, `10dc70`, `10eaf0`
(0); `bfe60`, `c5740`, `1891d0`, `10eef0`, `142570`, `b8bd0`, `e5840`,
`11b710`, `ba160`, `d6bc0`, `ba350` (4); `b58c0`, `20b140`, `20d570`,
`e6a20`, `13e420`, `1c73a0` (8); `20d220`, `e6b00`, `1103a0`, `1ce180`,
`1c68c0` (`c`); `1c9c80` (`10`); `1cb710` (`14`); `10f430` (`1c`).

### Not verified for slice 6c

- Captures from Waterworks and a campaign level (the slice's testable
  result): registers and RAM at the n-th call of each body, compared
  compared with the lifted build. The harness covers synthetic
  pools with stubbed callees; the player, actor, skull and dialogue
  paths are reached only with dressed fields.
- `111650`'s field names and what its callees do are read from data
  flow, not from the callees. Its callees' `ret` sizes come from each
  callee's own `ret`.
- That the stub answers reach every arm: `111650`'s ladder is driven by
  the names read from the image, so which arm a case reaches depends on
  their order, which the test does not assert.

## Slice 7a bodies

Bipeds and vehicles share the unit action update at `e6830`. The signed
offset at unit `+346` locates the action record. Its bitmap starts at
`+4`. The body visits set bits in ascending order and calls descriptor
slot `+4` with the object and action index. On return at `e6890`,
a zero result clears that action in the current word. The scan then
reads the bitmap again so it sees changes the callback made to later
actions. The high byte of saved ECX records whether any callback
returned nonzero, and AL returns that result.

**C** for the control flow and accesses: read at each entry with
`h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (a root per body over the slice 11
dressing plus a biped's fields, a parent of each type reached directly
or through a link flagged bit 26, and the counters, flags and animation
names the tests read; and the type update dispatcher over a biped), which
compare the float state at every callout like every root. Field names
are ours and **L**. The module header (`engine::sim_objects::biped`)
records `dd360`'s frame, the registers kept across calls and the order of
the calls; each body's doc comment records its own.

**Frame of `dd360`.** `sub esp, 16c`, then `ebp`, `esi`, `edi` pushed;
`ebx` is pushed only once the biped has a physics component (body `+b4`),
so the early return pops three registers and every site after the first
is at `esp` four bytes lower. The definition at `[S+14]` and the
animation block at `[S+24]` are overwritten on the seated path (the
`fistp` out and its float spill), so the body reads them where retail
does and nowhere else. The record at `[S+18..20]` is built from two
words read from the instructions at `dd3df` and `dd3e7`; under a biped
parent its second word is computed from two more (`dd51f`, `dd524`).

**Flags and floats.** Every body reproduces the arithmetic flags at
every transfer from the last instruction that sets them (rule 3),
through `sse.rs` and `alu.rs`, as slices 10 and 11 do; `lahf` writes `ah`
on the two `ucomiss` tests (`dd572`, `e4cc5`). Scalar SSE runs on the
guest's `xmm` registers. Three `fld`/`fistp` pairs (`dd476`, `e409a`,
`146a0a`) run on the guest's x87 state under its control word and leave
the `fistp`'s last-instruction and data pointers.

**Stores that look dead.** `1469f0` stores the product over its
argument slot and the integer over its saved `ecx`, so `ecx` returns
the integer. `e4c50` stores its float argument over its saved `ecx`
slot, so `ecx` returns that float on the paths that call `e4d90`.
`de190` reads `[U+9c..a4]`, which only `b8d30` writes: when neither
marker is found the values are whatever earlier frames left there, and
the bodies make every stack store retail makes so they match.

**Callees** that stay callouts, with the bytes `ret` pops: `e3700`,
`e5930`, `1faf30`, `e6900`, `10f820`, `de080` (0); `e3380`, `e0ef0`,
`bfa40` (4); `e01d0`, `dc5c0`, `e3c90`, `e40d0`, `e4450` (8); `e24f0`,
`e4d90` (`c`); `10f430` (`1c`); and the shared `1e5af0` (4),
`b58c0` (8), `b8d30` (`c`), `30bf0`, `113e40` (0). Fused bodies in the
tree: `bba20`, `ba1d0` (slice 4), `cba50` (slice 6a), `111650` (slice
6c) and `e6830`.

**A split block.** Function discovery takes `de336`, the look update's
third clamp step, for a function start (`h2 analyze --explain de336`:
"found by TailCall", from the `jmp short` at `de32c` and `de331`), so
the lifter marks those jumps as tail calls. The body runs it as its own
block; the harness's lifted side steps on through it (`SPLITS`).

**Not ported.** The movement and animation steps `e01d0`, `dc5c0`,
`e24f0`, `e0ef0`, `e3c90`, `e40d0`, `e4450` (with `df380` below them;
`e0dc0`, also below them, is a slice 7b body), the seat updates `e3380` and `e3700`, and `e4d90`, `e6900`,
`de080`, `e5930`, `10f430`, `bfa40`, `1faf30`, `10f820`. Each is a later
lane's body; the segments keep every frame slot and register they read.

### Not verified for slice 7a

- Captures from a match and a campaign level (the slice's testable
  result): registers and RAM at the n-th call of `BIPED_UPDATE`, for a
  biped on foot, airborne, seated in a vehicle and riding a biped,
  compared with the lifted build. The harness covers
  synthetic pools, with stubbed callees.
- `1469f0` has callers outside the biped update (one returns to
  `167507`), and the leaves may have others; the bodies run there too
  when the switch is on. The harness enters them only as roots and from
  `dd360`.
- Callee `ret` sizes are read from each callee's last `ret` before its
  padding.

## Slice 7b bodies

**C** for the control flow and accesses: read at each entry with
`h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (a root per body over the slice 7a
dressing plus the post-update's fields: the state `+3dc`, the parent,
the crouch, jump, probe and contact counters and flags, a physics
component whose contact records (`+88`, `48` bytes each, `+8c` of them)
take every kind the switch reads, the structure's surfaces and a contact
object's model; and the type post-update dispatcher over a biped), which
compare the float state at every callout like every root. Field and
point names are ours and **L**. The module header
(`engine::sim_objects::biped::post`) records `dd990`'s frame, the
registers kept across calls and the order of the calls; each body's doc
comment records its own.

**Frame of `dd990`.** `sub esp, b8`, then `ebp`, `esi`, `edi` pushed;
`ebx` is pushed only once the biped has a physics component (body `+b4`),
so the early return pops three registers (`ddf46`) and the rest four
(`ddf45`, or the bit 0 exit after `ddf16`). `[S+18]` holds the crouch
step as a float, then its low byte the jump update's answer, and the
whole dword is pushed to `dd0d0`; `[S+14]` holds the crouch height's out
float, then the contact record. The output `1e5a60` writes at `[S+20..28]`
overlays the root velocity at `[S+1c..24]`.

**The contact switch.** The output's index is checked against the
component's count `+8c` with `jle`, so an index equal to the count reads
the record past the last; a negative index compares the count with 0.
The record's word `+4` is none, or a kind in bits 29 to 31: 1 a surface
index alone, 4 an object through the per-index table `[51e9cc]` and its
model's part surfaces (`1efb40`, then the shape `1efd80` answers), 5 a
structure surface through `[4e0348]` `+144` and `+13c`; any other kind,
and none, take the record's own object `+18` when it is in the world
(body `+4` bit 22). Each kind stores the five contact words `+354..364`
in retail's order.

**Flags and floats.** Every body reproduces the arithmetic flags at
every transfer from the last instruction that sets them (rule 3),
through `sse.rs` and `alu.rs`; the `inc`, `neg` and `cmp` idioms the
slice adds are in the module (`inc` keeps CF). The crouch height's
`ucomiss` takes its parity branch as retail does. Scalar SSE runs on the
guest's `xmm` registers; `e4330` takes its speed in `xmm2` and the
post-update hands it `[+45c]`, plus the component's `+14` when `1cfdf0`
answers. Five `fld`/`fistp` pairs (`e441a`, `e46e2`, `e2ffa`, `e30a5`,
`e3314`) run on the guest's x87 state under its control word and leave
the `fistp`'s last-instruction and data pointers.

**Read from the image.** The `.rdata` constants (`45dbc0`, `45dbd8`,
`45dbdc`, `45dbbc`, `45dbfc`, `45dc0c`, `45dc68`, `45dd30`, `45dd74`,
`44ae90`, `43ff08`), the down vector `[4687bc]`, `1201a0`'s `[4687b0]`,
the probe's animation name, time and collision flags (the immediates at
`e314e`, `e3159`, `e3237`, `e323e`), and `dd0d0`'s jump table `dd2e4`,
read by the state less 1. The table has no bounds check: a state of 0 or
above 6 jumps through whatever follows it in retail, and the body
returns an unowned fault there instead. The harness keeps the state in
1 to 6.

**Stores that look dead.** `e4330` stores body `+348` bit 6 at
`[E+3]`, which nothing reads. `dd990` sets the damage packet's word at
`[S+bc]` and its byte at `[S+c4]` before `d66d0`, and `dd0d0` reuses the
object's argument slot `[S+58]` for three flag bytes in turn and then the
early mover; the bodies make every stack store retail makes.

**Callees** that stay callouts, with the bytes `ret` pops: `1ec500`,
`1d1c10`, `1e5a60`, `1cfdf0`, `1201a0`, `1c9c00`, `1e20b0` (answers in
`xmm0`), `1d0870` (0); `d6660`, `b9b90`, `ba160`, `badc0`, `1efb40`,
`25aad0`, `e4770`, `1fb6f0` (4); `d66d0`, `1c4b00`, `1efd80` (8);
`114b60` (`c`); `d7b80`, `1697c0` (`18`); `1e68a0` (`28`). Fused bodies
in the tree: `baf80`, `b7360`, `bba20` (slice 1), `ba1d0`, `b7740`,
`b9dd0` (slice 4), `1469f0`, `e4050` (slice 7a). None reaches `bd090`, so
no root is in `FLOAT_OPT_OUT`. Function discovery finds no start inside
any of the ten (`h2 analyze --functions`), so none needs `SPLITS`.

**Not ported.** The AI hooks `1ec500` and `1e5a60`, the damage packet
`d6660`, `d66d0` and `d7b80`, the jump `e4770`, the physics step
`1e68a0` and the queries below the contact switch and the probe. Each is
a later lane's body; the segments keep every frame slot and register they
read.

### Not verified for slice 7b

- Captures from a match and a campaign level (the slice's testable
  result): registers and RAM at the n-th call of `BIPED_POST_UPDATE`, for
  a biped standing, crouching, jumping, landing and on each contact kind,
  compared with the lifted build. The harness covers
  synthetic pools, with stubbed callees.
- The leaves and segments may have callers outside the post-update; the
  bodies run there too when the switch is on. The harness enters them
  only as roots and from `dd990`.
- The `fistp`s assume the guest's control word masks the precision
  exception, as in slice 7a.
- Callee `ret` sizes are read from each callee's last `ret` before its
  padding.