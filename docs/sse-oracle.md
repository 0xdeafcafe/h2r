# SSE/MMX hardware oracle

`crates/sse-oracle` is a native Rust client for a persistent x86_64 C/assembly
runner on Apple Silicon macOS with Rosetta 2. It measures translated x86
instructions; it is not a physical Pentium III. Results retain register bits,
NaN payloads, signed zeros, MXCSR sticky flags, comparison EFLAGS and memory
writes. Nothing uses C floating-point expressions to execute the target operation.

## Run

From the `sse-oracle` worktree, use scratch for builds and all derived data:

```sh
export CARGO_TARGET_DIR="$TMPDIR/sse-oracle-target"
```

Decode the read-only XBE through `analyze` and iced-x86. Release mode makes the
whole-image analysis substantially faster:

```sh
/opt/homebrew/bin/cargo run --release -p sse-oracle -- inventory /Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/default.xbe > "$TMPDIR/sse-inventory.json"
```

Generate a new JSONL file (refuses to overwrite an existing file). The final
argument is the number of random states per form, default 64. Default output
is approximately 7.6 GB; generation streams and uses a few MB of working memory.

```sh
/opt/homebrew/bin/cargo run --release -p sse-oracle -- generate "$TMPDIR/sse-inventory.json" "$TMPDIR/sse-corpus.jsonl" 64 > "$TMPDIR/sse-counts.json"
```

Audit the completed file, including sticky flags, arithmetic EFLAGS preservation,
eight-mode coverage, schema integrity and captured non-temporal stores:

```sh
/opt/homebrew/bin/cargo run --release -p sse-oracle -- audit "$TMPDIR/sse-corpus.jsonl" > "$TMPDIR/sse-audit.json"
```

Run the independent SDM probes. Each JSON check has `passed`, `actual` and
`expected`. This command reports differences without treating known Rosetta
limitations as a process failure; inspect `passed: false` entries.

```sh
/opt/homebrew/bin/cargo run --release -p sse-oracle -- fidelity > "$TMPDIR/sse-fidelity.json"
```

The C runner is built by `build_runner`, using
`clang -arch x86_64 -O2 -Wall -Wextra -Werror -mno-sse2`, and launched with
`arch -x86_64`. `build` prints its standalone path. Cargo itself remains arm64;
no x86 Rust target, Unicorn, or cross-compiled Rust standard library is needed.
Clang, the macOS SDK and Rosetta must be installed. CLI native build directories
include the PID, so concurrent clients do not overwrite one another's executable.

## Library and live queries

Other crates can depend on `sse-oracle` by path. Build once, keep one `Oracle`
per caller/thread, and reuse the process for many calls:

```rust
use sse_oracle::{build_runner, Oracle, State};

let scratch = std::path::PathBuf::from(std::env::var_os("TMPDIR").unwrap())
    .join("my-sse-oracle"); // use a dedicated directory per builder
let binary = build_runner(&scratch)?;
let mut oracle = Oracle::spawn(&binary)?;
let mut input = State::default();
input.xmm[0] = [0x3f800000, 11, 22, 33]; // 1.0, raw upper lanes
input.xmm[1][0] = 0x40000000;            // 2.0
let observed = oracle.query(&[0xf3, 0x0f, 0x58, 0xc1], &input)?; // addss xmm0,xmm1
assert_eq!(observed.state.unwrap().xmm[0], [0x40400000, 11, 22, 33]);
```

`inventory(&xbe_bytes)` returns reusable `Form` records with `mnemonic`, iced
`code`, executable `bytes`, occurrence count `sites`, and optional `immediate`.
It walks only instruction starts claimed by the converged analyzer, including
its accepted speculative functions. It neither linear-sweeps data nor proves
that all instructions are dynamically reachable. Unknown/rejected regions remain
outside the census. SSE/MMX classification comes from iced CPUID metadata.

`query` accepts exactly one SSE1/MMX instruction. Explicit registers may use
XMM0..7, MM0..7, EAX or ECX; memory must be `[rdi]`. The 16-byte memory slot is
16-byte aligned. The normalizer preserves operand kinds, immediate bytes and
register aliases, maps distinct registers within each class to slots 0, 1, ...,
and replaces address calculation with `[rdi]`. Use a 64-bit iced decoder on the
normalized `instruction` bytes to recover the operand mapping. These are runner
bytes, not an instruction address or the original XBE's addressing bytes.

For a one-off CLI query, `query REQUEST.json` accepts
`{"instruction":[243,15,88,193],"state":{...}}`, where `state` has all the fields
below. It prints an `Observation`. The library is preferable for differential
loops because it avoids spawning and compiling for every query.

