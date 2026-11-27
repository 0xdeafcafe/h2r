# Live game state: objects, hooks and the structure map

Part of [Halo 2 retail Xbox live game-state map](live-gamestate.md). Covers: 6. Objects, positions, facing, and vehicles; 7. Kill/death and damage hooks; 8. Structure BSP and a web top-down map.

## 6. Objects, positions, facing, and vehicles

### Object-header array

`objects_initialize` at `0x000b67c0` creates **2,048 twelve-byte headers** and publishes `H` at `0x004e0300`. Header `i` is at `read_u32(H+0x44) + i*0x0c`.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| datum salt | `+0x00` | `u16` | confirmed | Generic datum contract and `0x000badc0` |
| header flags | `+0x02` | `u8` | likely | Xbox header layout and iterators; individual flag meanings are not needed |
| concrete object type | `+0x03` | `u8` | confirmed | Read by `0x000badc0` for its type mask |
| cluster/location index | `+0x04` | `i16` | likely | Matching Xbox layout; not needed for player markers |
| body size | `+0x06` | `u16` | confirmed | Stored by allocator `0x000bc280` |
| object-body pointer | `+0x08` | `u32` | confirmed | Allocation and every typed lookup |

Safe unit lookup, equivalent to the relevant checks in `0x000badc0`:

```text
reject datum == 0xffffffff
slot = datum & 0xffff
reject slot >= read_i32(H+0x38)
header = read_u32(H+0x44) + slot*0x0c
reject read_u16(header+0) == 0
reject read_u16(header+0) != (datum >> 16)
reject ((1 << read_u8(header+3)) & 0x3) == 0   // biped or vehicle
body = read_u32(header+8)
reject body == 0
```

`0x000badc0` itself has custom `EAX = object datum`, one stack type-mask argument, `ret 4`, and returns the body pointer or zero in `EAX`. Unit mask is `0x3` (`0` biped, `1` vehicle).

Do not cache body pointers between snapshots: the object memory pool can compact and rewrite the header's pointer (`docs/engine/memory_pool.md`). Never scan the one-MiB object pool for players; follow only player unit datums and, optionally, their parents.

### Common object/unit body

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| definition tag datum | `+0x00` | `u32` | confirmed | Object creation and tag consumers |
| parent object datum | `+0x14` | `u32` | confirmed | Transform composition and parent walks |
| bounding center | `+0x30` | `f32[3]` | confirmed | Not the scoreboard origin |
| object origin | `+0x64` | `f32[3]` | confirmed | Written by transform setter `0x000b7430` |
| forward/facing vector | `+0x70` | `f32[3]` | confirmed | Written by `0x000b7430`; used by `0x000ba160` |
| up vector | `+0x7c` | `f32[3]` | confirmed | Written by `0x000b7430` |
| translational velocity | `+0x88` | `f32[3]` | confirmed | Recovered velocity writer; world units a second, not a tick: the projectile update moves `+0x64` by it times the seconds per tick (`0x000f97df`..`0x000f9803`) |
| concrete object type | `+0xaa` | `u8` | confirmed | HaloScript/object type tests |
| body vitality | `+0xec` | `f32` | confirmed | `object_get_health` `0x002a1a20` (0 while body-depleted) |
| shield vitality | `+0xf0` | `f32` | confirmed | `object_get_shield` `0x002a1a90` (0 while body-depleted) |
| damage flags | `+0x10a` | `u16` | confirmed | Bit 2 is body-depleted, set by `0x000d6800` |
| unit's player datum | `+0x13c` | `u32` | confirmed | `0x0014de90` |
| previous player datum | `+0x140` | `u32` | confirmed | Unlink path `0x0014cad0` |
| vehicle seat index | `+0x1fc` | `i16` | confirmed | `unit_new` sets -1 (`0x000c44c1`); the enter-vehicle routine `0x000ea1f0` indexes the vehicle's seats block (stride `0xb0`) with it and stores it (`0x000ea405`); leaving stores -1 (`0x000ea995`) |

