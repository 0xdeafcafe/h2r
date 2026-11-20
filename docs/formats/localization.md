# Halo 2 Xbox string IDs and localized text

Status: Built. Code: `crates/tags/src/localization.rs`. Switch: none.

Implemented in `tags::localization`, separately from the generated tag codecs.
The existing `tags::wire::StringId` is reused and re-exported as `tags::StringId`;
there is no competing ID representation. `Unic::read` is the specialized decoder,
independent of `DecodedTag`'s five original groups.

Evidence: retail cache build `02.09.27.09809`, all 30 maps in the supplied disc;
XBE SHA-256 `03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`.
Executable addresses below are guest VAs. This is static code/disc validation,
not a boot trace or proof that the main menu renders. No executable bytes,
layout files, localized text tables, or game-derived fixtures are
included in the implementation. The field accesses, boundaries and conversion
rules come from the executable and the disc.

## String IDs and name strings

A little-endian `u32` has index in bits 0–23 and name length in bits 24–31.
There is **no separately encoded set** in this Xbox format. `StringIdParts`
contains only `index` and `length`; bits 16–23 must not be split off as a
Halo 3-style set. The constants, decode logic, accessors and name resolver
live together in `wire.rs`; the accessors delegate to `decode`.
The length describes the identifier's name, not its translation. Names in the
surveyed maps are ASCII, so byte and character lengths coincide. Zero denotes
the empty identifier; `0xffffffff` is the null sentinel. Neither is a tag datum.

The cache-header name table is distinct from localized text: `header + 0x164`
is its count, `+0x168` its blob size, `+0x16c` the file offset of `count` u32
byte offsets, and `+0x170` the name blob's file offset. Indexing the offsets
with the ID's low 24 bits leads to a NUL-terminated name. The map crate already
loads this table; `StringId::resolve` checks the high-byte length as well.

All **412,293 locale IDs** in the disc resolve to a name of exactly the encoded
length. None uses index bits 16–23. Thus the disc confirms the encoding for
its populated domain, but cannot independently distinguish a 16-bit-only
encoding from a 24-bit index; the full 24-bit width is unverified. No
set-remapping table is introduced. The local
retail *localized* resolver compares all 32 bits, rather than decoding an
index or length. A general executable ID-to-debug-name resolver remains
unidentified; the host implementation uses the verified cache-header table.

## Per-table provenance

D = directly measured on this disc; E = executable accesses/control flow.

| Table / record | Derivation and verification |
|---|---|
| Packed ID and header name table | D: 325,864 name slots and all 412,293 locale keys; E: full-key equality at `0x1a02ee`. Full 24-bit width beyond observed indexes is unverified. |
| Nine `matg` descriptors | E: `0x1a01c3–0x1a01ce` computes `matg + (language + 14) * 28`, hence `+0x188`; loader at `0x1a01e0` uses count/size/file-offset fields and writes resident pointers/one-byte flag. D: 270 descriptors; all extents fit their own map. |
| Locale index | E: count times eight in loader, full key at entry +0, byte offset at entry +4 in `0x1a02d0`. D: all 412,293 offsets terminate inside the associated blob. |
| Locale blob | E: `0x140440` consumes byte sequences and stores 16-bit units; lookup adds the byte offset to the loaded blob base. D: all terminators found; 237,598 non-ASCII entries, six invalid UTF-8 tails, no supplementary scalar values in valid UTF-8 entries. |
| Compiled `unic` | E: `0x1a01b4`/`0x1a01b9` load signed shorts at +0x10/+0x12 with language stride four. D: all 1,404 payloads are exactly 52 bytes; first 16 bytes zero; all 12,636 ranges fit. |
| Language slots | E: dashboard mapping `0x11ca80`, cached selection at `0x47ff38`, and descriptor/range indexing. D: readable samples and script distributions support slots 0–7; slot 8 empty in all maps, so its language is unverified. |

### `matg` descriptor, size 0x1c, nine slots from +0x188

| Relative offset | Representation | Meaning / confidence |
|---|---|---|
| +0x00 | u32 | Runtime index pointer, assigned by loader; not a file offset. E; zero on disc. |
| +0x04 | u32 | Runtime blob pointer, assigned by loader. E; zero on disc. |
| +0x08 | u32 | Number of eight-byte index rows. D/E. |
| +0x0c | u32 | Blob byte count, including terminators. D/E. |
| +0x10 | u32 | Index's map-relative file offset. D/E. |
| +0x14 | u32 | Blob's map-relative file offset. D/E. |
| +0x18 | u8 | Loaded flag; loader writes 1, callers test a byte. E. |
| +0x19 | 3 bytes | Opaque; nonzero values occur on disc. Their meaning is not established. |

