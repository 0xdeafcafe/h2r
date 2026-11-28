# Object update bodies: damage, small types and lights

Status: Built (native only). The bodies of slices 8, 11, 5 and 6b: the damage update, the item, scenery, device, machine, crate, creature and projectile updates, the lights, and the unit state.
Contract page for phase 3 of the [simulation rewrite](sim-rewrite-objects.md); entries and conventions are in [object-update.md](object-update.md).

## Slice 8 body

**C**: read at `d5de0` with `h2 disasm`, and compared with the lifted
original by `tests/sim_objects_differential.rs` (root
`OBJECT_DAMAGE_UPDATE`, and inside `bc470`'s tree).

**Frame** (`engine::sim_objects::damage`). `sub esp, 70`, then `ebx`,
`ebp`, `esi`; `edi` is pushed only once the damage information `d5b60`
returns is non-zero, so the early exit (`d664f`) pops three and returns
`al` = `dl` (0). With `S` the `esp` after the four pushes: `S+13` the
changed byte (the result), `S+14` the actor's record or 0, `S+18` the
damage information, `S+1c` the section cursor and then the shield
ceiling, `S+20` the header offset and then float temporaries, `S+24`
the section count and temporaries, `S+4c` a 13-dword record zeroed with
`rep stosd` for `da110`, `S+84` the object. `ebp` holds the body from
the entry to the return and `edi` the object from `d5fe9`. The body is
the retail code block by block, keyed by retail address, so the lifted
original can take over at any of its 26 return sites and the reverse.

**Floating point.** `xmm3` holds the image's 1.0 from `d6095` across the
calls to `14de90`, `13a6e8` and `a7a30`, and `xmm1` holds 0 from `d6159`
into the regeneration and decay blocks; the body keeps every XMM
register as retail leaves it at each call. The section decay rounds
with `fld` / `fistp` (`d6551`, `d6555`) under the guest's x87 control
word, and leaves the x87 state as `fistp` does (FIP `d6555`, its FOP
and data pointer).

**Callees**, every one a callout, with the bytes `ret` pops: `d5b60`
the damage information (`ecx` the object; 0), `d9f70` (4), `da110`
(`14`), `dbc80` (`c`) and `d6bc0` (4) per section, `1e4a10` the actor
(0), `d66d0` the damage owner (8) and `d6800` the depletion (`10`) for a
pending kill (flags bit 13), `dbfb0` pending damage (`c`), `1588b0` a
player's recharge rate (4), `14de90` (0) and `13a6e8` (4) the
overshield's drain, `a7a30` replication (4), `1e9720` a team's rate
(0), `ba7f0` (`10`) and `176780` (`10`) the shield effects, `138880`
whether stun timers hold (0), and `d8b50` the recent-damage accumulators
(4).

**Events.** The function applies no damage itself: damage reaches an
object through the damage report `d9640` and a kill is credited at
`15ce70` (`PLAYER_KILLED`), whichever code calls them (`dbfb0` from
here, projectiles, melee, explosions). So `sim_bridge::sim_objects` records
the events with observers at those two entries, the same places the
Live telemetry's hooks read, and the events equal the hook-derived
ones over a match:

- `Damage` (`sim_objects::damage::damage`, at `DAMAGE_REPORT`): the
  report (`guest_layout::DamageReport`: damage code, flags, effect tag,
  owner player, object and team, direction, origin, and body and shield
  damage, L), the victim's handle, type, controlling player and position,
  and the game tick. A report that does not read records nothing.
- `Kill` (`sim_objects::damage::kill`, at `PLAYER_KILLED`): the attacker
  and victim players (`[esp+4]`, `[esp+8]`), the damage code (`[esp+10]`'s
  low byte, L), each player's unit and its position, and the tick.

The buffer (`sim_objects::events`, read through `sim_state::events` and
the agent API's `sim.events`) belongs to the game thread and is emptied
at `GAME_TICK`'s entry, so at a tick boundary it holds the tick that
just ran. Events are always recorded.

**The Halo 1 health model** ([health.md](../halo1/health.md)) hooks
this point with a `before` and an `after`. With both on, the installer
guards the body (see "Code paths"): `bc470` reaches it through the
runtime, the chain runs the `before`, the body through its shadow, then
the `after`, as it does around the original.

## Slice 11 bodies

Each is **C** for its control flow and accesses: read at its address
with `h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (the roots below, and the type
dispatchers and walks, which reach every one of them through the
descriptors' slots). Field names are ours and **L**. Each body file's
header records its frame, the registers it keeps across calls and the
order of its calls; this section records what a caller or a later slice
relies on.

**Shared rules.** The bodies keep the `xmm` registers, MXCSR and x87
state as retail leaves them, as well as the eight integer registers:
float arithmetic goes through `sim_objects::sse` (the `simd` model, or
the host's binary32 operation when MXCSR rounds to nearest with PE
sticky and both operands are ordinary), x87 sequences through
`x87::abi` on the guest's x87 state. A body stores into its caller's
argument slots where retail does: `f8de0` replaces the low byte of its
object argument with the mover flag and pushes the whole dword,
`118b80` replaces it with `118b10`'s `al`, `115a40` stores the record
base over it. Flags are reproduced at every transfer from the last
instruction that sets them; where that is a shift by more than one, OF
and AF are left undefined, as the IR does.

**Item `10b4d0`.** The early-mover follow for items woken by a mover
(body `+c1` bit 0), the fall and damping of body `+94..+9c`, the wake
for post (`bba20`, fused), and the garbage timer at body `+12e`: when it
reaches 0 outside network mode 4 the item deletes itself (`bc190`, an
effect when definition `+110` is set, `176780`, then `OBJECT_DELETE`
`b8540` on its own handle). The deletion is a callout in retail order;
the walk's slot scan reads the freed slot afterwards as retail does.
Every object then calls `10b850(object)`, which reads the early mover's
record ("Who reads it, and when" above).

**Scenery `10a2f0`.** Advances the animation block at body `+12a`
(`1cb710`) and, when the block's animation has more than one frame
(`1daea0`'s answer `+14`), wakes the scenery for the post walk.

**Device `106790`, machine `1152e0`, `115a40`.** The device update
moves the power (`+140`) and position (`+134`) toward their targets or
groups (`[4e0328]`, `c` bytes a group) and wakes the machine through
`bd020` and `115b10`; the machine update runs after it for machines
(type ancestry `obje`, `devi`, `mach`): a cycling machine's position
(x87 rate, `fstp` spill), a door's blocking test (the spatial visit
`bb050(1, 3, ..)` into a `10`-entry buffer, then `b8d30`/`b9fc0` per
result) and its cluster bit words. `115a40` is a leaf: exit `eax`,
`ecx` (the range end), `edx` and the flags as retail's.

**Crate `11bce0`.** With body `+c0` bit 12 and a physics component,
`1d24a0(component record, 1.0)`; unless body `+c1` bit 0, the tick
count into body `+bc`.

**Creature `118700`, `118b80`.** The AI calls stay callouts in retail
order (`1e5bb0` with 21 arguments, `1e6360`, `1e5af0`, `1e6120`,
`1e55d0`; after physics `1ec500`, `1e68a0`, `1e5a60`). `118700` keeps
`fld`/`fistp` for the timer at body `+204` on the guest's x87 state.

**Projectile `f8de0`.** Asks the waking mover for its displacement
(`109a00`, `ebx` = the velocity copy), runs the step `f8eb0` (a body,
so `drive` fuses the call), and adds the displacement to body `+88`
when `109a00` answered.

**Projectile step `f8eb0`** (`projectile_step.rs`). `1247` bytes, 39
calls. The body is retail's basic blocks keyed by address, one
statement per instruction, so every instruction that sets flags, an
`xmm` register, MXCSR or the x87 state is reproduced and the lifted
original can take over at any return site. Frame `sub esp, 118` and
four pushes; the file header records the slots. In retail order: the
attachment (`fd410`, `17c880`, `16ba40`, `fd460`) unless body `+12c`
bit 1; the timers at `+158..+164` and `+1aa`; then sub-steps while the
tick's time lasts (at most ten hits): the speed through x87 `fsqrt`;
homing on the target `+144` (`badc0`, `1e9700`, `f87f0`, `30bf0`, an
x87 `fsin`/`fcos` rotation, `b75a0`); drag (`24590`, `faa30`);
gravity through `[51e9c4]`; the collision `fc030` and on a hit
`1ca9f0`, `faa60`, `1ca690`; the flyby test per local player
(`1897c0`); the orientation from the velocity (`30bf0`, `11d000`) or
the spin; the new transform and velocity (`b75a0`, `b7740`, `1c4b00`,
and `b9b90`, `b7360`, `bba20` when it moved); the attachment's update
(`17bbd0`). After the loop, by the state word `+130`: detonation
`fc330` or deletion `OBJECT_DELETE` `b8540` (a callout in retail
order), or in network mode 4 the attachment refresh (`b9d20`, `bef30`,
`b8b70`).

Float registers across calls: `24590` returns the length in `st(0)`,
which the `fadd` after it consumes; `30bf0` and `1e9700` return in
`xmm0`, and `xmm1` holds a value across `1e9700` (`mulss xmm0, xmm1`
after it); `faa30` is entered with `xmm0` set. Callee cleanup: `fd410`,
`17c880`, `16ba40`, `30bf0`, `24590`, `faa30`, `11d000`, `b9d20`,
`b8b70` `ret`; `fd460`, `badc0`, `1e9700` (through its tail jump to
`1e96a0`), `f87f0`, `fc030`, `b7740`, `b9b90`, `b8540` `ret 4`;
`17bbd0`, `1ca9f0`, `1c4b00` `ret 8`; `b75a0`, `fc330` `ret 0c`;
`1897c0` `ret 10`; `faa60`, `1ca690`, `bef30` `ret 14`.

### Not verified for slice 11

- Captures from campaign (the slice's testable result): registers and
  RAM at the n-th call of each slice 11 point, on a level with machines
  and doors, creatures, scenery animations, crates, and items that reach
  their garbage timer, compared with the lifted build. The
  harness covers synthetic pools only.
- The harness's stubs answer in a float register only for `30bf0`,
  `1e9700` (`xmm0`) and `24590` (`st(0)`). Other callees that return a
  float in a register leave it as the caller had it, so a body's reading
  of their answer is checked only as far as that value goes.
- The projectile step's harness pool is synthetic: the stub for `f87f0`
  (the target's point) does not fill its out buffer, and the stub for
  `fc030` fills only the collision record's fraction (`+4`), so the
  homing and the rest of the hit arithmetic run on what the stack held.
  A capture from a level with homing projectiles and ricochets covers
  those values.
- Callee `ret` sizes for `1d24a0` and `1c6620` are read from their tails
  (`1c6620` jumps to `1c6560`, `ret 4`); the rest from their first `ret`.

## Slice 5 bodies

`sim_objects::light::blend` interpolates the two radius endpoints (light
record `+ec`) and the two intensity endpoints (`+c4`) in the same order.
It returns both the sum and the weighted second endpoint, which remains
visible in the caller’s XMM registers. The scalar SSE model handles
inputs outside the kernel’s proved range and tracks exceptions when the
guest precision flag is clear.

Each is **C** for its control flow and accesses: read at its address
with `h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (roots `c1400`, `c0c80`, `c0b00`,
`c2d00`, `c3220` and `c3260`). Field names are ours and **L**. The module
header of `sim_objects/lights/mod.rs` names the element fields; each body
file's header records its frame, the registers it keeps across calls and
the order of its calls. This section records what a caller or a later
slice relies on.

**Shared rules.** Integer registers, `esp` and the frame slots match
retail at every transfer. The arithmetic flags are undefined at every
call and return: the liveness table shows no flag reader at any slice 5
entry or site, nor after any call of these entries from `bef30`. Scalar
SSE goes through `sim_objects::sse`; the colour scaling in `c0b00` runs
on the guest's x87 state through `x87::abi`, so its last-instruction
pointer and data pointer are retail's (`c0c48` and the blue channel).
The light lists themselves stay guest: `1cac60` adds, `1cae40` removes,
`16ba40` frees the datum, `16bc00` is the datum iterator.

**Walk `c1400`.** Per live light, in index order: `c0c80(handle)`; a
linked light whose owner or marker parent is a live object with POST set
is removed (`1cae40`, when active) and relinked (`c2d00`); a timed light
past its definition's lifetime expires (`c3260(handle, 1)`). The array
header, high-water mark and bitmap are re-read after each light, so
lights a callee creates or deletes are seen as retail sees them.

**Record `c0c80`.** The body is the per-function port of
`LIGHT_RECORD_UPDATE` (kept as the oracle `oracle.light_record` in
`sim_physics::ORACLES`) with its transfers returned to `drive`, so its
calls to `c0b00`, `baf80`, `bab40`, `c2d00` and `c3220` are fused. It
links a light that turns on (with no owner, or when `b9ce0` answers no
and `b9d20` yes) only while the object globals' first byte
`[[4de2f4]]` is set, and unlinks a linked light that turns off. The body
takes every light through the same arithmetic, so the old placed-light
bypass has no switch of its own.

**Phase `c0b00`.** The phase is game time `+8` × `+4`, or a per-light
offset `((handle × 1387) & 7fffffff) mod (ticks per second × 3c)` added
to game time `+8`, × `+4`. A zero ticks-per-second is the guest's divide
error, reported as a fault at the `idiv` (`c0b3b`). Then up to three
function blocks: brightness (`13b390`, clamped and mapped unless the
function's flags byte `+1` has a bit of `f0` set), colour (`13b390`, then
`13bc00`, whose packed RGB scales the three channels), and two
`13bb90` values into `[out]` and `[out + 4]`. The colour bytes are stored
over the input's argument slot, high first, as retail does.

**Link `c2d00`, unlink `c3220`, expire `c3260`.** `c2d00` computes the
world position from the owner's node matrices or marker (`b8c40`,
`16640f`, `b8d30`) or a marker parent's node matrix (`142570`, then the
two local directions rotated by its rows in retail's operand order),
finds the cluster (`16a8e0`, `11bed0`, `ba300`) and adds the light
(`1cac60`), then sets flags bit 3. `c3220` removes an active light and
clears bit 3. `c3260` removes a linked, active light, updates the marker
parent's body flags `+4` bit 6 when `notify` is set (scanning the other
lights with `16bc00`), and frees the light (`16ba40`).

