# Bots: vehicles and Firefight

Seat entry, driving and Firefight seats (sections 3.9 and 3.10). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 3.9 Vehicles

The user, after playing the Clankers: "clankers also don't seem to
interact with vehicles, try and board, go and take, lock on with
rockets." The vehicle code lives in `crates/live/bots/src/vehicle.rs`, `vehicle/` and
`mind/ride.rs`. The bot sees each seat and its rider. It can enter a free
vehicle, board a slow enemy, pilot a Banshee, drive ground vehicles and
man a turret. A rocket launcher holds its lock on a vehicle before firing.

**Fields.** `O` is an object's body, `U` a unit's, `V` a `vehi` tag's
metadata and `S` one of its seat definitions.

| Field | Where | Type | Confidence | From |
|---|---|---|---|---|
| parent: a rider's vehicle, a mounted turret's vehicle | `O+0x14` | datum | confirmed | live-gamestate.md; `engine::view::objects::Body::parent`; `live-telemetry` `object_in` (riders) and `vehicle_mount` (turrets) |
| position, forward, up | `O+0x64`, `+0x70`, `+0x7c` | `f32[3]` each | confirmed; relative to the parent when there is one | `engine::view::objects::Body`; live-telemetry.md (a mounted turret's `+0x64` is relative to its vehicle). A rider's own position is not used: as telemetry does, a rider is where its vehicle is |
| body vitality | `O+0xec` | `f32`, 0 to 1 | confirmed | `object_get_health` `0x2a1a20` (live-telemetry `layout::BODY_VITALITY`) |
| wrecked | `O+0x10a` bit 2 | `u16` | confirmed | `0xd6800` (`guest::BODY_DEPLETED`) |
| the unit's seat | `U+0x1fc` | `i16`, -1 none | confirmed | live-telemetry `layout::UNIT_SEAT`: the enter-vehicle routine `0xea1f0` stores it (`0xea405`), leaving stores -1 (`0xea995`) |
| seats block | `V+0x1c8` count, `+0x1cc` pointer | `i32`, `u32` | confirmed | `crates/formats/tags/src/defs/unit.txt` (`group vehi`, `seats`, `0xb0` each; the seat label at `+4` checked against 05a's scripts, `h1scripts::seats`); the stride is `0xea1f0`'s `imul edi,0xb0` (`0xea330`) |
| seat flags | `S+0` | `u32` | traced and disc-checked | Driver bit 2, gunner bit 3, boarding bit 11, forbids players bit 16. See `h1objects::rebuild` and `h1objects/tests/vehicle_rebuild_disc.rs` (`0x000cc810`, `0x000c8c87`, `0x000c92c0`). The first unflagged passenger is used as fallback driver **[guess]**. |
| boarding target seat | `S+0x3e` | `i16` | traced | `0x000c92c0`, `h1objects::rebuild::SEAT_BOARDING` |
| entry marker and admission limits | `S+0xc`, `+0x98`, `+0x9c`, `+0xa0`, `+0xa4` | string ID, four `f32`s | traced and disc-checked | `0x000c6fb0` resolves the marker, with the object's origin for an empty ID. `0x000c9040` checks eye-to-marker radius, marker facing, look cone and distance to the vehicle. See `h1objects/tests/vehicle_ground_disc.rs`. |
| render-model reference and marker data | `V+0x38` → `hlmt+4` → `mode` | tag datums | traced and disc-checked | `defs/obje.txt` and `defs/hlmt.txt`. In `defs/mode.txt`, `mode+0x58` is the `0xc`-byte group block (ID `+0`, markers at `+4`); the `0x24`-byte marker holds variant indices at `+0/+1`, a node at `+2`, translation `+4` and quaternion `+0x10`. |
| live marker transform | `O+0x114/+0x116` | block size and signed relative offset | traced | [native-renderer.md](../graphics/native-objects.md#research-points-code-unless-marked-h2-disasm). Stride `0x34`; matrix scale `+0`, forward `+4`, left `+0x10`, up `+0x1c`, position `+0x28`. |
| chassis dimensions | `mode+0x14` compression block | position bounds | disc-checked | `defs/mode.txt`; each record begins with six floats giving xyz min/max. The planner uses lateral extent and vertical span, with margin and class fallbacks **[guess]**. |
| vehicle type | `V+0x1f0` | `u16` | traced and disc-checked | `0xef545` compares it with 5 and 6. `h1objects::rebuild::H1_VEHICLE_TYPE` and its disc test compare the classes by vehicle (tank 0, jeep 1, scout 4, fighter 5, turret 6). |
| fighter secondary weapon | `V+0x1c0`, `weap+0x2c8/+0x2d0` | weapon references, trigger and barrel blocks | traced and disc-checked | `defs/unit.txt` and `bots::arms`. The retail Ascension Banshee weapon has two triggers and barrels. Barrel 1's projectile is `banshee_bomb`. The tag establishes availability in the weapon definition. The game decides whether the variant will fire it. |
| the gun mounted on a Warthog | a vehicle object whose parent is the Warthog; its gunner's parent is the gun | | confirmed | live-telemetry.md (`parent` on a frame vehicle); `live-telemetry` `vehicle_mount` |
| a rocket launcher | `weap` zoom levels `+0x1fe` 1, and the blast role (§3.6) | | checked on Zanzibar's tags (zoom) | §3.6's handling table: the rocket launcher is the only blast weapon with a zoom level |

How the bot reads them (`vehicle::read::sight`, once a frame for each bot
in a multiplayer game, after the perception): the objects array (at most
2048 slots) for live vehicle objects that are not wrecked and whose
definition is a `vehi` tag, each with its class and seats; a mounted
turret's pose is made a world one from its vehicle's; then each listed
player's living biped whose parent is one of those vehicles fills the
seat `U+0x1fc` names, and the bot's own is its `riding`. A fault leaves
the bot on foot, never blind. It is kept apart from `brain::Perception`
(the mind is handed it with `Mind::see_vehicles`), so every other test
and caller plays as before.

`guest::perceive` puts a rider where its vehicle is (its own
`+0x64` is relative to it): a Warthog's driver and an enemy on its gun are
seen at the Warthog and at the gun, not near the world's origin.

**Lock-on** (`vehicle::lock`). With a rocket launcher in hand
(`lock::locks_on`) and the target riding a vehicle, the bot aims at the
vehicle's middle (0.3 over its origin) with no lead (a locked rocket
follows it), and fires only once it has held the vehicle in the reticle
(the vehicle's apparent size, 0.8 world units, plus the skill's fire
cone) for the lock time. At infantry nothing changes: fired without a lock, at
the feet for Heroic and Legendary (§3.4). The game's own lock time is not
traced; the bot assumes 1 s (`lock::LOCK_TICKS`) and adds its margin:

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| waits for the lock | no (fires as at infantry) | yes | yes | yes |
| holds past 1 s | | 0.4 s | 0.2 s | 0.07 s |
| off the vehicle forgiven | | 3 ticks | 6 ticks | 8 ticks |

The bot does not zoom: Halo 2's launcher is taken to lock without it
(§12). "A blast weapon with a zoom" stands for the rocket launcher; the
fuel rod's zoom was not looked at (it is not on Zanzibar). If it has one,
a bot with it waits a second at a vehicle and then fires: slower, not
wrong.

**Ground-route performance.** Ground-vehicle A* accounted
for 2,917 of 5,201 game-thread samples in a Waterworks 2v2 sim. Only a
ground driver asks for this path. A failed plan leaves it free to choose
again next think, including picking a new roam goal.

An exhausted search now remembers which polygons it reached from that start.
Further goals outside that set fail immediately. A successful search can
reuse its corridor for the same start and goal polygons and append the
current snapped endpoint.

Each driver caches portal clearance for its loaded map, vehicle datum and
exact hull dimensions. Steering still checks the next leg from the vehicle's
current position. A blocked leg drops the cached corridor for the next plan
and keeps the normal steering retry.

**Which vehicles.** By class (`vehicle::Class::spec`).

| Class | Seat taken | Controller behaviour |
|---|---|---|
| Jeep (Warthog) | Driver | Ground routing |
| Scout (Ghost, Spectre, Wraith) | Driver | Ground routing and primary fire |
| Tank (Scorpion) | Driver | Ground routing and primary fire |
| Fighter (Banshee) | Driver | Altitude control, attack runs, boost, evasive rolls and landing |
| Turret | Gunner, standalone or on a teammate's vehicle | Track and fire into the contested area, dismount if flanked |
| Boat or plane | None | |

The bot looks up the chosen seat's entry markers in the actual vehicle's
render model and places them using the live node matrices. Scorpion and
Wraith driver seats can offer two positions. It searches for a stand point
on the floor within the tag's radius and facing limits, looks at the marker
and holds X while in range of the seat.

The stand-point offsets and the 0.6 world-unit eye height used in this check
are **[guess]**. A retry uses a closer point covered by the same entry data.

**Go and take** (`vehicle::plan::take`, `Mind::mount_plan`). A phantom
can choose a vehicle after 1.5 s with no enemy in view if it carries no
objective and seeks no weapon within 20 units. A weapon farther away can
become a vehicle errand. The phantom drives to within 3 units of it and
finishes on foot.

In an objective game it chooses a vehicle only when the objective point is
more than 20 units away. It takes the nearest vehicle by distance (plus 3
per unit of height) within the skill's range that:

- is a class it drives, whole (over 0.4), upright, standing still (under
  0.5 a second);
- has its driver's seat free and nobody of the other side aboard;
- no teammate on foot is clearly nearer to (within 6, and nearer than 0.8
  of the bot's distance): they have it;
- it has not given up on, or got out of, in the last 10 s.

The bot walks to a valid entry point and tries X (action, Default layout)
for up to 40 ticks. It retries closer, then shuns the vehicle for 10 s.
It confirms entry using `U+0x1fc` and the parent. It abandons the attempt
after 12 s, when the seat is taken, or when an enemy gets in.

**Turrets.** A standalone turret or a free gun on a nearby teammate's
vehicle is worth taking when it has a clear line to recent enemy contact
or the objective the bot is defending or hunting. On the gun, an enemy
within 1.8, or within 5 and off the firing line on the same floor, can
make the bot dismount **[guess]**.

| | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| takes a vehicle within | 12 | 20 | 28 | 35 |
| mans a teammate's gun | no | yes | yes | yes |
| boards within (XY) | never | 1.5 | 2.2 | 2.8 |
| boards going slower than (units a second) | | 1.5 | 3 | 4.5 |
| runs infantry over | no | yes | yes | yes |

**Board** (`vehicle::plan::board`). An enemy (in free-for-all, anyone)
driving a vehicle (not a Banshee) on the bot's level (within 1 in height),
within the skill's boarding range, slower than its boarding speed: in a
fight or not, the bot approaches an authored boarding marker targeting
the occupied driver's seat and holds X when it meets the admission checks.
It lets go when the vehicle speeds off or after 3 s. Speeds come from the vehicles' positions tick to tick
(`vehicle::Memory`), not from `O+0x88`, whose units are not checked.

**Drive** (`vehicle::drive`, `Mind::ride`). Halo 2 steers a Ghost or a
Warthog by the camera: the left stick says where to go relative to where
the player looks. The driver:

- goes to the objective's point, else the enemy it fights, else where an
  enemy was last seen, else the weapon it seeks (the errand it mounted
  for), else a roaming goal (§3.2);
- plans over walk edges whose portals fit the vehicle and routes through
  their clear spans. Centre and wheel-line probes check floor, overhead
  room and walls before steering, cutting a corner, reversing or trying
  to splatter someone. After drifting off the mesh it keeps its goal and
  looks for nearby ground it can steer back onto. With an objective on
  an unreachable road it dismounts to finish on foot. If the chassis
  cannot reach a roaming goal, the phantom blacklists it for 10 s. A
  driver that goes 4 s without usable steering or a fight gets out and
  continues on foot. A failed roam plan counts as 16 ticks towards that
  limit, and a fight or usable steering clears the count. While it
  waits for a route, the driver sweeps the view;
- looks at the carrot (or the enemy it fights) and pushes the stick that
  way, full ahead when the camera is on it, down to 0.35 at a turn of 1.6
  radians or more;
- with infantry in front within 15, drives at them (splatter), the skill
  allowing;
- shoots with the vehicle's guns when its seat has them (a Ghost's, a
  Scorpion's; not a Warthog driver's);
- under throttle and slower than 0.4 a second for 1.5 s, backs up for 1 s;
  the third time within 15 s it is stuck for good.

**Fly** (`vehicle::flight`). The Banshee aims and climbs toward a cruise
height, uses hull-width collision probes and pitches into a visible target
for a strafe run. It breaks off at close range, boosts away, and uses a
lateral evasive roll when damaged and high enough. It requests a fuel rod
bomb on an aligned downward run when the fighter tag has a secondary trigger
and barrel.

It descends for an objective dismount or a damaged vehicle. Cruise height,
collision envelope, combat ranges, cooldowns and landing tolerances are
**[guess]**.

The Default Xbox pad uses left trigger for boost (also recorded in
`input::touch_layout::DRIVER`) and right trigger for primary fire.
The original manual describes the Banshee's secondary fuel rod weapon
and its ability to roll and loop. A with the left stick for the roll,
and B for the bomb, are **[guess]** in the phantom action path pending a
game run. The phantom lane already maps these pad buttons and triggers
to actions (§5.1). [Original Xbox manual](https://halo.wiki.gallery/images/7/76/H2_Human_Manual.pdf)

A gunner aims and shoots (the gun's trigger held within 15 units and in
the skill's bursts beyond, §3.4); a
passenger shoots its own gun as on foot. With nothing to shoot they look
where the enemy was last seen, else ahead.

**Getting out** (`drive::leave`): the vehicle under 0.2, lying flipped
for 1 s, stuck for good (driver), nowhere to drive for 4 s (driver; see
above), nobody driving for 4 s (gunner, passenger), or within 3 of an
objective point it must reach on foot (the flag or ball it is going
for). It holds X 15 ticks in every 20, so a press and a hold both work,
and stops driving. It avoids the vehicle it left for 10 s. When it
leaves because it has nowhere to drive, it avoids the vehicle for 30 s
and roams on foot.

**Seeing it in a game.** Two or three Clankers on a map with vehicles
(Coagulation, Headlong, Waterworks, Zanzibar), Slayer, Normal or
Legendary. §11.4 is the checklist.

**Open** (§12): phantom observations of each seat's entry and
exit, the lock time and zoom requirement, and measured lift and flight
behaviour. A Warthog driver waiting for its gunner remains open.

## 3.10 Firefight

Code: `crates/live/bots/src/firefight.rs`, tests in
`crates/live/bots/src/firefight/tests.rs`. The mode is
[firefight.md](firefight.md).

Clankers play Firefight as players. They are always on a player team, never
on the AI's. With one player team they fill seats on it alongside people;
with two they fill seats on either, keeping the teams even, and fight the AI
and the other player team.

An ally squad's actors are on a player team (docs/game/firefight.md,
"Allies"): a bot drops the actors on its own team, or with one player team
any on a player team, from its perception before sight
(`firefight::drop_allies`), so it never targets them. With two teams the
other team's ally stays an enemy; and with the AI's team a player team
nothing is dropped, since no actor can be told from an ally.

**Seats.** While the lobby is set up for Firefight the game sets the
player team count (`bots::firefight::set_seat_teams`, 1 or 2). A new
phantom's lobby team (`phantom::session::lobby_team`) is then
`firefight::seat_team`: team 0 with one player team, else whichever of 0
and 1 has fewer players, 0 on a tie. `rebalance` moves a phantom on any
other team onto a player team. The host gives the actors a team no player
holds (firefight.md, "The host runtime").

**The match.** The Firefight runtime describes the match to the bots
with `firefight::set_context(Some(Context))` and clears it with `None`:

| `Context` field | Meaning |
|---|---|
| `player_teams` | the player teams' engine team indices, Firefight team 0 first (one or two) |
| `ai_team` | the actors' engine team |
| `bases` | each player team's home base point: the kit's zones `ff_base_00` and `ff_base_01` (the host's `Kit.bases`) |
| `lives` | each player team's lives left, `None` for unlimited |
| `phase` | `Fighting` during a wave, `Between` before and between waves |
| `drop_zones` | the kit's drop zones, where waves arrive |

**Enemies.** Each sample, while a context is set and the bot is alive,
`firefight::read_actors` appends the actor pool's living actors to the
perception's others. Each is a `Seen` with id `ACTOR_ID_BASE` (`0x100`) plus
the actor's datum index, its unit's origin (a rider's is its vehicle's) and
its team. A unit whose body is depleted is left out. `firefight::normalise`
then makes the game a team game; with one player team it puts every player
on that team whatever the engine's team byte says; and it gives actors the
AI's team, or `ACTOR_TEAM` when the AI's team is a player team or the bot's
own.

The sight filter keeps an actor only while it is in view (an actor has no
player bit), and the mind treats an actor as an enemy whatever its team
(`firefight::hostile`). So actors go through the same sight, target choice,
aim and grenade path as players.

A teammate is never a target. With two teams the other team's players are
enemies as in any team game.

**Behaviour.** `firefight::State::plan` gives the goal, after an outside
order and before the objective's:

| Condition | Goal |
|---|---|
| the team's lives at or under its living players (the bot and teammates in view or marked) | hold home within `REGROUP_RADIUS` (3) |
| shield (at most 1) plus health under the name's fall-back line | hold home within `FALLBACK_RADIUS` (2.5) |
| between waves, within the errand radius of home, and not at a drop zone alone | none: the bot rearms, collects grenades and roams as in any game |
| otherwise | hold home within the name's hold radius |

The errand radius is `ERRAND` (2) hold radii. A point within `DROP_NEAR` (8)
of a drop zone outside the hold radius is the drop zone's, and the bot goes
there only with a teammate within `COMPANY` (5). Once out of its free area,
it walks back inside the hold radius before it is free again.

A hold goal leashes a fight to the radius (§3.3), so the bot fights from
home and never chases out to a drop zone. Weapons and ammunition outside the
free area are left where they lie (`State::blocks`, in the weapon plan's
filter).

Two numbers come from the name (`bots::name_profile`): the hold radius
is `HOLD_RADIUS` (8) times 0.75 to 1.25 by the name's aggression, and
the fall-back line is 0.4 to 1.0 of vitality by its retreat.

A bot on no player team (never seated so) asks nothing of Firefight and
still fights the actors.

**Not verified.** Whether the actor pool's positions match what a
player sees on a client, whether the sight filter's chest height suits
every AI class, and the leash and radii in a played match.