The preexisting generated `LocaleDescriptor.loaded: u32` preserves the final
four bytes. **Only its low byte is the loaded flag.** The host reader ignores
serialized runtime pointers and flag values, and obtains residency by actually
reading the file ranges. These are plain offsets in the owning map: do not
apply metadata magic, a shared-resource selector, or an ISO partition offset
twice. The map reader's bounded region handles the container base.

The loader rounds reads to 512 bytes and allocations to 4096 bytes. Those are
I/O/allocation padding, not extra locale entries or extra string bytes. The
host reader needs only `count * 8` and `data_size` logical bytes.

Slot order: 0 English (`en`), 1 Japanese (`ja`), 2 German (`de`), 3 French (`fr`),
4 Spanish (`es`), 5 Italian (`it`), 6 Korean (`ko`), 7 Chinese (`zh`), 8 Portuguese
(`pt`, unverified: slot 8 is empty on the disc). Dashboard values 1–8 map to slots 0–7; the inspected mapping sends
value 9 and out-of-range values to English, not Portuguese. The API permits
explicit slot selection; it does not emulate dashboard preference policy.

### Locale rows and compiled `unic`

Each locale index row is `(packed_string_id: u32, blob_byte_offset: u32)`.
Offsets are relative to that language's blob. Rows are in list order, not a
sorted dictionary; offsets need not be sequential or unique.

The compiled `unic` is 0x34 bytes: 16 observed-zero bytes, followed by nine
`(start: i16, count: i16)` ranges at `0x10 + language * 4`. The first 16 bytes
are retained as a zero-form guard, not claimed to be fully understood editor
fields. Nonzero forms are rejected as unsupported. Each range selects rows
in its language's **global** index, not character offsets or bytes in the tag.
An empty count dereferences nothing; a nonempty range requires nonnegative
start/count and a checked end within the table. Fourteen empty ranges on disc
have a nonzero start, so an empty range must not be equated with `(0, 0)`.

Across all language tables there are 43,059 repeated-key rows beyond the first
occurrence, and 12,434 `(map, language, ID)` groups with conflicting byte text.
There are no duplicate keys **within** an individual `unic` language range.
All 412,293 rows are covered by those ranges exactly once (separate structural
probe checked membership, not merely the sum). A global ID dictionary would
lose context-dependent text. Use the widget's `unic` when resolving UI labels.

## Executable lookup and conversion

The inspected call chain is:

```text
UI wrapper 0x230134 (call at 0x23014d)
  -> 0x1a0180: resolve unic datum and select language
  -> 0x1a0350: require resident locale
  -> 0x1a02d0: scan selected index range for full 32-bit key
  -> 0x140440: convert returned bytes into 16-bit text
  -> bounded copy into a 256-unit UI buffer
```

`0x230134` clears the destination and retains the requested key in EDI; it gets
a UI object and passes its member at +0x1c as the `unic` datum. This identifies
a consumer of the resolver, not the whole main-menu constructor. Additional
inspected direct callsites `0x19378c`, `0x193855`, and `0x160fdd` independently
read the same signed `unic` ranges and use the same converter.

At `0x1a02d0`, EAX is range start, ECX count, EDX the locale descriptor, and
EDI the packed key. It walks the bounded range and returns runtime blob base
plus row offset for the first match. A miss returns the empty byte-string
address `0x43e5e1`. The native API improves observability by returning `None`
for a miss and `Some("")` for an explicitly empty translation. It rejects
corrupt ranges instead of reproducing the executable's skip-invalid-index
loop. There is no automatic English fallback in this lookup path.

The blob is UTF-8-like byte text, **not on-disc UTF-16**. The converter handles
one-, two-, three- and four-byte forms by accumulating six-bit continuation
payloads. Invalid lead/continuation bytes advance one byte with no output.
It does not enforce modern UTF-8 shortest-form or scalar validity rules.
The store at `0x1404d1` writes CX, so four-byte input is narrowed to its low
16 bits rather than made into a UTF-16 surrogate pair. The new `utf16*` APIs
preserve those code units. `text*` uses Rust's lossy UTF-16 conversion only
where an isolated surrogate cannot be represented in a Rust `String`.

The six strict UTF-8 failures are all 255-byte entries ending in an incomplete
multibyte sequence: five in mainmenu (two Japanese, three Korean), one Korean
entry in 04b_floodlab. Their prefix decodes and their incomplete suffix emits
nothing under the engine rule. They are counted as diagnostics, not failed
layout checks. Valid strings contain private-use controller/font glyphs; these
are preserved, not replaced with English button names.

The original UI wrapper converts into 256 units, copies at most 255 and writes
a terminating zero at unit 255. The native API returns full text without a
terminator or UI truncation. A guest UI adapter must cap at 255 units and append
NUL; font/glyph interpretation belongs to the renderer. Supplementary-character
behavior is code-derived and synthetic-tested, not exercised by this disc.

## API and CLI

