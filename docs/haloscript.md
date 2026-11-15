# HaloScript in the retail Xbox cache

`crates/hs` reads the compiled scenario tables, recovers the executable's type
and builtin descriptors, and renders Lisp-like source. `h2 hs` prints source to
the terminal; no game scripts, extracted tables, or binary fixtures are build
inputs. Keep menu scripting running; it needs a small builtin subset.
A native VM with the retail evaluator's semantics lives in `hs::vm`; its
evidence, binding layer and disc validation are in
[engine/hs_vm.md](engine/hs_vm.md). It is not yet wired into the game.

## Evidence and scope

Surveyed 2026-11-14/15 against the owner's `disc/halo2.iso`, cache build
`02.09.27.09809`. XBE SHA-256:
`03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`;
title `4D530064`, certificate version 3, entry `0x002d0aee`.
Addresses below are guest VAs. Descriptive function names are assigned from
behavior, not recovered debug symbols. Evidence is static instructions, tables,
and cache contents, not an execution trace.

Sources are the local executable and disc, inspected using this repository's
`h2 disasm` and bounded cache reader. The starting anchors and source-use policy
are [engine-map.md](engine-map.md) and [map-format.md](map-format.md).
The prior scenario schema supplied the initial `+0x1a8/+0x1b0` leads; that schema
already documents community-source influence. The engine map supplied the HS
address window, pools and stride leads. This is not a clean-room claim. No
community HS layouts, opcode lists, implementation bodies, or XML were fetched,
translated, copied, or used as build inputs for this lane.

**E** below means directly supported by inspected engine instructions/data.
**D** means tested against every disc map. **Inference** identifies an
interpretation with a narrower evidential limit. Bounds alone are not semantic
proof. The limited tables below each have their own evidence, rather than a
copied scenario child-block catalogue.

Reproduce instruction evidence with your own disc (the address/count are
examples, not an instruction dump):

```sh
cargo run -p cli --release -- disasm disc/halo2.iso 0x20a750 30
cargo run -p cli --release -- disasm disc/halo2.iso 0x209940 25
cargo run -p cli --release -- disasm disc/halo2.iso 0x2a0c40 30
```

## Scenario tables and per-table provenance

Descriptors are `u32 count, u32 guest_address`, except the strings descriptor's
count is bytes. Integers are little-endian. The parser checks nonempty spans
against the owning scenario payload and requires four-byte alignment; zero
count ignores the address. This deliberately does not guess other scnr blocks.

| Table / scnr field | Layout | Provenance and evidence |
|---|---|---|
| Legacy syntax data, `+0x1a8` | Byte count/address; no supported body | Prior schema lead; **D:** byte count is zero on all 30 maps. Populated legacy blobs are rejected, not parsed as the runtime table. |
| Token strings, `+0x1b0` | Byte count/address; NUL-terminated byte strings | **E:** `0x002a09c1–0x002a0a2f` loads `scnr+0x1b4`, selects type 9 nodes, adds node `+0xc`, writes the resulting runtime pointer to `+0x10`. **D:** all needed string/symbol offsets terminate inside this blob. |
| Scripts, `+0x1b8` | Count/address; stride `0x28` | **E:** `0x00209244–0x00209284` walks count/pointer with 40-byte increments and tests kind at `+0x20`; `0x00209520` selects command-script kind 5 from the same table; `0x00209883–0x002098d0` evaluates root `+0x24`. Return type `+0x22` is read by `0x00209b2d–0x00209b40`. **D:** printable fixed names match script-call tokens; all kinds, types and salted roots validate. |
| Globals, `+0x1c0` | Count/address; stride `0x28` | **E:** `0x002090ff–0x00209229` reads descriptors and advances by 40; root at `+0x24`, type at `+0x20`. `0x00209a55–0x00209a6b` selects a scenario global's declared type. **D:** references' retained names agree with these definitions. `+0x22` is preserved without assigned meaning. |
| Script tag references, `+0x1c8` | Count/address; stride 8: FourCC, salted tag datum | **D:** all 2,307 records fit, resolve full tag IDs and match classes/ancestry using the existing map reader; all bytes pass codec symmetry. **Inference:** this adjacent dependency list is identified as script tag references from its contents/context; a specific runtime consumer of this scnr field has not been established. Do not infer residency or loading order from it. |
| Runtime syntax slots, `+0x238` | Count/address; stride `0x14` | **E:** `0x0020a777–0x0020a783` passes these two fields to `0x0016b6b0`; that function publishes the data pointer, scans first-word salts and rebuilds occupancy. `0x0020a720–0x0020a727` sets node stride 20 and maximum capacity `0x9000`. **D:** all live links and roots resolve with full salts; all slots, including free slots, pass codec symmetry. |

