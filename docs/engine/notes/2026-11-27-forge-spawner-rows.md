# Forge spawner rows: spare clips, place at start and weight

Evidence for the three Properties rows the Forge design gives item
spawners and starting points with no retail field
([forge-map-variants.md](../../game/forge-map-variants.md), "Properties
without a retail field"; [forge-map-variants-ui.md](../../game/forge-map-variants-ui.md),
"The Properties page"). It builds on
[2026-11-26-forge-loose-ends.md](../../game/notes/2026-11-26-forge-loose-ends.md)
and [2026-11-23-item-game-types.md](../../game/notes/2026-11-23-item-game-types.md).
The design pages state what the code does; this note is the reading.

## 2026-11-27: read from the retail image

Commit `30e4fb3db` (branch `forge-engine`); retail 1.0 `default.xbe`
through `$H2_DISC`. Static reading with `h2 disasm`; no run.

**Reproduce.**

```sh
h2 disasm "$H2_DISC" 0x15ae70 260   # the per-tick netgame item update
h2 disasm "$H2_DISC" 0x15ac50 150   # respawn time, the state init 0x15ace0, the reset 0x15ad30
h2 disasm "$H2_DISC" 0x15c150 40    # the update's caller
h2 disasm "$H2_DISC" 0xd0b70 40     # a unit's weapon: loaded and reserve set, capped
h2 disasm "$H2_DISC" 0x101760 80    # ammo taken from a weapon of the same definition
h2 disasm "$H2_DISC" 0x14ef90 40    # the spawn chooser's loop
h2 disasm "$H2_DISC" 0x23ba90 60    # a starting location's weight
```

### The item state the spawner keeps

The update `0x15ae70` takes one argument, the state array
`[0x4e9ae8]+0x7dc` (pushed at `0x15c188`), and walks `scnr+0x120`
(stride `0x90`). Each entry has 8 bytes of state: the object handle
(`+0`), a countdown (`+4`, i16) and the abandon count (`+6`, i16). An
entry is updated on one tick in `[*GAME_TIME]+2` (`0x15aef2`..`0x15af07`),
so the countdown counts those updates. The array is 200 dwords, 100
entries (`0x15acf4`, `mov ecx,0C8h; rep stosd`), the block's limit. The
update returns at once in a distributed client (`[*GAME_GLOBALS]+0xc` is
4, `0x15ae7b`), so only the authority spawns.

At `0x15b137`..`0x15b1aa` a running countdown falls by one, and at zero
the update calls the spawner `0x15aad0` with the array and the entry's
index (`push esi; push ecx; call`, `ret 8`). The spawner stores the new
object's handle at the entry's `+0` and the index at the object's `+0xaf`
([forge.md](../../game/forge.md) 1.2). Nothing between that return and the
next entry writes the object.

### Spare clips (proven)

- **The reserve is what a player takes.** `0x101760` moves rounds from a
  weapon object into another of the same definition (`[edi] == [ebp]`,
  `0x101785`): for each magazine (`weap+0x2c0` count, `+0x2c4` elements,
  stride `0x5c`) it takes up to the element's `+0x0c` cap from the other
  weapon's reserve `+0x22a` and loaded `+0x22c` (`0x1017da`..`0x10183f`).
  Picking up the weapon itself carries the object, reserve and all.
- **The cap.** `0xd0b70` sets a unit's weapon's loaded rounds to at most
  the element's `+0x0a` and its reserve to at most `+0x0c`
  (`0xd0bd5`..`0xd0c00`), as `0x101a10` does
  (2026-11-26-forge-loose-ends.md). A clip is `+0x0a` rounds.
- **The write.** After `0x15aad0` returns, the entry's `+0` is the new
  weapon. Setting its first magazine's reserve to
  `min(clips * clip, cap)` is a write to a field the engine itself writes
  and reads, at a moment no engine code writes it again before a player
  can take it. In a Forge session the weapon the entry has out is written
  at once too.

Built: `forge_doc::Property::SpareClips`, `Session::set_spare_clips`,
`World::spare_clips`, `agent::forge::spare_clips`, and the game's
`forge_spare_clips` feature, an observer after `NETGAME_ITEM_SPAWN`
(`0x15aad0`).

### Place at start (write proven, not built)

- `0x15ace0`, the state's set-up, zeroes the array and sets every handle
  to `-1`. With no object and no countdown, the update arms the countdown
  at once (`0x15b03f`, `0x15b053` to `0x15b0e8`): the respawn time
  (`0x15ac50`) scaled by the variant (`0x15b0ef`..`0x15b134`). So an entry
  left in that state spawns after its respawn time.
- The reset `0x15ad30` (from the round reset `0x157ae0`, `ENGINE_ROUND_RESET`)
  instead deletes each object and sets each countdown to 1 and abandon
  count to 0 (`0x15ad86`, `0x15ad8d`), so every entry spawns on its first
  update.
- "Place at start: No" is therefore one write per entry after
  `ENGINE_ROUND_RESET`: a countdown the reset left at 1 with no object
  set to 0, so the update arms the respawn time itself, scaled as for any
  respawn.

**Why there is no row.** The write takes effect only at a round's start.
A Forge session's document is loaded at the session's first tick, after
`GAME_ACTIVATE` and its round reset, and a match on a built variant has
no document at all (the same is true of a spawner's team,
`rules/census.rs`). No path carries a document property to the round
reset of the game that plays it, so the row would do nothing in play.

