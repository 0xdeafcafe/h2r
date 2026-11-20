# Halo 2 retail Xbox live game-state map

Static-analysis deliverable for a live scoreboard and top-down player map. All addresses are **guest virtual addresses** in the retail 1.0 XBE, not host pointers. All integer fields are little-endian. Function and field names are assigned descriptions.

- XBE SHA-256: `03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d` (recomputed for this survey).
- **confirmed**: directly supported by inspected retail-XBE code, by the repository's recovered engine code, or by the documented whole-disc structural checks.
- **likely**: strong call-context evidence, but the named semantics are not completely established.
- No game or image build was run. Ghidra's current project did not contain the required function bodies, so direct XBE checks used the existing `h2 disasm`. No Ghidra names, types, or project data were changed.

## 1. Address model and shared datum arrays

Define these after successful map activation, checking each pointer for nonzero before use:

```text
G  = read_u32(0x004e6948)       // game globals
O  = G + 0x8                    // active game options, inline, 0x1118 bytes
V  = O + 0x134 = G + 0x13c      // active multiplayer variant, inline, 0x130 bytes
T  = read_u32(0x00510c54)       // game-time globals
E  = read_u32(0x004e9ae8)       // game-engine globals, 0xc9c bytes
P  = read_u32(0x004e8c24)       // players datum-array header
H  = read_u32(0x004e0300)       // object-header datum-array header
```

The pending options at `0x004ed3a8` are not the active match. `0x00137dd0` copies exactly `0x1118` bytes into `G+8`; the pending object becomes useful only after `0x0018f000` succeeds.

### Generic datum-array header (used by players and objects)

Recovered in `crates/hooking/guest-layout/src/datum.rs` and `docs/engine/datum_new.md`.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| capacity | `+0x20` | `i32` | confirmed | Generic recovered datum constructor |
| element size | `+0x24` | `i32` | confirmed | Generic addressing and lookups |
| valid | `+0x29` | `u8` | confirmed | Generic array lifecycle |
| high-water slot | `+0x38` | `i32` | confirmed | Iteration/lookup upper bound |
| active count | `+0x3c` | `i32` | confirmed | Allocation bookkeeping |
| element storage pointer | `+0x44` | `u32` | confirmed | All player/object access |
| occupancy bitmap pointer | `+0x48` | `u32` | confirmed | Generic iterator |
| total header size | `0x4c` | bytes | confirmed | Generic initializer |

A salted datum is `(salt << 16) | absolute_slot`; `0xffffffff` is none. For players and objects, the first `u16` of each element is its salt and zero means free. Statborg rows and BSP indices use the **absolute slot**, not the full salted datum.

## 2. Lifecycle and hook points

### Recommended hooks

| Event | Entry VA and ABI | Hook timing and confidence |
|---|---|---|
| Map activation / `GameStarted` | `0x0018f000`; custom `EAX = options`; no stack args; success in `AL` | **confirmed.** Call the original and emit only when it returns `AL != 0`. It copies active options, runs new-map callbacks, selects the initial BSP, and sets the ready flag. Entry alone is too early. The successful return tail is `0x0018f086`/`0x0018f08a`. |
| Higher-level successful start | `0x0018e8b0`; one stack arg `options` at `[esp+4]`, `ret 4`; success in `AL` | **confirmed.** Alternative wrapper around cache acquisition plus activation. Caller site `0x0018e91e` is after successful activation. Do not emit from `0x0018eaa0`: that is cache/resource acquisition only. |
| Completed simulation tick | `0x00137fe0`; no args; void | **confirmed.** Call the original, then sample only if `T+8` changed. `0x001380f3` immediately precedes the increment; `0x001380fb` is a common tail also reached without an increment, so an exit hook must compare the counter. |
| Game-ended latch | `0x001389c0`; no args; void | **confirmed.** Idempotently sets `G+0x1128` and initializes `G+0x112c`. Emit on return or on a `0 -> 1` transition at the completed-tick hook. Final score/winner resolution has run; quit/normal-end reason is not encoded in this function. |
| Post-game carnage report built | `0x00232d77`; no args; void; common return `0x002331a0` | **confirmed.** Optional hook for a finalized PGCR snapshot. `0x00196390` tail-jumps here from the game-end path. At entry the source stats exist but report construction is unfinished; use return or caller site `0x0015cb8d`. |
| Winner/round resolution (optional) | `0x0015b3a0`; custom `EBX = prospective winner`; one unknown stack arg, `ret 4`; void | **confirmed ABI; limited use.** Team index in team games, player datum in FFA, `-1` for none/tie. Also handles rounds, so this is not a standalone match-ended event. Correlate it with the later end latch if winner capture is needed. |
| Player death/kill credit | `0x0015ce70`; four stack args, `ret 0x10`; void | **confirmed.** See section 7 for arguments and score-update timing. |

`0x0018e8b0` uses ordinary stack argument order. `0x0018f000` and several simulation functions use LTCG register ABIs; do not infer their prototypes from a generic decompiler.

### Simulation lifecycle and tick envelope refinement

The addresses in `docs/engine/engine-map.md` are real, but `0x00067e30` is initialization, not match start.

| Function | ABI | Role | Confidence |
|---|---|---|---|
| `0x00067e30` | no args; void | Simulation/network-global initialization callback | confirmed |
| `0x00067ee0` | no args; void | Simulation new-map callback; useful only as an additional lifecycle signal | confirmed |
| `0x00067f60` | no args; void | Simulation disposal callback | confirmed |
| `0x00137f70` | custom `EAX = requested tick count`, `EBX = float *remainder` (read/write); void | May execute zero, one, or multiple `0x00137fe0` ticks | confirmed |
| `0x000682e0` | custom `EAX = 0x4048-byte simulation-state *`; void | Initializes per-tick simulation state | confirmed |
| `0x000683a0` | one stack arg `state *`, `ret 4` | Pre-simulation processing inside `game_tick` | confirmed |
| `0x000684e0` | custom `ESI = state *`; void | Post-simulation processing inside `game_tick` | confirmed |

