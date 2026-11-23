# Round start and player quit for script games: evidence

Evidence for [rules-engine.md](../../game/rules-engine.md) ("The host
runtime", "One scorer") and the points `ENGINE_ROUND_RESET` and
`ENGINE_PLAYER_LEFT`. It follows
[2026-11-21-engine-vtables.md](2026-11-21-engine-vtables.md) and
[2026-11-22-script-scoring-and-roles.md](2026-11-22-script-scoring-and-roles.md).
Nothing here states what the code does today; the design page does.

## 2026-11-23: where a round begins and where a player leaves

Commit `692357f5c`; retail 1.0 `default.xbe`. Static reading only: `h2
disasm`, a linear disassembly of `.text` searched for writes to the
engine globals (`E = [0x4e9ae8]`) and for `call`/`jmp` targets. No run.

**Reproduce.** `h2 disasm 0x12000 1200000 > text.asm` disassembles
`.text` in one pass (about 1.1 million lines). Writes of the round start
tick: lines matching `mov [e??+70h],` within 60 lines after a load of
`[4E9AE8h]` (five hits; `0x15c60a` and `0x15cbbc` are the engine's,
`0x162232`, `0x19f640` and `0x2bf340` are other structures or the
stack). Callers of a routine: `rg 'call 00157AE0h' text.asm`. Vtable
slots: the dword reader in the vtables note
(`python3 -I words.py disc/default.xbe 0x453d38 20`).

### The round state machine `0x15c550`

`E+0x6c` (`i16`) is the round state in force and `E+0xc04` the state
asked for. When they differ, `0x15c550` switches on the new state, calls
the engine's slot `+0x90` with it, and stores it in `E+0x6c`
(`0x15c6c7`) (**E**). On the new state:

| State | Path | What it does |
|---|---|---|
| 1 | `0x15c5de` | calls the round reset `0x157ae0`; stores the game time's tick (`[0x510c54]+8`) in `E+0x70`, the round start tick (`0x15c60a`); outside a distributed client, sets each player's lives `+0x1ac` from `G+0x1b8` (-1 when 0), marks the player entity dirty (`0x20`) and sets player `+2` bit 3 (`0x15c640` to `0x15c695`) |
| 2 | `0x15c599` | stores five seconds of ticks in `E+0xc00` and sets the statistics latch `[0x510ca1]` |
| 3 | `0x15c579` | adds one to `E+0x6e`, the rounds played, and stores one second of ticks in `E+0xc00` |

(**E**, all three; calling 1 "a round in play", 2 "the round's end" and
3 "between rounds" is **S**: it rests on the timers and the counter, and
a run that logs `E+0x6c` across a two-round game would confirm it.)

`0x15ca10`, called from `GAME_TICK` at `0x138073`, returns at once in a
distributed client (`[0x4e6948]+0xc` 4, `0x15ca34`) and drives the
machine: in state 2, when `E+0xc00` counts down to 0, it asks for state
3 through `0x15fe50` (EDX 3) while `E+0x6e` is below 31, else calls
`0x15ba00`; in state 3, when it runs out, it asks for state 1 (EDX 1)
and sets `E+0` bit `0x20` (`0x15ca6c` to `0x15ca74`) (**E**). So each
round after the first begins at `0x15c5de` → `0x157ae0` on the
authority only.

### The round reset `0x157ae0`

Callers: `0x158556` (in `0x158530`) and `0x15c5de` (**E**, the search
finds no other). `0x158530` is called from `GAME_ACTIVATE` at
`0x18f05e`; it skips `0x157ae0` in a distributed client (`0x158550`),
then sets `E+0x6c` and `E+0xc04` to 1 (`0x1585f4`, `0x1585f8`) (**E**).
So the first round's reset runs inside `GAME_ACTIVATE`, before an
observer after `GAME_ACTIVATE` (**E**).

The body (**E**): no engine (`[0x55e4d0 + 4 E+0xc14]` zero), it returns.
Else it zeroes 27 dwords from `E+0x74` (`0x157b07`), copies `G+0x190`
to `E+0xe0`, calls `0x159e20` when `E+0x6e` is positive, and outside a
distributed client calls `0x15dc40` and `0x15ad30`, resets the statborg
(`0x1e97b0` with `E+0x304`) and sets `E+0` bit `0x20`. It clears the
statistics latch, calls `0x157a80` (each player's `+0x1a7` to 0 and
`+0x1a8` to -1) and ends in a tail jump to the engine's slot `+0x0c`
(`0x157ba5`). Juggernaut's slot `+0x0c` is `0x24dc40` (`mov al,1;
ret`): its role mask `[0x510c9c]+0` is not touched by a round reset
(**E**, vtable `0x453d38`).

A distributed client takes the round start from replication instead:
`0x15cba0` (from `0x158604` and `0x24e511`) stores the round start tick
and calls `0x157a80` (**E**); a script runs only on the authority, so the
runtime does not hook it (**Design**).

### The player leaving: `0x152df0` and the engine's notice `0x157de0`

`0x152df0` (stdcall, the salted player datum at `[esp+4]`, `ret 4`) takes
a player out of the game (**E**): it marks the player's unit for
deletion (object `+0x10b` bit 2, `0x152e36`), detaches it (`0x14cad0`),
clears player `+0x30`, `+0x1a`, `+0x1c`, `+0x20`, `+0x24` and `+0x28`
with the tables they index, sets player `+2` bit 1
(`0x152f49`), stores the tick in `+0x10`, clears `+2` bit 0
(`0x152f69`), takes one from the player count `[0x4e8c20]+0`, and last
calls `0x157de0` with the datum in ESI (`0x152f6f`).

`0x157de0` (**E**): no engine, it returns. Else it calls `0x158140` when
`G+0x184` bit 0 is set (not in a distributed client), then the engine's
slot `+0x1c` with the player (`push esi; call [eax+1Ch]`, the slot pops
4), then posts an engine event through `0x19eb30` (record `+4` 11, `+0xc`
the player, `+0x10` its team from player `+0xc0`) and marks the player's
game-engine entity (`E+0x2c` by player index) dirty with `0x200`
through `0xb58c0`. Calling event 11 "a player quit" is
**S** (the announcer or kill-feed line it makes is not traced).

Juggernaut's slot `+0x1c` is `0x1921f0` (**E**): outside a distributed
client it clears the player's bit of the role mask `[0x510c9c]+0`,
marks the engine-globals entity (`E+0x24`, `0x20`) and rebuilds the
teams' mask (`0x192b20`). Slot `+0x14`, the player added (`0x157c3d`),
is the same body. Slayer's `+0x1c` is `0x72c70` (`ret 4`).

`0x152df0` has three callers (**E**): `0x84f99` (in `0x84f30`, from
`0x68431`: the player whose 12 bytes at `+4` match, skipped when already
leaving), `0x14c7f4` (in `0x14c630`, from `0x68407`, `0x834d2` and
`0x137e65`: players whose machine index `+0x1a` is not in the list it is
given) and `0x14bb9b` (in `0x14bac0`, from `0x18e719`: every player not
already leaving). `0x18e700` is called from `0x687c3` and from
`0x12bab4`, which runs it when `G+0x1120` is set; `GAME_ACTIVATE` sets
that byte at `0x18f03e`. That the second caller is a session's
membership change and the third the game's unload are **S**. After
`GAME_END` the runtime is idle, so an unload's quits reach no script
(**Code**, `Runtime::ended`).

`listed()` in `guest-layout/src/view.rs` and `player.rs` drops a player
with `+2` bit 1 (**Code**), so at `0x157de0`'s entry the leaver is off
the roster the script's `players` reads.

### What `JUGGERNAUT_TICK` did with a leaver

`0x192530` (**E**) walks the players and, for each one holding the role
whose `+2` bit 1 is set, or whose bit 0 is set and for whom `0x15d770`
(not read) or `PLAYER_OUT_OF_LIVES` answers true, clears its role
bit; when no player is left holding it, it draws one (`0x192a30` with
-1, `0x192667`). A script game answers this slot ("One scorer"), so the
leaver's bit is cleared by slot `+0x1c` alone, and a Juggernaut out of
lives keeps the role.

### Not verified

- **S**: state 1, 2, 3 as in play, round over, between rounds. A run
  that logs `E+0x6c`, `E+0x6e` and `E+0x70` across a two-round
  Juggernaut game confirms it, and that `ENGINE_ROUND_RESET` fires once
  a round after the first.
- **S**: whether the roster is filled when the start observer runs. If
  it is empty, the first `on round_start` deals no role and the
  built-in's `on tick` deals it at the first tick. A run's
  `rules.state` after the first tick shows which.
- **S**: what a client shows for event 11 and for the role change on a
  leaver.
