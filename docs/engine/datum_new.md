# Datum arrays: `datum_new` (0x16b840) and its siblings

Status: Built. Every leaf of the family has a Rust port with an adapter; `data_new` and
`data_dispose` stay generated because they call guest allocators.
Code: `crates/engine/src/datum.rs`, adapters in `crates/engine/src/abi.rs`.
Tests: `crates/engine/tests/differential.rs`. Switch: `impl.engine.datums`
(off by default; [switches.md](../runtime/switches.md)).

Recovered from `h2 disasm` of the retail XBE (`03215919…935d`),
window `0x16b570–0x16bd10`. Names are assigned (engine-map names where it
has them). Rust: `crates/engine/src/datum.rs`; adapters in
`crates/engine/src/abi.rs`.

## Header layout (`0x4c` bytes)

| Off | Type | Name (ours) | Evidence |
|---|---|---|---|
| `00` | `char[32]` | name | `strncpy(hdr, name, 0x20)`, then `hdr[0x1f] = 0` |
| `20` | i32 | maximum_count | capacity bound in `datum_new`, `_at` variants |
| `24` | i32 | element_size | `elem = size * i + data` everywhere |
| `28` | u8 | alignment_bits | stored from CL by `data_initialize` |
| `29` | u8 | valid | 0 from initialize, 1 from make_valid/attach |
| `2a` | u8 | flags | bits 0,1 set by initialize; bit 2 set after contiguous construction (43 inlined `or [esi+2a],4` plus `data_new`); bit 3 = debug fill `0xBA` on free (never set in retail) |
| `2c` | u32 | signature | `0x64407440`, bytes `@t@d` |
| `30` | u32 | allocator | object with vtable `+0` alloc(size), `+4` free(ptr), thiscall |
| `34` | i32 | free_scan_start | where `datum_new` starts scanning; deletes lower it |
| `38` | i32 | high_water | one past the highest slot in use; lookups and iteration stop here |
| `3c` | i32 | active_count | live elements |
| `40` | u16 | next_salt | seeded `name[0..2] \| 0x8000` |
| `44` | u32 | data | element storage |
| `48` | u32 | bitmap | occupancy, bit `i` of word `i >> 5` |

