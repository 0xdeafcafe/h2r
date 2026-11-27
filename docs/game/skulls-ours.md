# Skulls: our skulls

The skulls we add, and what is not verified. Part of [skulls.md](skulls.md).

## Our skulls

Status: built, tests only; not seen running. Every effect and the hidden
skulls want the in-game check at the end of this page.

Six skulls of our own, indices 15 to 20 after the game's fifteen. Retail's
array at `0x4f55dc` is fifteen bytes and `0x1df3e0` takes only `n < 15`,
so ours have no byte there and the game never runs them: each is our code
on the feature plan (`boot::skulls_custom`, feature `skulls-custom`). The
rest is shared with retail's skulls: the same per-profile `skulls.*`
values (`skulls.acrophobia`, `skulls.low_gravity`, `skulls.fall_damage`,
`skulls.unlimited_ammo`, `skulls.unlimited_grenades`,
`skulls.invincible`), locked on the Skulls page until collected (listed
after retail's fifteen, gold first again), turned on at a campaign start
with the profile's others (`skulls::custom_selection`, the same
`OPTIONS_QUEUE` hook), and carried in telemetry `NewGame.skulls` with the
retail ones (`live_proto::skulls::OURS`). The web's name table has them.

| n | Skull | Tier | Level | What it does |
|---|---|---|---|---|
| 15 | Acrophobia | silver | Uprising | flight: in the air, look up to climb, level out to hover, crouch to drop |
| 16 | Low Gravity | silver | Delta Halo | gravity halved for everything; offered in multiplayer |
| 17 | Fall Damage | gold | Metropolis | a landing faster than a jump's drains shield, then health |
| 18 | Unlimited Ammo | silver | Outskirts | spare ammo never runs out; reloads still happen |
| 19 | Unlimited Grenades | silver | Sacred Icon | both grenade types kept at four |
| 20 | Invincible | silver | The Great Journey | the player takes no damage |

Each is found on any difficulty (the description says so).

### How each works

All but Low Gravity act on the **local players' units** (players with a
local user, `+0x28 >= 0`, on a live unit), after every `game_tick`, in
campaign only (options mode `[[0x4e6948]+8] == 1`).

- **Acrophobia** (likely). Simplest sound way: no new input, no physics
  change, one float. While the biped is airborne (biped `+0x3dc` physics
  mode 2, likely: 1 on the ground, 2 in the air) and attached to nothing
  (`+0x14` is none) and not crouching (`+0x2c4` below 0.5), its vertical
  speed (`+0x88` velocity, z at `+0x90`) is raised to `aim_z * 4.0` world
  units a second, `aim_z` being the unit's aim vector's z (`+0x168`,
  likely). Looking up climbs, looking level hovers, looking down sinks
  slowly; crouching drops out; a jump's own speed is never lowered. Get
  airborne with a jump. The jump button held was not traced (the control
  latch at `[0x4ed284]+4` is the action test, not the held state), which
  is why it is the view and not the button. Since traced for the `fly`
  cheat (docs/runtime/debug-menu.md): the unit's control flags at
  `+0x148` hold it, bit 1 jump and bit 0 crouch, so the skull could move
  to the cheat's controls; it has not been changed. If the physics rewrites the
  velocity before it is used, the effect is weaker than meant: the in-game
  check says.
- **Low Gravity** (confirmed address, likely effect). The physics
  constants' gravity is the float at `[0x51e9c4] + 0`: the constants'
  set-up (`0x1eb584..0x1eb5b9`) writes the standard gravity
  (`[0x4455d8]`) there and publishes the pointer; builtin 127
  `physics_set_gravity` (`0x2a2b30`) writes `arg * [0x4455d8]`, and the
  reset builtin `0x2a2ad0` writes `[0x4455d8]`; 31 readers in physics and
  AI (e.g. the AI fall check at `0xe32af`) multiply by it. After each tick
  we write half of what the game last put there; when the game writes a
  new value (a map, a script) we halve that; when the skull is off we put
  the game's value back. Retail never reads `[0x4455d8]` again for gravity,
  so nothing undoes ours.
- **Fall Damage** (ours, likely offsets). Landing is the biped going from
  airborne to not between two ticks; the impact speed is the vertical speed
  of the tick before. Past 3.0 wu/s (a jump lands at about 2.5), each
  wu/s more takes 0.3 vitality: shield (`+0xf0`) first, then health
  (`+0xec`), the fractions `object_get_shield` (`0x2a1a90`) and
  `object_get_health` (`0x2a1a20`) read. A drained body is killed as
  builtin `unit_kill` (`0x2a3320`) does: `+0x10a |= 0x20`, then a call to
  `0xb7360` with EAX the unit. Retail's own falling damage was not found
  (the globals' damage block at `[0x4e034c] + 0x144` has the kill and
  fall-below-the-map damage, not landings), so this is ours, not a
  scaled retail value. Off while Invincible is on.
