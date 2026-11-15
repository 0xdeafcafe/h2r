# Checkpoint acceptance

`crates/xapi/tests/checkpoints.rs` is an ignored retail acceptance harness. It
reads `$H2_XBE`, lifts the storage owners, compiles generated C for the host,
and registers it with the real Runtime and kernel. Its game-state bytes and
scenario are synthetic. Generated code, headers and game bytes stay in scratch;
the test never writes the disc.

Run from the repository with clang available:

```sh
RUSTC_WRAPPER= CARGO_TARGET_DIR="$TMPDIR/checkpoints-target" CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 H2_XBE=/absolute/path/default.xbe cargo test -p xapi --test checkpoints -- --ignored --nocapture
```

The harness checks these separate contracts:

- `0x123e20` alternates the two N: snapshots and invokes the real writer at
  `0x2151f0`. One commit uses staging; the other exercises allocation fallback.
  Both retain exact `0x3fe000` completion counts and byte-exact state. Files are
  independently reserved to `0x40b000`, which is not the snapshot size.
- `0x123f60` reverts to each slot after state mutation. The generated read job
  and `0x2152e0` count checks run. State comes from generated XPhysicalAlloc;
  generated XPhysicalProtect enforces read-only backing during snapshots and
  after restore. Attempts to modify protected state fail.
- A fresh Runtime, kernel, handle table and host VFS reopen the same private
  directory under `$TMPDIR`. Full restore at `0x124380` checks the header and
  dispatches the expected synthetic scenario/BSP before reading state. N: files
  are opened without retaining the old writer handles: the full restore helper
  opens with read sharing and cannot share those writable handles. Its input is
  a base path; it appends `.bin`.
- Generated XCreateSaveGame writes the UTF-16LE BOM, Unicode name and NoCopy
  metadata under U:. The hash is checked against independent 128-bit arithmetic.
  Generated find/next/close exercise the 836-byte find-data layout, terminated
  directory/name arrays, enumeration-end error 18 and kernel handle cleanup.
  The directory, metadata bytes and creation time survive Runtime replacement
  without recreating the save.
- A separate U: payload and signature fixture survives the same restart and
  passes through full restore. Generated XCalculateSignatureBegin/Update/End
  use kernel SHA-1 with a synthetic, explicitly seeded signature-key export.
  Split and single updates match; changing state changes the signature; identical
  bytes and key produce identical signatures in the fresh Runtime.
- A small volume fails after a partial write. Exact counts invalidate that slot
  while the older slot remains recoverable. Short reads, a truncated header,
  and incorrect build, allocation identity, backing address or compatibility
  fields fail. Header fixtures close their writable handle before restore;
  a valid control uses the same file and sharing setup. Observers record the
  generated ReadFile result and the bytes seen by both real header validators.
  Each malformed-header case also disables its validator temporarily and
  verifies that restore then succeeds, breaking the rejection assertion.
  Identity failures leave all lifecycle callbacks and state untouched;
  compatibility failures may load scenario/BSP metadata but never enter the
  state callbacks or read the bulk state. A truncated header must show a
  successful short ReadFile and reach neither validator.

The fixture supplies the worker allocator/queue synchronously, then repeatedly
runs the **generated** I/O job until completion. Engine allocators and the
synthetic scenario/options/BSP lifecycle are explicit test seams. It does not
replace checkpoint policy, copies, count checks, header checks, XAPI metadata,
file wrappers or signatures. Native save overrides remain disabled.

This validates the storage boundaries, not a campaign playthrough. The
`game_save -> 0x226050` safety/delay policy, concurrent worker scheduling,
asset-backed scenario loading, save-and-quit UI/profile selection and game-level
signature rejection still need runner acceptance. N: is disposable cache;
reopening its scratch snapshots does not establish persistent resume. The U:
fixture deliberately tests persistence separately. The boot must supply a stable
per-title/console signing-key policy before generated signing runs; the test's
synthetic key is not a retail key or an interoperability claim. Runtime's
unconfigured key DATA exports remain placeholders.

## Reached kernel/VFS fix

Generated XCreateSaveGame at `0x330f74` passes a one-byte initial allocation for
SaveMeta.xbx, aliasing that LARGE_INTEGER with its output IO_STATUS_BLOCK.
NtCreateFile formerly stopped on every nonzero allocation. It now reads the
input before publishing the IOSB and passes it through VFS's allocated-open
methods. Negative sizes are rejected.

HostBackend reserves cluster-rounded **logical FATX capacity** without extending
EOF. Writes consume the reservation; EOF changes and the owner's last handle
close trim unused capacity. Each reservation retains its creating file-object
identity. Independent reader/writer closes cannot release it; duplicate handles
share its lifetime, and overwriting transfers ownership to the new file object.
Ordinary cleanup still flushes data/timestamps independently. Unwritten
reservations are session state, while file bytes and metadata persist.
Capacity checks precede create/overwrite so disk-full failures
do not truncate an older file. Existing ordinary opens ignore the allocation
hint; directories ignore it too. Backends without reservation support stop
before nonzero create/overwrite reservations change data.

The NT initial-allocation contract is documented in
[Microsoft's NtCreateFile reference](https://learn.microsoft.com/windows/win32/api/winternl/nf-winternl-ntcreatefile).
The harness establishes the observed Xbox caller, not every NT filesystem's
physical preallocation policy.
