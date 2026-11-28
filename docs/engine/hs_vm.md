# HaloScript VM (`hs::vm`)

Status: Partly built. The recompiled interpreter drives campaign scripts.
The Rust VM is checked against the lifted guest by a test-only differential
(see Validation). Campaign run parity and promotion are open.

`crates/formats/hs/src/vm` runs a decoded `hs::Scenario` against the
XBE's `hs::Catalog` and an `Engine` trait. Rust owns thread frames,
value slots, globals, and the slot-order scheduler. The live comparison
reads the guest's checkpointed state and leaves execution to the guest.
The decoder and table provenance are in [haloscript.md](../formats/haloscript.md).

Evidence is `h2 disasm` of the owner's `default.xbe` (SHA-256 `03215919…935d`).
Some functions were read through Ghidra decompilation and checked against
the disassembly. Names are descriptive, not recovered symbols. No disc data
is compiled in. `VmImage::from_xbe` reads the constants below from the
executable and refuses one that differs.

## Value model

Every value lives in a four-byte slot: thread locals, global cells, argument
arrays. Nothing tags the slot. The type decides how many bytes are read
(`Cell`, `HsType::size`, `Value::decode`):

| Width | Types |
|---|---|
| 0 | 0..=4 (unparsed, special form, function name, passthrough, void) |
| 1 | boolean |
| 2 | short, script, 13..=17, 20..=27, hud_message, enums 44..=49, object names 56..=61 |
| 4 | real, long, string, string_id, every datum, tag reference and object type |

The widths come from the store-width case lists in the external-global
refresh and write-back switches (`0x0020a2e0`, `0x0020a490`). At load they
are checked against the size table at `0x00445710` that `=` hands to
`repe cmpsb`. Narrow stores keep the slot's other bytes, and so does the VM
(`Cell::store`). Unbound externals (null value pointer) read as 0, as the
empty string (`0x00470010`), as `0xffff` or as `-1`, by width.

Strings are pointers. `=` on strings compares the pointers, not the text. The
VM encodes a script string as its offset in the string blob, which is the
offset the load fixup at `0x002a0a28` adds to the blob base. It encodes engine
text as a host-table index with bit 31 set. That fixup rewrites the data word
of **every** node of type 9, whatever its flags. A string-typed call or
global atom would be corrupted, so the VM faults on one (`StringFixup`).
No disc map contains one.

## Casts

The atom path (`0x0020a290`) and the return path (`0x00209ae0`) cast from a
source type to the node's type `+4`, with the same guard:

- identity when the types match, the source is passthrough, or the
  destination is an object name;
- an object destination converts only from an object name (`0x000bb760`,
  table `0x004de2d0`, index `0..0x280`). Any other source passes the handle
  unchanged;
- otherwise call `0x004f5770[dest*62 + source]`. `0x0020a1a0` fills the table,
  and nothing else writes to it (Ghidra xrefs: two readers plus the
  initialiser).

| Entry (dest <- source) | Body | Behaviour |
|---|---|---|
| void <- 5..=61 | `0x0011ca70` | 0 |
| boolean <- real, long | `0x0020a000` | `test eax,eax; sete al`: **true when the bits are zero**; upper 3 bytes kept |
| boolean <- short | `0x0020a020` | true when the word is zero |
| boolean <- string | `0x0020a040` | true when empty; upper bytes are the length's |
| real <- short / long | `0x0020a070` / `0x0020a090` | `cvtsi2ss` |
| real <- enum 44..=49 | `0x0020a0b0` | `(value + 1) as real` |
| short <- real | `0x0020a0d0` | `cvttss2si`, low word (NaN gives 0) |
| short <- long | `0x0020a110` | slot unchanged |
| long <- real / short | `0x0020a0f0` / `0x0020a100` | `cvttss2si` / `movsx` |
| object_list <- object 50..=55 | `0x0020a170` | -1 stays -1, else a new one-element list |
| object_list <- object name | `0x0020a130` | name to object, then list |
| object_list <- ai | `0x00272ea0` | engine |

