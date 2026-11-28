# Rules world API: the API

The world API. Part of [rules-world-api.md](rules-world-api.md).

## The world API

Every entry is a row of `caps::ENTRIES` with its capability, kind, order
and status, appended to the table (nothing is published, so the API
version stays 1). Reads need no capability. Every action here is
`Order::Queued`: made by the `before` on the next `GAME_TICK`, in
program order, returning nothing.

**Code layout.** One module per capability group, on both sides: the
entries, signatures and fake of a group in
`crates/formats/rules/src/world/<group>.rs`, and its guest half in
`crates/extensions/game/src/rules/world/<group>.rs`, for `objects`,
`vehicles`, `squads`, `bots`, `audio`, `hud`, `physics`, `data` and
`show`. The host's zone test (`world/zones.rs`, the labelled flags as
cylinders and hill polygons, behind `inside`, `ticks_held`, `on enter`
and `on leave`), `set_loadout` (`world/loadout.rs`), the pickers (`world/pick.rs`), the spawn rules
(`world/spawns.rs`) and the bodies (`world/body.rs`) are guest halves of the core entries, with no group
of their own
([rules-engine.md](rules-engine.md#the-host-runtime)).
`rules::vm::Game` gains one sub-trait per group, so a fake or a
host implements a group whole or not at all. The traits take typed
handles (`ObjectRef`, `PlayerSlot`, `SquadSlot`, `VehicleRef`,
`WidgetId`, `WaypointId`, `TagRef`, `Vec3Fixed`), never a `u32`; a
handle becomes a salted datum in one place,
`crates/extensions/game/src/rules/guest.rs`, which checks it is live
first. Each engine routine an entry reaches is one named point in
`crates/hooking/guest-layout/src/points.rs` with its signature; a
`world/` module holds no address, and an entry is **named**, and so
hostable, only once its point exists.

### Types

All are front-end types, erased before the IR as `zone` and `time` are
([rules-language-types.md](rules-language-types.md#front-end-types)):

| Type | What it is | In the IR |
|---|---|---|
| `vec` | three `fixed`: `vec(x, y, z)`, `.x`, `.y`, `.z`, `+`, `-`, `v * k` (`k` a `fixed`; a bare literal may lead), `length(v)`, `distance(a, b)`. World units in 16.16, so a coordinate is within ±32,768. It is the value of an expression or a `let`; it is not stored in a variable, option, table, parameter or result, and it has no `==`, order or `/` (the checker says so by name) | three `Fixed` registers kept together (`lang/lower.rs`, `vec_expr`); a `vec` read is three calls with an axis argument; `length` is the VM's `vec_length`, taken whole in 128 bits so no square overflows |
| `vehicle` | an object whose tag is a vehicle; `as_vehicle(o)` gives a `vehicle?`. Not built: the vehicle entries take an `object` and the game refuses a call on one that is no vehicle ("Vehicles and drop ships") | `Object` |
| `sound` | a tag path that must be a sound's, as `weapon` is a weapon's | `Tag`, with the tag table's group |
| `line` | a dialogue vocalisation by name, checked at load against the map's dialogue names | `Text` |
| label kind `path` | an ordered set of at least two labelled points, numbered from 0 with no gap and none twice; the start gate checks it (`labels::check`) and only `fly` takes it | `LabelKind::Path` in the label table, with the `numbered` flag |
| `hud` and `waypoint` declarations | a fixed list of HUD elements and waypoint slots the script owns (below) | a header table |

An action's `at` argument is an `object` (any handle, or a labelled
thing with no object, which already has a handle of its own) or a `vec`;
the compiler picks the entry form by type, as `message` does
(`caps::spelled`).

### Objects

Capability `objects.move` ("moves objects"); `player.move` ("moves
players") for a player's unit, so a hopper can deny one without the
other.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `o.position`, `o.velocity` (`vec`), `o.speed` (`fixed`) | `object_pos(Object, Int) -> Fixed`, `object_vel(Object, Int) -> Fixed`, reads; `o.speed` is `vec_length` of the velocity | the object datum's position and velocity, the root ancestor's for a child (`rules/world/objects.rs`; the engine's own are `OBJECT_WORLD_POSITION` `0xb9dd0`, `OBJECT_ROOT_VELOCITY` `0xba1d0`) | named (**Code**) | a read on the machine that runs the VM |
| `teleport(o, at)`, `teleport(o, at, offset)` | `teleport(Object, Fixed, Fixed, Fixed)`, queued: `at` is a `vec`, or an object, whose position the compiler reads and adds the offset to | `OBJECT_SET_TRANSFORM` (`0xb7430`) with the object's own forward and up kept, `al` false when the position is outside the world, which is a refused call, recorded, that moves nothing (`rules/world/objects.rs`) | named (**Code**); **S** that it marks position (R2) | position flag |
| `face(o, at)` | | | refused by name: whether the facing flags carry a host-set facing to clients is R2 | |
| `teleport(p, at)` | `teleport_player` | HaloScript `object_teleport`'s body `0x29ffb0` through `PLAYER_PLACE_NEAR` (`0x14e970`), which takes the unit out of a vehicle first (**Code**, `crates/extensions/agent/src/sim/calls.rs`, `crates/extensions/game/src/coop_players.rs`) | refused by name: the checker says a player is not moved, citing R9 | position flag; **S** how a client's own predicted biped takes it |
| `move_to(o, at, t)`, `stop(o)`; `on arrive(o)` | `move_to(Object, Fixed, Fixed, Fixed, Int)` and `stop(Object)`, now; the VM's own event | a **mover**: a `RulesExt` record (object, from, to, start tick, duration; `vm::Mover`, at most 16) the VM steps every tick in the order started, telling the game the place on the straight line from where the object was when `move_to` was called, and `to` exactly on the end tick, when `on arrive(o)` runs. `t` of 0 or less is one tick. A second `move_to` for an object replaces its mover with no `on arrive`; `stop` ends it with none; a seventeenth mover is a script error; the round's end clears them. The host puts the object at each step (`OBJECT_SET_TRANSFORM`) at the next boundary; the velocity of a step is not pushed (`OBJECT_SET_VELOCITY` needs two vectors in guest memory, and what marks the entity is R2) | named (**Code**); **S** that a client sees the move (R2, R3) | position flag |
| `move_along(o, path, t)` | | | refused by name: it needs the `path` label kind, which W8 builds; `move_to` takes one point | |
| `attach(o, to)`, `detach(o)` | `attach`, `detach` | no entry read; the object datum has a parent field, and HaloScript's `objects_attach` evaluator is not traced | untraced | nothing generic: only a unit's parent vehicle replicates (**E**). Until traced, `move_to` with the parent's `at` each tick is the script's follow |
| `set_open(o, open)` | `set_open(Object, Bool)`, now | a barrier opened or closed: the first `set_open(o, true)` notes where the object stands (`vm::Gate`, in `RulesExt::gates`, at most 16, the round's end closes them) and steps it 64 world units below, and `set_open(o, false)` steps it back and forgets it; asking for the state it is in does nothing, and a seventeenth open barrier is a script error. The steps are the host's puts (`OBJECT_SET_TRANSFORM`, as `teleport`), so the host adds no call; a put outside the world is a refused call, which the host tells the VM (`Vm::put_refused`, from `Runtime::finish`) when it was the put that opened a barrier: the VM forgets the barrier, so it stands closed in the state too and the next `set_open(o, true)` tries again | named (**Code**); whether the barrier stops blocking once put there and whether a client sees it move are R2 and R3 | position flag |
| `set_power(o, f)`, `set_position(o, f)` on a device | `device_power`, `device_position` | Forge's device sinks ([forge-features.md](forge-features.md#teleporters-buttons-and-triggers)); the engine setters are not traced | untraced | the device entity (type 16, **E**) |

`create` and `delete` are the existing entries. A created object has no
handle the script can hold, since a queued call returns nothing; a
script that must move what it makes labels a placement and moves that.

### Vehicles and drop ships

Capability `vehicles.drive` ("flies and loads vehicles").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `vehicle_riders(v)` (`int`), `vehicle_driver(v)` (`player?`) | reads | the unit's seat iterator (`UNIT_SEATS_BEGIN` and `UNIT_SEATS_NEXT`) | host reads pending | a read |
| `fly(v, path, t)` | `fly` | the host flies the ship along the path's points with `Flights` and puts it at each tick's pose through `OBJECT_SET_TRANSFORM` with notify; a notified transform reaches eligible Havok bodies, which retain velocity | **Code** in Firefight (`Game::fly`, `crates/extensions/game/src/firefight/ship.rs`); not built in the rules runtime | the vehicle entity (type 12) |
| `hover(v, on)` | `hover` | `VEHICLE_HOVER` (`0xf0fd0`) toggles the hold bit and refreshes the physics component on a change | named | vehicle replication unverified |
| `seat(p, v)`, `eject(p)` | `seat`, `eject` | the vehicle entry routine `0x11ade0` behind `unit_enter_vehicle` and the biped exit action at `0xe68c0` | traced | events 24 and 11 |
| `place_squad_in(s, class, n, v)` | `place_squad_in` | places `min(n, free seats)` actors at the ship and seats them through `VEHICLE_LOAD` (`0x11b0c0`); `AI_SQUAD_PLACE` alone selects by the scenario's vehicle palette and can leave actors unseated | **Code** in Firefight (`Game::drop_squad`, `ship::place_squad_in`); not built in the rules runtime | unit entities with the parent-vehicle flag |
| `unload(v)` | `unload` | `VEHICLE_UNLOAD` (`0x11b2b0`) requests exit from occupied seats with the all-seats mapping | named | event 11 |
| `on board(p, v)`, `on leave_vehicle(p, v)` | events | observers on the enter and exit events the engine queues (`SIM_EVENT_GENERATE` `0xb5a70`, types 24 and 11) | named | read on the `Authority` |

The host queues `hover` and `unload` for the tick boundary. It resolves a
labelled vehicle placement to a live datum and rechecks the datum before
calling retail. The fake also implements the entries below. A host
refuses scripts using them at start. [Retail traces for R6 and R7](notes/2026-11-24-vehicle-actions.md).

| Entry | Held back by |
|---|---|
| `fly` in the rules runtime | the mover's `Flights` state lives in Firefight's match; a plain script's host has none, and the pose after physics steps with riders and contacts is R7 |
| `place_squad_in` in the rules runtime | the squad entry is Firefight's; the plain rules host has no squad placement |
| `seat`, `eject`, `vehicle_riders`, `vehicle_driver` | host seat selection, player actions and synchronous seat reads |
| the kit's ship and paths in the plain rules host | the kit's ships are `ff_ship_<kind>` and its paths are the point sets `ff_path_NN`, which the Firefight guest reads (`firefight::guest::read_kit`); a plain script has no label for them. R8 settles headroom per map |

Four choices of the fake are the API's contract, and a host that gives
different answers is wrong until a run says the engine does:

- A **vehicle** is an `object` the game checks at the call. `vehicle` is
  not a front-end type, and there is no `as_vehicle`, `v.driver` or
  `v.riders` (the entries above read them): a type would add no safety,
  since a call on a crate is a refused call either way, and it would
  edit `lang/check.rs` in the same waves as W2, W3 and W5.
- A **refused call** changes nothing and is recorded (the fake's
  `refused`): a vehicle entry on an object that is no vehicle, a seat
  with none free, a dead player, a flight of no ticks or over a path with
  a point not placed.
- **Seats** fill driver first. A player who boards another vehicle leaves
  the first, and one who dies or quits leaves at once.
  `place_squad_in` seats `min(n, free seats)` actors and is refused when
  none fit; `unload` takes everyone out, players and actors.
- A **flight** is `vehicles::Flight`: one speed along the polyline of the
  path's points in number order, so the vehicle is at the last point on
  the tick `start + t` and covers equal lengths in equal ticks; its
  facing is the unit tangent of its segment (a corner belongs to the
  second). The math is 16.16 integers, so every machine and a film agree.
  The fake reports each flight's end once (`take_arrived`); `on arrive`
  is W3's event, and until it lands a script times the end itself
  (`now()` against the tick it started on, as `tests/vehicles.rs` does).

**Firefight's director flies the ship.** The director script
(`content/modes/rules/firefight.rules`) does not use the entries above: it
holds the ship's state (parked, inbound, unloading, outbound) and calls
the `firefight.director` entries `ff_drop_squad`, `ff_ship_hover`,
`ff_ship_unload` and `ff_ship_leave`, which the host makes with `Game::fly`,
`Game::drop_squad`, `Game::hover` and `Game::unload`
([firefight.md](firefight.md), "The drop ship"). Two reads tell it what
the kit has: `ship_ready(kind)` and `zone_path(zone, kind)`, and
`squad_ship(class)` the kind a class's faction rides (0 the Phantom, 1 the
Pelican, -1 none).

**A drop ship is a vehicle on a keyed path.** The kit places it, parked,
with a label (`label ship: object`), and labels the run-in as a `path`.
The script loads a squad, flies the path in a stated time, hears
`on arrive`, unloads and flies out. Three other ways are weighed:

| Way | Why not first |
|---|---|
| An AI pilot | engine flight needs airborne firing positions and a pathing kit a multiplayer map lacks; the result differs per run of the AI |
| A recorded path (`recording_play`) | the recording player (`0x1fb630`) applies unit control streams through `UNIT_APPLY_CONTROL` (`0xc6de0`, **Code**, `crates/extensions/game/src/h1_recordings.rs`); the streams are scenario data a multiplayer map does not hold, and authoring one needs a recorder |
| A device machine with an animation | no tag of ours can be authored for it (no game data) |

The Pelican and Phantom tags are not on a multiplayer map. They come
with the kit's stage 2 (`tags::schema::transplant` from the player's own
campaign maps, firefight.md "The kit"), so every machine builds them and
tag indices agree.

### AI squads

Capability `ai.squads` (exists: "places and commands AI"). A script
commands by squad and never per actor (rules-engine.md "Firefight, AI
and bounces").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `place_squad`, `place_squad_at`, `hunt`, `erase_squad`, `squad_alive` | exist | `actor_net::squads::SquadEntry` | named | the host log; bodies by unit entities |
| `go_to(s, zone)`, `hold(s, zone)` | `squad_go`, `squad_hold` | `AI_SET_ORDERS` (`0x275cb0`) with an order the kit authors for each drop zone and each labelled zone, as it authors `ff_hunt` (firefight.md "Guest boundary") | named point; the kit's orders not built | the host decides; bodies replicate |
| `follow(s, p)` | `squad_follow` | an order's follow target, a field the kit writes empty; how the engine resolves it is not read | untraced | as above |
| `attack(s, o)` | `squad_attack` | no entry; `AI_TARGET_SCORE` (`0x265d30`) is the point a preference would answer | traced | as above |
| `set_squad_team(s, t)` | `squad_team` | `AI_SET_TEAM` (`0x275b60`) | named | unit team in the entity |
| a map's own squads, by name: `label wardens: squad = "<squad name>"` | a label kind `squad` | the scenario's squad block (`scnr+0x160`), read by name as the kit's slots are | not built; see "Advanced data" |

### Bots

Capability `bots.orders` ("gives bots orders"). A bot is a phantom: a
player seat driven by a brain on the host ([bots.md](bots.md) section
5). To a script it is a `player`; a `p.is_bot` read of the phantom
registry (`bots::phantom::list`) is design, not built.

**Code** (slice W7). An order is a `bots::brain::Order` - `Go`, `Hold`,
`Follow` or `Attack` - kept on the brain's decisions
(`crates/live/bots/src/decisions.rs`). While one stands it is the bot's
goal in place of the objective's (`crates/live/bots/src/mind/strategy.rs`):
`Go` and `Hold` walk the mind's own nav follower, `Follow` and `Attack`
are its `Escort` and `Hunt` goals with the target's position from the
roster each tick. The order survives a death; only `bot_free` clears it.
The call is queued like every world action and joins the tick boundary
as a `rules::runtime::Call::BotOrder`, whose async body only the machine
`GAME_ENGINE_AUTHORITATIVE` answers true for runs. There it delivers
through `bots::phantom::order`; an order to a seat no joined phantom
holds is a refused call, recorded. A run playing a film back makes none
(`GuestGame`'s `film`), as "Films and Theater" says. The game side is
`crates/extensions/game/src/rules/world/bots.rs`: it resolves `bot_go`'s
and `bot_hold`'s zone through the census's labelled things and the
scenario's netgame flags: the centre is the mean of the zone's members'
positions, the reach the farthest member's distance from it and at
least 4 world units (`bots::objective::DEFEND_RADIUS`) while no zone's
capture volume is read.

| Script | Entry | Backed by | Status |
|---|---|---|---|
| `bot_go(p, at)`, `bot_hold(p, zone)` | `bot_go`, `bot_hold` | the order as the goal (`Go`, `Hold`), walked by the mind's nav follower over the map's nav mesh (`crates/live/bots/src/mind.rs`) | named |
| `bot_follow(p, q)`, `bot_attack(p, q)` | `bot_follow`, `bot_attack` | the brain's own escort and target choice (`crates/live/bots/src/decisions.rs`), with the order as its goal (`Escort`, `Hunt`) | named |
| `bot_free(p)` | `bot_free` | clears the order | named |
| `bot_goal_attack(t, zone)`, `bot_goal_defend(t, zone)`, `bot_goal_hold(t, zone)` | the same three | a place in team `t`'s goal table (`bots::objective::script`): every phantom of the team plays it as its objective ([bots.md](bots.md) section 3.7, "A script's goals") | named |
| `bot_goal_clear(t)` | `bot_goal_clear` | clears team `t`'s places | named |

A goal is a team's, not a seat's: one call a place for each side is
enough, and it lasts until the team's places are cleared or the game
ends (`Runtime::ended`, `stop` and a new game's start clear the table).
The host resolves the zone through `world::zones::Zones`, so a bot's
"in it" is the zone the script's `inside` tests, its own area or
corners included. Setting a zone a team already has changes its aim. A
team holds at most 8 places (`objective::script::TEAM_GOALS`, the
engine's objective records); a ninth is a refused call, recorded with
its text (`bot goal refused: team N has 8 goals already ...`). A call
through `none`, or with a zone that is no netgame flag, makes nothing.
An outside order still comes first for its seat.

An order returns nothing. Whether the bot got there is a world read
(`inside(p, zone)`, `distance`), so a script's state never depends on a
brain, which runs off the game thread (bots.md section 5.5). The effect
is the phantom's pad, which every session kind already carries as a
player's input. Seats are added in the lobby (bots.md section 7); a
script adds and removes none.

### Zones

No capability: core entries with a host half but no group of their own
(`world/zones.rs`). A zone is a labelled netgame flag - the host's census
has no Forge document at a game's start, so no zone is a trigger volume.
A zone's shape is an area round its flag
([gametype-markers.md](gametype-markers.md), "In a script"), from the
first of three that applies: the flag's own area, stored in the Forge
document of a map variant and carried by the census (`Thing::area`), a
radius, a box (turned with the flag's facing), a sphere or a path at its
own heights; else the flags that share a label
and a number, three or more of them (a King of the Hill hill's corners),
as one zone, `Area::of_corners`: the convex hull of their positions
across the floor; else the default area of the flag's marker around
it (`zones::default_area`): `Area::plate()` for a flag return, a bomb
goal and a siege goal, the capture plate their marker draws, and
`Area::default`, an upright cylinder, for any other. `rules.zones` reports each zone's source (`area`, `corners`
or `default`). The default's sizes are ours ("Not verified"). A body is a living
player's unit; the event order and the 64 zone events a tick are the
host runtime's ([rules-engine.md](rules-engine.md#the-host-runtime)).

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `inside(p, zone)` | `inside`, a read | the zone test above, against the living player units' positions | named | a read on the machine that runs the VM |
| `player_of(o)` | `player_of`, a read | the roster's units (the player datum's unit), so no unit field is read | named | a read |
| `ticks_held(zone, team)` | `ticks_held`, a VM read | the VM's own counters over `Game::zone_holder`: the one team whose living players alone are inside, `none` when the zone is empty or contested | named | the state blob |
| `on enter(zone, o)`, `on leave(zone, o)`; `on spawn(p)`, `on death(p)` | VM events | the roster's and the zones' changes since the last tick (`world::zones::events`), in the order the host runtime tells them | named | the state blob |
| `show_area(zone, team)`, `hide_area(zone)` | `show_area`, `hide_area` | no engine call: the zone is kept in the VM's state (`RulesExt::shown`, at most eight, in the order first shown; a ninth is a script error; `none` for the team is neutral; the round's end clears them), and the runtime hands the zones' shapes to the world-line source after each tick (`world/walls.rs`), which draws each wall alone in the team's colour ([gametype-markers.md](gametype-markers.md), "When a marker shows"). No capability: it is a display | named | the state blob; a machine that runs the VM draws it, a film's re-run included. A client that does not run the script draws it from a walls cue ([the show channel](rules-world-api.md#the-show-channel)) |

### Loadouts

Capability `player.weapons`, as `give` and `take`. A script names a
weapon by its tag path, but the host gives only the multiplayer weapon
list's (`rules::weapons`, the starting-weapon values the variant editor
offers); a path off the list is refused by name, and nothing named gives
nothing.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `set_loadout(p, primary, secondary)` | `set_loadout` | `STARTING_WEAPON_PICK` (`0x19eda0`) answers the value's tag from the map's multiplayer globals, as the spawn's weapon pass asks it; the weapon is made and added as a starting profile does (`placement_data_new`, `object_new`, `unit_add_weapon`, and `object_delete` when the unit refuses it) - the primary in mode 2, which empties the unit's weapon slots first and readies it, the secondary in mode 1 beside it (mode 2 when there is no primary) | named | the unit's weapon entities |

### Pickers

Capability `player.pick`. A script offers each local player a short list
to pick from before the round, and reads the pick. The VM keeps the offers
and the picks in its state (`RulesExt::picks`, at most 2 pickers numbered
from 0 of at most 6 rows each; `RulesExt::choices`, a row for each player
and picker), so a checkpoint, a film's keyframe and a joiner read them as
they read every other record.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `offer_pick(n, title)`, `pick_row(n, label)` | `offer_pick(Int, Text)`, `pick_row(Int, Text)`, now | `offer_pick` makes picker `n` with no rows, replacing a picker of that number and the picks made in it; `pick_row` adds a row. A picker past the second, a row for a picker not offered and a seventh row are script errors (`ErrorKind::Picks`). The round's end takes the offers and the picks. The host adds no call of its own | named (**Code**) | the state blob |
| `picked(p, n)` | `picked`, a VM read | the row (from 0, in the order the rows were added) player `p` picked in picker `n`; -1 when none was made, none is offered or the player is outside the roster | named (**Code**) | the state blob |
| `offer(p, picker, count)` | `offer(Player, Int, Int)`, now | opens the first `count` rows (1 to 6) of `picker` (an offered picker's labels) to player `p` as their loadout window (`RulesExt::loadout`, `vm::Window`); a row last picked that the window still holds stays, else the player is back on row 0. A picker or count out of range, or a player outside the roster, is a script error (`ErrorKind::Picks`). The round's end closes every window and keeps each row. The host adds no call of its own | named (**Code**) | the state blob |
| `choice(p)` | `choice`, a VM read | the row player `p` last picked in their window, 0 until one is; -1 for a player outside the roster | named (**Code**) | the state blob |
| the host's loadout report | `Vm::choose_loadout(player, row)` | a row at or past the window's count, past the rows of its picker, or a player outside the roster is refused (`false`) and the last pick stands. The pickers a window reads are not asked as plain pickers (`Vm::plain_picks`) and are not sent as cues; the host asks the player from the window (`rules/world/choice.rs`: a `Call::OfferChoice` opens the same value picker over the player's controller once for each new window, with the open rows) | named (**Code**) | the state blob |
| the host's report | `Vm::choose(player, picker, row)` | a pick of a picker not offered, a row it does not hold or a player outside the roster is refused (`false`); a second pick replaces the first | named (**Code**) | the state blob |

On the host (`rules/world/pick.rs`) the runtime reads the offers at each
tick boundary on the authority, outside a film. For each new set of offers
it asks this machine's local players one at a time, in roster order: a
`Call::OfferPick` makes the game's value picker (`ui_kit::picker`, the
`kit.picker.game` dialog in a game, a select dialog over the running game)
over the player's controller, titled by the offer with its rows as the
values. A press on a row is reported from the picker's setter and handed to
`Vm::choose` at the next boundary; the next player's picker opens then, or
after 30 seconds (B on the picker reports nothing). Picks made before the
round's end reset are still handed over. The picker is the existing
data-driven one, so no screen of its own and no per-script Rust.

A player on another machine is asked the other way. The authority sends
each picker as an offer cue on the show channel
([the show channel](rules-world-api.md#the-show-channel), `rules::show::Cue::Pick`: the
picker's number, its title and its rows as string-table indexes, at most
16 bytes) when the offers change - a picker taken away goes as an empty
one - and again every 90 ticks while any stands. A client holds them
(`rules/world/pick.rs`, `client_offer`) and asks its own local players one
at a time at the tick boundary with the same value picker, while the
transport says picks travel upstream (`actor_net/link.rs::pick_upstream`,
read once a tick into `client_ready`).

A press goes back as the pick message
(`game/src/actor_net/pick.rs`: the roster slot, the picker and the row,
three bytes, bounds-checked on both ends). On the Live tunnel it rides the
client-to-host `ActorPick` frame (`live_proto::tunnel::Kind::ActorPick`,
kind 5, port 0, peer 0), whose payload is this machine's id and then the
message. The machine id is the machine field of the client's local player
(`actor_net/pick.rs::read_local_machine`), which the game replicates, so
the host finds the roster slots by it. The host's tunnel reads the frame
only from a peer that negotiated the actor channel, within the peer's
datagram rate, and binds the machine id of a peer's first pick to that
peer: another peer cannot name it and the peer cannot change it. The
game thread takes the picks each tick (`actor_net/link.rs::take_picks`),
resolves the machine id to the remote, listed players that carry it
(`actor_net/pick.rs::seats_of`), and `actor_net/pick.rs::receive` keeps a
pick only for those players and hands it to `Vm::choose` like its own
local reports, so `picked` reads a remote pick as it reads a local one.
A client that has not yet read its own machine id (no local player in the
roster) sends nothing, and a full upstream queue (8 on the client, 32 on
the host) drops the pick; the player's next press replaces it.

Where it travels:

- **A dedicated host**, through the Live tunnel: the host's players are
  all remote, and every client pick travels. The relay in `live-server`
  and the Cloudflare worker's host room forward each frame as bytes, so
  both homes carry the frame as they carry the key request
  ([dedicated-host.md](../live/dedicated-host.md)).
- **System Link.** `crates/live/show-lan` carries the same message as
  a signed `pick` datagram (kind 4: peer id, a counter newer than the last
  accepted, the machine id, the message, the channel's tag), which
  `Host::take_picks` hands back with its machine id. The carrier is not
  yet wired to the game, so a System Link client's `pick_upstream` is
  false and its player is not asked.
- **A Live match with no dedicated host** has no tunnel: its host-run
  script reads -1 for a remote player, and a script falls back to the
  lobby's option, as `siege.rules` does.

A player who joins after the offers were made is asked by the next repeat
of the offer cue.

### Spawns

Capability `player.spawn`. A team's spawn rule is in the VM's state
until the round's end; the host reads it at each spawn, so neither call
waits for a tick boundary.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `spawn_areas(team, label, n)` | `spawn_areas` | `SPAWN_LOCATION_FREE` (`0x23bb70`), the free-spot test of `PLAYER_SPAWN_LOCATION` (`0x14ef00`), answers "taken" for each starting location not labelled `label` and numbered `n`; the chooser's own weights pick among the rest | named | the spawned unit |
| `spawn_near(team, zone, n)` | `spawn_near` | as above, keeping the `n` starting locations nearest the zone's middle; `none` or 0 lifts the rule | named | the spawned unit |

### Bodies

Capability `player.disguise`. A team's body is in the VM's state until
the round's end, and the host writes it at each spawn of the team's
players.

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `set_body(team, tag)` | `set_body` | the model byte (player `+0x88`) is the body's (0 `masterchief_mp`, 1 `elite_mp`) while `PLAYER_SPAWN` (`0x14f2e0`) runs, and the player's own after it; the representation pick `0x14f040` reads it for the unit type (`rules/world/body.rs`). `none` lifts it; another tag is a script error | named | the spawned unit; **S** a client's first-person hands |

### Audio

Capability `audio.play` ("plays sounds, dialogue and music").

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `sound_to(s, p)`, `sound_to_team(s, t)`, `sound_to_all(s)`: a sound in the ears of a player, a team or everyone | `sound_to`, `sound_to_team`, `sound_to_all` | the game's own sound start at each audience player's living unit, at its head marker: `SOUND_START_AT_MARKER` (`0x1891d0`), the call a replicated dialogue line and `audio.play` make (**Code**, `crates/extensions/game/src/rules/world/audio.rs`, `crates/extensions/game/src/actor_net/client/guest.rs`). The one-shot allocator (`SOUND_IMPULSE_ALLOCATE`, `0x126000`) is the game's own behind it; `sound_impulse_start`'s body (`0x2a9e00`) is not used | named | a sound cue (`rules::show::SoundCue`, kind 2) with the audience mask and the sound's index in the blob's tag table, which a `Viewer` checks is a sound-group entry before it plays it at its own players |
| `say(u, line)`: a unit speaks a line of its own voice | `say` | the unit's voice bank resolves the vocalisation (`0x1144a0`), `SOUND_START_AT_MARKER`, `BIPED_DIALOGUE_LIP_SYNC` (`0x109220`): the path a client plays a replicated line with (**Code**, `actor_net/client.rs`) | not built: no table of a map's dialogue names gives the `line` type its checked index | the actor channel's `Line` record, which exists |
| `sound_at(s, o)`: a sound at an object or a point | `sound_at` | `SOUND_IMPULSE_ALLOCATE` admits a one-shot; the scripts' `sound_impulse_start` evaluator is `0x2a9e00`; its body is not read (R4) | traced | a `Cue` with the object's entity or the point |
| `music(s)`, `music_stop()` | `music`, `music_stop` | a looping sound's start and stop: not traced (R5) | untraced | the show state's music slot, so a late machine starts it at the right offset |
| `announce(event, p)`: a retail game event, with its retail line and announcer sound | `announce` | the game-engine event record and its responses (`0x19de80`, `0x19e890`, `0x19e770`, hud-kit.md); how a host raises one with a player is R5. The first argument is the retail event's kind number until R5 names them | traced | the game engine event (type 7), on System Link too |

A sound is a tag the loaded map holds, checked like any tag path: a `sound`
is a string interned with the sound group (`TagGroup::Sound`), and the
start gate blocks a map whose tag index lacks that path by name
(`rules::labels::check`, `Census::sounds`). A census that does not read tag
paths (the running game's) checks none. A script brings no audio of its own.

The three `sound_to*` entries are spelled by name, not as one `sound`
overloaded on its audience as `message` is. A call through `none` plays
nothing, and a team with no seated players is an empty audience. The call is
queued and made at the next tick boundary; on the authority the boundary
sends the cue and plays the sound at the host's own players, and a machine
that is not the authority makes neither. The running game holds no tag paths, so
the host finds a sound by the loaded map's file: at each game start
`world::audio::activate` reads the file through the VFS (the file the
start gate's census reads) and installs a lookup from each `snd!` tag's
path to its datum, the datum the game loaded the tag under
(`world::audio::set_resolver`). A host with no lookup, or a loaded map that
holds no sound tag at a path the script plays, refuses the script at the
start, by the sound's path. A sound of a shared map (`shared.map`) is not
in the loaded map's file and is refused the same way. A host that runs a
film makes the same sounds.

A sound whose path starts `announcer/` is one of the announcer's clips
(`art/announcer/<clip>.wav`, [audio.md](../audio/audio.md#our-medals-announcer-lines)),
not a tag of the map. The start gate's census skips it
(`rules::labels::ANNOUNCER`), and each machine with a local player in the
audience, dead or alive, joins it to the announcer's queue
(`world::audio::say`), so it needs no sound lookup. A clip that is not one
of ours refuses the script at the start, by its path (`audio::refusal`).
The cue is the same sound cue, so a viewer and a film say it too.

A script that names `say`, `sound_at`, `music`, `music_stop` or `announce`
is refused at the start by the entry's name: R4 and R5 have not settled
what each does on the host. `sound_at` waits on R4: the body of
`sound_impulse_start` (`0x2a9e00`) is not read, so a positioned one-shot has
no traced call to make.

### HUD

Capabilities `hud.message` (exists), `hud.widgets` ("draws on the HUD")
and `hud.waypoints` ("marks places and things").

A script declares its HUD once, as the HUD kit describes one
([hud-kit.md](hud-kit.md#the-model-cratesrewriteshud-kit-core)): a fixed
list of widgets and waypoints, each a kind and a place. A state only
shows, hides and words them.

```text
hud hill_row: text at koth.hill
hud moved_row: banner at koth.banner
waypoint hill_mark at koth.mark
```

A place is a kit element's id: the HUD's name, a dot and the element
(`koth.hill`), so a script gives no pixels. A declaration's name is its
number, an `int` constant, which the calls below take. The kind is
`text`, `banner` or `meter`. At most 16 widgets and 8 waypoints; a place
taken twice is a compile error (`Refusal::HudSlot` in the verifier), and
a declaration without its capability is one too. The slot's HUD is the
one the game feeds from the state ([the HUDs a script feeds](#the-huds-a-script-feeds)).
The text's width against the element's is not checked.

A `section "Waves"` line splits a script: it compiles to nothing, the
outline nests the declarations after it under it (`ide::symbols`) and
editors fold it (`ide::fold`).

The state a widget keeps is its value, fill (0 to 255 for 0 to 1), style,
text (an index into the blob's strings) and audience, set by the calls
below; a waypoint keeps the object it marks; one widget keeps a value
for each roster slot (`hud_value_for`). `rules::show::view` makes
what one viewer sees of it, a pure function of the state, the blob and
the viewer's slot and team, and nothing in it reads the game. The state
is part of `RulesExt` (`show`), so a checkpoint, a film's keyframe and the
sim hash carry it, and a round's end clears it. A call that names a
widget or waypoint the script does not declare stops the script
(`ErrorKind::Hud`).

| Script | Entry | Engine | Status | Synced by |
|---|---|---|---|---|
| `message(to, "…{n}")` | exist | the notification insert (`hud_kit::NOTIFY`, E `0x24cbee`), per local user (**Code**, `crates/extensions/game/src/rules/world/show.rs`); the timed centred text (`0x24cc73`) is not used | named | a `Cue` (**Code**) |
| `hud_text(w, "…")`, `hud_value(w, n)`, `hud_fill(w, f)`, `hud_style(w, style)`, `hud_show(w, on)` | `hud_text`, `hud_value`, `hud_fill`, `hud_style`, `hud_show` | the VM's show state; `hud_kit::Handle::publish` draws it (**Code**, `crates/extensions/game/src/rules/world/hud.rs`). The text is a literal: message holes and an audience (`to`) are not built | named | `Show` (**Code**) |
| `hud_value_for(w, p, n)` | `hud_value_for` | the widget's value for the player in roster slot `p` (0 to 255), kept in the state's per-viewer table (`rules::show::PerViewer`: the widget's number and 16 bytes). `rules::show::view` gives each viewer the entry of its own slot in place of the widget's `value`; the fill, style and text stay shared. One widget at a time reads per viewer, and a second is a script error (`ErrorKind::Hud`); a `none` player sets nothing | named | `Show` (**Code**) |
| `side_of(p)`, `slot_of(p)` | `side_of`, `slot_of` | the roster: the team's index in a game with teams, 16 and the slot in one without; the slot | named | none: a read |
| `countdown(w, t)` | `hud_countdown` | the state holds the end tick; each machine counts its own ticks | not built | `Show` |
| `objective(to, text)`, `objective_mark(to, zone, kind)`, `meter(to, value, of)`, `countdown(to, ticks)` | `objective`, `objective_mark`, `meter`, `countdown`, now, for a `team` | the VM's per-team `Objective` (`RulesExt::objectives`: the line's string, at most `MAX_OBJECTIVE_MARKS` marked zones, the gauge, and the tick the clock ends at; `mark` is the waypoint's name, so a zone's mark is `objective_mark`); a fourth mark or an out-of-range kind is a script error (`ErrorKind::Hud`); the round's end takes it. The host adds no call; the objective HUD reads the record ([hud-kit.md](hud-kit.md#the-scripted-objective-hud)). A machine that runs no VM does not draw it | named | the state blob |
| `mark(wp, at)`, `mark(wp, at, to)`, `unmark(wp)` | `mark`, `mark_player`, `mark_team`, `unmark` | the object's place through `Handle::navpoint_at`, one navpoint a HUD; the text is the distance from the user's unit. The place is a zone's middle, else the unit of the player who carries the objective (the objective registry, `objectives::carrier`), else the object's origin (`Object::POSITION`, through `engine::view::objects::get`): `hud::place`. A mark's audience is every viewer, or `to`'s: a player's slot's bit or the bits of the team's seated players, resolved in the VM from the roster every machine holds (`Game::players_of_team`); `all` is the two-argument form, and the state carries the mask. The navpoint is the first local user's, as the kit's navpoints are shared. The text form is not built | named | `Show` (**Code**) |
| `medal(p, "…")`: a line with a medal glyph, counted in the film and on Live | `medal` | the notification queue, as the superbounce medals (hud-kit.md "The medals"); `GAME_MEDAL` (`0x1970a0`) takes only the game's own medal rows | traced | a `Cue` |

Scoreboard labels and the post-game report's columns stay the carrier's
(rules-engine.md "Not verified").

#### The HUDs a script feeds

The first word of a slot names a HUD of the kit's: `ctf`, `assault`,
`oddball`, `koth`, `territories` or `siege`
(`hud_kit_core::views::objective`, `content/modes/huds/`). The rules
feature registers them hidden when it installs
(`crates/extensions/game/src/rules/world/hud.rs`, `register`), and after
each tick, for each local user, the game makes the user's view of the
state, turns it into the objective views' `Fed` (`hud::fed`) and
publishes `objective::feed`'s values for each HUD the script's slots
name. A row the script leaves unset or hidden stays hidden. A side is a
team's index in a game with teams and 16 and a roster slot in one
without (`side_of`); a viewer is on a side (`Viewer::on_side`), so one
state reads "yours" to one team and "the enemy's" to the other. Each
HUD's contract with the script:

| HUD | Widget (slot) | Reads |
|---|---|---|
| `koth` | `koth.hill`; `koth.time`; `koth.banner`; waypoint `koth.mark` | style 0 open, 1 held by the side in the value, 2 contested; the seconds in the value; the banner's text; the navpoint over a zone, with the distance |
| `siege` | `siege.front`; `siege.clock`; `siege.side`; `siege.sites`, `siege.round`, `siege.phase`; `siege.banner`; waypoint `siege.mark` | style 0 open, 1 the side in the value on the front alone, 2 contested; the phase's seconds left in the value; the attacking side in the value (the viewer attacks when on it); each a count in the value of the total in the style; the banner's text; the navpoint over a zone, with the distance ([rules-engine.md](rules-engine.md#the-siege-hud)) |
| `territories` | `ter.owned`; `ter.banner`; waypoint `ter.mark` | the count in the value and the total in the style (the viewer's own, by `hud_value_for`: the territories its side holds, a team's in a game with teams and the player's own without); the banner's text; a navpoint over a zone |
| `oddball` | `odd.holder`; `odd.time`; `odd.banner` | style 0 loose, 1 carried, the value 16 and the carrier's slot (`slot_of`), worded with the player's name; the seconds in the value; the banner's text; the navpoint (`odd.mark`) over the ball a player holds, else the first one lying, with the distance |
| `ctf` | `ctf.flag0`, `ctf.flag1`; `ctf.banner` | each team's flag: style 0 at home, 1 carried (the value 16 and the carrier's slot), 2 dropped; the viewer's team's is "your flag"; the navpoint (`ctf.mark`) over the first flag away from its spawn |
| `assault` | `asl.bomb0`, `asl.bomb1`; `asl.banner` | each team's bomb as a flag, and style 3 armed with the seconds to detonation in the value; the viewer's team's is shown; the navpoint (`asl.mark`) over the first bomb away from its spawn |

A banner is timed by the kit from the moment its text changes
(`BANNER_TICKS`), so a script shows it for two ticks and hides it. The
`base` HUD is not fed from a script; its feed is slice GE5 of
[game-types-without-carriers.md](game-types-without-carriers.md).

### Physics

Halo 2's physics is Havok at an exact 30 Hz tick
([havok-runtime.md](../engine/havok-runtime.md),
[high-framerate.md](high-framerate.md)). Biped state reads on the host
require `hud.superbounce`.
Writes need `physics.push` ("pushes objects") or `player.push` ("pushes
players").

| Script | Entry | Engine | Status |
|---|---|---|---|
| `o.velocity`, `o.speed` | as "Objects" | the object datum | named |
| `p.airborne`, `p.crouching` | `airborne`, `crouching`, reads of a player | the biped's airborne bit and the unit's crouch request (including forced crouch), read by `superbounce_chain::read_players` | named |
| `on touchdown(p, down, up)`: a landing, with the fall speed before and the vertical speed after, `fixed` world units a second (`down` positive for a fall, `up` positive for rising), each rounded to the nearest `fixed` | `ir::Event::Touchdown`, `Input::Touchdown`, at most 16 a tick | the touchdown rule of `superbounce_charge::touchdown`, run for every player by the medal chain (`superbounce_chain::Physics::Touchdown`); none while a touchdown's launch window is open or under a strip's floor after a fall through it | named; the host delivers it while the superbounce mode runs; the fake lands the player |
| `on launch(p, spot, speed)`: a landing the host judged a superbounce, on the tick its launch window closes (three ticks after the touchdown). `spot` is the strip (`zone?`, the chain's `spot_id`; `none` for a seam no marked strip accounts for), `speed` the best vertical speed of the window, rounded **up** to the next `fixed`, so `speed > 16.0` holds exactly when the host's float comparison does | `ir::Event::Launch`, `Input::Launch`, at most 16 a tick | the chain's verdict (`superbounce_charge::judge` on a primed touchdown; `superbounce_chain::Physics::Launch`) | named; the host delivers it while the superbounce mode runs; the fake puts the player in the air |
| `on grounded(p)`: the player's run of bounces with no ground between them ended: a landing that did not bounce, the ground from the third tick after a bounce, a teleport, a new unit or a fall through a strip's floor. It follows the last `launch` of the run, before the next one | `ir::Event::Grounded`, `Input::Grounded`, at most 16 a tick | the chain's `live` going false (`superbounce_chain::Physics::Grounded`) | named; the host delivers it while the superbounce mode runs |
| `p.primed`: the player's level pair is in the state a landing bounces from | `primed`, a read of a player | the pair word of the biped's one pair against the level (`live_telemetry::havok_agents::fixed_pair`, **Code**) | named; its meaning is **Trace** in superbounce.md; the host answers from the chain's view of the biped (below): the level pair's word is the primed one |
| `sees(a, b)`: nothing solid between two objects | `sees`, a VM read of the sight table (`RulesExt::sight`) | `AI_LINE_OF_SIGHT` (`0x29dd50`) over the collision query `0x1697c0`, awaited in `rules::runtime::run_work` via `rules::world::physics::trace_sight` and `firefight::guest::line_of_sight` | named; the host answers at the next tick boundary |
| `touching(a, b)`, a contact's normal and impulse | reads | Havok's contact constraints (havok-runtime.md "Contacts"), read statically and never in a run | traced |
| `set_velocity(o, v)`, `push(o, v)`; `launch(p, v)` | `set_velocity`, `push`; `launch` | `OBJECT_SET_VELOCITY` (`0xb7740`), `quiet` clear | named; **S** for a client's own predicted biped |

**The physics events on the host.** The superbounce mode's medal chain
(`superbounce_chain::Chain::step`, every listed player, bots included)
says what it saw of each player's physics on each step
(`Chain::physics`, `Medals::physics`): a touchdown, a landing judged a
bounce with its launch, the end of a run. Each player gives at most one
of each a tick, so 16 of each event: their cap. The mode's tick observer
hands them to the runtime (`rules::feature::bounces`), which fires them
in the chain's order, slot by slot, before the tick's `on bounce` and
`on bounced` (`Runtime::physics`, `rules::world::physics::inputs`). The
mode runs only for a script that requires `hud.superbounce`, so a
script without it hears none of the three. A script keeps a chain on
them as the Rust chain does: a launch while the run is live adds one,
any other starts at 1, and `on grounded` ends the run. The Superbounce
script does exactly that (`content/modes/rules/superbounce.rules`), and
its scores and counts are checked against the Rust chain on every tick
of every chain scenario (`superbounce_chain_twin.rs`, run by
`superbounce_chain_tests.rs`).

**The biped reads on the host.** `p.airborne`, `p.crouching` and
`p.primed` read the medal chain's latest biped snapshot
(`superbounce_chain::PlayerTick`, `Medals::bodies`). The mode hands
the snapshot to the runtime on every step (`rules::feature::bounces`,
`Runtime::bodies`), and the game answers from it
(`rules::world::physics::read`). The reads return false until the first
snapshot and for players without a unit in it. `p.crouching` also
returns false for a depleted biped or a non-biped unit. The host
refuses a script that uses any of these reads without
`requires hud.superbounce` and names the read in its reason.

`sees` reads a **sight table** in `RulesExt` (`vm::Sight`, at most 32
ordered pairs, a 33rd is a script error, `ErrorKind::Sight`). The first
request returns false. The host traces each kept pair at the next tick
boundary, after queued engine calls and before that tick's handlers,
so the first answer has a one-tick delay. A labelled placement needs
a unique live object datum. The collision query uses the root objects'
positions and ignores those two root bodies. Missing endpoints or failed
queries return false. The fake traces at the end of `Vm::update`.
The VM drops a pair after 30 ticks without a request (`SIGHT_KEEP`). The table is in the state's bytes, so a
checkpoint, a film's keyframe and the sim hash carry it, and the round's
end clears it. Firefight's `zone_seen` and
`zone_distance` are this read and `distance`.

**Read-only against write.** A read costs nothing in sync: it is the
host's own state, rounded. A write is safe only where the engine already
replicates the result and keeps its own invariants, which is velocity
through its setter at a tick boundary: the solver integrates it next
tick like any impulse, and the velocity flag carries it. Writing
positions of bodies in contact, the pair word, contact points, per-body
gravity or collision filters is refused by having no entry: the Havok
heap is outside the checkpoint arena
([sim-rewrite-havok.md](../engine/sim-rewrite-havok.md#31-the-havok-heap-is-not-in-the-checkpoint-arena)),
a write there is not in a snapshot, and nothing replicates it. So the
API is one way in each direction: physics to script as rounded reads and
events, script to physics as a bounded velocity. A superbounce stays
observed, never made.

### Advanced data

Reads and one event, no capability. Slice W9.

| Script | Entry | From | Status |
|---|---|---|---|
| `o.health`, `o.shield` (`fixed`, 0 to 1) | `object_health`, `object_shield`, reads | the object's vitality fields (`guest-layout` `object.rs`), through `world::data::vitality` (**Code**, `crates/extensions/game/src/rules/world/data.rs`); an overshield reads 1, an object that is gone reads 0 | named |
| `o.tag`, `p.weapon` (`weapon?`) | none | a tag value is a table index of the script's own blob, and a map's tag is not in it; the held weapon needs the same lookup. No entry names either, so a script that writes them is a compile error | not built |
| `tag_int(t, "field")`, `tag_fixed(t, "field")` | `tag_int`, `tag_fixed`, reads | both arguments are constants written in the call (the verifier's `field_reads` holds each to the two `Const` ops before it). The field is a path of our own names (`vitals.maximum_shield`), resolved at load against the typed tag definitions (`tags::defs`) to an offset and a type by `world::data::DefsSchema`, and refused by name by `rules::fields::check` when they lack it. The game holds no tag paths, so `world::map_tags::activate` reads the loaded map's tag index through the VFS at `GAME_ACTIVATE` (**Code**, `crates/extensions/game/src/rules/world/map_tags.rs`; with `map_areas::activate`, from `EditorSources::activated`): each tag's path, group and datum. `Runtime::start` calls `rules::fields::check` after the label check, with `DefsSchema` over that index, and blocks the game on each problem (a tag the map lacks, a group or field the definitions lack, a float read by `tag_int`; one problem when the index could not be read). `Runtime::swap` and a film follow the label check's rules. The VM's read (`map_tags::read`) finds the tag's entry by its datum (`engine::tags::tag_entry_checked`, group and salt must match), reads the field's bytes at its defs offset from the entry's data address, and answers 0 for a tag the guest has not loaded | built; the rules half is also tested on `FakeGame` |
| `squad_alive(s)` | exists | `actor_net::squads::count_by_squad` | named |
| `squad_fighting(s)`, `squad_strength(s)` (`fixed`, 0 to 1) | reads | actor state beyond what the agent API's `world.ai` snapshot reads (`crates/extensions/agent/src/ai.rs`); no engine field is traced | untraced: the fake answers, a host refuses a script that reads either |
| `on actor_kill`, `on actor_assist`, `on squad_dead` | exist | `SimEvent::ActorKilled` (`crates/rewrites/sim-objects/src/events.rs`) | named |
| `on damage(o, by, amount)` | an event | `SimEvent::Damage`, the same stream: `o` the object, `by` the player it is credited to (`none` for none), `amount` the body and shield damage together as a fraction of full vitality. `world::data::damage_inputs` maps a tick's events to `Input::Damage`, the first 16 in the order the game applied them. `Runtime::post_tick` takes the tick's sim events (`sim_state::events`, filled by the object update in Rust), fires the damage inputs before the roster's changes and `on tick`, and drains them only for a script with an `on damage` handler | built; a tick with no events fires none |
| a map's own squads, by name: `label wardens: squad = "<squad name>"` | none | the scenario's squad block (`scnr+0x160`) is not read, and a kit squad slot is `squad(n)`; no research row settles the name lookup | not built; no label kind `squad` exists, so the declaration is a compile error |

Every machine in a game loads the same map, so a tag read is the same
on each. A float field is rounded once to `fixed`. A script names a
field by our schema's name; it sees no offset.
