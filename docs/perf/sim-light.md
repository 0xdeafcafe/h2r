# Light sims: RAM, disk and setup per match

Status: Design, with lane L2 built and not yet measured; the other lanes below build the rest.
Code: `crates/apps/cli/src/sim_cmd.rs` (`batch`), `crates/apps/cli/src/sim_cmd/cache_template.rs`, `crates/runtime/vfs/src/lib.rs` (`Vfs::try_map_read`), `crates/extensions/game/src/map_copy.rs`, `crates/live/sim-bench/src/diff.rs` (`Recorder`). Tests: named in each lane.
Switch: `H2_SIM_CACHE_TEMPLATE`, `map-copy.lobby` and `impl.sim.mapped-reads` exist ([switches.md](../runtime/switches.md)); each lane lists the switch it adds there.

A batch of headless matches (`h2 sim batch`, [sim.md](../runtime/sim.md))
pays per match about 143 to 171 MiB of dirty memory, about 170 MB of disk
while it runs, and 9 to 14 s of setup before tick 0
([notes/sim-10x.md](notes/sim-10x.md), 2026-11-22). This page ranks what
removes each and gives the lanes. One rule holds: the same seed gives the
same `state_hashes_v1`. Per-tick CPU is in [sim-10x.md](sim-10x.md); the
phantoms' action build is in [sim-headless-skips.md](sim-headless-skips.md).

Labels: **Code** is read from this repository, **Run** is a measurement in
a note, **S** is supposed and names what would verify it, **Design** is our
choice.

## Levers, ranked

RAM is private (dirty) MiB per match. Disk is per run while it runs.
Every saving is **S** until its lane's measurement is in a note.

