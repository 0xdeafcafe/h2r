# Game type reads for the carrier-free design: evidence

Evidence for [game-types-without-carriers.md](../game-types-without-carriers.md)
(slice GT0, and the "E first" marks) and for
[rules-engine.md](../rules-engine.md) ("Which engine index a script
rides", "Not verified"). Nothing here states what the code does today;
the design pages do.

## 2026-11-24: GT0, read from the retail image

Commit `4d1ae3913`; retail 1.0 `default.xbe`. Static reading only, with
`h2 disasm` and a dword reader. No run, no build, no map read.

**Reproduce.**

```sh
X=disc/default.xbe
h2 disasm $X 0x12000 1100000 > text.asm     # .text to 0x3694aa
h2 disasm $X 0x3694ac 40000 > text2.asm     # the rest of .text (no hits below)
h2 disasm $X 0x2bbfa0 30                    # Slayer +0x18: the start event
h2 disasm $X 0x19df10 60                    # the response picker
h2 disasm $X 0x215900 120                   # the variants of a content category
h2 disasm $X 0x216a50 14                    # record -> content category
h2 disasm $X 0x10ca80 150                   # an item's owner change
h2 disasm $X 0x15e250 40                    # objective picked up
h2 disasm $X 0xa45e0 200                    # the engine-globals entity methods
h2 disasm $X 0x13a050 220                   # the HUD widget state words
h2 disasm $X 0x195a40 80                    # the stats module's game start
h2 disasm $X 0x232d77 300                   # the post-game report build
h2 disasm $X 0x137dd0 30; h2 disasm $X 0x1574c0 110   # options copy, engine start
grep -n '55E4D0h\]' text.asm                # the 287 engine table loads
```

Dwords (vtable slots, jump tables) with the reader in
[2026-11-21-engine-vtables.md](../../engine/notes/2026-11-21-engine-vtables.md)
(`python3 -I words.py disc/default.xbe 0x19e09c 11` prints the event
type table). Enclosing functions were found by the `int3` padding before
a routine; a routine without padding is named by its first caller-visible
address only.

Shorthand: `E` is the engine globals `[0x4e9ae8]` (`E+0xc14` the engine
index), `G` the game globals `[0x4e6948]` (active options at `G+8`, so
the variant record at `G+0x13c` and its engine index at `G+0x180`; mode
byte `G+0xc`, 4 a distributed client).

### 1. The start line

**Verdict.** The start line is a game-engine event that each engine's
vtable slot `+0x18` posts to one player when that player's machine is in
the game. The event names its line by an **event type** and an index,
not by the engine: the response, its text and its sound come from the
map's multiplayer globals by event type. A Slayer game can say another
type's line by posting that type's event (or by answering Slayer's
`+0x18` with it). **E**, except that index 0 is the start line, which is
**S**.

- **Who posts it.** Slot `+0x18` is called from the player update
  `0x14d2a0` (`0x14d33d`) when player `+2` bit 0 turns on (the machine
  check `0x686e0` on player `+0x14`). Every engine's `+0x18` builds an
  event of type T, index 0, for the player at `+8`, and posts it with
  `0xa7c50` (to remote machines) then `0x19eb30` (locally); none runs on
  a distributed client (`G+0xc` 4). Bodies: Slayer `0x2bbfa0` (T 2), CTF
  and Assault `0x240300` (T 3, or 10 when `G+0x180` is 9; only for a
  player with a team, `player+0xc0`, and it goes on after the post,
  not read), Oddball `0x2bc370` (T 4), KotH `0x2bd260` (T 6),
  Juggernaut `0x192240` (T 8), Territories `0x2bf480` (T 9).
- **The event types.** Type 0 is the general block (the time-left lines
  at `0x15bf4c`, `0x15bf82`), 2 Slayer, 3 CTF, 4 Oddball, 6 KotH,
  8 Juggernaut, 9 Territories, 10 Assault (read from every poster:
  `0x2409b8` computes 3 or 10, `0x192266` 8, `0x2bc4bc` 4, `0x2bd620` 6,
  `0x2c081c` 9). Type 1 is posted from `PLAYER_KILLED` (`0x15d64a`,
  posted at `0x15d66d`), not read further; 5 and 7 have no block
  (below). The types are not engine indexes.
- **How a machine plays it.** `0x19eb30` delivers to the event's player
  (`+8`), or to each player when it is -1. `0x19de80` picks the audience
  (cause player, effect player, cause team, effect team, everyone) and
  `0x19df10` finds the response: the map's multiplayer globals
  (`matg+0x16c`), runtime element (`+0xc`), block `+0x98 + 8 * T`
  through the table `0x19e09c` (types 5 and 7 fall through with no
  block), matching the response's index word (`+4`) and audience
  (`+6`). `0x19e890` shows the text (local users only) and `0x19e770`
  queues the sound on the announcer queue (`0x23ef80`). The only engine
  read on the way is the token fill `0x19e1c0`, which tests that an
  engine exists (`0x19e3fb`) for team-score tokens and calls slot
  `+0xc8` (`0x19e706`); it never reads the index.
- **On a client.** `0xa7c50` sends only in mode 5 (the host of a
  distributed game); the client runs the same `0x19eb30` path on its own
  map's globals. So the line follows the type in the event, on every
  machine.
- **Making a Slayer game say another line.** A host posts
  `{type T, index 0, player p}` through `0x19eb90` (`0xa7c50` then
  `0x19eb30`), or answers Slayer's `+0x18` (`0x2bbfa0`) with that post
  instead of type 2. A type of ours needs a block the map's globals do
  not have; our clips go through the announcer queue directly.
- **Not read.** That the index 0 response of each type block is the
  game type's name line (**S**). The read that settles it: the runtime
  element's blocks of `shared.map`'s multiplayer globals, the sound of
  the response with index 0 in blocks `+0xa8`, `+0xb0`, `+0xb8`,
  `+0xc8`, `+0xd8`, `+0xe0`, `+0xe8` (`h2 tag`, not built in this
  worktree).

### 2. The game type list's filing

**Verdict.** The list files variants by **content category**, and the
category is computed from the record's engine index when a variant is
added to the built-in list or saved; the list itself never reads `+0x44`.
**E**.

- **Record to category.** `0x216a50` (EAX the record) jumps on `+0x44`:
  1 CTF to 7, 2 Slayer to 1, 3 Oddball to 4, 4 KotH to 2, 7 Juggernaut
  to 5, 8 Territories to 9, 9 Assault to 8. It has no bounds check:
  5, 6 and anything out of 1..9 jump through a zero or a stray entry.
- **Built-ins.** `0x217080` adds a record to the built-in list
  (`[0x51ea14]`: a count, then `0x1e4`-byte entries), with entry `+0`
  the category from `0x216a50` (`0x2170af`).
- **Saved variants.** The save paths pass the category from `0x216a50`
  to the save dialog `0x238c69` (`0x2baa29`, `0x2ca45d`, `0x2ca49b`). The
  saved entry keeps its category at `+0x38` of its `0x40`-byte listing
  entry (`[0x51ea14] + 0xbef8` per device), which the enumerator reads.
  That the dialog writes the entry's `+0x38` is inferred from the reader.
- **The list.** A game type row opens a variant list with its category at
  screen `+0x4fd8` (`VARIANT_CATEGORY_ROW_CHOSEN`, `VARIANT_LIST_LOADER`;
  **Code**, `tweakers.rs`). `VARIANTS_ENUMERATE` `0x215900` returns the
  saved entries whose `+0x38` equals the category (`0x2159b9`) and, with
  `builtins`, the built-ins whose `+0` does (`0x215ab1`).
- **What it means.** A script variant on index 2 is filed under Slayer
  (category 1) unless the list is answered by us, as `tweakers` does at
  `0x215900`. A saved file carries its category from the moment it is
  saved, so filing by `type` is a filter at `0x215900` (or a category of
  ours written at save), not a record change.

### 3. The HUD and the scoreboard

**Verdict.** Partly answered. Four routes carry engine state to a
machine's display, and only one of them goes through the engine object:
**E** for each route; which HUD widget shows which engine's state is not
traced.

- **The replicated engine-globals entity goes through the client's own
  engine object.** The entity's methods `0xa45e0`, `0xa4650`, `0xa4730`
  and the create `0xa4880` each call the local engine's `+0x9c` and go on
  only when the entity's type equals it; they then hand the data to the
  local engine's `+0xb0`, `+0xb4` or `+0xb8` (`0xa4633`, `0xa46a8`,
  `0xa4788`, `0xa48f6`). A mismatched type is refused (AL 0). `0xa46c0`
  stores the entity at `E+0x24`. The host creates it with the type from
  `+0x9c` (`0xa77c0`, `0x158622`, through `0xb57d0`). Per team entities
  (`E+0x2c`, 16) use `+0xbc` (`0x9afb0`), a body every engine shares
  (`0x24de50`). Bodies: Slayer and Oddball share `0x2bcdd0`, `0x2bce00`,
  `0x2bce40`; CTF `0x243390`, `0x243400`, `0x2437a0`; KotH `0x2bdb40`,
  `0x2bdb80`, `0x2bdc00`; Juggernaut `0x192870`, `0x1928b0`, `0x192900`;
  Territories `0x2c0410`, `0x2c0450`, `0x2c05f0`. So a client on index 2
  holds Slayer's globals block only, whatever the host's script does.
