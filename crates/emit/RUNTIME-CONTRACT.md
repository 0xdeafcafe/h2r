# What generated modules need from the runtime

This is the contract between the C the emitter writes (`crates/emit`) and
the runtime that loads it (`crates/runtime`). It covers what a generated
module imports, what it exports, and how the runtime has to register and
drive it. It is written against `h2abi.h` ABI v2 (v2 adds the preempt
word prefix of "Loop safe points"; a v1 host must reject v2 code). The layouts there
(`CpuAbi`, `X87Abi`, `TransferAbi`) and the named version probes are used
as they are. The emitter's support header is `crates/emit/c/h2gen.h`. It
is copied next to every generated module and declares everything below.

Items marked **new** were added to the runtime for generated modules (the
shims in `crates/runtime/src/generated.rs`, the registration adapter
`Dispatch::register_generated_module`). The others predate them and are
listed so the contract is complete.

## 1. Symbols a generated module imports

### 1.1 Memory: `h2abi_v1_read`, `h2abi_v1_write` (existing)

These are used exactly as `h2abi.h` documents them. Every guest load and
store in generated code is one call, and `alignment` is always 1. The
16-byte alignment rule of the aligned SSE forms is checked by the
generated code before the access (`#GP`, code 7 below), so the runtime never
sees a misaligned aligned-SSE access. A non-zero status is an event id. The
handler then returns `FAULT`/`ENTRY` with that `event_id` and `eip` = the
instruction's PC, and has made no other change for that instruction (the
IR orders faulting accesses before register and flag writes). The one
exception is a REP string instruction (section 4): iterations completed
before the faulting one keep their stores and their ESI/EDI/ECX progress.

### 1.2 Atomics: `h2abi_v1_atomic_rmw`, `h2abi_v1_xchg`, `h2abi_v1_cmpxchg` (existing)

Every supported LOCK-prefixed instruction (all but `lock adc`/`lock sbb`,
which stop as unsupported, and `lock cmpxchg8b`, which is not lowered), and
every `xchg` with memory, is exactly one call to one of these. Nothing brackets them and no lock is held between
calls. A handler may return, fault or stop at any point without leaving
anything open. The generated sequence for one instruction is:

1. Compute the address and the operand in locals (pure code, no guest
   access).
2. Store the eight GPR locals, EFLAGS (`| 2`) and the defined mask into
   `CpuAbi` (`H2_SYNC_OUT`).
3. Call the accessor with `out` pointing at a local `uint32_t` (never into
   `CpuAbi`, and never into the context).
4. If the status is non-zero, return `FAULT` with that event id and `eip` =
   the instruction's PC. The epilogue stores the locals again. They are
   unchanged, so `CpuAbi` is what it was before the call.
5. Otherwise reload the GPRs, EFLAGS and the defined mask from `CpuAbi`
   (`H2_SYNC_IN`). For `xadd` and `xchg`, merge `out` into the source
   register's slice (AL..BH/AX../EAX..).

What the runtime must provide (all of it is in `h2abi.h` and `atomic.rs`
today):

| IR operation | call | tag | the runtime must |
|---|---|---|---|
| `lock add/sub/and/or/xor` | `atomic_rmw` | 1, 3, 5, 6, 7 | store the result and set the six status flags (logic ops: CF = OF = 0, AF undefined) |
| `lock inc/dec` | `atomic_rmw` | 8, 9 | as add/sub 1, with CF and its defined bit preserved |
| `lock neg/not` | `atomic_rmw` | 10, 11 | neg: flags of 0 - x, CF = (x != 0); not: no flag change |
| `lock bts/btr/btc` | `atomic_rmw` | 12, 13, 14 | width 2 or 4 bytes; bit `operand mod (width * 8)`; the address is already displaced for a register offset; CF = the old bit; OF/SF/AF/PF undefined; ZF preserved |
| `lock xadd` | `atomic_rmw` | 1 (ADD) | as add; `out` = the old value |
| `xchg mem, reg` (with or without `lock`) | `xchg` | none | store `value`; `out` = the old value; no flag change |
| `lock cmpxchg` | `cmpxchg` | none | compare AL/AX/EAX (by width) with memory; equal: store `replacement`; otherwise load the accumulator slice from memory; flags of `acc - mem` |

