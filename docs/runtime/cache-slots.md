# Campaign cache integration gate

Status: Built.
Code: `crates/boot/src/map_slots.rs`, `crates/boot/src/map_copy.rs`, `crates/vfs/src/campaign_maps.rs`. Tests: `crates/kernel/tests/cache_slots.rs` (ignored; needs the disc).
Switch: none.

`crates/kernel/tests/cache_slots.rs` is an opt-in retail integration test.
It reads and analyzes the supplied XBE at runtime, lifts the cache path and its
callees, compiles a bounded C module in scratch space, and runs it against the
real kernel/runtime/VFS. No retail bytes or generated code belong in Git.

Run from the checkout with `TMPDIR` set to scratch space:

```sh
CARGO_PROFILE_TEST_OPT_LEVEL=1 \
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
of the persisted payload, a streaming byte comparison against the read-only ISO
from offset 0x800 through physical source EOF, invalidation before copying, a
payload flush before valid-header publication, and byte-for-byte disk/resident
header agreement.
The expected header preserves the source fields with only the retail path and
timestamp edits. Reopening the cache handle, discarding/reloading its header
through the generated read worker, and submitting a warm request must cause
neither disc I/O nor payload writes.

The byte oracle uses a separate, unaudited `DiscBackend` and two 64 KiB buffers;
its reads do not change the guest I/O counts or consume injected failures. Cache
capacity beyond source EOF is excluded, and source/cache truncation is an error.
Synthetic regressions cover checksum-preserving corruption beyond the router
sample, short reads, a final partial-qword byte, and truncated streams. XOR
checksums remain checked as part of the retail publication contract, but do not
serve as evidence of byte-exact copying.

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
The full active Cairo payload is compared against the ISO after every injected
failure and after recovery; the recovered tutorial payload is also compared.
This pause/reopen test does not terminate and relaunch the whole runner.

## Converted Halo 1 campaign slots

At boot an installed converted level larger than 280 MiB increases the
capacity of slots 4 and 5. Both the allocator and the selector read the
campaign row at `0x448fc8`. The header validator reads the new capacity
for Halo 1 campaign headers, bounded by its own 520 MiB check. The VFS
adds two slots' worth of growth to each selectable utility partition and
accounts for the extra space in the synthetic disk geometry. The size
lookup, slot selection and source inventory are documented with the
converter. [Campaign audio](../halo1/sounds.md#campaign-loops-and-cache-slots-built).

## Changed maps

A static read of the slot code (1.0 XBE), made when map packages became
installable while the game runs (docs/game/maps.md §3):

- **Reuse is by path alone.** `0x214a60` normalises the requested path
  (`0x214db0`, which makes `d:\maps\<name>.map`, lowercased) and compares
  it case-insensitively (`0x11c920`) with each slot's resident header
  path. That path is header `+0x20`, which is record `+0x24` from
  `0x557c90` (stride `0x804`). Nothing else is compared: no valid flag,
  size, timestamp or `+0x2d0` checksum.
- **The path comes from the copier.** Copy state 4 (`0x214343`) writes it
  and stamps a FILETIME at header `+0x14c`. The source map supplies
  neither.
- **The source is not opened on a hit.** The status function `0x213f10`
  returns 3 on a path match. The copy machine's idle state, the request
  API `0x213d20` and the loader `0x18eaa0` all take 3 as ready to use.
  The DVD file is opened only in state 1, after a miss.
- **What invalidates a slot.**
  - Being picked for another map of its type: state 7 zeroes the
    resident header and writes a zero header to the slot file.
  - A failed copy.
  - At boot, `0x2136c0`.

  Only slot 1 is ever checked against the DVD again (`+0x144`), and only
  at boot.
- **Slots by type** (`0x214b80`):

  | Type | Map | Slot |
  |---|---|---|
  | 0 | campaign | 4-5 |
  | 1 | multiplayer | 3 |
  | 2 | main menu | 2 |
  | 3 | shared | 0 |
  | 4 | single-player shared | 1 |

  Slots 0-1 are on Z:, 2-5 on N:.

If the bytes at a path change within a session, the game can reuse a
stale copy while the slot still holds that path. Generated mods have
their own `T:\$c\<offer>\<name>.map` paths, so a mod and its retail
base already have distinct cache identities.
[Generated offers](../game/maps.md#package-offers)

Without invalidation, a stale multiplayer copy persists until another
multiplayer map loads. A campaign copy can survive up to two other
levels. The game keeps no copy
of the header outside guest memory during a session, so the on-disk
slot header does not matter. The record's resident header is the only
thing checked.

**The fix: the `map-slots` feature** (`crates/boot/src/map_slots.rs`).

- **When it runs.** After `RENDER_FRAME`, on the main thread, where the
  copy machine `0x2141f0` also runs, at most every 500 ms.
- **What it checks.** Only while the copy machine is idle (`0x55bd04` is
  0), and only for slots 3-5 that are not the current map's (`0x55aca8`).
  It translates the disc or content path to its VFS name and asks
  `Vfs::map_changed`.
- **What it does.** A generated path is stale when its registered source
  changed or was removed and no open handle holds the old resolution.
  The hook zeroes the slot's 0x800-byte resident header, as state 7 does.
  The next request misses. The VFS then resolves the current source if
  the path is still installed.
  [VFS content maps](../../crates/vfs/src/content_maps.rs)
- **What it leaves alone.**
  - Slots 0-2: copies of other maps check their headers as dependencies
    (state 4, `0x214390`).
  - The copy failure table (`0x55acac`): a path that failed three times
    stays refused for the session.

Tests: `map_slots` (synthetic guest memory: drops, the current slot, a
copy in flight, the shared slots, a refilled slot) and
`crates/vfs/tests/packages.rs` (`map_changed`). Nothing here is checked
in game; see the lane report for the in-game checks.

Known gaps:

- An explicit `H2_MAP_DIR` override for `shared`, `single_player_shared`
  or `mainmenu` takes effect when its slot is refilled. Slots 0–1 are on
  Z:, which survives a reboot.
- If a map is loaded again straight after itself without the menu in
  between, its slot stays current and is not dropped. The multiplayer
  lobby runs on the main menu (slot 2), so between games slot 3 is not
  current. That is expected, not checked.

## Copy speed on the loading screen

**The finding.** A level or menu load on a map that is not in its slot
yet is paced by frames, not by the disc. The loading-screen loop
`0x163890` (called from `GAME_ACTIVATE` at `0x18f14b` and from the menu
paths) steps the copy machine (`CACHE_COPY_STEP`, `0x2141f0`) once per
iteration and draws and presents a loading frame in between:

```text
0x163955: progress; if [0x55bd0c] { 0x2141f0() }   ; at 0x1639bd
          SwitchToThread()                          ; 0x2d2084
          draw the loading screen, present, frame wait
          while the copy status is 0, 1 or 2