An element's first u16 is its salt; 0 means free. Handles are `(salt << 16)
| index`, `0xffffffff` = none. `data_new` allocates `size*count +
((count+31)>>5)*4 + (1<<align) + 0x4b` bytes.

Occupancy is recorded twice, and functions disagree on which they use:
**bitmap** for `datum_new`'s scan, `data_next_index` and everything built on
it; **salt word** for `datum_try_and_get`, `datum_get_absolute`,
`data_previous_datum`, `data_attach` and the `_at` allocators. They agree
unless something writes one without the other.

## Functions

All are LTCG custom-register functions with a plain `ret` unless noted.
"Clobbers" excludes flags (always clobbered). Every other register is
preserved. No function here asserts or calls KeBugCheck; failures are return
values.

| Address | Name | In | Out | Clobbers |
|---|---|---|---|---|
| `16b570` | `data_new` | EAX count, EDI allocator; stack name, size, align; `ret 0xc` | EAX header or 0 | ECX, EDX |
| `16b5d0` | `data_dispose` | ESI header | – | EAX, ECX, EDX |
| `16b5f0` | `data_initialize_contiguous` | ECX align, EDX size, EBX count; stack buffer, name, allocator; `ret 0xc` | – | EAX, ECX, EDX |
| `16b650` | `data_initialize` | ESI header; stack name, count, size, align, allocator, bitmap; `ret 0x18` | – | EAX, ECX, EDX |
| `16b6b0` | `data_attach` | ESI header, EAX count, ECX storage | – | EAX, ECX, EDX |
| `16b790` | `data_make_valid` | EAX header | – | EAX, ECX, EDX |
| `16b7a0` | `data_delete_all` | ESI header | – | EAX, ECX, EDX |
| `16b840` | `datum_new` | EDX header | EAX handle / -1 | ECX |
| `16b910` | `datum_new_at_index` | EAX handle, EDX header | EAX handle / -1 | ECX |
| `16b990` | `datum_new_at_absolute_index` | EDI index, EDX header | EAX handle / -1 | ECX |
| `16ba00` | `datum_initialize` | ESI element, EDX header | – | EAX, ECX |
| `16ba40` | `datum_delete` | EAX handle, EDX header | EAX = new live count | ECX |
| `16bae0` | `datum_try_and_get` | EAX handle, ESI header | EAX element / 0 | ECX, EDX |
| `16bb20` | `datum_get_absolute` | EDX index, ESI header | EAX element / 0 | ECX |
| `16bb50` | `datum_absolute_index_to_index` | ECX index, EDX header | EAX handle / -1 | EDX |
| `16bb70` | `data_iterator_next` | ESI iterator | EAX element / 0 | ECX, EDX |
| `16bbc0` | `data_next_datum` | EAX prev handle / -1, ESI header | EAX handle / -1 | ECX, EDX |
| `16bc00` | `data_next_index` | ECX header, EDX start | EAX index / -1 | ECX, EDX |
| `16bc40` | `data_last_datum` | stack header; `ret 4` | EAX handle / -1 | ECX, EDX |
| `16bcc0` | `data_previous_datum` | EAX header, EDX next handle / -1 | EAX handle / -1 | ECX, EDX |

Call counts in a linear scan of `.text`: `datum_new` 251, `datum_delete` 107
calls + 19 tail jumps, `data_delete_all` 95, `data_next_index` 93,
`data_make_valid` 62, `datum_try_and_get` 57, `data_initialize_contiguous`
53. Many more sites inline the lookups. At least one caller of
`datum_new_at_index` (`0x6b7ae`, `0x82206`, `0x12357c`) reads `[edx+0x44]`
right after the call, so EDX surviving is part of the contract.

### `datum_new` (0x16b840)

1. `i = free_scan_start`. While `i < high_water` (re-read each step): if
   bitmap bit `i` is clear, take it (unless `i == -1`, which falls through).
2. Otherwise take `high_water` itself, unless `high_water >= maximum_count`
   or `high_water == -1`: return -1 with nothing written. That slot's bitmap
   bit is not checked.
3. Set the bit; `active_count++`; `free_scan_start = slot + 1`;
   `high_water = slot + 1` if `slot >= high_water`.
4. Zero `element_size` bytes of the element, store `next_salt` in its first
   u16, `next_salt++`, and if it is now `0xffff` reset it to `0x8000`.
   So `0xffff` is never handed out unless seeded.
5. Return `(salt << 16) | slot` (slot OR-ed at full width).

### Allocation at a chosen slot (0x16b910, 0x16b990)

Fail (-1) if the index is at or past capacity (the absolute variant also
rejects negatives) or the element's salt is non-zero. The bitmap is not
consulted. They set the bit, bump `active_count` and `high_water`, but do
**not** move `free_scan_start`. Both run `datum_initialize`, which consumes
a salt; `datum_new_at_index` then overwrites it with the handle's salt.

### `datum_delete` (0x16ba40)

Uses only `handle & 0xffff`: no range, salt or liveness check. A stale
handle frees the slot's current occupant; deleting a free slot still
decrements `active_count`. Order: optional `0xBA` fill, clear bit, zero
salt, lower `free_scan_start`, and if the slot was the top one walk
`high_water` down past free salts (stopping at 0), then `active_count--`.

### Lookups

- `datum_try_and_get`: NONE → 0; `index >= high_water` → 0; salt 0 → 0;
  salt ≠ handle's → 0; else element. Bound is high-water, not capacity.
- `datum_get_absolute`: `index` in `0..high_water` and salt ≠ 0.
- `datum_absolute_index_to_index`: `-1` → `-1`; otherwise reads the salt at
  any index with no bounds check.

### Iteration

- `data_next_index(start)`: first set bit in `start..high_water`; negative
  start → -1. High-water read once.
- `data_next_datum(prev)`: `next_index(prev == -1 ? 0 : (prev & 0xffff) + 1)`.
- `data_iterator_next`: iterator `{+0 array, +4 handle, +8 index}`; on end
  stores `index = maximum_count`, `handle = -1`, returns 0.
- `data_last_datum` (`ret 4`): walks next_datum to the end. It keeps its
  cursor in its own **argument slot**, which the caller can see afterwards:
  -1 after reading the header pointer, then the current handle at the top
  of each step (`0x16bc4e`). The adapter makes the same stores, so after a
  fault mid-walk the slot holds the last handle reached.
- `data_previous_datum`: scans salts downward from `start - 1` (start =
  `high_water` for -1). When nothing is found it reads one more salt at
  element `-1` (`data - size`) before returning -1. Reproduced so faults match.

### Construction and clearing

- `data_initialize`: zero header, name, flags `|= 3`, capacity, size,
  `data = 0`, `valid = 0`, alignment, signature, bitmap, allocator. The
  stack arguments are read between those stores: name after the header
  clear, then capacity and size, the alignment byte (a 1-byte read) after
  the capacity store, the bitmap before the allocator. A fault on an
  argument leaves the earlier stores in place
  (`DatumArray::initialize_from_stack`).
- `data_initialize_contiguous`: `data = align_up(buf + 0x4c, 1 << (align &
  31))`, `bitmap = data + count * size`; initialize, flags `&= ~3`, set data,
  zero bitmap. Callers then set flag bit 2 themselves. It reads its
  arguments buffer, allocator, name, before any other access.
- `data_delete_all`: zero scan start/high-water/live, **re-seed** salt from
  the name (so the same array hands out the same salts after every clear),
  optional fill, zero each salt word to capacity, zero bitmap.
- `data_make_valid`: `valid = 1` then delete_all.
- `data_attach` (one caller, saved-state load): set capacity and storage,
  `valid = 1`, rebuild bitmap/high-water/live/scan start from salts, flags
  `&= ~2`. The salt is re-seeded, so new allocations can repeat salts
  already present.

### `datum_initialize` (0x16ba00)

Called from inside the two `_at` allocators (`0x16b969`, `0x16b9da`) and
inlined into `datum_new`. Zeroes `element_size` bytes at ESI (any address;
nothing is derived from an index) and stamps the next salt with the wrap
rule above. No bitmap or counter changes. Overridden separately so
generated copies of the `_at` allocators reach the Rust.

### Fills and faults

Every zeroing and `0xBA` fill here is `rep stosd` (`size >> 2` dwords) then
`rep stosb` (`size & 3` bytes); the Rust fills the same way, so a fault
part-way leaves the same prefix written. In `datum_new` the bitmap and
counter updates come before the element fill, and the salt is read after
it, so an element that runs into unmapped memory faults with the slot
marked live but no salt consumed. The original does the same.

### Not overridden

`data_new` and `data_dispose` call the guest allocator's virtual methods, so
they are not leaves. Their Rust form needs a guest call and continuation
(`ExecCtx::call_guest`); only `datum::allocation_size` is provided. The
sibling window `0x18e1f0–0x18e700` is a different structure, the memory
pool; see `memory_pool.md`.

## Differential evidence

40 random arrays per run (capacity 1–70, element sizes 2–64 including odd,
alignments 0–5, names of 0–3 random bytes, dirty memory underneath, 15% with
debug fill, 25% with the salt near the wrap) each driven through ~120 random
operations: allocation runs to full, chosen-slot allocation with live,
stale, NONE and random handles, deletes of live and stale handles,
lookups, every iterator, clears, re-validation and attach over random
salts. Registers (minus the clobber set), ESP and all 64 MB of memory
(minus the dead stack below the final ESP) matched in every call.
