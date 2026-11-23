# Item spawner game types, the matcher and respawn modes: evidence

Evidence for [forge-map-variants.md](../forge-map-variants.md) ("Fields by
kind", "Game types") and
[scenario-editor.md](../../editor/scenario-editor.md) section 2. Nothing
here states what the code does today; the design pages do.

## 2026-11-23: slice D1, read from the retail image

Commit `dd6d5d647`; retail 1.0 `default.xbe` through `$H2_DISC`. Static
reading only, with `h2 disasm`. No run; the disc census below has not
been run.

**Reproduce.**

```sh
h2 disasm "$H2_DISC" 0x15a090 60    # the matcher
h2 disasm "$H2_DISC" 0x15ae70 300   # the netgame equipment tick
h2 disasm "$H2_DISC" 0x15ad30 40    # the reset
h2 disasm "$H2_DISC" 0x19ef64 20    # the starting equipment pass
h2 disasm "$H2_DISC" 0x14ef00 100   # the spawn chooser
# callers and readers: one linear listing of .text, then grep
h2 disasm "$H2_DISC" 0x12000 1100000 > text.asm
grep -n 'call 0015A090h' text.asm
grep -n '+104h\]' text.asm          # starting locations, scnr+0x104
```

### The matcher `0x15a090`

Arguments: `edi` the slots (i16 each), `edx` the engine code, one stack
argument the slot count. It returns the index of the first slot that
matches, or -1.

| Address | What it does |
|---|---|
| `0x15a090`..`0x15a0ae` | loads the engine object `[0x55e4d0 + 4 * [[0x4e9ae8]+0xc14]]`; none goes to `0x15a120` |
| `0x15a0b7`..`0x15a0c0` | a match when the slot equals the code |
| `0x15a0c3`..`0x15a0d9` | a match when the code and the slot are each 1 or 9 |
| `0x15a0db`..`0x15a0e4` | a slot of 12 matches |
| `0x15a0e6`..`0x15a107` | a slot of 13 matches unless the code is 1 or 9 |
| `0x15a0fd`..`0x15a103` | a slot of 14 takes 13's path |
| `0x15a120`..`0x15a137` | no engine object: the first slot equal to 0 |

`grep 'cmp cx,0Eh'` over `.text` finds only `0x15a0fd`: the rule is not
inlined elsewhere. The only two calls are `0x15af32` (netgame equipment)
and `0x19ef91` (starting equipment). Both load the code from slot `+0x00`
of the engine's vtable first (`0x15af29`, `0x19ef88`; -1 when there is no
engine object), pass the entry's `+0x04` (`lea edi,[ebp+4]` at `0x15af2b`,
`0x19ef8a`) and push 4. The codes per engine are in
[engine-vtables](../../engine/notes/2026-11-21-engine-vtables.md); read
again here, King of the Hill's slot `+0x00` is `0x99810`, `xor eax,eax;
ret`, so its code is 0.

### The netgame equipment tick `0x15ae70`

Per entry (`(i * 9) << 4` at `0x15aeea`), on the entries of this tick's
stride (`0x15aef2`..`0x15af07`):

- `0x15af37`: no match skips the entry for this tick.
- State `[esp+2Ch]` is `[0x4e9ae8]+0x7dc`: 8 bytes an entry, the object
  (`+0`), the countdown (`+4`, i16) and the abandon count (`+6`, i16).
- `0x15b03f`: a running countdown goes to `0x15b137`.
- `0x15b053`: no object arms the countdown at once (`0x15b0e8`).
- `0x15b05d`: mode 0 goes to `0x15b080`, 1 to `0x15b06d`, any other value
  to `0x15b137` (no arming).
- Mode 1, `0x15b06d`: arms when bit 2 of the object's byte `+0x10a` is
  set.
- Mode 0, `0x15b080`: when the object's type `+0xaa` is 2, 3 or 4
  (`test al,1Ch` on `1 << type`) and bit 0 or 1 of its byte `+0x12c` is
  set, arms; otherwise arms when the squared distance from the object's
  `+0x30` to the entry's `+0x40` exceeds the float at `0x45dbc0`.
- `0x15b0e8`: arming sets the countdown to the respawn time
  (`0x15ac50`), scaled for the entry's type byte `+0x14` by game options
  bytes `+0x211` (type 0) or `+0x208` (types 1 to 7).
- `0x15b137`..`0x15b1aa`: the countdown falls by one; at 0, with flags
  bit 1 set and an object that is not a weapon, the old object is
  deleted (`0x15b19f`), and a vehicle for which `0x158ff0` or `0x15f060`
  is true waits a tick (`0x15b190`) instead; then
  `0x15aad0` spawns.

### The reset `0x15ad30`

Called at `0x157b44` in `0x157ae0`, which `0x158556` (in `0x158530`) and
`0x15c5de` (in the state machine `0x15c550`) call. For each entry it
deletes the object and sets the countdown to 1 and the abandon count to
0 (`0x15ad86`). Every entry whose slots match therefore spawns on its
first tick, whatever its respawn mode: there is no "place at start" field.

### Starting locations

Every load of `[scnr+0x104]` followed by a `0x34` stride was listed
(`grep '+104h\]'`, then the stride within 15 lines): `0x14bcaa`,
`0x14ef90`, `0x14f154`, `0x14f3da`, `0x14f4c6`, plus `0x1526ad`,
`0x1529ce` and `0x23ceba` for entry 0. They read the position and facing
(`0x11c7d0`, `0x23cec7`) and the player type `+0x2c`. The spawn chooser
`0x14ef00` scores each entry with `0x23ba90` (position and a random term)
and checks it with `0x23bb70` (a collision test at the position); neither
reads `+0x12`..`+0x2b`. Not found is weaker than found: code reaching the
block through another pointer would be missed.

### The census

`disc_item_game_types_and_respawn_modes_census` in
`crates/tools/scenario-edit/src/tests.rs` (ignored; needs the disc) reads
every multiplayer scenario, fails on a slot outside 0 to 14 or a respawn
mode outside 0 and 1, and prints the slot values, the slot sets, the
modes, the entries each engine code would spawn, and the starting
locations' words from `+0x12` to `+0x2a`.

```sh
cargo nextest run -p scenario-edit --run-ignored only --no-capture \
  -E 'test(disc_item_game_types_and_respawn_modes_census)'
```
