# Halo 2 retail Xbox live game-state map

Static-analysis deliverable for a live scoreboard and top-down player map. All addresses are **guest virtual addresses** in the retail 1.0 XBE, not host pointers. All integer fields are little-endian.

Function and field names are assigned descriptions.

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
| total header size | `0x4c` | bytes | confirmed | Generic initialiser |

A salted datum is `(salt << 16) | absolute_slot`; `0xffffffff` is none. For players and objects, the first `u16` of each element is its salt and zero means free. Statborg rows and BSP indices use the **absolute slot**, not the full salted datum.

## 2. Lifecycle and hook points

### Recommended hooks

| Event | Entry VA and ABI | Hook timing and confidence |
|---|---|---|
| Map activation / `GameStarted` | `0x0018f000`; custom `EAX = options`; no stack args; success in `AL` | **confirmed.** Call the original and emit only when it returns `AL != 0`. It copies active options, runs new-map callbacks, selects the initial BSP, and sets the ready flag. Entry alone is too early. The successful return tail is `0x0018f086`/`0x0018f08a`. |
| Higher-level successful start | `0x0018e8b0`; one stack arg `options` at `[esp+4]`, `ret 4`; success in `AL` | **confirmed.** Alternative wrapper around cache acquisition plus activation. Caller site `0x0018e91e` is after successful activation. Do not emit from `0x0018eaa0`: that is cache/resource acquisition only. |
| Completed simulation tick | `0x00137fe0`; no args; void | **confirmed.** Call the original, then sample only if `T+8` changed. `0x001380f3` immediately precedes the increment; `0x001380fb` is a common tail also reached without an increment, so an exit hook must compare the counter. |
| Game-ended latch | `0x001389c0`; no args; void | **confirmed.** Idempotently sets `G+0x1128` and initialises `G+0x112c`. Emit on return or on a `0 -> 1` transition at the completed-tick hook. Final score/winner resolution has run; quit/normal-end reason is not encoded in this function. |
| Post-game carnage report built | `0x00232d77`; no args; void; common return `0x002331a0` | **confirmed.** Optional hook for a finalized PGCR snapshot. `0x00196390` tail-jumps here from the game-end path. At entry the source stats exist but report construction is unfinished; use return or caller site `0x0015cb8d`. |
| Winner/round resolution (optional) | `0x0015b3a0`; custom `EBX = prospective winner`; one unknown stack arg, `ret 4`; void | **confirmed ABI; limited use.** Team index in team games, player datum in FFA, `-1` for none/tie. Also handles rounds, so this is not a standalone match-ended event. Correlate it with the later end latch if winner capture is needed. |
| Player death/kill credit | `0x0015ce70`; four stack args, `ret 0x10`; void | **confirmed.** See section 7 for arguments and score-update timing. |

`0x0018e8b0` uses ordinary stack argument order. `0x0018f000` and several simulation functions use LTCG register ABIs; do not infer their prototypes from a generic decompiler.

### Simulation lifecycle and tick envelope refinement

The addresses in `docs/engine/engine-map.md` are real, but `0x00067e30` is initialisation, not match start.

| Function | ABI | Role | Confidence |
|---|---|---|---|
| `0x00067e30` | no args; void | Simulation/network-global initialisation callback | confirmed |
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
| seconds per tick | `T+0x04` | `f32` | confirmed | Recovered game-time initialisation |
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

The agent API (`crates/extensions/agent`) and the co-op straggler fix (`crates/extensions/game/src/coop_players.rs`) instead observe the two HaloScript calls; the byte stays right across a checkpoint revert taken inside a cinematic, which observing the calls does not.

## 4. Players datum array

`players_initialize` at `0x0014b4b0` allocates **16 entries of stride `0x21c`** and publishes `P` at `0x004e8c24`. Element `i` is at `read_u32(P+0x44) + i*0x21c`. Iterate only below `P+0x38`, using the bitmap at `P+0x48` or the nonzero salt, and compose the current salted player datum from the salt and slot.

`player_iterator_next` at `0x0019f240` also skips records with flag bit 1 at `+0x02`; this is a useful “active player” filter for the scoreboard.

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

Join: player record `+0x28` (confirmed, `0..3`) selects the slot. A slot without the signed-in flag, a remote player (`+0x28 == -1`), or any value outside its range reads as "no appearance". The player's rank/level is an Xbox Live attribute, not guest memory - the matchmaking attributes carry no rank - so nothing is read for it.

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

