# `tag_get_checked` (0x122c10) and `tag_group_find` (0x122d00)

Status: Built. Code: `crates/hooking/guest-layout/src/tags.rs`, adapters in
`crates/rewrites/engine/src/abi.rs`. Tests: `crates/rewrites/engine/tests/differential.rs`,
`crates/rewrites/engine/tests/views.rs`. Switch: `impl.engine.tags` (off by default;
[switches.md](../runtime/switches.md)).

Recovered from `h2 disasm` of the retail XBE
(`03215919…935d`). Names are assigned. Rust: `crates/hooking/guest-layout/src/tags.rs`,
adapters in `crates/rewrites/engine/src/abi.rs`.

## Globals

| Address | Meaning |
|---|---|
| `0x00547f00` | pointer to the tag-index header (published by `cache_tags_load`) |
| `0x004e3b44` | pointer to the 16-byte tag-entry table |

The checked lookup bounds the slot with the index header's `+0x18` tag count
but takes entries from `0x4e3b44`, not from the header's own `+8` pointer.
Both must stay valid while generated code still runs.

## `tag_get_checked` (0x122c10)

**Convention (LTCG):** ECX = full tag datum, EBX = expected group FourCC
(numeric multi-char constant, e.g. `'proj'` = `0x70726f6a`). Plain `ret`, no
stack arguments. Result in EAX: the entry's metadata address (`entry+8`) or
0. Preserves EBX, ESI, EDI, EBP. Clobbers ECX, EDX, flags.

**Behaviour, in order:**

1. `slot = (i16) datum` (sign-extended low half). Negative → 0.
2. `slot >= index->tag_count` (signed) → 0.
3. `entry = slot * 16 + *0x4e3b44`. `entry->datum != datum` (all 32 bits) → 0.
   So stale salts fail. Then, at `0x122c3d`, an entry address of 0 → 0: the
   function keeps "no entry" as a null pointer, so an entry that lands on
   address 0 is rejected even when its datum matches.
4. `record = tag_group_find(entry->group)`.
5. Accept if `record->group`, `record->parent` or `record->grandparent`
   equals EBX (short-circuit, in that order). Accept → `entry->address`,
   otherwise 0.

**Failure paths:** no assert, no KeBugCheck. Two quirks, both reproduced:

- If the entry's group has no record, `tag_group_find` returns 0 and step 5
  reads guest addresses 0, 4 and 8 with no null check. On hardware that is
  whatever page 0 holds (or a fault). The Rust code performs the same reads.
- EBX = `0xffffffff` matches any tag whose group has an absent parent or
  grandparent, since absent ancestors are stored as `0xffffffff`.

**Callers:** nine direct call sites (`0x9c697`, `0x9ccba`, `0x9db21`,
`0x9f971`, `0xa002a`, `0xa17ea`, `0xa1a92`, `0xa1e15`, `0xa5be4`) with
expected groups `jpt!`, `weap`, `item`, `proj`, `obje`. All test EAX for zero
immediately. Several push/pop EBX around the call, consistent with EBX as an
input that is otherwise callee-saved. Most of the engine bypasses this
function and inlines `(datum & 0xffff) * 16 + *0x4e3b44` then `+8` with no
checks (`docs/engine/engine-map.md`), so overriding this does not change those.

## `tag_group_find` (0x122d00)

**Convention:** ECX = group FourCC. EAX = address of its 12-byte group record
or 0. Preserves EBX, ESI, EDI, EBP; clobbers ECX, EDX. Writes a 12-byte key
`{group, -1, -1}` into its own stack frame (dead after return).

1. `index = *0x547f00`; null → 0.
2. bsearch (`0x13ddd0`) over `index->groups` (`+0`), count `index->+4`,
   element size 12, comparator `0x122cf0`.
3. Hit index `-1` → 0; else re-reads `*0x547f00` and returns
   `groups + i * 12`.

**bsearch `0x13ddd0`** (custom: EAX = count; stack key, base, size, compare,
context; `ret 0x14`): `lo = base, n = count; while n: mid = lo + (n>>1)*size;
r = compare(key, mid, ctx); if r == 0 return (mid-base)/size; if r > 0 { lo =
mid+size; n-- } n >>= 1`. Shifts are arithmetic: a negative count never
reaches zero and loops until some probe matches. Reproduced as is;
`tag_group_find_bounded` takes a probe budget so tests can observe the hang
(e.g. count -1, key 1, `groups[-1].group = 2` never terminates). A hit at
index -1 is reported as not found, since -1 is also the miss value.

**Comparator `0x122cf0`** (stdcall, 3 args): returns `key->group -
elem->group` as a wrapping 32-bit subtraction read as signed. Ordering is the
sign of that difference, which differs from unsigned order when two groups
differ by 2³¹ or more. ASCII FourCCs never do; the Rust code still uses the
same arithmetic and probe sequence, so unsorted or odd tables give the same
answer as the original.

A map whose group records are out of order loses groups to this search.
`map::build` therefore writes the table sorted by the stored word, whatever
order the set declared its groups in. Before that, a fresh Halo 1 a10 listed
`obje` and `unit` ahead of `bipd` (ancestors declared first). The iterator
`0x122c70` then got 0 for `bitm` and faulted at campaign start.

## Differential evidence

Lifted original vs. Rust adapter on the IR interpreter (see this directory's
README.md for the method): random group tables of 0–24 records (10% unsorted,
5% with high-bit FourCCs), random parent/grandparent links, 0–40 tag slots
with empty slots and unknown groups, queries with live, stale-salt, NONE,
negative-slot and past-count datums against matching, ancestor, unknown and
NONE groups, plus null-index runs of `tag_group_find`. All register and
whole-memory comparisons agreed: over five seeds, 10,160 `tag_get_checked` and 10,800 `tag_group_find` calls, no disagreements.
