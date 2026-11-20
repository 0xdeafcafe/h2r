# Campaign indirect dispatch coverage

Status: Built. The tool is an analyzer example; the retail result below is one measurement.
Code: `crates/analyze/examples/dispatch_coverage.rs`, `crates/analyze/src/dispatch.rs`, `crates/analyze/src/pointer_seeds.rs`. Tests: `crates/analyze/tests/dispatch_coverage.rs`.
Switch: `H2_XBE` (input path); no other.

The `analyze` example `dispatch_coverage` measures dispatch entry coverage for
new-map `0x137ca0`, BSP `0x11c1b0`, HaloScript `0x209310`, objects `0xb7b40`,
AI `0x1c83c0` and Havok `0x311100`. It reads `$H2_XBE` at execution time. There
are no copied executable bytes or table contents in the source or tests.

## Reproduce

Run from the checkout; keep the source disc read-only. The optional
`--unseeded` argument disables only the pointer-table seeding pass, preserving
the rest of analysis so before/after results use the same implementation.

```sh
export H2_XBE=/path/to/default.xbe
KEEP=/path/outside/the/repo     # the TSVs are derived from your disc
cargo run -p analyze --release --example dispatch_coverage -- --unseeded > "$KEEP/dispatch-before.tsv"
cargo run -p analyze --release --example dispatch_coverage > "$KEEP/dispatch-after.tsv"
```

## Tests

Synthetic regressions in `crates/analyze/src/pointer_seeds.rs` cover a one-slot vtable followed by pointer-shaped metadata into an unclaimed MOV immediate (`heuristic_tail_does_not_seed_metadata_inside_unclaimed_mov`), a valid trapping callback that is the final rejected speculative candidate (`final_rejected_candidate_is_still_seeded_as_a_trap_callback`), and a flat far jump whose known instruction bodies stay protected (`seeding_respects_flat_far_jump_instruction_claims`). `crates/emit/tests/far_jmp.rs` runs a far jump through analyze, lift and emit. The codec fixture (`codec_schema_keeps_null_record_and_indexes_the_whole_family`, `crates/analyze/src/dispatch.rs`) checks all nine records, null record 0 and the exclusion of metadata fields; `crates/analyze/tests/dispatch_coverage.rs` is the retail test (`H2_XBE`, ignored without it).

The TSV contains:

- `census`: image-wide functions, address-taken labels, exported entry
  candidates, conflicts, convergence rounds and convergence verdict.
- `entry`: the complete image-wide entry census with discovery source and
  return behavior for functions, allowing lost or reclassified entries to be audited.
- `root` and `site`: each root's conservative reachable functions and all
  indirect call/jump instruction addresses, owners and candidate table bases.
  Kernel thunks and recovered switches are distinguished from unresolved
  register/memory transfers. `unassociated-sites` counts dynamic sites with no
  table reference anywhere in the owner, not all unresolved heap receivers.
- `table`: one row per referenced table, including its evidence, slot and
  distinct-target counts, non-function targets and grouped missing addresses.
- `catalog-table` / `catalog-target`: additional dispatch-evidenced tables
  outside the root closure, retained for unknown heap receivers; these are not
  asserted reachable from any root.
- `target`: every table slot, decoded target and `function`, `interior-entry`,
  `null` or `MISSING` status. An interior entry must actually appear in a
  discovered function's exported CFG, not merely have an instruction claim.
- `switch-table` / `switch-target`: recovered switch bounds, null holes and
  targets. `local-instruction` means an internal branch destination, not an
  externally callable entry; it does not need a new function seed.
- `unreached` / `cfg-error`: addresses whose descent cannot continue and CFG
  errors. Do not hide these when evaluating coverage.

Function/entry availability is an analyzer/CFG requirement for emission. It
is not a check that a particular generated module has been rebuilt, registered,
or executed without an unsupported instruction. Recompile the module after
changing discovery.

## Table evidence and graph limits

Known retail layouts are gated on title ID and entry address, then validated
as a unit. Lifecycle has 68 records of 36 bytes with nine callback columns;
new-map uses column 2, BSP init/dispose use columns 4/5, and switch preparation
uses column 6. HaloScript has 911 descriptor pointers with evaluator at +4;
return/parameter types, parameter spans and count are validated. Object tables
contain 13 descriptor pointers, callbacks at +0x10..+0x80 and a bounded,
null-terminated ancestry list from +0x84. Null callbacks stay null. Layout
provenance is documented in `engine-map.md`, `haloscript.md` and `objects.md`.
Animation codecs at `0x47fb14` have nine `0x28`-byte records: unused record 0
and codecs 1–8. Each has a default callback at +0 and six evaluation callbacks
at +0x10..+0x24. The full family span includes indexed bases such as `0x47fb28`
in null record 0, so referencing that base includes every codec. Name and
integer fields stay data; null callback slots remain null.
Three additional bounds prevent concrete false positives in the campaign
closure. The callback table at `0x44f978` has ten 12-byte records: the caller
at `0x2cbae9` uses an index multiplied by three and a four-byte memory scale;
only record +0 is a function pointer. The two remaining words are packed
fields, not callbacks. The base UI vtable at `0x4576d0` has three pointers;
constructors/destructor include `0x22cced`, and `0x22cd16` calls slot +4.
The packed values beginning at `0x4576dc` are outside that vtable. The UI
vtable at `0x4590b8` has 28 slots and ends before a UTF-16 line-ending string
at `0x459128`; that string is not a callback pointer either. These
address/layout facts are encoded; all targets are still read from the XBE.
The report fails visibly if the known retail layout does not validate.