```rust,ignore
use tags::{Language, Localization, StringId};

let localization = Localization::read(&mut map)?;
let unic = localization.unic(&mut map, unic_tag_id)?; // reads and validates all nine ranges
let label = localization.text_in(&unic, string_id, Language::English)?;
let guest_units = localization.utf16_in(&unic, string_id, Language::English)?;
let fields = StringId(0x05123456).decode(); // invented ID: length 5, index 0x123456
```

`Localization::first_text(id, language) -> Option<String>` and `first_utf16`
are **inspection only**: the first global match can be wrong for a widget
because IDs repeat with different text. Both items carry this warning.
`text_in`/`utf16_in` return `Result<Option<_>>`: `Err` identifies a corrupt
selected range, `Ok(None)` is a miss, and `Ok(Some(empty))` is an existing
empty translation. `unic` reads a tag and checks all nine ranges in one call;
supply the same map used to load the localization. Scoped lookups still check
their range, so later edits to a `Unic` cannot turn corruption into a miss.

`table`, `entries_for`, `bytes`, `text_at`, and `utf16_at` support inspection.
`StringId::resolve(map.strings())` returns the identifier name, not UI text.
Name resolution is not a prerequisite for loading or looking up localization:
only the complete 32-bit key matters, even for a modded row whose key cannot
resolve to a name (including the name-table null sentinel). The normal strings
command displays `<unresolved name>` for such a key and still prints its text.
`--all` separately validates names and reports index, length, and null-name
errors distinctly. It also rejects duplicate IDs inside each language range;
repeated IDs across different lists remain valid. These checks replace the
former self-checks that merely asked whether known-valid rows were accessible.

Run from this worktree; commands take the absolute disc path. Output stays on
the terminal, with control characters escaped. `--tag` is an exact `unic` path.
`--all` rejects map/tag/language filters and checks every language on every map,
continues after map/tag errors, prints counts only, and exits unsuccessfully
if any validation fails. It discovers maps, rather than hardcoding a count of 30.

```sh
cargo run -p cli --release -- strings mainmenu --lang en --tag 'ui\screens\game_shell\main_menu_screen\main_menu_screen' --disc disc/halo2.iso
```

```sh
cargo run -p cli --release -- strings --all --disc disc/halo2.iso
```

Validation result: **30 maps, 325,864 name slots, 270 descriptors, 1,404 `unic`
tags, 12,636 ranges, 412,293 locale rows, 412,293 scoped rows, six encoding
diagnostics, zero failures.** Counts include shared data once per containing
map. Mainmenu has 101 `unic` tags and 24,964 locale rows: English 3,148;
Japanese/German/French/Italian/Korean/Chinese 3,117 each; Spanish 3,114;
Portuguese zero. The main-menu list resolves six entries in English and
Japanese; Portuguese returns no entries.

Synthetic tests cover full-key matching, duplicate IDs across list scopes,
empty/missing/null distinctions, each language slot including a populated
ninth slot, malformed record sizes/guards/signed ranges, byte offsets and
terminators, bad name lengths/indices, data extents even with zero rows,
bounded ISO-region reads, oversized counts, and the engine conversion rules.
The review regressions additionally cover invalid range versus miss, the
read-and-validate helper, unresolved names accepted by lookup but rejected by
validation, duplicate IDs within a range, flag values consuming `--all`/help,
short `matg`, the exact encoded-byte limit, and a one-MiB string aliased 65
times to exhaust the scan budget. All fixtures are synthetic.
The focused `cargo test -p tags -p map -p cli` suite covers the above.
`cargo clippy -p tags -p map -p cli --all-targets --no-deps -- -D warnings`
passes without lint exceptions; the preexisting needless wrapper in the CLI's
hex parser was simplified. Formatting checks pass for the localized-text
modules, `wire.rs`, and `maps.rs`.

The **encoded** index/blob bytes across all languages are capped at 64 MiB.
This is not a 64 MiB allocation cap: during decoding the raw index coexists
with native `LocaleEntry` records (each larger than its eight-byte wire row),
so peak table storage can be roughly 2–3 times the encoded limit. Vec capacity,
allocator overhead, the map's other metadata, and requested decoded outputs
are additional. The raw index is dropped as soon as that table is decoded;
repeated offsets retain offsets rather than eagerly duplicated strings.
Terminator scanning has its own 64 MiB budget per table, counting the NUL.
Exhausting that budget reports `locale string scan limit exceeded`; reaching
the end of the blob without a NUL reports `unterminated locale string`.

The strings option parser requires a non-flag value after `--tag`, `--lang`
and `--disc`. A help flag in a value position is an error, not a help request.

Reproduce executable evidence with the existing disassembler (disc comes
before the address):

```sh
cargo run -p cli --release -- disasm disc/halo2.iso 0x1a0180 210
```

```sh
cargo run -p cli --release -- disasm disc/halo2.iso 0x140440 110
```