- **Engine events** (question 1): objective lines, their text and sound,
  by event type, with no engine read on the client.
- **The statborg** carries scores (`SCORE_ADD` marks its bits,
  firefight.md; stat rows below).
- **The widgets do not read the engine.** The widget state words are
  built by `0x13a050` from the per-user HUD state (ESI) and the game mode
  (`G+8`); it reads no engine index, no variant and no engine slot
  (`0x13a050`..`0x13a3a0`). Who sets the state bits a game-type widget
  is gated on (the scoreboard `nhdt`'s word D `0x400`, hud-kit.md) is not
  traced; that read is the writers of the per-user state that
  `0x13a3d4` passes in.
- **Per-engine globals are engine-only.** CTF's `[0x51ec80]`, Oddball's
  `[0x51ecc4]` and Juggernaut's `[0x510c9c]` are read only inside engine
  code (CTF `0x240160`..`0x2447f0`, Oddball `0x2bc1b0`..`0x2bcbb0`,
  Juggernaut `0x1921d0`..`0x192b20`, and `0xcd7b0`, `0x1588b0`,
  `0x159dd0`, `0x15eb20`, `0x15fef0` for Juggernaut): no HUD or UI
  routine reads them. KotH's and Territories' globals pointers were not
  searched.
- **Slots with no call found.** `+0xa4`, `+0xa8`, `+0xac`, `+0xc0`,
  `+0xc4` (shared bodies `0x24e320`, `0x24e360`, `0x24e490`,
  `0x24de80`, `0x24e0e0`) have no dispatch through the table and no
  direct call. Whether the HUD reaches them another way is not traced.

### 4. The post-game report

**Verdict.** The report's per-engine part is keyed on the variant
record's engine index, copied when the game starts, and its numbers are
the per-engine statborg rows that only the engine code writes. **E**.

- **The copy.** The stats module's game start `0x195a40` (from
  `0x67f50`, and only with an engine) copies the active record
  (`G+0x13c`, `0x130` bytes) to `0x510de8`, so its engine index is at
  `0x510e2c`. `0x196a90` returns it (0 when stats are off,
  `[0x510cb0]`, `[0x510cb1]`).
