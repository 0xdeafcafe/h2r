# Xbox storage: write metadata and write-behind

Status: Built. How a guest write reaches the host: metadata, then write-behind. Part of [Xbox storage boundary](storage.md).

### Write metadata

A write or truncation does not store the metadata record on the spot.
Storing it durably cost two fsyncs per guest write: while Halo 2 fills
its cache partition on the main menu, that was about a tenth of each
frame (docs/perf/menu-budget.md). A synthetic bench put it at 16 ms per write
against 0.09 ms (`cargo test -p vfs --release bench_write_metadata --
--ignored --nocapture`). The new write and change times and the archive
bit are stored:

- at `flush` (NtFlushBuffersFile), final cleanup (close), and backend
  drop, as access times are;
- by the backend's `vfs-metadata` thread, which checks every 250 ms for
  entries older than `vfs::METADATA_STALE` (1 s);
- by the write path, once the oldest pending entry is that old, but only
  if the thread could not be spawned. While the game streams writes, a
  store on the write path once a second (two syncs) was most of what
  `touch` cost the game thread;
- by `vfs::flush_pending_metadata()`, which reaches every live backend,
  leaked ones included. `h2 run` calls it when the run ends normally.

**Guarantee:** a crash, `process::exit`, an aborting panic, or a signal
(SIGINT, SIGTERM, a kill; nothing flushes on a signal)
loses at most about 1.25 s of write-time, change-time and archive-bit
updates. File data is covered by "Write-behind" below: at most the
bytes still queued (4 MiB) when the process dies without
`flush_pending_metadata`, which lands them first. A host power loss is
different: only `flush` syncs data, as before. A record lost that way falls back to the
previous record, as a torn one does. The bound relies on the flusher
thread; if it cannot be spawned (logged), only the write path, flush,
close and drop store pending metadata. Every record store holds the
pending map's lock, so a thread's store never lands after a newer one.
Removing a file holds that lock across the unlink and drops the pending
entry only once the file is gone, so a failed remove keeps both the
durable record and the write's pending metadata. Creating a name drops
any pending entry for it and holds the lock from clearing an orphaned
record until the new entry exists. Format holds the lock across the root
swap and clears pending metadata once the volume is empty; a failed format
moves nothing, so every pending entry still describes its file.

### Write-behind

All guest threads run on one host thread, so a host `write` stalls the
whole game, not just the guest thread that issued it. While Halo 2 copies
maps to its cache partition on the main menu (256 KiB NtReadFile and
NtWriteFile pairs), a 250 Hz profile put 10.8% of that thread in file
calls, most of it (355 of their samples) in the write syscall itself. `HostBackend` therefore queues the host data write
on a per-backend `vfs-write-behind` thread and returns.

What stays on the calling thread: path resolution,
capacity and growth checks (DiskFull, FileTooLarge), allocation and usage
accounting, the dirty set, write metadata (`touch`), and the returned
byte count (the whole buffer: the queued write is a `pwrite` loop that
writes all of it). Only the bytes travel, copied into a reused buffer
(a fresh 256 KiB allocation is a fresh mapping, and faulting it in cost
more than the copy), with an `Arc` of the kept-open handle; an evicted
handle's last copy closes on the writer thread. One FIFO thread writes them, so writes land in issue
order, per file and overall; overlapping writes resolve as they would
synchronously.

What the guest sees while bytes are in flight:

- `stat`, directory listings and the EOF/allocation every later call
  checks use a pending end of file per host path: the furthest byte any
  queued write to it reaches, combined with the host length (looked up
  first, so a write landing in between is still counted). Usage is the
  incrementally updated count, so volume queries agree too.
- Anything that reads or changes a file's host bytes or identity first
  waits for that file's queued writes: `read_at`, `set_len` (and
  `overwrite_allocated`), `flush`, `cleanup` (which flushes a written
  file), `remove`, `create` at the path, and `host_file` (an outside
  reader sees the bytes). `format` and backend drop wait for all of them,
  and so does `vfs::flush_pending_metadata`, so `h2 run`'s normal exit
  lands queued data before it stores metadata.