The generated code relies on these semantics:

- The whole span is validated before anything changes (write permission
  included), and memory and the `CpuAbi` changes are committed together
  under the runtime's memory lock, or not at all. `out` is written after the
  lock is released; that is safe because it is a local of the calling
  handler, disjoint from guest memory and from `CpuAbi`. On a fault,
  memory, `CpuAbi` and `out` are untouched.
- The accessor may read any GPR, EFLAGS and the defined mask, and change
  only what the table says. Undefined flags are stored as 0 with their
  defined bit cleared, the same rule as the IR ("undefined flags store 0").
- Widths are 1, 2 or 4 bytes. The bit operations are never emitted with
  width 1.
- The tags used are 1, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13 and 14. The lifter
  does not emit `lock adc` or `lock sbb` (tags 2 and 4). They are absent
  from the retail image, and the runtime's rule for them (all six status
  flags undefined when CF is undefined) is one the IR cannot express, so
  they stop as unsupported instead. `cmpxchg8b` is not lowered.
- A lock is required against other guest threads. A generated thread only
  ever holds it inside one accessor call, so it cannot deadlock against
  `h2abi_v1_read` and `h2abi_v1_write`.

The emitter's test host (`crates/emit/src/host.rs`) emulates these three
accessors after this table and the runtime's `atomic.rs`. It reports an
unmapped operand as a read fault where the runtime reports a write-access
check. That affects only the event's detail.

### 1.3 `h2shim_v1_event` (**new**, runtime exports)

```c
uint32_t h2shim_v1_event(ExecCtxOpaque *ctx, uint32_t pc, uint32_t saved_eip,
                         uint32_t code, uint32_t arg);
```

This records a non-memory guest exception or a stop, and returns its event
id (non-zero). Generated code returns `FAULT`/`ENTRY` (exceptions) or
`STOP`/`ENTRY` (stops) with that id. `cpu->eip` is `pc` in both cases.
`saved_eip` is the EIP the exception reports to the guest: `pc` for
faults, and the next instruction for the trap-style ones.