- **The report.** The build `0x232d77` (live-gamestate.md) stores it at
  `[0x50224c]` (`0x232de2`) and switches on 2, 1, 3, 4, 7, 8, 9 at
  `0x233055`..`0x23311a` to pick two stat rows per player; the column
  text is picked by the same switch at `0x23397a` and `0x2343d5`. Each
  machine reads its own copy.
- **The rows.** The stat table at `0x46ddc0` (`0x10`-byte rows) has
  common rows (games, kills, deaths, assists) and per-engine rows: 14 to
  22 CTF and Assault, 23 to 26 Oddball, 27 to 30 KotH, 38 to 42
  Juggernaut, 43 and 44 Territories. `0x1967d0` adds to a row; its
  callers are engine code (CTF `0x240ee0`, `0x240ff0`, `0x241520`,
  `0x242ba0`, `0x242e00`; Juggernaut `0x1922a0`; `0x15b930`, which
  Oddball and Territories use) and the common paths (`0x15b650`,
  `0x15ba90`, `0x15bb20`).
- **What it means.** An index 2 record shows Slayer's two columns. A
  host that wants CTF's columns writes `0x510e2c` (or `[0x50224c]`) and
  the CTF rows through `0x1967d0`, on every machine that draws a report.

### 5. Who carries a flag, bomb or ball

