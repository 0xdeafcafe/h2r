# Campaign cache integration gate

`crates/kernel/tests/cache_slots.rs` is an opt-in retail integration test.
It reads and analyzes the supplied XBE at runtime, lifts the cache path and its
callees, compiles a bounded C module in session scratch, and runs it against the
real kernel/runtime/VFS. No retail bytes or generated code belong in Git.

Run from the checkout with `TMPDIR` set to session scratch:

```sh
RUSTC_WRAPPER= CARGO_TARGET_DIR="$TMPDIR/cache-slots-target" \
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
CARGO_PROFILE_TEST_OPT_LEVEL=1 CARGO_BUILD_JOBS=2 \
H2_XBE=/path/to/default.xbe H2_DISC=/path/to/halo2.iso \
cargo test -p kernel --test cache_slots -- --ignored --nocapture
```

The ISO is opened read-only by `DiscBackend`. Cache files and generated output
are temporary and removed on test exit, including panic. Allow roughly 2 GiB
of disk scratch in addition to the Cargo target. The default volume limits are
750 MiB each; guest memory stays under the runtime's separate 64 MiB budget.

The fixture enters `0x2141f0` at state 3 after opening the source. Retail code
owns header validation, slot choice (`0x214b80`), invalidation, payload work
(`0x1a1100`), MMX checksum (`0x123110`), publication and source close/staging
teardown. Engine job constructors and file workers, including the generated
XAPI file wrappers, run as lifted code. A deterministic FIFO adapter runs their
callbacks and completion flags without starting the full engine thread pool.
The clock and title allocator are the only other overridden entries; generated
XAPI ReadFile/WriteFile/SetFilePointer/GetFileSize/FlushFileBuffers/CloseHandle
call the actual kernel. The job adapter completes each submitted job inline,
so concurrent cancellation and thread scheduling are outside this fixture.
The title arena allocator/free boundaries delegate to the kernel's contiguous
allocator/free; a fixed FILETIME keeps eviction ordering reproducible.
The fixture initializes the XBE TLS template and publishes its signed index
for generated XAPI last-error storage, because it enters after startup.

This gate covers the storage boundary, not a whole campaign activation. It does
not execute `0x18f000`, new-map/BSP callbacks, or GPU/audio resource disposal.
Those remain runner integration checks; stable cache staging mappings cannot
establish that renderer or sound ownership is leak-free. It also does not
replace the engine's map/tag/BSP loader with the Rust map reader.

The run first fills menu (slot 2), shared (0), and SP-shared (1), then introduction
(4), tutorial (5), and Cairo (4). Shared and campaign headers require the menu
record; testing campaign without those prerequisite headers fails retail
validation. Slot selection is also checked with both active campaign slots and
with files exceeding each map type's capacity.

For every file, assertions cover exact payload completion counts and final
header count, the retail accumulated checksum, an independent scalar checksum
of the persisted payload, invalidation before copying, a payload flush before
valid-header publication, and byte-for-byte disk/resident header agreement.
The expected header preserves the source fields with only the retail path and
timestamp edits. Reopening the cache handle, discarding/reloading its header
through the generated read worker, and submitting a warm request must cause
neither disc I/O nor payload writes.

After each campaign transition a generated raw-resource read uses a separately
allocated kernel buffer; freeing it must unmap it. At the end, all four resource
selectors are checked against the correct cache files. Every copy and warm
reload must restore the guest mapping baseline and leave no continuation or
source handle alive. The baseline includes one reusable file-object pool page:
freed kernel pool capacity intentionally remains mapped. Successful formatting
of both scratch volumes after shutdown checks that no cache file object remains.

One-shot short read, short write, disk-full, and corrupted-payload injections
must leave slot 5 invalid, preserve active Cairo in slot 4, close the source,
and free staging. A pause after the invalidation flush and a destination reopen
must expose an invalid cache; resuming the retail copy must recover normally.
This pause/reopen test does not terminate and relaunch the whole runner.
