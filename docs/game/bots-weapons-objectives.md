# Bots: weapons and objectives

Weapon choice, ammunition and the objective modes (sections 3.6 and 3.7). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 3.6 Weapons and ammunition (traced)

The user, after playing the Clankers: "they don't know when they have no
ammo". The bot now reads what it carries and what lies about
(`crates/live/bots/src/arms.rs`), and a pure decision (`arms::plan`) turns
that into a reload, a weapon switch, a walk to a weapon, or a melee.

**Fields.** Traced offline from the XBE with `h2 disasm` (static reading,
no game run). `U` is the unit's object body, `W` a weapon's, `D` the weapon's
`weap` tag metadata (`W+0` is its tag datum; the tag entry's `+8` the
metadata address, `tag_entry_checked`).

| Field | Where | Type | Confidence | Evidence |
|---|---|---|---|---|
| held weapon slot | `U+0x212` | `i8`, -1 none | confirmed | docs/live/live-gamestate.md ("Unit aim, held weapon"): `unit_new` `0xc4410` sets -1; `0xc58f0`, `0xc85c0`, `0xcf3d0` index `+0x218` with it |
| weapon datums | `U+0x218` | `u32[4]` | confirmed | as above; multiplayer fills two |
| grenade counts | `U+0x23e` | `i8[2]` (frag, plasma) | confirmed offsets, names unverified | live-gamestate.md ("Unit stance"): `0xcdff0` and the HUD index it with `U+0x23c` |
| magazine count, pointer | `D+0x2c0`, `D+0x2c4` | `i32`, `u32` | confirmed | `0x101a10` (sets a weapon's ammunition from a fraction): `mov eax,[edi+2C0h]; test eax,eax; jle`, then `mov ecx,[edi+2C4h]`; `0x101740` (ammunition between two weapons of one kind) loops over the block with stride `0x5c` (`add esi,5Ch`) |
| rounds a magazine holds | magazine definition `+0xa` | `i16` | confirmed | `0x101a10`: `movsx edx,word ptr [ecx+0Ah]`, times the fraction, rounded (`fistp`), is stored as the rounds loaded (`mov [esi+22Ch],dx`) |
| most rounds in reserve | magazine definition `+0xc` | `i16` | confirmed | `0x101a10` caps the reserve with it (`movsx ecx,word ptr [ecx+0Ch]; cmp ax,cx; cmovg`); `0x101740` tops a reserve up to it |
| reserve rounds | `W+0x22a` (magazine state `+2`) | `i16` | confirmed | `0x101a10`: reserve = reserve + loaded - new loaded (`mov ax,[esi+22Ah]; sub ax,dx; add ax,[esi+22Ch]`), clamped to 0..`+0xc`; `0x101740` moves rounds into it from the other weapon's reserve and loaded |
| rounds loaded | `W+0x22c` (magazine state `+4`) | `i16` | confirmed | as above; magazine states are `0x10` apart (`0x101740`: `add eax,10h; add ebx,10h`) |
| barrel count, pointer | `D+0x2d0`, `D+0x2d4` | `i32`, `u32` | confirmed | `0x101d20` loops `D+0x2d0` barrels from `D+0x2d4` with stride `0xec` (`imul ecx,0ECh`) |
| battery used per shot | barrel definition `+0xa8` | `f32` | confirmed | `0x101d20` returns 1 (the weapon "ages") when `D+0x2c0` is 0 or any barrel's `+0xa8` is over 0; the fire path at `0x1036d1` loads `[esi+0A8h]` and calls `0x101db0`, which adds it to `W+0x184` (`addss xmm0,[esi+184h]`, `0x101e39`) and clamps at 1.0 (`[0x45dbc0]`) |
| battery used | `W+0x184` | `f32`, 0 fresh to 1 dead | confirmed | as above; `0x101a10` writes `1 - fraction` there for an aging weapon; `0x101740` swaps it with a same-kind weapon's when theirs is fresher (`comiss`, then the swap). Skulls' Unlimited Ammo already keeps it at 0 (docs/game/skulls.md) |
| a weapon's holder | `W+0x14` | `u32` datum | confirmed | live-gamestate.md: the parent; none for a weapon on the ground |

**How the bot reads them.** `arms::read` takes the unit's four weapon
datums, each resolved like any object (salt and type 2, weapon) and its
definition through the tag entry (datum compared). A weapon whose
definition ages (`0x101d20`'s test) is a **battery**: `1 - W+0x184`. One
with magazines and no aging barrel is **rounds**: the first magazine,
loaded `W+0x22c` of `+0xa`, reserve `W+0x22a` of `+0xc`. The retail
energy sword has no magazines or barrels and uses battery age. On
exhaustion `0x101e69..0x101e74` replaces its definition with the tag at
`D+0x304`, the depleted blade on Zanzibar. The multiplayer weapon type
excludes the flag, bomb, ball and Juggernaut item from supplies. The two
hands use slots `U+0x212` and `U+0x213`. The first other occupied slot
is the stowed weapon.

`arms::pickups` walks the objects array (at most 2048) for live weapon
objects with no parent: placed weapons from the netgame spawns and dropped
ones alike. A pickup is **useful** when it is a gun with something in it.
A fault while reading weapons leaves the bot playing without the weapons
layer (`Me::arms` is `None`); it never blinds it.

**Handling.** What kind of gun a weapon is comes from fields of its
`weap` tag and its first barrel's projectile
(`arms::WeaponDef::profile`, `arms::Profile`):

| Field | Where | Confidence | From |
|---|---|---|---|
| zoom levels | `D+0x1fe` (`i16`) | checked on Zanzibar's tags | `boot::touch_glue` (sniper and beam rifle 2; battle rifle, carbine, rocket launcher 1) |
| aim assist reach | `D+0x20c` `cone_range`, `D+0x214` `pull_range` (`f32`, world units) | confirmed (engine) | [aim-assist.md](aim-assist.md) (`0x1a50a0`) |
| triggers block | `D+0x2c8` count, `+0x2cc` pointer, `0x40` each; `+6` the behaviour (`u16`: 0 while held, 1/4/5 once per pull, 2/3 charge) | checked on Zanzibar's tags | `boot::touch_glue`; [halo1/objects.md](../halo1/objects.md). None on the energy sword |
| rounds a second | barrel `+8` (`f32`, the most) | checked on Zanzibar's tags | `boot::touch_glue` |
| projectiles per shot | barrel `+0x6a` (`i16`) | field order from data, value pattern on retail | [halo1/objects.md](../halo1/objects.md) |
| projectile | barrel `+0x8c` (tag reference, datum at `+0x90`) | field order from data, value pattern on retail | [halo1/objects.md](../halo1/objects.md); the bot accepts it only when the entry's group is `proj` |
| projectile speed | `proj+0x17c` (`f32`, world units a second) | confirmed (engine) | `live-telemetry` (`0xf8487`) |
| projectile reach | `proj+0x16c` air damage range end, `proj+0xe0` maximum travel (`f32`, world units) | engine and disc | `0xfc016` copies the air endpoint into the live projectile. Blood Gulch's converted shotgun carries Halo 1's endpoint, 3, below its 40-unit travel limit. Coagulation's shotgun has 8 in both fields. |

The first trigger and the first barrel speak for the weapon. A fault
reading any of them leaves the gun weapon-blind (as before), not unread.
The **role** (`arms::Role`) follows: two zoom levels a **sniper**; once
per pull with a projectile under 100 a second a **blast** weapon (rocket
launcher, fuel rod, brute shot); 4 or more projectiles per shot a
**scatter** gun (shotgun), including converted tags with a held trigger;
once per pull otherwise
**precise** (magnum, battle rifle, carbine); held or charged **spray**
(SMG, plasma rifle, needler, plasma pistol); no trigger but aim assist a
**blade** (energy sword). The Zanzibar flag, ball and bomb each have a trigger. Each role has a band it is best at and a range
it shoots to (world units):

| Role | Band (`arms::preferred_range`) | Shoots from, to | Rating |
|---|---|---|---|
| unknown (not read) | 2.5 to 15 | 0 to 18.75 | 1 |
| blade | 0 to 1.5 | (melee) | 4 |
| scatter | 0 to 4 | 0 to 5 | 4 |
| spray | 1.5 to 9 | 0 to 11.25 | 2 |
| precise | 5 to 28 | 0 to 35 | 3 |
| sniper | 12 to 60 | 0 to 75 | 5 |
| blast | 5 to 25 | 2.5 to 31.25 | 5 |

One rule sets where every gun stops shooting: 1.25 times its own band's
far end (`weapons::FIRE_PAST_BAND`), whatever its class. A person sprays
a little past where a gun is good and never across the map, so an SMG
with an 8-unit band fires to 10, a battle rifle with a 20-unit band to
25. The band comes from the gun's own tag data (`weapons::reach`), and
the role table above is the fallback when the tag has none. Past the
band the trigger works in bursts from Normal up (§3.4). A plasma rifle or plasma pistol (by identity), or any
automatic whose projectile is slower than 150 world units a second,
keeps its target within 6 and so fires to 7.5 at most
(`weapons::SLOW_BOLT_BAND`, `SLOW_BOLT_FIRE`): a strafing target steps out
of slow bolts ([round 1 on Midship](notes/2026-11-23-bots-natural-round1.md):
the plasma rifle hit 0.42 of its shots under 3 units, 0.17 from 3 to 12
and almost none beyond). Beyond that the movement closes distance or the player draws
another weapon. For a visible approaching enemy, the draw considers half
a second of observed motion, capped at two units of advance.

**The decision** (`arms::plan`, pure, in order):

1. a blade in hand stays out in lunge range. With the enemy out of reach
   and a better-suited ready gun in the other slot, **switch**;
2. the held gun is dry (no rounds anywhere, or a battery under 2%):
   - the other has something: **switch** (Y);
   - an enemy in view within 2.5 world units: **melee** (run in, B);
   - a useful pickup within 30 world units, not blacklisted: **seek**
     the best near one (each rating point counts as 3 units nearer);
   - an enemy in view: **melee**; else nothing;
3. an empty magazine with a reserve: **reload** (X), in a fight or not;
   on Normal and up, with the other gun loaded and the enemy in its band,
   **switch** instead (faster than a reload);
4. draw a ready sword in lunge range or a shotgun in its close band,
   including where the current gun is also usable. A sniper takes over in
   its band beyond 80% of a rifle's preferred reach. **Switch** when
   the other gun alone can reach or it improves relative range fit by more than 0.3
   (0.6 on Easy). With similarly suitable guns the bot keeps its choice;
5. **reload**: calm (no enemy in view for 1.5 s) and under 35% of a
   magazine with a reserve; or calm, no enemy in view at all and under
   90% (`arms::TACTICAL_RELOAD`: the reload after a fight); or a lull
   (no enemy in view for 0.5 s) and under 60%; or the enemy beyond the
   gun's range and under 35%;
6. no enemy in view, calm or a lull, and no power weapon (rating 4 and
   up) carried: a power weapon within 25 it does not carry: **seek** it;
   holding the better of two guns, **switch** first, so the one dropped
   for it is the worse;
7. calm, the held gun under 20% of all it can carry and the other one too
   (or none): **seek** a useful pickup within 12;
8. a calm bot with a free slot seeks a higher-rated visible gun within
   15 units. A calm bot low on a specific grenade type (zero or one)
   walks over a visible pickup of that type within 12 units.

Seeking walks there along the nav mesh. A weapon of a kind it already
carries is walked over: the game moves its ammunition across by itself
(`0x101740`). One of another kind is taken by holding X on it (within 0.5
world units) for at most 1 s; the dry one in hand is dropped for it.

**Trips that fail** (`crates/live/bots/src/mind/errand.rs`). A walk to a
weapon may take 3 s plus 20 ticks per world unit of the straight line at
its start. A bot within 1 unit of it for 1.5 s while the weapons decision
still wants it has not got it: a walk-over that gives nothing, a weapon
on a ledge or in a wall. A held X that takes nothing in 1 s ends the same
way. The weapon (its object) is then refused for 30 s and its spot
blacklisted for 10 s, so the bot goes back to fighting and roaming
instead of walking the same trip again.

The presses are Halo 2's Default layout (X reload and action, Y switch,
B melee, A jump, left stick click crouch), which every bot's profile is
given whatever the human uses (`guest::bot_block`, §4.2).

**Dual wielding and power items.** The following fields were checked against
retail code and Zanzibar tag metadata. The weapons list belongs to the
loaded map, so object handles and tag datums can change between games.

| Field | Where | Trace |
|---|---|---|
| Off-hand eligibility | `D+0x12c`, u32 bits 22 or 23 | `0xcd8a8..0xcd8d7` tests the pickup. `0xcdaee..0xcdb0d` disables both off-hand choices if the primary fails the same test. Actions 5/6 recheck both objects through `0x101640` at `0xcd1a6` and `0xcd1b5`. Bit 23 also suppresses the primary pickup choices. The preferred retail pairs all have bit 22. |
| Weapon identity | `matg=[0x4e034c]`, mulg datum at `matg+0x16c`, universal block at mulg `+8/+0xc`, weapon list at universal `+0x68/+0x6c` | `0x15eb80` searches the eight-byte entries for the weapon datum at `+4`. Zanzibar entries 1, 6, 7, 8 and 9 resolve to SMG, plasma pistol, plasma rifle, brute plasma rifle and needler. Unrecognized definitions skip the pairing policy. |
| Multiplayer weapon type | `D+0x290`, i16 | Getter `0x101280`; the Assault carrier check at `0x24044d` compares with 1. Zanzibar has 0 for an ordinary gun or sword, 1 for a flag or bomb, 2 for a ball and 4 for the Juggernaut item. |
| Placement and nominal interval | object `+0xaf`, signed index into scenario `+0x120/+0x124`, stride `0x90`; spawn XYZ at placement `+0x40` | `0x15ae00` indexes the placement from the object. `0x15ac50` reads signed seconds at placement `+0xe`, then collection `+8` through the `itmc/vehc` reference at placement `+0x58/+0x5c`; zero falls through to 30 seconds. |
| Variant interval | placement class byte `+0x14`; game globals `G+0x211` for class 0, `G+0x208` for classes 1–7 | `0x15b0ef..0x15b134` applies option 1 disabled, 2 half, 3 double; other values keep the interval. |
| Live placement record | `E+0x7dc+8i`, 100 records; `+0` object datum, `+4` i16 seconds remaining, `+6` vehicle abandonment count | `0x15ace0` initializes the table, `0x15b24f` clears the deleted object. `0x15b03f..0x15b1aa` starts, decrements and spawns on zero. A sampled zero means the next service checks the start condition (placement i16 `+0x12`, mode 0 on use/movement, mode 1 on depletion, or a deleted object in either mode). Negative disables it. `0x15b005` increments the separate vehicle count. |
| Clock phase | `[0x510c54]+2` tick rate, `+8` game tick | `0x15aef2..0x15af07` services a placement when its index and the tick have the same remainder modulo tick rate. A deadline is the next service tick plus (remaining seconds minus one) more periods. **[guess]** The bot samples after the service on a coincident tick. Before that service the projected deadline can be one service period late. The engine can also postpone a spawn. |
| Overshield and camouflage | `eqip+0x12c` i16 values 2 and 3; `+0x130` effect seconds | `0x151ef5..0x151f99` converts seconds to ticks and dispatches. Overshield calls `0xd6af0` (unit `+0x10a` bit 4 while charging, shield `+0xf0` above 1 when charged). Camouflage calls `0x152000` with effect index zero, storing ticks at player `+0x198`. |

On Default controls a held Y asks for the secondary interaction at
`0x14f8ff..0x14f911`. `0x151940` selects weapon actions 5/6 for that
hand and 3/4 for the primary interaction (hold X). A short Y release
goes through `0x14f96f` and `0x151aa0` to drop the weapon selected by
`U+0x213`. The fallback action 19 goes through `0xe8de0`, clearing
the hand at `0xe84f1` and removing the inventory entry through `0xce920`.
A consumed hold suppresses that release. Clankers can pair two SMGs,
a plasma rifle or plasma pistol with an SMG, or two needlers on any
difficulty. Each Clanker's name sets its chance of wanting a second gun.
It makes one choice per primary weapon type and compatible ground weapon
each life. Staying near a declined gun or seeing it again keeps that
choice. A respawned pickup gets a fresh roll. An active plasma-pistol
combo or grenade throw keeps priority. In combat they take a pair only
within pickup distance and inside both guns' effective bands. Either hand can charge the pistol.
Only against a group (another enemy within 2.5 of the target), or with
the left gun dry and nothing to load, does the bot trade the left gun for
a grenade: the thrower first solves a safe grenade arc, taps Y and waits
for the left slot to clear before using the grenade trigger. A lone enemy
walking in, out of the left gun's band or lost for a moment keeps both
guns on it.

The placement table has no last-taken timestamp. The host keeps the last
successful take per placement, with these two read-only gameplay observers:

| Event | Source | Checks |
|---|---|---|
| Weapon taken | `0x10ca80`, EAX item, first stack argument new unit. The attach branch sets item `+0x12c` bits 0 and 2, stores the unit at `+0x154` and registers carried objectives at `0x10cb8f`. | Ignore detach (unit -1). Match the salted object to the live placement record and its position to the pad. Resolve the biped's `+0x13c` player and verify the player's `+0x2c` unit. Record game tick `[0x510c54]+8`. |
| Equipment taken | `0x151ea0` dispatches the effect. On success it calls `0xa89c0` with ECX unit and EDX equipment definition, then deletes the item at `0x151fe7`. Rejected pickups skip this call. | At the notification entry verify return PC `0x151fd2`. The pickup function's second stack argument is now at `ESP+0x20`. Resolve the equipment datum and check its definition, placement and player as above. |

Only a bot watching that pad can use the event to date its removal.
The salted object must match and the event tick must fall between its
last sighting on the pad and the first empty scan. If the observer is
unavailable, **[guess]** the first empty scan dates the pickup (up to 30
ticks late). After a gap in sight the bot leaves the time unknown.
For a witnessed disturbance or consumed power-up, the bot estimates the
next eligible service from the event tick, placement index, tick rate
and learned or public interval. The host's live countdown remains available
as an offline trace. The bot cannot tell if the engine postpones the spawn.
The travel policy budgets 12 ticks per world unit
plus a 20-tick lead at Legendary, scaled down at lower skills. Bots fight from a due pad,
and yield to objective or retreat orders. Rockets, both sniper-class
rifles and the live sword qualify by their handling. A visible hand
holding the same salted weapon object identifies the wielder. For a
consumed power-up the success event identifies the player and unit. The
bot remembers a visible recipient. Without an event, a sole visible
player within one world unit with the matching effect is **[guess]**
ownership. Ambiguous fallback observations remain unassigned. A later sighting refreshes the memory; death, a new unit,
a visibly expired effect or ten seconds without confirmation clears it.

## 3.7 Objectives (traced)

Stage 5. The bot reads the gametype, the objective objects and the
scenario's netgame flags (`crates/live/bots/src/objective.rs`), and a pure
decision (`objective::plan`) gives it a goal.

**Fields.** Traced offline from the XBE with `h2 disasm` and a reading of
its data section (static, no game run). `E` is the game-engine globals,
`G` the game globals, `V` the variant (`G+0x13c`), as in
docs/live/live-gamestate.md. Live telemetry reads none of these yet: it
names the gametype by `V+0x44` and the weapons by tag name.

| Field | Where | Type | Confidence | Evidence |
|---|---|---|---|---|
| game-engine globals | `[0x4e9ae8]` | pointer | confirmed | live-gamestate.md; `0x1574c0` zeroes `0x327` dwords of it |
| engine index | `E+0xc14` | `i32` | confirmed | `0x1574c0` copies `[G+0x180]` (`V+0x44`) there, and writes 0 when the engine has no object or the globals tag lacks its multiplayer block (`0x157578`) |
| engines | `0x55e4d0[index]` | pointer to a static object whose only dword is its vtable | confirmed | filled by `0x378500`: 1 CTF and 9 Assault share one (`[0x470a5c]` = `0x47f788`, vtable `0x459d18`); 2 Slayer `0x47fc80`, `0x45c6d8`; 3 Oddball `0x47fc84`, `0x45c7a8`; 4 KotH `0x47fc88`, `0x45c878`; 7 Juggernaut `0x47d8f0`, `0x453d38`; 8 Territories `0x47fc8c`, `0x45c948`. 51 thiscall slots each; the slot map is in [notes/2026-11-21-engine-vtables.md](../engine/notes/2026-11-21-engine-vtables.md) |
| objective object count | `E+0xc18` | `i32`, at most 8 | confirmed | `0x15e050` (register) bumps it after `cmp eax,8; jae`; `0x15e130` (unregister) moves the last record down and drops it |
| objective records | `E+0xc1c`, `0x10` each | | confirmed | `0x15e050`: `shl eax,4; lea edi,[eax+ebx+0C1Ch]` |
| record: the object | `+0` | `u32` datum | confirmed | `0x15e050` stores its first argument; every lookup (`0x15e180`, `0x15e2b4`, `0x15e327`) compares it |
| record: its team | `+4` | `i16` | confirmed | `0x15e050`'s second argument; the CTF flag spawn `0x2422d0` passes the flag's team (8 is the neutral one), Oddball's `0x2bc1f0` the ball's number; `0x15e410` finds a record by it; also written to the weapon's `+0x17e` |
| record: carrying unit | `+8` | `u32` datum, -1 none | confirmed | `0x15e250` (picked up) writes its second argument; `0x15e460` (dropped) writes -1 |
| record: carrying player | `+0xc` | `u32` datum, -1 none | confirmed | `0x15e250` copies the unit's `+0x13c` (its player, live-gamestate.md); `0x15e300` refreshes it; `0x15e460` writes -1 |
| carried flag on the weapon | weapon `+0x16c` bit 7 (bit 6: registered) | `u8` | confirmed | `0x15e2d4` `or 80h`, `0x15e49c` `and 7Fh`; `0x15e0a5` `or 40h`, `0x15e1b6` `and 0BFh` |
| who registers | | | confirmed | callers of `0x15e050`: CTF and Assault (`0x2423d9`), Oddball (`0x2bc26c`), `0x2c0a7d` (Territories registers the marker object with its territory index), and the network and object-create paths `0xa228f`, `0xd0ab5` |
| team designators in use | `E+8` | `u16` mask | confirmed | `0x158990` tests `1 << i` against it for `i` 0 to 8 |
| each designator's team | `E+0x10 + 2i` | `i16`, 9 | confirmed | `0x157790` builds the active-team mask at `E+6` and calls `0x157670` at `0x157a30`. The latter clears the mapping to -1, pairs a designator from `E+4` with an active team, updates masks `E+8/E+0xa` and writes the entry at `0x15770b..0x15771a`. `0x158990` performs the inverse lookup |
| netgame flags | `[[0x4e0350]+0x11c]`, count `[..+0x118]`, `0x20` each | | confirmed | `0x243b69`: `mov ecx,[4E0350h]; mov edx,[ecx+11Ch]; shl ...,5` |
| flag type, team, identifier | `+0x10`, `+0x12`, `+0x14` | `u16` each | confirmed | `0x19ec40`, the engine's flag query, compares the three with its arguments (-1 matches any) |
| flag game flags | `+0x16` | `u16` | likely | `0x243b8f`: bits 1, 2 and 4 against `V+0x104` (`G+0x240`), the CTF variant's flag setting |
| type 0, 1: CTF flag spawn, flag return | | | confirmed | `0x243b10(spawn?)`: type `al ? 0 : 1` for CTF |
| type 2, 3: bomb spawn, bomb return (the plate) | | | confirmed | as above, `al ? 2 : 3` when the engine is 9 |
| type 4: ball spawn | | | confirmed | Oddball init `0x2bc2e0..0x2bc32a` queries type 4, designator 8, any identifier and at most three indices into its globals at `+0xc` |
| type 11 to 18: hill `0` to `7`'s corners | | | confirmed | KotH's init `0x2bd140` collects the types `0xb..0x12` it finds, as `type - 11`, into `0x5092f0` (count `0x5092e8`); `0x2bdc70` asks `0x19ec40` for type `0xb + hill` |
| CTF and Assault globals | `[0x51ec80]` (= `E+0xfc`) | pointer | confirmed | `0x240160` zeroes `0x82` dwords from `E+0xfc` and stores the address there |
| ...: per-designator flag spawn, return | `+0x02 + 2i`, `+0x14 + 2i` | `i16` netgame flag index | confirmed | `0x240160`'s loop `0x2401a0`; `0x2422d0` takes `+0x02` at `0x158990(team)` |
| ...: neutral flag or bomb spawn | `+0x38` | `i32` index | confirmed | `0x2401e6`; `0x2422d0` for team 8 |
| ...: per-team state bits | `+0x170 + team` | `u8`, 9 | confirmed | Mask 8 marks armed at `0x2405a0`. Mask 2 marks planted at `0x242d25` after `0x15e360` releases the bomb at the scoring plate. |
| ...: per-team timer | `+0x1a0 + 2·team` | `i16`, 9 | confirmed | the Assault arming count (`0x240575..0x24058d`); -1 at a flag spawn (`0x2422e6`) |
| One-bomb defending team | `A+0x1f4`, A is the CTF/Assault globals | i32 game team | confirmed | `0x2402a4` selects it when `G+0x240 == 1`. `0x243a80` spawns the bomb for the other side. The scoring check at `0x242fc6..0x242fd6` requires the plate team to equal this field and differ from the carrier team. |
| Bomb team and active plates | record `+4` / weapon `+0x17e`; `A+0x14+2d`, `A+0x26+2d` | i16 team or neutral 8; two i16 flag indices per designator | confirmed | `0x2422d0..0x2423d9` uses the spawn team as the registration argument. Both the arming-volume check `0x244680` and scoring check `0x242ef0` consult the two selected plates. `0x242f65..0x242f7f` requires the game team in the `E+0xc` mask. A score needs the carrier within one world unit of a qualifying plate. |
| Oddball globals | `[0x51ecc4]` | pointer | confirmed | the only global of Oddball's functions besides the shared ones |
| ...: each ball's spawn | `+0xc + 4·ball` | `i32` netgame flag index | confirmed | `0x2bc1bb` |
| King of the Hill globals | `[0x51ecc8]` (= `E+0xfc`) | pointer | confirmed | `0x2bd157` |
| ...: the hill's corners | `+0x00`, `4` each, count `+0x60` | `i32` netgame flag index | confirmed | `0x2bdc96` asks for at most 16 into it; the count is made even and at least 4 is needed (`0x2bdc9b..0x2bdca7`) |
| ...: the hill in play | `+0x1a0` | `i32`, 0 to 7 | confirmed | `0x2bd20b` sets 0 at start; `0x2bd77f` moves it on (`0x2bd0b0` picks the next), when `+0x1a4` runs out |
| ...: ticks until the hill moves | `+0x1a4` | `i32` | confirmed | `0x2bd747..0x2bd772`: counted down, reset to `V+0xf4` (`G+0x230`) seconds × the tick rate; never when that is 0 |
| ...: players in the hill | `+0x1a8` | `u16` mask by player index | confirmed | `0x2be880` checks the live unit against the current hill bounds and geometry. `0x2bd304` increments player `+0x1b8` inside and clears it outside. `0x2bd6d0..0x2bd71d` builds the scoring mask from nonzero counters before the hill-move update |
| Territories globals and markers | `T=[0x51eccc]=E+0xfc`, `T+0x114` i16 count, `T+0x60+2i` i16 flag index | up to eight | confirmed | `0x2bf310` initializes T. `0x2bf5b0` queries netgame type 10, any designator, identifier i; `0x2bf61d` stores the first matching flag. Additional markers size the capture volume. |
| Territory owner | `T+0x70+4i` | salted player datum | confirmed | `0x2c075d` writes it; `0x2bfa3f` checks its salt. `0x2c0006..0x2c0047` compares team through player `+0xc0`. A stale owner gives no team. |
| Territory occupants | `T+0x95+8p` | signed byte by player slot | confirmed | `0x2bfc40` tests the unit against the marker and the radius and height values at `T+0/+0x20/+0x40+4i`. `0x2bfd83` stores the territory index on entry, `0x2bfe1d` stores -1 on exit. Multiple players can occupy one site. |
| Juggernaut identity | `J=[0x510c9c]=E+0xfc`, `J+0` | u16 player-slot mask | confirmed | Engine 7 initializes J at `0x1921d0`. `0x1922db..0x192339` tests the victim and transfers the role to the killer through `0x192970`. |
| the flag, ball, bomb tags | globals tag (`[0x4e034c]`) `+0x16c` the multiplayer block: `+0xc` → `+4` flag, `+0xc` ball, `+0x3c` bomb | `u32` tag datum | confirmed | `0x242261` (bomb, engine 9) and `0x24228b` (flag) create the object from it; Oddball `0x2bc231` the ball |

**How the bot reads them** (`objective::read`, from `guest::perceive` once
it has found itself):

- the gametype is `E+0xc14`; Slayer and unsupported engines read as no objective;
- each record gives an item: the object, its team, and the carrier's
  player index (as `Seen::id`). A carried item is where its carrier's unit
  is; otherwise where the object is. A record whose object is gone is
  skipped;
- the netgame flags the gametype uses become spots (CTF: types 0 and 1;
  Assault: 2 and 3; Oddball: 4), each with the game team its designator
  stands for (`E+8`, `E+0x10`). An unbound designator has no team;
- in King of the Hill the active corner list supplies a destination.
  The live occupancy mask determines who is scoring. Geometry remains an
  approximation for loose pickups and observations without a scoring mask.

A fault leaves `Perception::objective` `None`: the bot plays as in Slayer.
Script games ride a retail engine index (rules-engine.md, "Which engine
index a script rides") and keep its objective registry, so the same reads
serve a `.rules` game on the CTF or Assault engine.

**Without the engine's globals.** A rules script that keeps no per-engine
globals (docs/game/rules-engine.md, "Porting the seven engines") still
leaves the bots an objective, read from the scenario's netgame flags
(`objective/zones.rs`):

- Oddball with `[0x51ecc4]` zero: the neutral type-4 spawns (designator
  8, at most three). The balls themselves come from the objective
  records, which a script's `spawn_objective` fills;
- King of the Hill with `[0x51ecc8]` zero: the lowest-numbered hill's
  corners (types 11 to 18). Who is in it is judged by the corners;
- Territories with `[0x51eccc]` zero: `State::Markers`, for each
  identifier 0 to 7 the first type-10 flag with it (`+0x14`), as
  `0x2bf5b0` picks them. No owner is known; a player within
  `TERRITORY_REACH` (3 units XY, 2 in height) of a marker stands in it.

**What a bot knows of it.** Perception (`perceive::observe_for_brain`)
keeps a flag or bomb only while the bot sees it, carries it, or a
teammate carries it. The Oddball ball is kept wherever it is: its
waypoint is on every Oddball HUD, so a hidden carrier's place is known,
though the carrier as a player stays unseen (not traced; the HUD's ball
waypoint is taken from play). Hill and territory occupancy is masked to
the players the bot knows of.

**Roles** (`objective::role_of`). Every bot sorts its living teammates
and itself by player index; its rank and the team's size give its role,
so all of a team's bots agree with nothing said between them. One alone
attacks; of two, the higher index defends; from three, a third (rounded
down) defend, the highest ranks. Humans count in the ranks: the bots do
not know who is a bot.

**Belief** (`objective::belief`, CTF and Assault). Perception keeps a
loose flag or bomb, and an enemy carrying one, only while it is in view;
the bases (the netgame flags) are public. The planner works from what the
bot believes (`Belief::know`, `mind::strategy`):

- a flag or bomb in view is where it is; one seen loose is remembered
  there for 45 s, and forgotten when the bot is within 15 world units with
  the spot in view and it is not there;
- an enemy carrying our flag is followed to where it was last seen for
  4 s; then our flag is *missing*;
- a flag or bomb never seen (or forgotten) is at its spawn (CTF types 0;
  Assault type 2, a spawn with no team being the neutral bomb's). A spawn
  in view within 15 with nothing on it makes that team's flag or bomb
  missing;
- a missing one stays missing until it is seen or 20 s pass (a return or a
  capture puts it back at its spawn);
- a destination the stuck machine repaths at 3 times (section 3.2), or a
  loose item the bot stands on for 3 s without taking it, is given up for
  15 s (`Belief::pursue`): the item drops out of the plan, and a return
  point or plate drops out while another of its team remains.

So from the spawn the attackers head for the enemy base whether or not
the enemy flag is in view, and a bot looking at an empty spawn plays on
rather than waiting.

**Roles** (`objective::role_of`, `objective::Lean`). Every bot sorts its
living teammates and itself by player index; its rank and the team's
size give its slot, so all of a team's bots agree with nothing said
between them. One alone attacks; of two, the higher index defends; from
three, a third (rounded down) defend, the highest ranks. Humans count in
the ranks: the bots do not know who is a bot.

Within the slot the bot plays its name. `Lean::of` takes four of the
name's personality draws (aggression, caution, teamwork, objective
focus; section 3.3), each continuous within the difficulty's band:

| Leaning | From | What it changes |
|---|---|---|
| escorts | teamwork over the mean of aggression and focus | an attacker with another attacker nearer the loose flag or bomb covers them (**escort**) instead of racing them; the nearest attacker always goes |
| guard radius | 4 × (1 + aggression − caution), 3 to 8 | how far a defender strays from our flag or plate to fight |
| return range | 25 × (0.5 + teamwork) | how far an attacker turns back to return our dropped flag |
| carrier pull | 1.5 − focus | an attacker hunts our flag's carrier when it is nearer than this share of the way to its own target |

A neutral leaning (all 0.5) gives the ranked plan unchanged. Of the eight
round-1 names at Legendary, three escort and five grab.

**The decision** (`objective::plan_with`, pure; `plan` with a neutral
leaning and nothing missing):

CTF uses this order.

1. it carries the enemy flag: **carry** it to our return point, else our
   flag's spawn, along the nav mesh (straight at it while no route is
   ready);
2. an enemy carries our flag: **hunt** the carrier (defenders always;
   attackers by the carrier pull);
3. our flag lies more than 2 world units from its spawn: **go** and touch
   it to return it (defenders always; attackers within the return range).
   Our flag missing, a defender **goes** to the enemy return point (else
   their spawn), where the carrier is heading;
4. a teammate has the enemy flag: the remaining players divide escort and
   guard duties. A lone teammate escorts within 3 world units;
5. attackers: **go** for the enemy flag where it is believed to be (at its
   spawn or dropped), or escort a nearer attacker by their leaning. With
   the enemy flag missing they play as in Slayer until it turns up;
6. defenders: **hold** our flag's spawn within the guard radius, fighting
   from there.

Every bot's goal also takes its name's temper (`objective::Temper`:
patience and caution from the personality, `plan_with`).

**Oddball** (`zones::ball`):

1. carrying, **keep** it: cover away from the threat, melee at arm's
   length;
2. a loose ball: the two of the team nearest it (`RUSHERS`) **go** for
   it, the rest **hold** within `BALL_COVER` (6 units) of it. In a fight
   the runners keep walking onto it, shooting on the way, and hold X on
   it; in free-for-all everyone runs;
3. an enemy has it: **hunt** the carrier as a group. A hunter between 6
   and 18 units from it (`SHOWDOWN`, `STAGE`), with no teammate within 8
   (`GROUP`) and one coming more than a lead behind it (6 to 20 units, the
   patient the shorter), holds where it is (3 units) until they close
   up; a teammate more than twice `STAGE` further off (respawning) is not
   waited for. The carrier is the hunters' first target, and since it
   cannot shoot it does not count as a gun when a hunter weighs whether
   it is outnumbered;
4. a teammate has it: **escort** as a screen, `SCREEN` (3.5 units) out
   from the carrier towards the nearest enemy it knows within 25, the
   escorts fanned out by index (0, ±0.6, ±1.2 rad) so each stands in its
   own spot; with no threat, by the carrier. An escort's first target is
   an enemy within 12 of the carrier;
5. between balls, the closest neutral type-4 spawn.

**King of the Hill** (`zones::hill`). Approach the current corners'
centre. In the hill (the engine's mask, else the corners), **hold** the
centre within a share of its radius, 0.4 to 0.65 by caution
(`HILL_KEEP`, `HILL_KEEP_CAUTION`), so the cautious play its edge; where
the mask counts the bot in but the corners do not, hold the spot. Out of
it with two teammates already holding it and nobody contesting, a bot of
caution 0.6 or more (`EDGE_CAUTION`) covers it from just outside, within
`1 + caution` radii of the centre. Everyone else goes in, and an enemy in
the hill sends the cautious in too, to clear it; an enemy standing in it
is the preferred target. Holding the hill, a focused bot stays in a fight
one enemy longer before backing off. Moving the hill rebuilds its
corners and the bot follows the next scoring mask.

**Assault.** Take a friendly or neutral bomb to an active enemy plate.
In one-bomb, only the designated defender's plates score. Carrying the
bomb on foot inside the arming volume advances the counter toward
`G+0x244` seconds times tick rate. Outside, it falls by three per tick
to a floor of -1. Stay on the plate through arming; when armed, close to
score. The engine releases the planted bomb, records the planter at
`A+0x17c+4·bombTeam` and sets the planted bit; the team's point comes when
it goes off, about 3 s later, killing whoever stands near (round 1 on
Midship: the planter and an escort 3 units away). Escorts cover the
arming from the plate. A planted bomb is nobody's target: any goal whose
point lies within `BLAST_CLEAR` (6 world units, XY) of it, or a bot with
no goal standing there, becomes a **hold** within 1.5 of a spot 7.5 out
along the line from the bomb to the bot (`objective::clear_of`).
Defenders guard the plate within the guard radius or intercept the enemy
carrier. An ordinary dropped friendly bomb can be recovered by the
nearest teammate; a loose one elsewhere goes to the nearest attacker, the
escorting names covering it.

**Territories** (`zones::territory`). A team spreads over the sites.
Every bot ranks each pair of teammate and site the same way: a friendly
site with an enemy in it, or one the teammate stands in and is taking,
first; then a site not ours; then ours; nearer first. Going down that
list, each site takes one teammate, two where an enemy stands in it, so
two relieve a site under pressure while the rest take the others; a bot
the sites have no room for goes where it would alone. In its site it
**holds** within `TERRITORY_KEEP` (1.5 units) of the marker, fighting from
there; a site taken drops to the bottom of the list, so its taker moves
on to the next unless nothing else is left. Holding a territory, a
focused bot stays in a fight one enemy longer. FFA ownership follows the
player slot and every bot plays alone.

**Juggernaut.** Every other player sees the Juggernaut by its waypoint,
so a hunter's goal is the Juggernaut wherever it is: it walks to it with
no one in view and fights it on sight. Hunters ignore fellow hunters. The
Juggernaut has no marks on the others: it hunts the nearest hunter in its
own view, and roams otherwise (`juggernaut_quarry`,
`crates/live/bots/src/mind/strategy.rs`). It never withdraws, runs at a
visible hunter while firing, then melees up close. Movement bonuses come
from the game variant.

**Picking up.** Approach the chosen item and hold X while it remains
loose within pickup range: 0.5 world units in XY and 1 in height
(`TAKE_HEIGHT`), as an item on a step or lip sits off the unit's origin.
A goal counts as reached in XY within its radius (at least
`nav::REACHED`) and in height within its radius (at least 1), so a bot
does not walk on at an item it is standing on. `0x1517ec..0x1518d6` gives a nonzero
multiplayer weapon type the contextual interaction kind 7. The held-X
path accepts kinds 1 and 7 at `0x15195c` and requests primary pickup.
There is also a proximity path (`0x15098f`) whose primary pickup is
gated by inventory at `0x150dd0`. In Assault the pickup predicate
`0x2415d3..0x241611` rejects a planted bomb or a non-neutral bomb
belonging to another team. A bot returning its own CTF flag just
touches it. Once possession changes the carrier releases X.

### A script's goals

A rules script's objectives are not in the engine's globals, so a
script names them: `bot_goal_attack`, `bot_goal_defend` and
`bot_goal_hold` put a zone in a team's goals and `bot_goal_clear`
clears them (rules-world-api.md, "Bots"). The host keeps the table
(`crates/live/bots/src/objective/script.rs`), each place with its
zone's area; every phantom's perception carries a copy
(`Perception::goals`, filled in `guest.rs` beside the objective read).
While the bot's team has a place, `objective::script::plan` gives its
goal in place of the retail objective's (`mind/strategy.rs`, after an
outside order), and the mind plays it as any objective goal:

- `hold`: the whole team stands in the first such place.
- `attack` and `defend` both: the team splits by role (`role_of`, as in
  CTF); attackers take, defenders guard. One kind only: the whole team
  plays it.
- Taking: every attacker goes to the first `attack` place the script set
  (`Go` its middle) and, once in its area, keeps about the middle as on a
  hill (`Hold`, `HILL_KEEP` plus the name's caution).
- Guarding: defenders spread over the `defend` places by rank; each
  hunts the enemy nearest the middle within `DEFEND_REACH` (2) reaches
  of it, else takes the place as above, so it stands in and contests it.

Siege sets one `attack` for the attackers and one `defend` for the
defenders on the front its HUD marks, and sets them again when the front
moves (`content/modes/rules/siege.rules`, `show_front`).
