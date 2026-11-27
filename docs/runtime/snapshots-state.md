# Machine snapshots: kernel, devices and wiring

Status: Built. The kernel and machine state, D3D and audio devices, and the `state.*` wiring. Part of [Machine snapshots](snapshots.md).

## Kernel and machine

Code: `crates/runtime/kernel/src/snapshot.rs`; tests: `crates/runtime/kernel/tests/snapshot.rs`.

- `Machine::save_snapshot` requires a quiescent machine. It writes the
  sections `kcore` (version 4) and `machine` on the snapshot crate's
  codec, and `load_snapshot` reads them. The `Persist` trait and
  `persist_struct!` destructure every saved struct without `..`, and
  `save_core` destructures `Core` whole, so a new field does not compile
  until it has a decision. Mounted and storage-free games use the same
  format. The host checks for editor copies and previews before modifying
  the machine. Editor observers add no feature section or build-key entry.
  Recording and playback use the ordinary agent hooks.
- Saved: every table in the [inventory](snapshots.md#kernel), the clock with its
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
  return is built in ([Rebuildable continuations](snapshots.md#rebuildable-continuations)).
  `runtime::ContinuationLayout` (`ExecCtx::continuation_layout`,
  `restore_continuations`) holds the pool as plain data with every slot's
  generation and the reuse order, so tokens made after a load are the
  saved run's. A live closure that is not rebuildable refuses the save; a
  load with no rebuilder for an origin changes nothing.
- `vfs::Vfs::state` and `restore_state` (`crates/runtime/vfs/src/state.rs`):
  open objects by device and path with handle numbers, options, position,
  search cursor (directory entries are read again at load) and changed
  flag. A file object that can write, or wrote, carries the file's bytes
  (`SavedFile::contents`, up to `MAX_CARRIED_BYTES`). Refuses an object
  with delete access or delete-on-close, and a pending delete. A load
  looks every file up before it closes anything; the apply closes the
  open objects, writes each carried file back at its saved length
  (creating it if it is gone, leaving it if it holds them already) and
  re-opens the objects. A host write that fails there is logged and the
  object is re-opened on what the file holds. Tests:
  `crates/runtime/vfs/tests/state.rs`.
- Larger writable files (`vfs/src/journal.rs`): a map cache slot is
  180 MB and open for writing all the time the game plays, though the
  game writes it only while it copies a map in. The save records it as a
  `SavedFile::reference` (`vfs::FileReference`): the capturing VFS's
  identity, the file's write generation, its length and a 64-bit content
  fingerprint. The fingerprint is read once per generation and cached, so
  a capture of an unchanged slot reads nothing. From the first capture
  on, every change the VFS makes to the file (`Vfs::write`,
  `set_end_of_file`, an overwriting open, a delete at close) takes a fresh
  generation and first saves the bytes it overwrites and the old length
  (copy-before-write). A restore in the same VFS undoes those changes
  newest first back to the captured generation, and costs a `stat` when
  nothing was written. The undo's own changes are journaled too, so a
  later capture stays reachable after a restore to an earlier one. The
  journal keeps at most `vfs::MAX_JOURNAL_BYTES` (32 MiB) across files; a
  change past that drops the file's journal. A restore that the journal
  no longer reaches, or into another VFS, accepts the file only if its
  length and fingerprint match, and otherwise refuses with
  `StateError::Unjournaled` before anything changes. A map copied into
  the slot after a capture therefore refuses a restore to it, unless the
  slot holds the captured map again. Formatting or seeding the volume
  forgets its journals.
- `xapi::snapshot` (`crates/platform/xapi/src/snapshot.rs`): the `xapi` section.
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

Code: `crates/platform/d3d/src/device/snap.rs`; tests: `crates/platform/d3d/tests/snapshot.rs`.

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

Code: `crates/platform/audio/src/snap.rs`, `crates/platform/dsound/src/state/snap.rs`,
`crates/platform/dsound/src/register/snap.rs`, `crates/platform/dsound/src/output.rs`,
`crates/platform/dsound/src/xwma.rs`, `crates/platform/wma` (`Session::save_state`). Tests:
`crates/platform/audio/tests/snapshot.rs`, `crates/platform/dsound/tests/snapshot.rs`,
`crates/platform/dsound/tests/xwma.rs`, `crates/platform/wma/src/boundary.rs`.

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

Code: `crates/extensions/agent-proto/src/state.rs`, `crates/extensions/agent/src/exec/snap.rs`,
`crates/extensions/game/src/snapshot_glue.rs`. Tests: `crates/extensions/agent/src/exec/snap/tests.rs`
(a save waits for a tick; retries per tick then `BUSY`; load, fork, drop;
name checks; a paused game; exit) and `crates/extensions/game/src/snapshot_glue/tests.rs`
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