**Callee `ret` sizes.** None: `16bc00`, `bad50`, `17ca10`, `131c20`,
`b9ce0`, `b9d20`, `b8c40`, `c28b0`, `11bed0`, `ba300`, `16ba40`. `4`:
`13bc00`, `142570`. `8`: `1cae40`, `131d60`, `13b390`, `13bb90`,
`c0840`. `c`: `b8d30`. `10`: `16640f`. `1c`: `16a8e0`. `24`: `1cac60`.

### Not verified for slice 5

- Captures from campaign and multiplayer: registers and RAM at the n-th
  call of `c0c80` and `c2d00` on a level with lights that have a function
  source (owner node matrices and markers) and marker-parent lights,
  compared with the lifted build. The synthetic record fixture
  exercises a function source through both colour calls. Captures cover the
  owner-marker path (`b8c40`, `16640f`, `b8d30`) with real attachments.
- The harness answers `16bc00` from the synthetic bitmap; the iterator's
  own behaviour is not compared.

## Slice 6b bodies

Each is **C** for its control flow and accesses: read at its address
with `h2 disasm`, and compared with the lifted original by
`tests/sim_objects_differential.rs` (the four roots below over the
slice 6a dressing plus the fields they branch on, and the unit update
root, which fuses the first three). Field names are ours and **L**. The
body files' headers (`engine::sim_objects::unit_state`: `control.rs`,
`state.rs`, `seats.rs`) record each frame, the registers kept across
calls and what each function does.

