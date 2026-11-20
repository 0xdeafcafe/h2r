# `locale_string_id_find` (0x1a02d0) and `locale_string_id_exists` (0x1a0310)

Status: Built. Code: `crates/rewrites/engine/src/locale.rs`, adapters in
`crates/rewrites/engine/src/abi.rs`. Tests: `crates/rewrites/engine/tests/differential.rs`.
Switch: `impl.engine.locale` (on by default;
[switches.md](../runtime/switches.md)).

Recovered from `h2 disasm` of the retail XBE (`03215919…935d`).
Names are assigned. Rust: `crates/rewrites/engine/src/locale.rs`; adapters in
`crates/rewrites/engine/src/abi.rs`.

## Locale record

Nine 0x1c-byte records at `matg + 0x188 + language * 0x1c`
(`docs/formats/map-format.md`). `localized_tag_string_get` (`0x1a0180`) computes
the same address as `(language + 0xe) * 0x1c + matg`, with `matg` from
`0x004e034c` and the language from `0x0047ff38` (lazily initialized).

| Off | Meaning | Who writes it |
|---|---|---|
| `00` | resident index: `count` × `{u32 string_id, u32 blob_offset}` | `locale_tables_load` |
| `04` | resident string blob | `locale_tables_load` |
| `08` | i32 entry count | map |
| `0c` | blob size | map |
| `10` / `14` | index / blob file offsets | map |
| `18` | u8 loaded | `locale_tables_load` sets 1 |

A `unic` tag gives each language a `(start: i16, count: i16)` range at
`unic + 0x10 + language * 4`; callers sign-extend both halves.

## `locale_string_id_find` (0x1a02d0)

**Convention (LTCG):** EAX = first entry (i32), ECX = entry count (i32),
EDX = locale record, EDI = StringID (packed, full 32 bits). Plain `ret`.
EAX = string address. Preserves EBX, EDX, ESI, EDI, EBP; clobbers ECX.

1. `end = start + count` (wrapping). For `i` from `start` while `i < end`
   (signed):
2. Bounds clamp, inlined: `i < 0` is skipped; `i > record.count - 1` is
   skipped (`count - 1` wraps, so `count = i32::MIN` accepts everything).
   Out-of-range entries are never read.
3. `index[i].string_id == EDI` → return `record.blob + index[i].offset`.
4. No match → `0x0043e5e1`, a NUL byte in `.rdata` (checked against the
   loaded image), so a miss reads as an empty string.

The comparison is the whole packed value, length byte included: an ID with
the right low 24 bits and a different length misses. It does not check the
loaded flag; all four callers (`0x160fdd`, `0x19378c`, `0x193855`,
`0x1a036e`) test `+0x18` first and pass the result straight to the
conversion helper `0x140440`.

## `locale_string_id_exists` (0x1a0310)

**Convention:** ESI = locale record, ECX = first entry, EBX = StringID,
stack entry count; `ret 4`. Result in **AL only**: the upper 24 bits of EAX
are the caller's. Preserves EBX, ESI, EDI, EBP; clobbers ECX, EDX.

Not loaded (`+0x18 == 0`) → false. Otherwise the same scan as above; true on
the first match. One caller. The loaded byte is read first and the stack
count only after it (`0x1a0319`), so an unloaded table never touches the
argument.

## Differential evidence

Random tables of 0–60 entries (with 4 extra matching-looking entries past
the count, so an over-read would show), 10% of IDs differing from another
only in the length byte, 20% unloaded, and queries with `start` in
`-5..count+3` and `count` in `-2..count+6`, random keys and live keys. The
exists adapter was called with random EAX upper bits. All results,
registers and memory agreed.