For an unparented unit, marker position is `body+0x64`, and heading is `atan2(forward.y, forward.x)`; the world is Z-up. `0x000ba160` shows that parented objects compose their local transform with a parent node matrix. Therefore, for a player in a vehicle, the cheap robust path is:

1. Read the unit's parent datum at `+0x14`.
2. Salt-check that object and require concrete type `1` (vehicle).
3. Use the vehicle body's `+0x64` origin and `+0x70` forward, and optionally the unit's `+0x1fc` seat.
4. If drawing a parented biped rather than its vehicle, compose the full object matrix instead of assuming `+0x64` is world-space.

### Unit aim, held weapon, species and shots

Added 2026-11-16 for the viewer's player models, from static reading of the
XBE with an iced-x86 disassembly (no game run). Offsets are into the unit
body (`H` lookup above); `W` is a weapon object body.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|
| current weapon index | unit `+0x212` | `i8` | confirmed | `unit_new` `0x000c4410` sets it to `-1`; `0x000c58f0`, `0x000c85c0` and `0x000cf3d0` sign-extend it and index `+0x218` with it |
| second weapon index | unit `+0x213` | `i8` | confirmed layout; likely dual-wield | Set to `-1` with `+0x212`; `0x000c85c0` loops over both bytes |
| weapon object datums | unit `+0x218` | `u32[4]` | confirmed | `unit_new` sets all four to none; network-state builder `0x0009d450` walks the four slots and reads each weapon's magazines |
| aiming vector | unit `+0x168` | `f32[3]` | likely | Layout confirmed: `unit_new` copies the object forward (`+0x70`) into `+0x150`, `+0x15c`, `+0x168`, `+0x180`, `+0x18c`. `0x000cf3d0` casts the held weapon's targeting ray (collision test `0x001697c0`) from the unit origin `+0x64` along `+0x168` times the weapon definition's range (`+0x1b8`), so it is the world-space aim. Whether it is the smoothed or the desired vector is open: `0x0009d450` networks `+0x15c` instead |
| weapon owner | `W+0x14` | `u32` | confirmed | The ordinary parent datum; `0x00101fb0` finds a weapon's owning unit through it (unit mask `3` at `0x000badc0`) |
| magazine state | `W+0x228 + 0x10*i` | 16 bytes | confirmed layout | `0x00101a10` sets reserve `+0x22a` and loaded `+0x22c` (`i16`) from the definition's magazine block (`weap+0x2c0` count, `+0x2c4` pointer, stride `0x5c`); `0x00101740` moves ammunition between weapons. Not used: plasma weapons have no magazine |
| barrel state | `W+0x1a4 + 0x34*i` | `0x34` bytes | confirmed layout | Byte `+4` bit 1 (`W+0x1a8`) is a pending-fire flag: `0x000fd980` fires each flagged barrel and clears the flag |

**Held weapon:** `index = (i8) unit[+0x212]`; for `0..3`, `weapon = unit[+0x218 + 4*index]`. Check the weapon datum like a unit (salt, high-water, non-null body), require concrete type `2`, and require `W+0x14 == unit datum`. The weapon's definition tag datum is `W+0`.