| code | name | kind | `arg` | `saved_eip` |
|---:|---|---|---|---|
| 1 | `H2SHIM_DIVIDE_ERROR` | fault (#DE) | 0 | pc |
| 2 | `H2SHIM_BREAKPOINT` | trap (#BP: `int3`, `int 3`) | 0 | next |
| 3 | `H2SHIM_INVALID_OPCODE` | fault (#UD, `ud2`) | 0 | pc |
| 4 | `H2SHIM_INTERRUPT` | trap (`int n`, n != 3) | vector | next |
| 5 | `H2SHIM_OVERFLOW` | trap (`into`) | 0 | next |
| 6 | `H2SHIM_BOUND_RANGE` | fault (#BR) | 0 | pc |
| 7 | `H2SHIM_GENERAL_PROTECTION` | fault (#GP: misaligned 16-byte SSE, `ldmxcsr` with a reserved bit or DAZ) | 0 | pc |
| 8 | `H2SHIM_HALT` | stop (`hlt`) | 0 | pc |
| 9 | `H2SHIM_BAD_ENTRY` | stop: handler entered with a start selector it does not accept | the selector | the selector |
| 10 | `H2SHIM_UNKNOWN_TARGET` | stop: a switch loaded a target outside its validated cases | the target | pc |
| 11 | `H2SHIM_DESCENT_ERROR` | stop: analysis could not follow control past pc | 0 | pc |
| 12 | `H2SHIM_UNMODELLED` | stop: `popfd` would change TF/IF/IOPL/NT | 0 | pc |
| 13 | `H2SHIM_SIMD_EXCEPTION` | fault (#XM): an SSE floating-point exception whose MXCSR mask bit is clear | 0 | pc |
| 14 | `H2SHIM_FP_ERROR` | fault (#MF, vector 16): a waiting x87 instruction, an MMX instruction or `emms` found an unmasked x87 exception pending (status word ES) | the status word | pc |

Code 13 leaves MXCSR with the exception's flag bits set (the guest's
handler reads them) and the instruction's destination unwritten; every other
register is as before the instruction. Hardware raises `#XM` (vector 19)
when CR4.OSXMMEXCPT is set and `#UD` when it is clear; which one the guest
sees is the runtime's choice, as the kernel's CR4 setting is not modelled.

Code 14 changes nothing: the instruction at `pc` is the waiting one that
found the exception pending, not the one that raised it, whose address is
still in the x87 instruction pointer (FIP). `arg` is the status word, which
still holds the exception flags, ES and B; the runtime decodes an `arg`
above 0xFFFF as an unknown event. Every MMX instruction makes the check
first (`h2i_x87_wait`, before any access); `emms` makes it itself. See
docs/FLOAT-ABI.md §2.2 for the x87 helpers' side.

The runtime may map these onto its own `Event` variants. A runtime that
injects guest exceptions (SEH dispatch, the debugger) needs `saved_eip`.

### 1.4 `h2shim_v1_unsupported` (**new**, runtime exports)

```c
uint32_t h2shim_v1_unsupported(ExecCtxOpaque *ctx, uint32_t pc,
                               const char *mnemonic, const char *detail);
```

An instruction the lifter does not implement. This records a diagnostic and
returns its event id. Generated code then returns `STOP`/`ENTRY` with
`cpu->eip = pc`. The strings are static, NUL-terminated ASCII owned by the
module. Copy them if they are kept beyond the call. `detail` starts with
the ISA class (`integer:`, `x87:`, `sse:`, `mmx:`).

### 1.5 `h2shim_v1_rdtsc` (**new**, runtime exports)

```c
uint64_t h2shim_v1_rdtsc(ExecCtxOpaque *ctx, uint32_t pc);
```

`rdtsc` at `pc`: the guest's time-stamp counter, which generated code splits
into EDX:EAX. It cannot fail and records no event. The runtime keeps one
counter for every thread (the Xbox has one CPU), in `crates/runtime/src/tsc.rs`:

- It counts at the Xbox CPU's clock, `runtime::tsc::TSC_HZ` = 733,333,333 Hz
  (a 733 MHz Pentium III: a 133⅓ MHz bus times 5.5). That constant is the only
  definition of the frequency.
- It follows guest time: an embedder installs a clock with
  `Runtime::set_guest_clock` (100 ns ticks; the kernel installs its logical
  clock, the one `KeQueryPerformanceCounter` reads), and the counter is that
  time in cycles.
- Every read is at least `TSC_MIN_STEP` (64) cycles past the one before, so
  a guest spinning on the counter while logical time stands still still gets
  out. With no clock installed the step is all it does.

The emitter's test host and the IR interpreter count reads instead (each read
returns the counter and adds one), so differential tests are exact.

### 1.6 Nothing else

Handlers call no other runtime symbol. The one other thing they read is
the preempt word through the context pointer (§4, "Loop safe points"). x87, MMX and MXCSR state are read
and written directly in `CpuAbi` by `static inline` helpers in `h2gen.h`,
and SSE floating point by the `h2s_*` functions of `h2simd.h` (copied next
to `h2gen.h`, which includes it). Those helpers mirror
`ir::interp::intrinsic` (all but `ReadTsc`, which is §1.5) and need no
runtime support. `cpuid` is constant IR (`lift::lower::XBOX_CPUID`), and
`cli`, `sti` and `wbinvd` lower to nothing. The flat segment model is constant
IR too: `mov r/m, cs` stores selector 8, a far `jmp 0008:target` is a direct
jump, and `sgdt` stores the model GDT limit and base
(`lift::lower::XBOX_GDT`; the base is the mapped, read-only synthetic kernel
PE header, so no GDT is backed. Only the XAPI GDT helper at 0x2d6434 would
notice, and both its callers are bypassed under the HLE: 0x2d1e62 because the
last section is .rdata rather than INIT, 0x2d610c because the saved descriptor
is zero). The 16-bit `sgdt` is not lowered. Port I/O (`in`/`out`) is not lowered: it stops as unsupported. The
three sites are the D3D GPIO guards in `docs/d3d.md`.

MMX registers live in `X87Abi.st`, in physical order: `mmN` is `st[N]`
bytes 0-7, and an MMX write sets bytes 8-9 to 0xFFFF. Every MMX instruction
sets TOP (status bits 11-13) to 0 and the tag word to 0 (all valid); `emms`
sets the tag word to 0xFFFF. The runtime needs nothing for this, but a
debugger or an x87 implementation reading `X87Abi` sees these values.

## 2. Symbols a generated module exports

| symbol | type | meaning |
|---|---|---|
| `h2fn_XXXXXXXX` | `TransferAbi (CpuAbi *, ExecCtxOpaque *, uint32_t start)` | one handler per owner function, named by its entry VA |
| `h2gen_abi_version_<stem>` | `uint32_t (void)` | one per object file (`H2_DEFINE_GENERATED_ABI_NAMED`); every `H2GenEntry` points at the probe of the object that defines its handler |
| `h2gen_module_abi_version` | `uint32_t (void)` | the registration object's own probe |
| `h2gen_module` | `const H2GenModule` | the tables below |

```c
typedef struct H2GenEntry  { uint32_t addr, kind; H2Handler fn; uint32_t (*abi_version)(void); } H2GenEntry;
typedef struct H2GenResume { uint32_t pc, owner, start; } H2GenResume;
typedef struct H2GenModule {
    uint32_t abi_version, entry_count, resume_count, reserved;
    const H2GenEntry *entries;    /* entry_count rows, then a zero row */
    const H2GenResume *resumes;   /* resume_count rows, then a zero row */
} H2GenModule;
```

- `H2GenEntry.kind` 0 is a function entry. 1 is an address-taken interior
  label of the same handler (its own selector). Each `addr` appears once.
- `H2GenResume` says a return to `pc` resumes the handler of function
  `owner` (an `H2GenEntry` of kind 0) with selector `start` (today always
  `pc`). Each `pc` appears once. A `pc` can also be a function entry: a
  continuation that is another function's first instruction (A calls,
  and B starts right after the call). It is then registered as a resume of
  B, and B's entry row stays as it is.
- Every loop header is also a resume of its owner (§4, "Loop safe
  points"), with `start` = the header.

## 3. Registration adapter (**new**)

`Dispatch::register_generated_module` reads a module's tables on top of
`register_generated_object` and `register_resume`:

```rust
unsafe fn Dispatch::register_generated_module(&mut self, module: &H2GenModule,
    module_version: GeneratedAbiVersion) -> Result<RegisteredModule, RegistrationError>;
```

Its semantics:

1. Call `module_version()` and check `module.abi_version` against
   `H2_ABI_VERSION`. Reject the module on mismatch.
2. For each entry row, in table order:
   `register_generated_object(row.addr, row.abi_version, row.fn)`. Per-row
   probes matter because a module's objects can be rebuilt separately.
   Kind 0 and kind 1 rows register the same way: entry dispatch calls the
   handler with `start = addr`, which the handler accepts.
3. Then, for each resume row, `register_resume(row.pc, row.owner,
   row.start)`. Every `owner` is a kind-0 row registered in step 2.
4. On error, report the row and register nothing from the module. The
   runtime's implementation is all-or-nothing. Duplicates are a generator
   bug. Every resume `owner` is a kind-0 row of the same module, and the
   emitter never produces anything else.

## 4. How the runtime drives handlers

- **Entry.** `handler(cpu, ctx, start)`, where `start` is an entry address
  (dispatch) or a resume's `start` (return). Any other selector gives a
  `BAD_ENTRY` stop. The handler loads GPRs and EFLAGS from `CpuAbi` on
  entry. It does not trust `cpu->eip`.
- **Exit.** Every return leaves `CpuAbi` canonical: GPRs, `eflags | 2`, the
  defined mask, and `eip` = the transfer target (or the faulting PC).
  Transfers:
  - `CALL`/`ENTRY`: target = callee, `resume_pc` = continuation. The return
    address is already pushed.
  - `JUMP`/`ENTRY`: a computed jump, a tail jump, a branch side, a
    fall-through or a switch case that lands on another function's entry,
    and any `jmp [kernel slot]`.
  - `RETURN`/`RETURN_TARGET`: the target is the popped address. ESP already
    includes `ret n`.
  - `FAULT`/`ENTRY` or `STOP`/`ENTRY`: `event_id` is non-zero and was
    returned by an import above.
- **Returns resolve through resumes.** The generated module registers every
  continuation the guest can return to, including ones that are another
  function's entry (§2). A return must consult the resume table before
  entry dispatch, so an override of that function does not run in place of
  the caller's continuation. The runtime's current `RETURN_TARGET` order
  (resume, then entry) satisfies this. The emitter's test host has no entry
  fallback for returns at all, so its tests show every guest return in them
  lands on a registered resume.
- **REP string instructions.** A handler may run a whole `rep` loop in one
  call. If an iteration faults, ESI/EDI/ECX hold the completed iterations'
  progress, EFLAGS and the defined mask are as they were before the
  instruction, and `eip` is the instruction. Restarting at `eip` continues
  (next item).
- **Restarting after a fault.** Every REP-prefixed instruction's PC is
  registered as a resume of its owner (`H2GenResume`, `start` = the PC),
  even mid-block. To continue at a FAULT's `eip` (SEH
  `EXCEPTION_CONTINUE_EXECUTION`, a fixed-up access), the runtime resolves
  that PC through the resume table, then entry dispatch, as for a return.
  REP instructions are the only ones registered for restarting, because
  they are the only ones whose fault leaves architectural progress that
  must be continued. Any other faulting instruction can be restarted only
  if its PC happens to be a selector for another reason: a function entry,
  an interior entry or a call continuation (what `plan()` registers). A
  block start alone is not a selector. Otherwise the restart finds no
  selector and must be reported as a stop. The runtime does not restart
  on its own: its runner returns the FAULT to the caller, and continuing
  at the faulting PC (SEH `EXCEPTION_CONTINUE_EXECUTION`) is future work
  that would use this lookup. The Xbox has no demand paging of committed
  game memory, so the remaining case is an SEH handler that resumes at the
  faulting instruction. If that turns out to matter, the planned extension
  is on demand: a list of PCs (from a run's fault log) that the emitter
  registers as extra resumes, not a selector for every memory access.
- **Loop safe points** (**new**). A loop that calls nothing never returns
  to the runner, so the runner's own safe point (every transfer) cannot
  stop it, and a guest spin-wait on another thread would hang the
  cooperative scheduler. Generated code therefore polls at every loop
  header (`crates/emit/src/backedge.rs` finds them: the targets of the
  retreating edges of a DFS over local edges, so every cycle has one):
  - The object behind `ExecCtxOpaque *` starts with a `const uint32_t *`,
    the preempt word, fixed for the context's lifetime. A handler that has
    loops reads it once on entry (`H2_PREEMPT_LOCALS`). Each arrival at a
    header is one relaxed load of the word and a branch (`H2_LOOP_POLL`;
    clang rotates it onto the back-edge).
  - While the word is non-zero, arriving at the header returns
    `JUMP`/`RESUME` with `target_pc` = `eip` = the header, state canonical
    as at the top of the header. Nothing of the header has run.
  - The header is registered as a resume of the owner. Entering with that
    selector skips the poll, so every handler call makes at least one trip
    round the loop whatever the word says: a word that stays set slows a
    loop down but cannot stop it.
  - The runtime sets the word and reads it back at its safe point
    (`crates/runtime/src/preempt.rs`): that is where it delivers pending
    interrupts and where a time-sliced thread gives way. Resuming the
    `JUMP`/`RESUME` it returned continues the loop. A runtime that never
    sets the word sees no change at all; the test host's `Ctx` has the
    pointer first too.
  - No shim event code is used for this: the exit is an ordinary
    resumable transfer, so it records no event.
- **Exclusivity.** Handler pointers are synchronous and do not retain
  `cpu` or `ctx`. One `CpuAbi` is used by one thread at a time. C never
  unwinds.

## 5. Build inputs

Objects are compiled with clang at `-std=c11 -fno-strict-aliasing
-fno-fast-math -ffp-contract=off` plus the profile flags, for
`arm64-apple-macos11` or `x86_64-apple-macos11`, and archived as
`libh2gen-<arch>.a`. `h2 emit` defaults to the host architecture; use
`--arch all` for both, or `--arch arm64` / `--arch x86_64` for one.
`--jobs N` selects clang parallelism (default: available cores). The profile is `-O1 -gline-tables-only` by default;
`H2_OPT` (`0`, `1`, `g`, `s`, `2`) picks the optimisation level and
`H2_DEBUG` (`lines`, `full` for `-g`, `none`) the debug info. The object
cache is keyed on the sources, `h2gen.h`, `h2simd.h`, `h2x87.h`, `h2abi.h`, the
flags, the target and the toolchain identity. Clang dependency preprocessing
also discovers every transitive include for each translation unit, including
system/SDK and include-environment headers. The key includes their contents.
Headers are hashed once per build; dependency discovery is repeated on cache
lookup so newly added include shadows are detected. This work is reported
in the cache stage. The expanded `clang -###` invocation and the contents of
explicit/default configuration files clang reports reading are keyed too;
response-file option expansion is included. This handles config edits under
unchanged environment paths. The toolchain identity covers the
compiler executable, which is `clang` resolved on `PATH` once and then
invoked by that path for every compile: its path, its symlink target and
its content (size and mtime if over 64 MiB). It also covers that
compiler's version and resource directory, the `xcrun` clang, the SDK
path and version, `xcode-select -p`, and the SDK/include environment
variables. A different compiler or SDK never reuses an object.

Objects live in `<out>/obj` unless `H2_OBJ_CACHE` names another
directory, which several checkouts can share: an object's name is
`{stem}.{arch}.{key}.o` with the content key above, and each compile
writes a per-process temporary that is renamed into place, so two builds
never see a partial object. Reusing an object refreshes its modification
time. Each build pins its objects in `<out>/pin/<arch>` (hard links, or
copies across file systems) and archives and links the pins, so another
checkout pruning the shared cache can only cost a recompile. The archive
is written to a temporary, checked with `ar -t` to hold exactly the
build's objects, and renamed into place; any libtool diagnostic is a
failure. An existing archive is reused only if it passes the same check.

With `H2_OBJ_CACHE_MAX` set (`20G`, `512M`, or bytes), `h2 emit` then
deletes objects oldest first until the directory fits the cap. It never
deletes an object of the build that just ran, for either architecture,
even when those alone exceed the cap. The pins hold their objects' disk
space until the next build into the same `<out>`, so the space a shared
cache really uses can exceed the cap by the checkouts' current builds.

The CLI keeps an emission receipt under `<out>` (default `$TMPDIR/h2gen`).
A hit skips analysis, lifting and C emission only after checking the XBE,
package version, executable contents, and the contents and names of every
source and support header. Changing the executable invalidates this receipt
including when the package version is unchanged. ABI installation and the
object cache checks still run. `--no-emit-cache` or `H2_TRACE` forces the
front end to run again. See [BUILD-CACHE.md](../../docs/BUILD-CACHE.md).


The CLI serializes each complete output-directory transaction with
`<out>/.h2-emit.lock`, including receipt validation, source writes, ABI
installation, object/cache/pin work and final link/use. It rejects resolved
output/cache paths inside a git work tree unless `--allow-repo-output` is
explicitly supplied.

After taking the output lock, the CLI rejects every existing descendant
symlink in the output/cache trees, including directory, source/header, ABI,
pin, receipt, library and object aliases. This check applies even with
`--allow-repo-output`; the override changes only the repository-location
policy. The lock file itself is opened with `O_NOFOLLOW`.


Mutable files are published through new sibling inodes and atomic rename:
generated C, support/ABI headers, manifests, receipts, bridge sources and
native tool outputs never truncate an existing destination. This detaches
pre-existing hard links without changing the file at another link name.
Temporary paths are reserved with exclusive creation; unfinished output is
removed on failure. Cached objects keep their immutable hard links/pins,
and host dylibs retain their final install name after publication.
