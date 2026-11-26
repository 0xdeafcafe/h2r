# Forge loose ends: the respawn bits, the weapon reserve and the spawn weight

Evidence for [forge-map-variants.md](../forge-map-variants.md) ("Fields by
kind", "Properties without a retail field") and
[scenario-editor.md](../../editor/scenario-editor.md) section 7. Nothing here
states what the code does today; the design pages do.

## 2026-11-26: read from the retail image

Commit `a6154ae83`; retail 1.0 `default.xbe` through `$H2_DISC`. Static
reading only, with `h2 disasm`. No run. What a trace of the running game
must still show is in [forge-probes.md](forge-probes.md).

**Reproduce.**

```sh
h2 disasm "$H2_DISC" 0x12000 1100000 > text.asm      # one linear listing
grep -n '+10Ah\]' text.asm | grep -i ' or \| and '    # object byte +0x10a
grep -n '+12Ch\]' text.asm | grep -i ' or \| and '    # object dword +0x12c
h2 disasm "$H2_DISC" 0xd6800 60     # the routine that sets +0x10a bit 2
h2 disasm "$H2_DISC" 0x1076f4 90    # the routine that sets +0x12c bits 0, 1
h2 disasm "$H2_DISC" 0x23b060 90    # the influence sum of a location's weight
h2 disasm "$H2_DISC" 0x101a10 70    # the magazine adjust: reserve and clip
```

### The respawn modes' bits (D1)

- **Mode 1, bit 2 of `+0x10a`.** `0xd6800` takes an object handle, reads
  the object's `+0x10a`, and when bit 2 is clear sets it (`0xd683a`) and goes
  on to detach and release the object's children, then call `0xd6a70`. A set
  bit returns early (`0xd6834`). Nine direct calls reach it (`0xc43aa`,
  `0xd6015`, `0xd6bf1`, `0xd8073`, `0xd8f0b`, `0xda525`, `0xda5db`,
  `0xdc170`, `0x11a40f`). The bit is cleared only at `0xcaf19` (`and byte
  ptr [edi+0x10a], 0xfb`), in the routine at `0xcaf00` that then calls
  `0xb8b70` and `0xbb950`. The bit is therefore a once-only mark on an
  object that the engine has begun to take apart; its name is not
  established (**S**).
- **Mode 0, bits 0 and 1 of `+0x12c`.** `0x1076f4` takes an object handle
  and a pointer to a block element of the object's definition (`ebx`), sets
  the object's `+0x130` and `+0x13c` from that element's `+0x34` and
  `+0x36`, and copies bit 3 of the element's byte `+0x38` to bit 0 of the
  object's `+0x12c` (`0x1077fb`) and bit 4 to bit 1 (`0x107808`). It ends by
  setting bit 7 (`0x10782c`). The bits are the object's copy of two flags
  of one block element of its definition; which block and what the flags are
  is not established (**S**).

Neither bit is named, so respawn modes 0 and 1 are not given rows in
Properties. The census that would show which modes the disc's maps use is
still to be run (`disc_item_game_types_and_respawn_modes_census`, ignored,
needs the disc).

### The weapon's reserve (D5, spare clips)

- The weapon object's first magazine is at `+0x228` (stride `0x10`), reserve
  `i16` at `+0x22a` and loaded `i16` at `+0x22c`
  ([live-gamestate.md](../../live/live-gamestate.md) section 3;
  `guest_layout::Weapon::MAGAZINE_RESERVE`). `sim.state.set` and `sim.give`
  already write both for a held weapon (`agent::sim::state::set_magazine`).
- `0x101a10` adjusts a weapon's magazine from the definition's magazine
  block (`weap+0x2c0` count, `+0x2c4` base, stride `0x5c`): it reads the
  `i16` at element `+0x0a` as the clip (it scales it by a fraction clamped
  to 0 to 1 and moves that many rounds between loaded and reserve,
  `0x101a9c`..`0x101ac4`) and caps the reserve at the `i16` at element
  `+0x0c` (`0x101ad4`..`0x101adb`, `cmovg`). So a clip is the element's
  `+0x0a` rounds and the reserve's cap is its `+0x0c` (**E** for the use;
  that the two are the tag's `rounds loaded maximum` and `rounds total
  maximum` is **C**).
- The spawner `0x15aad0` stores the entry's index in the object's `+0xaf`
  ([live-gamestate.md](../../live/live-gamestate.md)), so the spawned weapon
  is found by that byte. Where the object's reserve is first set (its
  creation) was not read.

A spare-clips write is then: for each spawner entry with the property whose
weapon exists, set the first magazine's reserve to `min(clips * clip, cap)`.
No call writes the reserve of a weapon on the ground; one is needed.

### The spawn weight (D5, starting point weight)

`0x23ba90` copies the location's position and calls `0x23b060` with EAX the
weighing context ([siege note](2026-11-24-siege-spawns-and-bodies.md)) and
EDX the position. `[eax]` is a count and the elements are `0x20` bytes each,
from `eax+4`. Each element is a cylinder: a centre (`+0x00` to `+0x08`), an
inner radius (`+0x0c`), an outer radius (`+0x10`), a height above (`+0x14`)
and below (`+0x18`) the centre, and a weight (`+0x1c`). The distance is
horizontal. A position outside the outer radius or the height band adds
nothing; inside the inner radius it adds the weight; between them it adds
`(1 - (d - inner) / (outer - inner)) * weight` (`0x23b12c`..`0x23b144`, the
1.0 at `0x45dbc0`). The sum is the location's weight, and `0x23ba90` adds the
random term. Nothing reads the location's own bytes beyond the position, so
a per-point weight is a bias added to this sum. The elements come from the
engine's slot `+0x80`; what they are for each game type was not read (**S**).

The chooser keeps the heaviest location that passes the free-spot test
(`0x14efb6`..`0x14efe5`). A bias for entry `i` would be added at
`0x14efb1`, after `0x23ba90` returns the weight in XMM0 and before the
`comiss` at `0x14efb6`; no feature hooks there.
