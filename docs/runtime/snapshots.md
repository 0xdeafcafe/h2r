# Machine snapshots

Status: Built. Tested over synthetic machines and fakes; not yet run against
the game ("Not verified").
Code: `crates/runtime/snapshot` (container, RAM page codec, store, guest sections),
`crates/runtime/runtime` (`Guest::layout`, `restore_with`, continuations),
`crates/runtime/kernel/src/snapshot.rs`, `quiesce.rs`, `crates/hooking/hooks/src/snapshot.rs`,
`crates/extensions/game/src/snapshot_glue.rs`, `snapshot_probe.rs`,
`crates/extensions/agent/src/exec/snap.rs`, `crates/extensions/agent-proto/src/state.rs`.
Tests: `crates/runtime/snapshot/tests/machine.rs`, `crates/runtime/kernel/tests/snapshot.rs`,
`crates/runtime/kernel/tests/quiesce.rs`, `crates/platform/d3d/tests/snapshot.rs`,
`crates/platform/audio/tests/snapshot.rs`, `crates/platform/dsound/tests/snapshot.rs`,
`crates/platform/xapi/tests/snapshot.rs`, `crates/extensions/agent/src/exec/snap/tests.rs`,
`crates/extensions/game/src/snapshot_glue/tests.rs`.
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

## Pages

| Page | Holds |
|---|---|
| [Machine snapshots: kernel, devices and wiring](snapshots-state.md) | The kernel and machine state, D3D and audio devices, and the `state.*` wiring. |
| [Machine snapshots: the quiescence probe](snapshots-quiescence.md) | Running the probe, rebuildable continuations, feature policies and the render features. |

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