The main game-engine update is called at `0x0013803c` (`0x0015c150`), and end/post-game orchestration at `0x00138073` (`0x0015ca10`). `0x0015c550` is the no-arg post-game state transition helper; requested state is `E+0xc04`, published state is the `i16` at `E+0x6c`, and state `2` follows the post-game path.

## 3. Current map, mode, variant, teams, and limits

### Game and time globals

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| map-active flag | `G+0x01` | `u8` | confirmed | New-map lifecycle |
| active structure-BSP index | `G+0x02` | `i16` | confirmed | `0x0011c1b0` / `0x00137d40`; indexes the scenario BSP block |
| simulation mode | `G+0x0c` | `u8` | confirmed | Simulation/tick gates |
| game/simulation ready | `G+0x1120` | `u8` | confirmed | Set during successful `0x0018f000` |
| game-ended latch | `G+0x1128` | `u8` | confirmed | Set by `0x001389c0`, cleared by new-map setup |
| end-delay countdown | `G+0x112c` | `i32` | confirmed | Initialized from tick rate by `0x001389c0` |
| tick rate | `T+0x02` | `i16` | confirmed | Read from active options by `0x001465b0` |
| seconds per tick | `T+0x04` | `f32` | confirmed | Recovered game-time initialization |
| elapsed simulation ticks | `T+0x08` | `u32` | confirmed | Incremented at the end of eligible `game_tick` calls |
| game speed | `T+0x0c` | `f32` | confirmed | Recovered game-time layout |
| round/start tick for elapsed-match calculations | `E+0x70` | `u32` | likely | Death/round code computes `T+8 - (E+0x70)`; validate across rounds |

### Active options `O = G+8`

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| scenario type | `O+0x00` | `i32` | confirmed | `1` campaign, `2` multiplayer, `3` main menu; other accepted values unresolved |
| tick rate | `O+0x06` | `i16` | confirmed | Copied to `T+0x02` |
| campaign difficulty | `O+0x12a` | `i16` | confirmed | 0 easy, 1 normal, 2 heroic, 3 legendary; `0x00138110` defaults it to 1, the menu's campaign start (`0x0023643f`) sets it, level boot writes it ([level-boot.md](../runtime/level-boot.md)) |
| scenario path | `O+0x1c` | `char[256]` | confirmed | Consumed by cache acquisition at `0x0018eaa0` |
| initial BSP index | `O+0x120` | `i16` | confirmed | Read by `0x0018f000` |
| inline variant | `O+0x134` | 0x130 bytes | confirmed | Multiplayer construction and sanitizer at `0x0019d650` |

Use `O+0x00`, not the cache type, to exclude the main menu. For reference, cache-header type `0x00547840` is an `i16` with `0` campaign, `1` multiplayer, `2` menu, `3` shared, `4` campaign-shared; scenario tag `scnr+0x10` uses `0` campaign, `1` multiplayer, `2` main menu.

### Variant `V = O+0x134 = G+0x13c`

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| custom variant name | `V+0x04` | UTF-16LE `[32]` | confirmed | 0x40-byte area; sanitizer terminates its final code unit |
| game-engine index | `V+0x44` | `i32` | confirmed | Copied to `E+0xc14` by `0x001574c0` |
| engine flags | `V+0x48` | `u32` | confirmed | Bit 0 is team play; used by scoring and post-game code |
| round setting | `V+0x4c` | `i32` | confirmed | Sanitized to `0..6` |
| score limit | `V+0x50` | `i32` | confirmed | Compared with player/team scores; zero bypasses limit checks |
| time limit, seconds | `V+0x54` | `i32` | confirmed | `0x00162470` multiplies it by tick rate to compute remaining ticks |
| join-in-progress setting | `V+0x58` | `i32` | confirmed | Sanitized to `0..2` |
| maximum players | `V+0x74` | `i32` | confirmed | Sanitized to `0..16` |
| team-score aggregation mode | `V+0xa4` | `i32` | confirmed | Sanitized to `0..2`; used by team score updates |
| maximum teams | `V+0xb4` | `i32` | confirmed | Sanitized to `0..8` |

`team_play = ((read_u32(V+0x48) & 1) != 0)`. The active engine-index mirror is `read_i32(E+0xc14)`.

Engine indices, verified against the retail dispatch and matching Xbox engine implementations:

| Index | Variant |
|---:|---|
| 0 | none / campaign |
| 1 | Capture the Flag |
| 2 | Slayer |
| 3 | Oddball |
| 4 | King of the Hill |
| 5 | Race (deprecated) |
| 6 | Headhunter (deprecated) |
| 7 | Juggernaut |
| 8 | Territories |
| 9 | Assault |

### Cache identity (inline header at `0x00547700`)

| Field | Address / offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| cache type | `0x00547840` (`header+0x140`) | `i16` | confirmed | Cache-header validation |
| short map name | `0x00547898` (`header+0x198`) | `char[32]` | confirmed | Terminated-length validation at `0x00122870` |
| cache scenario path | `0x005478bc` (`header+0x1bc`) | `char[256]` | confirmed | Recovered cache header |
| scenario tag pointer | `0x004e0350` | `u32` | confirmed | Cache loader publication |
| active BSP tag pointer | `0x004e0348` | `u32` | confirmed | `0x0011c1b0` / `0x00122b40` |

For the web API, use the short map name plus the scenario path for disambiguation, and send `G+0x02` with every map/BSP change.

### Cinematics (cutscenes)

HaloScript's `cinematic_start` and `cinematic_stop` (builtins 554 and 555, evaluators `0x002a9740` and `0x002a9760`, `engine::points::CINEMATIC_START`/`STOP`) each call one engine function, `0x0013bff0` and `0x0013ca80`. Between them the scripts own the players: units are teleported, hidden or frozen, AI bipeds and vehicles are puppets, and scripted deaths happen. Telemetry reads one byte to know it is in one.