| # | Lever | RAM saved | Disk saved | Setup saved | Exactness risk | Built | Lane |
|---|---|---:|---:|---:|---|---|---|
| 1 | Warm N:: the batch seeds the menu and match-map slots after the boot's format | 0 | about 168 MB (all of N:) on APFS | the lobby's copy wait and the menu's copy, 3 to 7 s | Low: same class as the warm Z:, which leaves hashes equal | yes, unmeasured | L2 |
| 2 | Boost the main loop's background copy on a sim or light host | 0 | 0 | 2 to 5 s where N: is cold (the producer run, `h2 sim run`) | Low: same calls in the same order | yes, unmeasured | L2 |
| 3 | Compact component recorder; `detail = "training"` reports | 15 to 30 with components on | report from about 10 MB to under 100 KB | 0 | None: host-side, same JSON for the full detail | no | L1 |
| 4 | Census in the report: setup phases, read bytes by file, heap by tag, guest pages written | 0 (it prices rows 5 to 8) | 0 | 0 | None | no | L1 |
| 5 | Frozen slots: runs bind the template's slot files read-only, no clone or copy | 0 to 5 | 734 MB + 168 MB a run on a filesystem without clones (ext4, overlayfs) | 0 to 1 s | Low: same bytes; a guest write detaches | no | L3 |
| 6 | Mapped reads from frozen slots, padded for the host page | 15 to 40 on 16 KiB, 20 to 45 on 4 KiB | 0 | 0 | Low: private mapping, same bytes | the mapper is built; it never fires in a sim | L3 |
| 7 | Fork a loaded-level template per map, variant and roster | private drops to what a match writes: 35 to 60 left on macOS, 25 to 45 on Linux | data dir shared; near 0 | all but about 0.5 s | Medium: one process state shared by every seed | no | spike, below |
| 8 | Heap budgets after the census | 10 to 20 | 0 | 0 | Low | partly ([host-halving.md](../live/host-halving.md#halving-the-memory)) | after L1 |
| 9 | Sim stores (`H2_SIM_STORE`) as built | 0 | 0 | 0 | – | built, no use to a sim | none; see below |
| 10 | Linux only: the phantom build's two full RAM copies a tick | up to 128 | 0 | 0 | – | – | [sim-headless-skips.md](sim-headless-skips.md) |

Order of work: L1 and L2 at once (no shared files), then L3, then the
fork spike with L1's numbers. Row 7 makes row 6 redundant where it lands,
but row 5 is its prerequisite: forked children share one data directory.

## RAM

### Why `mapped_read_bytes` is 0

**Code.** A sim is a light host (`crates/apps/boot/src/lib.rs`:
`host_light` is true when `sim_mode` is set), so
`options.map_guest_reads` is on and macOS RAM is the paged mapping
(`Backing::MachPaged`, `crates/runtime/runtime/src/cow.rs`). The store is
not the gate, and flat RAM is off. The refusal is in
`Vfs::try_map_read` (`crates/runtime/vfs/src/lib.rs`), before page
congruence is ever tested:

1. The title reads a level's tag data from its **cache slot**
   (`cache00N.map` on Z: or N:), not from `D:\maps`
   ([cache-slots.md](../runtime/cache-slots.md)). A slot is a host file:
   `HostBackend::film_source` (`crates/runtime/vfs/src/host.rs`) returns
   `FilmMapSource::region`, which has no `immutable_len`, so
   `FilmMapSource::immutable_read` (`film_map.rs`) returns `None`: reason
   `mutable, composed or noncontiguous source`.
2. If the slot's handle is open for writing (the copier writes through
   the slot record's handle), the earlier test fires instead: reason
   `writable handle`. **S**: which of the two; the run's `log.txt` has the
   line `guest mapped read: copying <path> ...: <reason>`, printed once
   per reason.
3. The only reads from an immutable source are the copy machine's 1 MiB
   reads of `D:\maps\*.map`. Their target is the copy buffer, which the
   next job overwrites, so mapping them would save nothing.

So mapped reads cannot apply to a sim until the slot itself is immutable
(L3). Congruence comes after that: guest physical `0x61000` (the title's
arena base, `LoadOptions::halo2`) is 4 KiB into a 16 KiB page, so a slot
file needs a leading pad chosen per host page size.

### Sim stores

**Code.** `SimStoreBackend` serves `D:\maps\<map>.map`. In a sim the one
reader of those paths is the copy machine, which reads every byte from
`0x800` to the end. **S**: a recording from a cold run therefore covers
the whole file, and one from a warm-Z: run holds no read of `shared.map`
at all (`h2 sim store build` prints stored of logical bytes; that checks
it). A store as built saves a sim nothing. The useful form is a *sparse
frozen slot*: record the reads of the slot files and keep only those
ranges in the template (row 5's follow-up; a miss stays a named error).

### The 74 MiB outside guest RAM

**Run**: 143 MiB footprint = 64 guest RAM + 39 other large + 35 small
([notes/sim.md](../runtime/notes/sim.md), Waterworks; no per-consumer
census exists). **Code**, the largest consumer found by reading:
`sim_bench::diff::Recorder` keeps up to `MAX_ENTRIES` = 250,000
`(String, String)` pairs in per-tick `BTreeMap`s, and `finish()` clones
the lot for the report. **S**: 15 to 30 MiB of small allocations in a
9,000-tick match with components on, plus the clone at the end. L1
replaces it with interned names and `u64` values (about 3 MiB) and adds
the heap-tag census (`mem-watch` tags) that names the rest.

### Guest RAM

**S**: the note counts all 64 MiB as dirty. What a match writes after
load is not measured: `guest_written_pages` is only reported over a
copy-on-write base ([sim.md §6.2](../runtime/sim-metrics.md#62-sharing-guest-ram-with-machine-snapshots)).
L1 adds a census without a base: hash every 4 KiB page at the first
counted tick and at the end, report pages changed at 4 KiB and at 16 KiB
grain. That number sizes rows 6 and 7.

### A forked template

One process boots, opens the lobby, seats the roster, loads the map and
stops at the first `GAME_TICK` of the game, before `sim: match seed`
writes the seed (`crates/extensions/game/src/sim_glue.rs`). It forks
once per match; the child takes its seed, bot parameters and report
path, writes the seed and spawn words as a booted run does, and plays.
Guest RAM, the heap and the module tables are shared copy-on-write.

**Code**, what blocks seeding a batch from a machine snapshot today:
`host_lobby`'s policy forbids `state.save` outside a film keyframe
("dedicated network host"); `sim-metrics` and `bot_glue` save their
sections (`sim_glue/film_state.rs`, `bot_glue/film_state.rs`: seat
bindings, machines, the owned squad, the phantom `Books`) only in a film
session; the Live HLE has no section; the
clock-only audio has no section ([host-halving.md](../live/host-halving.md#template-processes)).
A fork needs none of those sections. It needs:

- one thread at the fork (bot think workers, the VFS write-behind
  thread, the agent socket and the memory watchdog stopped, then started
  again in the child);
- per-child state swapped in: `Inner.matched` (seed, bot parameters,
  arms), the brains rebuilt from it, the report path, the XNet port
  block, the agent socket;
- a shared read-only data directory (row 5), with T: and U: writes going
  to a per-child overlay;
- the template keyed by image stamp, `h2`, disc, switch set, map,
  variant and roster (names, teams, order). Seeds and tuning are not in
  the key.

Open: whether macOS lets this process fork safely; whether anything
between `GAME_ACTIVATE` and the first tick reads the match seed or the
bot parameters; the fallback (`state.load` with `"cow": true` in a fresh
process) shares guest RAM only and needs the sections above.

## Disk

**Code**, what a batch run writes (`sim_cmd.rs`, `RunDir`):

| Path under `<out>/runs/<id>/` | Size | Kept |
|---|---|---|
| `data/disk/Partition5/cache00{0,1}.map` (Z:) | 734 MB logical; a clone on APFS, a full copy where `fclonefileat` and reflinks are missing | removed with `data/` |
| `data/disk/Partition4/cache00{2,3}.map` (N:) | about 84 MB each, written by the guest's copier on every boot | removed |
| `data/disk/Partition1` (TDATA, UDATA), `data/metadata/` | **S**: KiB; L1's census reports it | removed |
| `report.json` | **S**: about 0.5 MB of tick hashes plus up to about 10 MB of components (`MAX_ENTRIES`) | yes |
| `log.txt` | the child's stdout and stderr | yes |
| `match.toml`, `audit.json` (audits only) | KiB | yes |

No film, frame dump or telemetry file is written unless `--upload`,
`H2_FILM_OUT` or `H2_TELEMETRY_FILE` asks. A training batch needs the
report's result, players and kills; hashes and components are for
`h2 sim diff`.

**N: copies.** The VFS cannot turn the copier's writes into clones:
macOS has no range clone, and a map's payload in the ISO is 2 KiB
aligned, which Linux `FICLONERANGE` refuses. Two exact routes remain:

- *Seed after the format* (**Design**, built by L2). The boot still formats N:
  (`VfsStorage::mount_cache`, `crates/platform/xapi/src/vfs_storage.rs`);
  straight after, the host clones the template's slot 2 and slot 3 files
  and their metadata records into the empty partition. The title finds
  valid headers and copies nothing, as it does on Z:. The guest reads
  the same payload bytes. What differs from a cold run: the header's
  FILETIME (`+0x14c`, eviction order of campaign slots only), that
  files exist on N: after a format, and the producer's
  `preferences.dat` on Z:. **Code**: the boot's slot pass (`0x213970`)
  takes a valid N: slot of the right size as it takes slot 0; nothing in
  the title invalidates N: alone. It needs the preferences file: without
  it the cache services (`0x2136c0`) see language -1 and build 0 and
  delete every slot file on Z: and N:, so the Z: template carries it
  ([cache-slots.md](../runtime/cache-slots.md#sim-batches-a-warm-z)).
  **Test**: `crates/runtime/kernel/tests/cache_slots.rs` copies Lockout
  into slot 3, formats and seeds N:, and reopens the slot with no disc
  read and no payload write.
- *Elide on write* (fallback if the title rejects a seeded N:). The
  copier's write of bytes equal to the disc's at the same offset becomes
  an extent that points at the disc; the slot file stays sparse. Same
  calls, same bytes, no payload on disk; the copy still takes its frames.

## Setup time

**Run**: 9 to 14 s from process start to the first counted tick
(`setup_s`, `sim_glue::speed`). No breakdown is measured. **S**, from the
code path: module load and hooks 2 to 4 s; the menu's copy and load 1 to
2 s; lobby steps 1 s; the match map's copy 2 to 6 s; level load and seat
binding 1 to 2 s. L1 reports the phases.

**Code**: the main loop steps the copy machine once an iteration, only
when the job-done byte is set (call at research point `0x12b56c`,
returning to `0x12b571`), the same shape as the loading-screen loop
`map_copy` already boosts. Each step is one job of at most 1 MiB and a
job finishes on the worker's next slice, so the lobby copies 1 MiB per
two main-loop frames: about 170 frames for Lockout.

**Is boosting it exact?** The machine sees the same calls in the same
order with the same results. What changes is how many main-loop frames
the lobby runs. What a frame count could leak, and why it does not reach
`state_hashes_v1`:

| Carrier | Why it does not leak | Label |
|---|---|---|
| Shared random seed | written at the game's first tick (`write_seed`) | Code |
| Spawn words | written before the first spawn (`write_spawn_words`) | Code |
| Guest clock at match start | already differs between runs; only the slot FILETIME keeps it | Code |
| Menu-time game state (datum salts, effects on the menu map) | **S**: a new game re-creates its arrays. An A/A run with `impl.sim.arena-hash` checks it | S |
| Lobby frame count itself | cold runs already differ (a job still running waits a frame), and 15 matches repeat exactly under heavy load; a warm Z: run equals a cold one | Run ([notes/sim.md](../runtime/notes/sim.md), [notes/sim-10x.md](notes/sim-10x.md)) |

## Lanes

Each lane writes code only; the coordinator builds, tests and measures.
Measurement for all three: the batch of the 2026-11-22 throughput entry
(eight Lockout seeds), `h2 lab sim --host laptop --branch <branch> --wait
--parallel 8 --build-env H2_OPT=2 <batch>`, against job 0060; then
`h2 sim diff` of every seed against job 0056's reports must print
"every component agrees". Results go in [notes/sim-10x.md](notes/sim-10x.md).

### L1: census and lean reports

- **Files**: `crates/live/sim-bench/src/diff.rs` (`Recorder`),
  `crates/live/sim-bench/src/metrics.rs`, `crates/live/sim-bench/src/config.rs`,
  `crates/extensions/game/src/sim_glue.rs`, `crates/extensions/game/src/host_mem.rs`,
  `crates/apps/cli/src/sim_cmd.rs`, [sim.md §6](../runtime/sim.md#6-metrics).
- **Work**: (a) `Recorder` stores interned component ids and `u64`
  values and serialises without cloning; the JSON is byte-identical.
  (b) `wall.setup` with `boot_s`, `menu_s`, `lobby_s`, `copy_wait_s`,
  `load_s`, `bind_s`, taken at the lobby's step changes
  (`sim_glue/hosted.rs`) and `GAME_ACTIVATE`. (c) `wall.reads`: copied
  and mapped bytes per guest path, and the first refusal reason. (d)
  `wall.heap_tags` from `mem-watch`. (e) `wall.guest_pages_written_4k`
  and `_16k` by page hashes. (f) `wall.data_dir_bytes`, read by the
  batch before it removes `data/`. (g) batch key `detail = "training"`:
  no components, tick hashes every 300th tick plus one rolling digest.
- **Switch**: `impl.sim.page-census` (on; off for timing arms). `detail`
  defaults to `full`.
- **Tests**: `Recorder` old against new JSON on a seeded series,
  truncation and same-tick replacement; metrics JSON round trip;
  a report without the new fields still loads.
- **Gate**: footprint down by the recorder's share with components on;
  `ms_per_tick` within 0.05 with the census off.

### L2: warm N: and the lobby copy pace

Status: built, waiting for its gate measurement. The template's parts,
the seed after the format and `map-copy.lobby` are described in
[cache-slots.md](../runtime/cache-slots.md#sim-batches-a-warm-n). The
first step's finding is **Code**: the title accepts a seeded N: slot,
but deletes every slot file at boot when Z: has no `preferences.dat`,
which the Z: template therefore carries. The warm Z: measured in job
0060 had no preferences file, so its slots were deleted at each boot
(**S**, from the code: it fits that job's unchanged setup time).

- **Files**: `crates/apps/cli/src/sim_cmd/cache_template.rs`,
  `crates/apps/cli/src/sim_cmd.rs`, `crates/runtime/vfs/src/lib.rs`
  (`Config`), `crates/platform/xapi/src/vfs_storage.rs` (`mount_cache`),
  `crates/extensions/game/src/map_copy.rs`,
  [cache-slots.md](../runtime/cache-slots.md), [switches.md](../runtime/switches.md).
- **First**: read the slot-open path with `h2 disasm` and confirm a slot
  file with a valid header on N: is accepted at boot as on Z:. If the
  title invalidates N:'s slots itself, build the elide-on-write route.
- **Work**: (a) the template gains `<key>/cache/mainmenu/cache002.map`
  and `<key>/cache/<map>/cache003.map` with their records, saved from a
  producer run per map (one producer per map; the gate keys on it). (b)
  `vfs::Config::cache_seed` names the directory; after `mount_cache`
  formats the partition the VFS clones the seed's files in
  (`clone_or_copy`). (c) `map_copy` also boosts the main loop's call
  (return address `0x12b571`) on a light host, with the 16-step budget.
- **Switch**: `H2_SIM_CACHE_TEMPLATE=0` turns the seed off with the Z:
  template. `map-copy.lobby` (on for light hosts; `-map-copy.lobby` is
  the A/B arm).
- **Tests**: template save, populate and prune for N: (as
  `cache_template::tests`); a seeded partition after a format holds the
  slots and their records, an unseeded one is empty (`vfs` storage
  test); `map_copy::tests` with the main loop's return address: same
  bytes, checksum and step order in fewer frames; `cache_slots.rs`
  (needs the disc) with a seeded slot 3.
- **Gate**: `setup_s` down by at least 3 s at `--parallel 8`;
  `data/disk/Partition4` adds no blocks on APFS (`du`); hashes equal to
  job 0056 for all eight seeds, and one seed with `impl.sim.arena-hash`
  on in both arms.

### L3: frozen slots and mapped reads

- **Files**: `crates/runtime/vfs/src/host.rs`, `crates/runtime/vfs/src/film_map.rs`,
  `crates/runtime/vfs/src/lib.rs` (`try_map_read`),
  `crates/runtime/vfs/src/sim_store.rs` (the recording gains the
  destination address), `crates/runtime/kernel/src/files.rs`,
  `crates/apps/cli/src/sim_cmd/cache_template.rs`.
- **Work**: (a) a utility partition can *bind* a slot name to an
  immutable template file at an offset: reads and `stat` come from it, a
  guest write copies it up into the run's own file first. No clone, no
  copy. (b) a bound slot's `film_source` is `immutable_region`, and
  `try_map_read` accepts a writable handle on a bound, unwritten file.
  Pages already mapped stay valid after a copy-up: the template file is
  never modified. (c) the template stores each slot behind a pad of 0 to
  one host page, chosen from the recorded bulk reads so the largest
  ones are congruent with their physical target. The pad is per page
  size and part of the template key.
- **Switch**: `impl.sim.frozen-slots` (on in a batch with a template);
  `impl.sim.mapped-reads` stays the copy arm's switch.
- **Tests**: bind, read, `stat`, copy-up on write, and a format of a
  bound partition (`vfs` storage tests); `kernel/tests/files.rs` mapped
  read from a bound slot with a 4 KiB and a 16 KiB pad; pad choice from
  a synthetic recording.
- **Gate**: `mapped_read_bytes` above 0 and footprint down by at least
  15 MiB a match on the laptop; a run's `data/` under 1 MB; hashes equal.

## Rules and limits

- A lever that cannot engage says so in the run's log and report; it
  never falls back silently ([sim-10x.md](sim-10x.md#rules-and-limits)).
- Templates are keyed by image stamp, disc and `h2`; a pad adds the page
  size, a forked template adds map, variant, roster and switch set.
- Nothing here changes a guest call's result or order during a match.

## Not verified

- Which refusal reason a sim prints for its slot reads.
- The split of the 9 to 14 s, the 736 MB of copied reads on a warm Z:,
  and of the 74 MiB of heap.
- How many guest pages a match writes, at either page size.
- That a seeded N: and the carried preferences file leave every tick
  hash equal (the title's acceptance of the slot is read from the code).
- That a store built from a cold recording is the whole map.
- That menu-time state never reaches a match's arena.
- Whether macOS can fork this process, and every saving in the table.

## Related

- [sim-10x.md](sim-10x.md): per-tick speed; [notes/sim-10x.md](notes/sim-10x.md): the batch measurements.
- [host-halving.md](../live/host-halving.md): the same memory levers for a paced host, and the template process.
- [cache-slots.md](../runtime/cache-slots.md): the copy machine, the warm Z: and the format of N:.
- [sim.md](../runtime/sim.md): the report fields, stores and snapshots.