A script definition is a 32-byte fixed name, `u16 kind`, `u16 return_type`,
`u32 root_datum`. A global definition substitutes `u16 value_type`, an
uninterpreted `u16`, then the initializer root. Name padding is retained.
The six script-kind name pointers at `0x0046fff8` identify startup, dormant,
continuous, static, stub and command_script; kind behavior is corroborated by
initialization and completion paths below.

### Syntax node wire record

| Offset | Width | Meaning / engine evidence |
|---|---|---|
| `+0` | u16 | Salt; zero means free. `0x0016b6f4` tests it while reconstructing occupancy. |
| `+2` | u16 | Builtin/script index on call nodes; original value type on literals. `0x00209946` selects builtin; `0x00209964` selects script; `0x00209a8f` supplies literal source type to conversion. |
| `+4` | u16 | Expression/result type; supplied to conversion at `0x00209a8b`; type 9 tested at `0x002a0a1d`. |
| `+6` | u16 | Flags: bit 0 atom, bit 1 scenario-script call, bit 2 global atom, bit 3 retained node. Tests at `0x00209a28`, `0x00209940`, `0x00209a2f`, `0x0020a923`. |
| `+8` | u32 | Next sibling datum. `0x00207d91–0x00207db6` traverses it; argument evaluator `0x00209e51` advances through it. |
| `+0xc` | u32 | Token-string offset where applicable; source-only offset otherwise. String fixup at `0x002a0a28`. |
| `+0x10` | u32 | First child datum for calls; value bits/global index for atoms. `0x00207d85`, `0x00209a35`, `0x00209a88`. |

Datums encode low 16-bit slot, high 16-bit salt; `0xffffffff` terminates lists.
The engine often masks directly, but the host decoder validates the entire
handle. Free slots can contain `0xba` poison beyond their zero salt: attempting
to decode their remaining fields semantically invents corrupt links/types.
Observed live flag combinations are 8, 9, 10 and 13. Other supported combinations
have the same lower bits without the retained bit; unknown combinations fail.

A call node's child is a type-2 function-name atom; its sibling chain is the
argument list. The call and its name atom both carry the opcode. For bit-1 calls
that index selects the scenario script table, **not** the builtin table. Mapping
all type-2 atoms to builtins incorrectly gives hundreds of conflicting names.
Calls and argument links are a graph; validation rejects stale salts, free-slot
links, malformed heads and cycles, including in unrooted live slots.

Numeric/void atom `+0xc` values can point beyond the retained string blob; they
refer to discarded source, not retained text. Render their stored bits instead.
Type-9 runtime pointer words are also not portable cache pointers: use the
retained offset just as the engine's load-time fixup does. Some strings contain
non-UTF-8 bytes; the renderer preserves those as `\xNN`, not replacement chars.
Symbolic resource/object/AI/enum literals retain their original token spelling;
the renderer does not claim to decode every type's packed runtime value.

## Executable builtin and type tables

The actual retail function descriptor is smaller than an editor definition.
**There is no builtin-name pointer in it.** Searching for expected command names
and following the interpreter instead finds the following:

| Table | Layout / provenance |
|---|---|
| Builtin pointer table `0x004744e0` | 911 u32 guest descriptor pointers. **E:** interpreter indexes it at `0x0020994d`, calls descriptor `+4`, and reads return type through it at `0x00209b21`. **D:** all 911 descriptors/parameter types validate. Adjacent count word `0x0044f6bc` is 911, agreeing with pointer/descriptor extents. |
| Type-name pointer table `0x0046ff00` | 62 u32 pointers to NUL strings. Located from the type-name string at `0x00453f10` and pointer xrefs, then checked against typed descriptors and node values. **E:** conversion dispatch `0x0020a2c7` uses stride 62; literal and string handling corroborate scalar indices. A full parser using this name table is not claimed. |
| Script-kind names `0x0046fff8` | Six u32 string pointers, immediately after the 62 type names; startup/dormant/continuous semantics corroborated by interpreter branches. |
| External globals `0x00473468` | `0x41d` descriptor pointers; declared type u16 at descriptor `+0`, external value pointer at `+4`. `0x00209a41–0x00209a50` reads type; `0x0020a2e0` onward refreshes values. The catalog reads types only. Names are not inferred for unused entries. |

Builtin descriptor fields:

| Offset | Type | Interpretation |
|---|---|---|
| `+0` | u16 | Return type index |
| `+2` | u16 | Flags; 0, 1 and 2 observed (844, 42, 25 entries). Exact bit-0 semantics unresolved. |
| `+4` | u32 | Evaluate guest VA; slot 3 is null, retained as such |
| `+8` | u32 | Optional usage/argument-help string pointer, **not the function name** |
| `+0xc` | u16 | Parameter count |
| `+0xe` | u16[count] | Parameter type indices; descriptor storage rounds up to four bytes |

Parameter interpretation is direct: wrapper `0x002a0c40–0x002a0c69` reads the
count at `+0xc`, passes the array at `+0xe` to `0x00209d50`, which checks argument
types and allocates four-byte result slots. Flags-2 special forms have custom
argument evaluation, so a zero parameter count does not mean zero arguments.
For example `sleep` has custom optional arguments, while `camera_set` has two
ordinary parameters, type 15 and type 7. A null evaluate pointer is not a safe
no-op: it is an explicit unsupported runtime dispatch if a compiled call reaches
it. No inspected disc call reaches the null slot.

Builtin names are recovered at inspection time by joining each **builtin call**
index with its child token's string offset. Across all 30 maps this yields 390
names, without conflicting spellings for an index. The other 521 descriptors
remain unnamed; the XBE does not supply the missing names in this layout. No
community opcode table fills the gaps. `Catalog::observe` retains the map-derived
names in memory and rejects conflicting evidence transactionally.

The supported XBE is checked by title/certificate/entry, count and dispatch
instruction anchors, then bounded table reads. These are compatibility guards,
not a cryptographic identity check; modified executables with the same anchors
still require review. All function/type metadata is read from the supplied XBE.

## Menu requirements

The menu needs a small builtin subset. Inspect the exact subset at runtime with
`h2 hs mainmenu --builtins --disc disc/halo2.iso`. The tool reads the owner's disc;
no per-script call list or script-derived summary is retained here.

## VM structure recovered from the interpreter

- Nodes pool pointer: `0x004f9394`; thread pool: `0x004f9384`; global-value pool:
  `0x004f9380`; current-thread datum: `0x004f938c`; scheduler enabled byte:
  `0x004f9388`. Their data-array payload pointer is at `+0x44`.
- `0x00208ff0` allocates up to `0x140` (320) threads of `0x418` bytes and `0xc00`
  global slots of eight bytes. The first `0x41d` global slots are external
  globals. Scenario globals occupy subsequent slots. Node global-index bit 15
  selects external versus scenario global; it is not part of a scenario index.