## JSONL schema 1

Each line is one `Fixture`, independently deserializable with serde:

| Field | Meaning |
|---|---|
| `schema` | Integer `1` |
| `backend` | `"rosetta2-x86_64"`; do not silently treat as PIII |
| `mnemonic` | Lowercase iced mnemonic, e.g. `addss`, `pshufw` |
| `code` | iced encoding name, distinguishing operand widths/directions |
| `instruction` | Array of unsigned instruction bytes for this normalized form |
| `case` | Deterministic case identifier; combine with bytes and input MXCSR for identity |
| `input` | Full input `State` |
| `output` | `{"state": State or null, "exception": null or signal_number}` |

A `State` is:

| Field | Representation |
|---|---|
| `xmm` | 8 registers, each `[u32; 4]`; lane 0 contains bits 31:0 |
| `mm` | 8 MMX registers, each `[u32; 2]`; low word first |
| `mxcsr` | Raw unsigned 32-bit register; output `& 63` is sticky exception flags |
| `eflags` | Raw unsigned 32-bit captured EFLAGS; compare arithmetic bits with `& 0x8d5` |
| `eax`, `ecx` | Unsigned 32-bit GPR bits, including integer conversion results |
| `memory` | `[u32; 4]`, little-endian bytes at RDI, returned after any store |

All numbers are decimal JSON **integers**. Reinterpret lane bits as binary32;
never parse them as JSON floating-point values. MMX uses split u32 words so
JavaScript also preserves every bit. On a trapped instruction, `state` is null:
there is no misleading completed/partial register snapshot. `exception` is a
macOS POSIX signal (SIGILL=4, SIGFPE=8, SIGBUS=10, SIGSEGV=11), not an Intel
exception vector. The process catches the signal and can answer the next query.

Only CF/PF/AF/ZF/SF/OF are supplied from input EFLAGS. IF and reserved bit 1 are
set in the runner; DF, TF and privileged flags are excluded. COMISS/UCOMISS
outputs include the resulting arithmetic flags. Other instructions should retain
them. MXCSR inputs must have all six exceptions masked, DAZ clear and reserved
bits clear. Sticky input bits are allowed and are not cleared between the load
and the target instruction. The runner restores its host MXCSR afterwards.
LDMXCSR can load a different value from memory, including deliberate fidelity
probes for unsupported bits.

Assembly loads and dumps every register, captures flags immediately after the
instruction, fences stores, and performs EMMS only after capturing MMX values.
It preserves the host ABI. The target instruction is in a W^X page followed by
RET; changing code invalidates the translation cache. Same-instruction batches
reuse the page. The standalone binary protocol is deliberately low level and
trusted: 244-byte requests (`u32 length`, 16 padded opcode bytes, 224 state bytes)
and 228-byte replies (224 state bytes, `u32 signal`), all little endian and in
State field order. Use the Rust validator when accepting instruction bytes.

This schema does not capture x87 tags/control/status, unmasked SIMD exceptions,
32-bit segmentation/address faults, alignment faults, timing or cache ordering.
EMMS fixtures expose raw MMX preservation, not x87 tag transitions. Non-temporal
stores and prefetch/fence mnemonics receive data/state coverage, not a memory
ordering proof. Register alias cases have one physical input value, as on x86.

## Generated coverage

Observed on macOS 27.0 build 26A428 / arm64, using Rosetta and Apple clang 21.0.0.
The analyzed XBE yielded **129,429 sites, 85 mnemonics, 228 normalized forms**.
Forms distinguish register/memory operands, aliases, direction and every
immediate actually found in the XBE; unobserved immediate values are available
through live queries but are not exhaustively generated.

Each form has **22,472 fixtures**: `(52*52 + 64 + 14 + 20 + 7) * 8`.
The 52-entry edge matrix includes ±0, ±infinity, signed QNaNs and SNaNs with
payloads, minimum/maximum denormals and normals, binary32 precision boundaries,
halfway rounding cases, and adjacent int32 conversion boundaries. The same raw
bits represent -0.0 and i32::MIN, so that entry intentionally occurs twice.
Additional cases cover 64 deterministic random states with independent lane bits,
14 shift counts, 20 integer saturation patterns, and 7 pre-existing sticky masks.
Every case runs with all four RC settings and both FTZ settings. DAZ is always
off. LDMXCSR's generated memory value is a legal, masked MXCSR for that mode.
The PRNG is xorshift64 seeded `0x243f6a8885a308d3`, restarted per form.

