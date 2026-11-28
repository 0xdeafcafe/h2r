# Simulation rewrite, phase 3: the object and unit update

Status: design, 2026-11-20. Slices 0 to 11 are built: the
dispatcher and the switch (default off), the views and `sim-state`,
slice 1's bodies (the walks, the per-object updates, the type
dispatchers, the wake leaves and the early-mover wake), slice 3's
(function values, the export evaluator, the import dispatcher, the seven
type import handlers and change colours, all segments), slice 5's
(the light walk, the light record update and its phase step, and the
light-list link, unlink and expire; the lists stay guest), slice 8's (the
damage update, and the `Damage` and `Kill` events), slice 4's (the
bounds, attachment refresh, node-matrix notify and component transform,
the transform setters and getters, and `bd090` as the phase 1 port run
as a body, whose skeleton walk is phase 1b's `build_matrices`), slice 6a's (the unit update's shell, movement, direction clamp, idle
test and aim update), slice 6b's (the unit update's control, state,
powered seats and control refresh), slice 6c's (the unit update's timers, dialogue
update, team with its seat iterator, active camouflage and integrated
light, and the unit animation state `111650`), slice 7a's (the biped update `dd360` with its
leaves, timer, fall, animation state and look updates; the movement and
animation steps below it stay callouts), slice 7b's (the biped
post-physics update `dd990` with its crouch, ground probe, jump, physics
input, landing and contact steps; the AI hooks, the damage packet and the
physics step stay callouts), slice 9's (the weapon update `fdfb0`), slice
10's (the vehicle update `efde0`, with the type physics and the rest of
its tree left as callouts), and slice 11's (the small type updates and
the projectile step `f8eb0`). The differential
harness compares MXCSR, the XMM registers and the x87 state at every callout of
every root, except the roots in its `FLOAT_OPT_OUT`. The contracts they keep are in
[object-update.md](object-update.md). It details phase 3 of
[sim-rewrite.md](sim-rewrite.md) (the plan; its section 8 decisions bind this
page). Phase 1 (animation and pose, `crates/rewrites/sim-anim`) and phase 2 (collision
and spatial queries) land first; section 6 says what this phase needs from
them.