The request must come from a tick observer (or later), not from a
`before` on `GAME_TICK`. Once any feature observes the tick, the tick
chain calls the original through its shadow with an after-hooks
continuation (`hooks::install::AFTER_HOOKS`) that stays live until the
original returns, so a slice a `before` ends stops inside the tick with
that continuation live, and the quiescence check refuses it on every tick
(`crates/hooking/hooks/tests/install.rs`,
`a_slice_asked_before_the_tick_ends_inside_its_after_hooks`). The bounce
search's fork keeps its ask from its `before` and ends the slice from its
tick observer ([bounce-search.md, "The fork"](bounce-search.md#the-fork)).

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
| Entry table (`live`) | `Runtime::live` | Rebuild | Follows overrides and `Guest::code_id`, which `adopt` moves, so a restore re-reads every entry's execute bit. |
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
| Open files: path, mount, options, position, enumeration cursor, changed flag | `Vfs::objects: OpenFile`, `vfs/src/lib.rs` | Save | Re-open by device and path on load, keeping `vfs::Handle` and kernel handle numbers. A file open for writing is saved whole and written back before the re-open: the title keeps `z:\preferences.dat` open for writing (GENERIC_READ and GENERIC_WRITE, FILE_OPEN_IF) from boot to exit and rewrites it in place. One over `vfs::MAX_CARRIED_BYTES` (1 MiB) is saved by reference and rewound through the VFS's journal: the game keeps its map cache slots (`cache00N.map`, 180 MB) open for writing while it plays. A save refuses an object that may delete. |
| Delete-pending sets, next handle | `Vfs::pending`, `cleanup_pending`, `next_handle` | Save | `next_handle` built; a save refuses while a delete is pending. |
| Host file descriptors, write-behind queue, reservations | `HostBackend`, `WriteBehind`, `vfs/src/host.rs`, `write_behind.rs` | Rebuild | Flush (`vfs::flush_pending_metadata`) before saving. |
| Files on the host disk (saves, cache partitions) | the data root | Not in the snapshot | A load after the game wrote a save or a cache file sees the newer file. Only files open for writing at the save are carried or referenced (above); closed files are not recorded. |
| Editor play copy | `MapOverrideBackend`, `vfs/src/state.rs` | Refuse | An active file preparation, reservation or published binding prevents save and restore, including snapshot-based rewind and film keyframe capture. Cancelling preparation releases its claim immediately. A closed DVD source can still supply a resident cache slot. Restart without the binding to use snapshots. Startup directory overrides resolve through the boot mount. |
| Unlinked editor placement preview | `agent/src/sim/calls/placement.rs` | Refuse | An outstanding preview or an interrupted creation whose result is unknown prevents save and restore. A different scenario clears it. Deleting a multiplayer preview also clears it. A checkpoint may still hold a deleted campaign preview on the same mission. |

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
| Surfaces of guest surface headers that are not texture levels | `other_surfaces` | Rebuild | Made again on next use. Without the frame graph's store blank, pixels lost; with it the role's surface comes back as the loading run left it ([retire-d3d-display.md](../graphics/retire-d3d-display.md#the-graphs-targets)). |
| Texture, vertex and index caches, memos, depth copies and aliases | `cache`, `vertex_cache`, `index_cache`, `translated`, `draw_*`, `microcode_*`, `depth_*` | Rebuild | Keyed by guest address; `Device::drop_host_caches`. |
| Render-to-texture targets, alias mirror | `rtt`, `mirror`, `raster.rs` | Rebuild | The save writes dirty levels and the mirror back to guest RAM first; a load drops them. |
| Write tracker | `tracker` | Keep | Every copy held over it is dropped with the caches. |
| Backend GPU objects, pipelines, upload rings, presenter | `GfxBackend`, `ScaledBackend`, `Nv2aShaders`, `gfx::*` | Rebuild | Kept as they are; nothing in them is guest state. |
| Render thread | `d3d-render`, `threaded.rs` | Quiesce | The caller drains it with `ThreadedBackend::finish()` before saving. |

The teardown (`Device::release_device`) writes refcounts into guest
memory, so a load uses the host-only `Device::drop_host_caches` instead.

### audio (dsound, audio, wma)

Decisions per field are in the `snap` modules ([Devices](snapshots-state.md#devices-d3d-and-audio)).

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
| Agent session, timeline, perf, telemetry | | Ignore | Observers. The agent's view of the game (cutscene, map, map ticks) is its `agent` section. The host refuses capture and restore while an unlinked editor preview can still exist. |

### What "the guest can rebuild" means here

Nothing is rebuilt by asking the guest to redo work: the game never sees a
load. "Rebuild" always means a host cache that refills from guest RAM on
use, or boot state recreated by booting.

## How a load works

In the process that is already running the same image, at a slice
boundary that passes the quiescence check:

1. Open the file, check magic, format version, the file CRC and the build
   key (image build stamp plus switches). Refuse a machine with a pending
   or published editor copy or an unlinked preview. Validate and decode the
   kernel and thread sections, check their boot dependencies and resolve
   their saved file objects before applying any part of the snapshot. This
   check also runs before a film keyframe restores its Live claims or RAM.
2. Quiesce devices as for a save, then drop host caches: D3D caches and
   tracker, audio output, input devices' held state.
3. Guest: check reservations, claims and code guards match, then replace
   mappings and RAM in place (`Guest::restore_with`, which decodes straight
   into the guest's store).
4. Runtime: TSC. Kernel: `Core` whole, then the machine's threads
   with fresh `ExecCtx`s carrying the saved CPU and continuation pool.
5. VFS: close every open object, write back the files carried whole,
   rewind the ones carried by reference, re-open the saved objects by
   path, seek.
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
the safe fallback if in-process cache invalidation turns out leaky.

## File format

Code: `crates/runtime/snapshot/src/container.rs`, `pages.rs`.

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

`crates/runtime/snapshot/src/store.rs`: `$H2_SNAPSHOTS`, else
`snapshots` in the cache directory
([storage.md](storage.md#host-directories)), the same directory as the
pipeline cache and the agent socket. Never the
repository: snapshots hold game data. Files are
`<root>/<build id>/<name>.h2snap`, the build id being an FNV-64 of the
build key, so a rebuilt image starts with an empty list and old builds'
directories can be removed whole. Names are `a-z 0-9 . _ -`, at most 64;
writes are atomic (temporary file, then rename).

## Kernel and machine

See [Machine snapshots: kernel, devices and wiring](snapshots-state.md).

## Quiescence probe

See [Machine snapshots: the quiescence probe](snapshots-quiescence.md).

### Rebuildable continuations

See [Machine snapshots: the quiescence probe](snapshots-quiescence.md).

### Feature policies

Every `feature::Feature` declares a `SnapshotPolicy` in its own `build` (`rg 'b\.snapshot\(' crates`). Detail: [Machine snapshots: the quiescence probe](snapshots-quiescence.md).

## Rules and limits

- A snapshot loads only into the same build and switches, into a machine
  that is already booted; a mismatch refuses and changes nothing.
- **Host files**: the snapshot does not cover the data root, only the
  files open for writing at the save (large ones by reference, rewound
  through a journal of at most 32 MiB). Loading after the game wrote and
  closed a save or a cache partition file can diverge.
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
- **The boot loop's wiring** (`crates/apps/boot/src/lib.rs`) has no test.
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