Unfilled entries are null, and the guest would call address 0. The VM reports
`InvalidCast` instead. The inverted boolean entries are live data:
short-to-boolean casts happen in retail scripts (see Validation). They deserve
a runtime trace before anyone "fixes" them.

## Threads

Thread datum `0x418` bytes (array `0x004f9384`, 320 slots, `0x00208ff0`):

| Field | Meaning |
|---|---|
| `+2` | category: 0 script, 1 global init, 2 transient (console), 4 externally driven (skipped by `hs_update`) |
| `+3` | flags: bit 0 first entry, bit 1 saved sleep, bit 2 console inspect |
| `+4` | script index |
| `+8` | wake tick; -1 finished, -2 dormant |
| `+0xc` | saved wake (flag bit 1) |
| `+0x10` | current frame; frames are `parent, expr, dest, u16 cursor, locals` from `+0x18`, 1,024 bytes |

`0x002097c0` creates a thread. Dormant scripts start at -2, everything else at
0. The VM allocates slots the way `datum_new` does: lowest free slot from the
scan start, then the high-water slot. Scheduling order is slot order.

New map (`0x002090a0`): delete all threads, then enable the scheduler. Then,
on one category-1 thread, evaluate each scenario global's initialiser in table
order straight into its cell (EBX = cell, `0x00209191`). A call initialiser
runs to completion synchronously (`0x002091a5`). Object-list globals take a
reference, and each global then goes through write-back. Then delete that
thread and create a category-0 thread for each startup, dormant and continuous
script, in script-table order. Static (3) and stub (4) scripts get no thread.
Command scripts (5) are not created here.

`hs_thread_execute` (`0x00209850`) first sets wake to 0. If only the root
frame is left, it evaluates the script root again. Then it loops while a frame
is above the root. It breaks when wake < 0, when the scheduler is disabled, or
when the game-globals byte `+0x1120` is set and `now < wake`. It clears the
first-entry flag and dispatches the top frame: builtins through descriptor
`+4` (`0x004744e0`), script calls through `0x00209bf0`. When the root is
reached again:

- category 0: startup and dormant scripts get wake -1; continuous scripts
  keep wake 0 and restart on the next update;
- category 4: wake -1;
- category 2: the thread is deleted.

Because a continuous thread completes at its wake tick and only restarts on
the next update, `(sleep 10)` in a continuous script repeats every 11 ticks.

`hs_update` (`0x00209310`) is Rust: tick phase 8 of the tick driver
(`crates/rewrites/engine/src/hs_update.rs`). Each thread it picks runs in
the lifted execute `0x00209850`, counted in `hs_update::Stats::executed`.

| Step | What the update does |
|---|---|
| 1 | returns at once, with no tail call, if scripts are off (`[0x4f9388]` 0); else reads `game_time + 8` once |
| 2 | walks occupied slots in order with `data_next_index`, which re-reads the high-water mark |
| 3 | runs every thread that is not category 4 and has `0 <= wake <= now` |
| 4 | stops if the scheduler is disabled, then tail-calls `0x001deed0`, the object-list sweep: every list with no references (`+4` 0) is freed with its node chain (`0x2963b0`); Rust in `crates/rewrites/engine/src/hs_object_lists.rs` |

So a thread woken by an earlier slot runs in the same tick, and one woken by
a later slot runs on the next.

## Evaluation

`0x002099f0` evaluates an atom immediately into the destination: a literal
cast from its source type `+2`, or a global, with the external refreshed
first (`0x00209bc0`), cast from the global's declared type. A call stores the
destination in the current frame, pushes a frame and sets first entry. Return
(`0x00209ae0`) casts from the builtin's or script's declared return type,
writes through the parent's destination and pops. Fixed-signature builtins
use `0x00209d50`, which evaluates one argument per entry and checks each node
type against the descriptor's parameter list.

The VM binds natives **by evaluate address**, never by name. Names exist only
in map tokens.