**Transfers.** The eight integer registers, the `xmm` registers, MXCSR
and the x87 state are retail's at every call and return, and every store
retail makes is made, stack stores included (the `push 1` arguments of
`10f720`, the record `189210` takes, the flag word `c58f0` stores as a
word and reads back as a dword with the stale upper half). Flags are
left undefined at every transfer: `scripts/sim-liveness.py` shows no
caller and no callee of the four reading a flag before writing it (and
`d1080` reads only DF, which no body touches). x87: `c49b0`'s `fld;
fistp` (`c4c08`, `c4c0c`) and `ce0c0`'s three `fld; fabs; fstp`
(`ce254`..`ce2ec`) run through `x87::abi` on the guest's x87 state with
the pointers the last one leaves.

**Constants.** String IDs (`c4cf3`, `c4cfb`, `c4dfb`, `c4e03`, `c5c03`)
and the `0.5` pushed at `c59e1` and `c59fa` are read from the
instructions that carry them in the loaded image. Float constants are
read from their image addresses.

**Control `c49b0`.** The desired aim and look (`+15c`, `+180`) come from
`d0080`'s facing when body `+134` bit 21 is set, otherwise (bit 1 clear)
from the parent's seat node or the unit's own `+70`; then the timed bits
(`+1ec`, `+1f0`, `+148` bit 16 toggled every `[45dc20]` × ticks a second
ticks, `+1c0`), the two linked objects at `+248` and `+24c`, and the
`+334`/`+338` pair. The `idiv` by that tick count is the guest's divide
error when the count is 0; the body faults at `c4c19` as the lifted code
does.

**State `c58f0`.** Timers `+1f5`, `+1fa`, `+1fb`; unit states through
`e68c0`; held weapons' time stamps and states; `+23c` through `110ab0`
and `cbec0`; the idle test `c85c0` (a slice 6a body, fused) choosing
`+240` and the name `189210` plays; per held weapon `100430` with a flag
word from body `+148`; then `d1540`.

**Seats `c5eb0` and refresh `ce0c0`.** `c5eb0` calls `d0c10` and
`ce0c0`, then moves each of the definition's powered seats (`+1b8`
records at `+1bc`) toward 1 or 0 by one tick over the seat's time, into
body `+25c + 4n`. Its result is the top byte of the `ecx` it pushed, so
`ecx` returns with it, and the Halo 1 drive model's `before` hook on
`ce0c0` sets that byte through the return address `c5f04`. `ce0c0`
clears body `+134` bits 25 and 26, asks `d1080` for the control frame,
and stores the position's change (`+270..+284`), its changes in the
basis (`+294..+29c`) and their blend (`+288..+290`); bits 25 and 26 are
set when an absolute change passes a threshold. The drive model's hook
makes `UNIT_CONTROL_REFRESH` a guarded body: `c5eb0` reaches it through
the runtime, which runs the hook, then the body.

**Callees**, every one a callout except `c85c0`, `ce0c0` and slice 7a's
`e68c0` and `e4050` (bodies, fused), with the bytes `ret` pops: `d0d20`,
`d0080`, `10f5f0`, `c7070`, `1143d0`, `f42f0`, `cc380`, `113da0`,
`113df0`, `cbec0`, `189210`, `d0c10`, `138880` (0); `10f720`, `113d20`,
`113d60`, `110ab0`, `d1540` (4); `d1080` (8); `100430` (`c`).

### Not verified for slice 6b

- Captures from Waterworks and a campaign level with vehicles and
  turrets (the slice's testable result): registers and RAM at the n-th
  call of each of the four, compared with the lifted build. The harness covers synthetic pools with stubbed callees.
- The harness dresses only units whose linked objects are pool objects;
  a linked object of another type, and `10f5f0`'s names beyond the two
  the bodies test for, are read only from the code.
- The Halo 1 drive model's hook with the body guarded is read from the
  installer (`hooks::install::guard_shared_bodies`), not run.

## Related

- [object-update.md](object-update.md): entries, callouts and code paths.
- [sim-rewrite-objects.md](sim-rewrite-objects.md): the design and slices.
