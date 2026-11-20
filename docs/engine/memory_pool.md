# Memory pool (0x18e1f0–0x18e700): contract notes, not yet in Rust

Status: Reference. No Rust code implements these functions; the pool
allocator is still the recompiled original. Code: none. Switch: none.

Recovered from `h2 disasm` (`03215919…935d`). This window holds
the pool allocator used by objects and UI, not more datum-array code. It is
documented here because it was surveyed with the datum primitives; the
Rust port is a separate step (it calls guest allocators and has a debug
path through `0x3314b0`).

## Header (`0x40` bytes, cleared by the initializer)

| Off | Meaning |
|---|---|
| `00` | signature `0x706f6f6c` (`'pool'`) |
| `04` | `char[32]` name (`strncpy` 0x20, forced NUL at `+0x23`) |
| `24` | allocator object (vtable `+0` alloc, `+4` free) |
| `28` | data base: `align_up(header + 0x40, 16)` |
| `2c` | capacity in bytes (from EBX) |
| `30` | free bytes (starts at capacity) |
| `34` | first block (list head) |
| `38` | last block (list tail) |
| `3c` | u8: blocks don't record an owner back-pointer |
| `3d` | u8: try first-fit / gap allocation before appending |
| `3f` | u8: debug headers (`'head'`, 0x10 extra bytes per block) |

Blocks carry a 16-byte header before the payload: `+0` size, `+4` owner
pointer (the caller's slot that receives the payload address, used for
compaction), `+8` next, `+0xc` previous.

## Functions

| Address | Assigned name | Convention / effect |
|---|---|---|
| `18e1f0` | `memory_pool_new` | EAX allocator (0 → default `*0x468758`), ECX capacity, stack name; allocates `capacity + 0x50` and initializes. Returns header or 0. |
| `18e230` | `memory_pool_dispose` | EDX header: clears 0x40 bytes, frees through the allocator. |
| `18e250` | `memory_pool_initialize` | ESI header, EBX capacity; stack name, allocator; `ret 8`. |
| `18e2b0` | `memory_pool_allocate` | ECX pool; stack (out slot, size, tag); EAX a second tag (EAX and the third argument only reach the debug header). Rounds `size + 0x10` (`0x20` with debug headers) up to 16. With `+3d` set tries the first gap (`18e690`), then the head (`18e600`); always falls back to the tail (`18e560`). Records the out slot as owner unless `+3c`, subtracts from free bytes, stores the payload address to the out slot; AL = success; `ret 0xc`. |
| `18e340` | `memory_pool_reallocate` | Grows in place when the next block allows, else allocate + copy (`rep movs`) + free. |
| `18e430` | `memory_pool_free` (unlink) | EAX = owner slot, EDX pool: returns bytes, unlinks. |
| `18e470` | `memory_pool_compact` | Slides every block down with `memmove` (`0x320890`) and rewrites each owner slot. |
| `18e4f0`/`18e560`/`18e600`/`18e690` | block-insert helpers | Link a block after/before neighbours, at the tail, at the head, or into the first gap that fits. |

Compaction rewrites caller-owned pointer slots, so a Rust port has to keep
those slots in guest memory and preserve the exact block order; it cannot
hand out host allocations.