| Address | Index | Semantics |
|---|---|---|
| `0x00207c20` | begin | each child into one slot; value of the last, 0 if none |
| `0x00207cf0` | begin_random | each entry steps the LCG and probes `(i + ((seed>>16)*n >> 16)) % n` for an unused branch; one-word bitmap (more than 32 branches would overrun it, so the VM faults) |
| `0x00207ef0` | if | false with no else returns 0 |
| null | cond (3) | null evaluator; the compiler lowers `cond`, and no disc call reaches index 3 |
| `0x00208070` | set | value evaluated straight into the global; write-back; object-list reference counts; returns the refreshed global |
| `0x002081e0` | and/or | start at index==5; fold bytes; continue only while the running value equals the start value |
| `0x00208310` | + - * / min max | first operand seeds the accumulator; `index - 7` jump table; SSE single. min takes the operand on `ja`; max takes it on `jbe`, so NaN wins in max |
| `0x002084c0` | = != | both operands typed as the **first** operand's node type; byte compare over its size |
| `0x00208570` | > < >= <= | real as real, long **converted to single**, otherwise the low word |
| `0x002087e0` | sleep | ticks = `fistp(d * (1/30) * rate)`, `0x0045dc6c` and `game_time + 2`; d < 0 or ticks <= 0 is a no-op (so `sleep 0` does not yield); finished targets ignored; sleeping another thread saves its wake once |
| `0x00208750` | sleep_forever | the optional script argument is *evaluated* (`0x0020879b` calls `0x002099f0`, so a script-typed global names the script it holds); that script's thread, or this one, gets wake -2 |
| `0x002089b0` | sleep_until | period default 30, timeout default -1. It returns when the last condition value was true or the timeout has passed. Otherwise it starts the condition and sets wake = now + max(1, period), capped at the timeout. The condition starts through `0x002099f0` (`0x00208bae`). A call condition only pushes its frame and, when sleeps yield (`+0x1120`), the loop stops before it runs, so it is first evaluated one period later. A literal or global condition is sampled on the call tick, but that sample is only acted on at the next wake. Either way the wait does not end on the call tick. |
| `0x00208c40` | wake | reads the script index from the atom's data word (no evaluation), then `0x00209c80` |
| `0x00208e10` | (23, inspect) | evaluate; console threads (flag bit 2) format through `0x0046fd70` |
| `0x00208f10` | unit | keep the object if `1 << type(+0xaa)` is in `0x004456d4[index-23]` (bipeds and vehicles), else -1 |
| `0x002a0c40` | not | `cmp byte,0; sete` |
| `0x002a0c90` | pin | low if low > x, high if x > high, else x |
| `0x002a29d0` | random_range | `low + ((high-low) * (seed>>16) >> 16)`, 32-bit, 16-bit result |
| `0x002a2a50` | real_random_range | `low + (high-low) * (seed>>16) * (1/65535)` (`0x0045dc50`) on the x87, rounded to single (computed in f64 here) |

The engine's random seed is shared (`*0x004e7408`; about 200 readers across
the engine). The LCG is `seed * 0x19660d + 0x3c6ef35f`.

Wake (`0x00209c80`) leaves finished threads alone and sets wake to 0. If a
saved state exists, it restores that and stops. Otherwise, if the top frame's
node has opcode `0x15`, it pops that frame; if the frame below has it, it
pops two and clears first entry. The test reads node `+2` whatever the node's
kind, so a script-call frame for script index 21 matches too. The VM keeps
that.

**Retail-discarded builtins.** 306 of the 911 descriptors are stubs, and the
VM reproduces them. 297 are `mov ecx,[esp+8]; xor eax,eax; call 0x00209ae0;
ret 0Ch`, which returns 0 *without evaluating its arguments*. Nine evaluate
their arguments, then return 0; `print` (`0x002ab6c0`) is one of them.
`VmImage::from_xbe` decodes each evaluate body (iced-x86) and matches both shapes by mnemonic and operands, including the call targets, the builtin-table operand and `ret 0Ch`.
The VM passes a discarded string stub's text to `Engine::print`. That is an
observation hook, not retail behaviour.

## Binding layer

`Vm::binding(index)` resolves each builtin to one of:

