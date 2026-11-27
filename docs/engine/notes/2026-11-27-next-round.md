# Ending a round without ending the game: evidence

Evidence for [rules-engine.md](../../game/rules-engine.md) ("The engine
API", `next_round`) and the point `ROUND_END`. It follows
[2026-11-23-round-start-and-player-quit.md](2026-11-23-round-start-and-player-quit.md),
which reads the round state machine `0x15c550` and its driver `0x15ca10`.
Nothing here states what the code does today; the design page does.

## 2026-11-27: how the engine ends a round

Commit `6d0582f2a`; retail 1.0 `default.xbe`. Static reading only: `h2
disasm`, a linear disassembly of `.text`. No run.

**Reproduce.** `h2 disasm 0x12000 1200000 > text.asm`. Writes of the
asked-for round state: `rg '\+0C04h\]' text.asm` (two stores of the
constant 2, `0x15b5fe` and `0x15b997`, beside `0x1585f8` and `0x15fe61`).
Callers: `rg 'call 0015B3A0h' text.asm`, `rg 'call 0015B980h' text.asm`.

### The round resolution `0x15b3a0`

EBX is the winner (a team or player index, -1 for none) and one stack
byte is passed on (`ret 4`) (**E**). It counts a round won for the winner
in the statborg (`0x1e9df0` with 6 and 7, `0x15b3fa`), then weighs the
variant's rounds setting, game globals `G+0x188` (`[0x4e6948]+0x188`),
against the rounds played `E+0x6e` (`0x15b570` to `0x15b5e5`) (**E**):

| `G+0x188` | The game ends when |
|---|---|
| 0 | at once (`E+0x6e >= 0`) |
| 1 | `E+0x6e >= 1` |
| 2 | `E+0x6e >= 3` |
| 3 | `E+0x6e >= 5` |
| 4, 5, 6 | the winner has won 2, 3, 4 rounds (`edi`, the statborg's count) |

That 0 to 3 are 1, 2, 4 and 6 rounds and 4 to 6 "first to" is **S**: it
rests on the comparisons, not on the settings screen's text. With 31
rounds played, the game ends whatever the setting (`0x15b5ea`).

A game that ends: outside a distributed client (`G+0x0c` 4) it stores 2
in `E+0xc04` (`0x15b5fe`) and, with the end latch `G+0x1128` clear, calls
`GAME_END` (`0x15b617`) (**E**). This is what the runtime's `end_round`
does (`firefight::guest::end_game`). A game that goes on: it calls
`0x15b980` with 0 (`0x15b630`) (**E**).

Its callers are the engine's own round ends (**E**): the score limit
(`0x15b7a8`, `0x15b906`, against `G+0x18c`), the time limit
(`0x15bf8f`, after `0x15b330` names the leader), `0x15ba82`,
`0x15caf3` in the driver, `0x16236a` and `0x1623f9`, and CTF's tick
(`0x240e1d`).

### The round end `0x15b980`

stdcall, one byte on the stack (`ret 4`) (**E**). In a distributed client
it returns. Else it stores 2 in `E+0xc04` (`0x15b997`), the state asked
for, and unless the end latch `G+0x1128` is set or the byte is nonzero it
builds an untargeted game-engine event of kind `0x1f` and sends and shows
it (`GAME_EVENT_SEND` `0xa7c50`, `GAME_EVENT_SHOW` `0x19eb30`). That the
event is the "round over" line and sound is **S**. Its only caller is
`0x15b3a0` (**E**).

So a round ends without the game ending when `E+0xc04` becomes 2 with
`GAME_END` not called. The state machine then runs as the earlier note
reads it: state 2 for five seconds (`E+0xc00`), state 3 for one second
with `E+0x6e` one higher, then state 1, whose path calls the round reset
`0x157ae0`, which the runtime observes (`ENGINE_ROUND_RESET`) (**E**).
With `E+0x6e` at 31 the driver calls `0x15ba00` instead, which calls
`GAME_END` (**E**).

### The scores across a round reset

The round reset resets the statborg with `0x1e97b0` (`E+0x304`). For
each of the 16 player rows (`0x1c` bytes) it adds the round's value at
row `+0x04` into row `+0x06` and, with the end latch clear, zeroes `+0x04`
(`0x1e97c2` to `0x1e97e8`) (**E**). That `+0x06` is the game's total and
the post-game's score is **S**.

### Not verified

- **S**: the kind `0x1f` event is the retail round-over line.
- **S**: what `0x159e20`, called by the round reset when `E+0x6e` is
  positive, does with each player's unit (`0xb83b0` with 1 for each).
- **S**: whether `PLAYER_STATS` `+0x0`, which `player_score` reads, is
  the round's or the game's score after a reset. A run of a two-round
  Siege that logs `player_score` before and after the reset settles it.