- **Unlimited Ammo** (likely). For each weapon the unit holds (`+0x218`,
  four datums), each magazine's reserve (`W+0x22a`, magazines at `W+0x228`,
  `0x10` each, at most two) is set to its definition's maximum (the `weap`
  magazine block, count `+0x2c0`, pointer `+0x2c4`, `0x5c` each; the
  maximum at `+0x0c`, the field `0x101a10` caps the reserve with). The
  loaded rounds (`W+0x22c`) are not touched, so the magazine empties and
  the weapon reloads. A weapon with no magazines (plasma) has its charge
  kept full (`W+0x184`, the used fraction `0x101a10` writes, set to 0).
  **Not Bandana**: retail's Bandana (14) is gated in actor code
  (`0x25cc4d`) and, by the community's account, makes the magazine itself
  bottomless (no reloads) and grenades too; ours keeps reloads, does
  nothing for grenades, and acts on the local players only.
- **Unlimited Grenades** (confirmed offsets). The unit's two grenade counts
  (`+0x23e`, signed bytes, fragmentation and plasma; `+0x23c` the selected
  type) are kept at four or more. Four is the most the campaign carries
  (community).
- **Invincible** (confirmed). The bit builtin 90 `object_cannot_take_damage`
  (`0x2a20c0`, through `0xd87e0`) sets: body `+0x10a |= 0x80`, kept set on
  the local players' units every tick (builtin 91 clears it; a script
  that does is overridden the next tick). Turning the skull off leaves the
  bit until the next unit (a death, a level): we do not know whether a
  script set it too.

### Hidden collectables

One per level, from each level's real geometry (`map.glb`, the BSPs and
the scenario, read off the disc with this repo's tools; renders kept
outside the repo). On a campaign level with a spot, once its BSP is the
active one (`[[0x4e6948]+2]`), and while any signed-in profile has not
collected that skull, we create the retail skull weapon there:
`objects\weapons\multiplayer\ball\head_sp` (`head` on High Charity), its
tag datum read from the mounted source for the active cache slot and
checked against the guest's scenario datum. If the slot has no known
source, the locator looks up the loaded map name. This includes packages
and editor play copies (`skulls_custom::Custom::skull_tag_for`).
Creation is the game's own: `placement_data_new` (`0xb7930`, ESI a
`0xc4`-byte placement, stdcall definition, owner -1, 0) then position at
placement `+0x1c` (as the netgame equipment spawn at `0x15ab9d` copies it)
and `object_new` (`0xb7b40`, stdcall placement, the datum in EAX). It has
the retail skull's colour: the placement's colour fields were not traced,
so there is no tint.

