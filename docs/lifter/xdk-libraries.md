# XDK library identification

Status: Built. `h2 symbols` identifies XDK library functions by byte signature.
Code: `crates/symbols/src/lib.rs`, `h2 symbols` in `crates/cli/src/main.rs`. Tests: unit tests in `crates/symbols/src/scan.rs` and `solve.rs`.
Switch: none.

The plan replaces Direct3D 8, DirectSound and XAPI at the API level, so we
need to know which functions in the XBE are statically linked Xbox SDK (XDK)
library code and which library function each one is. `h2 symbols` does that
with byte signatures; this page covers what it uses, how it matches, what it
finds in Halo 2, and what it misses.

## What Halo 2 links

From the XBE library table (`h2 info`). Every entry is build 5849 (XDK
5849, the final retail XDK):

| library    | notes |
|------------|-------|
| `XAPILIB`  | XAPI: startup, threads, events, file/path helpers, device init |
| `XBOXKRNL` | import stubs only; no signature database |
| `LIBCMT`, `LIBCPMT` | C/C++ runtime; no signature database |
| `DSOUND`   | DirectSound, in the `DSOUND` section |
| `XVOICE`, `VOICMAIL` | voice chat / voice mail; no signature database |
| `XONLINES` | Xbox Live client, `XONLINE` and `XNET` sections |
| `D3D8LTCG` | Direct3D 8, built with link-time code generation, `D3D` section |
| `XGRAPHCL` | XGraphics (LTCG flavour); no signature database (see gaps) |

Library-to-section mapping is not one-to-one: XAPI startup code sits in
`.text`, the `XPP` section holds the USB/device stack, and D3D code is in
`D3D`. Third-party code (Bink in
the `BINK*` sections, WMA in `WMADEC`) is not part of the XDK and is out of
scope here.

## Where the signatures come from

Cxbx-Reloaded's [XbSymbolDatabase](https://github.com/Cxbx-Reloaded/XbSymbolDatabase)
holds OOVPA signatures (Optimized Offset-Value Pair Arrays): for each symbol
and XDK build, a handful of `(offset, byte)` pairs from the start of the
function, plus references to other symbols' addresses (call targets, data
pointers). Signatures are per library and per build revision, and there is a
separate LTCG database for D3D8 because link-time code generation changes
register allocation and calling conventions.

### Licence

The repo is MIT OR Apache-2.0. XbSymbolDatabase is mixed:

- The library code (`src/lib`, headers) is MIT.
- The signature data (`src/OOVPADatabase`, all `*_OOVPA.c` and `*.inl`, plus
  the `.def` xref lists) carries per-file `SPDX-License-Identifier: ODbL-1.0`.
  ODbL is a share-alike database licence, which does not fit our licences, and
  a translated copy of the data would still be a derivative database.

So the data is **not vendored**. `scripts/setup-xbsymboldb.sh` downloads a
pinned commit into `.tools/XbSymbolDatabase` (git-ignored, same pattern as
`scripts/setup-ghidra.sh`), and `crates/symbols` reads the C sources from
there at run time. The matcher is our own pure-Rust code with no dependency
on the C project. Nothing derived from the data is committed, including this
page's numbers beyond the summary counts.

- Pinned commit: `20eced544726f5558c5a408458f38a086cc4e543`
- Override location with `H2_XBSYMDB=<checkout>` or `h2 symbols --db <checkout>`.

## How matching works

`crates/symbols` is a reader plus a matcher.

**Reading the database.** The sources are C preprocessor macros, so
`parse.rs` is a purpose-built reader, not a C parser. It handles signature
headers (`OOVPA_SIG_HEADER_*`, plus the older `OOVPA_NO_XREF`/`OOVPA_XREF`
form), match bodies (`{ off, val }`, `OV_MATCH`, `XREF_ENTRY`), `#define`
aliases between revisions, `#if 0` / `#ifndef` blocks (evaluated as a default
build), and the `REGISTER_OOVPAS*` table entries. LTCG entries find their
signature through the same name mangling the C macros use (stack size plus
`__LTCG_<reg><index>...`).

**Choosing what to run.** The XBE header gives each library's build number.
`XAPILIB`, `DSOUND`, `XONLINES` and `D3D8LTCG` enable their databases
(`Xapi`, `DSound`, `XOnline` and `XNet`, `D3D8` and `D3D8LTCG`), each
searched only in the sections Cxbx searches for it (for example `.text` and
`D3D` for D3D8). For every symbol, all revisions at or below the library
build run, and the highest revision that matches wins. Entries registered
only for Cxbx's hand-written scanners are skipped.

**References.** Many signatures pin a function by what it calls or reads. A
reference matches either as an absolute address or as a rel32 displacement
to the referenced symbol's address.

- Function references wait until that function has been found (matching runs
  in waves until nothing new resolves).
- Data references (the D3D device pointer, state offsets) have no signature of
  their own; Cxbx derives them from the code that uses them. We probe the tasks
  that wait only on data symbols with those references unchecked, let unique
  hits vote on the value, then re-run the loose tasks with the value pinned.
  Tasks that still match many places with the references unchecked are
  dropped, because a signature with its pinning reference removed matches
  noise (one short D3D signature matched 249 places before this step).

A function's address is recomputed after every wave from the highest
revision that matches uniquely, so it can move. Each finished run records the
addresses it was checked against, and any run whose reference later moves is
discarded and re-run; no result survives on a stale reference
(`crates/symbols/src/solve.rs`).

