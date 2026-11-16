# Golden boot traces

Status: design and format only. The `oracle::golden` module, the
`h2 golden-trace` command and the CLI's `unicorn` feature described here are
not on master; the boot's kernel trace (docs/runtime/BOOT.md, `kernel-trace.tsv`) is
what exists today.

`oracle::golden` captures XBE execution using the optional GPL Unicorn backend.
It uses the **same `analyze::cfg::CfgBuilder` leaders as the lifter**. It never
uses Unicorn translation-block hooks. A code hook emits a block entry whenever
execution reaches a CFG leader, including fall-through into a loop leader.
Consecutive callbacks for one REP instruction are collapsed: REP counts as one
instruction event, including the zero-count case. A branch that later revisits
that REP counts as a new instruction. These rules are independent of TB caching.

## Running and profiles

```sh
CARGO_TARGET_DIR="$TMPDIR/golden-target" cargo build -p cli --features unicorn
```

```sh
"$TMPDIR/golden-target/debug/h2" golden-trace /absolute/path/to/halo2.iso --steps 100000 --out "$KEEP/realistic.jsonl" --registers-every 1
```

```sh
"$TMPDIR/golden-target/debug/h2" golden-trace /absolute/path/to/halo2.iso --profile lifter --fixed-time --steps 100000 --out "$KEEP/lifter.jsonl" --registers-every 1
```

The input must be an absolute ISO or XBE path. The ISO's `default.xbe` is loaded
in memory, without extraction or alternate disc search. A denied disc read must
be reported without trying a workaround. `--out` is required, its parent must
exist, and the destination must not exist. An output inside a repository is
refused, including linked worktrees and symlink aliases into a checkout. Retail
traces belong in a directory of your own outside the repository (`$KEEP`
above); scratch files and builds belong in `$TMPDIR`.

Unicorn remains optional and is excluded from default builds. See
[oracle.md](oracle.md) for licensing, architectural limitations, and the known
macOS sandbox cache-probe issue. Host SIGILL is not a guest execution result.

| Contract | `realistic` (default) | `lifter` |
| --- | --- | --- |
| Memory | Sparse XBE mappings with section permissions, separate stack/KPCR/TLS | Exactly `[0, 64 MiB)` flat RWX RAM during execution |
| Initial ESP | `0x70fffffc` | `0x03ffeffc` |
| Return sentinel | `0x74010000` | `0xfffffffc` |
| FS base | `0x73000000`, minimal KPCR | `0x04000000`, immediately outside RAM; FS accesses fault |
| Import slots | Function traps `0x74000000 + ordinal*16`; separate data pages | Unmodified `0x80000000 | ordinal`, including data imports |
| Kernel boundary | Explicit scripts, then resume | Stop before kernel handling, with post-call CPU/stack and arguments |
| Thread handoff | First unsuspended created thread after entry returns | None |
| Lifter comparison | **Not compatible** with the lifter harness | Matches `lift::harness::initial_state` and `run_to_kernel` |

The lifter profile follows the actual unchanged harness: `RAM=64<<20`,
`STACK_TOP=RAM-0x1000`, ESP=`STACK_TOP-4`, `HARNESS_EXIT=0xfffffffc`.
The older ESP `0x03fffffc` quoted during review is not the current harness
contract. Initial CPU values are zero GPRs except ESP/EIP, EFLAGS=2,
MXCSR=`0x1f80`, empty x87 tags, control=`0x037f`, and zero x87/XMM contents.
The decoded entry is used (`0x002d0aee` for the documented retail build).
Headers/sections and virtual tails match the harness loader. No TLS or KPCR is
seeded in this profile. A temporary GDT initializes Unicorn's hidden FS cache,
then is unmapped before execution; it leaves no bytes in the flat RAM image.

The shared CFG post-pass and its analyzer metadata are prerequisites from the
lifter. Golden does not modify the lifter harness. Compare against the
same analyzer revision and leader set, not TB boundaries or function discovery
from another version. Executing outside analyzed instructions stops visibly.

## Scripted realistic startup

Nonempty sections are mapped at their VAs; zero-vsize sections are skipped,
including their unused raw backing. Adjacent pages with matching permissions
are coalesced into mapping/protection ranges. Shared-page permissions are
unioned. Headers are RW for the entry's certificate-size clamp. The stack is
page-rounded, at least 256 KiB using the XBE stack-commit field, capped at
16 MiB, ending at `0x71000000`. Image ranges stay below `0x10000000`.