```text
C = read_u32(0x00510c50)        // cinematic block, 0x24 bytes, in the game state
playing = C != 0 && read_u8(C + 5) != 0
```

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|
| block pointer | `0x00510c50` | `u32` | confirmed | Allocated from the game-state allocator (cursor `0x004e6084`) by `0x0013bf00`, size `0x24` (`0x0013bf27`), zeroed and stored at `0x0013bf4e`: a checkpoint saves and restores it with the game state |
| cinematic in progress | `C+0x05` | `u8` | confirmed | `0x0013bff0` (`cinematic_start`) writes 1 to `+4` and `+5` (`0x0013c01b`, `0x0013c01e`); `0x0013ca80` (`cinematic_stop`) writes 0 to both (`0x0013caa9`, `0x0013cac2`). The accessor `0x0013cb40` returns `+5`, 0 when the pointer is null. Readers of `+5`: the campaign pause menu refuses to open (docs/game/settings-menu.md), the tick budget `0x00146660`, the camera projection's aspect (`0x0002fc96`, `0x0002fcba`), and more than a dozen others (`0x0001ded0`, `0x00025931`, `0x000311e5`, `0x00031361`, `0x000335c9`, `0x0004ac53`, `0x000d4e43`, `0x000e452f`, `0x0012e4b6`, `0x0014672f`, `0x0016ce4a`) |
| companion flag | `C+0x04` | `u8` | likely | Set and cleared with `+5` by the same two functions, but not only by them: the map reset `0x0013bf60` sets it to 1 in a campaign (options mode 1, `0x0013bfa6..0x0013bfb0`), and `0x0013bfc0` clears `+5` and `+0x22` always but `+4` only outside a campaign. So `+4` can be on with no cinematic playing; telemetry does not read it |

The same two functions also write the `0x84`-byte block at `[0x004de2f4]` (`+0x80` and `+0x81` set and cleared; builtins 208 and 209 set them from a boolean argument, the shape of `cinematic_show_letterbox` and its immediate form), the players globals' `+6`, the game globals' `+0x11f8` and the AI globals' `+0x20` (cleared by start, set by stop). Those are effects of a cinematic; `+5` above is the state the game itself tests. The main menu's background also runs `cinematic_start`; telemetry is only on in a game.

The agent API (`crates/extensions/agent`) and the co-op straggler fix (`crates/apps/boot/src/coop_players.rs`) instead observe the two HaloScript calls; the byte stays right across a checkpoint revert taken inside a cinematic, which observing the calls does not.

## 4. Players datum array

`players_initialize` at `0x0014b4b0` allocates **16 entries of stride `0x21c`** and publishes `P` at `0x004e8c24`. Element `i` is at `read_u32(P+0x44) + i*0x21c`. Iterate only below `P+0x38`, using the bitmap at `P+0x48` or the nonzero salt, and compose the current salted player datum from the salt and slot.

`player_iterator_next` at `0x0019f240` additionally skips records with flag bit 1 at `+0x02`; this is a useful “active player” filter for the scoreboard.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| datum salt | `+0x00` | `u16` | confirmed | Datum contract and player iterators |
| player flags | `+0x02` | `u16` | confirmed | Bit 0 connection state; bit 1 records are skipped by the engine iterator |
| originating machine address | `+0x14` | 6 bytes | confirmed | Creation at `0x0014c090` |
| originating machine index | `+0x1a` | `i16` | confirmed | `-1` for none |
| originating machine user | `+0x1c` | `i16` | confirmed | Creation/rebinding code |
| originating machine controller | `+0x20` | `i32` | confirmed | Creation/rebinding code |
| local controller index | `+0x24` | `i32` | confirmed | `-1` for remote/unbound |
| local user index | `+0x28` | `i16` | confirmed | `0..3` for local, `-1` for remote/unbound; maintained by `0x0014f190` |
| current unit object datum | `+0x2c` | `u32` | confirmed | Maintained by `0x0014cad0`; `0xffffffff` for no current unit |
| previous/deleted unit datum | `+0x30` | `u32` | confirmed | Not a substitute for `+0x2c` |
| active player name | `+0x44` | UTF-16LE `[32]` (64 bytes) | confirmed | Active configuration starts here; `0x0014be90` compares `0x20` code units |
| active team | `+0xc0` | `i8` | confirmed | `-1` for none; configuration/team bookkeeping |
| shadow/original configuration | `+0xd4` | 0x90 bytes | confirmed | `0x0014be90` copies the source here before verification |
| respawn delay | `+0x164` | `i32` | confirmed | Respawn update `0x0015dd40` |
| lives remaining | `+0x1ac` | `i16` | confirmed | `-1` means unlimited; death/round handling |

Read the name as exactly 32 code units and append a local terminator; do not rely on a terminator beyond the field. Actual names normally terminate earlier.

Derived values:

```text
local        = read_i16(player+0x28) != -1
has_unit     = read_u32(player+0x2c) != 0xffffffff
waiting/dead = read_u32(player+0x2c) == 0xffffffff
player_datum = (read_u16(player+0) << 16) | slot
```

There is no separate player-datum alive boolean to substitute for `+0x2c`. For object-level death state, object `+0x10a` bit 2 is body-depleted, but it can differ transiently from the current-unit link.

The local-user map is at `read_u32(0x004e8c20)+0x0c`: four `i32` salted player datums. The four local-controller mappings begin at `+0x1c`; local machine index is `+0x98`.

### Local player slots and profile appearance

The appearance fields a scoreboard wants (profile colours and emblem layers) are **not** pinned inside the `0x21c` player record; the configuration gap between the name (`+0x44`) and team (`+0xc0`) is the unvalidated candidate. What the same-hash decompilation does establish is where a signed-in profile lives, addressable through the confirmed local user index at `+0x28`:

| Field | Address / offset | Type | Confidence | Evidence |
|---|---|---:|---|---|
| local player slots | `0x0054e8e0` | 4 × `0xc70` bytes | likely | One slot per controller; `s_player_slot_view` |
| slot flags | `slot+0x00` | `u32` | likely | Bit 4 (`0x10`) signed in with a profile; bit 6 profile changed during a game |
| slot profile copy | `slot+0x18` | 0x1e0 bytes | likely | The signed-in `s_player_profile` copied here raw |
| appearance block | `slot+0x130` (`+0x18`+`0x118`) | 0x10 bytes | likely | `s_player_profile_appearance` in the profile |
| profile colours | `+0x130..+0x133` | 4 × `i8` | likely | Palette indices; 18 colours (string-id table at `0x0014986f`) |
| model | `+0x134` | `u8` | likely | Not needed for identity |
| emblem foreground | `+0x135` | `u8` | likely | 0..63; profile byte `+0x11d` as the settings list shows it (`0x00149b5a`) |
| emblem background | `+0x136` | `u8` | likely | 0..31; profile byte `+0x11e` (`0x00149da5`) |