**Verdict.** The weapon system. A carried objective is a weapon item; the
unit's inventory code attaches it through the item's owner change, which
then tells the engine. The engine's slots only record and react. **E**.

- **The attach.** `0x10ca80` (EAX the item, one stack argument the new
  unit or -1) is the item's owner change, called from the unit's
  inventory routines (`0xce4cf` in `0xce470`, `0xceaa0` in `0xcea70`,
  `0xcebf4` and `0xcec57` in `0xceb30`). It writes the owner
  (`+0x154`) and the item's state bits (`+0x12c`); for a weapon (object
  type 2) with the objective bit (`+0x16c` bit 6) it calls
  `0x15e250` (picked up, `0x10cb8f`); on a drop, `0x15e460`
  (`0x10cca5`).
- **The engine is told.** `0x15e250` finds the item in the engine's
  objective registry (`E+0xc18` count, entries of `0x10` from `E+0xc1c`),
  writes the holder unit (`+8`) and player (`+0xc`), sets the item's
  `+0x16c` bit 7, and calls slot `+0x5c`. With no engine object it
  returns at once (`0x15e26b`) and the registry is not updated.
  `0x15e460` is the drop's twin (`+0x60`).
- **The slots attach nothing.** CTF's `+0x5c` `0x240ff0` and `+0x60`
  `0x2411b0`, Oddball's `0x2bc420` and `0x2bc4f0` add stat rows
  (`0x1967d0`), mark replication (`0xa7810`) and post events; KotH,
  Territories and Juggernaut use the shared `0x24dc50` (nothing).
  Registration (`0x15e050`, `+0x54`) is called by the engines' spawn
  code (`0x2423d9`, `0x2bc26c`, `0x2c0a7d`) and by `0xa228f`, `0xd0ab5`.
- **What it means.** A script's flag is a weapon object with the
  objective bit, given by the normal pickup; registering it needs the
  registry (`0x15e050`), which needs an engine object.

### 6. Where the engine is chosen at game start

**Verdict.** At game activation, after the options are copied into the
game globals and cleaned, and before any subsystem's start. The earliest
swap that every later reader sees is at `GAME_ACTIVATE` entry, on the
options in EAX. **E**.

- **The order.** `GAME_ACTIVATE` `0x18f000` calls `0x137ca0` with the
  options (`0x18f007`). `0x137ca0` calls `0x137dd0`, which copies the
  `0x1118`-byte options to `G+8` (`0x137de5`) and, for a multiplayer game
  or a nonzero `G+0x180`, runs the clean-up `0x19d650` on `G+0x13c`
  (`0x137e05`). Then `0x137ca0` calls every subsystem's start (`+0` of
  each `0x24`-byte entry of `0x440de0`, `0x137cd0`); the game engine's is
  entry 22, `0x1574c0`.