CS=8 and DS=ES=SS=GS=16 are flat. FS=24 has KPCR base `0x73000000`.
KPCR +0 is the exception sentinel; +4/+8 are stack bounds, +0x18/+0x1c are
self pointers, +0x20 points at the embedded PRCB at KPCR+0x28, and that PRCB's
first field, CurrentThread, is the KTHREAD at `0x73001000`. Thus both
`FS:[0x28]` and `[FS:[0x20]].CurrentThread` resolve to the same KTHREAD.
KTHREAD+0x28 is TlsData. These offsets and the embedding follow the
[Cxbx FS layout](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/support/EmuFS.cpp#L53-L72)
and [KPCR/KPRCB declarations](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/types.h).

The parent TLS at `0x72000000` has a leading payload pointer followed by the
raw XBE template and zero-fill, aligned to 16 bytes. TLS callbacks are rejected.
The guest entry calculates/stores its own signed TLS index. One child may be
handed off to its SystemRoutine with `(StartRoutine, StartContext)` after the
entry returns. Its separate stack ends at `0x70000000`; TLS is at
`stack_top-TlsDataSize`. The raw template and leading pointer are installed
before guest execution, just as for the parent. TLS size zero is allowed and
sets KTHREAD.TlsData to null. A nonzero size must fit the template. The retail
guest system routine also copies/zeros TLS at `0x2d2150..0x2d2191`; correctness
does not rely on that additional initializer. There is no general scheduler.

`golden/exports.rs` has one named inventory of the 135 documented functions
and 17 data exports. Tests check the complete inventory against
[kernel-imports.md](../runtime/kernel-imports.md), including stdcall word counts and
fastcall/cdecl distinctions. Knowing an ABI does not imply a stub exists.
Unknown ABIs and **known but unhandled** ordinals stop, rather than returning
success. This includes KeSetEvent, KeWaitForSingleObject and NtResumeThread.
Suspended creations receive a handle but are never scheduled; attempting to
resume one stops explicitly.

Supported scripts are allocation exports (pool, contiguous, system, kernel
stack and virtual memory), event/mutant/semaphore/thread handle creation,
NtClose, uncontended RtlEnter/LeaveCriticalSection, identity
MmGetPhysicalAddress, and time queries. These are test scripts, not kernel
HLE: allocations are monotonic zeroed RW pages, frees/protections/physical
placement are not implemented, and synchronization is not validated. Handles
start at `0x100` and increase by four. NtCreateMutant has three stack words;
NtCreateSemaphore has four. Each successful return performs exact stdcall
cleanup and preserves nonvolatile registers and EFLAGS. Nonreturning thread
termination, firmware return and bugcheck stop.

All script writes and allocation mappings are recorded, including handle and
ThreadId outputs, virtual base/rounded size, and 64-bit system time. A script
error records any already-completed effects before stopping. Kernel calls are
counted when a trap is reached, even if reading ESP or arguments fails.

Performance counter and system time return the **semantic instruction count**;
frequency is fixed at 1,000,000. This feedback depends on prior execution.
`--fixed-time` freezes counter/system-time values at zero, while preserving the
frequency, for diff runs. Neither mode reads a host clock. RDTSC/RDPMC stop
before execution, including prefixed forms. Other unsupported CPU/system/IO
instructions and memory faults stop. No device or guest SEH is emulated.

Data exports use individual zero-filled RW, NX pages at
`0x75000000 + ordinal*4096`, grouped into ranges when adjacent.
XboxKrnlVersion is scripted as 1.0.5849.0; hardware flags and key bytes are
zero placeholders. Reading function traps as data stops.

## JSONL version 2

One compact UTF-8 JSON object is written per line. No timestamps or host paths
are included. **Skip the first `header` line when diffing runs.** It contains
backend/profile/options/image identity and is provenance, not CPU execution.
Profiles and options still must be checked for compatibility before skipping
it. Version 1 hashes and TB visit sequences are incompatible with version 2.

| `type` | Contents |
| --- | --- |
| `header` | Version 2, backend, script `boot-stubs-v2`, profile/options, entry/base, image FNV-1a hash, FS base and import count |
| `block` | Zero-based `n`, entry `eip`, raw `hash`, `comparable_hash`, `defined_flags`, and full `regs` every K blocks or null |
| `kernel` | Decimal ordinal, name, return EIP/ESP, ordered hex arguments, known ABI flag and optional argument error; partial arguments are retained on failure |
| `return` | Ordinal, `stubbed:true`, scripted EAX or null, `effects` and optional error. Error records do not resume guest execution. |
| `thread` | Scripted guest system/routine/context, TLS and ESP at the single handoff |
| `stop` | Block/instruction/call counts, final EIP, reason, full registers and defined-flags mask |

An effects object has `writes` (guest u32 `address`, little-endian `bytes`
array) and `allocations` (guest `address`, `size`, `zeroed:true`). Addresses in
registers, effects and summary EIP are unsigned decimal. Event EIP/ESP/argument
strings are eight lowercase hex digits without prefixes. Hashes are sixteen
lowercase hex digits. Setup/output errors may leave an incomplete file; only
a complete capture has a `stop` footer. A guest stop is a successful capture,
not proof that the guest booted.

`regs` contains `cpu` (the oracle's CpuState), segment selectors ordered
CS,DS,ES,SS,FS,GS, and `fs_base`. x87 ST byte arrays in snapshots are logical
ST(0)..ST(7); XMM arrays are little-endian bytes. CR0=`0x21`, CR4=`0x600` and
paging disabled are fixed. Unsupported changes to segments/control registers
stop. A block is captured before its first instruction. A faulting instruction
counts once if its code hook ran; traps and sentinel addresses are excluded.

## Pinned register hashes

Both hashes use FNV-1a-64: basis `cbf29ce484222325`, XOR each input byte then
multiply by `100000001b3` modulo 2^64. No JSON serialization participates.
Every scalar below has its stated width and little-endian encoding, with
no padding, lengths, field names or separators.

Raw `hash` has this fixed 300-byte layout:

1. Twelve u32s: EAX,ECX,EDX,EBX,ESP,EBP,ESI,EDI,EIP,EFLAGS,MXCSR,FS base.
2. Four u16s: x87 control,status,tag,opcode; two u32s: FIP,FDP; two u16s:
   code selector,data selector.
3. Eight logical ST(i) arrays of ten bytes each, then eight XMM arrays of
   sixteen bytes each.
4. Six u32 segment selectors: CS,DS,ES,SS,FS,GS.

`comparable_hash` has a fixed 266-byte layout:

1. The same twelve u32s, replacing EFLAGS with
   `EFLAGS & defined_flags & oracle::flags::EFLAGS_MASK`.
2. Three u16s: x87 control,status,tag. FIP/FDP/opcode and FP code/data
   selectors are dropped because they are backend bookkeeping.
3. Eight **physical** x87 registers, ten bytes each (rotate the logical
   snapshots using TOP), then the eight XMM byte arrays.
4. One u32: `defined_flags & oracle::flags::EFLAGS_MASK`. CPU segment
   selectors are omitted because the lifter models FS base rather than a GDT.

Validity starts at EFLAGS_MASK and propagates at completed instruction
boundaries via `oracle::flags::effect`: written defined bits become valid,
undefined bits become invalid, and untouched bits retain validity. A zero-count
REP preserves validity. Consuming an undefined flag stops the run. The mask is
part of the comparable hash, so a validity disagreement is not silently hidden.
Do not compare unmasked raw EFLAGS or FP bookkeeping to the lifter.

Known vectors: empty bytes hash to `cbf29ce484222325`, ASCII `hello` to
`a430d84680aabd0b`. A raw default CpuState with EIP=`0x00100000`, FS base=0
and six zero selectors hashes to `9a2a839a78091ee6`. Tests pin these vectors
and the mask/FP exclusions. Enabling serde_json `preserve_order` cannot alter
these layouts.

## Validation and limits

Run `cargo test -p oracle --features unicorn` and
`cargo test -p cli --features unicorn`. The tests use synthetic XBEs and cover
the CFG loop/fall-through and REP cases, flat harness memory/state/imports,
raw/comparable hash vectors, the whole ABI inventory, stdcall cleanup, every
implemented output shape, unreadable arguments, explicit unhandled stops,
time feedback, template copying/zero TLS, PRCB consistency, zero-vsize sections,
range coalescing, and output path rejection. Default oracle/CLI builds remain
independent of Unicorn.

For a lifter diff, use the same XBE/analyzer, profile, initial CPU/memory and
fixed-time policy. Skip only the header; compare ordered block PCs and
comparable hashes, then kernel boundary state/arguments. With K=1, investigate
the first mismatch field by field using the documented layout and validity
mask. The realistic profile is a separate scripted experiment and cannot be
diffed directly against the lifter harness. Matching hashes do not prove memory
or device equivalence: there is no whole-memory hash, and no Xbox/xemu capture
is implied. Never commit retail traces or disc-derived bytes.