Join: player record `+0x28` (confirmed, `0..3`) selects the slot. A slot without the signed-in flag, a remote player (`+0x28 == -1`), or any value outside its range reads as "no appearance". The player's rank/level is an Xbox Live attribute, not guest memory — the matchmaking attributes carry no rank — so nothing is read for it.

Remote players' profiles arrive over the network, not through the local slots, so their appearance is unknown until the in-record location is validated; the configuration gap `+0x84..+0xc0` (in both the active block and the shadow copy at `+0xd4`) is the candidate to check at runtime.

## 5. Game-engine statborg and scores

The stats are separate from the player datum. `E` points to a 0xc9c-byte game-engine global block. The inline statborg begins at `E+0x304` and is 0x254 bytes. It is usable only when the selected object at `0x0055e4d0[engine_index]` is nonnull and its valid field is set; campaign should not be assumed to have multiplayer stats.

```text
player_stat(i) = E + 0x308 + i*0x1c   // absolute player slot, 0..15
team_stat(t)   = E + 0x4c8 + t*0x12   // team index, 0..7
```

### Per-player stat row

All nine networked counters through `+0x10` are signed 16-bit values. Update helpers clamp them to `[-30000,30000]`.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| current-round score | `+0x00` | `i16` | confirmed | Raw score accessor `0x001587b0` |
| cumulative/multi-round score | `+0x02` | `i16` | confirmed | Round-boundary folding at `0x001e97b0` |
| kills | `+0x04` | `i16` | confirmed | `0x001ea5b0` updates field 2 |
| deaths | `+0x06` | `i16` | confirmed | `0x001ea770` updates field 3 |
| suicides | `+0x08` | `i16` | confirmed | `0x001ea770` updates field 4 |
| betrayals | `+0x0a` | `i16` | confirmed | `0x001ea770` updates field 5 |
| assists | `+0x0c` | `i16` | confirmed | `0x001e9fa0` updates field 6 after assist attribution |
| rounds won | `+0x0e` | `i16` | confirmed | Winner/round path `0x0015b3a0` |
| ninth networked stat | `+0x10` | `i16` | likely | Layout confirmed; semantics unresolved and not needed |
| consecutive-kill count | `+0x12` | `i16` | confirmed | `0x001ea5b0` / reset at `0x001ea770` |
| multiple-kill counter | `+0x14` | `i16` | likely | Layout and update confirmed; exact medal semantics are not needed |
| last-kill tick | `+0x18` | `u32` | confirmed | Written from `T+0x08` at `0x001ea5b0` |

For an individual score, read row `+0x00` directly. `0x0019fc70` deliberately returns the team score in a team game. Use `+0x02` if the UI wants a cumulative multi-round score.

### Per-team stat row

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| current-round team score | `+0x00` | `i16` | confirmed | `0x001587f0` and `0x0015b650` |
| cumulative team score | `+0x02` | `i16` | confirmed | Same round folding as players |
| team kills | `+0x04` | `i16` | confirmed | Same field-2 update helper |
| team deaths | `+0x06` | `i16` | confirmed | Same field-3 update helper |
| team suicides | `+0x08` | `i16` | confirmed | Same field-4 update helper |
| team betrayals | `+0x0a` | `i16` | confirmed | Same field-5 update helper |
| team assists | `+0x0c` | `i16` | confirmed | Same field-6 update helper |
| team rounds won | `+0x0e` | `i16` | confirmed | Winner/round path |
| ninth team stat | `+0x10` | `i16` | likely | Layout confirmed; semantics unresolved |

Useful team masks in `E`: present teams `+0x04`, playing teams `+0x0c`, team-active mask `+0x0e`, all `u16`. These are optional; player `+0xc0` is sufficient to group the scoreboard.

## 6. Objects, positions, facing, and vehicles

### Object-header array

`objects_initialize` at `0x000b67c0` creates **2,048 twelve-byte headers** and publishes `H` at `0x004e0300`. Header `i` is at `read_u32(H+0x44) + i*0x0c`.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| datum salt | `+0x00` | `u16` | confirmed | Generic datum contract and `0x000badc0` |
| header flags | `+0x02` | `u8` | likely | Xbox header layout and iterators; individual flag meanings are not needed |
| concrete object type | `+0x03` | `u8` | confirmed | Read by `0x000badc0` for its type mask |
| cluster/location index | `+0x04` | `i16` | likely | Matching Xbox layout; not needed for player markers |
| body size | `+0x06` | `u16` | confirmed | Stored by allocator `0x000bc280` |
| object-body pointer | `+0x08` | `u32` | confirmed | Allocation and every typed lookup |

Safe unit lookup, equivalent to the relevant checks in `0x000badc0`:

```text
reject datum == 0xffffffff
slot = datum & 0xffff
reject slot >= read_i32(H+0x38)
header = read_u32(H+0x44) + slot*0x0c
reject read_u16(header+0) == 0
reject read_u16(header+0) != (datum >> 16)
reject ((1 << read_u8(header+3)) & 0x3) == 0   // biped or vehicle
body = read_u32(header+8)
reject body == 0
```

`0x000badc0` itself has custom `EAX = object datum`, one stack type-mask argument, `ret 4`, and returns the body pointer or zero in `EAX`. Unit mask is `0x3` (`0` biped, `1` vehicle).

Do not cache body pointers between snapshots: the object memory pool can compact and rewrite the header's pointer (`docs/engine/memory_pool.md`). Never scan the one-MiB object pool for players; follow only player unit datums and, optionally, their parents.