**What it would take.** The property has to be in the built map, read
before the first `ENGINE_ROUND_RESET`:

1. **The record.** A per-game Forge block in the map's tail, in the
   manner of the areas record (`game_markers::record`, read at activation
   by `map_areas::activate`, before the rules runtime's start): rows of
   (block, index, print, property), written by `forge_build` from the
   document's properties that reach play (Place at start; Spare clips and
   Weight too, so a match on a built variant keeps them). The materialiser
   knows each entry's final index (`forge_doc::resolve`).
2. **Reading it.** `EditorSources::activated` reads the record with the
   areas and fills the game's tables (`spare_clips::TABLE`,
   `spawn_weight::TABLE`, and a Place at start set) before the round
   reset runs.
3. **The write.** An observer after `ENGINE_ROUND_RESET` sets the
   countdown (`[GAME_ENGINE]+0x7dc + 8 * index + 4`) of each entry with
   Place at start No from 1 to 0. A later round's reset runs it again.
   In a Forge session the same write runs when the property changes,
   for the next round.
4. **Peers.** Only the authority spawns (`0x15ae7b`), so only the host
   needs the table; a member builds the same package and holds it unused.

### Weight (built)

- **The chooser.** `PLAYER_SPAWN_LOCATION` (`0x14ef00`) starts with the
  best weight `[0x45df28]` = -1000.0, weighs every starting location
  (`0x14ef90`..`0x14efff`) and keeps the heaviest that passes the free-spot
  test (`0x14efb6` `comiss`, then `0x23bb70`). `0x23ba90`'s one caller is
  `0x14efb1`; EAX the location, the weight back in XMM0 (`ret 0Ch`).
- **The influence.** The context (`0x23bc40`, `[esi]` a count up to `0x80`)
  is filled by `0x23b170`, `0x23b500`, `0x23b8e0` and the engine's slot
  `+0x80` (Oddball `0x2bcd50` and KotH `0x2bdb00` add spheres of kinds 5, 6
  and 7, 8; the default `0x24dc50` adds none). Each sphere comes from
  `0x23ba10`, whose weights are the table `0x502260` (`0xc` a kind: inner
  radius, outer radius, weight) and the heights `0x502258`, `0x50225c`.
  `0x23aea0` fills the table from the multiplayer globals (`mulg`'s
  universal element, `+0x184`..: four rows of `0xa8`, three kinds each,
  and the scenario's overrides at `scnr+0x318`). On lockout the kinds'
  weights are -0.75, 0.1, -0.75, 0.05, 0, 0.05 (row 0) and -1.0, 0.05, -1.0,
  0.5, -0.75, 0.25 (row 1), the heights 3.0: an enemy weighs -0.75 to -1,
  everything else 0.05 to 0.5 (read with `tag.read` on
  `mulg:multiplayer\multiplayer_globals`).
- **The random term.** `0x23bb04`..`0x23bb54`: the seed in ESI (from the
  player index, spawn count and options, `0x14ef50`) stepped
  `x * 0x19660d + 0x3c6ef35f`, its top 16 bits times `[0x45dc50]`
  (1/65535), times the first float of `mulg`'s universal element's block
  `+0x530` (`[[*GLOBALS_TAG]+0x16c]` tag `+0x0c`, `+0x534`), which is 1.0
  on the disc. So the term is uniform over `[0, 1]`.
- **The bias.** Low and High add -0.5 and +0.5 after `0x23ba90` returns
  (`agent::forge::spawn_weight::BIAS`). Against a location of equal
  influence, High wins when its draw plus 0.5 beats the other's draw:
  seven times in eight. It is smaller than one enemy sphere (0.75 to 1),
  so a High location by an enemy still loses to a clear one, and much
  larger than the non-enemy spheres, so it outweighs a teammate's pull.
  The effect is static evidence; a run that counts spawns was not made.

Built: `forge_doc::Property::Weight` (`SpawnWeight::{Low, High}`),
`Session::set_spawn_weight`, `World::spawn_weight`,
`agent::forge::spawn_weight`, and the game's `forge_spawn_weight` feature
(an observer before `SPAWN_LOCATION_WEIGHT` keeps EAX, a hook after adds
the bias to XMM0).

### Weight: the earlier reading


- The spawn chooser (`PLAYER_SPAWN_LOCATION`, `0x14ef00`) loops over the
  starting locations (`0x14ef90`..`0x14efff`, the index at `[esp+14h]`).
  Its only call of `0x23ba90` is at `0x14efb1`, with EAX the location
  (`scnr+0x104` + `0x34` × index), three stack arguments and `ret 0Ch`;
  the weight comes back in XMM0 and is compared with the best so far at
  `0x14efb6`.
- `0x23ba90` is the influence sum `0x23b060` plus a random term: the
  game seed stepped `x * 0x19660d + 0x3c6ef35f`, its top 16 bits, times
  the float at `0x45dc50`, times a float of the globals tag
  (`[[*GLOBALS_TAG]+0x16c]`'s tag `+0x0c`, `+0x534`), `0x23bb04`..`0x23bb54`.
- An after-hook on `0x23ba90` could add a bias to XMM0. What a Low or
  High bias must be is not known: the influence elements come from the
  engine's slot `+0x80` and what they hold for each game type was not
  read, nor the globals float that scales the random term. A bias of the
  wrong size either does nothing or picks one location every time.

The influence weights and the scale were read the same day (above).
