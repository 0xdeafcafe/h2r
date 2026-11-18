# Machine snapshots (design)

Status: phases 1 to 5 built; the first check on a running game is the
user's ([steps](#the-first-real-check)). Phase 1
([below](#phase-1-what-was-built)): the file format, the guest RAM page
codec, the store, the guest layout and RAM restore, and the time-stamp
counter, in `crates/snapshot` and `crates/runtime`, with the input tap's
resume in `crates/xapi`. Phase 2 ([below](#phase-2-the-quiescence-probe)):
`Machine::quiescent`, per-feature snapshot policies and the opt-in
`H2_SNAPSHOT_PROBE`. Phase 3 ([below](#phase-3-what-was-built)): the
kernel's `Core`, the machine's threads and their continuation pools, the
VFS's read-only open files, the raw disk, XAPI's pads and the input tap's
cursor, feature sections (`h1-health`, skulls, lockstep), and a policy
for every feature ([third round](#third-round-every-feature)). Phase 4
([below](#phase-4-what-was-built)): the `d3d`, `audio`, `dsound`,
`audioout` and `wma` sections. Phase 5 ([below](#phase-5-what-was-built)):
`state.save`, `load`, `list`, `drop` and `fork` through the agent API,
run by the boot loop between slices. None of it has run against the game
yet. Whatever is not marked built on this page is design.

This is phase 5 of the [agent API](agent-api.md): `state.save {name}` and
`state.load {name}`. A user plays to a heavy fight and saves; agents load
that exact moment as often as they like, replay recorded input from it, and
profile the frame rate. The page lists every piece of state a running game
holds outside guest RAM, decides for each whether a snapshot saves it,
rebuilds it from guest RAM or refuses while it is live, says when a snapshot
can be taken at all, and plans the work in phases that are each testable
without running the game.

## Evidence labels

- **Code**: read from this repository at `c84bdd1d`. File given.
- **Doc**: stated by another page here; follow the link.
- **S**: speculation or a design choice. Check it before building on it.

Nothing on this page was established by running the game. No game data is
on this page; guest addresses appear only as named research points.

## The short version

- **Guest threads are not host threads.** One host thread drives every
  guest thread as an explicit context: a `CpuAbi` plus the transfer to
  resume with. Blocking HLE calls save that transfer, never a host stack
  (**Code**: `kernel/src/threads.rs` module doc; `runtime/src/preempt.rs`).
  Between two `Machine::run` slices no guest code is on the host stack.
  That is what makes whole-machine snapshots possible at all.
- **The hard part is Rust state held across guest code.** Three kinds:
  callback continuations (Rust closures waiting for a guest call to return,
  keyed by a token that sits on the guest stack), overrides that keep Rust
  state across yields (the D3D frame wait), and device threads that run on
  wall time (audio output, the D3D render thread, Bink). None of these can
  be serialised. A snapshot is only taken when none is live, and phase 2 has
  to find out whether that ever happens at a tick boundary in a real fight
  (**S**: not measured; this is the biggest unknown).
- **Everything else is plain data or a cache.** The kernel's `Core` is
  maps of records; guest RAM and the layout are bytes and lists; D3D and
  audio caches are keyed by guest address and can be dropped. The work
  there is volume, not difficulty.
- **A snapshot loads only into the same build and switches**, into a
  machine that is already booted. Boot-time state (dispatch table,
  overrides, reservations, the feature plan) is rebuilt by booting, then
  checked against the snapshot, never restored.

## When a snapshot can be taken

### How guest threads run (**Code**)

- `kernel::Machine` (`kernel/src/threads.rs`) owns every guest thread as a
  `Thread { cpu: CpuAbi, ctx: ExecCtx }` and a kernel `Record` (state,
  `resume: TransferAbi`, `wake_value`, priority, TIB words, IRQL). Guest
  stacks are guest memory (`guarded_thread`). `Machine::run(n)` runs `n`
  scheduler iterations: drain deferred signals, wake, pick a thread or a
  DPC, run it until it blocks, yields or is preempted.
- A thread gives up the host only at a runner safe point: a kernel call
  that blocks or yields, or preemption, where the runner stashes the
  transfer it was about to dispatch (`ExecCtx::preempted`) and returns
  `STOP` (`runtime/src/preempt.rs` module doc). Either way the host stack is
  unwound back to `Machine::run`.
- Preemption is counted in guest memory accesses, not host time, and the
  kernel clock is logical (`kernel/src/clock.rs`: "No host wall clock"), so
  the schedule is reproducible. The `Pacer` only makes the host wait.
- The boot's main loop (`boot/src/lib.rs`, the loop around `m.run(1)`) runs
  one slice at a time and, between slices, services input
  (`xapi::service_input`), the lockstep harness and agent `after_slice`,
  and advances audio (`dsound_glue::advance`).
- Direct calls between generated handlers nest on the host stack, but a
  handler that hands a transfer up returns all the way out; the runner
  replays those frames into its shadow stack (`runtime/src/direct.rs`).

### What can still be live between slices (**Code**)

| State | Where | Why it blocks a snapshot |
|---|---|---|
| Callback continuations | `ExecCtx::continuations` (`runtime/src/continuation.rs`), made by `ExecCtx::call_guest` | A boxed Rust closure, resumed when the guest returns to a token address (`0xb0000000..0xf0000000`) that is on the guest stack. A thread blocked inside a guest callback an override made holds one. |
| A delivered interrupt | `ExecCtx::interrupt_token`, the interrupt queue (`runtime/src/interrupt.rs`) | The interrupted CPU state is a host-side snapshot until the callback returns. A queued interrupt holds a host `GuestInterrupt`. |
| A DPC in progress | `Machine::dpc`, `Core::dpc_complete` | Same as a continuation. |
| The D3D frame wait | `d3d/src/vblank.rs`, `Wait { saved: CpuAbi, ret_pc, … }` | Rust state kept across yields by an override. |
| Vblank delivery | `d3d/src/vblank.rs` `deliver_then_return` | A continuation. |
| A WMA begin-packet callback | `dsound/src/xwma.rs` | A guest callback with Rust state around it. |
| dsound event ops not yet delivered | `dsound` `Hle::outbox` | Queued Rust work, delivered before every call returns. (Queued stream callbacks are plain data and saved.) |
| Deferred signals | `Kernel::deferred` | Drained at the start of every slice; empty right after one only if no host thread queued since. |

### The rule (**S**)

A snapshot is taken **between two slices, at the first slice boundary
after a game tick, where a quiescence check passes**:

1. Every thread's `ExecCtx` has no live continuation, no interrupt in
   service, no pending fault. No DPC is running, the interrupt queue and the
   kernel's deferred-signal queue are empty.
2. Every installed feature and device reports itself quiescent (a new
   `Snapshot` capability, below): D3D not inside the frame wait or a vblank
   delivery, push buffer synced, no Begin/End open, no visibility query
   outstanding; dsound with an empty outbox and callback queue; no Bink
   movie open; no WMA callback in flight; Live signed out with no socket.
3. Host device threads are held: the D3D render thread drained
   (`ThreadedBackend::finish`), the dsound pump and output gated.

If the check fails, the request waits for the next tick and tries again,
for at most a bounded number of ticks, then answers `BUSY` naming every
blocker. That answer is the measurement phase 2 needs.

How the tick boundary becomes a slice boundary: the agent's tick observer
(`boot/src/agent_glue.rs`, which observes `GAME_TICK` last) calls
`Runtime::request_preempt` when a save is pending. The game thread then
stops at its next runner safe point, a few transfers later, at a point
fixed by the guest's own execution, so it is the same point on every
replay. The save runs in the main loop between slices, not inside the tick
observer, where the game thread's host stack still holds guest frames.
That also means a save requested while the agent has the game paused
inside the tick observer first steps to the slice end.

## Inventory

**Decision** is one of: **Save** (serialise into a section), **Rebuild**
(drop or recompute from guest RAM and the boot), **Check** (rebuilt by
booting the same image and switches; the snapshot records it and a load
refuses a mismatch), **Quiesce** (must be empty or idle; snapshot refused
otherwise), **Ignore** (diagnostics). Sources are **Code** unless marked.

### runtime

| State | Type, file | Decision | Why |
|---|---|---|---|
| Physical RAM, 64 MB | `Guest::ram`, `guest.rs` | Save | The machine. Zero pages and unchanged pages are cheap (format below). Built. |
| Mappings and permissions | `Guest::mappings` | Save | The guest maps and unmaps at run time. Built. |
| Physical high-water mark, id counter | `Guest::next_physical`, `next_id` | Save | Later allocations must land where the original's would. Built. |
| Reservations, HLE claims, code guards | `Guest::reservations`, `claims`, `code` | Check | Made by the boot and HLE crates. A reservation made later (an HLE arena on first use) makes snapshots before and after it incompatible; the load says so. Built. |
| Mapping hints, fast-page table | `Guest::mapping_hints`, `fast_pages` | Rebuild | Caches over the mappings. Built. |
| Write watch | `Guest::watch`, `watch.rs` | Rebuild | Subscriptions are host consumers'. A restore marks every watched page dirty so they re-read. Built. |
| Time-stamp counter | `Tsc::last`, `tsc.rs` | Save | `rdtsc` never repeats; it continues from the saved value. Built. |
| Guest clock hook | `Tsc::clock` | Rebuild | Installed by the boot. |
| Dispatch table and generation | `Runtime::dispatch`, `dispatch_gen` | Check | Overrides and modules installed by the boot; determined by build and switches. Edits at run time (switches changed live) are a later problem. |
| Interrupt queue | `InterruptQueue` | Quiesce | Host `GuestInterrupt`s. |
| Preempt words | `Runtime::words` | Rebuild | Per-context wake hints, authoritative state is elsewhere. |
| Per-context: CPU | `Thread::cpu: CpuAbi`, `cpu.rs` | Save | Canonical, host-independent by design ("versioned C types transport canonical state"). |
| Per-context: stashed transfer | `ExecCtx::preempted` | Save | Plain `TransferAbi`. |
| Per-context: continuations | `ExecCtx::continuations` | Quiesce | Rust closures. Rebuildable ones are saved: the whole pool's slot generations and reuse order too, since the tokens are on the guest stack. Built. |
| Per-context: slice | `ExecCtx::slice` | Save (S) | Budget counters. Likely re-armed per slice, so empty at a boundary; check. |
| Per-context: direct-call block, resolve cache, fast memory view | `direct.rs`, `ExecCtx::resolved`, `fast_memory` | Rebuild | Caches keyed by dispatch generation. A load makes fresh contexts. |
| Per-context: shadow call stack, events | `frames.rs`, `events.rs` | Ignore | Diagnostics; "never affects control flow". |

### kernel

| State | Type, file | Decision | Why |
|---|---|---|---|
| Thread records | `Core::threads: BTreeMap<u32, Record>`, `threads.rs` | Save | State, wait, suspend count, initial CPU, resume transfer, wake value, TIB, IRQL, priorities. Plain data. |
| Ready queue, wait order, yielder | `Core::ready`, `wait_order`, `yielder`, `yielded` | Save | Scheduler order decides determinism. |
| Machine's thread table, schedule, DPC stack | `Machine::threads`, `schedule`, `dpc_stack_top`, `drained`, `preemptions` | Save | Plus the CPUs above. `pacer` is Rebuild (wall clock). |
| Handles and objects | `Core::handles`, `objects`, `owned_objects`, `references`, `guest_references`, `mutant_owners`, `critical`, `links`, `sections` | Save | Plain maps. Handle values are monotonic (`handles.rs`) and must continue. |
| Logical clock | `Core::clock: Clock`, `clock.rs` | Save | Already restorable: `Clock::resume(epoch, advances, charging)`. `work_carry` too. |
| Timers and DPC queue | `Core::timers`, `dpcs` | Save | Records keyed by guest address. |
| Periodic sources | `Core::periodic: Periodic`, `periodic.rs` | Save + Check | Schedule fields (`origin`, `next`, `owed`, `missed`, `queued`) are data; `handler` is a Rust closure registered at boot, matched by id and name. |
| Files | `Core::files`, `file_handles`, `files.rs` | Save | Each kernel file handle maps to a `vfs::Handle`; re-opened through VFS on load (below). Built. |
| Raw disk device | `rawdisk::RawDisk`, a VFS backend holding up to 2 MiB of written sectors (the cache database) in host memory | Save | Shared with `Core::raw_disk` (`RawSectors`) since the VFS owns the backend. Built. |
| Kernel memory allocator | `Core::memory: memory::Memory`, `memory.rs` | Save | Allocator bookkeeping over guest pages. |
| Config, console, launch page, TLS template, KPCR, bridge | `Core::console`, `launch_page`, `tls`, `kpcr`, `bridge`, `data` | Check | Set by the boot. |
| Deferred signals | `Kernel::deferred` | Quiesce | Drained every slice. |
| Debug log, stops, failures | `Core::log`, `stops`, `deferred_failures` | Ignore | Diagnostics. |

### vfs and storage

| State | Where | Decision | Why |
|---|---|---|---|
| Open files: path, mount, options, position, enumeration cursor, delete flag | `Vfs::objects: OpenFile`, `vfs/src/lib.rs` | Save | Re-open by device and path on load, keeping `vfs::Handle` and kernel handle numbers. Read-only objects only: a save refuses while one is open for writing or deletion. Built. |
| Delete-pending sets, next handle | `Vfs::pending`, `cleanup_pending`, `next_handle` | Save | `next_handle` built; a save refuses while a delete is pending. |
| Host file descriptors, write-behind queue, reservations | `HostBackend`, `WriteBehind`, `vfs/src/host.rs`, `write_behind.rs` | Rebuild | Flush (`vfs::flush_pending_metadata`) before saving. |
| Files on the host disk (saves, cache partitions) | the data root | Not in the snapshot (S) | A load after the game wrote a save or a cache file sees the newer file. Phase 3 either records the writable files' contents or refuses while one is open for writing. |
| Map overrides | `MapOverrideBackend` | Rebuild | Opened at boot. |

### xapi and input

| State | Where | Decision | Why |
|---|---|---|---|
| Open pad handles, `next_handle`, last state | `Xapi::handles`, `xapi/src/lib.rs` | Save | The guest holds the tokens; replay counts handle mismatches (`tap.rs`), so numbering must continue. Rebound to the device now on the port. Built. |
| Device change words | `Xapi::changes`, voice types' | Save | 12 bytes each; mirrored in guest RAM too. Built. |
| Rumble, completions, signals, launch request | `Xapi::pending_feedback`, `completions`, `signals`, `launch_request` | Quiesce | Built (`Xapi::snapshot_blockers`). |
| Save-game searches | `Xapi::searches` | Quiesce | Refused while open. Built. |
| Input tap cursor | `InputTap::cursor`/`seek`, `xapi/src/tap.rs` | Save | In the `xapi` section; a load seeks the installed recorder or replayer. Built. |
| Live pads, keyboard and mouse, touch | `input::InputManager`, `backend/*`, touch statics | Rebuild | Host devices. Held keys are not carried: the next sync publishes the host's state. |
| Per-port published state and polling | `InputManager` ports | Save | Packet numbers continue from the saved run's (`port_snapshot`, `restore_port`). Built. |
| Scripted pad cursor | `ScriptedInput` `Playback` | Save | Refuses to rewind today (`ScriptError::Rewind`). |

### d3d and gfx

The device is one `Device<B>` behind a mutex (`d3d/src/device.rs`); its
module doc already separates what the guest owns from what the device owns.

| State | Where | Decision | Why |
|---|---|---|---|
| Bindings: render target, depth, textures, palettes, shaders, streams, viewport, transforms, scissors | `Device` fields | Save | The device is authoritative for them. Built. |
| Vertex program and constants, NV2A register state, pushed render state, immediate-mode registers | `program`, `constants`, `hw: HwState`, `pushed`, `immediate` | Save | What the GPU model last saw can differ from guest arrays. Built. |
| Push-buffer cursors, fence, swap count, register-window shadow | `consumed`, `fence`, `fence_at`, `swap_count`, `regs` | Save | After `sync_push_buffer`. Built. |
| Begin/End open, push-buffer callback, latched fault, depth-grid request, visibility counting, `PersistDisplay` surface | `immediate`, `active_callback`, `undelivered`, `fault`, `pending_grid`, `hw.zpass_count_enable`, `display.persisted` | Quiesce | `Device::snapshot_blockers`. Built. |
| Display: gamma, flicker, vblank counters, vblanks not delivered | `Display`, `device/display.rs` | Save | `per_swap` is a host setting, kept. Built. |
| Visibility tests | `visibility: HashMap<u32, VisibilityTest>` | Save | The save reads every ticket's count first (a wait), so the saving run and a load answer the same from then on. Polling mode still makes later counts depend on GPU timing (`threaded.rs`). Built. |
| Frame-buffer ring order and flags | `swap: Swap` | Save | By each buffer's `Data`; the buffers themselves are Check. Built. |
| Destroys the guest has yet to run | `pending_destroys`, `torn_down` | Save | Built. |
| Frame and depth surface pixels | backend surfaces | Rebuild | Not saved: a load keeps the loading device's pixels, so the first frame's history (Halo samples last frame's buffers by parity) is the loading run's. Draw streams do not depend on it. |
| Surfaces of guest surface headers that are not texture levels | `other_surfaces` | Rebuild | Made again blank on next use. Pixels lost. |
| Texture, vertex and index caches, memos, depth copies and aliases | `cache`, `vertex_cache`, `index_cache`, `translated`, `draw_*`, `microcode_*`, `depth_*` | Rebuild | Keyed by guest address; `Device::drop_host_caches`. Built. |
| Render-to-texture targets, alias mirror | `rtt`, `mirror`, `raster.rs` | Rebuild | The save writes dirty levels and the mirror back to guest RAM first; a load drops them. Built. |
| Write tracker | `tracker` | Keep | Every copy held over it is dropped with the caches. |
| Backend GPU objects, pipelines, upload rings, presenter | `GfxBackend`, `ScaledBackend`, `Nv2aShaders`, `gfx::*` | Rebuild | Kept as they are; nothing in them is guest state. |
| Render thread | `d3d-render`, `threaded.rs` | Quiesce | The caller drains it with `ThreadedBackend::finish()` before saving. |

The teardown (`Device::release_device`) writes refcounts into guest
memory, so a load uses the host-only `Device::drop_host_caches` instead.

### audio (dsound, audio, wma)

Built in phase 4 (below); decisions per field are in the `snap` modules.

| State | Where | Decision | Why |
|---|---|---|---|
| Guest objects: buffers and streams (format, data pointer, volume, 3D, effects, mix bins, gates, packet records, mixer ids), root, headroom, effects image, listener, voice rings, token counter | `DirectSound`, `dsound/src/state.rs` | Save | Keyed by guest pointer. Built. |
| Layout (stream vtable, effect count) | `DirectSound::layout` | Check | Fixed by the build. Built. |
| Mixer consumer, lease counter, `frames_per_do_work`, pad router | `DirectSound` | Keep | The loading process's wiring. |
| Mixer: every buffer (with its copy of the guest bytes), stream (queued packets as decoded frames, history), mix-in, effect state, master volume, listener, ids, render clock, tapped samples | `audio::Mixer`, `fx.rs` | Save | Bit-exact PCM after a load. Built. |
| Reverb | `Reverb` | Save | Delay lines and every derived gain, saved exactly. Built. |
| Mixer rate | `Mixer::sample_rate` | Check | Built. |
| Decoded buffer copies, filter coefficient caches, scratch | `SoundBuffer::decoded`, `FxState::coeffs` | Rebuild | Recomputed from the same inputs, bit-equal. |
| Arena bookkeeping | `Hle::arena` | Save | Bytes in guest RAM; the allocator lists are host-side. Built. |
| Event pins | `Hle::pins` | Save | Token and object address; a load adopts them from the loaded kernel's count (`Kernel::adopt_reference`). Built. |
| Stream callbacks queued by a host render | `Hle::callbacks` | Save | Plain data. Built. |
| Event ops published, not delivered | `Hle::outbox` | Quiesce | Built (`Hle::snapshot_blockers`). |
| WMA codec instances | `WmaHle::streams`, `wma` `Session`, `Decoder` | Save | Whole session and decoder state. Built. |
| Output grid (logical pacing) | `output.rs` `Grid` | Save | Origin and frames rendered. Built. |
| Output device, PCM ring, resampler, rendered queue | `DeviceSink`, `sink.rs`, `Shared::queue` | Rebuild | Output only. |
| Pump and device-paced output | `dsound-pump`, output thread | Quiesce | Wall time: `Output::snapshot_blockers`. |

### Features, boot glue and the rest

| State | Where | Decision | Why |
|---|---|---|---|
| Switches | `feature::Switches` | Check | Part of the build key. |
| Feature plan, hooks, arena | `feature::Registry`, `Plan`, `Installed` | Check | Deterministic from switches; arena bytes are guest RAM. |
| Boot glue statics holding guest addresses | `layout_glue`, `campaign_menu`, `list_class`, `stopgaps::KSTACK_NEXT` | Check | Fixed by boot order. |
| Boot glue statics set during play | `health::MAP_MODE`; `skulls_custom::CAMPAIGN`; `level_boot::LOADED_AT`; `skulls_mp`/`skulls_custom` shared state | Save | Each feature owns a section (`SnapshotPolicy::Section`). Built for `h1-health` (re-derived from the loaded map), `skulls-custom`, `skulls-mp`, and the [third round](#third-round-every-feature)'s. |
| Co-op | `coop_live::LOBBY`, `COOP_MODE_PENDING` | Quiesce | Refuse during co-op. Built: `coop-screens`, `coop-live`, `coop-players`, `coop-jip` forbid while armed or in a session. |
| Live | `live-hle` sockets, client thread | Quiesce | Refuse while signed in. |
| Bink | `BinkHle` slots, platform decoder | Quiesce | Refuse while a movie plays. |
| Interpreter fallback caches | `boot/src/fallback.rs` | Rebuild | |
| Lockstep recorder counters | `lockstep::Recorder::ticks`, `presents` | Save | So a trace can continue across a load. Built: the `lockstep` feature's section. |
| Agent session, timeline, perf, telemetry | | Ignore | Observers. The agent's view of the game (cutscene, map, map ticks) is its `agent` section. |

### What "the guest can rebuild" means here

Nothing is rebuilt by asking the guest to redo work: the game never sees a
load. "Rebuild" always means a host cache that refills from guest RAM on
use, or boot state recreated by booting.

## How a load works (S)

In the process that is already running the same image, at a slice
boundary that passes the quiescence check:

1. Open the file, check magic, format version, the file CRC and the build
   key (image build stamp plus switches).
2. Quiesce devices as for a save, then drop host caches: D3D caches and
   tracker, audio output, input devices' held state.
3. Guest: check reservations, claims and code guards match, then replace
   mappings and RAM in place (`Guest::restore_with`, which decodes straight
   into the guest's store). Built.
4. Runtime: TSC (built). Kernel: `Core` whole, then the machine's threads
   with fresh `ExecCtx`s carrying the saved CPU and continuation pool
   (built).
5. VFS: close every open object, re-open the saved ones by path, seek
   (built, read-only objects).
6. Devices: D3D bindings and GPU-model state, surfaces; dsound objects,
   mixer, WMA sessions, re-pinned events.
7. XAPI handles, input tap cursor (resume replay, or continue recording),
   lockstep counters, feature sections (built; the boot does not call them
   yet).
8. **Determinism check**: `lockstep::hash_guest` over the loaded machine
   must equal the hash stored at save time (per-MiB chunks, so a mismatch
   names the MiB), and `hash_sim` likewise. Then, optionally, run N ticks
   with the input replay resumed and compare against the trace the saving
   run recorded from the same tick (`lockstep::compare` on records from
   that sequence number on). Both runs need the same observers installed,
   since the tick observer changes dispatch (**Doc**:
   [agent-api.md](agent-api.md) "Cost").

A fresh-process load (boot, then load before the first slice) is the same
sequence after the boot reaches its first slice. It costs a boot, but is
the robust fallback if in-process cache invalidation turns out leaky.

## File format

Built in phase 1 (`crates/snapshot/src/container.rs`, `pages.rs`).

```text
magic        8 bytes  "H2SNAP\r\n"
format       u32      1
header       u32 len + bytes: build stamp, switches, name, tick,
                      presents, notes (key/value)
header crc   u32
sections     u32 count, then each:
  tag        8 bytes ASCII ("ram", "guestmap", "tsc", "kcore", ...)
  version    u32      the section's own layout version
  codec      u8       0 stored, 1 zstd
  raw len    u64
  stored     u64 len + bytes
  crc        u32      CRC-32 of the decoded bytes
file crc     u32      CRC-32 of everything before it
```

- Little-endian throughout. The header can be read without the build key,
  so `state.list` shows snapshots of other builds and says why they don't
  load.
- **Build key**: `boot::build_stamp` (XBE FNV-64, generator, analysis and
  ABI versions, codegen flags; **Code**: `boot/src/lib.rs` `stamp_for`)
  plus the switch string. A different image, a regenerated module or a
  different switch set refuses to load.
- **RAM section** (`ram`): per page a kind byte (zero, data, same as a
  base snapshot); a CRC-32 per non-zero page; data pages zstd level 1 in
  independent 1 MB frames. Zero pages cost one byte. A **delta** against a
  base snapshot names the base and stores only changed pages; each base
  page's CRC is checked against the base supplied at load, so a wrong base
  fails loudly. The encoder takes an optional dirty-page hint (from
  `runtime::watch`) to skip comparing clean pages; a wrong hint is caught
  by the same CRC at load.
- Sections are versioned on their own, so a subsystem can change its
  layout without touching the container. Since a snapshot only loads into
  the build that wrote it, version skew happens only for tools reading old
  files.

## Where snapshots live

Built (`crates/snapshot/src/store.rs`): `$H2_SNAPSHOTS`, else
`~/Library/Caches/h2recomp/snapshots` on Apple hosts,
`%LOCALAPPDATA%\h2recomp\cache\snapshots` on Windows,
`$XDG_CACHE_HOME/h2recomp/snapshots` (or `~/.cache/...`) elsewhere: the
same cache directory as the pipeline cache and the agent socket. Never the
repository: snapshots hold game data. Files are
`<root>/<build id>/<name>.h2snap`, the build id being an FNV-64 of the
build key, so a rebuilt image starts with an empty list and old builds'
directories can be removed whole. Names are `a-z 0-9 . _ -`, at most 64;
writes are atomic (temporary file, then rename).

## Phases

Each phase is proven by tests over synthetic state, as the agent API's are.
None needs a game run until the last step of phase 5.

1. **Container, RAM, store, runtime basics.** Built (below).
2. **Quiescence probe.** Built (below). A `Snapshot` capability for features
   (`feature::Feature` gains "stateless", "has a section", or "forbids
   snapshots while live", defaulting to forbid, so an unaudited feature
   fails closed). `Machine::quiescent()` reports every blocker in the table
   above. An opt-in, read-only probe (`H2_SNAPSHOT_PROBE`) logs, per tick,
   which blockers were live. Tests: synthetic machines with a thread
   blocked in a guest callback, a queued interrupt, a DPC. The user's one
   run with the probe on in a fight answers the biggest unknown.
3. **Kernel and machine.** Built (below): sections for `Core` and the
   machine's threads; VFS re-open; the raw disk; XAPI and the tap cursor;
   feature sections. Tests: the
   kernel crate's existing synthetic threads (`kernel/tests`): run a
   synthetic multi-threaded guest for N slices, snapshot, run M more and
   hash; load into a freshly booted machine, run M and compare hashes
   tick by tick.
4. **Devices.** Built (below). D3D save, drop caches, restore;
   dsound, mixer, WMA. Tests
   over the D3D null backend and the audio crate's synthetic voices:
   render, snapshot, render more; load, render the same, compare draw
   streams and mixed PCM.
5. **Wiring.** Built (below). `state.save`, `state.load`, `state.list`,
   `state.drop`, `state.fork` in `agent` and `agent-proto`, the save
   request in the tick observer and the save itself between slices in the
   boot loop. Tests: the agent crate's executor with a fake machine. Last:
   the user saves a fight in `h2 serve`, an agent loads it repeatedly and
   checks the lockstep hash and N replayed ticks.

## Risks and unknowns

- **Quiescence may never hold at a tick boundary** (S). If the render
  thread or a sound thread always sits inside an override's continuation
  when the main thread ticks, snapshots need those continuations made
  serialisable: replace the closure with a named, registered continuation
  kind plus plain data. That is a runtime contract change. Phase 2
  measures it before anything else is built.
- **Which Halo 2 threads exist during a fight, and where they wait**, is
  not established on this page (S).
- **Host files**: the snapshot does not cover the data root. Loading after
  the game wrote a save or a cache partition file can diverge (S).
- **Run-time reservations**: an HLE crate that reserves physical memory
  on first use (a movie arena, say) makes earlier and later snapshots
  incompatible with a machine in the other state. The load refuses with
  the difference; whether that bites in practice is unknown.
- **Frame pixels**: whether one frame of blank history surfaces matters to
  a profile (S). Probably not; it matters to `render.compare`.
- **Wall-clock devices**: audio device pacing and GPU visibility polling
  make runs nondeterministic whether or not snapshots exist; a snapshot
  replay inherits that. Logical audio pacing and synchronous visibility
  queries are prerequisites for an exact replay.
- **Size and speed**: 64 MB of RAM, mostly in use during a fight, at zstd
  level 1 is estimated at tens of MB and well under a second (S: not
  measured on game RAM). Frames are independent, so compression can be
  spread over threads without a format change.
- **Live switch changes** (agent API overrides) change the dispatch table
  at run time; the build key then has to carry the live switch state, not
  the launch one.

## Phase 1: what was built

- `crates/snapshot` (no game data, no kernel dependency):
  - `container`: the format above; `SnapshotWriter`, `Snapshot::open`
    (build key check, file CRC), `Snapshot::section`/`require` (per-section
    version and CRC), `read_header` for listing.
  - `pages`: the RAM codec with zero pages, deltas against a base, dirty
    hints and per-page CRCs.
  - `guest`: the `guestmap`, `ram` and `tsc` sections; `save_guest`,
    `load_guest`, `save_tsc`, `load_tsc`.
  - `store`: the cache-directory store; save, load, list, remove.
  - `tests/machine.rs`: synthetic machines saved through the store and
    loaded into a second machine booted the same way; `lockstep::hash_guest`
    equal after load and after further identical play; a delta over its
    base; a differently booted machine refused and left untouched.
- `runtime`: `Guest::layout`, `Guest::restore`, `Guest::restore_with`
  (`guest/state.rs`), `Permissions::bits`/`from_bits`,
  `Runtime::tsc_last`/`restore_tsc_last`.
- `xapi::tap`: `Replayer::resume(log, next)` and `Recorder::resume(log)`,
  so replay or recording carries on from a snapshot's input cursor.

## Phase 2: the quiescence probe

What was built (**Code**, tests over synthetic machines):

- `runtime`: every continuation records an origin and the guest address
  it called (`ExecCtx::call_guest_named`; plain `call_guest` records the
  closure's type name). `ExecCtx::live_continuations`,
  `interrupt_in_service`, `fault_pending` report them.
- `kernel::quiesce`: `Machine::quiescent()` returns every blocker:
  continuations per thread (and on the DPC context), an interrupt
  callback in service, a pending fault, queued interrupts, deferred
  signals, a periodic handler that is running, a stopped machine. The
  continuation every created thread holds for its whole life
  (`THREAD_RETURN`, which only terminates the thread) is listed as
  rebuildable, not as a blocker: a load can recreate it at the same token
  (phase 3 must add that to `Continuations`). Tests in
  `kernel/src/quiesce.rs` and `kernel/tests/quiesce.rs`, the latter over
  guests that really run: threads waiting in the kernel are quiescent; a
  thread blocked inside an override's guest callback is a blocker until
  the callback returns.
- `feature`: `Builder::snapshot(SnapshotPolicy)`: `Stateless`,
  `ForbidWhileLive(fn)` (the function says, between slices, why it is
  live), or, for a feature that says nothing, `Unaudited`, which blocks
  every snapshot. `Plan::snapshot_blockers()` lists them;
  `Plan::describe_target` names the hooked point behind an after-hook
  continuation (`feature: after hooks`, called through a chain's shadow
  entry).
- `kernel::quiesce::Tally`: counts checks and blockers and makes the
  rate-limited lines.
- `boot/src/snapshot_probe.rs`: the probe.

### Running the probe

```sh
H2_SNAPSHOT_PROBE=1 ...       # check at the end of the slice each game tick ends in
H2_SNAPSHOT_PROBE=slice ...   # don't end slices early; check at the next boundary
```

`H2_SNAPSHOT_PROBE=1` installs a `snapshot_probe` feature whose tick
observer counts ticks and calls `Runtime::request_preempt`, so the slice
ends a few transfers after the tick, where `state.save` would act. That
changes the schedule: don't compare a probed run with an unprobed one
under lockstep. Every line starts with `snapshot-probe:`:

```text
snapshot-probe: on, checking at the end of the slice each game tick
snapshot-probe: tick=812 machine=blocked [thread 1: continuation feature: after hooks -> 0x... (after hooks on <point> (0x...) [feature:label])] host_blockers=31
snapshot-probe: tick=813 machine=quiescent host_blockers=31
snapshot-probe: progress tick=1800 checked=1800 machine_quiescent=... (..%) fully_quiescent=0 (0.0%)
snapshot-probe: summary checked=... machine_quiescent=... (..%) fully_quiescent=... (..%)
snapshot-probe: top machine <count> (<%>) <blocker>
snapshot-probe: top host <count> (<%>) feature <id>: not audited for snapshots
```

- A `tick=` line is printed only when the set of machine blockers
  changes, at most 200 of them; a `progress` line every 1800 checks; the
  `summary` and up to ten `top machine` and ten `top host` lines at exit.
- **Machine** blockers are the hard question this phase exists to answer.
  **Host** blockers are features' live predicates and, since phase 5,
  the host subsystems' own (`snapshot_glue::Host::blockers`: d3d, dsound
  and its output, XAPI, a Bink movie open, Xbox Live signed in, co-op),
  each as `host <owner>`.

### Rebuildable continuations

A continuation made with `ExecCtx::call_guest_rebuildable(.., origin,
state: [u32; 4], ..)` promises that its closure depends only on `origin`,
`target`, `state` and the context it runs on. Quiescence lists it as
rebuildable, with its state, instead of as a blocker. Phase 3 makes it
again at load: a registry from `origin` to a factory, and a
`Continuations` insert at a given token (the token is on the guest stack,
so it must be the same). Two exist (**Code**):

| Origin | Made by | State | Rebuilt as |
|---|---|---|---|
| `kernel: thread return` | `Machine::collect_threads`, every created thread's start routine | none | terminate the thread with success |
| `trace: kernel call return` | `boot::trace::install_bounded`, every kernel call, through the shadow `0xa0ff0000 + 4 * ordinal` | caller's return address, trace sequence number | return to the caller; the trace record is gone, and `finish` of an unknown one does nothing |

### First run: Old Mombasa, 2026-11-17

`H2_SNAPSHOT_PROBE=1`, headless, 5256 ticks checked (run by the
coordinator; the log is outside the repository). `machine_quiescent` was 0.
There was one machine blocker, on every tick: thread 3's
`boot::trace::install_bounded` continuation calling `0xa0ff03a8`, the trace
shadow of ordinal 234, `NtWaitForSingleObjectEx`. That is a worker thread
waiting in the kernel for its whole life, under the kernel-call trace's
wrapper, which is always installed. Nothing else: no after-hook or async
entry continuation was live at any tick boundary, no interrupt in service,
no queued interrupt. The wrapper is now rebuildable (above), so the next
run should show the machine quiescent at tick boundaries (**S** until it
is run).

Host blockers were the eleven features on in that run, all unaudited, and
the host subsystems. The features are now audited:

| Feature | Policy | Why |
|---|---|---|
| `engine-leaves` | stateless | pure functions of guest memory; string sources fixed at boot |
| `render-loop` | stateless | a port's state is in its continuation, which the machine check reports |
| `controls-text` | stateless | an async port: its future is parked in a continuation |
| `ui-strings` | stateless | strings written and addresses fixed at install |
| `level-boot` | stateless | `LOADED_AT` is a host mark for perf sessions; the game never reads it |
| `map-slots` | stateless | a wall-clock throttle; which package served a map is VFS state |
| `snapshot_probe` | stateless | an observer |
| `layouts-value` | forbid while live | between the Controls row text's before and after hooks |
| `campaigns` | forbid while live | while the level select is built, or a request is to be let through; the campaign shown is menu state, and in a game the level's campaign comes from the map |
| `campaigns-screen` (and every list class) | forbid while live | while our loader runs the Controls constructor; layouts and loaders are fixed at build, `Shown` is set up again before a screen draws |
| `h1-health` | forbid while live | while a damage update or body restore is between its hooks. The model follows the loaded map (`set_mode` at game start, from guest memory) and the meter is published every tick, so a load must call `set_mode` again from the loaded map (phase 3) |
| `touch-glue` | forbid while live | while touch controls are shown: unapplied touch input is host state |

The features that were off in that run were audited in the
[third round](#third-round-every-feature). Still unaudited then: the host
subsystems (d3d, dsound, xapi, bink, live, lockstep), which are phase 4.
Phase 4 found that the D3D frame wait and vblank delivery keep their Rust
state in `ExecCtx::call_guest` continuations, which the machine check does
see; `host d3d` can report `Device::snapshot_blockers` once the probe is
wired to it (phase 5).

## Phase 3: what was built

Tests over synthetic machines (`kernel/tests/snapshot.rs`): three guest
threads sharing an event, sleeping on different periods and reading a
read-only VFS file at its position run N slices and are saved (RAM, TSC,
`kcore`, `machine`); the original runs M more. A machine booted the same
way loads the file and runs M; every slice's digest (mapped RAM, clock,
CPUs, scheduled thread) matches, then whole RAM, schedule, clock, TSC and
VFS state. One created thread returns from its start routine after the
load, through its rebuilt continuation. Also: a snapshot loads twice into
one machine; a machine booted with other data exports refuses it; a live
closure continuation refuses the save.

- `runtime`: `ContinuationLayout` (`ExecCtx::continuation_layout`,
  `restore_continuations`): the pool as plain data, every slot's
  generation and the reuse order, so tokens made after a load are the
  saved run's. A live closure that is not rebuildable refuses the save; a
  load with no rebuilder for an origin changes nothing.
- `kernel::snapshot`: `Machine::save_snapshot` (refuses unless
  `quiescent()`) and `load_snapshot`, sections `kcore` and `machine`, on
  the snapshot crate's codec. A `Persist` trait; `persist_struct!`
  destructures without `..`, and `save_core` destructures `Core` whole,
  so a new field does not compile until it is given a decision. Saved:
  every table in the inventory, the clock with its history and charging
  bit, periodic schedules (put back onto the booted source of the same ID
  and name, which keeps its Rust handler). Checked against the booted
  machine: KPCR, export bridge, data exports, title ID, utility partition.
  Kept from it: TLS template, console config, image sections, storage
  handle, diagnostics. `Rebuilders` maps a continuation origin to a
  factory; the kernel's thread return is built in.
- `vfs::Vfs::state`/`restore_state`: open objects by device and path, with
  handle numbers, options, position and search cursor (entries are read
  again at load). Refuses while an object can write or delete, or a delete
  is pending; a load looks every file up before it closes anything.
- `boot::trace::rebuilder`: the kernel-call trace's return continuation,
  the one blocker the Old Mombasa probe run found, for `Rebuilders`.

Design changes from the plan above: the kernel depends on `crates/snapshot`
for the codec. The machine's own scheduling state (`schedule`, `drained`,
`dpc_stack_top`, `preemptions`) is saved; its config fields
(`preempt_slice`, `max_transfers_per_slice`, `pacer`) stay the booted
machine's. Per-context event logs are not saved (diagnostics; a quiescent
resume transfer carries no event). Writable files are refused rather than
recorded.

Second round (branch `snapshot-phase3b`):

- Raw disk: `RawDisk` keeps its sectors behind `rawdisk::RawSectors`, which
  `Core::raw_disk` shares from `set_shared_vfs`; `kcore` (version 2) saves
  them and a load replaces the booted disk's (`kernel/tests/snapshot.rs`).
- `xapi::snapshot`: the `xapi` section. Pad handles (token, port, last
  state), `next_handle`, the gamepad and communicator change words, each
  port's published state and polling parameters
  (`InputManager::port_snapshot`/`restore_port`). A load rebinds each
  handle with `InputManager::reopen`: on an empty port it reads as
  unplugged and stays stale after an insertion, and the change words
  report the difference from the saved ports at the next
  `XGetDeviceChanges`. Refused while rumble, a launch request or a save
  search is in flight. Tests in `xapi/tests/snapshot.rs`.
- Tap cursor: `InputTap::cursor` and `seek` (recorder: read count,
  truncate; replayer: next read; the agent's tap forwards to the chained
  one). The `xapi` section saves the installed tap's cursor; a load seeks
  the installed tap and returns the cursor for a caller that installs one
  later.
- Feature sections: `SnapshotPolicy::Section` (and
  `section_forbid_while`): a feature's bytes and a load function given the
  runtime. `Plan::save_sections`/`load_sections` collect them; a load
  refuses a file whose section features differ before loading any.
  `snapshot::features` stores them as one `features` section. Built for
  `h1-health` (no bytes; the load calls `set_mode` from the loaded map's
  name), `skulls-custom` (campaign set, said/found bits, ticks, the
  level's skull, gravity, falls; waits while its guest calls are queued),
  `skulls-mp` (active set, said flags, profiles; waits while a Famine call
  is between its hooks) and `lockstep` (tick and present counters).

Left:

- Done in phase 5: the boot saves and loads the kernel's, XAPI's and the
  features' sections.
- Every feature now has a policy ([third round](#third-round-every-feature)).
- A load into a running game drops the old contexts' fast-memory views and
  direct-call blocks with them; untested outside synthetic machines.

### Third round: every feature

Branch `snapshot-features`. Every `feature::Feature` in the tree says
what it is; none is `Unaudited`. A predicate's test sets each live
condition and checks it is reported, and clear again; a section's test
round-trips its bytes and checks that damaged bytes change nothing.

| Feature | Policy | Why |
|---|---|---|
| `skulls` | section, forbid while live | live while a pickup call is between its `SKULL_STOCK` hooks. Saves the tracker's last look at the skull array (one bool each); without it a load reads the loaded array's differences as pickups. Profiles are the settings store's, which a load does not rewind |
| `skulls-page` | section, forbid while live | live while the kept difficulty press runs. Saves the kept press, the page's phase (and closing page), the campaign Skulls screen's profile and whether Halo 1 options show. The page's open time is wall clock and not kept |
| `skulls-mp-row` | section | the profile the multiplayer Skulls screen was opened for; `ROW_ON` is fixed at build |
| `cheats` | forbid while live | while a cheat is on or has something left to undo (flagged units, a speed we set, the HUD flag, a flight) |
| `coop-probe` / `coop-live` | forbid while live | while armed (`Arm::snapshot_live`: the probe forced on, or a co-op session armed the hooks), while a session start's peer count is shown changed, or a hash log is open |
| `coop-players` | forbid while live | while armed, or one of our player spawns is running; gather and cinematic flags are read only while armed |
| `coop-jip` | forbid while live | while armed: a join is an exchange with a peer no snapshot holds |
| `coop-latency` | stateless | an observer: windows and counters feed only the log line |
| `coop-screens` | forbid while live | while hosting or joined, the join screen is due, the hooks are armed, or the Co-op Campaign row's lobby-mode window is open |
| `aim-assist` | section, forbid while live | live while the touch player's slowdown and follow window is open. Saves the retail values held per field; without them a load over scaled tags takes scaled values for retail and compounds them |
| `signin-strings` | forbid while live | while a sign-in keyboard of ours is open; otherwise the page and every id's address are fixed at build |
| `ui-kit-texts.*` (every kit screen) | section, forbid while live | live while a press waits for the action, our keyboard is open, or work waits for a screen's free. No bytes: a load marks every text slot dirty, so the next draw writes them over the saved run's page. Row values are the settings store's |
| `mods-screens` | section, forbid while live | live while a mod download is under way (it writes to disk). Saves the open detail page's mod; a load rewrites every text. The list and installed set are the server's and the disk's |
| `settings-rows`, `controls-rows` | stateless | model strings fixed at build, values the settings store's; async answers are continuations |
| `h1-options-row`, `clankers-row` | forbid while live | while a Game Setup list's constructor is between its hooks (`h1_options_row::building_live`) |
| `variant-more-options-row` | stateless | `ROW_ON` fixed at build; text and press are guest memory and continuations |
| `hud` | section | the style in force, one byte; not re-picked from the map at a load, because the choice counts only from a game start |
| `h1-hud` | section | Halo 2's own motion sensor table, which the guest's copy no longer shows after a Halo 1 map |
| `loaded_map` | section | no bytes: a load publishes the loaded map's name again for the host panel |
| `h1-precision` | stateless | the swap table is read at install; the swap is in guest memory |
| `map-select` | forbid while live | while a Select Map is to be built, one of ours is up, or a map press waits to close the categories |
| `variant-vitality` | forbid while live | while a shield step's damage is scaled and its after hook has not restored it |
| `variant-ext-live` | forbid while live | while this machine is a Live guest playing its host's options (`FROM_HOST`), which a load has no host to resend |
| `ui-watch` | stateless | an observer; what it last read is read again every tick |
| `agent` | section | whether a cutscene runs, the map and its tick count: a loaded cutscene answers `cinematic`, and the first tick after a load is not a map change. Connections, pauses and recordings are the session's |
| `live-telemetry` | stateless | reads the guest, never writes it; a rewound tick is a revert the session already handles |
| `render-timing` | forbid while live | while a render stage is open between its observers |
| `render-census` | forbid while live | once its access trap is armed (from the first render frame); nothing disarms it |

Left from this round (**S**, not built):

- List classes keep row state of their own that `list_class`'s policy
  (constructor running) does not ask about: the More Options page's
  unsaved edit, `SigninRows`' shown flips, the mods list and detail,
  `map_select`'s picked category. A load while one of those screens is up
  could show stale rows. `list_class` should ask its `Rows` for liveness.
- `SkullsScreen` has no close, so a snapshot saved before a Skulls screen
  was opened leaves a running process's profile in place at a load; a
  press opens it again before it shows.
- `live-telemetry` is stateless for the guest, but a load during a Live
  match would upload a rewound match record; forbid while a match is in
  progress if that matters.
- Co-op's background threads can change the role at any moment; the
  boot's `state.save` must check and save without letting them in between.

## Phase 4: what was built

D3D (`d3d/src/device/snap.rs`, tests in `d3d/tests/snapshot.rs`):

- `Device::save_snapshot(mem, out)` adds the `d3d` section. Called
  between slices and before guest RAM is saved, since it writes to guest
  memory: it drains the push buffer, writes dirty render-to-texture levels
  and the alias mirror back, and reads every visibility count. Refused
  (`SnapshotError::Busy`) while `Device::snapshot_blockers` names
  anything.
- `saved()` destructures `Device` and `Display` without `..`, each field
  under its decision, so a new field does not compile until it has one.
- `Device::load_snapshot(snapshot)` decodes and checks first (frame-buffer
  and depth headers, the ring's buffers, size and flip, the register
  window), then drops host caches and restores. Nothing changes on a
  mismatch. Guest RAM must already be loaded.
- When a D3D save is allowed: the frame wait (`vblank.rs`, `Wait`) and
  vblank delivery hold their state only in `call_guest` continuations,
  which `Machine::quiescent` reports. The device itself holds no Rust
  state across yields. So a save is allowed when the machine is quiescent,
  `snapshot_blockers` is empty and a render thread has been drained.
- Tests over `RecordingBackend`: a device draws three frames with a
  transform, a scissor and a visibility test, is saved and draws three
  more. A second device, created the same way but having drawn other
  frames with other state, loads the section over the saved guest memory
  and draws the same three frames. The recorded clears, draws and
  presents match, as do the ring, the visibility answer and guest memory.
  A device created with other present parameters refuses the section
  unchanged; a save inside Begin/End is refused.

Audio (`audio/src/snap.rs`, `dsound/src/state/snap.rs`,
`dsound/src/register/snap.rs`, `dsound/src/output.rs`, `dsound/src/xwma.rs`,
`wma` `Session::save_state`; tests in `audio/tests/snapshot.rs`,
`dsound/tests/snapshot.rs`, `dsound/tests/xwma.rs` and `wma`'s
`boundary.rs`):

- `Mixer::save_snapshot` adds `audio`; `load_snapshot` is
  `check_snapshot` (decode, check the rate, change nothing) then
  `apply_snapshot`. Every struct is destructured without `..`.
- `Hle::save_snapshot` adds `dsound` and `audio`: the `DirectSound`
  objects and settings, the arena's lists, the pins as (token, object
  address) and the queued stream callbacks. Refused while event batches
  are undelivered or a pin cannot name its object (`EventRef::address`).
  `Hle::load_snapshot` decodes and checks both sections and adopts the
  pins (`Events::adopt`) before changing anything; the loading run's own
  pins are dropped with `EventRef::forget`, which takes no count, because
  the loaded kernel's `references` are the saved run's. Boot's kernel
  pins implement all three (`Kernel::adopt_reference`,
  `DispatcherReference::forget`).
- `Output::save_snapshot` adds `audioout` (the logical grid's origin and
  frames rendered); device pacing refuses (`Output::snapshot_blockers`).
- `WmaHle::save_snapshot` adds `wma`: every codec instance by handle with
  its callback, context and whole `Session` (decoder overlap, held
  reservoir packet, exponents, block sizes, partial input, queued PCM),
  and the next handle. Exact state turned out small, so no
  refuse-while-decoding policy is needed: a begin packet pulling input
  is a guest call continuation, which the machine check refuses.
- Tests: a mixer with looping, filtered, enveloped, occluded and 3D
  buffers, a stream with queued packets and a pitch LFO, a 3D mix-in, a
  tapped bin and the reverb renders, is saved and renders on; a fresh
  mixer loads it and renders the same PCM bit for bit (and a mixer at
  another rate refuses unchanged). Through the registered overrides, a
  world with a packet half played, an event pinned and a callback queued
  is saved with guest RAM; a second world with other objects and a pin of
  its own loads it and plays on with the same PCM, statuses, signals,
  callbacks and arena allocations. A WMA session with a held packet,
  half a packet of input and an overlap tail decodes on identically; a
  loaded codec instance decodes the rest of a stream like the saved one.

Wired in phase 5 (below), as planned here:

- Save order between slices: `ThreadedBackend::finish`, then
  `Device::save_snapshot` (D3D writes guest memory), then
  `Hle::save_snapshot`, `Output::save_snapshot`,
  `WmaHle::save_snapshot`, then guest RAM and the kernel. Hold the
  output between `advance_to` calls (logical pacing) while saving.
- Load order: guest RAM, `kcore` and `machine`, then
  `Device::load_snapshot`, `Hle::load_snapshot` (after `kcore`: it
  adopts the saved pins), `Output::load_snapshot`,
  `WmaHle::load_snapshot`, XAPI and feature sections.
- Report `Device::snapshot_blockers`, `Hle::snapshot_blockers` and
  `Output::snapshot_blockers` as host blockers in the probe; `bink-hle`
  (which adds voices to the same mixer) must refuse while a movie plays.
- Not covered: movie voices in the mixer (Bink refuses), the pad
  router's labels (host setting, kept), stream health counters (kept).
- Frame pixels could be saved through `Backend::read_surface` and
  `write_surface` if the first frame after a load matters to
  `render.compare`.

## Phase 5: what was built

Branch `snapshot-phase5`. Tests over fakes; nothing here has run against
the game.

- `agent-proto::state`: method names, params (`name`, `replace`,
  `max_ticks`; `from` for a fork), results (`Saved`: path, bytes, section
  tags, retries, ms; `Loaded`: tick, presents, notes, parts applied, the
  RAM hash check) and `code::BUSY` (7), whose `data` lists the blockers.
- `agent` (`exec/snap.rs`): `state.list` and `state.drop` are answered at
  once from the store; a name to load must exist and a name to save must
  not (unless `replace`), checked before anything waits. A save, load or
  fork is queued; at the next tick `Agent::on_tick` returns true and the
  `agent` feature's observer calls `Runtime::request_preempt`, so the
  slice ends a few transfers after the tick. A paused game lets the job
  through to that slice end and holds again at the next tick (so a load
  while paused shows the loaded tick plus one). The boot loop calls
  `Agent::run_snapshots` between slices; a job that finds a blocker waits
  for the next tick, up to `max_ticks` (90 by default), then answers
  `BUSY`. With no ticks (loading, or `-agent`) every slice boundary is a
  try. The machine is driven without the agent's lock held: the agent's
  own feature section locks it.
- `boot::snapshot_glue`: the parts and their orders. Save: `d3d` (drains
  the render thread through the new `Backend::drain`, then
  `Device::save_snapshot`), `dsound` (and the mixer's `audio`),
  `audioout`, `wma`, `guest` (layout, RAM, TSC), `kernel` (`kcore`,
  `machine`), `xapi`, `features`, then `ramhash`, `lockstep::hash_guest`
  of the RAM saved. Load: `guest`, `kernel`, `d3d`, `dsound`, `audioout`,
  `wma`, `xapi`, `features`, then the RAM hash compared (the MiBs that
  differ are named). Blockers: `Machine::quiescent`, every feature's
  policy, each part's own, a Bink stream open, Xbox Live signed in on any
  controller, a co-op session. The audio output is logical, advanced only
  by the boot loop, so it stands still while a save runs. Feature
  policies are asked again after the save: one that went live meanwhile
  (a co-op thread) throws the file away. A load that fails part way says
  which parts were already loaded.
- The build key is the running tier's stamp (`boot::build_stamp`, or
  `tier::fast_stamp`), `Switches::spec` (canonical
  switch text), the installed feature IDs and the host parts present, so
  a run with other switches, features or subsystems has its own store
  directory. Snapshots are written to the store under `$H2_SNAPSHOTS` or
  the cache directory, never the repository.
- The probe (`H2_SNAPSHOT_PROBE`) reports the same host blockers.
- `h2 agent state save NAME [replace]`, `state load NAME`, `state list`,
  `state drop NAME`, `state fork FROM NAME [replace]`.
- Tests: `agent`'s `exec/snap/tests.rs` (a save waits for a tick; retries
  per tick then `BUSY`; load, fork, drop; name checks; control; a paused
  game; exit) and `boot`'s `snapshot_glue/tests.rs` (through the agent
  over fake parts: save and load orders, the file in the store and its
  header, a RAM hash mismatch naming the MiB, blockers by owner, a
  blocker that appears during the save, the build key).

Not covered: the boot loop's own wiring (`lib.rs`) has no test; the
first check below is it.

### The first real check

On the user's machine, with `h2 serve` (or `h2 play` with `H2_AGENT`)
running the image as usual:

1. Play or drive to a fight. Optionally run with `H2_SNAPSHOT_PROBE=1`
   to see how often a tick boundary is quiescent.
2. `h2 agent state save oldmombasa.fight`. A `BUSY` answer lists what
   stayed live; send it back if it repeats.
3. Load it twice and compare the same N ticks after each load, on one
   connection:

   ```sh
   h2 agent - <<'EOF'
   claim
   pause
   state load oldmombasa.fight
   trace start player.0
   step 300
   trace stop
   state load oldmombasa.fight
   trace start player.0
   step 300
   trace stop
   release
   EOF
   ```

   Each `state load` answer should say `"hash": {"checked": true,
   "equal": true}`. Then `h2 agent flight show <file>` on the two trace
   files `trace stop` named; their rows should match tick for tick.
4. Repeat the loads a few times, and once after playing on for a while,
   and report the `ms` of saves and loads and the file `bytes`.

## Related

- [agent-api.md](agent-api.md): the `state` methods and phases
- [checkpoints.md](checkpoints.md): the game's own checkpoint, the coarse
  fallback
- [high-framerate.md](../game/high-framerate.md): `lockstep` and
  `xapi::tap`
- [BOOT.md](BOOT.md): what the boot sets up, and in which order