Pickup is a local player's unit holding it: the skull's datum in one of
the unit's weapon slots or the unit as its parent (`held_by`, the pure
function the tests run). Then: recorded and on for every signed-in
profile (`skulls::record_pickup`, as for retail's), on for the rest of
this level, our HUD message "Skull found: <name>" through the game's own
(`0x24cbbf`, EDI our string id, stdcall local user; what `0x1df3e0`
calls), a loud stderr line, and the object removed as `object_destroy`
(builtin, `0x2a14d0`) does: `0xbeb30` (refuses with AL non-zero), then
`object_delete` (`0xb8540`). A checkpoint revert or a BSP change that
takes the object away places it again.

### Calls into the game

Calls into the game run at one point: a `before` on `game_tick`, first in
its chain, that, when calls are queued, answers with an async body that
makes them and then runs the tick (whose chain runs again and continues),
as `coop_probe`'s lobby setup does.

Where, in words:

- **Outskirts** (Unlimited Ammo): in the shade beneath the far end of the
  pier.
- **Metropolis** (Fall Damage): at the very end of the gantry, where the
  bridge stops short.
- **Delta Halo** (Low Gravity): behind the falling water above the gorge
  bridge.
- **Sacred Icon** (Unlimited Grenades): up in the corner bay of the great
  octagonal chamber.
- **Uprising** (Acrophobia): among the supply crates on the raised platform
  past the first cliff descent.
- **The Great Journey** (Invincible): at the end of the gully under the
  overhang, just north of where you begin.

<details>
<summary>Exact coordinates (spoilers)</summary>

Game units, z up, the floor plus 0.15 (`boot::skulls_custom::SPOTS`).

| Map | Skull | BSP | x | y | z | Where |
|---|---|---|---|---|---|---|
| `03a_oldmombasa` | Unlimited Ammo | 1 | -452.00 | -468.00 | -7.12 | on the beach, under the deck at the seaward end of the long pier (y -460), among the pilings |
| `03b_newmombasa` | Fall Damage | 1 | 452.50 | 71.80 | -5.45 | the east end of the northern covered bridge-tube off the walkway at x 432, 12 above the ground |
| `05a_deltaapproach` | Low Gravity | 0 | -168.00 | -302.50 | 25.42 | a ledge behind the tall waterfall south of the Forerunner bridge, reached from the ruins path |
| `06a_sentinelwalls` | Unlimited Grenades | 0 | -390.80 | 134.20 | 4.35 | the south-west bay of the raised ring in the first octagonal room (centre -387, 145) |
| `08a_deltacliffs` | Acrophobia | 1 | 134.40 | 117.60 | 68.65 | a roofed platform between Covenant supply crates, stepped up to from the east |
| `08b_deltacontrol` | Invincible | 0 | 167.00 | 22.80 | 12.55 | the tip of the dead-end gully north-west of the start (175.6, 10.0) |

How they were picked: each floor is a flat render triangle (normal z
above 0.95) with headroom, reachable on foot in a flood fill over the
level's floors from its placements and start, clear of placed weapons, and
far from the retail skull. The render geometry is not collision: crates,
scenery, kill volumes and water are not all in it, so the in-game check
confirms each. Known soft spots: Outskirts' sand is a little below the
river's water plane (the river does not cover it); Sacred Icon's bay is on
the main path; Uprising's crates are physics objects; The Great Journey's
gully tip slopes, so the skull may roll a little.

</details>

`H2_SKULLS_REVEAL=1` logs every spot at boot and, on a level with one,
its coordinates, state and each local player's distance every 150 ticks
(five seconds), on stderr only.

### In multiplayer

Only Low Gravity is offered (`skulls_mp::MP`,
`live_proto::skulls::MULTIPLAYER` = `[4, 6, 16]`): gravity is one constant
every machine simulates with, so the host's set, applied on every machine
at game start the way Famine and Blind are, keeps them agreeing. The rest
act on the local players only; in a game the host simulates, a guest's
invincibility or ammo would be undone or disagree, so they are campaign
only.

### Switching it off

`H2_FEATURES=skulls-custom=off`. When
the plan cannot place its tick hooks (another feature holding an address
it needs), boot says `skulls-custom: NOT installed: <why>` and nothing of
ours is placed or applied; retail's skulls are untouched either way.

### What it logs

- boot: `skulls-custom: hooks installed via feature plan at 0x... (tick:
  calls, effects, hidden skulls); hidden: Acrophobia on Uprising, ...`,
  or `skulls-custom: NOT installed: <why>`, or `skulls-custom: off: <why>`
- campaign start: `skulls: campaign start for <profiles>: <all on, ours
  included>` and `skulls: ours on this campaign: <ours>`
- level load: `skulls: <map> hides <skull> (bsp n): not yet collected by
  everyone here; it will be placed` (or `... already collected by every
  profile here; not placed`, or `... cannot be placed: <why>`)
- `skulls: placed <skull> at (x, y, z) as object 0x...` (or `skulls:
  object_new refused <skull>; not placed this level`)
- pickup: `skulls: ==== FOUND <skull> on <map> by player 0 (object 0x...);
  on for the rest of this level ====`, the `skulls: <skull> kept for
  <profile>, saved to <path>` lines, `skulls: removed the found skull
  (object 0x...)`
- effects, once a game each: `skulls: Low Gravity: gravity <g> -> <g/2>
  (physics constants 0x...)`, `skulls: <skull> is acting on the local
  players this game`; each hard landing: `skulls: Fall Damage: player 0
  landed at 8.00 wu/s: 1.50 vitality (shield 1.00 -> 0.00, health 1.00 ->
  0.50)`
- with `H2_SKULLS_REVEAL=1`: `skulls reveal: <skull> on <map> (bsp n) at
  (x, y, z) [hint]: <placed|collected|waiting for bsp n ...>; player 0
  12.3 wu away`

## Not verified

Only a game run shows these.

- The campaign flow with a named profile: a pickup is kept (`[profiles."<profile>".skulls]`
  in `settings.toml`), the next campaign start turns the kept skulls on
  from the first encounter, a skull set off stays off, co-op with two named
  profiles keeps a pickup for both, and the `default` profile and `H2_LEVEL`
  fall back as the "Who it is for" bullet says.
- The Skulls page: a profile with no skull collected starts as retail with
  no page; a profile with one gets the page after the difficulty; Start
  Level loads the level on the chosen difficulty from the level select,
  New Campaign and a lobby's Game Setup; B returns to the difficulty list;
  the list is full width less the picture, scrolls past 13 rows, and the
  picture and description fade with the page. Whether co-op reaching
  `difficulty_lobby` (screen 1) needs a page.
- The model picture: that the skull weapon's model loads from the disc
  (the log line names the map), faces +x with its forehead where the
  decal goes, and looks right lit and tinted; that the picture widget
  reaches the sorted widget call and is replaced there; that `+0x64` is
  the widget's fading alpha; that the UI's 640 by 480 units map to the
  target as `guest_rect` assumes on a wide target; and that both copies
  of a scaled target draw it.
- The multiplayer Skulls row in the variant editor: its pane, B back to the
  editor, and the screen on a lobby's own stack (window 3).
- Multiplayer: the host's Low Gravity is applied on every machine and a
  guest's own profile does not matter; Famine and Blind apply for the
  local players only.
- Our skulls: each hidden skull is found at its place (the `H2_SKULLS_REVEAL`
  lines name the spots), is removed on pickup, is kept per profile and
  placed again until every profile here has it; Low Gravity, Fall Damage,
  Unlimited Ammo and the others act as "How each works" says. The jump
  button held in air is not traced.
- `H2_FEATURES=skulls-screen=off`, `skulls-page=off`, `skulls-mp=off` and
  `skulls-custom=off` leave the matching parts as retail, with the log
  lines "What it logs" names.