- Thread `+0`: datum salt; `+2`: thread category byte; `+3`: flags; `+4`: script
  index (or -1); `+8`: signed wake time/state; `+0xc`: saved sleep state;
  `+0x10`: current frame pointer; `+0x14`: value storage; `+0x18`: start of the
  1,024-byte inline stack. Constructor `0x002097c0` establishes the root frame.
  Category zero is ordinary scenario execution. Other categories have distinct
  synchronous/transient/command paths; they are not all script-kind values.
- Frame `+0`: parent pointer; `+4`: current expression datum; `+8`: result
  destination; `+0xc`: u16 local byte cursor; locals begin at `+0xe`. Frame push
  at `0x00209aa0–0x00209acf` places the next frame at old frame + cursor + `0x10`.
  Return `0x00209ae0` converts the value to the expression's expected type,
  writes through the saved destination and restores the parent frame. These
  are guest stack addresses, not Rust references.
- `0x002099f0` evaluates atoms immediately or pushes a call frame. Thread flag
  bit 0 marks first entry; `0x00209850` clears it before dispatch and passes
  that status to evaluate. Evaluators retain progress in frame locals across
  yields. Bit-1 call nodes take `0x00209bf0`, which evaluates the script root.
- New-map initialization `0x002090a0` evaluates global initializers first and
  creates ordinary threads for script kinds 0, 1 and 2. Kind 1 starts with wake
  state -2 (dormant); other ordinary threads start at zero.
- `hs_update`, `0x00209310`, checks enabled state, reads tick count through
  `0x00510c54 +8`, enumerates occupied thread slots, skips category 4 and negative
  wake states, and runs threads whose wake time is <= current tick. This is
  called in the simulation tick by `0x00137fe0`, as documented in engine-map.
- On completion, startup/dormant threads receive -1; continuous threads retain
  their root frame and are eligible to start again on the next update
  (`0x0020997e–0x002099d5`). Transient category-2 threads are deleted. An idle
  scheduler therefore is not equivalent to removing scripting from the menu.
- Sleep evaluator `0x002087e0` evaluates a duration and optional script target.
  The positive duration path at `0x00208925–0x0020899a` converts authored
  30-Hz units through `1/30` at `0x0045dc6c` and the current tick rate at time
  globals `+2`, then adds current tick. It preserves an interrupted thread's
  previous sleep state in `+0xc` under thread flag bit 1. This is not evidence
  that the whole runtime is invariably 30 Hz.
- Wake helper `0x00209c80` leaves -1 completed/permanent-sleep threads alone;
  otherwise it clears wake time, restores a saved sleep state when flag bit 1
  is set, and has explicit frame-unwind handling for opcode `0x15` (the
  condition-wait evaluator at `0x002089b0`). Script-to-thread lookup is
  `0x00209e70`. A Rust VM must preserve dormant, completed and timed waits as
  distinct states; an OS-thread sleep is not an equivalent implementation.

The VM in [engine/hs_vm.md](engine/hs_vm.md) reproduces these structures in
Rust; its stack bound is a host policy. No guest-compatible VM ABI is promised here. The
interpreter uses register arguments in several helpers; these notes must not
be turned directly into ordinary C calling conventions. Retain generated VM
execution until replacement behavior is tested against a runtime trace.

## Validation and use

Run against a personally supplied disc:

```sh
cargo run -p cli --release -- hs --all --disc disc/halo2.iso
cargo run -p cli --release -- hs mainmenu --disc disc/halo2.iso
cargo run -p cli --release -- hs example_map --script example_script --disc disc/halo2.iso
cargo run -p cli --release -- hs mainmenu --builtins --disc disc/halo2.iso
```

A local `.map` path is accepted too; the XBE still comes from `--disc` (or
`H2_DISC`). `--builtins` reports statically reached signatures and guest evaluate
addresses. Script filtering prints one definition only. Missing/ambiguous script
names are errors. Source is printed only to stdout; closed output pipes are
accepted. Never commit redirected source or use it as a test fixture.