**Species and weapon names:** an object's definition tag datum (`body+0`) is the full salted datum of its tag-index entry. Tag *names* are, likely, not in guest memory: the cache loader's read (`0x00122a52` calling `0x00122d60`) is sized from the header's index/metadata fields (`0x00547710..0x0054771c`), and no instruction references the header's tag-name fields (`0x005479c0..0x005479cc`) by absolute address. The map file on disc has the same index, so the host reads the names there; a name table is trusted only when the file's scenario datum equals the guest's, read from the tag-index header at `[0x00547f00]+0x0c` (the loader reads it at `0x00122ad8` and publishes that tag's metadata pointer at `0x004e0350`). Multiplayer bipeds are named like `objects\characters\masterchief\masterchief_mp` and `objects\characters\elite\elite_mp`.

**Shots:** `0x00104150` is the barrel fire: stdcall `(weapon object datum, i16 barrel index)`, `ret 8`, with a `0x1e98`-byte frame. Its only direct caller is the pending-fire loop `0x000fd980` (reached from `0x000a2df5` and `0x000fd95e`). It resolves the owner through `0x00101fb0`, reports the shot to the game engine (`0x0015cca0` with the owner's player datum), and creates the projectiles of every firing marker of the barrel through `object_new` (`0x000b7b40`). One call per shot is **likely** (a shotgun's pellets are created inside one call; a battle-rifle burst should be three calls). A hook on it counts shots for every weapon type, including those without magazines, which a rounds-loaded delta (`W+0x22c`) would miss; the delta would also need reload and pickup filtering.

### AI bipeds, teams and vehicles

Added 2026-11-16 for campaign frames (static reading, no game run).

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|
| actor datum | unit `+0x12c` | `u32` | confirmed | `unit_new` `0x000c4410` sets it to none; `0x000c6810` indexes the actor datum array (`[0x004f55f0]`, element stride `0x888`) with it and copies the actor's team (`actor+0x24`, `i16`) into the unit |
| player datum | unit `+0x13c` | `u32` | confirmed | Section above; set to none by `unit_new` |
| team | unit `+0x138` | `i16` | confirmed | `unit_new` takes it from the placement (`placement+0x64`), and in campaign (`[G+8] == 1`) when that is 0 or -1 from the unit definition's default team (`+0xc0`); `0x0014bfc0` copies the player's team (`player+0xc0`); `0x000c6810` the actor's; `0x000b7930` hands `(i16)+0x138` on as the owner team of objects the unit creates |
| up vector | `+0x7c` | `f32[3]` | confirmed | Section above |
| campaign team names | - | enum | likely | 0 default, 1 player, 2 human, 3 covenant, 4 flood, 5 sentinel, 6 heretic, 7 prophet: the tag definitions' order, not checked against code |

An AI biped is a biped object (type 0) with an actor datum, no player datum,
and damage flags (`+0x10a`) without bit 2. A unit's vehicle is its parent
when the parent is type 1; the vehicle's own `+0x64/+0x70/+0x7c` are world
space unless the vehicle is itself parented (turrets on dropships).

Campaign re-activation: `0x0018e700` copies the active options (`G+8`,
`0x1118` bytes), re-selects the BSP and calls `0x0018f000` again, so the
activation hook also fires on checkpoint reverts and restarts of the same
map.

### Unit stance, actions, damage and machines

Added 2026-11-16 for frame `stance`, `actions`, `hits` and `machines`
(static reading, no game run). Enum value names not established by the
code cited are marked unverified.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|
| unit flags | unit `+0x134` | `u32` | confirmed | Bit 23 asks to crouch: the crouch update at `0x000dd83f` tests it |
| crouch amount | unit `+0x2c4` | `f32` 0..1 | confirmed | `0x000dd83f` moves it toward 1 (bit 23 set) or 0 by a rate times the tick length; `biped_new` initialises it |
| selected grenade type | unit `+0x23c` | `i8` | confirmed index; names unverified | `0x000cdff0` and the HUD (`0x002759cf`) index `+0x23e` with it; 0 frag, 1 plasma (unverified) |
| grenade counts | unit `+0x23e` | `i8[2]` | confirmed | As above; `0x000cbef3` tests each |
| biped physics mode | biped `+0x3dc` | `u8` | likely | `biped_new` (`0x000de5f6`) clears it; code compares it with 1, 2, 3, 5 and 6 (`0x000dd897` in the crouch update: 6). Value names unverified |
| translational velocity | `+0x88` | `f32[3]` | confirmed | Above; telemetry uses only the sign of z |
| held weapon magazine | `W+0x22a` / `W+0x22c` | `i16` | confirmed | Reserve and loaded rounds, above |
| scenario placement index | `+0x1a` | `i16` | confirmed | `object_create` by name (`0x000bb670`) passes the name's placement index to `0x000bf0f0`, which hands it to the placement-data builder `0x000d5060` (call `0x000bf152`; stored at placement data `+0x10`, `0x000d520b`); `object_new` copies placement data `+0x10` here (`0x000b7c87`) with the object identifier (`+0xa4`, `+0xa8`); -1 for objects the scenario did not place (`0x000b7c63`). Index into the object type's scenario block (docs/live/live-telemetry.md, "Scenario objects") |
| netgame equipment index | `+0xaf` | `i8` | confirmed | The netgame spawner `0x0015aad0` writes the spawn's index into `scnr+0x120` here (`0x0015ac15`) after `object_new`, which sets `0xff` (`0x000b7db1`); deletion (`0x000b85c3`) calls `0x0015b220` to clear the spawn's object |
| machine object type | header `+3` = 7 | `u8` | likely | The engine's type order (biped 0, vehicle 1, weapon 2 confirmed); the scarab of `03a`/`03b` is a `mach` tag, in `08a` a `scen` |
| equipment and projectile object types | header `+3` = 3, 5 | `u8` | confirmed | The type table at `0x00468630` (bipd 0, vehi 1, weap 2, eqip 3, garb 4, proj 5, ...; `layout::OBJECT_TYPE_*`). Frames send projectiles (`proj`, position `+0x64`, velocity `+0x88` as the projectile update moves them, `0x000f8ee2` on) and loose weapons and equipment (no parent, `+0x1a` and `+0xaf` both -1) from the world scan (packed version 6) |
| device position | device `+0x140` | `f32` 0..1 | confirmed | The machine network-state builder `0x000a3df0` copies it under flag `0x400`. The machine update at `0x0011546d` adds the tick's travel to it. The device code passes it to the normalised-frame setter `0x001c6620` at call site `0x00106930`. |
| device power | device `+0x134` | `f32` 0..1 | confirmed | The setter's caller at `0x00106945` and `0x0010695d` uses it to blend powered and depowered rates. The viewer needs only the resulting position. |
| device position channel | device `+0x18c` | animation channel | confirmed | `0x00108721` takes its address when starting the position action. The existing channel layout applies. Its graph datum is at `+0x18c` and its signed animation index at `+0x192`. The setter `0x001c6622` checks the index at channel `+6` for -1 and resolves the animation using the graph at channel `+0`. |
| damage reporting type | report `+0x00` low 6 bits | enum | likely | 6-bit value; value names unverified |

`0x000d9640` applies a report to its victim: it adds `+0x44` (clamped to 1
when report flag bit 0 is set) and `+0x48` to the victim's recent-damage
accumulators (`+0x100`, `+0xf8`), which is why the amounts read as
fractions of vitality. It has four callers (`0x0009c76e`, `0x000d8292`,
`0x000d87a2`, `0x000dc211`).

## 7. Kill/death and damage hooks

### Best player-facing event: `0x0015ce70`

Standard four-argument stack convention, callee pops 16 bytes:

| Argument | Stack offset at entry | Type | Meaning |
|---|---:|---|---|
| attacker player datum | `[esp+0x04]` | `u32` | Finally attributed killer; `0xffffffff` for none |
| victim player datum | `[esp+0x08]` | `u32` | Victim player |
| unknown flag | `[esp+0x0c]` | `u32` | **Not** a betrayal flag: it is set for free-for-all kills between enemies too (seen 2026-11-19, a Clankers free-for-all whose every kill was sent as a betrayal). Nothing reads it; a betrayal is a teammate killed in a team game (`live_proto::game::is_betrayal`). |
| packed damage code | `[esp+0x10]` | `u8` in low byte | Low 6 bits damage reporting type, high 2 bits modifier |

The direct call is at `0x001ea460` inside attribution routine `0x001e9fa0`. It follows recent-damage attribution, so the killer is final. It is not universal for campaign AI or arbitrary units: a game engine/statborg and a victim player index are required.

The packed code belongs to the credited damage source. If recent damage
earns the kill, the caller takes its code from the victim's attribution
entry (`0x001ea16c`). When the credited player dealt the final damage it
uses that damage's code (`0x001ea1d4`). Both paths pass the code at
`0x001ea45c`.

Live reads the actual killing damage from the active damage report. On the
base image the sole direct call to attribution is at `0x000d9aca`, after
the report's body-depleted flag is checked at `0x000d9aa2`. This applies
to falls and kill volumes that reach the player-kill hook as well. At
attribution entry `EDI` still holds the report. The prologue saves it at
`0x001e9fb6`. When the kill hook sees the return to attribution
(`[esp] = 0x001ea465`) and attribution's return to the damage reporter
(`[esp+0x74] = 0x000d9acf`), the saved report pointer is at
`[esp+0x1c]`. The guest-layout reader checks both sites and the depletion
flag before using the report's code and owner. The attribution argument
slot for the incoming code is reused as an assist counter at
`0x001ea2c7`. An unrecognised return to the damage reporter emits one diagnostic.
A read fault or unrecognised return on this path leaves the source unknown. A
direct call outside attribution supplies its own damage code in argument
four. A delayed grenade keeps its identity if its owner switches weapons
or dies, and a fall still reports falling when another player earns credit.

- Modifier 1 marks a headshot. The body-damage path checks the hit
  section and damage effect at `0x000d8d69` and `0x000d8d75` before it
  sets the modifier at `0x000d8db4`. Report construction copies that
  packed byte at `0x000d955c`.
- The kill handler classifies generic melee, the sword and objective
  strikes as melee at `0x0015cecd`. It treats modifier 2 as an
  assassination for melee and as a stick for a plasma grenade at
  `0x0015d295` and `0x0015d2bf`.
- The roadkill branch at `0x0015d2b0` recognises a player collision or
  modifier 3. For a generic collision Live compares the lethal report's
  owner player with the victim. World and self collisions retain the
  collision source when that modifier is absent. Environmental deaths
  carry the source the engine reports, including falling and guardians.

Timing matters:

- At entry, an enemy killer's kill counter has already been incremented, but the engine's score callback near the end of `0x0015ce70` has not necessarily run.
- After `0x0015ce70` returns, the victim death/suicide/betrayal and assist accounting can still be pending in `0x001e9fa0`.
- Recommended: emit the event arguments immediately, then publish the authoritative stat rows at the next completed-tick sample. If a hook must see all death stats complete, wrap `0x001e9fa0` and emit after its return.

`0x001e9fa0` has five stack args, `ret 0x14`: statborg pointer, victim object datum, reported player datum, reported team (`i16` in the arg slot), and packed damage code. It performs final killer/assist attribution and all associated stat updates.

### Damage-aware alternative: `0x000d9640`

Custom ABI: `EDI = damage_report *`, one stack arg victim object datum, `ret 4`; void. Filter `(report.flags & 1) != 0` for the body-depleted transition.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| packed damage code | `+0x00` | `u8` | confirmed | Low 6 bits type, high 2 bits modifier |
| report flags | `+0x04` | `u32` | confirmed | Bit 0 body-depleted/death transition |
| damage-effect (`jpt!`) tag datum | `+0x08` | `u32` | confirmed | Resolved through the tag-entry table |
| reported owner player datum | `+0x0c` | `u32` | confirmed | Copied to unit owner state |
| reported owner object datum | `+0x10` | `u32` | confirmed | Used for owner/current-weapon paths |
| reported owner team | `+0x14` | `i16` | confirmed | Team/hostility handling |
| direction | `+0x18` | `f32[3]` | confirmed | Damage report consumers |
| origin | `+0x24` | `f32[3]` | confirmed | Damage report consumers |
| body-damage amount | `+0x44` | `f32` | likely | Body-vitality update path |
| shield-damage amount | `+0x48` | `f32` | likely | Shield-vitality update path |
| distance | `+0x4c` | `f32` | confirmed | Damage report layout |

The `jpt!` datum is the most precise damage identity here; it is not a weapon tag. Current-weapon lookup through the reported owner is only corroboration and is not reliable for grenades, delayed projectiles, vehicles, melee, or recent-damage attribution. Use `0x0015ce70` for final killer identity.

For arbitrary object death without player attribution, `0x000d6800` is the body-depleted transition. It takes four stack args (`object datum`, `damage_owner *`, `notify_parent`, additional flag), `ret 0x10`. The owner is player datum `+0`, object datum `+4`, team `i16 +8`; it has no damage code or damage tag.

## 8. Structure BSP and a web top-down map

### Scenario BSP selection

The scenario's structure-BSP block is at `scnr+0x210` (count/pointer), with records of stride `0x44`.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| scenario BSP block count | `scnr+0x210` | `u32` | confirmed | `0x0011c1b0` and viewer decoder |
| scenario BSP block pointer | `scnr+0x214` | `u32` | confirmed | Same |
| BSP tag datum in record | `record+0x14` | `u32` | confirmed | Loaded by `0x00122b40` |
| lightmap tag datum in record | `record+0x1c` | `u32` | confirmed | Loaded by the same switch |

Use `G+0x02` to select the active record. Do not assume BSP zero; campaign maps switch BSPs. `0x004e0348` is the active resolved BSP tag pointer and can be used as a consistency check.

### Geometry needed for bounds and rendering

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| cluster block | `sbsp+0x9c` | block, stride `0xb0` | confirmed | Engine `0x00041040` and `StructureRender` |
| material block | `sbsp+0xa4` | block | confirmed | Part material validation |
| instanced definitions | `sbsp+0x138` | block, stride `0xc8` | confirmed | Engine and decoder |
| instanced placements | `sbsp+0x140` | block, stride `0x58` | confirmed | Engine and decoder |
| cluster world bounds | `cluster+0x54` | `f32[6]` | confirmed | `[xmin,xmax,ymin,ymax,zmin,zmax]`; whole-disc vertex checks |
| instance uniform scale | `placement+0x00` | `f32` | confirmed | Whole-disc placement validation |
| instance forward/left/up basis | `placement+0x04/+0x10/+0x1c` | three `f32[3]` | confirmed | Orthonormal/right-handed checks |
| instance translation | `placement+0x28` | `f32[3]` | confirmed | World placement checks |
| instance definition index | `placement+0x34` | `i16` | confirmed | Engine `0x000410c5` |

Instance transform: `world = translation + scale * (x*forward + y*left + z*up)`.

There is no single decoded BSP world AABB in the repository. For exact bounds, reduce every vertex of every world-space triangle, including transformed instances. Cluster bounds are useful per-cluster approximations, not a substitute for that reduction.

### Reusing the viewer for the web

The standalone viewer's `bsp` tool (removed; see `git log -- crates/tools/viewer`) had `world_triangles`, which builds all world-space triangles for one BSP:

- cluster positions are already world-space (`:613-616`);
- definitions are loaded once and transformed per placement (`:618-626`);
- drawn-part filtering and triangle expansion are already implemented (`:600-610`).

The public lower-level APIs are:

- `StructureRender::read` - `crates/formats/tags/src/structure.rs:288`;
- `GeometryHeader::load` - `crates/formats/tags/src/structure.rs:135`;
- `Section::vertices(None)` - `crates/formats/tags/src/geometry.rs:928`;
- `Section::part_triangles` - `crates/formats/tags/src/geometry.rs:917`;
- `InstancedGeometryInstance::transform` - `crates/formats/tags/src/structure.rs:213`.

Two practical export routes:

1. **Lowest effort:** revive the removed `bsp --render` path offline and ship its top-down PNG plus the world-to-image scale/offset. The renderer looks down Z and culls faces with `normal.z <= 0`. Its framing is the 1st–99th percentile of triangle centroids, not exact world bounds (`:707-725`), so export those computed framing values rather than recomputing an AABB on the server.
2. **Vector/interactive:** refactor the private `world_triangles` logic into an offline exporter, project drawn triangles to XY, and emit quantized triangles, an occupancy raster, or boundary edges as SVG/JSON. This currently has no triangle/SVG/GeoJSON/outline CLI. Use exact vertex bounds if the vector output should include everything; optionally retain the renderer's back-face/percentile policy to hide the enclosing shell.

Precompute every scenario BSP offline. Runtime traffic then needs only `{map, bsp_index}` and player XY/heading, not live BSP geometry.