```

Each step submits one job of at most 1 MiB (a read, then a write; the
sizes at `0x448ff0`) and clears the job-done byte `0x55bd0c`
(`0x1a1286`); the worker sets it when the job is finished, and the loop
steps only while it is set. On the console the DVD set the pace. Here a
read is a synchronous host read, finished by the worker's next slice, so
the copy runs at exactly 1 MiB every two loading frames.

From a 03a_oldmombasa headless boot on fresh data (master 26f45a90,
`H2_FRAME_STATS_CSV`): presents 1 to 1744 are all the loading screen
(20 draws each), 824 of them read exactly 1 MiB, 838 MiB in all (main
menu, shared, single-player shared, then the level), and the level is
loaded at present 1744. The d3d report counts 15 vertical blanks over
those 1,744 presents: the loop does not wait on the display, so guest
time barely moves and the pacer never sleeps. The load costs 1,744 times
whatever one loading frame costs the host: 3.2 to 5.7 s headless, far more
in a window where each present is dearer.

**The fix: the `map-copy` feature** (`crates/boot/src/map_copy.rs`, on by
default, `H2_FEATURES=-map-copy` turns it off). When the copy machine is
called from the loading-screen loop (return address `0x1639c2`), a
`before` hook answers with an async body that runs the step the loop
asked for, then up to 15 more (127 more when the batch starts in payload
state 11 or 12 for one of the shared or menu slots 0–2), each once the
job before it is done,
yielding with `SwitchToThread` as the loop does (at most 8 yields a job;
a job still running is left to the next frame). It stops when the machine
is idle (`0x55bd04` is 0). The machine sees the same calls in the same
order with the same job results, only with fewer frames drawn between
them; the game's own flush loop `0x213fa0` steps it back to back with
nothing drawn. The main loop's call (`0x12b571`) and the flush loop's are
left alone. `H2_FEATURES=-map-copy.boot` uses the 16-step budget for all
slots. The title still owns reads, checksums, invalidation, flushes and header
publication in either mode.

Tests (`map_copy::tests`, a synthetic copy machine and worker): the same
bytes, checksum and step sequence in 3 frames instead of 42 for 21
chunks; a slow worker; a job that outlasts its yields; idle; the plan.