- **The choice.** `0x1574c0` clears `E` and writes `E+0xc14` from
  `G+0x180` (`0x1574ec`). It writes 0 when the map's multiplayer globals
  have no runtime element (`0x157578`), and -1 when the engine's `+0x04`
  refuses (`0x1575c8`). Later readers of the record: the round start
  (`0x158530`, `0x18f05e`), the stats copy (`0x195a40`, question 4).
- **The earliest swap.** Before `0x137dd0`'s copy: `GAME_ACTIVATE` entry,
  options in EAX, record at options `+0x134`, engine index at options
  `+0x178`. The swapped record then goes through the clean-up, which must
  accept it. Between `0x137dd0` and `0x1574c0` (an after hook on
  `0x137dd0`) works too but skips the clean-up. Upstream, each machine
  builds its options from its own session (`SESSION_GAME_START`
  `0x6ec80`), whose record a client decoded with the wire codec; a swap
  on the host alone leaves clients on the old index, and their
  engine-globals entity type then disagrees (question 3). Every machine
  must make the same swap from the same record.

### Owning the four paths

What a Rust owner must take over for a game with no retail engine
object, path by path. **E** unless marked.

**The engine table `0x55e4d0`.**

- Written once, by `0x378500` (a static initializer, referenced from
  `0x461778`), from the object pointers `[0x475af0]` (Slayer, index 2),
  `[0x470a5c]` (CTF, 1 and 9), `[0x475af4]`, `[0x475af8]`,
  `[0x46dd58]`, `[0x475afc]`; index 10 is written 0, 0, 5 and 6 are
  `.bss` zero.
- Read at 287 sites: 61 make a slot call (listed below), 226 only test
  that the entry is nonzero. The tests gate most of the multiplayer game:
  scoring (`SCORE_ADD`), the stat updates (`0x1e97b0`..`0x1ea880`, by
  their callees), the stats
  start (`0x195a40`), spawning (`0x23b060`..`0x23bc40`), events,
  replication (`0xa45e0`..`0xa4910`), the player and team code
  (`0x14bc80`..`0x163080`). A zero entry at index 2 turns all of it off
  as in a campaign game, so removing the object is not a deletion; it is
  supplying another.
- Unknown index: indexes 0, 5, 6 and 10 read 0 (no engine). An index
  above 10 reads past the table (`0x55e4fc` on); `0x1574c0` writes -1 on
  a refused start, which reads `0x55e4cc`. Neither is guarded.
- Some slot sites do not test the entry themselves and rely on their
  caller: `0x157670` (`+0x68`), `0x162060` (`+0xa0`; both its callers,
  `0x68028` and `0x6af4c`, test), `0x15ee30` (`+0x94`), `0x9afb0`
  (`+0xbc`), `0x159c28` (`+0x98`), the entity methods' second loads.
- **A Rust owner supplies** a nonzero engine object at `[0x55e4d8]`: one
  dword pointing at a 51-slot table whose entries are our thunks
  (thiscall, ECX the object, the callee pops), installed after
  `0x378500` runs and before the first game. Then every existence test
  passes and each slot below is ours. Hooking the 226 tests instead is
  not practical.

**The slots a game still calls** (from the 61 sites; Slayer's body is
what the owner replaces):

