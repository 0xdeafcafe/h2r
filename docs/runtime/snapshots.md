# Machine snapshots

Status: Built. Tested over synthetic machines and fakes; not yet run against
the game ("Not verified").
Code: `crates/snapshot` (container, RAM page codec, store, guest sections),
`crates/runtime` (`Guest::layout`, `restore_with`, continuations),
`crates/kernel/src/snapshot.rs`, `quiesce.rs`, `crates/feature/src/snapshot.rs`,
`crates/boot/src/snapshot_glue.rs`, `snapshot_probe.rs`,
`crates/agent/src/exec/snap.rs`, `crates/agent-proto/src/state.rs`.
Tests: `crates/snapshot/tests/machine.rs`, `crates/kernel/tests/snapshot.rs`,
`crates/kernel/tests/quiesce.rs`, `crates/d3d/tests/snapshot.rs`,
`crates/audio/tests/snapshot.rs`, `crates/dsound/tests/snapshot.rs`,
`crates/xapi/tests/snapshot.rs`, `crates/agent/src/exec/snap/tests.rs`,
`crates/boot/src/snapshot_glue/tests.rs`.
Switch: `H2_SNAPSHOTS` (store root), `H2_SNAPSHOT_PROBE` (off), listed in
[switches.md](switches.md).

This is the machine half of the [agent API](agent-api.md)'s `state.*`
methods: `state.save {name}` and `state.load {name}`. A user plays to a
heavy fight and saves; agents load that exact moment as often as they like,
replay recorded input from it, and profile the frame rate. The page lists
every piece of state a running game holds outside guest RAM, says for each
whether a snapshot saves it, rebuilds it from guest RAM or refuses while it
is live, and says when a snapshot can be taken at all. The rule that must
hold: a snapshot is taken only between two slices with nothing live, and
loads only into the same build and switches.

## Evidence labels

- **Code**: read from this repository; the path is given.
- **Test**: shown by a named test over synthetic state.
- **Doc**: stated by another page here; follow the link.
- **S**: supposed, not verified.

No game data is on this page; guest addresses appear only as named research
points.

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
  be serialised. A snapshot is only taken when none is live. Whether that holds at a
  tick boundary in a real fight is the biggest open item ("Not verified").
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

### The rule

A snapshot is taken **between two slices, at the first slice boundary
after a game tick, where a quiescence check passes**:

1. Every thread's `ExecCtx` has no live continuation, no interrupt in
   service, no pending fault. No DPC is running, the interrupt queue and the
   kernel's deferred-signal queue are empty.
2. Every installed feature and device reports itself quiescent
   (`feature::SnapshotPolicy`, below): D3D not inside the frame wait or a vblank
   delivery, push buffer synced, no Begin/End open, no visibility query
   outstanding; dsound with an empty outbox and callback queue; no Bink
   movie open; no WMA callback in flight; Live signed out with no socket.
3. Host device threads are held: the D3D render thread drained
   (`ThreadedBackend::finish` (through `Backend::drain`)), the dsound pump and output gated.

If the check fails, the request waits for the next tick and tries again,
for at most `max_ticks` ticks (`agent_proto::state::DEFAULT_TICKS`, 90;
at most `MAX_TICKS`), then answers `BUSY` naming every blocker.

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
| Physical RAM, 64 MB | `Guest::ram`, `guest.rs` | Save | The machine. Zero pages and unchanged pages are cheap (format below). |
| Mappings and permissions | `Guest::mappings` | Save | The guest maps and unmaps at run time. |
| Physical high-water mark, id counter | `Guest::next_physical`, `next_id` | Save | Later allocations must land where the original's would. |
| Reservations, HLE claims, code guards | `Guest::reservations`, `claims`, `code` | Check | Made by the boot and HLE crates. A reservation made later (an HLE arena on first use) makes snapshots before and after it incompatible; the load says so. |
| Mapping hints, fast-page table | `Guest::mapping_hints`, `fast_pages` | Rebuild | Caches over the mappings. |
| Write watch | `Guest::watch`, `watch.rs` | Rebuild | Subscriptions are host consumers'. A restore marks every watched page dirty so they re-read. |
| Time-stamp counter | `Tsc::last`, `tsc.rs` | Save | `rdtsc` never repeats; it continues from the saved value. |
| Guest clock hook | `Tsc::clock` | Rebuild | Installed by the boot. |
| Dispatch table and generation | `Runtime::dispatch`, `dispatch_gen` | Check | Overrides and modules installed by the boot; determined by build and switches. A run-time switch change is not carried by the build key (see Rules and limits). |
| Interrupt queue | `InterruptQueue` | Quiesce | Host `GuestInterrupt`s. |
| Preempt words | `Runtime::words` | Rebuild | Per-context wake hints, authoritative state is elsewhere. |
| Per-context: CPU | `Thread::cpu: CpuAbi`, `cpu.rs` | Save | Canonical, host-independent by design ("versioned C types transport canonical state"). |
| Per-context: stashed transfer | `ExecCtx::preempted` | Save | Plain `TransferAbi`. |
| Per-context: continuations | `ExecCtx::continuations` | Quiesce | Rust closures. Rebuildable ones are saved: the whole pool's slot generations and reuse order too, since the tokens are on the guest stack. |
| Per-context: slice | `ExecCtx::slice` | Save  | Budget counters. Likely re-armed per slice, so empty at a boundary; check. |
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
| Files | `Core::files`, `file_handles`, `files.rs` | Save | Each kernel file handle maps to a `vfs::Handle`; re-opened through VFS on load (below). |
| Raw disk device | `rawdisk::RawDisk`, a VFS backend holding up to 2 MiB of written sectors (the cache database) in host memory | Save | Shared with `Core::raw_disk` (`RawSectors`) since the VFS owns the backend. |
| Kernel memory allocator | `Core::memory: memory::Memory`, `memory.rs` | Save | Allocator bookkeeping over guest pages. |
| Config, console, launch page, TLS template, KPCR, bridge | `Core::console`, `launch_page`, `tls`, `kpcr`, `bridge`, `data` | Check | Set by the boot. |
| Deferred signals | `Kernel::deferred` | Quiesce | Drained every slice. |
| Debug log, stops, failures | `Core::log`, `stops`, `deferred_failures` | Ignore | Diagnostics. |