Total: **5,123,616 fixtures**, **7,635,773,919 bytes** of uncompressed JSONL.
Per-mnemonic counts include all its observed forms:

| Mnemonic | Forms | Fixtures |
|---|---:|---:|
| `addps` | 2 | 44,944 |
| `addss` | 3 | 67,416 |
| `andnps` | 1 | 22,472 |
| `andps` | 2 | 44,944 |
| `cmpps` | 2 | 44,944 |
| `cmpss` | 2 | 44,944 |
| `comiss` | 3 | 67,416 |
| `cvtpi2ps` | 1 | 22,472 |
| `cvtps2pi` | 1 | 22,472 |
| `cvtsi2ss` | 2 | 44,944 |
| `cvttss2si` | 2 | 44,944 |
| `divss` | 2 | 44,944 |
| `emms` | 1 | 22,472 |
| `ldmxcsr` | 1 | 22,472 |
| `maxps` | 2 | 44,944 |
| `maxss` | 2 | 44,944 |
| `minps` | 2 | 44,944 |
| `minss` | 2 | 44,944 |
| `movaps` | 3 | 67,416 |
| `movd` | 4 | 89,888 |
| `movhlps` | 2 | 44,944 |
| `movhps` | 2 | 44,944 |
| `movlhps` | 2 | 44,944 |
| `movlps` | 2 | 44,944 |
| `movmskps` | 1 | 22,472 |
| `movntps` | 1 | 22,472 |
| `movntq` | 1 | 22,472 |
| `movq` | 3 | 67,416 |
| `movss` | 3 | 67,416 |
| `movups` | 2 | 44,944 |
| `mulps` | 3 | 67,416 |
| `mulss` | 3 | 67,416 |
| `orps` | 1 | 22,472 |
| `packssdw` | 2 | 44,944 |
| `packsswb` | 1 | 22,472 |
| `packuswb` | 1 | 22,472 |
| `paddd` | 1 | 22,472 |
| `paddsw` | 1 | 22,472 |
| `paddw` | 2 | 44,944 |
| `pand` | 2 | 44,944 |
| `pcmpeqd` | 2 | 44,944 |
| `pcmpeqw` | 1 | 22,472 |
| `pcmpgtw` | 1 | 22,472 |
| `pinsrw` | 1 | 22,472 |
| `pmaddwd` | 2 | 44,944 |
| `pmaxsw` | 2 | 44,944 |
| `pminsw` | 2 | 44,944 |
| `pmovmskb` | 1 | 22,472 |
| `pmulhw` | 2 | 44,944 |
| `pmullw` | 1 | 22,472 |
| `por` | 1 | 22,472 |
| `prefetchnta` | 1 | 22,472 |
| `prefetcht0` | 1 | 22,472 |
| `pshufw` | 9 | 202,248 |
| `pslld` | 4 | 89,888 |
| `psllq` | 2 | 44,944 |
| `psllw` | 10 | 224,720 |
| `psrad` | 2 | 44,944 |
| `psraw` | 2 | 44,944 |
| `psrld` | 4 | 89,888 |
| `psrlq` | 3 | 67,416 |
| `psrlw` | 4 | 89,888 |
| `psubd` | 1 | 22,472 |
| `psubusw` | 2 | 44,944 |
| `psubw` | 1 | 22,472 |
| `punpckhbw` | 1 | 22,472 |
| `punpckhdq` | 1 | 22,472 |
| `punpckhwd` | 2 | 44,944 |
| `punpcklbw` | 3 | 67,416 |
| `punpckldq` | 3 | 67,416 |
| `punpcklwd` | 3 | 67,416 |
| `pxor` | 3 | 67,416 |
| `rcpss` | 2 | 44,944 |
| `rsqrtps` | 1 | 22,472 |
| `rsqrtss` | 3 | 67,416 |
| `sfence` | 1 | 22,472 |
| `shufps` | 53 | 1,191,016 |
| `sqrtss` | 2 | 44,944 |
| `stmxcsr` | 1 | 22,472 |
| `subps` | 2 | 44,944 |
| `subss` | 2 | 44,944 |
| `ucomiss` | 2 | 44,944 |
| `unpckhps` | 1 | 22,472 |
| `unpcklps` | 1 | 22,472 |
| `xorps` | 3 | 67,416 |

## Committed fixtures (no Rosetta needed)

Normal builds and tests never touch Rosetta. `crates/sse-oracle/fixtures/sse-fixtures-v1.bin.zst`
is committed (**671,747 bytes**, 76,002,648 raw) and embedded in the library with
`include_bytes!`. It contains **all 5,123,616 corpus fixtures**: the complete
corpus compresses to under 1 MB, so no sampling was needed and the
"edge cases plus stratified random sample" selection degenerates to everything:

| Category | Cases per form | x 8 RC/FTZ modes x 228 forms |
|---|---:|---:|
| `edge-I-J` (full 52x52 matrix, both operand orders) | 2,704 | 4,932,096 |
| `random-K` (xorshift64, 64 per form) | 64 | 116,736 |
| `shift-count-N` | 14 | 25,536 |
| `integer-A-B` (saturation patterns) | 20 | 36,480 |
| `sticky-M` | 7 | 12,768 |
| Total | 2,809 | 5,123,616 (22,472 per form) |

```rust
for f in sse_oracle::fixtures() { /* sse_oracle::Fixture, schema 1 */ }
```

`fixtures()` yields the same `Fixture` records as the JSONL schema, grouped by
form, then case, then mode (`rc*2 + ftz` ascending). It decodes lazily, works on
arm64 or any host, and needs no x86, clang or Rosetta. `tests/fixtures.rs`
checks counts per form and category, 8-mode coverage per form, per-mnemonic
totals against the table below, and known outputs.

File layout (zstd level 22 of this little-endian stream):

```
magic "SSEFIX1\0" | seed u64 (0x243f6a8885a308d3) | n_forms u32
per form:  mnemonic, code, instruction  (each u8 length + bytes)
           n_cases u32
           per case: case_id u16, then 8 modes (rc*2 + ftz), each:
             mask u64: bit k = output word k differs from input word k
                       (words: xmm 32, mm 16, mxcsr, eflags, eax, ecx, memory 4);
                       bit 63 = trapped, low byte = signal, no state
             one u32 per set bit in 0..56
```

