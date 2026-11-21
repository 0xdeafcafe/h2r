# Retail game engine vtables: evidence

Evidence for [rules-engine.md](../../game/rules-engine.md) ("Which engine
index a script rides", "The host runtime"), [bots.md](../../game/bots.md)
section 3.7 and [game-options.md](../../game/game-options.md). Nothing here
states what the code does today; the design pages do.

## 2026-11-21: the engine table, the seven vtables and the slots that make rules

Commit `f89d0aa6d`; retail 1.0 `default.xbe`. Static reading only: `h2
disasm`, `h2 analyze --explain`, and dwords read from the XBE's sections.
No run.

**Reproduce.** Instructions: `h2 disasm <addr> <count>`. Dwords (vtable
slots, the engine table's sources): `h2 disasm` decodes data too, but a
reader is clearer; this one maps a virtual address through the XBE section
headers:

```python
import struct, sys
d = open(sys.argv[1], 'rb').read()
base, = struct.unpack_from('<I', d, 0x104)
n, sh = struct.unpack_from('<II', d, 0x11c)
def rd(a):
    for i in range(n):
        _, va, vs, ra, rs = struct.unpack_from('<5I', d, sh - base + i * 0x38)
        if va <= a < va + vs:
            return struct.unpack_from('<I', d, ra + a - va)[0] if a - va + 4 <= rs else 0
a, k = int(sys.argv[2], 16), int(sys.argv[3])
for i in range(k):
    print(hex(a + 4 * i), hex(rd(a + 4 * i)))
```

`python3 -I words.py disc/default.xbe 0x45c6d8 52` prints Slayer's vtable.
Call sites of the table were found by a byte search of `.text` for the
table's address (`d0 e4 55 00`, 287 hits), each decoded with `h2 disasm`
and followed to its first indirect call.

### The engine table holds objects; `+0` of each is the vtable

`0x378500` fills the table from pointers in `.data`:

```text
0x00378500  mov ecx,[475AF0h]
0x00378511  mov [55E4D8h],ecx        ; index 2
0x00378535  mov [55E4D4h],eax        ; index 1, eax = [470A5Ch]
0x00378546  mov [55E4F4h],eax        ; index 9, the same object
0x0037854b  mov dword ptr [55E4F8h],0
```

The sources are object addresses, and each object's only dword is its
vtable:

| Index | Table slot | Object | Vtable |
|---|---|---|---|
| 1 CTF, 9 Assault | `0x55e4d4`, `0x55e4f4` | `0x47f788` | `0x459d18` |
| 2 Slayer | `0x55e4d8` | `0x47fc80` | `0x45c6d8` |
| 3 Oddball | `0x55e4dc` | `0x47fc84` | `0x45c7a8` |
| 4 KotH | `0x55e4e0` | `0x47fc88` | `0x45c878` |
| 7 Juggernaut | `0x55e4ec` | `0x47d8f0` | `0x453d38` |
| 8 Territories | `0x55e4f0` | `0x47fc8c` | `0x45c948` |

Indexes 0, 5 and 6 are never written (zero in `.bss`); 10 is written 0.
Every dispatch has the same shape (`PLAYER_KILLED`'s last call):

```text
0x0015d672  mov ecx,[4E9AE8h]
0x0015d678  mov edx,[ecx+0C14h]          ; E+0xc14, the engine index
0x0015d67e  mov ecx,[edx*4+55E4D0h]      ; ECX = the engine object
0x0015d689  mov eax,[ecx]                ; its vtable
0x0015d6a4  call dword ptr [eax+78h]
```

So every slot is thiscall: ECX the engine object (no body found reads
it), arguments on the stack, the callee pops them.

**The `0x88` discrepancy.** `0x45c650` and `0x453cb0` are not engine
vtables. Their slots 1 and 2 are `0x22e315` and `0x22e335` and slot 10 at
`0x45c678` is `0x2300ea`, the generic UI screen event handler
([skulls.md](../../game/skulls.md)); the string `squad setting list`
follows at `0x45c6c0`. `0x45c650 + 0x114` is `0x45c764`, which is Slayer's
`0x45c6d8 + 0x8c`; `0x453cb0 + 0x114` is `0x453dc4`, Juggernaut's
`0x453d38 + 0x8c`. The trait query is slot `+0x8c` of the vtables in the
table, and it does have call sites, through the slot.

### Layout

Each vtable is 51 slots (`0xcc` bytes), then a zero dword (Slayer's at
`0x45c7a4`). Shared bodies fill most slots: `0x175f40` (`ret`),
`0x72c70` (`ret 4`), `0x24dc50` (`ret 8`), `0xa45d0` (`mov al,1; ret 4`),
`0xa5750` (`mov al,1; ret 8`), `0x24dc40` (`mov al,1; ret`).

| Slot | Dispatched from | Stack args, pop | What it is | Slayer | Others |
|---|---|---|---|---|---|
| `+0x00` | `0x15af29` (in `0x15ae70`), `0x15f498`, `0x19ef88` (spawn's starting weapons) | none | a per-engine code, EAX | `0x9bec0` returns 2 | CTF/Assault `0x240140` 1 or 9 from `[G+0x180]`, Oddball 3, KotH 0, Juggernaut 7, Territories 8 |
| `+0x04` | `0x1575bc` in the engine start `0x1574c0` | none | start the engine; false leaves `E+0xc14` 0 | `0x2bbf50` clears `E+0xfc..+0x108`, stores `E+0xfc` at `0x51ecc0`, true | own bodies |
| `+0x10` | `0x1579bb` (`0x157970`, team games, per team) | 1, 4 | per team at start | `0xa45d0` | |
| `+0x14` | `0x157c3d` (`0x157be0`, from the player add `0x14bc00`) | player, 4 | player added | `0x2bbf70` writes player `+0x1b8` = -1 | |
| `+0x18` | `0x14d34b` in the player update `0x14d2a0` | player, 4 | the player's machine joined (flag bit 0 set) | `0x2bbfa0` posts an event (`0xa7c50`, `0x19eb30`) | |
| `+0x1c` | `0x157e2f` (`0x157de0`, from `0x152df0`, the player set change) | player, 4 | not read | `0x72c70` | |
| `+0x24` | `0x158025` (`0x158000`, from `0x14c320`) | player (EAX), EBX; 8 | not read | `0x2bc110` walks the players | |
| `+0x3c` | `0x15c247` in `ENGINE_UPDATE` `0x15c150`, per player | player, 4 | per-player update | `0x72c70`, nothing | CTF/Assault `0x2403b0` (captures: `0x240711` calls `0x242e00`), Oddball `0x2bc920`, KotH `0x2bd2c0`, Juggernaut `0x192420`, Territories nothing |
| `+0x48` | `0x15c2dc` in `ENGINE_UPDATE` | none | per-tick update | `0x175f40`, nothing | CTF/Assault `0x240d70`, Oddball `0x2bcbb0`, KotH `0x2bd680`, Juggernaut `0x192530`, Territories `0x2c0230` |
| `+0x4c` | `0x15c063` in `0x15c000`, per player | player, 4 | a float returned in ST(0), clamped into player `+0x19c` | `0x24dd50`, the constant `[0x45dbc0]` | |
| `+0x54`, `+0x58` | `0x15e105` (`0x15e050`), `0x15e1b3` (`0x15e130`) | 1, 4 | objective registered, unregistered | `0x72c70` | |
| `+0x5c`, `+0x60` | `0x15e2f0` (`0x15e250`), `0x15e4c1` (`0x15e460`) | 2, 8 | objective picked up, dropped | `0x24dc50` | |
| `+0x64` | `0x1624c9` (`0x162470`, the time limit) | ticks left, EBX, 1; 12 | the ticks left, EAX | `0x24dd40` returns its first argument | |
| `+0x6c` | `0x15cf75` in `PLAYER_KILLED`; `0x15d735`, `0x23b9bd`, `0x2bc89e` | team, team; 8 | true when the two are enemies | `0x24dd70`: teams differ | Juggernaut `0x192670` |
| `+0x78` | `0x15d6a4`, last in `PLAYER_KILLED` | attacker, victim, flag, kind; 16 | the kill callback | `0x2bc000` | below |
| `+0x7c` | `0x15d323` in `PLAYER_KILLED` | 3, 12 | a replacement for a value `PLAYER_KILLED` keeps, -1 none | `0x24dd90` returns -1 | CTF/Assault `0x240f90` |
| `+0x80` | `0x23bc89` (`0x23bc40`, from `PLAYER_SPAWN_LOCATION` `0x14ef00`) | 2, 8 | spawn-location weighting | `0x24dc50` | |
| `+0x8c` | `0x15b28e` (`0x15b270`, per player in `ENGINE_UPDATE`), `0x2404ad`, `0x2bc967` | player, trait; 8; AL | the trait query | `0x24dda0` | own bodies; each falls back to `0x24dda0` |
| `+0x90` | `0x15c6af` in the state machine `0x15c550` | state `E+0xc04`, 4 | | `0x72c70` | CTF/Assault `0x241fc0` |
| `+0x98` | `0x159c36` (`0x159ac0`, in `ENGINE_UPDATE`) | 1, 4 | | `0x2bcc80`, false | |
| `+0x9c` | `0xa4602`, `0xa4672`, `0xa46df`, `0xa4752`, `0xa47c2`, `0xa48a9`, `0xa77d8`, `0x158622` | none | the replicated engine-globals entity type, EAX | `0x99810` returns 0 | CTF/Assault 1, Oddball 2, KotH 3, Territories 4, Juggernaut 5 |

The `+0x9c` values are the six types of the replicated globals entity
([notes/ai-networking.md](../../game/notes/ai-networking.md)); the slot's
callers at `0xa45f0` onwards are, by their neighbours, the replication
code (inferred). The other slots are dispatched from `0x157ed0` (`+0x20`),
`0x1582d0` (`+0x28`), `0x158530` (`+0x30`, from `GAME_ACTIVATE`),
`0x1595d0` (`+0x34`), `0x15ea50` (`+0x38`), `0xcd7b0` (`+0x40`),
`0x15e8e0` (`+0x44`), `0x157670` (`+0x68`), `0x15cbf0` (`+0x74`),
`0x15ee30` (`+0x94`), `0x162060` (`+0xa0`), `0xa4620..0xa48f6`
(`+0xb0`..`+0xb8`), `0x9afbf` (`+0xbc`) and `0x19e1c0` (`+0xc8`), or by no
call the search found; what they do is not read.

`0x15b3a0` (the round resolution), `PLAYER_OUT_OF_LIVES` `0x15db30` and
`PLAYER_RESPAWN_READY` `0x15dd40` load the table only to test that an
engine exists; they make no slot call. `PLAYER_SPAWN` `0x14f2e0` makes
none either; its starting-weapon pass `0x19ef40` reads slot `+0x00`.

### Where each engine scores

`SCORE_ADD` `0x15b7c0` has ten call sites. By engine:

| Engine | Kill callback `+0x78` | Scores from |
|---|---|---|
| Slayer | `0x2bc000` | the kill callback: `0x2bc05d`, `0x2bc077`, `0x2bc0f7` |
| Juggernaut | `0x1922a0` | the kill callback: `0x19238c`, `0x1923db` |
| Oddball | `0x2bcc10`: events only (`0x15b930`) | `+0x48` `0x2bcbb0` → `0x2bc990` → `0x2bce70` (`0x2bceb2`) |
| KotH | `0x2bda60`: events only | `+0x48` `0x2bd680` → `0x2beee0` (`0x2bf1e7`, `0x2bf2d2`) |
| Territories | `0x2ed7f0`: `ret 10h`, nothing | `+0x48` `0x2c0230`, tail jump to `0x2c0a90` (`0x2c0b32`) |
| CTF, Assault | `0x240ee0`: events only (`0x1967d0`) | `0x242e00` (`0x242e0e`), from `+0x3c` at `0x240711` and from `0x242741` |

`0x15b930` and `0x1967d0` post a game engine event (ECX the event
number); what the client does with one is not traced.

### Slayer's kill callback `0x2bc000`

```text
0x002bc000  mov ecx,[4E6948h]
0x002bc006  cmp byte ptr [ecx+0Ch],4      ; distributed client: ret 10h
0x002bc011  mov ebp,[esp+0Ch]             ; victim  (entry [esp+8])
0x002bc016  mov esi,[esp+0Ch]             ; attacker (entry [esp+4])
0x002bc03e  mov dl,[esp+18h]              ; flag (entry [esp+0Ch])
0x002bc05d  call 0015B7C0h                ; SCORE_ADD(eax = +1 or -1, attacker)
0x002bc068  test byte ptr [ecx+22Ch],4
0x002bc077  call 0015B7C0h                ; SCORE_ADD(-1, victim)
0x002bc082  test byte ptr [edx+22Ch],1
0x002bc08b  mov eax,[esp+1Ch]             ; kind (entry [esp+10h])
0x002bc0f7  call 0015B7C0h                ; SCORE_ADD(bonus, attacker)
0x002bc0ff  ret 10h
```

The arguments are `PLAYER_KILLED`'s own first three and a local it set
(`0x15d685`: `[esp+10h]`, 13 or 6 on the paths at `0x15d60a`/`0x15d61a`).
`G+0x22c` is record `+0xf0`, the type-area flags (settings 28, 29, 30 are
its bits 0, 1, 2 in `variants::record::field`). The attacker's score is
skipped with no attacker (-1) or when the victim's player `+2` has bit 1;
it is +1 with the flag clear, else -1, except a suicide (attacker equal to
victim), which scores -1 only with bit 1. Bit 2 takes 1 from the victim.
Bit 0 adds to the attacker, with no flag and not a suicide, 1 when the
kind is 0..14 plus 1 when `0x23f260` (the attacker's team, the victim)
halved is 0. Calling bits 0, 1, 2 bonus, suicide and death penalties is
inferred from these paths, not from a label.

### Hooking the slot bodies

`h2 analyze --explain` finds each of `0x2bc000`, `0x1922a0`, `0x2bcbb0`,
`0x2bd680`, `0x2c0230`, `0x192530` and `0x240d70` as a function start by
data pointer (the vtables), and `0x24dda0` by call. `0x2c0230` follows the
previous function's tail jump at `0x2c022b`, so it has no return or
padding before it. Slayer's per-tick and per-player slots are the shared
`0x175f40` and `0x72c70`, which many slots of every engine use: a hook
there is a hook on all of them, so a Slayer carrier's tick is answered at
`ENGINE_UPDATE` or not at all.
