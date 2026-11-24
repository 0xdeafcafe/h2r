# Spawn locations for scripts: evidence

Evidence for [rules-engine.md](../rules-engine.md) ("The engine API",
"Phases: attack and defend"). Nothing here states what the code does
today; the design page does.

## 2026-11-24: the spawn chooser, read from the retail image

Commit `92bde37b3`; retail 1.0 `default.xbe` through `$H2_DISC`. Static
reading only, with `h2 disasm`. No run.

**Reproduce.**

```sh
h2 disasm "$H2_DISC" 0x14ef00 100   # PLAYER_SPAWN_LOCATION, the chooser
h2 disasm "$H2_DISC" 0x23ba90 60    # a location's weight
h2 disasm "$H2_DISC" 0x23bb70 40    # the free-spot test
h2 disasm "$H2_DISC" 0x23bc40 20    # the weighing context
h2 disasm "$H2_DISC" 0x14f2e0 110   # PLAYER_SPAWN
# callers: one linear listing of .text, then grep
h2 disasm "$H2_DISC" 0x12000 1100000 > text.asm
grep -n 'call 0023BB70h' text.asm
grep -n 'call 0023BA90h' text.asm
grep -n 'call 0014EF00h' text.asm
```

### The chooser `0x14ef00`

ECX the player handle (kept in EDI from `0x14ef26` to the end), three
stack arguments, `ret 0Ch`: the unit type the player spawns as, a flag
the weight routine reads, and a flag that skips the count below.

| Address | What it does |
|---|---|
| `0x14ef0a`..`0x14ef68` | the player datum (`PLAYERS` `+0x44`, `0x21c` each) into EBX; a seed from the player index, the player's spawn count (`+0x218`) and two words of the game options (`[0x4e6948]+0x10`, `+0x14`); the best index -1 and the best weight from `[0x45df28]` |
| `0x14ef6c` | `0x23bc40`: the weighing context, a `0x1000`-byte buffer on the stack; it ends in the engine's vtable slot `+0x80` ([engine-vtables](../../engine/notes/2026-11-21-engine-vtables.md)) |
| `0x14ef90`..`0x14efb1` | each entry `i` of `scnr+0x100` (count `[scnr+0x100]`, base `[scnr+0x104]`, stride `0x34`): `0x23ba90` with EAX the entry, stack the player, the first argument and the context; the weight in XMM0 |
| `0x14efb6`..`0x14efc1` | `comiss` against the best: not heavier skips the entry |
| `0x14efc3`..`0x14efd5` | heavier: `0x23bb70` with ECX the entry and EAX the unit type; AL 0 skips the entry |
| `0x14efd7`..`0x14efe5` | AL 1: the entry becomes the best |
| `0x14f001`..`0x14f019` | with the third argument 0 and a best found, the player's spawn count `+0x218` goes up |
| `0x14f02d`, `0x14f032` | AX the best index, `0xffff` for none |

So the chooser keeps the heaviest entry that passes the free-spot test.
An entry the test refuses leaves the best as it was, and the loop goes
on to the next entry, so a lighter entry that passes can still win. No
entry's weight depends on another's test.

**The weight `0x23ba90`** copies the entry's position (`+0x00`, three
`f32`), weighs it with `0x23b060`, and with an engine present adds a
random term: from the context's seed when the second stack argument's low
byte is set, from the game's generator (`[0x4e7408]+4`) otherwise. Every
entry is weighed, refused or not, so the draws do not depend on the test.

**The free-spot test `0x23bb70`**: EAX -1 passes at once (`0x23bb73`).
Otherwise the unit type's definition gives a capsule (`+0x28c`, `+0x290`
or `+0x298`), and `0x1c5210` tests it at the entry's position; something
in it answers AL 0. A plain `ret`: no stack argument.

**Callers.** `0x23bb70` has one call, `0x14efce`, inside the chooser.
`0x23ba90` has one, `0x14efb1`. `0x14ef00` has two: `PLAYER_SPAWN`
(`0x14f391`, arguments unit type, 1, 0) and `0x14f0f0` (`0x14f143`,
arguments unit type, 0, 1), which the players update `0x14d3a0` calls at
`0x14d4b0` for a player waiting to spawn; it hands the location to
`0x11bed0` and counts nothing.

**What the entry's other fields do.** Nothing in the chooser reads the
entry's team (`+0x10`) or its game-type slots; the weight reads only the
position ([scenario-editor.md](../../editor/scenario-editor.md),
"Player starting locations"). A retail map's starting locations are not
split by team in the engine's pick.

### What a script's spawn rule needs

- **The point.** Answering `0x23bb70` with AL 0 for an entry outside a
  team's rule leaves the chooser's own pick among the rest: the same
  weights, the same draws, the same tie order. The test is called only
  for an entry heavier than the best so far, so the answer costs a
  compare per such entry. ECX is the entry, so its index is
  `(ECX - [scnr+0x104]) / 0x34`.
- **The player.** The test has no player argument; the chooser's ECX is
  read at its entry instead. Both callers of the chooser are for one
  player, and the test has no other caller.
- **Every entry refused.** The chooser answers `0xffff`. `PLAYER_SPAWN`
  then fails (`0x14f398`), and the players update retries the spawn the
  next tick, as on a map whose spots are all stood on
  ([live-coop.md](../../live/live-coop.md) section 12, "location pick").
- **Not traced.** How `0x23b060` weighs a position (the influence of
  living players), and what the engine's slot `+0x80` adds to the
  context. Neither is needed to restrict the pick.

### What the live probe must show

On a Slayer game of Siege with a flag stand per team (a CTF map), on the
host, through `h2 agent`:

1. Each attacker's first spawn is at one of the four starting locations
   nearest the attackers' flag stand, and each defender's at one of the
   four nearest the first site.
2. After the attackers take a site, their next wave spawns at the four
   nearest that site.
3. With all four of a side's locations stood on, that side's dead wait
   and spawn the tick one is free, and nobody spawns elsewhere.
4. A client's view of those spawns matches the host's (the spawn is the
   host's; clients get the unit).