| Slot | Call sites | Slayer's body | What the owner must do |
|---|---|---|---|
| `+0x00` engine code | `0x15af19`, `0x15f483`, `0x19ef76` | `0x9bec0` (2) | return 2 |
| `+0x04` start | `0x1575bc` | `0x2bbf50` | clear its globals, return true |
| `+0x0c` round reset tail | `0x157ba5` (`ENGINE_ROUND_RESET`) | `0x24dc40` | return true |
| `+0x10`, `+0x14`, `+0x1c`, `+0x20`, `+0x24`, `+0x28` | `0x15797c`, `0x1579ad`; `0x157bf3`, `0x157c32`; `0x157df0`, `0x157e24`; `0x157ee3`, `0x157f22`; `0x15800c`; `0x1582e3`, `0x158327` | `0xa45d0`, `0x2bbf70`, `0x72c70`, `0x72c70`, `0x2bc110`, `0x72c70` | team and player add, leave, change |
| `+0x18` machine in game | `0x14d33d` | `0x2bbfa0` | post the start event of the script's type (question 1) |
| `+0x30`, `+0x34`, `+0x38` | `0x15878e`; `0x1595f5`; `0x15ea5c` | `0x175f40`, `0x72c70`, `0x24dc60` | activate; not read |
| `+0x3c` per player | `0x15c23d` | `0x72c70` | nothing, or the script's per-player handler |
| `+0x40`, `+0x44` | `0xcde79`; `0x15e8ec` | `0xa5750`, `0x24dc50` | not read |
| `+0x48` per tick | `0x15c296`, `0x15c2d3` | `0x175f40` | the script's tick |
| `+0x4c` | `0x15c03b` | `0x24dd50` | a float per player |
| `+0x54`..`+0x60` objective | `0x15e0fb`, `0x15e1a9`, `0x15e2e1`, `0x15e4b2` | `0x72c70`, `0x24dc50` | the script's objective events (question 5) |
| `+0x64` time left | `0x1624bd` | `0x24dd40` | return its argument |
| `+0x68` | `0x15767b` | `0x224240` | not read |
| `+0x6c` enemies | `0x15cf4a`, `0x15d722`, `0x23b98b`, `0x2bc887` | `0x24dd70` | teams differ (or the script's roles) |
| `+0x74`, `+0x7c` | `0x15cc7a`; `0x15d317` | `0x2dae30`, `0x24dd90` | not read; -1 |
| `+0x78` kill | `0x15d67e` (call `0x15d6a4`) | `0x2bc000` | the script's kill handler, the one scorer |
| `+0x80` spawn weighting | `0x23bc51`, `0x23bc7e` | `0x24dc50` | nothing, or the script's spawn areas |
| `+0x8c` trait query | `0x15b27b`, `0x24049d`, `0x2bc95b` | `0x24dda0` | the host's trait answer |
| `+0x90`, `+0x94`, `+0x98` | `0x15c667`, `0x15c69f`; `0x15ee3c`; `0x159c28` | `0x72c70`, `0x24de40`, `0x2bcc80` | state change; not read; false |
| `+0x9c` entity type | `0xa45f0`, `0xa4660`, `0xa46cf`, `0xa4740`, `0xa47b0`, `0xa488c`, `0xa77cb`, `0x158615` | `0x99810` (0) | return 0 on every machine |
| `+0xa0` | `0x16208b` | `0x175f40` | nothing |
| `+0xb0`, `+0xb4`, `+0xb8` entity data | `0xa4620`; `0xa4690`; `0xa4770`, `0xa48e6` | `0x2bcdd0`, `0x2bce00`, `0x2bce40` | encode and apply the globals block (or ours) |
| `+0xbc` team entity | `0x9afbf` | `0x24de50` | as Slayer |
| `+0xc8` token fill | `0x19e706` | `0x241510` | as Slayer |

The other slots (`+0x08`, `+0x2c`, `+0x50`, `+0x70`, `+0x84`, `+0x88`,
`+0xa4`..`+0xac`, `+0xc0`, `+0xc4`) have no dispatch through the table;
the owner fills them with Slayer's bodies.

**The record clean-up `0x19d650`.**

- EBX the record (`0x130` bytes); returns AL 1 when the record was
  already clean, else resets it to a default Slayer record (`+0x44` 2)
  and returns 0 (`0x19de43`). It clamps the common fields, clamps
  `+0x44` to 1..9 (`0x19d9b4`), clamps the type area by engine through
  `0x19de54` (5 and 6 write 10, which fails the compare), then compares
  the whole record with its entry copy (`0x19dcc7`).
- Callers: `0x7d9b7` (the codec), `0x137e05` (the options copy),
  `0x148a21` and `0x14a064` (in `0x1473b0`), `0x19d640` (`0x19d620`,
  which checks a copy), `0x212b3b`, `0x212bfb` (`0x212bc0`, the content
  read the variant list uses), `0x212ed9`, `0x212f1d`.
- **A Rust owner supplies** the same answer for every record it accepts
  (`variants::record::check`, **Code**, models it), hooked at
  `0x19d650`. With every script record on index 2 the retail body is
  correct as it is; only an index of ours needs the hook.

**The wire codec decode `0x7d520`.**

- EAX the record out, ECX the bit reader (`0x1959c0`). Reads 4 bits into
  `+0x44` first (`0x7d538`); 0 zeroes the record and succeeds
  (`0x7da1f`). The type area is read by engine through `0x7da38`; 5, 6
  and above 9 fail (`0x7da13`, AL 0). It ends with the clean-up on a copy
  and returns its AL (`0x7d9b7`), so a record the clean-up would change
  fails to decode.
- Callers: `0xb0c10` (in `0xb0900`) and `0xb1e09` (in `0xb1c80`). The
  encoder was not read.
- **A Rust owner supplies** a decode and an encode that agree on every
  machine; for index 2 records the retail pair already does.

**The lobby protocol map `0x161c90`.**

- ECX the record; returns 2 (distributed) for `+0x44` 1 to 4 and 7 to
  9, 1 (synchronous) for 5, 6 and anything out of range, through the
  byte table `0x161cbc`.
- Callers: `0x640a7` (in `0x64060`) and `0x7155d` (in `0x71480`).
- **A Rust owner supplies** 2 for every script record.

**A fifth path: the content category `0x216a50`** (question 2). It
jumps on `+0x44` with no bounds check, on save and on the built-in add.
An index outside 1..4 and 7..9 must not reach it.

| Path | Callers | Unknown index | Owner supplies |
|---|---|---|---|
| Engine table `0x55e4d0` | 287 loads: 61 slot calls, 226 existence tests | 0, 5, 6, 10: no engine (campaign-like); above 10 or -1: reads outside the table | a nonzero object at `[0x55e4d8]` with a 51-slot table of thunks |
| Clean-up `0x19d650` | 9 sites (codec, options copy, content reads) | clamped to 1..9; 5, 6 reset to Slayer | the same accept/reset answer; nothing for index 2 |
| Codec decode `0x7d520` | `0xb0c10`, `0xb1e09` | 0 zeroes and succeeds; 5, 6, above 9 fail | an encode and decode that agree; nothing for index 2 |
| Protocol map `0x161c90` | `0x640a7`, `0x7155d` | 1 (synchronous) | 2 |
| Category `0x216a50` | `0x2170af`, `0x2baa29`, `0x2ca45d`, `0x2ca49b` | jumps through a zero or a stray entry | a category; nothing for index 2 |

### Verdicts

| # | Question | Address | Verdict | Evidence |
|---|---|---|---|---|
| 1 | The start line | slot `+0x18` (Slayer `0x2bbfa0`), `0x19eb30`, `0x19df10`, `0x19e770` | an event of type T (2 Slayer, 3 CTF, 4 Oddball, 6 KotH, 8 Juggernaut, 9 Territories, 10 Assault), index 0, looked up in the multiplayer globals by type; a Slayer game can post another type's event | **E**; index 0 being the name line **S** |
| 2 | The list's filing | `0x216a50`, `0x217080`, `0x215900` | by content category, computed from `+0x44` when a variant is added or saved; the list compares stored categories | **E** |
| 3 | HUD and scoreboard | `0xa45e0`..`0xa4910`, `0x13a050` | the engine-globals entity is applied by the client's own engine slots after a type match; events and the statborg need no engine; the widget state reads no engine; which widget shows which engine state is not traced | **E**, partial |
| 4 | Post-game report | `0x195a40`, `0x232d77` (`0x233049`, `0x23397a`) | keyed on the record's engine index copied at game start (`0x510e2c`); per-engine stat rows written only by engine code | **E** |
| 5 | Carried objectives | `0x10ca80`, `0x15e250`, `0x15e460` | the weapon system attaches; the engine is told through `+0x5c`, `+0x60` and keeps a registry | **E** |
| 6 | Engine chosen | `0x137dd0`, `0x1574c0` (`0x1574ec`) | `E+0xc14` from `G+0x180` at the engine's subsystem start; swap at `GAME_ACTIVATE` entry on every machine | **E** |

## 2026-11-24: GE1 and GE3, the clean-up's exit, the protocol map and the decode's type area

Commit `ed7a7da50`; the same image and tools as above. Static reading
only; the differential test
`crates/rewrites/engine/tests/variant_record_differential.rs` checks the
first two against the lifted original.

**Reproduce.**

```sh
h2 disasm disc/default.xbe 0x19d650 40      # the clean-up's entry copy
h2 disasm disc/default.xbe 0x19d9b4 20      # the index clamp and type-area jump
h2 disasm disc/default.xbe 0x19dcb3 60      # the compare and the reset
h2 disasm disc/default.xbe 0x19de10 20      # the two exits
h2 disasm disc/default.xbe 0x161c90 20      # the protocol map
h2 disasm disc/default.xbe 0x137dd0 22      # the options copy's call
h2 disasm disc/default.xbe 0x7d7f0 40       # the decode's type-area jump
h2 disasm disc/default.xbe 0x7d990 80       # the decode's tail and exits
h2 disasm disc/default.xbe 0x1959c0 70      # the bit reader
```

- **The clean-up's exits.** EBX the record, no stack arguments, ESI, EDI
  and EBP saved. It copies the record to its frame, clamps in place, and
  compares (`0x19dcc7`). Unchanged, it returns at `0x19de4b` with EAX 1
  (EAX was zeroed for the compare), ECX 0 (the compare ran out) and EDX
  0 (the routine's zero register from `0x19d678`). Changed, it builds a
  default Slayer record in its frame (a localized name through
  `0x1a0180` when `[0x4e034c]` has a string table, flags
  `& 0xffff8f9a | 0xf9a`, `+0x44` 2) and copies it over the record,
  returning AL 0 (`0x19de43`). An index clamped to 1..9 that changed skips
  the type area (`0x19d9d4`); 5 and 6 reach the type-area jump and are
  written 10 (`0x19dcb3`), which fails the compare.
- **The options copy ignores the answer.** `0x137dd0` calls the clean-up
  on `G+0x13c` (`0x137e05`) and goes on with the record as cleaned.
- **The protocol map** is ECX the record, `+0x44` minus 1 in ECX, EAX 1;
  in range (0..8 after the decrement) EDX is the byte at `0x161cbc + n`
  and the jump goes through the two-entry table at `0x161cb4` (0 to
  `0x161cac`, EAX 2; 1 to the `ret`). The bytes are 0 for 1..4 and 7..9
  and 1 for 5 and 6. Out of range EDX is untouched.
- **The decode's type area.** After the common fields (the last is
  `+0xd7` at `0x7d824`) it loads `+0x44`, decrements it and jumps to the
  failure exit `0x7da13` (AL 0) above 8 (`0x7d82a`..`0x7d831`), else
  through the table at `0x7da38`: 5 and 6 are the failure exit too. Each
  engine's reads end in the shared tail `0x7d9a8`, which copies the
  record to the frame, runs the clean-up on the copy (`0x7d9b7`) and
  returns its AL. An index of 0 zeroes the record and returns AL 1
  (`0x7da1f`). The bit reader `0x1959c0` is EDI the reader (`+0` the
  words, `+4` the size in bytes, `+0x10` the bit position), the count on
  the stack (`ret 4`): it reads LSB first, up to the bits left, and
  advances the position by the full count.