**Output.** `(va, library, name, xdk_version)` per match, plus flags: the
LTCG variant, a signature that matched at several addresses, and an address
that two different names claim. `xdk_version` is the signature revision that
matched. For the D3D8 LTCG database, revisions below 3911 (for example 1024
to 2060) are variant identifiers inside that database, not XDK builds.

## Using it

```
scripts/setup-xbsymboldb.sh         # once
h2 symbols                          # per-library counts + cross-check
h2 symbols --list                   # every match: va library revision name
h2 symbols --no-xcheck              # skip running the analyzer
```

The cross-check runs `h2 analyze` and reports, per library and overall, how
many identified addresses are discovered function starts.

## What it finds in Halo 2

`h2 symbols` on the retail 5849 XBE (about 8 s, most of it the analyzer):

| database   | found | signatures runnable at 5849 | LTCG variants | on analyzer start | not on a start |
|------------|------:|----------------------------:|--------------:|------------------:|---------------:|
| D3D8       |    18 |                         246 |             0 |                18 |              0 |
| D3D8LTCG   |    66 |                         189 |            29 |                66 |              0 |
| DSound     |   165 |                         328 |             0 |               155 |             10 |
| Xapi       |    43 |                          76 |             0 |                42 |              1 |
| XOnline    |    13 |                          14 |             0 |                13 |              0 |
| XNet       |    10 |                          12 |             0 |                 5 |              5 |
| XGraphic   |     0 |                           - |             - |                 - |              - |
| **total**  |   315 |                             |               |               299 |             16 |

(311 distinct addresses; the 315 includes one address named by four different
weak signatures and CopyRects found by both D3D8 and D3D8LTCG. Per-library
"on analyzer start" counts above are per match, so 295 distinct addresses are
starts and 16 are not.)

- **D3D8 and D3D8LTCG together: 84 matches** (64 `D3DDevice_*`, 9 `D3D_*`, 4
  `CDevice_*`, the rest resource and miniport helpers). Mostly the
  `SetRenderState_*` and `SetTextureState_*` family, `Direct3D_CreateDevice`
  and buffer/surface helpers.
- **DSound: 165**, most of them the `CDirectSound*` buffer and stream methods
  at revisions 3911 to 4134 that the build has not changed since.
- **XAPI: 43**, including the real entry point (`mainCRTStartup`), thread and
  event helpers, and `XapiInitProcess`.
- **XOnline 13 and XNet 10**: the database itself is small for these two
  (14 and 12 runnable signatures).
- **XGraphics: 0.** See below.

Cross-check against the analyzer: 295 of 311 addresses are function starts the
analyzer already found. The 16 that are not are real function starts the
analyzer folded into a neighbouring function (a `push esi` right after
a `ret 14h` inside `XNET`, a `push ebp; lea ebp,[esp-70h]` prologue in `XPP`).
That makes the signature set a useful extra seed list for function discovery
(about 5% of matches here), and the disagreement count a regression number for
the analyzer.

## Known gaps

- **Coverage is partial by design.** Only a fraction of the library is
  signed: 84 of the 435 runnable D3D signatures hit. The gap is mostly
  functions Halo 2's build inlined or compiled differently, not misses on
  present code, but we have no way to tell the two apart without ground truth.
- **LTCG and inlining.** Halo 2 links `D3D8LTCG`, so many D3D entry points are
  compiled with custom register calling conventions and are inlined into
  callers (render-state setters, push-buffer helpers, `Begin`/`End`,
  `DrawVertices`). Cxbx has LTCG signature variants (the `__LTCG_*` entries,
  29 of our 66 LTCG-database matches), and the signatures that exist cover only a subset of the
  functions. Anything
  inlined has no function to find at all, so the D3D replacement cannot rely
  on signatures alone; it will have to hook at higher-level choke points
  (device creation, the push-buffer kick-off) and, where needed, name
  functions by hand.
- **No database for XGRAPHCL.** Cxbx comments out its XGraphics LTCG database
  (`XGRAPHCL`), and the non-LTCG `XGraphic` database only has revisions up to
  4361. Nothing is found.
- **No database for the C runtime or voice libraries.** `LIBCMT`, `LIBCPMT`,
  `XVOICE`, `VOICMAIL` and `XBOXKRNL` import stubs have no signatures.
- **Stalled dependencies.** About 170 signature runs never start because
  they reference a function that was not found (mostly `CDirectSoundVoice_*`
  entries that other `CDirectSoundBuffer_*`/`Stream_*` signatures call).
  `h2 symbols --list` prints which references stalled them.
- **Weak signatures.** Some D3D8 signatures are short and match at the same
  address under several names (one address carries four). They are flagged
  `(shared)`; treat those names as candidates, not facts.
- **Data symbols are only voted.** The derived device pointer and state
  offsets come from majority vote across unique hits; a build where very few
  signatures match uniquely would derive nothing and lose every dependent
  signature.
- **Section limits.** Searches follow Cxbx's section lists. Library code
  linked into other sections (for example D3D helper code in `.text`) is
  covered only where that list says so.
- **Hot-fixed XDK builds.** Halo 2's libraries may carry QFE hot fixes; Cxbx
  special-cases some QFE levels for 5558 to 5659. We do not.