### Common object/unit body

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| definition tag datum | `+0x00` | `u32` | confirmed | Object creation and tag consumers |
| parent object datum | `+0x14` | `u32` | confirmed | Transform composition and parent walks |
| bounding center | `+0x30` | `f32[3]` | confirmed | Not the scoreboard origin |
| object origin | `+0x64` | `f32[3]` | confirmed | Written by transform setter `0x000b7430` |
| forward/facing vector | `+0x70` | `f32[3]` | confirmed | Written by `0x000b7430`; used by `0x000ba160` |
| up vector | `+0x7c` | `f32[3]` | confirmed | Written by `0x000b7430` |
| translational velocity | `+0x88` | `f32[3]` | confirmed | Recovered velocity writer; world units a second, not a tick: the projectile update moves `+0x64` by it times the seconds per tick (`0x000f97df`..`0x000f9803`) |
| concrete object type | `+0xaa` | `u8` | confirmed | HaloScript/object type tests |
| body vitality | `+0xec` | `f32` | confirmed | `object_get_health` `0x002a1a20` (0 while body-depleted) |
| shield vitality | `+0xf0` | `f32` | confirmed | `object_get_shield` `0x002a1a90` (0 while body-depleted) |
| damage flags | `+0x10a` | `u16` | confirmed | Bit 2 is body-depleted, set by `0x000d6800` |
| unit's player datum | `+0x13c` | `u32` | confirmed | `0x0014de90` |
| previous player datum | `+0x140` | `u32` | confirmed | Unlink path `0x0014cad0` |
| vehicle seat index | `+0x1fc` | `i16` | confirmed | `unit_new` sets -1 (`0x000c44c1`); the enter-vehicle routine `0x000ea1f0` indexes the vehicle's seats block (stride `0xb0`) with it and stores it (`0x000ea405`); leaving stores -1 (`0x000ea995`) |

For an unparented unit, marker position is `body+0x64`, and heading is `atan2(forward.y, forward.x)`; the world is Z-up. `0x000ba160` shows that parented objects compose their local transform with a parent node matrix. Therefore, for a player in a vehicle, the cheap robust path is:

1. Read the unit's parent datum at `+0x14`.
2. Salt-check that object and require concrete type `1` (vehicle).
3. Use the vehicle body's `+0x64` origin and `+0x70` forward, and optionally the unit's `+0x1fc` seat.
4. If drawing a parented biped rather than its vehicle, compose the full object matrix instead of assuming `+0x64` is world-space.

### Unit aim, held weapon, species and shots

Added 2026-11-16 for the viewer's player models, from static reading of the
XBE with an iced-x86 disassembly (no game run). Offsets are into the unit
body (`H` lookup above); `W` is a weapon object body.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|
| current weapon index | unit `+0x212` | `i8` | confirmed | `unit_new` `0x000c4410` sets it to `-1`; `0x000c58f0`, `0x000c85c0` and `0x000cf3d0` sign-extend it and index `+0x218` with it |
| second weapon index | unit `+0x213` | `i8` | confirmed layout; likely dual-wield | Set to `-1` with `+0x212`; `0x000c85c0` loops over both bytes |
| weapon object datums | unit `+0x218` | `u32[4]` | confirmed | `unit_new` sets all four to none; network-state builder `0x0009d450` walks the four slots and reads each weapon's magazines |
| aiming vector | unit `+0x168` | `f32[3]` | likely | Layout confirmed: `unit_new` copies the object forward (`+0x70`) into `+0x150`, `+0x15c`, `+0x168`, `+0x180`, `+0x18c`. `0x000cf3d0` casts the held weapon's targeting ray (collision test `0x001697c0`) from the unit origin `+0x64` along `+0x168` times the weapon definition's range (`+0x1b8`), so it is the world-space aim. Whether it is the smoothed or the desired vector is open: `0x0009d450` networks `+0x15c` instead |
| weapon owner | `W+0x14` | `u32` | confirmed | The ordinary parent datum; `0x00101fb0` finds a weapon's owning unit through it (unit mask `3` at `0x000badc0`) |
| magazine state | `W+0x228 + 0x10*i` | 16 bytes | confirmed layout | `0x00101a10` sets reserve `+0x22a` and loaded `+0x22c` (`i16`) from the definition's magazine block (`weap+0x2c0` count, `+0x2c4` pointer, stride `0x5c`); `0x00101740` moves ammunition between weapons. Not used: plasma weapons have no magazine |
| barrel state | `W+0x1a4 + 0x34*i` | `0x34` bytes | confirmed layout | Byte `+4` bit 1 (`W+0x1a8`) is a pending-fire flag: `0x000fd980` fires each flagged barrel and clears the flag |

**Held weapon:** `index = (i8) unit[+0x212]`; for `0..3`, `weapon = unit[+0x218 + 4*index]`. Check the weapon datum like a unit (salt, high-water, non-null body), require concrete type `2`, and require `W+0x14 == unit datum`. The weapon's definition tag datum is `W+0`.

