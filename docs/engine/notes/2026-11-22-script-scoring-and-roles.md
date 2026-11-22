# Script games on a carrier engine: scoring, roles and objectives: evidence

Evidence for [rules-engine.md](../../game/rules-engine.md) ("The host
runtime", "One scorer") and the points `JUGGERNAUT_ROLE_SET`,
`ODDBALL_BALL_RETURN` and `OBJECTIVE_RETURN`. It follows
[2026-11-21-engine-vtables.md](2026-11-21-engine-vtables.md). Nothing
here states what the code does today; the design page does.

## 2026-11-22: what the carriers' scoring, role and objective routines do

Commit `1c331062f`; retail 1.0 `default.xbe`. Static reading only: `h2
disasm`, and a byte search of every section for `call rel32` and `jmp
rel32` to a target. No run.

**Reproduce.** Instructions: `h2 disasm <addr> <count>`. Callers: this
script prints every `e8`/`e9` whose target is one of the arguments:

```python
import struct, sys
d = open(sys.argv[1], 'rb').read()
base, = struct.unpack_from('<I', d, 0x104)
n, sh = struct.unpack_from('<II', d, 0x11c)
targets = [int(t, 16) for t in sys.argv[2:]]
for i in range(n):
    _, va, vs, ra, rs = struct.unpack_from('<5I', d, sh - base + i * 0x38)
    blob = d[ra:ra + rs]
    for off in range(len(blob) - 5):
        if blob[off] in (0xe8, 0xe9):
            rel, = struct.unpack_from('<i', blob, off + 1)
            if (va + off + 5 + rel) & 0xffffffff in targets:
                print(hex(va + off), hex((va + off + 5 + rel) & 0xffffffff))
```

`python3 -I callers.py disc/default.xbe 0x15b7c0` lists `SCORE_ADD`'s
callers.

### Every call of `SCORE_ADD` is a carrier's own scoring

The search finds ten calls of `0x15b7c0`, the ten the vtables note
assigns to engines, and no other:

| Site | Engine | In |
|---|---|---|
| `0x2bc05d`, `0x2bc077`, `0x2bc0f7` | Slayer | the kill callback `0x2bc000` |
| `0x19238c`, `0x1923db` | Juggernaut | the kill callback `0x1922a0` |
| `0x2bceb2` | Oddball | `0x2bce70`, from the per-tick slot |
| `0x2bf1e7`, `0x2bf2d2` | King | `0x2beee0`, from the per-tick slot |
| `0x2c0b32` | Territories | `0x2c0a90`, the per-tick slot's tail jump |
| `0x242e0e` | CTF, Assault | `0x242e00`, the capture |

Each is a 5-byte `call rel32`, so the return address `SCORE_ADD` finds at
`[esp]` is the site plus 5.

### Juggernaut's kill callback `0x1922a0`

At entry `[esp+4]` is the attacker and `[esp+8]` the victim (after
`sub esp,24h` and two pushes, `esi = [esp+34h]`, `ebx = [esp+30h]`);
`ebp` is the role mask's address `[0x510c9c]`. Nothing runs in a
distributed client (`[0x4e6948]+0xc` 4, `0x1922ae`). Three paths:

- **The Juggernaut is killed by another player** (attacker not the
  victim, the victim's bit set, `0x1922e6`): it posts a game engine event
  (`0x19eb90`, a record of type 8, subtype 2, the attacker and victim),
  clears the victim's role (`0x192970` with CL 0), gives the attacker the
  role (`0x192970` with CL 1) and posts event `0x26` (`0x1967d0`, ECX
  `0x26`, EAX the attacker). No score.
- **The Juggernaut kills** (the attacker's bit set, not a suicide): `SCORE_ADD`
  +1 for the attacker (`0x19238c`) and event `0x27`.
- **Otherwise**: with type-area flag bit 3 (`G+0x22c` bit 3) and neither
  player the Juggernaut (`0x192950`, the bit test), `SCORE_ADD` -1 for the
  attacker (`0x1923db`). Then, when the victim was the Juggernaut (a
  suicide or no attacker), its role is cleared and `0x192a30` draws a new
  one, excluding the victim.

`0x192950` (`player` on the stack, `ret 4`) is true when the player's bit
of the mask word is set.

### The role setter `0x192970` and the draw `0x192a30`

`0x192970` takes the salted player datum in EAX (its index the bit) and
the new state in CL. Setting posts an event record of type 8, subtype 1,
with the datum (`0xa7c50` then `0x19eb30`, skipped in a distributed
client) and ORs the bit into the mask word; clearing ANDs it out. Either
way it marks the engine-globals entity (`0xb58c0` with `E+0x24` and
`0x20`) and calls `0x192b20`, which rebuilds the mask's second word
(`+2`) as the teams (player `+0xc0`) that hold the role. It returns with
`ret`, popping nothing.

`0x192a30` (`player` to exclude on the stack) walks the players and
keeps those without the role, other than the excluded one, that pass its
checks of `0x15d770`, the lives left (`G+0x1b8`, player `+0x1ac`) and
player `+2` bits 0 and 1; it draws one
with the game's generator (`[0x4e7408]`, `* 0x19660d + 0x3c6ef35f`, the
high 16 bits modulo the count) and calls `0x192970` with CL 1.

### Juggernaut's per-tick slot `0x192530` keeps the role

Nothing in a distributed client. It walks the players: a Juggernaut
whose player record `+2` has bit 1, or bit 0 with `0x15d770` or
`PLAYER_OUT_OF_LIVES` true, loses its bit (which of these is a quit is
not read), the engine-globals
entity is marked and the teams' mask rebuilt; the others are counted.
When the mask changed it is written back and marked. When no Juggernaut
is left it calls `0x192a30(-1)`. It scores nothing.

### Juggernaut's trait query `0x1926c0`

Slot `+0x8c` of `0x453d38`. For a player whose bit is set it answers
trait 1 from `G+0x22c` bit 2, trait 2 from bit 4 and trait 3 from bit 6,
and false for any other trait; for any other player it calls the base
query `0x24dda0`, which answers trait 2 from `G+0x184` bit 12, trait 3
from bit 13 and trait 1 from bit 2 (`G+0x184` is record `+0x48`, the
variant flags). The three call sites of the slot (`0x15b28e`,
`0x2404ad`, `0x2bc967`) each ask trait 1, the active camo: `0x15b270`
gives the player's unit camo (`0x1520a0`) for `[0x510c54]+2` times a
constant when the answer is true.

### Oddball and CTF give their carrier traits themselves

Oddball's query `0x2bcc90` calls `0x19f3c0` with the player and 2 (the
objective of that kind the player holds, -1 none). For a carrier it
answers trait 2 when `G+0x232` is 0, trait 1 from `G+0x22c` bit 1 and
trait 3 from bit 2; otherwise it falls back to `0x24dda0`. CTF's falls
back at `0x242084`. So a ball or flag carrier's traits follow the object
it holds, not a role a script could set.

### Registering, returning and re-creating objectives

`0x15e050` (object, team on the stack, `ret 8`) registers an objective:
with an engine and fewer than 8 records it sets the object's `+0x16c` bit
6, writes the team to object `+0x17e`, marks the object's entity
(`0x1000`), appends the record (object, team, -1, -1), calls the engine's
slot `+0x54` with the object and, when the object is held (`+0x154`), the
pickup `0x15e250`. It does not look for the object among the records
already there. Its callers: `0xa228f`, `0xd0ab5`, CTF `0x2423d9`, Oddball
`0x2bc26c`, Territories `0x2c0a7d`.

`0x15e130` (object in EAX) unregisters: the drop `0x15e460` when the
record has a carrier, slot `+0x58`, the object's bit 6 and `+0x17e`
cleared and the entity marked, the record removed. `0x15e7b0` calls it
for an object of type 2 (`+0xaa`, a weapon) with bit 6 set; its
callers were not read.

`0x15e360` (object in ESI, a position pointer on the stack, `ret 4`) is
the objective return the engines share, called from Oddball `0x2bc361`
and CTF `0x242b41` and `0x242c7e`. With an engine: when the object's
`+0x16c` bit 7 is set it is dropped from the unit at `+0x154`
(`0xce520`); then `0xb75a0` moves it to the position with the default
forward and up vectors (`[0x4687a8]`, `[0x4687b0]`), `0xb73b0` runs on it,
and its `+0x150` and `+0x14c` become -1.

**Oddball.** `0x2bc340` (ball number in EAX, `ret`) returns ball `n`:
`0x15e410` finds the record whose team (`+4`) is `n`, `0x2bc1b0(n)` copies
the position of its spawn flag (`[0x51ecc4]+0xc+4n`, an index into the
netgame flags at `scnr+0x11c`, `0x20` each) and `0x15e360` moves the ball
there. `0x2bc1b0` answers false, writing nothing, when the spawn index is
-1, and `0x2bc340` does not check it; its one caller, the idle timer in
`0x2bc990` (`0x2bcaca`), posts an event first. The per-tick slot
`0x2bcbb0` walks the balls (`0` to `G+0x230`, the variant's ball count)
that have a spawn: a ball with a record is updated (`0x2bc990`), a ball
without one is created again (`0x2bc1f0`).

`0x2bc1f0` (ball number in EBX) creates a ball: the ball tag from the
globals tag's multiplayer block (`[0x4e034c]+0x16c`, then `+0xc`, `+0xc`),
the placement record initialised by `0xb7930` (ESI the record, `0xc4`
bytes), its position from `0x2bc1b0`, the object made by `0xb7b40`,
registered with `0x15e050(object, n)`, an event of type 4, subtype 1, and
the object enrolled for replication (`0xa7870`).

**CTF and Assault.** The flag and bomb respawn at `0x2424c6`..`0x2424f0`
unregisters (`0x15e130`) and deletes (`0xb8540`) each of the engine's
objectives and calls `0x2422d0(i)` for each team `i` (0 to 8) that has
none; `0x2422d0` is also called at `0x2402eb` and `0x24334a`. The flag
return that calls `0x15e360` at `0x242b41` takes its position from the
netgame flags (`scnr+0x11c`) and ends with `ret 4` at `0x242b5c`; where it
starts and what selects its flag were not read.

### What a script game can and cannot reach from these

- `role_traits` on Juggernaut is `0x192970`: the role, its traits (the
  slot reads the mask) and their replication come from the engine's own
  path. On the other carriers there is no role a script can set:
  Oddball's and CTF's traits follow the held object, and Slayer, King and
  Territories have no role traits.
- `reset_objective` on Oddball is `0x2bc340`, with the record's team as
  the ball number, after checking the ball has a spawn flag.
- `spawn_objective(tag, at, team)` needs a tag datum for the script's tag
  path and a position for `at`. Nothing in guest memory carries the
  loaded map's tag names (the engine takes its objective tags from the
  globals tag); the names are in the map file (`hot_tags::Names`), which
  the rules runtime does not read. A script on the host has no object to
  name as `at` but the registry's own: Forge labels are not built.
- `register_objective(o, team)` is `0x15e050`, which does not check for
  duplicates; the only objects a script on the host can name are already
  registered.