### vfs and storage

| State | Where | Decision | Why |
|---|---|---|---|
| Open files: path, mount, options, position, enumeration cursor, delete flag | `Vfs::objects: OpenFile`, `vfs/src/lib.rs` | Save | Re-open by device and path on load, keeping `vfs::Handle` and kernel handle numbers. Read-only objects only: a save refuses while one is open for writing or deletion. |
| Delete-pending sets, next handle | `Vfs::pending`, `cleanup_pending`, `next_handle` | Save | `next_handle` built; a save refuses while a delete is pending. |
| Host file descriptors, write-behind queue, reservations | `HostBackend`, `WriteBehind`, `vfs/src/host.rs`, `write_behind.rs` | Rebuild | Flush (`vfs::flush_pending_metadata`) before saving. |
| Files on the host disk (saves, cache partitions) | the data root | Not in the snapshot | A load after the game wrote a save or a cache file sees the newer file. A save refuses while a file is open for writing (above); closed files are not recorded. |
| Map overrides | `MapOverrideBackend` | Rebuild | Opened at boot. |

### xapi and input

| State | Where | Decision | Why |
|---|---|---|---|
| Open pad handles, `next_handle`, last state | `Xapi::handles`, `xapi/src/lib.rs` | Save | The guest holds the tokens; replay counts handle mismatches (`tap.rs`), so numbering must continue. Rebound to the device now on the port. |
| Device change words | `Xapi::changes`, voice types' | Save | 12 bytes each; mirrored in guest RAM too. |
| Rumble, completions, signals, launch request | `Xapi::pending_feedback`, `completions`, `signals`, `launch_request` | Quiesce | (`Xapi::snapshot_blockers`). |
| Save-game searches | `Xapi::searches` | Quiesce | Refused while open. |
| Input tap cursor | `InputTap::cursor`/`seek`, `xapi/src/tap.rs` | Save | In the `xapi` section; a load seeks the installed recorder or replayer. |
| Live pads, keyboard and mouse, touch | `input::InputManager`, `backend/*`, touch statics | Rebuild | Host devices. Held keys are not carried: the next sync publishes the host's state. |
| Per-port published state and polling | `InputManager` ports | Save | Packet numbers continue from the saved run's (`port_snapshot`, `restore_port`). |
| Scripted pad cursor | `ScriptedInput` `Playback` | Save | Refuses to rewind (`ScriptError::Rewind`). |

### d3d and gfx

The device is one `Device<B>` behind a mutex (`d3d/src/device.rs`); its
module doc already separates what the guest owns from what the device owns.