All 30 disc maps passed, including the shared resource maps and maps with only
free syntax slots. Aggregate counts: **338,867 slots; 295,322 live nodes;
5,297 scripts; 798 globals; 2,307 references**. All live call indices resolve to
the 911 builtin descriptors or their scenario script definitions. No unrooted
live nodes or salt mismatches were found. Every retained table passes byte-for-byte codec symmetry, including 43,545 free slots, name padding and reserved words.

The decompiler preserves each call node and function-name atom as one Lisp
list, without collapsing implicit `begin` wrappers. Its independently tokenized
output contains exactly **295,322 reconstructed node occurrences**. This is a
**node-count** check, not a compiler capable of reconstructing original
salts, cache addresses, resource bindings or inferred types. Whitespace/comments
from original source are unavailable. Nonfinite reals use a lossless
`#real-bits:XXXXXXXX` notation; non-UTF-8 string bytes use escapes. These are
readable inspection forms, not a promise of unchanged H2EK compiler input.

Synthetic tests cover dead-slot poison and reserved-byte preservation, reference
wire order, salted/free/out-of-range links, cycles, malformed call heads, table
bounds/alignment, unsupported legacy blobs, script/global bounds, unknown
builtins/types, conflicting names, escaped byte strings, negative zero, script
selection, malformed source, and deep iterative traversal. No disc tests run in
the default suite; `hs --all` (and the optional example) explicitly opt into disc integration checks.

Verification commands for this lane:

```sh
cargo fmt -p hs --check
cargo clippy -p hs -p cli --all-targets --no-deps -- -D warnings
cargo test -p hs -p cli
```

The changed packages are checked on base `8e4dc8c`, using `--no-deps` to keep
unrelated dependency lints outside this lane.

## Host limits and typed API

Token lookups inspect at most 4,097 bytes (4,096 bytes plus NUL); a longer or
unterminated token is an error. This bounds repeated scanning and UTF-8 checks
even when many nodes share one string offset. A blob is capped at 64 MiB.
Before decoding any record, the table reader checks every count and span:
36,864 syntax slots, 32,767 scripts/globals each, and 65,536 references.
The reference/string/output limits are host resource policies, not recovered
engine capacities. Output appends check remaining capacity before writing,
including escaped atoms, definition headers and delimiters. The maximum source
size is 64 MiB; callers can request a smaller budget. Node expansion has a
separate bound for shared graphs.

Global indices must have canonical 16-bit storage: zero extension, or sign
extension for negative i16 external indices. The disc contains 906 sign-extended
external references; rejecting every upper word would reject retail maps.
Arbitrary upper bits are rejected. Fixed-signature builtin calls check argument
count and expression types against the XBE parameters. Custom-argument forms
(flag 2) retain their custom evaluator contract; their arity/type rules are not
claimed to be validated by the fixed-signature check.

`Scenario`, `Catalog` and their invariant-bearing records expose read-only
accessors. `Scenario::expression(datum)` returns a borrowed `Expression`:
`Call`, `ScriptCall`, `Global` or `Atom`. Calls expose bounded child-datum
iterators; script calls expose their target definition. `Literal` distinguishes
void, boolean, real bits, short, long, byte string, function name and symbolic
values with their raw bits. Real bits preserve NaN payloads and negative zero.
The typed literal decoder dispatches on the original source type (node opcode),
with the function-name special case, and is shared by validation and rendering.
Use `validate(catalog)` before execution; a typed view alone is not a complete VM
semantic verifier. DAG sharing is preserved in storage and traversed without
recursion; emitted occurrences and unique node counts are reported separately.

`hs --all` performs codec symmetry plus a source node-count check, not compilation
back to a cache. `--builtins` traverses reachability without rendering source.
All HS output modes treat a closed output pipe as normal termination and
propagate other write failures.