### Medals

Every medal the game awards goes through one function, `0x001970a0`
(`GAME_MEDAL`). It takes its arguments in registers: the medal index in
EAX, the player's absolute slot in ECX and a count in EBX (always 1). It
adds to the post-game statistics when they are on (`[0x510ca0]` set and
`[0x510ca1]` clear); the call itself is made either way.

The index is a row of the medal table at `0x0046e120`. Its 24 rows are
the game's medal ids, in this order:

| Index | Id | Index | Id | Index | Id |
|---:|---|---:|---|---:|---|
| 0 | `double-kill` | 8 | `bonecracker` | 16 | `berserker` |
| 1 | `triple-kill` | 9 | `stealth-kill` | 17 | `overkill` |
| 2 | `killtacular` | 10 | `vehicle-kill` | 18 | `flag-taken` |
| 3 | `kill-frenzy` | 11 | `carjack` | 19 | `flag-save` |
| 4 | `killtrocity` | 12 | `stick-it` | 20 | `flag-returned` |
| 5 | `killimanjaro` | 13 | `killing-spree` | 21 | `bomb-planted` |
| 6 | `sniper-kill` | 14 | `running-riot` | 22 | `bomb-save` |
| 7 | `roadkill` | 15 | `rampage` | 23 | `bomb-diffused` |

The callers are the kill medals from the kill routine `0x0015ce70`
(`0x15d45f` to `0x15d524`), Carjack (`0x15e8d2`) and the flag and bomb
medals (`0x240f59` on). Live's copy of the table is `OFFICIAL` in
`crates/live/live-proto/src/film/events/medals.rs`; the telemetry
observer turns each call into a `GameMedal` event
([live-api.md](live-api-theater-stats.md#medals)).

## 6. Objects, positions, facing, and vehicles

Moved to [Live game state: objects, hooks and the structure map](live-gamestate-objects.md#6-objects-positions-facing-and-vehicles).

### Object-header array

### Common object/unit body

### Unit aim, held weapon, species and shots

### AI bipeds, teams and vehicles

### Unit stance, actions, damage and machines

## 7. Kill/death and damage hooks

Moved to [Live game state: objects, hooks and the structure map](live-gamestate-objects.md#7-killdeath-and-damage-hooks).

### Best player-facing event: `0x0015ce70`

### Damage-aware alternative: `0x000d9640`

## 8. Structure BSP and a web top-down map

Moved to [Live game state: objects, hooks and the structure map](live-gamestate-objects.md#8-structure-bsp-and-a-web-top-down-map).

### Scenario BSP selection

### Geometry needed for bounds and rendering

### Reusing the viewer for the web

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

Selective contiguous reads are roughly **2–4 KiB per full 16-player snapshot**, or about **12 KiB** if all 16 complete `0x21c` player records are read unnecessarily. This is a few hundred scalar reads, negligible beside a game tick. Avoid reading all 2,048 headers or the one-MiB object pool.

Quantize positions/headings for transport and send names/options only when changed.

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
- The medal award function `0x001970a0` is static-analysis-only: check one double kill and one Carjack arrive as `GameMedal` events with the right slot.
- PGCR's detailed output structure was not mapped; only its construction hook is included.
- The kill callback identifies the credited damage reporting type. Distinguishing two modded weapons sharing one reporting type would require resolving the individual cause.
- Options types other than `1`, `2`, and `3`, the ninth stat counter, and exact original names for game-engine state values remain unresolved and are not needed for the initial scoreboard.
- Unit aim and shots are static-analysis-only: check that `+0x168` follows the reticle (not lagging it, which would point at `+0x15c`) and that one `0x00104150` call is one shot for automatic, burst, spread and plasma weapons.
- The appearance path above is static-analysis-only: validate the local player-slot/profile offsets against the profile editor, and find where remote players' appearance lives in the `0x21c` record (candidate: the `+0x84..+0xc0` configuration gap). Rank is Xbox Live state and is not recoverable from guest memory.

## References

- Local anchors and layouts: `docs/engine/engine-map.md`, `docs/engine/datum_new.md`, `docs/engine/memory_pool.md`, `docs/formats/structure-bsp.md`, `crates/hooking/guest-layout/src/datum.rs`, `crates/formats/tags/src/structure.rs`, `crates/formats/tags/src/geometry.rs`.
