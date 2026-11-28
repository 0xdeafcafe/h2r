# Machine snapshots: the quiescence probe

Status: Built. Running the probe, rebuildable continuations, feature policies and the render features. Part of [Machine snapshots](snapshots.md).

## Quiescence probe

Code: `crates/runtime/kernel/src/quiesce.rs`, `crates/extensions/game/src/snapshot_probe.rs`;
tests: `crates/runtime/kernel/src/quiesce.rs` and `crates/runtime/kernel/tests/quiesce.rs`,
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
  or fixed at build (`engine-leaves`, `ui-strings`, `settings-rows`, and
  the Halo 1 effects `h1-flag`, `h1-glow`, `h1-lightning` and
  `h1-sound-promotion`, whose panels, random seeds and promotion credits
  come from guest memory and tags).
- **Forbid while live**: host state that is live only between hooks or while
  a screen is up (`cheats` while a cheat is on, the co-op features while
  armed, `h1-health` while a damage update is between its hooks,
  `live-map-pack-rotation` while a hopper block read is between its
  hooks, `render-timing` while a render stage is open).
- **Section**: host state a load puts back (`skulls`, `skulls-custom`,
  `skulls-mp`, `hud`, `h1-hud`, `lockstep`'s counters, `agent`'s view of the
  game, `loaded_map`, `aim-assist`, `game-variants`' listing of the store
  variants the guest's variant list names (not the store on disk, and not
  the sim's script block, which is the run's setting), and `sim-world`'s extension records (`ext`) with
  their checkpoint-slot copies, empty when there are none;
  [sim-rewrite.md](../engine/sim-rewrite.md), "The `SimExt` blob").

Open items for list classes and screens are under "Not verified".

### The render features

Boot registers the render features in every run, with their
`impl.render.*` switches off by default. Each declares its policy before
any early return for a switch that is off, so a run with all of them off
is not `Unaudited` and `state.save` is not refused. The other `native-*`
features (`crates/rewrites/render-bridge/src/native_*.rs`) are `Stateless`.

None of them changes simulation state. A save runs between slices, with
the D3D render thread drained. Section loads run last, on the
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
