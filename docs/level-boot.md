# Level boot: start a campaign level directly

`h2 play disc/halo2.iso --level 03a_oldmombasa [--difficulty heroic]`, or
`H2_LEVEL=03a_oldmombasa [H2_DIFFICULTY=heroic]` for `h2 run` (and `h2
play`), loads that campaign level in place of the main menu: no title
screen, profile choice or menus. The difficulty is `easy`, `normal` (the
default), `heroic` or `legendary`. The game gets one default local player,
as it does for the main menu itself. Code: `crates/boot/src/level_boot.rs`.

`H2_PERF_FROM=level` (with an `h2` built with `--features perf`) opens the
profiler session at the first present after the level loaded; with no
level requested it waits for the first game that is not the main menu.

## What a run says

```
play: starting 03a_oldmombasa on heroic (scenarios\solo\03a_oldmombasa\03a_oldmombasa)
level boot: requesting 03a_oldmombasa heroic (scenario ..., campaign C, map M; in place of the main menu, reason 0)
level boot: options queued
level boot: 03a_oldmombasa loaded at present N
perf: session from present N+1, the first after 03a_oldmombasa loaded
```

The `play:` line is `h2 play` only. A name the disc lacks stops before the
build with the disc's campaign map names, one per line. If the globals'
level list does not name the scenario, a `level boot:` line says so and the
campaign and map ids stay -1 (the options' own default). A game that
activates with another scenario prints `level boot: <map> activated (mode
N), not the requested level`.

## How the game starts its first map

Static research in the retail XBE (`h2 disasm`). The main loop
(`0x12b450`) calls `request_mainmenu(0)` whenever the game state
(`0x4ed294`) is neither 2 nor 5 and no options are pending (`0x4ed39d`):
at boot that is the first map the game asks for. `request_mainmenu`
builds game options on its stack, clears and defaults them, writes mode 3
and the main menu's scenario path, adds one default local player and
queues them. The pending transition (`0x18e810`) then tears down, loads the
cache and activates the game.

The main menu's campaign start (CAMPAIGN, level, difficulty) is
`0x23643f`: it builds the same options with mode 1, the campaign and map
ids and difficulty from the menu's selection globals (`0x54e7c0`,
`0x54e7c4`, `0x54e7c8`), the scenario path from the level list
(`0x19c270`), players from the signed-in profiles (`0x23654b`), and queues
them through the same `0x18e790`.

The level boot answers the first `request_mainmenu` call itself: the
game's own clear-and-default, the fields `0x23643f` writes for one player,
the game's own default player (as `request_mainmenu` does), the game's own
queue, then `ret 4` with ESI preserved. Everything after (cache slots,
`single_player_shared.map`, new-map callbacks, BSP 0) is the engine's
normal path. Later calls go to the original, so a failed level load still
falls back to the main menu.

The loaded mark wraps activation: after the original returns AL=1, the
game globals' active options name the scenario that loaded.

### Addresses

| Address | Name here | Use |
| --- | --- | --- |
| `0x0012b450` | main loop iteration | calls `request_mainmenu(0)` at `0x12b482` when no game runs and none is pending |
| `0x0018f1c0` | `REQUEST_MAINMENU` | `request_mainmenu(reason)`, stdcall, `ret 4`; **hooked**: the first call builds the level's options instead |
| `0x00138110` | `OPTIONS_NEW` | clear the `0x1118`-byte options (ESI) and default them: mode 1, difficulty 1, ids -1, tick rate, seed |
| `0x00138640` | `OPTIONS_PLAYERS` | stdcall `(count, options)`, `ret 8`: `count` default local players (`+0x268`, `+0x2cc`, entries at `+0x2d4`, `0xe4` each) |
| `0x0018e790` | `OPTIONS_QUEUE` | copy options (EAX) to the pending block `0x4ed3a8`, set pending `0x4ed39d` |
| `0x0023643f` | menu campaign start | the menu's own campaign launch; the fields we write follow it |
| `0x0019c270` | level lookup | `(campaign, map)` to the globals' level record |
| `0x0018f000` | `GAME_ACTIVATE` | activation, options in EAX, AL=1 on success; sets globals `+0x1120`; **hooked** (original, then the loaded mark) |
| `0x004e6948` | `GAME_GLOBALS` | game globals pointer: active options at `+8`, active byte at `+0x1120` |
| `0x004e034c` | globals tag pointer | `matg`; `+0x170` is the campaign level list `0x19c270` reads |

| Options field | Offset | Written |
| --- | --- | --- |
| mode | `+0x0` | 1 campaign (3 main menu, 2 multiplayer) |
| campaign id | `+0x14` | from the level list, else -1 |
| map id | `+0x18` | from the level list, else -1 |
| scenario path | `+0x1c`, `0x104` bytes | the cache header's scenario path |
| difficulty | `+0x12a` (word) | 0 easy, 1 normal, 2 heroic, 3 legendary |
| more than one player | `+0x12c` (byte) | 0 |
| its inverse | `+0x12d` (byte) | 1 |

The campaign and map ids are read at launch from the level's own `matg`
(else `mainmenu.map`'s): block `+0x170`, element 0, whose first field is a
`(count, address)` block of `0x108`-byte levels: `+0` campaign id, `+4`
map id, `+8` scenario path. The level names on the disc are the campaign
maps' file names (`h2 maps` lists them; cache type Campaign).

Our guest memory: `0xa2f40000` (the activation original's alias, RX) and
`0xa2f41000` (the options we build, `0x2000` RW).

## Unverified

Proved by tests over a fake guest (the options fields, the globals read,
the level match, the perf trigger, the level-list read, the name lookup)
and the CLI parsing tests. Not yet run against the game. The open question
for the first run is whether anything the main menu sets up (UI globals,
the signed-in profile, the selection globals `0x54e7c0`-`0x54e7c8`, which
we leave alone) matters to a level started without it. A default
profile's settings (look sensitivity, layout) are whatever the game holds
before any profile is chosen.

The level-list layout at `matg +0x170` is read from the code of `0x19c270`
and checked against the retail disc by the ignored test
`level_boot::tests::every_disc_level_resolves` (every campaign map
resolves to campaign 1 and a map id; `H2_DISC=... cargo test -p boot --lib
level_boot -- --ignored --nocapture`). Resolving reads the disc's map
headers at launch, about a second.