The object walks, export lookup, light endpoint blends and unit look basis
use the Rust kernels in `crates/rewrites/sim-objects`, as does the
tag-function curve evaluator `13b390`: an idiomatic kernel
(`functions/curve.rs`) that predicts every guest call. The guest's answer
is compared with the prediction until 4096 calls have matched with no
mismatch; then the kernel answers, one answerable call in 64 is still
compared, and a mismatch returns every call to the guest. The counts are
kept in `engine::sim_objects::functions::parity`. The action walk
`e6830` serves both bipeds and vehicles. Six object points support live
segment comparison through the [shadow checks](shadow-exec.md#registration-and-the-agent-api).

Evidence marks: **C** confirmed (read from the retail image with `h2 disasm`
on 2026-11-20, or already encoded in a differential-tested port in
`crates/rewrites/engine/src/sim_physics`), **L** likely, **O** open. Names are ours.
Addresses are guest virtual addresses; bare numbers in tables are hex.
Sample counts come from the Waterworks profile behind
[sim-budget.md](../perf/sim-budget.md) (84,358 main-thread samples, 14.12
ms/tick).

## Decisions

1. **No persistent Rust object table.** Object headers and bodies are read
   and written through typed zero-copy views. SoA exists only as per-phase
   working sets. A persistent mirror cannot be kept honest: 1,387 guest
   functions touch the object array, every type callback can create, delete,
   relink and wake objects mid-walk, the object pool compacts and moves
   bodies, and a checkpoint revert is a `memcpy` Rust never sees.
2. **Bodies are segments over retail stack frames.** A Rust body that can
   call guest code runs as straight Rust between calls, and at every call
   out the guest stack and the eight integer registers hold what retail
   holds. No Rust state has to survive a guest call. The lifted original can
   take over at any return site and our body can take over a frame the
   original made. Snapshots stay `Stateless`.
3. **One local dispatcher.** Calls between ported bodies never go through the
   runtime. Only a call to code that is still guest leaves Rust.
4. **Parallelism is validated speculation.** Workers precompute pure kernels
   from a gather taken at walk start. The serial walk, in retail order,
   re-gathers each kernel's inputs, uses the precomputed output only if the
   input bytes are identical, and otherwise computes inline. The result is
   always `kernel(inputs at the retail moment)`, so it cannot depend on the
   worker count, on whether speculation ran, or on what a callback did in
   between.
5. **Type updates never run in parallel under bar E.** Unit, biped, vehicle,
   weapon, projectile and damage updates read and write other objects, draw
   from the shared RNG, and create and delete objects. They run on the game
   thread in retail order, first as guest callouts and then as Rust bodies,
   type by type.
6. **Readers get typed views, not a copy.** `sim-state` reads the arena at
   tick end through the same views the subsystem uses. Events come from the
   Rust body that applies them.
7. **Parallel pose is the last slice and is gated on a measurement.** After
   the native serial ports, per-object pose math is small (estimate in
   section 7). It is built only if the co-op campaign census shows at least
   0.25 ms/tick left in the pose kernels.

## 1. What this phase removes, measured

Method: the profile's folded stacks, charging each sample to its innermost
guest frame ("with services", the same measure as `guest_total` in
`profile.txt`; `bd090` gives 490 either way). Windows are address ranges, not
call paths: a function in the weapon window also runs from weapon fire. The
whole object family, `a0000..120000`, holds 16,637 samples, 19.72% of the
thread.

| Window | Samples | % thread | Heaviest (samples) | Owner |
|---|---:|---:|---|---|
| Walks and per-object core `b6000..be800` | 4,683 | 5.55 | `bd090` 490, `bab40` 282, `b8d30` 263, `bcc20` 253, `bb050` 253, `b89b0` 227, `ba1d0` 167, `ba8c0` 163, `bdef0` 156, `bc470` 148, `bdc40` 142, `b9dd0` 132, `bc820` 126 | this phase, less the rows below |
| of which phase 1 | −895 | | `bd090` 490 (shared, see section 6), `b8d30` 263, `bdc40` 142 | phase 1b, 1c |
| of which phase 2 | −530 | | `bb050` 253, `b89b0` 227, `b8940` 50 | phase 2 |
| Colours, attachments, finalize `be800..c0000` | 262 | 0.31 | `bef30` 98, `be8e0` 28, `bf380` 17 | this phase |
| Lights `c0000..c4000` | 671 | 0.80 | `c0c80` 432, `c1400` 83, `c0b00` 64 | this phase |
| Type dispatch `108000..10a000` | 558 | 0.66 | `108d90` 317, `108c60` 67, `109140` 44, `109050` 42 | this phase |
| **Core subtotal (this phase only)** | **4,749** | **5.63** | | |
| Unit `c4000..d2000` | 2,883 | 3.42 | `c7840` 325, `c60c0` 260, `ce0c0` 195, `cba50` 195, `cafc0` 190, `c49b0` 158, `c58f0` 150, `c6b90` 138 | object update (the animation subsystem owns `c7840` when both are enabled) |
| Unit animation `110000..115000` | 1,072 | 1.27 | `112070` 361 (phase 1c), `111650` 210, `1130f0` 85 | 711 this phase |
| Biped `dc000..ee000` | 1,597 | 1.89 | `df380` 139, `dd360` 138, `e01d0` 137, `e24f0` 137, `dc5c0` 116, `dd990` 109 | this phase |
| Vehicle `ee000..f8000` | 1,447 | 1.72 | `f6d10` 224, `efde0` 183, `eec90` 175, `f3010` 162 | this phase |
| Weapon `fd000..106000` | 761 | 0.90 | `ff5f0` 204, `fdfb0` 117, `100520` 88 | this phase |
| Damage `d2000..dc000` | 403 | 0.48 | `d5de0` 201 | this phase |
| Scenery and item `10a000..110000` | 298 | 0.35 | `10dc70` 70 | this phase |
| Device, machine, crate, creature | 230 | 0.27 | `1152e0` 100 | this phase |
| Projectile `f8000..fd000` | 190 | 0.23 | `f8eb0` 107 | this phase |
| **Type trees subtotal** | **8,520** | **10.10** | | |

At 14.12 ms/tick a percentage point is 0.141 ms. The inner-frame measure
already includes host services under a guest frame; the runtime time that is
not under any guest frame (21.2% of the thread) scales with guest code, so
the upper bound multiplies by 1.27.

| Scope | Cost today | Removed at 4× |
|---|---:|---:|
| Core | 0.79 to 1.01 ms | 0.60 to 0.76 ms |
| Core + unit + unit animation + biped | 1.66 to 2.11 ms | 1.25 to 1.58 ms |
| Everything in the table | 2.20 to 2.80 ms | 1.65 to 2.10 ms |

So the plan's exit figure for phase 3 (at least 1.2 ms on Waterworks) needs
the core, the unit tree and the biped tree. Vehicles and weapons are the
margin. The co-op campaign numbers do not exist yet (phase 0 census,
`design-attribution` lane); section 9 says which rows they are expected to
move.

## 2. The retail shape

### Where the walks sit in a tick (C)

`GAME_TICK` `137fe0` calls, in order: sim record `682e0`, `683a0`, players
`14d3a0`, `138db0`, AI `1c83c0`, `1fb510`, `20b660`, game engine `15c150`,
`195ed0`, HaloScript `209310`, `138c10`, **pre-physics `b6f10`**, world
physics `1c3d30`, **post-physics `b7060`**, **finalize `b7150`**, `2268e0`,
effects `176e40`, **lights `c1400`**, `15ca10`, players-after `14d8b0`, then
the rest. AI submits unit control before the pre-physics walk reads it.

### Object header (`0xc` bytes; flags byte at `+2`)

| Bit | Name | Evidence |
|---|---|---|
| `01` | ACTIVE | C. Required by every walk. |
| `02` | AWAKE | C. Set up the parent chain by `b7360`. Cleared at the end of `bc470` when the update returned 0 and POST is clear. |
| `04` | POST | C. Cleared at the start of `bc470`, set by `bba20` on an active object. The post walk visits `flags & 0c == 04`. |
| `08` | NEW | C. Excludes the object from the pre and post walks. Finalize clears it and runs both updates. Set at creation by `b7b40` and by `109580` for woken weapon, equipment, garbage and projectile objects. |
| `10` | DELETE | C. Finalize pass B deletes it (`b83b0`, `b8460`). |
| `20` | DEFER | C. `bc470` returns 1 at once when set; finalize clears it on every live header. Set on the child by the attach function `b93b0` ([object-update.md](object-update.md)). |
| `40` | ATTACH | C. Post-update calls `bef30` when set and the bounds did not change. |
| `80` | CHILD | C. `bba20` replaces the handle by the root ancestor (`baf80`, parent at body `+14`). |

Header `+3` is the type (0 bipd, 1 vehi, 2 weap, 3 eqip, 4 garb, 5 proj,
6 scen, 7 mach, 8 ctrl, 9 lifi, 10 ssce, 11 bloc, 12 crea); `+8` is the body
pointer, which the pool may rewrite.

### Object body fields this phase reads or writes (C unless marked)

| Offset | Field |
|---|---|
| `+00` | definition tag datum |
| `+04` | flags; bit 8 "in the spatial partition" (read by `bdef0`), bit 30 cleared by `bd090` |
| `+0c`, `+10`, `+14` | next sibling, first child, parent |
| `+28` | location (cluster reference) |
| `+30`, `+3c` | bounding sphere centre and radius |
| `+40`, `+4c` | centre and radius the spatial partition last registered |
| `+50`, `+5c` | second sphere (offset by the definition's `+24..+2c`, radius scale `+20`) |
| `+64`, `+70`, `+7c`, `+88` | position, forward, up, velocity |
| `+a0` | scale |
| `+aa` | type |
| `+b3` | sleep timer, decremented by `bc470` |
| `+b4` | physics component handle (array at `51e9b8`, stride `a0`) |
| `+b8` | the early mover that woke this object |
| `+c0` | bit 1 and bit 2: woken latch and its one-tick copy (rotated by `bc820`); bit 3: on the early-mover list; bit 6: component active |
| `+d8` | bit 0 gates `a9570` (O: meaning) |
| `+ec`, `+f0`, `+10a` | body vitality, shield vitality, damage flags |
| `+114`, `+116` | node-matrix byte count (÷ `34`) and offset |
| `+11e` | attachments offset |
| `+12a` | animation block offset (`ffff` none) |

### The type table (C)

`468630` holds 13 descriptor pointers. Each descriptor lists its ancestry at
`+84`. The dispatchers walk the ancestry and call the slot when it is set:
`108c60` slot `+40` (update), `108cd0` slot `+44` (post-physics), `108d90`
slot `+4c` (function import), `109050` slot `+6c` (pose), `1090d0` (after
root, slot O), `108a90` slot `+2c` (new).

| Descriptor | `+40` update | `+44` post | `+4c` import | `+6c` pose |
|---|---|---|---|---|
| `obje` `4678e8` | | | `bcc20` | |
| `unit` `4679b0` | `c6b30` | | `c6b90` | `112070` |
| `bipd` `467a78` | `dd360` | `dd990` | `de3b0` | |
| `vehi` `467b40` | `efde0` | | `f6d10` | `f0f50` |
| `item` `467c08` | `10b4d0` | | | |
| `weap` `467cd0` | `fdfb0` | | `ff5f0` | |
| `proj` `467f28` | `f8de0` | | `fbe70` | |
| `scen` `467ff0` | `10a2f0` | | | |
| `devi` `468248` | `106790` | | `106d70` | `107000` |
| `mach` `468310` | `1152e0` | (`+48` `115a40`) | `11bd60` | |
| `bloc` `468180` | `11bce0` | | `11bd60` | |
| `crea` `468568` | `118700` | `118b80` | | |

`eqip`, `garb`, `ctrl`, `lifi` and `ssce` have none of these four.

### The three walks (C; the existing ports are the register-level record)

All three set the phase byte `[[4de2f4]+3]` to 1 on entry and 0 on exit.
`c0c80` and `bf380` read that global.

**Pre-physics `b6f10`.**

1. Early-mover pass. The list is 32 handles at `[5107f0]+1d00`, count at
   `+1d80` (read once). For each handle, re-reading the header each time:
   `flags & 0b == 03` → `bc470(handle)`; then, with the flags re-read,
   `flags & 0c == 04` → `109580(handle)`.
2. Slot pass over live headers. The high-water mark and the bitmap pointer
   are re-read after every visited slot. `flags & 0b == 03` and body `+c0`
   bit 3 clear → `bc470`.

`109580` (C) asks the spatial index (`bb050`, all six lists, up to `400`
results) for objects in the mover's sphere (`+28` location, `+30` centre,
`+3c` radius). For each result that is ACTIVE, neither NEW nor DELETE, and
has body `+c0` bit 2 clear: set `+c0` bit 2, store the mover at `+b8`, wake
it (`b7360`), and for types weap, eqip, garb and proj also `bba20` and set
NEW.

**Post-physics `b7060`.**

1. Early-mover pass: `flags & 0c == 04` → `bc820(handle)`, then
   `109660(index in ecx, handle in eax)`.
2. Slot pass: `flags & 0c == 04` and body `+c0` bit 3 clear → `bc820`.
3. `fd910` (→ `baeb0`, `fd980` → weapon fire `104150`).

**Finalize `b7150`.**

1. Slot pass A. Every live header: clear DEFER (always a store). If NEW,
   ACTIVE and AWAKE are set and DELETE is clear: clear NEW, `bc470`; then if
   POST is set, `bc820`.
2. `fd910`.
3. Slot pass B. DELETE → `b83b0(handle, 1)`, `b8460(handle, 1)`.
4. Tail jump to `bf380`.

### Per-object pre-update `bc470(object) -> al` (C)

1. Not AWAKE → return 0. DEFER → return 1. Clear POST (store).
2. Body `+c0` bit 2 → `bba20(eax = object)`.
3. Game-globals byte `[[4e6948]+c]` is 4 or 5 and body `+d8` bit 0 →
   `a9570(eax = object)`.
4. Animation block present and its `+68` is not −1 → `1cd4e0`; `al` starts
   the result.
5. `108c60(object)`: type update dispatch; OR into the result.
6. `d5de0(object)`: damage update; OR.
7. `be8e0(object)`: change colours; OR.
8. Children: `first child`, then after each child's recursive `bc470` the
   child's header is looked up again for `next sibling`.
9. Component present and `+c0` bit 6 → `1ced00(edi = component record)`.
10. Sleep timer `+b3` non-zero → decrement, return 1. Otherwise, if the
    result is 0 and POST is clear, clear AWAKE. Return the result.

### Per-object post-update `bc820(object)` (C)

1. Component present → `bc5e0(object)` (physics component to object
   transform; calls `b7430`, `b7740`, `b91d0` and the `1d....` component
   accessors).
2. Type is not `mach` → `bd090(object)` (node matrices). Otherwise, if the
   component is active and its flags bit 15 is set, `1d3920(component, mask,
   0)` with a zeroed node bit mask on the stack.
3. `108cd0`: type post-physics dispatch.
4. Body `+c0`: bit 1 := old bit 2, then bit 2 := 0 (two stores).
5. `bdef0(object) -> al`: bounds and partition (below).
6. ATTACH set and `al == 0` → `bef30(object, 1, 1, 0, 0)` (attachment
   refresh; light list calls `c3220`, `c2d00`, `c1720`; recursive).
7. Children, as in pre-update.
8. `be1d0(eax = object)` (→ `b8c00`, `109140`).

**`bdef0` (C).** For type `bloc` with an active component, `1d44f0` may
supply the sphere. Otherwise the centre is the definition's `+08..+10`
(pre-multiplied by the root matrix scale when it is not 1) through the root
node matrix, and the radius is body scale × definition `+04`. Each row is
scalar SSE in this order: `(m_z·z + m_y·y) + m_x·x + t`. The second sphere
follows from definition `+20` (when positive) and `+24..+2c`. The sphere the
partition uses is chosen by definition flags bit 12. If the object is in the
partition and its registered centre moved by more than a threshold (squared
distance, `comiss`) or its radius changed (`ucomiss`), call `b87b0`
(disconnect), store the new registered sphere, call `b8600(object, 0)`
(connect) and return 1. If neither changed, return 0 and store nothing
further. If the object is not in the partition, store the registered sphere
and return 1.

## 3. Crates and modules

```
crates/rewrites/sim-core/                  (phase 1a creates it; this phase adds)
  src/spec.rs                     Spec<K>: validated speculation (section 7)
  src/jobs.rs                     Jobs: fixed chunking (plan section 6)
crates/rewrites/sim-objects/      no `runtime` dependency
  src/lib.rs
  src/view.rs                     Objects, ObjRef: the pool, live-slot scan, handles, child links
  src/defs.rs                     DefTable (static, per map)
  src/synth.rs                    synthetic pools and tags for tests (feature `synth`)
  src/walk.rs                     early-mover list
  src/wake.rs                     b7360, baf80, bba20, bc7b0 (leaf kernels)
  src/events.rs                   the tick's SimEvent buffer (section 8)
  src/damage.rs                   Damage and Kill events, read at d9640 and 15ce70
  src/bounds.rs                   bdef0 arithmetic
  src/functions.rs                bab40, ba8c0, 108d90 evaluator
  src/colors.rs                   be8e0
  src/lights.rs                   c0c80, c0b00 record update
  src/pose.rs                     bd090 orchestration over sim-anim
  src/unit/, biped/, ...          one module per type tree
  src/phase.rs                    WalkSet (built); PoseSet, BoundsSet (slice 12)
crates/rewrites/sim-state/                 read-only API (section 8)
crates/hooking/guest-layout/src/object.rs  every offset in section 2: header and
                                  physics flag bits, body fields, ObjectDefinition
crates/rewrites/engine/src/sim_objects/    the guest boundary (needs `runtime`)
  mod.rs                          BODIES table, local dispatcher, supported()
  frame.rs                        retail frame helpers (push, pop, slots)
  walks.rs                        b6f10, b7060, b7150
  object.rs                       bc470, bc820, dispatchers 108c60/108cd0/109050/1090d0
  wake.rs                         the wake leaves' bodies over src/wake.rs; 109580
  damage.rs                       d5de0; the Damage and Kill recorders
  functions/                      bab40, ba8c0, 108d90, bcc20, be8e0 and the import handlers (segments)
  item.rs, scenery.rs, ...        the slice 11 type updates; sse.rs, alu.rs their SSE and flag idioms
  ...                             one file per ported segment body
crates/rewrites/sim-bridge/src/sim_objects.rs installer (called by `sim_native`)
docs/engine/object-update.md      the contract page (engine README format)
```

`sim-objects` depends on `sim-core`, `sim-anim` and the phase 2 crate
through a trait (section 6), and reads guest memory through
`guest_layout::Mem`, the trait `engine` already implements for its guest
memories; offsets come from `guest_layout`'s layouts, so `sim-objects`
adds names, not a second copy of an offset. `engine` depends on
`sim-objects`.

## 4. Data

### Typed views (the access path)

```rust
pub struct Objects { pub array: DatumArray, pub data: u32, pub bitmap: u32, pub stride: u32 } // re-read per segment
pub struct ObjRef { pub handle: Datum, pub header: ObjectHeader, pub body: Object }      // guest_layout views
```

- An `ObjRef` is never held across a guest call or a tick: the pool moves
  bodies. A segment resolves it from the handle each time it resumes.
- Hot kernels read a record with one bulk read (`[u8; 0x12c]` for the common
  body, `[u8; 12]` for a header) and decode in Rust. Stores are individual
  and keep the retail width and order per field (a byte store stays a byte
  store), because the 4 KiB write tracker and partial-fault behaviour see
  them.
- The live-slot scan reads one bitmap word at a time and never reads a word
  before the previous one was exhausted, as the existing port does.

### `DefTable` (static)

One entry per object definition tag, built the first time an object of that
definition is touched, from the tag bytes resident in guest memory (the same
rule as `ClipStore`: decode what the guest would read, so it cannot
disagree).

What `sim_objects::defs::Def` holds today is the bounds (`tag`, `flags`,
`bounding_radius`, `bounding_offset`, `dynamic_scale`, `dynamic_offset`,
read through `guest_layout::ObjectDefinition`). The rest of this shape
arrives with the slices whose kernels read it:

```rust
pub struct Def {                    // ~96 bytes plus slices
    pub kind: u8,
    pub ancestry: [u8; 3],          // descriptor ids, obje first
    pub update_mask: u8, post_mask: u8, import_mask: u8, pose: u8, // which ancestry levels have the slot
    pub flags: u16,                 // definition +02
    pub sphere_radius: f32,         // +04
    pub sphere_centre: [f32; 3],    // +08
    pub dyn_radius: f32,            // +20
    pub dyn_offset: [f32; 3],       // +24
    pub functions: Box<[Function]>, // +64/+68, stride 20
    pub colors: Box<[ChangeColor]>,
    pub attachments: Box<[Attachment]>, // +94, stride 18
    pub nodes: u16, pub node_parents: Box<[i16]>, // from the model
}
pub struct DefTable { by_tag_index: Vec<u32 /* 0 = absent, else index + 1 */>, defs: Vec<Def>, epoch: Option<u64> }
```

Keyed by tag datum index (an entry also keeps the full datum, so a reused
index with a new salt decodes again), valid for one epoch. Map overrides
and mods (`crates/extensions/game/src/map_override.rs`) replace map files through
the VFS, so their tags are in memory before any object exists, and lazy
building sees them. The one writer of tag data after load is a hot tag
edit (`edit.set`, `edit.revert`, `crates/extensions/hot-tags`), which
retail sees at its next read; the slice that gives `DefTable` its first
runtime reader also bumps the epoch on every such edit. The descriptor
slots in the type table are image constants and are read from guest
memory once per process.

### Transient SoA sets (`phase.rs`)

Built on the game thread inside one segment, read by workers, dropped or
kept as derived state for the rest of the walk (section 7).

`WalkSet` is built: one row per live object (handle, flags, type, `+c0`,
parent, position, forward, up, animation block), with the slot pass's
visits by type. No walk gathers it yet; the walks run as retail-frame
bodies ([object-update.md](object-update.md#the-walks-in-rust)). `PoseSet`
below is slice 12's:

```rust
pub struct PoseSet {
    // one row per object the post walk is predicted to pose, in predicted visit order
    pub handle: Vec<u32>, pub slot: Vec<u16>,
    pub parent_row: Vec<u16>,        // NONE or a row with a smaller index
    pub parent_node: Vec<u16>,
    pub wave: Vec<u8>,               // hierarchy depth
    pub node_base: Vec<u32>, pub node_count: Vec<u16>,
    pub input: Vec<(u32, u32)>,      // span in `inputs`
    // SoA over every node of every row
    pub rot: Vec<[f32; 4]>, pub trans: Vec<[f32; 3]>, pub scale: Vec<f32>,
    pub parent: Vec<i16>,
    pub out: Vec<NodeMatrix>,        // 0x34 bytes each
    pub sphere: Vec<[f32; 8]>,       // bounds kernel output per row
    pub inputs: Vec<u8>,             // exact gathered bytes, for validation
}
```

Capacity is kept between ticks; contents never are. With 2,048 objects and
64 nodes each the worst case is about 8 MiB; Waterworks needs under 400 KiB.
The plan's +32 MiB per match holds.

### Derived state

Only two things: the `DefTable` lookup (static in content, derived in
lifetime) and a walk's `Spec` results. Both are dropped on an epoch bump.
Neither is ever needed for correctness.

## 5. The guest boundary

### Bodies, segments, frames

A **leaf kernel** is a plain Rust `fn` over views. It never calls guest
code. `b7360`, `baf80`, the bounds arithmetic and (once every import handler
is Rust) the function evaluator are leaf kernels.

A **segment body** may call guest code. It is written as retail-site arms:

```rust
pub struct Body {
    pub point: &'static Point,
    pub sites: &'static [u32],   // retail return addresses of its guest calls
    pub run: fn(&mut Cx<'_>, pc: u32) -> Result<Next>,
}
pub enum Next { Call { target: u32, site: u32 }, Ret { pop: u16 }, Tail { target: u32 } }
pub struct Cx<'a> { pub cpu: &'a mut CpuAbi, pub g: &'a mut Guest }
```

Rules for a segment body (the **callout contract**):

1. **Frame.** At every `Next::Call`, `ESP` equals retail's, and every stack
   slot the body will read after the call holds what retail holds there
   (saved registers, spilled locals, arguments). Values that live across a
   call live only in those slots or in the callee-saved registers retail
   uses for them. Rust locals die at the call.
2. **Registers.** At every call out and at the final return, all eight
   integer registers equal retail's. The image is link-time optimised: a
   callee may take arguments in any register and a caller may rely on a
   register the callee never touched. The existing ports record the values
   for the walks; a new body takes them from the disassembly.
3. **Flags.** Arithmetic flags are reproduced where the existing ports
   reproduce them. A new body may mark them undefined at a transfer
   (`eflags_defined`) only when slice 0's liveness scan (below) shows no
   reader.
4. **Memory.** Every guest-visible store retail makes is made, with the
   same value and width, stack stores included. That covers the stores a
   leaf kernel makes below the `ESP` it returns with (its pushes, and the
   return addresses and arguments of its internal calls): the caller's
   next deeper frame overlays them, and a slot that frame never writes
   holds what retail left there. The datum adapters' relaxation (engine
   README, "Fault behaviour") does not apply to bodies.
5. **Order.** Stores that a callee can observe happen before the call.
   Between two calls the order of stores is free.
6. **Float environment.** `supported(cpu)` is checked once per body entry
   (MXCSR round-to-nearest, no flush-to-zero, no denormals-are-zero, x87
   control as the existing `Controls` classes). If it fails, the original
   runs (`resolve_original`, or `resolve_original_resume` at a site), which
   rule 1 makes safe.
7. **RNG.** Every draw from `[4e7408]` stays where retail makes it, on the
   game thread.

Rules 1 and 2 give compatibility in both directions: a frame our body built
can be finished by the lifted code, and a frame the lifted code built (the
float environment was unsupported at entry, or the switch covers only part
of a tree) can be finished by ours.

### Local dispatcher

`engine::sim_objects::drive(cpu, g, table, pc) -> Result<TransferAbi>`:

```
loop:
  body = BODIES.find(pc)            // entry or site; sorted table, binary search
  match body.run(cx, pc):
    Call{target, site}: push site
        if a body starts at target and supported(cpu): pc = target; continue
        else: return TransferAbi::call(target, site)       // true callout
    Ret{pop}: target = pop return address; esp += pop
        if target is a body's site and supported(cpu): pc = target; continue
        else: return TransferAbi::ret(target)
    Tail{target}: same test as Call, without the push
```

Every entry and every site of every enabled body is registered with the
runtime (`implement(..).resumes(sites)`), exactly as `sim_physics::PORTS` is
today. Indirect callouts (the type dispatchers) resolve the slot value and
take the same path, so a type callback that has a body is entered without
the runtime and one that has none is a callout.

The dispatcher adds no yield points. The scheduler gets control at true
callouts, as it does inside any lifted function. A walk with no callouts
left is not reached in this phase (damage, AI, sound and effects stay
guest); revisit then.

### Entries taken over

"Sig" is the register convention where a named point already records it;
slice 0 confirms the rest and adds the points.

| Entry | Sig | Kind | Slice |
|---|---|---|---|
| `b6f10` `OBJECTS_PRE_PHYSICS` | no args, `ret` | segment (port exists) | 1 |
| `b7060` `OBJECTS_POST_PHYSICS` | no args, `ret` | segment (port exists) | 1 |
| `b7150` `OBJECTS_FINALIZE` | no args, tail `bf380` | segment | 1 |
| `bc470` `OBJECT_PRE_UPDATE` | `(object: stack 0) -> al`, `ret 4` | segment (port exists) | 1 |
| `bc820` `OBJECT_POST_UPDATE` | `(object: stack 0)`, `ret 4` | segment (port exists) | 1 |
| `108c60`, `108cd0`, `109050`, `1090d0`, `108d90` | C, in [object-update.md](object-update.md) | segment (indirect callout) | 1, 3 |
| `b7360`, `baf80`, `bba20`, `bc7b0` | `eax` (`b7360`, `bba20`), `ecx` (`baf80`), stack 0 with `ret 4` (`bc7b0`) (C) | leaf | 1 |
| `109580` | `(mover: stack 0)`, `ret 4` | segment until phase 2's `bb050` is callable from Rust, then leaf | 1 |
| `bab40` `OBJECT_FUNCTION_VALUE`, `ba8c0`, `bcc20` `BASE_OBJECT_IMPORT` | recorded | segment, then leaf | 3 |
| `be8e0` `OBJECT_CHANGE_COLORS` | `(object: stack 0) -> al`, `ret 4` | segment (port exists), then leaf | 3 |
| `c6b90`, `de3b0`, `f6d10`, `ff5f0`, `fbe70`, `106d70`, `11bd60` (imports) | O | segment or leaf | 3 |
| `bdef0`, `bef30`, `be1d0`, `bc5e0` | `bdef0`: `(object: stack 0) -> al`, `ret 4`; others O | segment | 4 |
| `bd090` | `(object: stack 0)`, `ret 4` | segment; shared with phase 1b | 4 |
| `c1400`, `c0c80` `LIGHT_RECORD_UPDATE`, `c0b00` | `c0c80`: `(light: stack 0)`, `ret 4` | segment (port exists for `c0c80`) | 5 |
| `c6b30` `UNIT_UPDATE` and its nine children | recorded for `c6b30` | segment (port exists for the shell and `c7840`) | 6 |
| `dd360`, `dd990` and trees | O | segment | 7 |
| `d5de0` `OBJECT_DAMAGE_UPDATE` | `(object: stack 0) -> al`, `ret 4` | segment | 8 |
| `fdfb0`, `efde0`, `10b4d0`, `10a2f0`, `106790`, `1152e0`, `f8de0`, `11bce0`, `118700`, `118b80` | O | segment | 9 to 11 |

Not taken over in this phase: `object_new` `b7b40`, delete `b83b0` and
`b8460`, `fd910`, `bf380`, `109660`, `a9570`, `1cd4e0`, `1ced00`, `1d3920`,
`1d44f0`. They stay callouts.

### Guest writes that must still happen

For the bodies read so far (the contract page extends this per slice):

| Body | Stores |
|---|---|
| walks | phase byte `[[4de2f4]+3]` 1 then 0; finalize: every live header's flags with DEFER cleared (a store even when unchanged), NEW cleared before `bc470` |
| `bc470` | header flags with POST cleared; sleep timer `+b3`; header flags with AWAKE cleared; its argument slot `[esp+10]` is overwritten with the header pointer (a later arm reads it) |
| `bc820` | body `+c0` twice (bit 1 from bit 2, then bit 2 cleared); the zeroed node mask on the stack before `1d3920` |
| `109580` | per result: body `+c0 |= 4`, body `+b8 = mover`, header `|= 08`; plus the wake stores |
| `b7360` | nothing if the object is already AWAKE; otherwise header `|= 02` for the object and every ancestor up the parent chain (the loop at `b7382` does not re-test, so an ancestor that is already AWAKE is stored again) |
| `bba20` | for a CHILD, the handle becomes its root ancestor first; when ACTIVE: the `b7360` stores, then header `|= 04`; then `bc7b0` (recursive over children; `b7360` for types bipd, vehi and crea, mask `1003`) |
| `bdef0` | `+30..+38`, `+3c`, `+50..+58`, `+5c`; then `+40..+48`, `+4c` only on the paths in section 2 |
| `bd090` | body `+04` bit 30 cleared; orientations; node matrices at `[body + [body+116]]`; the one-time static mask at `55eeb4`/`55eed4` |
| spatial stamps | `bb050` writes the visit stamps at `4de2d8..4de2fc` and `4e7411..4e7414`; phase 2 owns them, and `109580` must produce them through phase 2's entry, not skip them |

### Guest readers that must keep being fed

| State | Readers |
|---|---|
| Header flags, type, body pointer | everything (1,387 functions); replication `682e0`; HaloScript; AI; the agent API and telemetry raw readers; `scene-extract/src/layout/objects.rs` |
| Position, basis, velocity, scale | Havok sync, AI, weapons, camera, sound, replication, render |
| Node matrices | Havok rigid-body fetch `1ef070`, markers `b8d30`, bounds `bdf90`, attachments `bef30`, render (`OBJECT_RENDER_SETUP` `133790`), first person |
| Spheres `+30..+5c` and the partition lists | `bb050`, the collision datum walk `b89b0`, `109580`, render culling, AI perception, lights |
| Change-colour block | render |
| Light datums and light lists | render (`native_lights_glue`), `c1400` next tick |
| `+b8`, `+c0`, sleep timer | next tick's walks |
| Phase byte | `c0c80`, `bf380` |
| Early-mover list | `109660`, riders |

Nothing here is written at tick end from a Rust copy. Every store goes to
guest memory when retail makes it, so a callback, a script or a revert in
the middle of a tick sees retail state.

## 6. Consuming phases 1 and 2

**From phase 1 (`sim-anim`, `sim-core`).** `bd090` is one function with two
owners. Phase 1b owns the arithmetic (local records to node matrices, the
root transform `bdc40`). This phase owns the orchestration: which
orientation block, which callouts, in which order. The seam is three pure
functions this phase requires of `sim-anim`:

```rust
pub fn sample(clip: &Clip, frame: Frame, mask: &[u32], out: &mut [LocalNode]);              // 1a
pub fn build_matrices(k: &Constants, locals: &[Record], parents: &[i16], keep: &[u32],
                      xmm: [Vec4; 8], out: &mut [NodeMatrix]) -> Result<Walk, Unsupported>; // 1b
pub fn unit_pose(state: &UnitPoseIn, clips: &ClipStore, out: &mut [LocalNode]) -> UnitPoseEffects; // 1c
```

The [unit pose body](unit-pose.md) services an object pose request at
`112070` through its real guest calls. The pure interface above describes
the proposed pure pose interface. The [animation manager](animation-manager.md)
handles the channel and graph calls. In that interface, `UnitPoseEffects` lists the guest effects besides the output
(resource requests through `1dd9d0` and `1236f0`, any RNG draw through
`1dd630`), so the caller issues them in retail order on the game thread.
`bd090` is the register-level port `sim_physics/node_matrices.rs`, adopted
into `BODIES` unchanged; its skeleton walk calls `build_matrices` (the
root in `out[0]`, the stack mask as `keep`), with the per-node inline
leaves as the fallback ([skeleton-walk.md](skeleton-walk.md)).

Attached objects compose with the parent's node matrix (`b8bd0`, `ba160`).
The walks visit a parent before its children, so the parent's matrices in
guest memory are final when the child reads them.

**From phase 2 (collision).** Through one trait, so the order of landing
does not matter:

```rust
pub trait Spatial {
    /// bb050: fills `out` in retail order and writes the guest visit stamps.
    fn visit(&mut self, g: &mut dyn GuestMemory, lists: u32, types: u32, location: Location,
             centre: [f32; 3], radius: f32, out: &mut [u32]) -> Result<u16, MemFault>;
    /// The cluster set b8600 would link for this sphere (pure; static BSP only).
    fn clusters(&self, location: Location, centre: [f32; 3], radius: f32, out: &mut Vec<u16>);
}
```

Until phase 2 provides it, the default implementation is "not available"
and `109580`, `b87b0` and `b8600` stay callouts. Phase 2 keeps ownership of
the partition's guest lists; this phase never writes them itself.

## 7. The parallel job model

### What is independent

| Step | Reads beyond its own object | Shared writes | Verdict |
|---|---|---|---|
| Type update (`+40`), post (`+44`) | anything; RNG | anything; creates and deletes objects | serial, retail order, always |
| Damage `d5de0` | AI, players | effects, kills, AI | serial |
| Wake and early-mover visit | partition | other objects' flags, visit stamps | serial |
| Function values and change colours | parent chain and held weapon through imports; game time | own colour block | pure kernel |
| Pose `bd090` | parent's node matrix; component nodes; clip residency | own matrices and orientations; resource-cache requests | pure kernel after the requests are issued serially |
| Bounds `bdef0` arithmetic | definition | own spheres | pure kernel |
| Partition relink | static BSP | cluster lists | compute pure (`Spatial::clusters`), link serial |
| Attachment refresh `bef30` | own matrices and markers | light datums and lists | compute pure, stores serial |
| Light record `c0c80` | parent function values, markers | light datum, light lists | compute pure, list changes serial |

### Validated speculation (`sim_core::Spec`)

```rust
pub trait Kernel {
    type Static: Sync;
    /// Append every byte the output depends on. Game thread only.
    fn gather(g: &dyn GuestMemory, st: &Self::Static, item: u32, into: &mut Vec<u8>) -> Result<(), MemFault>;
    /// Pure. No guest memory, no clock, no RNG, no globals.
    fn compute(st: &Self::Static, input: &[u8], out: &mut Vec<u8>);
}
pub struct Spec<K: Kernel> { key: (Epoch, u32 /* tick */, u8 /* walk */), rows: Vec<Row>, inputs: Vec<u8>, outputs: Vec<u8>, by_slot: Vec<u16> }
impl<K: Kernel> Spec<K> {
    pub fn prepare(&mut self, jobs: &Jobs, g: &dyn GuestMemory, st: &K::Static, items: &[u32]);
    /// The only way a result leaves: gather now, compare, reuse or compute.
    pub fn take(&mut self, g: &dyn GuestMemory, st: &K::Static, item: u32, scratch: &mut Vec<u8>) -> Result<&[u8], MemFault>;
}
```

- `prepare` runs at the top of a walk segment, on the game thread: gather
  every predicted item in table order, then `jobs.for_chunks` computes
  outputs into disjoint, pre-sized slices.
- `take` runs in the serial walk at the exact retail position of the
  kernel. It gathers the item's inputs again and compares them byte for byte
  with the speculated input. Equal: return the speculated output. Different,
  absent, or the key is stale: compute inline. The caller then scatters with
  ordinary stores, in retail order.
- So the bytes written are `compute(gather(now))` in every case. Worker
  count, chunk size, speculation on or off, and a lost `Spec` (snapshot
  restore, epoch bump, switch off) cannot change them. The cache-cold arm of
  the ladder is "never call `prepare`".
- A wrong prediction costs time only. A kernel whose inputs include another
  kernel's output is speculated in **waves**: wave 0 holds roots; wave `d`
  takes the speculated output of its parent in wave `d − 1` as input. `take`
  validates against the real parent matrices.
- `gather` must be complete. That is the one proof obligation, and it is
  tested (section 11): perturb each input byte class between `prepare` and
  `take` and require the inline result.

`Spec` state is **derived** in the plan's taxonomy. It lives for one walk.

### `Jobs`

As the plan's section 6: a persistent pool; chunks are fixed ranges of item
index (8 rows for pose, 32 for bounds), so chunk `i` computes the same items
whatever the worker count; no reductions across items; workers see only
`&Static` stores and the set's Rust buffers. Default 1 worker on hosts and
sims, `cores / 2` capped at 4 for `h2 play`. With 1 worker `prepare` is
skipped entirely.

### Floats and SIMD

- Kernels use `sim_core::lanes` for every float operation, including scalar
  ones. Retail's scalar SSE (`mulss`, `addss`, `subss`, `divss`, `sqrtss`)
  maps to lane helpers that take operands in retail (destination, source)
  order and carry the x86 NaN-operand rule, because raw Rust `a + b`
  propagates NaN payloads differently on arm64 and x86-64 and films must
  replay across both. The fast path (all operands zero or in
  `[2^-63, 2^63)`) is the bare host instruction, as today.
- `comiss`/`ucomiss` followed by `lahf; test ah,44h; jnp` (the pattern in
  `bdef0`) goes through a `lanes::cmp` helper that returns the x86
  unordered result.
- Across objects, SIMD is lane-wise only: four objects' bounds rows in one
  `f32x4`, each lane performing retail's scalar sequence. No horizontal
  sums, no FMA, no reordering.
- x87 sequences (unit aim `c7840`, anything with `fsin`/`fpatan`) use the
  `x87` crate's exact operations and stay scalar.

### Expected size of the parallel win

After the serial native ports, sampling plus matrices costs on the order of
130 ns per node. Waterworks poses roughly 3,000 nodes a tick (estimate from
object counts, to be measured in slice 12's step 0): about 0.4 ms serial,
about 0.12 ms with four workers, and nothing on hosts. Co-op campaign fights
pose more. That is why the slice is last and gated.

## 8. Reading typed state: `sim-state`

Read-only, called on the game thread at tick end (after `GAME_TICK`
returns), never from a worker, never mid-tick.

```rust
pub struct ObjectId(pub u32);                     // salted handle
pub enum ObjectKind { Biped, Vehicle, Weapon, Equipment, Garbage, Projectile, Scenery,
                      Machine, Control, LightFixture, SoundScenery, Crate, Creature }
pub struct ObjectFlags { pub active: bool, pub awake: bool, pub new: bool, pub deleting: bool, pub child: bool }
pub struct Transform { pub position: [f32; 3], pub forward: [f32; 3], pub up: [f32; 3], pub scale: f32 }

pub struct ObjectView<'a> { /* &dyn GuestMemory, header, body */ }
impl ObjectView<'_> {
    pub fn id(&self) -> ObjectId;
    pub fn kind(&self) -> ObjectKind;
    pub fn flags(&self) -> ObjectFlags;
    pub fn definition(&self) -> TagId;
    pub fn parent(&self) -> Option<ObjectId>;
    pub fn children(&self) -> Children<'_>;        // first child, next sibling
    pub fn transform(&self) -> Transform;          // local; parented objects are parent-relative
    pub fn velocity(&self) -> [f32; 3];
    pub fn bounding_sphere(&self) -> ([f32; 3], f32);
    pub fn vitality(&self) -> Vitality;            // +ec, +f0, +10a
    pub fn node_count(&self) -> usize;
    pub fn nodes(&self, out: &mut [NodeMatrix]) -> usize; // world space, copied from [body+116]
    pub fn as_unit(&self) -> Option<UnitView<'_>>; // bipd, vehi
}
impl UnitView<'_> {
    pub fn player(&self) -> Option<PlayerId>;      // +13c
    pub fn seat(&self) -> Option<i16>;             // +1fc
    pub fn actor(&self) -> Option<ActorId>;        // campaign; offset O (slice 2 step 0)
    // aim, weapons, team: offsets from docs/live/live-gamestate.md "Unit aim, held weapon"
}
pub struct ObjectIter<'a>;                         // live headers in slot order
```

- `SimRead::objects()` is `ObjectIter`. `SimRead::pose(object, out)` is
  `ObjectView::nodes`: the matrices are already in the arena, exact, and
  cost a copy. `SimRead::anim(object)` decodes the animation block at
  `+12a` for consumers that have no sim (the web viewer samples the shared
  clip store itself).
- Views borrow guest memory for the call. Nothing is cached, so a revert or
  a keyframe load needs no invalidation here.
- **Events.** A per-tick `Vec<SimEvent>`, cleared at `GAME_TICK`'s entry,
  appended only by serial code on the game thread: `ObjectDeleted { id,
  kind, definition }` from finalize pass B (slice 1), `Damage` and any
  `ActorKilled` at the damage report `d9640`, and `Kill` at the player kill
  credit `15ce70`. Observers append at the callee entries. The buffer
  holds one tick and stays outside the keyframes and hash. The buffer (`sim_objects::events`, read through
  `sim_state::events` and the agent API's `sim.events`) belongs to the
  thread that runs the walks. The records are in
  [object-update.md](object-update.md), "Slice 8 body".
- `live-telemetry/src/raw.rs` and `agent/src/raw.rs` move onto these views at
  their owners' pace. This lane does not edit them.

## 9. Co-op campaign

The walks have no campaign branch. What changes is the load and the callout
set:

- **AI-driven units** take the same path as players. `ai_update` `1c83c0`
  runs before the pre-physics walk and submits control through `1e4390` →
  `c6de0` (`UNIT_APPLY_CONTROL`); `c6b30` consumes it. The AI calls made from
  inside the trees stay callouts in retail order: `c6740` → `1c9c80`,
  `dd360` → `1e5af0`, `dd990` → `1e5a60` and `1ec500`, `d5de0` → `1e4a10` and
  `1e9720` (it also reads the actor array `4f55f0`), `118700` → `1e5bb0`,
  `1e6360`, `1e6120`, `1e55d0`.
- **More sleeping objects.** The slot pass scans up to 2,048 headers and
  most are not AWAKE; the scan is a few bitmap words and one flags byte per
  live header in Rust.
- **Early movers** (lifts, vehicles) and the proximity wake `109580` matter
  far more than in multiplayer; `109660` stays a callout and should be read
  in slice 0.
- **Types absent from Waterworks:** creatures (`118700`, `118b80`), devices
  and machines (`106790`, `1152e0`, `115a40`), crates. Their slices are
  ordered by the phase 0 campaign census, not by the table in section 1.
- **Scripts** create, delete and attach objects outside the walks; nothing
  here caches across that.
- **Lockstep.** Both peers run the same build and switches, so the callout
  contract's exactness is not needed for co-op to stay in sync; it is needed
  so that `h2 sim diff` against the lifted build proves each slice.
- **Halo 1 content** runs the same bodies. Halo 1 health hooks
  `OBJECT_DAMAGE_UPDATE`; slice 8 keeps that extension point: a body
  another feature hooks is guarded, so its chain runs the hooks around
  the Rust body as around the lifted one.

## 10. Invalidation

| State | Dropped when | Why it is safe |
|---|---|---|
| `DefTable` | epoch bump: map load, revert `123f60`, full restore `124380`, snapshot load, tick regression, hot tag edit | rebuilt from tag bytes; a revert does not change tags, so the bump is conservative |
| `Spec` results | walk exit; any epoch bump; key mismatch | `take` validates inputs anyway |
| `ObjRef`, `Objects` | end of the segment | never stored |
| Event buffer | first walk of a new tick; epoch bump | not persistent state |
| Type-table slot cache | never (image constants) | |

There is nothing else. No per-object Rust state survives a segment.

## 11. Verification

The plan's ladder, with these specifics.

1. **Unit tests (`sim-objects`, `sim-core`), synthetic data only.** The
   live-slot scan at word boundaries; child relink during a walk; each
   kernel's edge cases (scale exactly 1, NaN centre, threshold equality in
   `bdef0`); `Spec` with 1, 2 and 8 workers giving identical bytes; `Spec`
   with every input class perturbed between `prepare` and `take`; `Spec`
   never prepared.
2. **Segment differential tests** (`crates/rewrites/engine/tests/sim_objects_differential.rs`,
   `differential` feature). The existing harness model: run the owner's XBE
   on the IR interpreter to the next call or return and compare with the
   Rust segment. Compared: all memory at and above `ESP`, all memory outside
   the stack, eight integer registers, defined flags, the XMM registers,
   the x87 state, transfer target.
   Masked: the dead stack below `ESP`. Seeded callee answers exercise
   register clobbers and changes to the live set (create, delete, relink,
   pool compaction that moves the body) between segments. Add two things the
   current harness lacks:
   - **fused transcripts**: run a whole root (for example `b6f10`) with the
     dispatcher fusing native bodies, and require the sequence of true
     callouts (target, registers, memory) to equal the lifted sequence;
   - **mixed frames**: enter with the lifted code, switch to the Rust body
     at each site in turn, and the reverse.
3. **Oracles.** The ports in `sim_physics` (`objects.rs`, `object_update.rs`,
   `colors.rs`, `light.rs`, `unit_aim.rs`) and `sim_native` (`object.rs`,
   `object_base.rs`, `unit.rs`, `marker.rs`) stay as test oracles: each new
   body is also compared with the port it replaces before the port's switch
   is deleted.
4. **Call captures** for states too rich to synthesise (unit and biped
   updates): registers and RAM at the n-th call of a point, from a
   Waterworks match and from a campaign level; kept in the user's data
   directory, never committed.
5. **Run parity**, the lifted build against the bodies, with
   `impl.sim.arena-hash`: the three sim-budget cases, plus a headless co-op
   campaign level through the agent API. Every `state_hashes_v1` row equal;
   `h2 sim diff` prints nothing (its per-component breakdown names the pool
   on a miss).
6. **Cache-cold arm:** `DefTable` cleared every tick and `Spec` never
   prepared; identical hashes.
7. **Worker arm:** 1, 2 and 8 workers; identical hashes.
8. **Cross-architecture:** the Waterworks case and the campaign case on
   x86-64 Linux and arm64 macOS; identical hash series.
9. **Behaviour:** the headless Halo 1 suite before the user plays.
10. **Timing:** three unprofiled medians per arm and one profiled pair per
    slice; the window table in section 1 is the before column.

## 12. Implementation slices

Each is one lane. Lanes write code only; the coordinator builds and tests.
Every slice updates `docs/engine/object-update.md` with the contracts it
read, and deletes the per-function switches it replaces (their bodies stay
as oracles while a test uses them).

| # | Slice | Contents | Depends on | Testable result | Samples addressed |
|---|---|---|---|---|---:|
| 0 | Research and scaffolding | Contract page with the open items of section 13 resolved by static reading; register live-in/live-out scan for every entry and callout in slices 1 to 5 (linear scan from each call site and each callee entry until every register is written); crate skeletons; `BODIES` and `drive` with no bodies; installer; the two harness additions | `sim-core` exists | dispatcher unit tests; harness runs with zero bodies | 0 |
| 1 | Walks | `b6f10`, `b7060`, `b7150`, `bc470`, `bc820` adopted from the ports into `BODIES`; dispatchers `108c60`, `108cd0`, `109050`, `1090d0`; leaves `b7360`, `baf80`, `bba20`, `bc7b0`; `109580`; `ObjectDeleted` events. Retires `native_objects_pre_physics`, `native_objects_post_physics`, `native_object_pre_update`, `native_object_post_update` | 0 | fused transcripts for the three roots; run parity | 894 (`bc470` 148, `bc820` 126, `bc7b0` 98, `bba20` 79, `b6f10` 77, `b7150` 70, `108c60` 67, `b7060` 60, `baf80` 44, `109050` 42, `b7360` 36, `1090d0` 28, `108cd0` 19) |
| 2 | Views and `sim-state` | `layout`, `view`, `DefTable`, `sim-state` objects API, unit tests on fake memory. Can run beside slice 1 | 0 | unit tests; an agent API method `sim.objects` listing typed objects (documented in agent-api.md, per AGENTS.md) | 0 |
| 3 | Functions and colours | `bab40`, `ba8c0`, `108d90`, `bcc20`, `be8e0`; import handlers `c6b90` (with `d1210`, `d1410`), `ff5f0`, `f6d10`, `de3b0`, `fbe70`, `106d70`, `11bd60`. Retires `native_object_import`, `native_object_value`, `native_base_object_import`, `native_object_change_colors` | 1 | segment tests; evaluator becomes a leaf when all seven imports are in | 1,713 (`108d90` 317, `bab40` 282, `bcc20` 253, `f6d10` 224, `ff5f0` 204, `ba8c0` 163, `c6b90` 138, `13bb90` 74, `be8e0` 28, `d1210` 18, `de3b0` 12) |
| 4 | Post-update shell | `bdef0` arithmetic, `bef30`, `be1d0`, `bc5e0`, transform setters `b7430`, `b7740`, `b9dd0`, `b9fc0`, `ba1d0`; `bd090` orchestration adopted and joined to `sim-anim` | 1, phase 1b | segment tests; parity | 1,038, plus the orchestration share of `bd090` (490 shared with phase 1b) (`ba1d0` 167, `bdef0` 156, `b9dd0` 132, `bef30` 98, `b7430` 75, `b9fc0` 73, `bc5e0` 69, `b8600` 51, `b9d20` 48, `109140` 44, `b9ce0` 43, `b91d0` 28, `b7740` 26, `b87b0` 18, `b8c00` 10) |
| 5 | Lights | `c1400`, `c0c80`, `c0b00`, list operations `c2d00`, `c3220`, `c3260`. Retires `native_light_record` and `static-light` (the unconditional body subsumes the bypass) | 3 | segment tests; parity on the light profile | 671 |
| 6a | Unit: shell and movement | `c6b30`, `c60c0`, `cba50`, `c85c0`, `c7840` (from the port, if phase 1c has not taken it). Retires `native_unit_update` | 3 | captures from Waterworks and a campaign level | 960 (`c7840` 325, `c60c0` 260, `cba50` 195, `c85c0` 112, `c6b30` 68) |
| 6b | Unit: state | `c49b0`, `c58f0`, `c5eb0`, `ce0c0` | 6a | captures | 578 (`ce0c0` 195, `c49b0` 158, `c58f0` 150, `c5eb0` 75) |
| 6c | Unit: rest | `c6740`, `114c60`, `c6990` (with `d0620`, `d0690`), `c6810`, `c50a0`, `111650` (unit animation state, `a0e` bytes) | 6a | captures | 456 (`111650` 210) |
| 7a | Biped update | `dd360` and its tree (`e24f0`, `e01d0`, `dc5c0`, `df380`, `de190`, `e4050`, `e0dc0`, `e6830`, `e2d80`, …) | 6a | captures | 963 |
| 7b | Biped post-physics | `dd990` and its tree (`dd0d0`, `dd7d0`, `e30c0`, `dd8e0`, `e4680`, …) | 7a, 4 | captures | 280 |
| - | **Phase 3 exit check** | Ladder clean; Waterworks at least 1.2 ms/tick faster than before slice 1; co-op campaign delta recorded | 1 to 7 | | |
| 8 | Damage | `d5de0`; `Damage` and `Kill` events; the Halo 1 health extension point kept | 1 | captures; events equal the hook-derived ones over a match | 403 |
| 9 | Weapon | `fdfb0` (`15c6` bytes; split by state group if a lane cannot finish it) | 3 | captures | ~760 |
| 10 | Vehicle | `efde0` and tree | 6a | captures | ~1,450 |
| 11 | Small types, campaign order | `10b4d0`, `10a2f0`, `106790`, `1152e0`, `115a40`, `f8de0` with `f8eb0`, `11bce0`, `118700`, `118b80`; order from the campaign census | 1 | captures from campaign | ~720 on Waterworks |
| 12 | Parallel pose (gated) | Step 0: measure nodes posed per tick and time left in pose kernels on Waterworks and co-op campaign. Go only at 0.25 ms/tick or more. Then `Spec`, `Jobs`, `PoseSet`, waves, the worker arm | 4, phase 1c | `Spec` unit tests; worker arm | measured |
| 13 | Retire | Native only: no switch and the per-function switches in the plan's phase 3 list are gone | exit | | |

The functions named in slices 1 to 7 hold 7,553 samples (8.95% of the
thread, 1.26 to 1.60 ms). At 4× that removes 0.95 to 1.20 ms, which is at the
edge of the exit figure: the window totals in section 1 are larger than the
named functions because each tree has a tail of small callees. Expect the
exit to need either those tails or one of slices 8 to 10 (weapon and vehicle
add 2,208 samples). Measure after slice 7 before deciding.

Slices 1 and 2 can start today. Slice 3 needs only slice 1. Slices 6 to 11
are independent of each other once their dependencies are in, and are
ordered here by Waterworks samples; the campaign census may reorder 8 to 11.

Files each lane owns: its own modules under `crates/rewrites/sim-objects/src` and
`crates/rewrites/engine/src/sim_objects`, one row block in `BODIES`, its section of
the contract page, and its test file section. Shared edits (`BODIES`,
`points.rs`, the installer) are append-only.

## 13. Risks, and what this design could not resolve

**Risks.**

- **Uninitialised stack reads.** Rule 4 makes bodies leave the dead stack
  as retail does. A body that missed a store there is exact only if no
  guest callee reads a local it did not write. Retail code can.
  The segment harness masks the dead stack, so it would not see it; run
  parity would, as a hash difference with no failing segment. Mitigation: in
  the harness, run each *lifted* callout target twice with two different
  dead-stack fills and require equal results; a difference names the
  function, and the body before it then reproduces the stale slots that
  callee reads.
- **Register liveness under link-time optimisation.** Rule 2 keeps all
  eight registers exact, which costs lane time on every new body. The scan
  in slice 0 can relax it per site, but a wrong scan is a silent desync.
  Keep "all eight exact" unless the time is needed.
- **Frame fidelity is shape fidelity.** Segment bodies keep retail's frame
  layout at call sites. That is less freedom than the plan's non-goal
  ("no call graph or register state inside a subsystem") suggests; the
  freedom is real only between callouts and in leaf kernels. It goes away
  as callouts go away.
- **The win is callout-bound.** A body with many guest callouts saves
  little: each is a runtime dispatch (`resolve_resume` alone is 0.78% of the
  thread today). Slice order follows "port the callees that turn a segment
  body into a leaf".
- **Type trees are large.** `fdfb0` is `15c6` bytes with 42 distinct
  callees; `efde0` `8b9` with 36; `dd360` `45e` with 25. Each is weeks of
  reading for a lane, and the profile says the per-function payoff is thin.
  Stop when the exit criterion holds and the campaign census does not ask
  for more.
- **Overlap with phase 1.** `bd090` and `c7840` have two owners. Whoever
  lands second adopts the first body into its table; neither rewrites the
  other's arithmetic.
- **Census gap.** Every number here is Waterworks with bots. Campaign may
  rank creatures, devices and damage far higher.
- **`Spec` input completeness.** A missed input is a wrong result only when
  that input changes between `prepare` and `take`, which is rare and
  therefore hard to see. The perturbation test must enumerate inputs from
  the kernel's read set, not from the gather code.

**Read from the lifted code.** The DEFER and NEW setters, the early-mover
list at `[5107f0]+1d00`, the record `109660` writes and when riders read it,
the network-mode byte that `bc470` step 3 tests, `fd910`/`baeb0`, `bf380`,
`be1d0` → `109140`, the slot `1090d0` dispatches, the register conventions
in section 5, the RNG question (no draw is reachable from function values
or imports by direct calls) and the unit offsets are contracts in
[object-update.md](object-update.md).

**Not resolved (they need a run, not a read).**

- Nodes posed per tick (the estimate in section 7).
- How the logical clock should be charged for a long Rust segment
  (`charge_accesses` in `runtime/src/preempt.rs`): today a port is charged
  only for the guest accesses it makes, which is deterministic, and this
  design keeps that.

## Summary

1. Phase 3 replaces the three object walks, the per-object pre and post update, the function evaluator, lights and the type update trees with Rust bodies, native only.
2. Addressable cost on Waterworks: core 4,749 samples (5.63% of the thread), type trees 8,520 (10.10%); the 1.2 ms exit needs the core plus the unit and biped trees, and is at the edge on the named functions alone (0.95 to 1.20 ms).
3. There is no persistent Rust object table: typed views are the access path and SoA exists only as per-phase working sets.
4. Bodies are segments over retail stack frames with all eight registers exact at every call out, so no Rust state crosses a guest call and the lifted code and Rust can swap at any return site.
5. A local dispatcher fuses ported bodies; only calls to still-guest code reach the runtime.
6. Type, damage and wake updates stay serial in retail order for good; pose, bounds, function values and light records are pure kernels.
7. Parallelism is validated speculation: workers precompute, the serial walk re-gathers inputs and reuses a result only on a byte-exact match, so output never depends on worker count.
8. Theater and stats read typed `ObjectView`s from the arena at tick end; poses are a copy of the node matrices; events come from the serial bodies.
9. Co-op campaign uses the same bodies; AI calls stay guest callouts in retail order, and the small-type slices are ordered by the campaign census.
10. Thirteen slices; 1 (walks) and 2 (views and `sim-state`) can start now; parallel pose is last and gated on 0.25 ms/tick measured.