Case inputs are not stored. They are regenerated from `case_id` by the same
deterministic generator that wrote the corpus (`corpus.rs`: edge pair `i*52+j`
with ids 0..2704, `random-k` 2704+k, `shift-count` 3000+n, `integer` 3100+n,
`sticky` 3200+n), then the mode bits `rc<<13 | ftz<<15` are ORed into MXCSR
(and LDMXCSR's memory operand is set to that MXCSR). `regen-fixtures` verifies
every selected corpus row's input against this regeneration, so a generator
change fails regeneration instead of silently corrupting the fixtures.
Fixtures are generated instruction inputs and outputs only; there is no game data.

### Regenerating

From an existing corpus (no Rosetta; streams it in about 30 s, parsing every row):

```sh
/opt/homebrew/bin/cargo run --release -p sse-oracle -- regen-fixtures "$TMPDIR/sse-corpus.jsonl" crates/sse-oracle/fixtures/sse-fixtures-v1.bin.zst
```

Directly from hardware (explicit opt-in; prints "skipped" and exits 0 if
`arch -x86_64` or clang is absent):

```sh
/opt/homebrew/bin/cargo run --release -p sse-oracle -- regen-fixtures --live "$TMPDIR/sse-inventory.json" crates/sse-oracle/fixtures/sse-fixtures-v1.bin.zst
```

Selection lives in `fixtures::keep` / `fixtures::selected_cases`. If the corpus
ever grows beyond a few MB compressed, narrow `keep` there (the file format
already stores an arbitrary case subset per form; the loader and test counts
follow `selected_cases`). All other Rosetta commands (`build`, `fidelity`,
`query`, `generate`) also skip cleanly when Rosetta is absent, and the hardware
integration test only runs with `SSE_ORACLE_ROSETTA=1`.

## Fidelity and trust policy

The executable fidelity suite checks default NaNs, denormals, FTZ, payload
quieting, MIN/MAX source selection, comparison EFLAGS, rounding, integer
conversion boundaries and approximation accuracy. Its approximation sweep uses
4,096 positive normal values in [1,4), across all eight modes for each of RCPSS
and RSQRTSS: 65,536 queries. These are independent reference expectations, not
expected values copied from the corpus. Test results apply to the measured
Rosetta/macOS version; rerun after upgrades.

Measured result: **444 passed / 449 checks**, with five failed probes representing
three limitations:

| Probe | Rosetta observation | Rule to implement |
|---|---|---|
| LDMXCSR `0x00011f80` or `0x80001f80` | Completes, reports `0x1f80` | Intel requires #GP for reserved high bits; trust Intel |
| LDMXCSR `0xffffffff` | Completes, reports `0xffff` | Intel requires #GP; trust Intel |
| LDMXCSR `0x1fc0` (DAZ) | Completes and retains DAZ | Bit 6 is reserved on PIII: #GP. Modern DAZ support is an architectural difference, not a modern-SDM violation |
| LDMXCSR `0x0000` (clear exception masks) | Reports `0x1f80` | Intel preserves the loaded mask bits. Rosetta cannot establish unmasked exception behavior |

The generated corpus uses masked, legal MXCSR values and DAZ=0, so these
limitations are isolated to the separate fidelity probes. The five failed probes
must remain visible; do not patch the fixture observations to imitate Intel.

No divergence was found in the tested denormal/FTZ cases or default NaN:
`+inf + -inf` and `sqrt(-1)` returned **`0xffc00000` with IE**. The tested SNaN
payload quieted to `0x7fc12345`; MIN/MAX returned the second operand's raw bits.
Exact tiny multiplication produced a subnormal with FTZ off and signed zero
with UE|PE when FTZ was on. FTZ did not act as DAZ.

RCP/RSQRT special-value probes and the approximation sweep passed. Maximum
measured relative errors were **0.000181436539** (RCPSS) and **0.000152209862**
(RSQRTSS), within **1.5 / 4096**. RC/FTZ did not change those approximations,
and no exception flags were raised. Example Rosetta fingerprints:

| Input bits | RCPSS output | RSQRTSS output |
|---|---|---|
| `0x3f800000` (1.0) | `0x3f7ff800` | `0x3f800000` |
| `0x3f801000` | `0x3f7fd800` | `0x3f7ff000` |

**PIII approximation-table identity is unverified.** The SDM specifies an error
bound, not a bit-for-bit PIII table. No physical PIII/Xbox was available for this
run. These fingerprints are Rosetta observations, not confirmed PIII outputs or
confirmed table deviations. Use Intel's special-case and error rules; exact Xbox
RCP/RSQRT replay requires physical PIII captures or a separately verified PIII
table. Do not replace the approximation with correctly rounded division/sqrt
and assume its bits are equivalent.

Full-corpus audit passed: **5,123,616 rows / 228 forms / 85 mnemonics / 0 signals**.
All forms had all eight modes, sticky/EFLAGS checks passed, and all MOVNTPS/MOVNTQ
memory captures matched their source registers.

The Intel sources are the authority when a documented architectural rule and
Rosetta disagree:

- [Intel SDM Volume 1](https://cdrdv2-public.intel.com/843827/253665-sdm-vol-1-dec-24.pdf),
  §§4.8.3, 10.2.3 and 11.5–11.6: NaNs, MXCSR, exceptions and FTZ/DAZ.
- [Intel's earlier SDM Volume 1](https://www.intel.com/content/dam/support/us/en/documents/processors/pentium4/sb/25366521.pdf),
  Tables 4-3, 4-7 and 11-1: indefinite NaN, propagation and masked responses.
- [Intel Architecture Optimization Reference Manual](https://download.intel.com/design/PentiumII/manuals/24512701.pdf),
  reciprocal/reciprocal-square-root approximation and refinement.
- [Intel's PIII-era LDMXCSR reference](https://qcd.phys.cmu.edu/QCDcluster/intel/vtune/reference/vc148.htm):
  bit 6 and bits 31:16 are reserved for the SSE1 target.

## Validation and integration

`cargo test -p analyze -p sse-oracle` exercises the analyzer's zero-immediate
regression, scalar register/memory upper-lane differences, MMX saturation,
non-temporal stores, unaffected register preservation, all 64 pre-existing
sticky combinations, code-cache replacement and signal recovery. The signal
recovery test sends UD2 through the low-level runner; the safe API rejects UD2.
The hardware integration test is opt-in (`SSE_ORACLE_ROSETTA=1`, needs Rosetta/clang); the fixture tests run everywhere.

`cargo fmt -p sse-oracle --check` and
`cargo clippy -p sse-oracle --all-targets --no-deps -- -D warnings` check the new
crate. The analyzer-wide format check also reports existing formatting differences.
The dependency-inclusive clippy command currently stops on existing
`analyze` style lints (manual_is_multiple_of, chunks_exact_to_as_chunks and
unnecessary_sort_by). No workspace-wide style cleanup is part of this change.
Whole-XBE inventory in debug mode relies on the checked subtraction in
`analyze::after_boundary`, which is already on master; this change does not
touch `analyze`.

Consumers should compare output bits, sticky flags and arithmetic EFLAGS, with
explicit exceptions for the limitations above. Use Intel rules for illegal
MXCSR loads and keep approximation-table identity separate from numerical error
bounds. Keep corpus/inventory/reports in scratch, outside the repository and
`disc/`; no game data and no full JSONL corpus is committed; only the compressed fixture set above is.
