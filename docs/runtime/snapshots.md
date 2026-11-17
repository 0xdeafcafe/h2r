# Machine snapshots (design)

Status: phase 1 built ([below](#phase-1-what-was-built)): the file format,
the guest RAM page codec, the store, the guest layout and RAM restore, and
the time-stamp counter, all in `crates/snapshot` and `crates/runtime`, with
the input tap's resume in `crates/xapi`. Nothing is wired into a running
game yet. Everything after phase 1 on this page is design.

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
| dsound stream callbacks and event ops not yet run | `dsound` `Hle::callbacks`, `outbox` | Queued Rust work. Can be drained. |
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
| Per-context: continuations | `ExecCtx::continuations` | Quiesce | Rust closures. |
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
| Files | `Core::files`, `file_handles`, `files.rs` | Save | Each kernel file handle maps to a `vfs::Handle`; re-opened through VFS on load (below). |
| Kernel memory allocator | `Core::memory: memory::Memory`, `memory.rs` | Save | Allocator bookkeeping over guest pages. |
| Config, console, launch page, TLS template, KPCR, bridge | `Core::console`, `launch_page`, `tls`, `kpcr`, `bridge`, `data` | Check | Set by the boot. |
| Deferred signals | `Kernel::deferred` | Quiesce | Drained every slice. |
| Debug log, stops, failures | `Core::log`, `stops`, `deferred_failures` | Ignore | Diagnostics. |

### vfs and storage

| State | Where | Decision | Why |
|---|---|---|---|
| Open files: path, mount, options, position, enumeration cursor, delete flag | `Vfs::objects: OpenFile`, `vfs/src/lib.rs` | Save | Re-open by path and seek on load, keeping `vfs::Handle` and kernel handle numbers. Read-only map and disc files are always safe. |
| Share-mode sets, next handle | `Vfs::pending`, `cleanup_pending`, `next_handle` | Save | |
| Host file descriptors, write-behind queue, reservations | `HostBackend`, `WriteBehind`, `vfs/src/host.rs`, `write_behind.rs` | Rebuild | Flush (`vfs::flush_pending_metadata`) before saving. |
| Files on the host disk (saves, cache partitions) | the data root | Not in the snapshot (S) | A load after the game wrote a save or a cache file sees the newer file. Phase 3 either records the writable files' contents or refuses while one is open for writing. |
| Map overrides | `MapOverrideBackend` | Rebuild | Opened at boot. |

### xapi and input

| State | Where | Decision | Why |
|---|---|---|---|
| Open pad handles, `next_handle`, last state | `Xapi::handles`, `xapi/src/lib.rs` | Save | The guest holds the tokens; replay counts handle mismatches (`tap.rs`), so numbering must continue. Rebind each to a port. |
| Device change words | `Xapi::changes` | Save | 12 bytes; mirrored in guest RAM too. |
| Rumble, completions, signals, launch request | `Xapi::pending_feedback`, `completions`, `signals`, `launch_request` | Quiesce | |
| Save-game searches | `Xapi::searches` | Save or Quiesce | Rare; refuse first. |
| Input tap cursor | `Replayer::next`, `Recorder` log length, `xapi/src/tap.rs` | Save | The cursor is `ReplayStats::replayed` (or the recorder's log length); `Replayer::resume` and `Recorder::resume` continue from it. Built. |
| Live pads, keyboard and mouse, touch | `input::InputManager`, `backend/*`, touch statics | Rebuild | Host devices. Release held keys on load. Packet numbers per port: Save (S), if the game compares them. |
| Scripted pad cursor | `ScriptedInput` `Playback` | Save | Refuses to rewind today (`ScriptError::Rewind`). |

### d3d and gfx

The device is one `Device<B>` behind a mutex (`d3d/src/device.rs`); its
module doc already separates what the guest owns from what the device owns.

| State | Where | Decision | Why |
|---|---|---|---|
| Bindings: render target, depth, textures, palettes, shaders, streams, viewport, transforms, scissors | `Device` fields | Save | The device is authoritative for them. |
| Vertex program and constants, NV2A register state, pushed render state | `program`, `constants`, `hw: HwState`, `pushed` | Save | What the GPU model last saw can differ from guest arrays. |
| Push-buffer cursors, fence, swap count | `consumed`, `fence`, `swap_count`, … | Save | After `sync_push_buffer`; no Begin/End open, no callback in flight (Quiesce). |
| Display: gamma, flicker, vblank counters | `Display`, `device/display.rs` | Save | |
| Visibility tests | `visibility: HashMap<u32, VisibilityTest>` | Save | Resolve every ticket to a count first. Polling mode makes counts depend on GPU timing, a determinism hazard (`threaded.rs`). |
| Frame-buffer ring order and flags | `swap: Swap` | Save | |
| Frame and depth surface pixels | backend surfaces | Save (S) | Only on the host. Halo samples last frame's buffers by parity, so blanks would show for about a frame. Phase 4 decides whether a frame of wrong pixels matters to a profile; if not, Rebuild blank. |
| Texture, vertex and index caches, memos, write tracker | `cache`, `vertex_cache`, `index_cache`, `tracker`, … | Rebuild | Keyed by guest address and validated by generations. |
| Render-to-texture targets, alias mirror | `rtt`, `mirror`, `raster.rs` | Rebuild | After `flush_render_targets` writes dirty host pixels back to guest RAM. |
| Backend GPU objects, pipelines, upload rings, presenter | `GfxBackend`, `ScaledBackend`, `Nv2aShaders`, `gfx::*` | Rebuild | Flush and wait first. |
| Render thread | `d3d-render`, `threaded.rs` | Quiesce | Drain with `finish()`. |

The existing teardown (`Device::release_device`) is not usable as is: it
writes refcounts into guest memory. Phase 4 adds a host-only "drop caches"
next to it.

### audio (dsound, audio, wma)

| State | Where | Decision | Why |
|---|---|---|---|
| Buffer and stream objects: format, guest data pointer, volume, pitch, 3D, mix bins, packet records | `DirectSound::objects`, `dsound/src/state.rs` | Save | Keyed by guest pointer. |
| Mixer: buffers' play cursors, loop state, effect state | `audio::Mixer`, `SoundBuffer`, `fx.rs` | Save | `SoundBuffer` holds a copy of guest bytes taken at `Unlock`/`Play`; Save the copy for bit-exact audio. |
| Stream packet queues | `audio::Stream` | Save | Hold decoded frames; the guest buffer address is not kept, so they cannot be re-read. |
| Reverb tails | `Reverb` | Save or reset | A reset is a short audible glitch, not a guest-visible change. |
| Arena bookkeeping | `Hle::arena` | Save | Bytes in guest RAM; the allocator lists are host-side. |
| Event pins | `Hle::pins` | Save | Token to kernel handle; re-pin on load. |
| Callbacks, outbox | `Hle::callbacks`, `outbox` | Quiesce | Drain at the boundary. |
| WMA decoder sessions | `WmaHle::streams`, `wma/src/decoder.rs` | Save | Overlap buffers and partial packets cannot be derived from guest RAM. |
| Output grid (logical pacing) | `output.rs` `Grid` | Save | |
| Output device, PCM ring, resampler | `DeviceSink`, `sink.rs` | Rebuild | Output only. |
| Pump and output threads | `dsound-pump`, output thread | Quiesce | Device pacing runs on wall time: refuse unless logical pacing is on. |

### Features, boot glue and the rest

| State | Where | Decision | Why |
|---|---|---|---|
| Switches | `feature::Switches` | Check | Part of the build key. |
| Feature plan, hooks, arena | `feature::Registry`, `Plan`, `Installed` | Check | Deterministic from switches; arena bytes are guest RAM. |
| Boot glue statics holding guest addresses | `layout_glue`, `campaign_menu`, `list_class`, `stopgaps::KSTACK_NEXT` | Check | Fixed by boot order. |
| Boot glue statics set during play | `health::MAP_MODE`, `METER`; `skulls_custom::CAMPAIGN`; `level_boot::LOADED_AT`; `skulls_mp`/`skulls_custom` shared state | Save | Each feature owns a section. |
| Co-op | `coop_live::LOBBY`, `COOP_MODE_PENDING` | Quiesce | Refuse during co-op. |
| Live | `live-hle` sockets, client thread | Quiesce | Refuse while signed in. |
| Bink | `BinkHle` slots, platform decoder | Quiesce | Refuse while a movie plays. |
| Interpreter fallback caches | `boot/src/fallback.rs` | Rebuild | |
| Lockstep recorder counters | `lockstep::Recorder::ticks`, `presents` | Save | So a trace can continue across a load. |
| Agent session, timeline, perf, telemetry | | Ignore | Observers. |

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
   with fresh `ExecCtx`s carrying the saved CPU and stashed transfer.
5. VFS: close every open object, re-open the saved ones by path, seek.
6. Devices: D3D bindings and GPU-model state, surfaces; dsound objects,
   mixer, WMA sessions, re-pinned events.
7. XAPI handles, input tap cursor (resume replay, or continue recording),
   lockstep counters, feature sections.
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
2. **Quiescence probe.** A `Snapshot` capability for features
   (`feature::Feature` gains "stateless", "has a section", or "forbids
   snapshots while live", defaulting to forbid, so an unaudited feature
   fails closed). `Machine::quiescent()` reports every blocker in the table
   above. An opt-in, read-only probe (`H2_SNAPSHOT_PROBE`) logs, per tick,
   which blockers were live. Tests: synthetic machines with a thread
   blocked in a guest callback, a queued interrupt, a DPC. The user's one
   run with the probe on in a fight answers the biggest unknown.
3. **Kernel and machine.** Sections for `Core` and the machine's threads;
   VFS re-open; xapi handles; tap cursor; feature statics. Tests: the
   kernel crate's existing synthetic threads (`kernel/tests`): run a
   synthetic multi-threaded guest for N slices, snapshot, run M more and
   hash; load into a freshly booted machine, run M and compare hashes
   tick by tick.
4. **Devices.** D3D save, drop caches, restore; dsound, mixer, WMA. Tests
   over the D3D null backend and the audio crate's synthetic voices:
   render, snapshot, render more; load, render the same, compare draw
   streams and mixed PCM.
5. **Wiring.** `state.save`, `state.load`, `state.list`, `state.drop`,
   `state.fork` in `agent` and `agent-proto`, the save request in the tick
   observer and the save itself between slices in the boot loop. Tests:
   the agent crate's executor with a fake machine. Last: the user saves a
   fight in `h2 serve`, an agent loads it repeatedly and checks the
   lockstep hash and N replayed ticks.

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

## Related

- [agent-api.md](agent-api.md): the `state` methods and phases
- [checkpoints.md](checkpoints.md): the game's own checkpoint, the coarse
  fallback
- [high-framerate.md](../game/high-framerate.md): `lockstep` and
  `xapi::tap`
- [BOOT.md](BOOT.md): what the boot sets up, and in which order