- `Native` (above);
- `Override`, a Rust body registered with `vm.bindings.bind(index, ..)`;
- `Engine`, which goes to `Engine::call` with evaluated arguments and the
  descriptor's evaluate address, so an adapter can call the recompiled
  original;
- `Null`.

The `Engine` trait also supplies:

- game time and tick rate;
- the shared seed;
- object type, object-name lookup and object lists (for casts and `unit`);
- external-global refresh and write-back;
- object-list reference counts;
- the `0x0020a490` object marking (`global_stored`);
- print and inspect hooks;
- the `+0x1120` gate (`sleeps_yield`, default true).

`mock::RecordingEngine` answers by a `Neutral`, `Truthy` or `Random` policy
and records calls. `mock::Coverage` aggregates runs.

## Host policies

These are not retail behaviour. Each reports a `Fault`, stops that thread and
leaves the others running:

- the 1,024-byte stack, counted as `0x10` per frame plus the evaluator's
  cursor reservations;
- a dispatch limit of 2^20 per execution;
- null casts and null evaluators;
- argument type mismatches, which the guest would read as garbage;
- an initialiser that sleeps.

## Validation

`crates/formats/hs/tests/vm_disc.rs`, `#[ignore]`, reads `H2_DISC` (and optionally
`H2_XBE`, `H2_HS_TICKS`, `H2_HS_MAP`) and prints to stdout only. Each of the
30 maps runs under each of the three policies:

1. 3,000 ticks of `initialize` plus `update`;
2. every static and stub script called on a transient thread, with sleepers
   given 300 ticks per batch of 16;
3. every dormant script woken, then 3,000 more ticks.

Every map runs with no fault. The counts per builtin kind, the casts
performed and the share of scripts started are in
[notes/feature-engine.md](notes/feature-engine.md). Per-builtin lists are
printed by the test; they come from map tokens, so they are not reproduced
here.

The synthetic guest differential in
`crates/rewrites/sim-bridge/src/sim_native/hs_differential.rs` runs the
owner's lifted map initialiser and five updates on invented tables. Its
Rust arm receives the same ticks and engine query answers. It compares
salted thread slots, frame depth, wake state, globals, dispatch counts and
the ordered engine calls after initialisation and each tick. The
individual boundary tests exercise sparse slots, nested calls and external
global widths. The fixture data is independent of the executable used
for the lifted instructions.

## Not verified

Open items, in the order the game wiring needs them:

1. Verify the lifted differentials with the owner's XBE. Measure coverage
   on both census B levels, including sleeps, cutscenes, checkpoint
   restore and encounter transitions. Repeat the campaign runs with
   caches cold. Promotion requires matching thread and global state,
   engine effects and identical arena hashes on both architectures per
   the [verification ladder](sim-rewrite.md#verification-ladder-every-phase-climbs-all-of-it).
2. Define the guest adapter contract. `EngineCall` suits synchronous Rust
   bodies and mocks, but two things are missing before recompiled
   evaluators can sit behind it:
   - **strings**: string cells are VM blob offsets or host-table indices,
     not guest pointers. The adapter needs a way to resolve a cell to bytes
     (and a guest address) and to intern engine-returned text, and
     `Engine::call` cannot reach the VM's host-string table today;
   - **suspension**: `call` returns a value or an error. A guest evaluator
     runs on a guest thread frame (`0x00209d50`, `0x00209ae0`) and can leave
     its frame pending. The adapter needs either provisioned guest
     thread/frame records with those helpers intercepted, or a
     suspend/resume result, plus integration tests for both.
   Then build the guest-backed `Engine`: its `call` builds an argument frame and calls the
   recompiled evaluator at `EngineCall::evaluate`. Fixed-signature evaluators
   only read argument slots after `0x00209d50` returns, so the adapter can
   pre-fill them.
3. Resolve the `+0x1120` gate, the category-4/command-script path
   (`ai_command_scripts_initialize` `0x00257d00`), and the console path
   `0x002093d0` in full.
4. Move engine-facing builtins into Rust one family at a time with
   `Bindings::bind`, starting with the most-called pure queries: `list_get`,
   `list_count`, `game_difficulty_get`.
