# Campaign cache integration gate

Status: Built.
Code: `crates/extensions/game/src/map_slots.rs`, `crates/extensions/game/src/map_copy.rs`, `crates/runtime/vfs/src/campaign_maps.rs`, `crates/rewrites/engine/src/map_status.rs`, `crates/rewrites/sim-bridge/src/sim_status_poll.rs`, `crates/apps/cli/src/sim_cmd/cache_template.rs`, `crates/runtime/vfs/src/host.rs` (`seed`), `crates/platform/xapi/src/vfs_storage.rs` (`mount_cache`). Tests: `crates/runtime/kernel/tests/cache_slots.rs` (ignored; needs the disc), `crates/rewrites/engine/tests/map_status_differential.rs` (needs `H2_XBE`), `sim_cmd::cache_template::tests`, `map_copy::tests`, `crates/platform/xapi/tests/cache_seed.rs`.
Switches: `impl.sim.status-poll` ([The status poll](#the-status-poll), on by default), `map-copy.lobby` ([Copy speed on the loading screen](#copy-speed-on-the-loading-screen), on for light hosts), `H2_SIM_CACHE_TEMPLATE` ([Sim batches: a warm Z:](#sim-batches-a-warm-z), on by default).

`crates/runtime/kernel/tests/cache_slots.rs` is an opt-in retail integration test.
It reads and analyses the supplied XBE at runtime, lifts the cache path and its
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
The fixture initialises the XBE TLS template and publishes its signed index
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
[Generated offers](../game/maps-new-maps.md#package-offers)

Without invalidation, a stale multiplayer copy persists until another
multiplayer map loads. A campaign copy can survive up to two other
levels. The game keeps no copy
of the header outside guest memory during a session, so the on-disk
slot header does not matter. The record's resident header is the only
thing checked.

**The fix: the `map-slots` feature** (`crates/extensions/game/src/map_slots.rs`).

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
  [VFS content maps](../../crates/runtime/vfs/src/content_maps.rs)
- **What it leaves alone.**
  - Slots 0-2: copies of other maps check their headers as dependencies
    (state 4, `0x214390`).
  - The copy failure table (`0x55acac`): a path that failed three times
    stays refused for the session.

**Native readers open the slot's source.** The renderer's loaders, the
HUD kit and the menu overlay read the running map through
`native_map::open`, which opens the file the active slot was copied from
(`map_slots::active_file`, published by `loaded_map` before
`GAME_ACTIVATE` and again at the start), not a path built from the map's
name. A disc map, a mod or Live map in the data partition's content
store (`m-<id>-<MapId prefix>`), a Forge build and a rewritten or
spliced file (`vfs::FileSource::Memory` or `Spliced`) all resolve
through it. A slot path the VFS cannot serve fails the open, naming the
path; only a map that is not the running one (a disc map named by a
caller) is found by `<cdrom>\maps\<name>.map` or the disc's headers.

The mount lives as long as the boot (`boot::run_configured`). The map
builds take one loader at a time (`native_map::begin_load`), so a world
load queued behind a long object preload can run after the guest has
ended; its open then fails with "the game ended before the load ran",
not "the map's VFS is not mounted", which means no boot mounted one.

**Reloading an editor play copy.** Queueing the same scenario can hit the
active slot without opening the source. The copier closes the DVD source
after publishing its header. Subsequent resource reads use the cache slot's
file handle. The warm lookup, source close and resource routing are covered
in `crates/runtime/kernel/tests/cache_slots.rs`.

For `map.reload {file}`, a worker prepares and validates the play copy.
At the tick boundary the agent checks the active source, idle copier and
readability of the pending options and their flag. It reserves the DVD source and clears the first byte of each matching
resident path. An open guest handle refuses the reservation. If a write
fails, the host retains the original keys for rollback. It restores a
key only while that slot still belongs to the old source. A slot claimed
by a later load keeps its new key.
The cache file, slot handle and other header fields stay intact.

After an interrupted queue call, the host compares the pending options
with the reload's options before the transition runs. A match publishes
the binding and applies the staged skull selection. Missing, superseded
or unreadable options finish the request with an error and release the
DVD reservation. Later ticks retry any outstanding key restoration.

The expected sequence for a same-map reload is a **static inference**.
The pending transition tears down the active slot, the path lookup at
`0x214a60` misses, and state 1 reopens the DVD source for the retail
copier. The storage fixture exercises the individual copy and warm-lookup
paths. An end-to-end fixture for active-slot teardown followed by that
reopen is pending. [Level boot](level-boot.md#how-the-game-starts-its-first-map)

Tests: `map_slots` (synthetic guest memory: drops, the current slot, a
copy in flight, the shared slots, a refilled slot) and
`crates/runtime/vfs/tests/packages.rs` (`map_changed`). Nothing here is checked
in game; see the lane report for the in-game checks.

Known gaps:

- An explicit `H2_MAP_DIR` override for `shared`, `single_player_shared`
  or `mainmenu` takes effect when its slot is refilled. Slots 0–1 are on
  Z:, which survives a reboot.
- Without an explicit file rebind, a map loaded again straight after itself
  can reuse its current slot. The multiplayer
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

**The fix: the `map-copy` feature** (`crates/extensions/game/src/map_copy.rs`, on by
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
nothing drawn. The flush loop's call is left alone.
`H2_FEATURES=-map-copy.boot` uses the 16-step budget for all slots.

**The main loop's copy.** The main loop steps the machine once a frame
too, only while the job-done byte is set (the test at `0x12b564`, the
call at `0x12b56c`, returning to `0x12b571`; it ignores the result). That
is the copy the System Link lobby waits on while the match's map goes
into slot 3: 1 MiB every two main-loop frames. On a light host (a sim or
a dedicated host) the sub-feature `map-copy.lobby` answers that call the
same way, with the 16-step budget: the same calls in the same order with
the same job results, in fewer frames. The main loop calls the machine
every frame, idle too; an idle machine gets the original call, and a copy
that call starts is boosted from the next frame. It is on by default there
(`profile_switches`, `crates/apps/boot/src/registration.rs`) and off
elsewhere; `H2_FEATURES=-map-copy.lobby` is its A/B arm. The title still owns reads, checksums, invalidation, flushes and header
publication in either mode.

Tests (`map_copy::tests`, a synthetic copy machine and worker): the same
bytes, checksum and step sequence in 3 frames instead of 42 for 21
chunks; from the main loop's return address with `map-copy.lobby`, the
same bytes, checksum and steps in 6 frames instead of 82 for 41 chunks;
a slow worker; a job that outlasts its yields; idle; the plan.

## The status poll

A static read of the 1.0 XBE, and a memo of its answer under
`impl.sim.status-poll` (on by default; `crates/rewrites/engine/src/map_status.rs`,
the hooks in `crates/rewrites/sim-bridge/src/sim_status_poll.rs`, registered
on light hosts).

- **The poll.** `0x163680` (called by `0x163820`) picks the map to load
  next. Each call asks the status `0x213f10` about scenario paths held in
  `0x4687fc`, `0x468800`, `0x468804` and `0x468808` (the main menu's is
  `scenarios\ui\mainmenu\mainmenu`): two to five calls, each from the same
  stack depth with the same paths, every frame of a game.
- **The status.** `0x213f10` (EAX a C string) returns 3 when a slot holds
  the path (`0x214a60`, case-insensitive over the six resident paths at
  `0x557cb4`, stride `0x804`), 4 when a copy request names it and its state
  at `+0x104` is 3 or more (`0x214060`, exact compare over eight requests
  of `0x108` bytes at `0x55acac`), 2 when the first queued path
  (`0x55bd21`) matches and `0x55bf24` is not -1 (`0x213e30`), 1 when the
  second (`0x55bf28`) matches and `0x55c028` is not -1 (`0x213e90`), else 0.
  Each step normalises its side again with `0x214db0`: the name after the
  last `\` (a `t:\` or `w:\` path whole), cut at the first `.`, lowercased,
  then `sprintf` to `d:\maps\%s.map` (`%s.map` for `t:\` and `w:\`). One
  answer costs two to seven `sprintf` calls and their CRT string compares.
- **What it reads and writes.** Besides the path: the six slot paths, the
  request block, the bytes `0x55bd21` to `0x55c02b`, and the CRT locale
  handle `0x480ce8` (`tolower`, `0x321846`, reads tables only when it is
  not 0). It writes only its own stack below the entry ESP, preserves EBX,
  ESI, EDI and EBP, and leaves EAX, ECX, EDX and the arithmetic flags,
  whose last writer is `add esp, 100h`.

`engine::map_status::Cache` keys an answer on the entry ESP, the path
pointer, the path's bytes (at most 240, so every compare ends inside the
blocks), and a copy of everything else listed above. A `before` hook on
`MAP_STATUS` returns the kept EAX, ECX, EDX and arithmetic flags on a hit;
on a miss the original runs and an `after` hook keeps its answer if the
path and the blocks still read as they did at the entry. Any change to a
read byte is a miss, so the copy machine, `map_slots` and a loaded
snapshot need no notice. A longer path, a queued path with no NUL in its
block, or a locale other than C runs the original every time; the first
such call prints one line. Not reproduced: the original's stores to the
dead stack, and the continuation token the `after` hook's shadow call
leaves at the entry ESP during a miss.

Tests: `map_status::tests` and `sim_status_poll::tests` (synthetic guest
memory: keys, every block, a change during the call, bypasses, the plan);
`map_status_differential` (needs `H2_XBE`): the status and the frame poll
against the lifted original over seeded slots, requests and queues, each
case run seeded (a miss), with fresh dead-stack garbage (a hit) and after
one changed byte, comparing every register and every store outside the
dead stack.

## Sim batches: a warm Z:

Each `h2 sim batch` and `h2 sim train` run has a data directory of its own
(`<out>/runs/<id>/data`, [sim.md](sim.md)), so every boot copies maps from
the disc into the cache slots. What survives a boot decides what can be
kept warm.

**What makes a map cached.** All of it is in files under the data root,
and the boot reads it back:

- **The preferences.** `z:\preferences.dat` (0x1fc bytes, signed) is
  read at boot (`0x121350`) before the cache services start. A missing or
  invalid file leaves the defaults (`0x121220`): language -1 at
  `0x510838`, build 0 at `0x510a08`. The cache services (`0x2136c0`) then
  compare those with the running language and build `0x2651`; on a
  mismatch they delete `cache000.map` to `cache005.map` on both Z: and N:
  (`0x2138f0`) and store the current values, which the main loop saves
  to the file (`0x121120`). So a data directory with slot files but no
  preferences file loses its slots at boot and copies every map again.

- **The slot files.** Slot *n* is `cache%03d.map`: slots 0-1 on Z:
  (`disk/Partition5`), slots 2-5 on N: (`disk/Partition4`)
  ([xapi.md](xapi.md#the-second-cache-volume-n)). Each starts with the
  map's 0x800-byte header (`head` at 0, `foot` at 0x7fc). The copier
  writes a zero header first and the map's header only after the payload
  is flushed, so a valid header means a finished copy
  ([the integration gate](#campaign-cache-integration-gate)). The resident
  record at `0x557c90` is loaded from that header; reopening a slot and
  reloading its header causes no disc read or payload write
  (`crates/runtime/kernel/tests/cache_slots.rs`).
- **The FATX metadata.** Each file's times and attributes are a record
  under `metadata/PartitionN/<name in hex>/record`
  (`crates/runtime/vfs/src/host.rs`, `record_path`). A missing record
  reads as the host file's times.
- **The cache database** (which partition is whose, raw-disk byte
  0x800) is not on disk: the kernel seeds it in memory on every boot
  (`crates/runtime/kernel/src/rawdisk.rs`).

**What survives a boot.** XapiInitProcess mounts Z: without a format
(`fFormatClean = FALSE`, [xapi.md](xapi.md#launch-utility-and-startup)),
so slots 0 (`shared`) and 1 (`single_player_shared`) persist. N: is
formatted by `XMountCacheDrive` on every boot
(`crates/platform/xapi/src/cache.rs`, `VfsStorage::mount_cache`), so the
main menu (slot 2), the multiplayer map (slot 3) and campaign levels
(slots 4-5) are copied again on every boot, from any data directory.
The map the System Link lobby waits for is in slot 3, on N:.

**What the boot accepts.** After that check, `0x213970` opens each slot
file with `OPEN_ALWAYS`. A file whose size is not its slot's capacity is
resized and gets a zero header (and the first such file has the higher
slots' files deleted). A file of the right size has its header read
(`0x2149c0`) and checked by `0x122870`; only slot 1 is also checked
against the DVD. Slots 2 to 5 on N: are taken as slot 0 on Z: is,
so a valid slot file on N: after the format is used.

**What a slot depends on.** The payload is the disc map's bytes. The
header is the map's header with two edits: the path (copy state 4,
`0x214343`) and a FILETIME at `+0x14c`, the guest's clock at the copy.
The FILETIME orders eviction between campaign slots 4-5 and nothing
else reads it. Nothing in Z:'s slots names the run, its seed, its
session or its match. The boot checks slot 1 against the DVD (`+0x144`),
and `0x2136c0` invalidates on the language or build the preferences
hold, so a template made by the same image from the same disc, carrying
its preferences file, passes both.

**What a warm Z: changes.** Only work before the match: a boot that
finds slots 0-1 valid skips their copy, so the loading screen draws
fewer frames before the main menu. The sim writes the shared random seed
(`RANDOM_SEED_PTR`) and the spawn words at the match's first tick
(`crates/extensions/game/src/sim_glue.rs`), and its tick 0 is the match
start. Lobby length and frame counts can already differ between cold
runs (a copy job still running is left to the next frame, `map-copy`),
and repeated runs give identical `state_hashes_v1`
([notes/sim.md](notes/sim.md)). That a warm Z: leaves every tick hash
unchanged is inferred from this, not yet measured: an A/A `h2 sim diff`
of a warm run against a cold one on the same seed checks it.

**The template.** `h2 sim batch` and `h2 sim train` (`play_runs`) keep
the finished cache files per image in `<out>/sim-cache/utility/<key>/`, one directory a part, each
with `disk/` (the files) and `metadata/` (their records):

| Part | Files | Placed |
|---|---|---|
| `utility/` | Z:'s finished `cache000.map` and `cache001.map`, and `preferences.dat` | into the run's Z: before it boots |
| `cache/mainmenu/` | N:'s `cache002.map` holding `d:\maps\mainmenu.map` | into N: after the boot's format ([a warm N:](#sim-batches-a-warm-n)) |
| `cache/<map>/` | N:'s `cache003.map` holding `d:\maps\<map>.map` | the same |

The key hashes the image's build stamp (`<dylib>.stamp`), the disc's
path, length and mtime, and the running `h2`'s, so a new build, disc or
`h2` starts a new template; saving any part removes the other keys'. The
map is not in the key: each map's slot 3 is a part of its own. A Z: part
is saved only with its preferences file, since without it the boot
deletes the slots. A slot 3 part is saved only when its header names
the run's map on the disc; a Map Pack map (a `t:\` path) has none.

- **Populating a run.** Before a run boots, the batch puts the
  template's Z: files and their records into the run's
  `disk/Partition5` with `fclonefileat` (copy-on-write on APFS, so the
  runs share one copy's blocks). Where cloning fails it copies
  (`std::fs::copy`, which shares extents through `copy_file_range` on
  Linux filesystems that can) and the run's status line says
  `copied, no clone`. A Z: that already has one of those files is left
  alone and never becomes the template. The run's other Z: files never
  enter the template.
- **Making it.** The first run of each map that finds a part of the
  template missing claims that map; the batch's other runs of that map
  wait for it to finish rather than copy the same maps at once. It uses
  the parts that exist. If it exits 0, each missing part is staged in
  `.staging-<key>-<pid>-<n>` and renamed into place, so concurrent savers
  (two maps, or two batches on one image) leave one whole part. A batch
  claims once a map: if that run saves nothing, the map's other runs go
  on without.
- **Cleanup.** The batch removes `<out>/runs/<id>/data` after each run
  and `h2 lab` removes every `runs/*/data`
  (`crates/apps/cli/src/lab/simbench.rs`, `remove_run_data`). The
  template is under the image's `--out`, not the batch's, and the run's
  copies are clones, so neither touches it.
- **The switch.** `H2_SIM_CACHE_TEMPLATE=0` (or `off`, `false`, `no`)
  makes every run copy Z: and N: from the disc, the cold reference.

## Sim batches: a warm N:

Every boot formats N: (`XMountCacheDrive`, `VfsStorage::mount_cache`),
and the format stays. Straight after it, before the title opens anything
on N:, the VFS clones the directories `vfs::Config::cache_seed` names
into the empty partition (`Vfs::seed_device`, `HostBackend::seed`): each
directory's `disk/` files and their `metadata/` records, with
`fclonefileat` where it can (`vfs::clone_or_copy`). A sim batch names
the template's `cache/mainmenu` and `cache/<map>` parts in
`H2_SIM_CACHE_SEED` (joined as `PATH` is) on each child; the boot reads
it on a light host. The boot then finds valid slot 2 and slot 3 headers
([what the boot accepts](#sim-batches-a-warm-z)) and copies neither, and
the System Link lobby's status for the match's map is 3 at once.

The batch seeds N: only behind a warm Z:, and slot 3 only with slot 2:
at boot the first slot file of the wrong size (an empty slot 0 or 2)
makes `0x213970` delete every higher slot's file, a seeded one too.

- **Exactness.** The guest reads the same payload bytes. The header's
  FILETIME (`+0x14c`) is the producer's, and orders eviction between
  campaign slots only. The preferences file is the producer's at its
  exit: the language and build, plus menu-time values it saved (such as
  the lobby's last map at `0x51098c`). A cold run starts from defaults.
  That none of this reaches `state_hashes_v1` is not measured; an A/A
  `h2 sim diff` of a warm run against a cold one on the same seed, with
  `impl.sim.arena-hash`, checks it.
- **Failure.** A seed that cannot be placed (a name already there, a
  read error) formats the partition again and prints
  `vfs: N: not seeded (...)`; the title finds an empty N: and copies, as
  a cold run does. A file open on N: refuses the seed as it refuses the
  format. A placed seed prints `vfs: N: seeded with <n> files`.
- **Disk.** On APFS the seeded slots are clones of the template, so a
  run's `disk/Partition4` adds no blocks until the guest writes it.

Tests: `crates/platform/xapi/tests/cache_seed.rs` (a seeded N: holds the
slots and records after its format, an unseeded one is empty, only N: is
seeded, a failed seed leaves it empty); `cache_slots.rs` copies Lockout
into slot 3, keeps it as a seed, formats and seeds N:, and reopens the
slot with no disc read or payload write.