**Species and weapon names:** an object's definition tag datum (`body+0`) is the full salted datum of its tag-index entry. Tag *names* are, likely, not in guest memory: the cache loader's read (`0x00122a52` calling `0x00122d60`) is sized from the header's index/metadata fields (`0x00547710..0x0054771c`), and no instruction references the header's tag-name fields (`0x005479c0..0x005479cc`) by absolute address. The map file on disc has the same index, so the host reads the names there; a name table is trusted only when the file's scenario datum equals the guest's, read from the tag-index header at `[0x00547f00]+0x0c` (the loader reads it at `0x00122ad8` and publishes that tag's metadata pointer at `0x004e0350`). Multiplayer bipeds are named like `objects\characters\masterchief\masterchief_mp` and `objects\characters\elite\elite_mp`.

**Shots:** `0x00104150` is the barrel fire: stdcall `(weapon object datum, i16 barrel index)`, `ret 8`, with a `0x1e98`-byte frame. Its only direct caller is the pending-fire loop `0x000fd980` (reached from `0x000a2df5` and `0x000fd95e`). It resolves the owner through `0x00101fb0`, reports the shot to the game engine (`0x0015cca0` with the owner's player datum), and creates the projectiles of every firing marker of the barrel through `object_new` (`0x000b7b40`). One call per shot is **likely** (a shotgun's pellets are created inside one call; a battle-rifle burst should be three calls). A hook on it counts shots for every weapon type, including those without magazines, which a rounds-loaded delta (`W+0x22c`) would miss; the delta would also need reload and pickup filtering.

### AI bipeds, teams and vehicles

Added 2026-11-16 for campaign frames (static reading, no game run).

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|
| actor datum | unit `+0x12c` | `u32` | confirmed | `unit_new` `0x000c4410` sets it to none; `0x000c6810` indexes the actor datum array (`[0x004f55f0]`, element stride `0x888`) with it and copies the actor's team (`actor+0x24`, `i16`) into the unit |
| player datum | unit `+0x13c` | `u32` | confirmed | Section above; set to none by `unit_new` |
| team | unit `+0x138` | `i16` | confirmed | `unit_new` takes it from the placement (`placement+0x64`), and in campaign (`[G+8] == 1`) when that is 0 or -1 from the unit definition's default team (`+0xc0`); `0x0014bfc0` copies the player's team (`player+0xc0`); `0x000c6810` the actor's; `0x000b7930` hands `(i16)+0x138` on as the owner team of objects the unit creates |
| up vector | `+0x7c` | `f32[3]` | confirmed | Section above |
| campaign team names | — | enum | likely | 0 default, 1 player, 2 human, 3 covenant, 4 flood, 5 sentinel, 6 heretic, 7 prophet: the tag definitions' order, not checked against code |

An AI biped is a biped object (type 0) with an actor datum, no player datum,
and damage flags (`+0x10a`) without bit 2. A unit's vehicle is its parent
when the parent is type 1; the vehicle's own `+0x64/+0x70/+0x7c` are world
space unless the vehicle is itself parented (turrets on dropships).

Campaign re-activation: `0x0018e700` copies the active options (`G+8`,
`0x1118` bytes), re-selects the BSP and calls `0x0018f000` again, so the
activation hook also fires on checkpoint reverts and restarts of the same
map.

### Unit stance, actions, damage and machines

Added 2026-11-16 for frame `stance`, `actions`, `hits` and `machines`
(static reading, no game run). Enum value names not established by the
code cited are marked unverified.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|
| unit flags | unit `+0x134` | `u32` | confirmed | Bit 23 asks to crouch: the crouch update at `0x000dd83f` tests it |
| crouch amount | unit `+0x2c4` | `f32` 0..1 | confirmed | `0x000dd83f` moves it toward 1 (bit 23 set) or 0 by a rate times the tick length; `biped_new` initialises it |
| selected grenade type | unit `+0x23c` | `i8` | confirmed index; names unverified | `0x000cdff0` and the HUD (`0x002759cf`) index `+0x23e` with it; 0 frag, 1 plasma (unverified) |
| grenade counts | unit `+0x23e` | `i8[2]` | confirmed | As above; `0x000cbef3` tests each |
| biped physics mode | biped `+0x3dc` | `u8` | likely | `biped_new` (`0x000de5f6`) clears it; code compares it with 1, 2, 3, 5 and 6 (`0x000dd897` in the crouch update: 6). Value names unverified |
| translational velocity | `+0x88` | `f32[3]` | confirmed | Above; telemetry uses only the sign of z |
| held weapon magazine | `W+0x22a` / `W+0x22c` | `i16` | confirmed | Reserve and loaded rounds, above |
| scenario placement index | `+0x1a` | `i16` | confirmed | `object_create` by name (`0x000bb670`) passes the name's placement index to `0x000bf0f0`, which hands it to the placement-data builder `0x000d5060` (call `0x000bf152`; stored at placement data `+0x10`, `0x000d520b`); `object_new` copies placement data `+0x10` here (`0x000b7c87`) with the object identifier (`+0xa4`, `+0xa8`); -1 for objects the scenario did not place (`0x000b7c63`). Index into the object type's scenario block (docs/live/live-telemetry.md, "Scenario objects") |
| netgame equipment index | `+0xaf` | `i8` | confirmed | The netgame spawner `0x0015aad0` writes the spawn's index into `scnr+0x120` here (`0x0015ac15`) after `object_new`, which sets `0xff` (`0x000b7db1`); deletion (`0x000b85c3`) calls `0x0015b220` to clear the spawn's object |
| machine object type | header `+3` = 7 | `u8` | likely | The engine's type order (biped 0, vehicle 1, weapon 2 confirmed); the scarab of `03a`/`03b` is a `mach` tag, in `08a` a `scen` |
| equipment and projectile object types | header `+3` = 3, 5 | `u8` | confirmed | The type table at `0x00468630` (bipd 0, vehi 1, weap 2, eqip 3, garb 4, proj 5, ...; `layout::OBJECT_TYPE_*`). Frames send projectiles (`proj`, position `+0x64`, velocity `+0x88` as the projectile update moves them, `0x000f8ee2` on) and loose weapons and equipment (no parent, `+0x1a` and `+0xaf` both -1) from the world scan (packed version 6) |
| device position | device `+0x140` | `f32` 0..1 | confirmed | The machine network-state builder `0x000a3df0` copies it under flag `0x400`. The machine update at `0x0011546d` adds the tick's travel to it. The device code passes it to the normalized-frame setter `0x001c6620` at call site `0x00106930`. |
| device power | device `+0x134` | `f32` 0..1 | confirmed | The setter's caller at `0x00106945` and `0x0010695d` uses it to blend powered and depowered rates. The viewer needs only the resulting position. |
| device position channel | device `+0x18c` | animation channel | confirmed | `0x00108721` takes its address when starting the position action. The existing channel layout applies. Its graph datum is at `+0x18c` and its signed animation index at `+0x192`. The setter `0x001c6622` checks the index at channel `+6` for -1 and resolves the animation using the graph at channel `+0`. |
| damage reporting type | report `+0x00` low 6 bits | enum | likely | 6-bit value; value names unverified |

`0x000d9640` applies a report to its victim: it adds `+0x44` (clamped to 1
when report flag bit 0 is set) and `+0x48` to the victim's recent-damage
accumulators (`+0x100`, `+0xf8`), which is why the amounts read as
fractions of vitality. It has four callers (`0x0009c76e`, `0x000d8292`,
`0x000d87a2`, `0x000dc211`).

## 7. Kill/death and damage hooks

### Best player-facing event: `0x0015ce70`

Standard four-argument stack convention, callee pops 16 bytes:

| Argument | Stack offset at entry | Type | Meaning |
|---|---:|---|---|
| attacker player datum | `[esp+0x04]` | `u32` | Finally attributed killer; `0xffffffff` for none |
| victim player datum | `[esp+0x08]` | `u32` | Victim player |
| unknown flag | `[esp+0x0c]` | `u32` | **Not** a betrayal flag: it is set for free-for-all kills between enemies too (seen 2026-11-19, a Clankers free-for-all whose every kill was sent as a betrayal). Nothing reads it; a betrayal is a teammate killed in a team game (`live_proto::game::is_betrayal`). |
| packed damage code | `[esp+0x10]` | `u8` in low byte | Low 6 bits damage type, high 2 bits modifier |

The direct call is at `0x001ea460` inside attribution routine `0x001e9fa0`. It follows recent-damage attribution, so the killer is final. It is not universal for campaign AI or arbitrary units: a game engine/statborg and a victim player index are required.

Timing matters:

- At entry, an enemy killer's kill counter has already been incremented, but the engine's score callback near the end of `0x0015ce70` has not necessarily run.
- After `0x0015ce70` returns, the victim death/suicide/betrayal and assist accounting can still be pending in `0x001e9fa0`.
- Recommended: emit the event arguments immediately, then publish the authoritative stat rows at the next completed-tick sample. If a hook must see all death stats complete, wrap `0x001e9fa0` and emit after its return.

`0x001e9fa0` has five stack args, `ret 0x14`: statborg pointer, victim object datum, reported player datum, reported team (`i16` in the arg slot), and packed damage code. It performs final killer/assist attribution and all associated stat updates.

### Damage-aware alternative: `0x000d9640`

Custom ABI: `EDI = damage_report *`, one stack arg victim object datum, `ret 4`; void. Filter `(report.flags & 1) != 0` for the body-depleted transition.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| packed damage code | `+0x00` | `u8` | confirmed | Low 6 bits type, high 2 bits modifier |
| report flags | `+0x04` | `u32` | confirmed | Bit 0 body-depleted/death transition |
| damage-effect (`jpt!`) tag datum | `+0x08` | `u32` | confirmed | Resolved through the tag-entry table |
| reported owner player datum | `+0x0c` | `u32` | confirmed | Copied to unit owner state |
| reported owner object datum | `+0x10` | `u32` | confirmed | Used for owner/current-weapon paths |
| reported owner team | `+0x14` | `i16` | confirmed | Team/hostility handling |
| direction | `+0x18` | `f32[3]` | confirmed | Damage report consumers |
| origin | `+0x24` | `f32[3]` | confirmed | Damage report consumers |
| body-damage amount | `+0x44` | `f32` | likely | Body-vitality update path |
| shield-damage amount | `+0x48` | `f32` | likely | Shield-vitality update path |
| distance | `+0x4c` | `f32` | confirmed | Damage report layout |

The `jpt!` datum is the most precise damage identity here; it is not a weapon tag. Current-weapon lookup through the reported owner is only corroboration and is not reliable for grenades, delayed projectiles, vehicles, melee, or recent-damage attribution. Use `0x0015ce70` for final killer identity.

For arbitrary object death without player attribution, `0x000d6800` is the body-depleted transition. It takes four stack args (`object datum`, `damage_owner *`, `notify_parent`, additional flag), `ret 0x10`. The owner is player datum `+0`, object datum `+4`, team `i16 +8`; it has no damage code or damage tag.

## 8. Structure BSP and a web top-down map

### Scenario BSP selection

The scenario's structure-BSP block is at `scnr+0x210` (count/pointer), with records of stride `0x44`.

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| scenario BSP block count | `scnr+0x210` | `u32` | confirmed | `0x0011c1b0` and viewer decoder |
| scenario BSP block pointer | `scnr+0x214` | `u32` | confirmed | Same |
| BSP tag datum in record | `record+0x14` | `u32` | confirmed | Loaded by `0x00122b40` |
| lightmap tag datum in record | `record+0x1c` | `u32` | confirmed | Loaded by the same switch |

Use `G+0x02` to select the active record. Do not assume BSP zero; campaign maps switch BSPs. `0x004e0348` is the active resolved BSP tag pointer and can be used as a consistency check.

### Geometry needed for bounds and rendering

| Field | Offset | Type | Confidence | Evidence |
|---|---:|---|---|---|---|
| cluster block | `sbsp+0x9c` | block, stride `0xb0` | confirmed | Engine `0x00041040` and `StructureRender` |
| material block | `sbsp+0xa4` | block | confirmed | Part material validation |
| instanced definitions | `sbsp+0x138` | block, stride `0xc8` | confirmed | Engine and decoder |
| instanced placements | `sbsp+0x140` | block, stride `0x58` | confirmed | Engine and decoder |
| cluster world bounds | `cluster+0x54` | `f32[6]` | confirmed | `[xmin,xmax,ymin,ymax,zmin,zmax]`; whole-disc vertex checks |
| instance uniform scale | `placement+0x00` | `f32` | confirmed | Whole-disc placement validation |
| instance forward/left/up basis | `placement+0x04/+0x10/+0x1c` | three `f32[3]` | confirmed | Orthonormal/right-handed checks |
| instance translation | `placement+0x28` | `f32[3]` | confirmed | World placement checks |
| instance definition index | `placement+0x34` | `i16` | confirmed | Engine `0x000410c5` |

Instance transform: `world = translation + scale * (x*forward + y*left + z*up)`.

There is no single decoded BSP world AABB in the repository. For exact bounds, reduce every vertex of every world-space triangle, including transformed instances. Cluster bounds are useful per-cluster approximations, not a substitute for that reduction.

### Reusing the viewer for the web

`crates/tools/viewer/src/bin/bsp.rs:591` already has `world_triangles`, which builds all world-space triangles for one BSP:

- cluster positions are already world-space (`:613-616`);
- definitions are loaded once and transformed per placement (`:618-626`);
- drawn-part filtering and triangle expansion are already implemented (`:600-610`).

The public lower-level APIs are:

- `StructureRender::read` — `crates/formats/tags/src/structure.rs:288`;
- `GeometryHeader::load` — `crates/formats/tags/src/structure.rs:135`;
- `Section::vertices(None)` — `crates/formats/tags/src/geometry.rs:928`;
- `Section::part_triangles` — `crates/formats/tags/src/geometry.rs:917`;
- `InstancedGeometryInstance::transform` — `crates/formats/tags/src/structure.rs:213`.

Two practical export routes:

1. **Lowest effort:** run the existing `bsp --render` path offline and ship its top-down PNG plus the world-to-image scale/offset. The renderer looks down Z and culls faces with `normal.z <= 0` (`crates/tools/viewer/src/bin/bsp.rs:728-746`). Its framing is the 1st–99th percentile of triangle centroids, not exact world bounds (`:707-725`), so export those computed framing values rather than recomputing an AABB on the server.
2. **Vector/interactive:** refactor the private `world_triangles` logic into an offline exporter, project drawn triangles to XY, and emit quantized triangles, an occupancy raster, or boundary edges as SVG/JSON. This currently has no triangle/SVG/GeoJSON/outline CLI. Use exact vertex bounds if the vector output should include everything; optionally retain the renderer's back-face/percentile policy to hide the enclosing shell.

Precompute every scenario BSP offline. Runtime traffic then needs only `{map, bsp_index}` and player XY/heading, not live BSP geometry.

## 9. Recommended sampling plan

### Hook and publication cadence

1. On successful `0x0018f000`, create a session and read `O`, `V`, cache identity, `G+0x02`, and `T`. Start only options types `1` and `2`.
2. On every `0x00137fe0` return, check `T+0x08`. If unchanged, do nothing. If changed:
   - read `G+0x02`, `G+0x1120`, and `G+0x1128`;
   - sample player/stat rows every tick if desired, or every 2–3 ticks for the web;
   - resolve each live player's current unit and optional vehicle parent;
   - detect map/BSP changes and publish them with the next snapshot.
3. Hook `0x0015ce70` for immediate attributed kill events. Reconcile event scores from the next completed-tick stat snapshot.
4. On the `G+0x1128` transition, publish final scores and stop movement updates. Use `0x00232d77` return only if a separate finalized-PGCR message is wanted.
5. Treat pointer loss, a new activation, or options type `3` as session teardown; do not continue using cached `P`, `H`, `E`, or object-body pointers.

A good default is **positions at 15 Hz, scoreboard at 10 Hz, kill events immediate**, while retaining the completed-tick hook so no event is lost. Sampling every tick is also inexpensive locally.

### Per-tick reads and approximate cost

Read only live players and their units:

| Data | Read size / work |
|---|---|
| time and lifecycle | A few words from `G`, `T`, and `E` |
| player table metadata | High-water/data/bitmap; 16-slot bitmap is tiny |
| player identity/state | `+0x02`, `+0x24..+0x30`, name at `+0x44`, team at `+0xc0`; names can be sent only on change |
| player and team stats | At most 16×0x1c + 8×0x12 = 592 bytes |
| one unit per player | 12-byte header, parent datum, origin/forward/up, type, and optional damage flags; re-read body pointer each time |
| one vehicle per occupied player | Another 12-byte header and transform, only when parent type is vehicle |

Selective contiguous reads are roughly **2–4 KiB per full 16-player snapshot**, or about **12 KiB** if all 16 complete `0x21c` player records are read unnecessarily. This is a few hundred scalar reads, negligible beside a game tick. Avoid reading all 2,048 headers or the one-MiB object pool. Quantize positions/headings for transport and send names/options only when changed.

Consistency rules:

- Use datum high-water and salt checks; never use only a nonzero body pointer.
- Statborg rows use absolute player slots; event arguments carry salted player datums.
- Use current-round score `+0x00` for the ordinary live scoreboard and cumulative score `+0x02` only when wanted.
- For team games, send both individual and team scores; do not substitute the team-aware score accessor for individual stats.
- Campaign may lack a valid statborg; player identity, current unit, and positions still work.
- Publish kill events with tick number and let the next stat snapshot be authoritative if an event arrives during stat mutation.

## 10. Open items / first runtime checks

- All findings here are static. Validate one local Slayer match and one campaign map before enabling the service by default; then check system-link/Live sessions, whose runtime paths were not traced.
- Confirm the name field against real gamertags and variant names; storage is confirmed as 32 UTF-16 code units.
- Check vehicle-passenger transforms against gameplay. Generic parented-object composition is known, but only the cheap vehicle-parent path is recommended here.
- Persistent winner storage and normal-end/quit reason are not recovered. `0x0015b3a0` supplies a transient prospective winner, while `0x001389c0` supplies only the end latch.
- PGCR's detailed output structure was not mapped; only its construction hook is included.
- A direct causative weapon tag is not available at `0x0015ce70`. Correlate `0x000d9640`'s `jpt!` damage datum if damage-effect identity is sufficient.
- Options types other than `1`, `2`, and `3`, the ninth stat counter, and exact original names for game-engine state values remain unresolved and are not needed for the initial scoreboard.
- Unit aim and shots are static-analysis-only: check that `+0x168` follows the reticle (not lagging it, which would point at `+0x15c`) and that one `0x00104150` call is one shot for automatic, burst, spread and plasma weapons.
- The appearance path above is static-analysis-only: validate the local player-slot/profile offsets against the profile editor, and find where remote players' appearance lives in the `0x21c` record (candidate: the `+0x84..+0xc0` configuration gap). Rank is Xbox Live state and is not recoverable from guest memory.

## References

- Local anchors and layouts: `docs/engine/engine-map.md`, `docs/engine/datum_new.md`, `docs/engine/memory_pool.md`, `docs/formats/structure-bsp.md`, `crates/hooking/guest-layout/src/datum.rs`, `crates/formats/tags/src/structure.rs`, `crates/formats/tags/src/geometry.rs`, `crates/tools/viewer/src/bin/bsp.rs`.