| State | Where | Decision | Why |
|---|---|---|---|
| Bindings: render target, depth, textures, palettes, shaders, streams, viewport, transforms, scissors | `Device` fields | Save | The device is authoritative for them. |
| Vertex program and constants, NV2A register state, pushed render state, immediate-mode registers | `program`, `constants`, `hw: HwState`, `pushed`, `immediate` | Save | What the GPU model last saw can differ from guest arrays. |
| Push-buffer cursors, fence, swap count, register-window shadow | `consumed`, `fence`, `fence_at`, `swap_count`, `regs` | Save | After `sync_push_buffer`. |
| Begin/End open, push-buffer callback, latched fault, depth-grid request, visibility counting, `PersistDisplay` surface | `immediate`, `active_callback`, `undelivered`, `fault`, `pending_grid`, `hw.zpass_count_enable`, `display.persisted` | Quiesce | `Device::snapshot_blockers`. |
| Display: gamma, flicker, vblank counters, vblanks not delivered | `Display`, `device/display.rs` | Save | `per_swap` is a host setting, kept. |
| Visibility tests | `visibility: HashMap<u32, VisibilityTest>` | Save | The save reads every ticket's count first (a wait), so the saving run and a load answer the same from then on. Polling mode still makes later counts depend on GPU timing (`threaded.rs`). |
| Frame-buffer ring order and flags | `swap: Swap` | Save | By each buffer's `Data`; the buffers themselves are Check. |
| Destroys the guest has yet to run | `pending_destroys`, `torn_down` | Save | |
| Frame and depth surface pixels | backend surfaces | Rebuild | Not saved: a load keeps the loading device's pixels, so the first frame's history (Halo samples last frame's buffers by parity) is the loading run's. Draw streams do not depend on it. |
| Surfaces of guest surface headers that are not texture levels | `other_surfaces` | Rebuild | Made again blank on next use. Pixels lost. |
| Texture, vertex and index caches, memos, depth copies and aliases | `cache`, `vertex_cache`, `index_cache`, `translated`, `draw_*`, `microcode_*`, `depth_*` | Rebuild | Keyed by guest address; `Device::drop_host_caches`. |
| Render-to-texture targets, alias mirror | `rtt`, `mirror`, `raster.rs` | Rebuild | The save writes dirty levels and the mirror back to guest RAM first; a load drops them. |
| Write tracker | `tracker` | Keep | Every copy held over it is dropped with the caches. |
| Backend GPU objects, pipelines, upload rings, presenter | `GfxBackend`, `ScaledBackend`, `Nv2aShaders`, `gfx::*` | Rebuild | Kept as they are; nothing in them is guest state. |
| Render thread | `d3d-render`, `threaded.rs` | Quiesce | The caller drains it with `ThreadedBackend::finish()` before saving. |

The teardown (`Device::release_device`) writes refcounts into guest
memory, so a load uses the host-only `Device::drop_host_caches` instead.

### audio (dsound, audio, wma)

Decisions per field are in the `snap` modules ([Devices](#devices-d3d-and-audio)).

| State | Where | Decision | Why |
|---|---|---|---|
| Guest objects: buffers and streams (format, data pointer, volume, 3D, effects, mix bins, gates, packet records, mixer ids), root, headroom, effects image, listener, voice rings, token counter | `DirectSound`, `dsound/src/state.rs` | Save | Keyed by guest pointer. |
| Layout (stream vtable, effect count) | `DirectSound::layout` | Check | Fixed by the build. |
| Mixer consumer, lease counter, `frames_per_do_work`, pad router | `DirectSound` | Keep | The loading process's wiring. |
| Mixer: every buffer (with its copy of the guest bytes), stream (queued packets as decoded frames, history), mix-in, effect state, master volume, listener, ids, render clock, tapped samples | `audio::Mixer`, `fx.rs` | Save | Bit-exact PCM after a load. |
| Reverb | `Reverb` | Save | Delay lines and every derived gain, saved exactly. |
| Mixer rate | `Mixer::sample_rate` | Check | |
| Decoded buffer copies, filter coefficient caches, scratch | `SoundBuffer::decoded`, `FxState::coeffs` | Rebuild | Recomputed from the same inputs, bit-equal. |
| Arena bookkeeping | `Hle::arena` | Save | Bytes in guest RAM; the allocator lists are host-side. |
| Event pins | `Hle::pins` | Save | Token and object address; a load adopts them from the loaded kernel's count (`Kernel::adopt_reference`). |
| Stream callbacks queued by a host render | `Hle::callbacks` | Save | Plain data. |
| Event ops published, not delivered | `Hle::outbox` | Quiesce | `Hle::snapshot_blockers`. |
| WMA codec instances | `WmaHle::streams`, `wma` `Session`, `Decoder` | Save | Whole session and decoder state. |
| Output grid (logical pacing) | `output.rs` `Grid` | Save | Origin and frames rendered. |
| Output device, PCM ring, resampler, rendered queue | `DeviceSink`, `sink.rs`, `Shared::queue` | Rebuild | Output only. |
| Pump and device-paced output | `dsound-pump`, output thread | Quiesce | Wall time: `Output::snapshot_blockers`. |

### Features, boot glue and the rest

| State | Where | Decision | Why |
|---|---|---|---|
| Switches | `feature::Switches` | Check | Part of the build key. |
| Feature plan, hooks, arena | `feature::Registry`, `Plan`, `Installed` | Check | Deterministic from switches; arena bytes are guest RAM. |
| Boot glue statics holding guest addresses | `layout_glue`, `campaign_menu`, `list_class`, `stopgaps::KSTACK_NEXT` | Check | Fixed by boot order. |
| Boot glue statics set during play | `health::MAP_MODE`; `skulls_custom::CAMPAIGN`; `level_boot::LOADED_AT`; `skulls_mp`/`skulls_custom` shared state | Save | Each feature owns a section (`SnapshotPolicy::Section`). `h1-health` re-derives from the loaded map; every other feature's policy is in [Feature policies](#feature-policies). |
| Co-op | `coop_live::LOBBY`, `COOP_MODE_PENDING` | Quiesce | Refuse during co-op. `coop-screens`, `coop-live`, `coop-players`, `coop-jip` forbid while armed or in a session. |
| Live | `live-hle` sockets, client thread | Quiesce | Refuse while signed in. |
| Bink | `BinkHle` slots, platform decoder | Quiesce | Refuse while a movie plays. |
| Interpreter fallback caches | `boot/src/fallback.rs` | Rebuild | |
| Lockstep recorder counters | `lockstep::Recorder::ticks`, `presents` | Save | So a trace can continue across a load. The `lockstep` feature's section. |
| Agent session, timeline, perf, telemetry | | Ignore | Observers. The agent's view of the game (cutscene, map, map ticks) is its `agent` section. |

### What "the guest can rebuild" means here

Nothing is rebuilt by asking the guest to redo work: the game never sees a
load. "Rebuild" always means a host cache that refills from guest RAM on
use, or boot state recreated by booting.


## How a load works

In the process that is already running the same image, at a slice
boundary that passes the quiescence check:

1. Open the file, check magic, format version, the file CRC and the build
   key (image build stamp plus switches).
2. Quiesce devices as for a save, then drop host caches: D3D caches and
   tracker, audio output, input devices' held state.
3. Guest: check reservations, claims and code guards match, then replace
   mappings and RAM in place (`Guest::restore_with`, which decodes straight
   into the guest's store).
4. Runtime: TSC. Kernel: `Core` whole, then the machine's threads
   with fresh `ExecCtx`s carrying the saved CPU and continuation pool.
5. VFS: close every open object, re-open the saved ones by path, seek
   (read-only objects).
6. Devices: D3D bindings and GPU-model state, surfaces; dsound objects,
   mixer, WMA sessions, re-pinned events.
7. XAPI handles, input tap cursor (resume replay, or continue recording),
   lockstep counters, feature sections (built; the boot does not call them
   yet).
8. **Determinism check**: `lockstep::hash_guest` over the loaded machine
   must equal the `ramhash` stored at save time (per-MiB chunks, so a
   mismatch names the MiB; `boot/src/snapshot_glue.rs`). To go further, run
   N ticks with the input replay resumed and compare against the trace the
   saving run recorded from the same tick (`lockstep::compare` on records
   from that sequence number on). Both runs need the same observers
   installed, since the tick observer changes dispatch (**Doc**:
   [agent-api.md](agent-api.md) "Cost").

A fresh-process load (boot, then load before the first slice) is the same
sequence after the boot reaches its first slice. It costs a boot, but is
the robust fallback if in-process cache invalidation turns out leaky.


## File format

Code: `crates/snapshot/src/container.rs`, `pages.rs`.

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

`crates/snapshot/src/store.rs`: `$H2_SNAPSHOTS`, else
`~/Library/Caches/h2recomp/snapshots` on Apple hosts,
`%LOCALAPPDATA%\h2recomp\cache\snapshots` on Windows,
`$XDG_CACHE_HOME/h2recomp/snapshots` (or `~/.cache/...`) elsewhere: the
same cache directory as the pipeline cache and the agent socket. Never the
repository: snapshots hold game data. Files are
`<root>/<build id>/<name>.h2snap`, the build id being an FNV-64 of the
build key, so a rebuilt image starts with an empty list and old builds'
directories can be removed whole. Names are `a-z 0-9 . _ -`, at most 64;
writes are atomic (temporary file, then rename).


## Kernel and machine

Code: `crates/kernel/src/snapshot.rs`; tests: `crates/kernel/tests/snapshot.rs`.

- `Machine::save_snapshot` refuses unless `Machine::quiescent()` and
  `load_snapshot` write and read the sections `kcore` (version 2) and
  `machine` on the snapshot crate's codec. The `Persist` trait and
  `persist_struct!` destructure every saved struct without `..`, and
  `save_core` destructures `Core` whole, so a new field does not compile
  until it has a decision.
- Saved: every table in the [inventory](#kernel), the clock with its
  history and charging bit, the periodic schedules (put back onto the
  booted source of the same id and name, which keeps its Rust handler),
  the raw disk's sectors (`rawdisk::RawSectors`, shared by `Core::raw_disk`
  from `set_shared_vfs`), and the machine's scheduling state (`schedule`,
  `drained`, `dpc_stack_top`, `preemptions`).
- Checked against the booted machine: KPCR, export bridge, data exports,
  title id, utility partition. Kept from it: TLS template, console config,
  image sections, storage handle, diagnostics, and the machine's config
  fields (`preempt_slice`, `max_transfers_per_slice`, `pacer`). Per-context
  event logs are not saved.
- `Rebuilders` maps a continuation origin to a factory; the kernel's thread
  return is built in ([Rebuildable continuations](#rebuildable-continuations)).
  `runtime::ContinuationLayout` (`ExecCtx::continuation_layout`,
  `restore_continuations`) holds the pool as plain data with every slot's
  generation and the reuse order, so tokens made after a load are the
  saved run's. A live closure that is not rebuildable refuses the save; a
  load with no rebuilder for an origin changes nothing.
- `vfs::Vfs::state` and `restore_state`: open objects by device and path
  with handle numbers, options, position and search cursor (directory
  entries are read again at load). Refuses while an object can write or
  delete, or a delete is pending; a load looks every file up before it
  closes anything.
- `xapi::snapshot` (`crates/xapi/src/snapshot.rs`): the `xapi` section.
  Pad handles (token, port, last state), `next_handle`, the gamepad and
  communicator change words, each port's published state and polling
  parameters (`InputManager::port_snapshot`, `restore_port`). A load rebinds
  each handle with `InputManager::reopen`: on an empty port it reads as
  unplugged and stays stale after an insertion, and the change words report
  the difference from the saved ports at the next `XGetDeviceChanges`.
  Refused while rumble, a launch request or a save search is in flight.
- The input tap's cursor (`InputTap::cursor` and `seek`, `xapi/src/tap.rs`)
  is in the `xapi` section: a load seeks the installed recorder or
  replayer (`Replayer::resume`, `Recorder::resume`) and returns the cursor
  for a caller that installs one later.
- Feature sections: `SnapshotPolicy::Section` and `section_forbid_while`
  carry a feature's bytes and a load function given the runtime.
  `Plan::save_sections` and `load_sections` collect them;
  `snapshot::features` stores them as one `features` section; a load
  refuses a file whose section features differ before it loads any.
- A load into a running game drops the old contexts' fast-memory views and
  direct-call blocks with them. Only synthetic machines test that.

Test (`kernel/tests/snapshot.rs`): three guest threads sharing an event,
sleeping on different periods and reading a read-only VFS file run N slices
and are saved (RAM, TSC, `kcore`, `machine`); the original runs M more. A
machine booted the same way loads the file and runs M; every slice's digest
(mapped RAM, clock, CPUs, scheduled thread) matches, then whole RAM,
schedule, clock, TSC and VFS state. One created thread returns from its
start routine after the load, through its rebuilt continuation. A snapshot
loads twice into one machine; a machine booted with other data exports
refuses it; a live closure continuation refuses the save.

## Devices (D3D and audio)

### D3D

Code: `crates/d3d/src/device/snap.rs`; tests: `crates/d3d/tests/snapshot.rs`.

- `Device::save_snapshot(mem, out)` adds the `d3d` section. It runs between
  slices and before guest RAM is saved, since it writes to guest memory: it
  drains the push buffer, writes dirty render-to-texture levels and the
  alias mirror back, and reads every visibility count. It returns
  `SnapshotError::Busy` while `Device::snapshot_blockers` names anything.
- `saved()` destructures `Device` and `Display` without `..`, each field
  under its decision.
- `Device::load_snapshot(snapshot)` decodes and checks first (frame-buffer
  and depth headers, the ring's buffers, size and flip, the register
  window), then drops host caches and restores. Nothing changes on a
  mismatch. Guest RAM must already be loaded.
- The frame wait (`vblank.rs`, `Wait`) and vblank delivery hold their state
  only in `call_guest` continuations, which `Machine::quiescent` reports.
  The device holds no Rust state across yields, so a save is allowed when
  the machine is quiescent, `snapshot_blockers` is empty and the render
  thread has been drained.
- Test (over `RecordingBackend`): a device draws three frames with a
  transform, a scissor and a visibility test, is saved and draws three
  more. A second device, created the same way but having drawn other frames
  with other state, loads the section over the saved guest memory and draws
  the same three frames. The recorded clears, draws and presents match, as
  do the ring, the visibility answer and guest memory. A device created
  with other present parameters refuses the section unchanged; a save
  inside Begin/End is refused.

### Audio

Code: `crates/audio/src/snap.rs`, `crates/dsound/src/state/snap.rs`,
`crates/dsound/src/register/snap.rs`, `crates/dsound/src/output.rs`,
`crates/dsound/src/xwma.rs`, `crates/wma` (`Session::save_state`). Tests:
`crates/audio/tests/snapshot.rs`, `crates/dsound/tests/snapshot.rs`,
`crates/dsound/tests/xwma.rs`, `crates/wma/src/boundary.rs`.

- `Mixer::save_snapshot` adds `audio`; `load_snapshot` is `check_snapshot`
  (decode, check the rate, change nothing) then `apply_snapshot`. Every
  struct is destructured without `..`.
- `Hle::save_snapshot` adds `dsound` and `audio`: the `DirectSound` objects
  and settings, the arena's lists, the pins as (token, object address) and
  the queued stream callbacks. Refused while event batches are undelivered
  or a pin cannot name its object (`EventRef::address`). `Hle::load_snapshot`
  decodes and checks both sections and adopts the pins (`Events::adopt`)
  before changing anything; the loading run's own pins are dropped with
  `EventRef::forget`, which takes no count, because the loaded kernel's
  `references` are the saved run's (`Kernel::adopt_reference`,
  `DispatcherReference::forget`).
- `Output::save_snapshot` adds `audioout` (the logical grid's origin and
  frames rendered); device pacing refuses (`Output::snapshot_blockers`).
- `WmaHle::save_snapshot` adds `wma`: every codec instance by handle with
  its callback, context and whole `Session` (decoder overlap, held
  reservoir packet, exponents, block sizes, partial input, queued PCM), and
  the next handle. A begin packet pulling input is a guest call
  continuation, which the machine check refuses, so no
  refuse-while-decoding policy is needed.
- Not covered: movie voices in the mixer (Bink refuses while a movie
  plays), the pad router's labels (a host setting, kept), stream health
  counters (kept).
- Tests: a mixer with looping, filtered, enveloped, occluded and 3D
  buffers, a stream with queued packets and a pitch LFO, a 3D mix-in, a
  tapped bin and the reverb renders, is saved and renders on; a fresh mixer
  loads it and renders the same PCM bit for bit (a mixer at another rate
  refuses unchanged). Through the registered overrides, a world with a
  packet half played, an event pinned and a callback queued is saved with
  guest RAM; a second world with other objects and a pin of its own loads
  it and plays on with the same PCM, statuses, signals, callbacks and arena
  allocations. A WMA session with a held packet, half a packet of input and
  an overlap tail decodes on identically.

Frame pixels are not saved; they could go through `Backend::read_surface`
and `write_surface` if the first frame after a load matters to
`render.compare`.

## Wiring: `state.*`

Code: `crates/agent-proto/src/state.rs`, `crates/agent/src/exec/snap.rs`,
`crates/boot/src/snapshot_glue.rs`. Tests: `crates/agent/src/exec/snap/tests.rs`
(a save waits for a tick; retries per tick then `BUSY`; load, fork, drop;
name checks; a paused game; exit) and `crates/boot/src/snapshot_glue/tests.rs`
(through the agent over fake parts: save and load orders, the file in the
store and its header, a RAM hash mismatch naming the MiB, blockers by owner,
a blocker that appears during the save, the build key).

- `agent_proto::state` holds the method names, params (`name`, `replace`,
  `max_ticks`; `from` for a fork), results (`Saved`: path, bytes, section
  tags, retries, ms; `Loaded`: tick, presents, notes, parts applied, the RAM
  hash check) and `code::BUSY` (7), whose `data` lists the blockers.
- `state.list` and `state.drop` are answered at once from the store. A name
  to load must exist and a name to save must not (unless `replace`),
  checked before anything waits. A save, load or fork is queued; at the
  next tick `Agent::on_tick` returns true and the `agent` feature's observer
  calls `Runtime::request_preempt`, so the slice ends a few transfers after
  the tick. A paused game lets the job through to that slice end and holds
  again at the next tick (a load while paused shows the loaded tick plus
  one). The boot loop calls `Agent::run_snapshots` between slices; a job
  that finds a blocker waits for the next tick, up to `max_ticks`, then
  answers `BUSY`. With no ticks (loading, or `-agent`) every slice boundary
  is a try. The machine is driven without the agent's lock held: the
  agent's own feature section locks it.
- `boot::snapshot_glue` owns the parts and their orders. Save: `d3d` (drains
  the render thread through `Backend::drain`, then `Device::save_snapshot`),
  `dsound` (and the mixer's `audio`), `audioout`, `wma`, `guest` (layout,
  RAM, TSC), `kernel` (`kcore`, `machine`), `xapi`, `features`, then
  `ramhash`, the `lockstep::hash_guest` of the RAM saved. Load: `guest`,
  `kernel`, `d3d`, `dsound` (after `kcore`: it adopts the saved pins),
  `audioout`, `wma`, `xapi`, `features`, then the RAM hash compared (the
  MiBs that differ are named). A load that fails part way says which parts
  were already loaded.
- Blockers: `Machine::quiescent`, every feature's policy, each part's own,
  a Bink stream open, Xbox Live signed in on any controller, a co-op
  session. The audio output is logical, advanced only by the boot loop, so
  it stands still while a save runs. Feature policies are asked again after
  the save: one that went live meanwhile (a co-op thread) throws the file
  away.
- The build key is the running tier's stamp (`boot::build_stamp`, or
  `tier::fast_stamp`), `Switches::spec` (canonical switch text), the
  installed feature ids and the host parts present, so a run with other
  switches, features or subsystems has its own store directory.
- `h2 agent state save NAME [replace]`, `state load NAME`, `state list`,
  `state drop NAME`, `state fork FROM NAME [replace]`.

## Quiescence probe

Code: `crates/kernel/src/quiesce.rs`, `crates/boot/src/snapshot_probe.rs`;
tests: `crates/kernel/src/quiesce.rs` and `crates/kernel/tests/quiesce.rs`,
the latter over guests that really run (threads waiting in the kernel are
quiescent; a thread blocked inside an override's guest callback is a blocker
until the callback returns).

- `runtime`: every continuation records an origin and the guest address it
  called (`ExecCtx::call_guest_named`; plain `call_guest` records the
  closure's type name). `ExecCtx::live_continuations`,
  `interrupt_in_service` and `fault_pending` report them.
- `kernel::quiesce`: `Machine::quiescent()` returns every blocker:
  continuations per thread (and on the DPC context), an interrupt callback
  in service, a pending fault, queued interrupts, deferred signals, a
  periodic handler that is running, a stopped machine. The continuation
  every created thread holds for its whole life (`THREAD_RETURN`, which only
  terminates the thread) is listed as rebuildable, not as a blocker.
  `kernel::quiesce::Tally` counts checks and blockers and makes the
  rate-limited lines.
- `feature::Builder::snapshot(SnapshotPolicy)`: `Stateless`,
  `ForbidWhileLive(fn)` (the function says, between slices, why it is
  live), `Section` (bytes saved and loaded), or, for a feature that says
  nothing, `Unaudited`, which blocks every snapshot, so an unaudited feature
  fails closed. `Plan::snapshot_blockers()` lists them;
  `Plan::describe_target` names the hooked point behind an after-hook
  continuation (`feature: after hooks`, called through a chain's shadow
  entry).

### Running the probe

```sh
H2_SNAPSHOT_PROBE=1 ...       # check at the end of the slice each game tick ends in
H2_SNAPSHOT_PROBE=slice ...   # don't end slices early; check at the next boundary
```

`H2_SNAPSHOT_PROBE=1` installs a `snapshot_probe` feature whose tick observer
counts ticks and calls `Runtime::request_preempt`, so the slice ends a few
transfers after the tick, where `state.save` would act. That changes the
schedule: don't compare a probed run with an unprobed one under lockstep.
Every line starts with `snapshot-probe:`:

```text
snapshot-probe: on, checking at the end of the slice each game tick
snapshot-probe: tick=812 machine=blocked [thread 1: continuation feature: after hooks -> 0x... (after hooks on <point> (0x...) [feature:label])] host_blockers=31
snapshot-probe: tick=813 machine=quiescent host_blockers=31
snapshot-probe: progress tick=1800 checked=1800 machine_quiescent=... (..%) fully_quiescent=0 (0.0%)
snapshot-probe: summary checked=... machine_quiescent=... (..%) fully_quiescent=... (..%)
snapshot-probe: top machine <count> (<%>) <blocker>
snapshot-probe: top host <count> (<%>) feature <id>: not audited for snapshots
```

- A `tick=` line is printed only when the set of machine blockers changes,
  at most 200 of them; a `progress` line every 1800 checks; the `summary`
  and up to ten `top machine` and ten `top host` lines at exit.
- **Machine** blockers are the hard question the probe answers. **Host**
  blockers are features' live predicates and the host subsystems' own
  (`snapshot_glue::Host::blockers`: d3d, dsound and its output, XAPI, a Bink
  movie open, Xbox Live signed in, co-op), each as `host <owner>`.

The result of the first probe run is in
[notes/snapshots.md](notes/snapshots.md).

### Rebuildable continuations

A continuation made with `ExecCtx::call_guest_rebuildable(.., origin,
state: [u32; 4], ..)` promises that its closure depends only on `origin`,
`target`, `state` and the context it runs on. Quiescence lists it as
rebuildable, with its state, instead of as a blocker. A load makes it
again: a registry (`Rebuilders`) from `origin` to a factory, and a
`Continuations` insert at a given token (the token is on the guest stack,
so it must be the same). Two exist (**Code**):

| Origin | Made by | State | Rebuilt as |
|---|---|---|---|
| `kernel: thread return` | `Machine::collect_threads`, every created thread's start routine | none | terminate the thread with success |
| `trace: kernel call return` | `boot::trace::install_bounded`, every kernel call, through the shadow `0xa0ff0000 + 4 * ordinal` | caller's return address, trace sequence number | return to the caller; the trace record is gone, and `finish` of an unknown one does nothing |

### Feature policies

Every `feature::Feature` declares a `SnapshotPolicy` in its own `build`
(`rg 'b\.snapshot\(' crates`). A predicate's test sets each live condition
and checks it is reported, and clear again; a section's test round-trips its
bytes and checks that damaged bytes change nothing. The kinds, and what
each one holds:

- **Stateless**: observers, and rewrites whose state is all in guest memory
  or fixed at build (`engine-leaves`, `ui-strings`, `settings-rows`).
- **Forbid while live**: host state that is live only between hooks or while
  a screen is up (`cheats` while a cheat is on, the co-op features while
  armed, `h1-health` while a damage update is between its hooks,
  `render-timing` while a render stage is open).
- **Section**: host state a load puts back (`skulls`, `skulls-custom`,
  `skulls-mp`, `hud`, `h1-hud`, `lockstep`'s counters, `agent`'s view of the
  game, `loaded_map`, `aim-assist`).

Open items for list classes and screens are under "Not verified".

### The render features

Boot registers the render features in every run, with their
`impl.render.*` switches off by default. Each declares its policy before
any early return for a switch that is off, so a run with all of them off
is not `Unaudited` and `state.save` is not refused. The other `native-*`
features (`crates/feature/src/native_*.rs`) are `Stateless`.

None of them changes simulation state. A save runs between slices, with
the D3D render thread drained and the pipelined render not in flight
(`pipeline-render` forbids while it is). Section loads run last, on the
game thread, over loaded guest RAM and a loaded device; they clear
Mutex-guarded statics and take no device lock. Their GPU caches are
keyed by what they read from guest RAM at every ask (the map name, and
for the world the BSP address and index), so a load that changes map
re-keys them.

| Feature | Off | On | What a load drops |
|---|---|---|---|
| `occlusion` | stateless | section, no bytes | `occlusion::forget`: every view's streaks, last camera and depth, the armed capture, the captures in flight, the published depth frames (`MAILBOX`), the last grid. `Tracker::begin` already reset on a camera cut or when the frame went backwards. A load to a later frame with the camera close by kept streaks keyed by handles that may now name other objects |
| `native-world` | stateless | section, no bytes | `native_world::forget_pending`: the layer-3 bits waiting to be set back at the next layer draw, without setting them (the loaded item lists are the loaded render's) |
| `native-objects` | stateless | section, no bytes | `native_objects::forget_pending`: as the world's |
| `native-lights` | stateless (also when not placed with render-timing on) | section, no bytes | `native_lights::forget_runs`: the open run, the split light and the split layer's pending restore. `VIEW_TARGET` is asked for again at every view |
| `native-transparents` | stateless | stateless | nothing: the open run and the objects drawn last one view (`new_view` forgets them), and it never writes guest memory |
| `native-post` | stateless | stateless | nothing: no frame history (no temporal AA, no exposure adaptation); the screen effects are keyed by map, and the proof and fallback sets only ever send an effect back to D3D |


## Rules and limits

- A snapshot loads only into the same build and switches, into a machine
  that is already booted; a mismatch refuses and changes nothing.
- **Host files**: the snapshot does not cover the data root. Loading after
  the game wrote a save or a cache partition file can diverge.
- **Run-time reservations**: an HLE crate that reserves physical memory on
  first use (a movie arena, say) makes earlier and later snapshots
  incompatible with a machine in the other state. The load refuses with the
  difference.
- **Frame pixels** are the loading run's: the first frame's history (Halo
  samples last frame's buffers by parity) is not the saved run's. Draw
  streams do not depend on it; `render.compare` may.
- **Wall-clock devices**: GPU visibility polling makes runs
  nondeterministic whether or not snapshots exist (`d3d/src/threaded.rs`);
  a snapshot replay inherits that. Audio output is logical (see Wiring).
- **Live switch changes** (agent API overrides) change the dispatch table at
  run time; the build key carries `Switches::spec`, so it must carry the live
  switch state rather than the launch one for such a run to key correctly.
- Compression is spread over independent 1 MB frames, so it can be threaded
  without a format change.

## Not verified

Only a run on the game shows these.

- **Quiescence at a tick boundary in a fight.** The first probe run
  ([notes](notes/snapshots.md)) found one machine blocker on every tick,
  since made rebuildable. That the machine is quiescent at tick boundaries
  during a fight, and how often, is not yet seen.
- **Size and speed**: 64 MB of RAM, mostly in use during a fight, at zstd
  level 1 is estimated at tens of MB and well under a second; not measured
  on game RAM.
- **The boot loop's wiring** (`crates/boot/src/lib.rs`) has no test.
- **List classes and screens** keep row state that `list_class`'s policy
  (constructor running) does not ask about: the More Options page's unsaved
  edit, `SigninRows`' shown flips, the mods list and detail, `map_select`'s
  picked category. A load while one of those screens is up could show stale
  rows. `list_class` should ask its `Rows` for liveness.
- `SkullsScreen` has no close, so a snapshot saved before a Skulls screen
  was opened leaves a running process's profile in place at a load; a press
  opens it again before it shows.
- `live-telemetry` is stateless for the guest, but a load during a Live
  match would upload a rewound match record.
- Co-op's background threads can change the role at any moment; the boot's
  `state.save` checks again after the save and throws the file away, but a
  save that lets them in between is not tested.

To check on a running image, with `h2 serve` (or `h2 play` with `H2_AGENT`):

1. Play or drive to a fight. Optionally run with `H2_SNAPSHOT_PROBE=1` to
   see how often a tick boundary is quiescent.
2. `h2 agent state save oldmombasa.fight`. A `BUSY` answer lists what stayed
   live.
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
4. Repeat the loads a few times, and once after playing on for a while, and
   record the `ms` of saves and loads and the file `bytes`.

## Related

- [agent-api.md](agent-api.md): the `state` methods
- [checkpoints.md](checkpoints.md): the game's own checkpoint, the coarse
  fallback
- [high-framerate.md](../game/high-framerate.md): `lockstep` and
  `xapi::tap`
- [BOOT.md](BOOT.md): what the boot sets up, and in which order
- [notes/snapshots.md](notes/snapshots.md): the first probe run