Other tables start as contiguous executable-pointer runs in file-backed data
sections. A constructor's object store of a table address, or an indirect
control transfer with a fixed table displacement, provides dispatch evidence.
The runs are split at these observed table bases, excluding unrelated prefix
words. Statically referenced single callback cells are included too. These
are candidate vtables/function-pointer tables: the last run's end can include
adjacent metadata, and the report keeps the `known-layout=false` distinction.
Arbitrary packed numeric arrays are not classified as callback tables just
because some values happen to fall in executable ranges.

Reachability follows direct calls, exiting CFG edges, and all targets of a
table referenced by any instruction in a reached function. Address-taken
interior targets conservatively include their whole owner. A table reference
in the same function is an association, not proof that a particular register
or heap object supplies that table to a particular indirect call. Unknown
heap-built tables, callbacks written at runtime, and paths behind unresolved
indirect edges can be absent. The report is a static lower bound on sites and
a conservative overapproximation where it expands known tables. It cannot
prove every campaign dispatch is covered; only an instrumented journey can
resolve the remaining dynamic frontier.

## Seeding

`pointer_seeds::seed` is called from one point after ordinary discovery has
settled for a round, and processes table evidence to a fixpoint.
Known layouts submit targets to the existing overlay validator without an
alignment/prologue guess. Heuristic pointer runs do not authorize fresh ordinary
function decodes in unknown bytes: an apparent tail slot might be unrelated
metadata pointing into an unclaimed instruction's immediate. Such candidates
remain in the inventory as unresolved. Observed table bases alone do not prove
the tail's length or index stride. Instruction-body/data conflicts, blacklists
and speculative arbitration remain in force. Already-decoded instruction starts
can become interior entries rather than duplicate functions.

There is one narrow exception to speculative trap rejection. A table member
that stores an address within its own table to an object and immediately
executes INT3 is an explicit trapping callback. Its complete instruction span
must be compatible with existing claims, and its start must satisfy the
ordinary alignment/boundary heuristic before it becomes a function. Speculative
trials restore the previous active-function state when they end, so a final
rejected candidate cannot block its later insertion as a trap callback. It
still executes INT3 in generated code: this closes `UnsupportedTarget`, not
the deliberate guest trap. Unreferenced runs and stack-local stores do not
provide this evidence. Switch sizing/recovery is unchanged.

## Runtime diagnosis

`Runner` already emits `Event::UnsupportedTarget { pc, target, kind, route }`.
After the stop, log `ExecCtx::report(event_id)` or drain `take_events()` before
the bounded event buffer evicts records. Keep the call-site PC and transfer
kind/route: CALL/JUMP need an entry; a RETURN may require a resume instead.
Record the generated module revision alongside a report, since stale code can
miss entries the current analyzer discovers.

This change deliberately does not add a "log and skip every unknown target"
mode. An unknown callee's cdecl/stdcall cleanup, return value, memory effects,
and callback ownership are unknown; an unknown jump has no call continuation
to skip to. Continuing would corrupt the stack or omit initialization and make
later observations unreliable. Keep the stop, repair the missing entry or
register a contract-specific implementation, regenerate, then rerun the boot.
A replay/emulation fallback could collect additional targets without dropping
behavior, but no such fallback is introduced here.

## Measured retail result

Measured at commit `55d7ac5`, with pointer seeding disabled/enabled
in the same analyzer, and XBE SHA-256
`03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`.

| Measure | Unseeded | Seeded |
|---|---:|---:|
| Functions | 18,686 | 18,695 |
| Address-taken labels | 203 | 207 |
| Callable entries | 18,889 | 18,902 |
| Non-null target misses in the catalog | 13 | 0 |
| Analysis conflicts | 0 | 0 |

Both analyses converge in five rounds. Seeding loses no existing entry. Nine
new functions preserve INT3 behavior; four other targets become interior entries.
Two addresses that a gap scan would make standalone functions are decoded as
part of other owners: `0x2d1e67` is an instruction in owner `0x2d1e10`, and
`0x2d6454` is in owner `0x2d6434`, which returns through its flat far jump.

The catalog has 453 tables, with 165 in the union of root closures. The union
contains 2,229 indirect sites; 1,423 dynamic sites have no associated table
reference in their owner. The codec family adds all 63 callback slots (seven
nulls, 56 non-null slots, 27 distinct targets already discovered as functions).
Its association with `0x1c7428` is present for new-map, BSP, HS, objects and AI.
The table catalog includes constructors outside the root closure, retained as
candidates for unresolved heap receivers. Only the `0x455f58` miss below is in
the conservative root closure before seeding. Do not interpret the catalog
miss count as thirteen campaign-observed failures.

| Table | Missing target before | After |
|---|---|---|
| `0x004537b8` | `0x002305f5` | covered |
| `0x00455498` | `0x001c2380` | covered |
| `0x004554c0` | `0x001c2370` | covered |
| `0x00455500` | `0x001c2360` | covered |
| `0x00455ef0` | `0x001ea970` | covered |
| `0x00455f58` | `0x001ea9b0` | covered |
| `0x00455fc0` | `0x001ea9a0` | covered |
| `0x00456008` | `0x001ea990` | covered |
| `0x00456048` | `0x001ea980` | covered |
| `0x00456088` | `0x001ea960` | covered |
| `0x00465298` | `0x00354c28` | covered |
| `0x004652a8` | `0x00354e28` | covered |
| `0x004652b8` | `0x00354c28` | covered |
| `0x004652c8` | `0x00354f28` | covered |

The lifecycle, object descriptor, HaloScript evaluator and animation codec
targets were already covered. Null slots remain null. The remaining dynamic
frontier is preserved in the site inventory; no campaign execution or success past a deliberate
INT3 is claimed. Regenerate the emitted module and repeat the inventory after
integrating other analyzer changes.