- `flush` keeps its meaning: queued data lands, then the file is synced.

Host write errors: the guest's write already returned success. The first
failure for a path is logged once and latched; the next operation on
that file that waits for it (`read_at`, `set_len`, `flush`, `cleanup`,
`remove`, `create`), or the next `write_at`, returns it, once, and does
nothing else. Bookkeeping assumed the write landed, so usage and the
directory indexes are rebuilt from the host at that point. A successful
format drops latched errors with the files; a failed one keeps them. A drop logs nothing more: the failure was
logged when it happened. Nothing below the backend sees the difference
between a failure reported late and one reported at once, except that
a failed write's bytes may be partly on the host.

Memory is bounded: at most 4 MiB (`MAX_IN_FLIGHT`) is queued or being
written; a write that would go over waits for the writer to catch up (one
write larger than the bound is admitted once the queue is empty). A
sustained write rate above the host's therefore throttles the caller
back to synchronous speed, never further.

A crash, `process::exit` without `flush_pending_metadata`, or a signal
loses at most the queued bytes; a process that exits normally or drops
its backends loses nothing. `HostBackend::with_write_behind(false)` gives
synchronous writes, as does a failure to spawn the thread (logged). A
differential test runs the same random sequence of creates, overlapping writes, reads, stats, truncations, flushes,
cleanups, removals, listings and volume queries with write-behind on and
off and compares every result and the final host bytes
(`crates/runtime/vfs/src/host_write_behind_tests.rs`). `cargo test -p vfs
--release bench_write_behind -- --ignored --nocapture` measures the
calling thread's time per 256 KiB write either way, with the caller busy
for 0, 1 or 5 ms between writes. On a heavily loaded host (load average
about 130) the median went from 300 to 600 µs synchronously to 33 to
60 µs queued with room between writes; back to back, backpressure holds
both to the host's rate (about 270 µs each).

Persistent metadata lives under `metadata/PartitionN`, outside guest-visible
mounts. Each lowercase path component is hex encoded as a metadata-directory
component; a `record` contains `HVF1`, the little-endian u32 attributes and
four little-endian u64 times, 40 bytes total. A backend builds each guest
path's record path once and keeps it (`RecordPaths` in
`crates/runtime/vfs/src/host.rs`): every read asks for its file's record. Missing/truncated/invalid records
fall back to host defaults; a later successful metadata write repairs them.
Updates write and fsync a staging file, rename it over the record, and fsync
the containing directory. New metadata directories are synced through their
parents. Removal unlinks the file (or, after removing known litter, rmdirs
the directory) **before** retiring metadata. ENOTEMPTY, permission errors,
or litter cleanup failure therefore leave the entry's attributes/timestamps
intact, durable record and pending write metadata alike. If a successful
unlink or rmdir is followed by a metadata retirement error, the deletion
succeeds and logs a warning; the orphan record is never read. Creation clears any orphaned metadata subtree
before adding a new host entry, so later recreation cannot inherit stale
attributes or child records. Post-removal parent-sync errors are also logged
without undoing a successful namespace operation. Preserve both `disk/` and `metadata/` when
backing up storage.
Out-of-band removal/recreation of a host file can retain obsolete metadata.

Host allocation is emulated in 16 KiB units, including one unit per directory
below the volume root. Growth holes read as zero but count toward logical
allocation even if the host stores them sparsely. Volume capacities are
emulator defaults: Partition1 5 GiB, Partition2 500 MiB and each cache 750 MiB;
`HostBackend::with_volume` customizes label, serial and capacity. Growth
checks logical quota and FATX maximum size before writing; host ENOSPC is
also mapped to DiskFull. Available space is cached from eligible host entries and
does not promise the host has that much physical free space. The root,
metadata, FAT structures, fragmentation and directory growth are not charged
as a physical FATX allocator would charge them.
