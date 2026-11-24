# 60 and 120 fps

Status: Partly built. The setting, the determinism harness and the frame owner's display frames between two scenes (camera blend) are built and tested with fakes; the display frames have not been run in the game. Object blending on display frames, 120 measured in game and variable refresh are design.
Code: `crates/extensions/lockstep`, `crates/platform/xapi/src/tap.rs`, `crates/apps/app/src/fps.rs`, `crates/rewrites/frame-scene/src/display.rs`, `crates/rewrites/native-render/src/frame_graph/display_rate.rs`, `crates/extensions/game/src/d3d_glue.rs`, `crates/extensions/game/src/lockstep_glue.rs`. Tests: sections 6, 7 and 9.
Switch: `H2_FPS`, `frame-scene.sink`, `H2_LOCKSTEP*`, `H2_INPUT_RECORD`, `H2_INPUT_REPLAY`, listed in [switches.md](../runtime/switches.md).

The rule: **the guest tick stays 30 Hz at every display rate.** Super bounces are a physics and collision quirk of the 30 Hz tick, so the simulation stays bit-identical to 30 Hz play and only what is drawn runs faster. Every community attempt that raised the tick lost super bounces; the ones that kept a 30 Hz tick and interpolated the picture kept them ([notes/high-framerate.md](notes/high-framerate.md) has the record). The planned home of 60 and 120 is the frame-scene renderer ([framescene.md](../graphics/framescene.md)); the rule carries over to it.

In short:

- **The guest is untouched.** Same 60 Hz logical vblanks, same two-vblank frame interval, same one tick per frame, same input polls. No guest code, guest global or guest timing changes.
- **The in-between frames are made on the host.** The native renderer keeps the last two sealed scenes and draws display frames between them with each view's camera blended between the two ticks (section 7). The guest never sees these frames.
- **Host world marks follow the made frame's camera.** The window's marks (editor marks, the superbounce figures) project with the camera each in-between frame drew the world with, and animate by the window's clock (section 9).
- **The proof is the guest's own state.** The same input recording gives the same guest-memory hash at every present and every tick with the mode off, at 60 and at 120 (section 6).

The mode only helps once a level holds a steady 30 fps ([gameplay-budget.md](../perf/notes/gameplay-budget.md)); below that the pacer rebases and the game plays in slow motion.

## 1. How the tick, the frame and the vblank are coupled

### In the guest

Addresses and names come from [engine-map.md](../engine/engine-map.md) ("Startup and
the main loop", "Frame phases"), [d3d.md](../graphics/d3d.md) ("Vertical blank") and a
read of the disassembly (E). Anything marked "read" below is from that
disassembly and is not checked in a run.

One main-loop iteration, `0x0012b450`, called repeatedly by `0x0012b690`,
does one rendered frame:

| order | what | anchor |
|---|---|---|
| 1 | map and state transitions | `0x0018f1c0`, `0x0018e810` |
| 2 | input poll (XInputGetState, once per iteration) | `0x00124950`, then `0x00217c40` |
| 3 | frame timing: vblanks since the last frame become `dt` | `0x0012b0e0` |
| 4 | network receive | `0x0008df50` |
| 5 | tick budget: `dt` becomes a whole number of ticks | `0x00146660` |
| 6 | player-control preparation | `0x001870a0` |
| 7 | run the budgeted ticks (zero, one or more) | `0x00137f70` calling `game_tick` `0x00137fe0` |
| 8 | network send | `0x0008dfc0` |
| 9 | per-frame, variable-`dt` work: director (camera), observer | `0x00137ed0`, `0x001554b0`, `0x0016f280` |
| 10 | controller feedback (rumble) | `0x00222a70` |
| 11 | render and present | `0x00223240` -> views `0x002234e0` / `0x0002b5d0` -> `0x00013f10` -> Swap |
| 12 | sound service | `0x00125b40` |

Inside a tick (`0x00137fe0`): players before `0x0014d3a0`, AI `0x001c83c0`,
scripts `0x00209310`, objects before physics `0x000b6f10`, Havok
`0x001c3d30` (into `0x00311100`, with the tick's `dt`), objects after
physics `0x000b7060`, object finalisation `0x000b7150`, players after
`0x0014d8b0`. The tick count is `game_time + 8`.

**Game time** is a 0x24-byte record whose pointer is at `0x00510c54`
(`game_time_initialize` `0x00146550`; `game_time_new_map` `0x001465b0`
copies the tick rate from the game options). `+2` is the tick rate, `+4` the
tick length, `+8` the tick count, `+0xc` the game speed. Read: `+0x10` is
the **leftover fraction of a tick** the budget carries from frame to frame,
and `+0x14..+0x20` drive a speed ramp. So `0x00146660` turns
`rate * speed * dt + leftover` into whole ticks, keeps the fraction, caps the
count per frame and reports the simulated time.

**Frame time.** Read: `0x0012b0e0` keeps a 64-bit "frame target" vblank
count at `0x004e6398`, set to the previous target plus the frame interval.
The interval comes from a word at `0x00485aca`, floored at 2 under one
game-mode condition. `dt` is the vblanks since the last target divided by the
refresh word at `0x00485ac0` (60 when unset), clamped. d3d.md already says
this `dt` feeds the tick budget, so physics and script sleeps follow vblanks.

**Vblank and the frame wait.** The GPU interrupt calls Halo's callback
`0x0012b2a0`, which bumps the 64-bit counter at `0x00485ab0` and records
swaps at `0x00485ab8`/`0x0055e6b8`. The frame wait `0x0012b2e0` spins until
the counter reaches the frame target minus one. With the interval at 2 and a
60 Hz vblank, a frame is 1/30 s, `dt` is exactly one tick, the leftover stays
at zero and every frame runs exactly one tick.

Three things to take from this:

- **The engine already knows about fractional frames.** The tick rate is a
  game option, scripts scale their sleeps by it (`sleep` at `0x002087e0`,
  [engine/hs_vm.md](../engine/hs_vm.md)), and the budget carries a leftover
  fraction. That is what Halo 2 Vista and MCC changed to run a 60 Hz tick,
  and it is what this design must not touch.
- **Several things happen once per frame, not once per tick:** the input
  poll, frame timing, director and observer, rumble, render and sound. Any
  approach that changes how many main-loop iterations run per tick changes
  how often those run and when input is sampled relative to the tick.
- **A guest-native 60 fps exists in the data** (the interval word). Setting
  it to 1 would run the main loop at 60 Hz with every other frame running no
  tick. It is tempting and cheap, and it is rejected in section 3: it changes the
  input-poll and per-frame cadence, it would re-render identical state unless
  the renderer reads the leftover fraction (unknown; an xref search for
  readers of `game_time + 0x10` would answer it), and it doubles the guest's
  render cost.

### In our runtime

- **The logical clock** (`kernel::Clock`, crates/runtime/kernel/README.md "Time and
  work") counts 100 ns ticks. Generated code charges one ns per guest memory
  access. The clock jumps to the next deadline when every thread waits. Rust
  overrides and DPCs are uncharged. KeTickCount, the performance counter,
  the TSC, timers and vblanks all read this one clock, so a run's schedule
  depends only on the guest's work and its inputs, never on wall time.
- **Vblanks** come from a kernel periodic source at `Rate::HZ_60`
  (`crates/apps/boot/src/lib.rs`, `d3d::vblank::clocked_vblanks`) on that clock.
  The frame-wait override `FrameWait::HALO2_CLOCKED` yields until the target,
  so the clock jumps straight to the vblank (d3d.md). A level frame is
  33.3 ms of logical time and 2 vblanks ([gameplay-budget.md](../perf/notes/gameplay-budget.md)).
- **Wall time is outside.** `app::pacing::Pacer` and `kernel::pacer::Pacer`
  only sleep the host so guest time doesn't run ahead of real time. They
  use the kernel counter in 100 ns units. In windowed play the shell checks
  it after each slice and each audio-only idle advance, including at
  Uncapped. A slow host can fall below 30 ticks per wall second.
  `h2 play --unpaced` explicitly removes the wall-clock limit for profiling.
- **Presentation never blocks the guest.** Swap goes through the present
  hook into a 3-slot mailbox; the window shows the newest frame
  ([app.md](../runtime/app.md), "Presentation"). The render thread is the
  default (`H2_RENDER_THREAD=0` turns it off): every `d3d::Backend` call
  runs on it, after the device has resolved the draw on the guest's thread
  ([render-thread.md](../graphics/render-thread.md)). The guest still feels
  the render thread on wall time: the frame-in-flight limit at `Swap`, the
  `D3DSWAP_FINISH` GPU wait, and reads that drain the queue (write-backs,
  pixel counts that are not back). None of those changes the logical
  schedule.

That last pair is what makes this design possible: anything done after the
present hook, on host surfaces, can't reach the guest's schedule or memory.

## 2. Prior art

Dated research on what Halo 2 Vista, MCC, Cartographer, Chimera, Halo CE PC, the Xbox patches and general technique did is in [notes/high-framerate.md](notes/high-framerate.md).

What the record says: every approach that raised the tick (Vista, MCC, the CE experiment) lost super bounces or broke timers, AI, weapons and animations, and needed per-constant rescaling it never finished. Every approach that kept 30 Hz and interpolated the picture (Cartographer campaign, Chimera, ChupathingyCE) kept the game as authored. Their remaining bugs were particles, contrails, camera shake, HUD and motion-sensor fades, the first-person weapon and camera, cutscene animation, teleports and effects spawned mid-tick. Most came from running per-frame guest code more often, which this design avoids by not running it more often.

## 3. Recommended design

### Options

| option | simulation | super bounces | guest cost | quality | verdict |
|---|---|---|---|---|---|
| **A. Faster tick** (60 or 120 Hz, the Vista/MCC way) | changes | lost (Vista, MCC) | 2-4x | native | rejected |
| **B. Guest frame interval 1** (the `0x00485aca` word) | per-tick code unchanged, but input polls and per-frame code run twice per tick | at risk: input-to-tick phase changes | ~2x render | duplicate frames unless the renderer interpolates; caps at 60 | rejected |
| **C. Guest render sub-frames** (Cartographer: write interpolated transforms into guest memory, run the guest renderer, restore) | unchanged if every side effect is undone | kept, if isolation holds | +render cost per presented frame | best: the game culls, sorts and spawns effects at the interpolated camera | fallback |
| **D. Host replay with interpolated matrices** (this design) | unchanged by construction | kept | no guest instructions; replays share the render thread and GPU, so the guest waits more at the frame-in-flight limit, the `D3DSWAP_FINISH` wait and read-back drains when the render thread is the slower side | camera and object motion smooth; particles move at 30 Hz | **recommended** |
| E. Image-space frame generation (reprojection with depth and motion) | unchanged | kept | as D, less render work | disocclusion smears, HUD and transparency artefacts | not worth it in general |
| E1. Rotation-only reprojection of frame N's world colour | unchanged | kept | as D, one full-screen pass per display frame | camera turns smooth (most of what players see at 30), translation and objects step at 30 Hz; needs overscan for the edges | not built (section 8) |
| F. Extrapolation (render ahead of the newest tick) | unchanged | kept, but drawn wrong | as D | no added latency, but overshoots on every sudden change: landings, collisions, the bounce itself | camera aim only |

**E1, the cheap comparison.** Capture frame N's world colour before the
first-person weapon and HUD passes (the pass tag below says where), render
the view with some overscan, and at each display frame draw that image
rotated by the camera's rotation between the tick and the in-between
camera (a homography, no depth needed for a pure rotation). The weapon and
HUD go on top unchanged. Its cost is one full-screen pass per display
frame, independent of the draw count, so it fits 60 and 120 where D's
geometry replay (below) does not yet. It does nothing for strafing or
moving objects. D is what is built, on the native renderer's scenes
(section 7); E1 is not (section 8).

C is what a mature PC mod does, and it renders more correctly than D. It
loses here for two reasons. Guest CPU is our bottleneck: a level frame is
mostly guest-side ([gameplay-budget.md](../perf/notes/gameplay-budget.md)), and C pays the guest's render path once per presented
frame. And proving C safe means proving the guest's renderer has no side
effect that survives a restore: object render caches, LOD and visibility
state, the frame counter in `0x00013f10`, resource-cache countdowns that
gate Swap, the RNG if effects draw from it, D3D's device model (RTT dirty
bits, depth generations, fences), and the logical clock, which charges every
guest access. D has none of that to prove: the guest runs the same
instructions on the same schedule whether it is on or off.

### Design D: host replay

#### Frames and timing

- Each guest present already hands the backend a frame: the ordered
  `ResolvedDraw`s with their streams, state and shader constants, plus
  clears, copies and uploads. `H2_BOOT_CAPTURE` already records exactly this
  for offline replay (`crates/extensions/game/src/d3d_diag.rs`), so replaying a frame
  is not new.
- The presenter keeps two frames, N-1 and N, each stamped on the game's
  thread at the `Swap` entry and carried with the present command. Not in
  the posted present closure: by the time the render thread runs it, the
  guest may already be in tick N+1, so a `game_time` read there races.
  A tick count, if one is wanted, must be read at the same point.
- For a display frame due at wall time `t`, use the measured arrival
  times, not the pacer's origin (a rebase moves the origin):
  `alpha = (t - T(N)) / (T(N) - T(N-1))`, clamped to [0, 1], which is one
  frame of delay. A guest frame that ran no tick (pause, load) has nothing
  to interpolate. One that ran two (catch-up) interpolates over its longer
  gap. Frames further apart than a guest running below 30 is allowed
  (section 7: 45 ms) are not blended: the newest is held as drawn, and the
  history restarts.
- If guest frame N+1 is late, hold at `alpha = 1`. Never extrapolate
  objects.
- The extra frames are drawn into host-only surfaces. They never touch a
  guest-visible render target, never write back to guest memory and never
  change `Device` state.

#### What gets interpolated

Interpolation is a substitution of vertex-shader constants in the replayed
draws. Constants arrive through the setters at `0x003f71d0..0x003f7410` and
the push buffer (d3d.md, inventory and "Vertex-shader constants"), so the
device sees every one.

1. **Camera** (built, section 7). Read each view's camera from the view records that
   `render_game_views` `0x002234e0` builds, by observing, not writing. Find
   which constants hold that view's view and projection by rebuilding them on
   the host and checking they match the uploaded words exactly. Interpolate
   position linearly, orientation by quaternion slerp, and field of view
   linearly. Rebuild the constants and substitute them in every draw of that
   view. World-space data (BSP, decals, particles, contrails) then renders
   correctly from the in-between camera. Particles only move on their own at
   30 Hz.
2. **Objects** (built by draw signature, section 7; tagging by object is
   design). Tag each draw with the object being drawn by
   observing the guest's per-object render entry (still to be found; the
   object render cache at `0x0003d270` is the place to start). For each
   matrix constant: if it holds view-projection times world, recover the
   world part with the tick's view-projection inverse. Match it to the same
   object and node in frame N-1, interpolate translation, rotation and scale
   separately, and rebuild the constant with the in-between camera. Skinned
   draws do this per node. A hypothesis worth checking first: if the game
   passes the setter a pointer into an object's node-matrix block, the guest
   address alone identifies the object and node.
3. **First-person weapon and hands** (built). They are drawn with their own
   projection. Interpolate their nodes in camera space, so
   they stay locked to the in-between camera instead of swimming.
4. **Shadows** (design). Shadow-map passes are render-to-texture passes
   with the light's matrices. Replay them into private copies with the
   objects' interpolated worlds. Until then, shadows of moving objects step
   at 30 Hz.

#### Rules that keep it safe

- **Only matched, verified draws are interpolated.** A draw is eligible
  when its constants were rebuilt bit-exactly from observed state at frame N
  and its object exists in both frames. Anything else is replayed as frame N
  drew it. The design fails towards "30 fps-looking", never towards wrong.
- **Snap instead of blend** on a camera cut (scripted cinematic cuts,
  respawn, map or BSP change, checkpoint revert): detect a cut with a
  position and angle threshold, and better, by observing the scripted-camera
  path (`0x0016c840`). Snap an object that moves too far in one tick, and one
  created or deleted between the two ticks. Cartographer uses the same
  guard.
- **Screen-space work replays unchanged:** HUD, menus, motion sensor, text,
  fades and full-screen passes. They step at 30 Hz, as authored.
- **Guest readbacks stay on real frames.** Depth aliases, pixel counts (lens
  flares, visibility) and RTT write-backs happen once per guest frame.

#### Care list

| thing | at 30 Hz with D | fix, if wanted |
|---|---|---|
| particles, contrails, beams (vertices built on the CPU) | world-anchored, so correct under camera motion; their own motion steps at 30 Hz; billboards face the tick camera | match particle draws across frames and interpolate positions (later, optional) |
| LOD and visibility culling | chosen for tick N's camera; things just off-screen can pop at the edges for a tick during fast turns | accept; or C for those draws |
| render-to-texture effects (water, active camo, screen effects) | sample frame N's textures, up to a tick old | accept |
| animations authored at 30 Hz | node interpolation smooths them; deliberate pops get blended (MCC's note) | snap per object on large node jumps |
| HUD, motion sensor, timers, text | 30 Hz | accept: they are 30 Hz on the Xbox |
| world-anchored HUD markers (waypoints, nav points, name tags) | screen positions computed by the guest for tick N, so they lag the in-between camera by up to a tick and slide against the world | re-project them from their world position (needs the guest's marker records), or accept |
| camera shake and screen effects | 30 Hz | accept |
| projected lights and the flashlight | their projection matrices are constants of their own; the lit area steps at 30 Hz while the geometry under it moves smoothly | classify projector matrices like cameras and blend them |
| water reflections and refractions | the reflection texture is rendered for tick N's camera; under the in-between camera the reflection slides against the surface by up to a tick | accept; or replay the reflection pass with the reflected in-between camera |
| muzzle flashes and effects attached to objects | particle vertices are built on the CPU in world space for tick N, so they trail a blended gun or vehicle by up to a tick | match particle systems to their parent object (phase 4) |
| split-screen | every view replays; cost scales with the views, and a layout change must restart the blend | first slice: a single gameplay view; a view-set change resets the history |
| cutscene cuts | must snap | cut detection above |
| sound, rumble | unchanged: the guest runs them per guest frame | nothing to do |

#### Input latency and aim

Interpolation adds about one tick of latency (up to 33 ms, half that on
average). Aim is where players feel it. From an input poll to light on the
screen, at a steady 30 fps guest (S: estimates from the code paths; a
measurement would verify them):

| stage | 30 | 60 with the replay |
|---|---|---|
| poll to the tick that uses it | same frame: the poll is at the top of the main loop | same |
| tick to `Swap` (the guest renders) | the rest of the guest frame, up to 33 ms | same |
| `Swap` to the render thread finishing the frame | 0 when it keeps up; up to 2 frames when it is the slower side (`MAX_FRAMES_IN_FLIGHT`) | same queue, plus the replays' share of it |
| the frame becoming visible | the window redraws on the publish, then the display's vsync (0-16.7 ms at 60 Hz) | the first in-between frame is drawn at the frame's arrival (alpha 0, frame N-1's pose), then on the replay's grid (60 or 120 Hz), each drawn by the window at its publish |
| the new tick fully shown | at once | one frame interval later (alpha 1): +33 ms; movement towards it starts at the next display frame, +0-16.7 ms |

Two answers, in order:

- Ship without it. The 30 Hz Xbox already has this latency for a
  rendered tick.
- Then optionally add **visual aim prediction**. Rotate the render camera by
  the yaw and pitch the next tick will apply, computed from the live stick
  or mouse with the game's own look-rate rule. That needs reading how
  `0x001870a0` turns the stick into a turn rate, including any acceleration.
  It's a rotation of the drawn camera only. The guest still reads the same
  input at the same poll, and the next tick's real aim replaces the
  prediction. A mismatch shows as a small correction, never as a simulation
  change. Mouse look already becomes stick deflection per 1/60 s
  ([input.md](../input/input.md)), which keeps the rule a function of input alone.

#### Render thread, frame budget, VRR

- **Where it runs.** The replay belongs on the render thread (`d3d::ThreadedBackend`),
  after the backend has recorded the guest's frame. The guest's frame is encoded as before. Replays reuse the recorded passes and pipelines and
  change only uniforms: record the frame once into render bundles (or
  equivalent pass lists) whose uniform buffers are rewritten for each
  replayed frame. The render thread is required, not optional: replays on
  the guest's thread would put host GPU work back on the critical path that
  render-thread.md took it off.
- **Budget.** At 60 Hz a replay gets 16.7 ms, at 120 8.3 ms, and the
  render thread also has the guest's own frame to encode. The encode
  and submit cost is measured in [gameplay-budget.md](../perf/notes/gameplay-budget.md)
  ("Buffer count and per-pass bookkeeping"; figures in
  [notes/high-framerate.md](notes/high-framerate.md), "Encode budget"). A replay that
  re-encodes the frame's back-buffer draws costs the same per draw, so it
  does not fit twice per tick at those counts. Replays must not re-encode
  from scratch: per replay, a few hundred uniform updates and one bundle
  execution per pass. That target needs measuring before 120 is promised;
  60 is the only rate built.
- **Precondition.** The guest must hold 30 Hz first. Below that the pacer
  rebases and the game plays in slow motion; interpolating slow motion helps
  nothing. If the guest misses a frame, the presenter holds at alpha 1 for
  that span rather than guessing.
- **Present modes.** The window uses `AutoVsync` (`crates/apps/app/src/surface.rs`). For a fixed
  60 or 120 Hz display: render one replay per vsync, with alpha computed for
  the predicted display time, not the submit time. "60 fps" on a 120 Hz
  panel means a replay every other vsync. For **VRR** (ProMotion,
  FreeSync, G-Sync): present as soon as each replay is ready, capped by the
  setting, with alpha for the time it is predicted to appear. wgpu doesn't
  report presentation timestamps, so predict from the last frame
  intervals; Metal's display-link timing is the better source on macOS.
  Jitter in that prediction shows as judder, so it is the thing to measure.
- **Settings.** The toggle is the Video screen's Frame Rate row
  (`graphics.fps`, [settings-menu.md](settings-menu.md) "Video screen"), also
  `H2_FPS` / `h2 play --fps` (section 6). 30 means off: no display
  frames between scenes (section 7). There is no switch.
- **Pass tags.** A pass tag is what this design wants: which draws are the
  world, which the first-person weapon, which the HUD. The sealed scene
  gives it: each draw has its view and slot, and a carried draw whose
  view rows are the view camera's is marked `CameraUse::View`
  ([framescene.md](../graphics/framescene.md), section 7).

## 4. Super bouncing

### What is known

From Halopedia (https://www.halopedia.org/Super_bouncing, its talk page
https://www.halopedia.org/Talk:Super_Bouncing) and the Halo Stunting wiki
(https://halostunting.fandom.com/wiki/Super_Bounce):

- A crouch glitch keeps the player crouched (with a smaller collision shape)
  after they try to stand. Jumping onto a seam between collision polygons
  then lets the player sink into the geometry, and the collision response
  pushes them out with a very large velocity.
- Known spots include Lockout, Ascension, Burial Mounds, Coagulation,
  Sanctuary, Turf, Warlock, Zanzibar and Relic.
- Bounces differ between NTSC/PAL60 and PAL50 consoles; one Zanzibar bounce
  works only on NTSC/PAL60.
- The editors themselves call the explanation speculative. "Havok
  overcorrecting at gaps" (https://www.giantbomb.com/super-bouncing/3015-401/)
  is not verified.
- PC "removed" them. MCCBounceEnable and the Cartographer post both say a
  30 Hz tick brings them back, and there is video of a 30-tick Vista build
  bouncing (https://www.youtube.com/watch?v=NEu_A6bVu3Y).

Our own map of the engine fits that: the collision BSP is the 0x44-byte
element of a collision model ([collision.md](../formats/collision.md)), the Havok BSP
integration is `0x001c2910`, and the step `0x001c3d30` runs once per tick
with the tick's `dt`.

### Why tick-exact plus host replay keeps it

A bounce is a function of the per-tick sequence: the position and
velocity entering each step, the step length, the seam the shape lands on,
the crouch state, and which tick each input arrives on. A faster tick
changes the step length and so how deep the shape gets into the seam in one
step and how hard it is pushed out. That is the community's account of why
60 Hz lost them. The PAL50 difference points at a second dependency. At
25 frames a second against a 30 Hz tick, frames run one or two ticks
unevenly, so the phase between input polls and ticks changes. (That is our
inference; no source says so.) Option B would change that phase too.

Design D changes neither. The guest runs every instruction it runs, on
the same logical schedule, and polls input on the same frames. So the
bounce, the positions it passes through and its timing are bit-identical.
The test below checks exactly that, instead of trusting the argument.

### Regression test

**Input recording keyed to ticks (built).** `xapi::tap` logs every XInput
packet the guest actually read, with the tick count at the read (section 6).
A human does a bounce in `h2 play` with recording on; the log replays
deterministically, because the logical clock is the same with the same
inputs. The recording is ours (button and stick values over time), not game
data, so it can be committed. The expected game-state values are derived
from the disc and are never committed: the test compares runs with each
other.

**The test** (not built; an ignored retail acceptance test, like
[checkpoints.md](../runtime/checkpoints.md)), headless, through
`GuestHost::headless`:

1. Load the map. Start the recording at its epoch.
2. Run with the mode off. At the end of every `game_tick` (the lockstep
   observer on `0x00137fe0`), record a hash of guest RAM and the local
   player's biped position and velocity.
3. Assert the bounce happened: the player's height over the bounce window
   exceeds a threshold given in the test (a fraction of the bounce height,
   not a retail value).
4. Run again with presentation at 60, then at 120, with a deterministic fake
   display clock driving the presenter (so the host side does real replays
   on every run). Assert that the per-tick hash sequence and the per-present
   frame PNGs of real frames match run 2 exactly.
5. Control: run 2 twice and require identical hashes. That catches
   nondeterminism in the runtime itself before it is blamed on the mode.

Optional negative control, to prove the test can see a lost bounce: force
the tick rate in the game options to 60 for one run, and require step 3 to
fail.

**Where to run it.** Halopedia's spots are multiplayer maps, so the bounce
test needs a local multiplayer game on Lockout (or a campaign bounce someone
finds). Step 4 does not need a bounce: on the scripted run into The Heretic
(the input string in [gameplay-budget.md](../perf/notes/gameplay-budget.md)) it
proves "mode on changes nothing the guest can see" over 300 level frames.

**Cheaper unit tests** (no game): alpha arithmetic and clamping; snap
thresholds; constant reconstruction against synthetic camera and object
records; replay into host-only surfaces leaving `Device` state unchanged
(compare the device model before and after a replay); the presenter's API
taking only owned or shared snapshots, so it can't write guest memory by
construction.

## 5. Effort, risks and phases

Rough sizes, for one person who knows the d3d and gfx crates.

| phase | work | size | exit test |
|---|---|---|---|
| 0 | **Prerequisite, not part of this:** levels at a steady 30 fps; render thread on by default | ([gameplay-budget.md](../perf/notes/gameplay-budget.md)) | worst frame under 33 ms |
| 1 | **Built** (section 6): determinism harness: per-tick guest-RAM hash, tick-keyed input recorder, run comparison, fake display clock | 1-2 weeks | two mode-off runs match; Heretic run matches with a replay that draws frame N unchanged |
| 2 | **Partly built** (sections 7 and 8). Camera interpolation: two-frame store, alpha from measured arrivals, view constants rebuilt and verified, cut detection, replay on the render thread, pacing at 60 and 120 with a headroom fallback, the setting (built); E1 for comparison (built); then bundles, VRR | 2-4 weeks | identical hashes with the replay on and off; replay cost under 16 ms at 60 (8 ms for 120) |
| 3 | Objects (matched by signature in section 7; the rest is design): draw tagging by object, world recovery, node and skinning interpolation, first-person weapon, teleport snaps, shadows | 3-6 weeks | same; visual check in a level with moving actors |
| 4 | Polish: particle and contrail matching (optional), visual aim prediction, cutscene checks | 2-3 weeks | same |
| 5 | Super-bounce regression test, once a bounce map is reachable | 1 week plus the recording | section 4's test |

Risks, most likely first:

1. **Constant classification.** If matrices reach constants in forms that are
   hard to recover (pre-multiplied with extra terms, packed, or built in the
   vertex program from other data), the verified-rebuild rule leaves those
   draws at 30 Hz. It degrades safely, but the result may look patchy until
   the guest's render paths are mapped. Phase 2's camera pass is the cheap
   probe: if view constants can't be matched, rethink before phase 3.
2. **Replay cost.** Per-replay cost has to be a fraction of the guest frame's encode.
   If bundles or reused passes don't get there on Metal, 120 slips to 60.
3. **Prerequisite slippage.** All of this waits on the 30 fps work.
4. **Display timing.** Without presentation timestamps from wgpu, alpha
   jitter may be visible on VRR. Measure early.
5. **Fidelity versus C.** Edge popping, particle self-motion at 30 Hz and
   stale RTT effects may be judged not good enough. Option C is the
   fallback, with its isolation proof as the cost. The phase 1 harness is
   exactly what C would need too, so it is not wasted either way.
6. **The tempting shortcut.** Option B (the interval word) or a faster tick
   would demo sooner. Both change the guest, and section 2 says what
   the record shows.

What must not change in any phase: the vblank source rate, the refresh word,
the frame-interval word, the tick rate, the number of main-loop iterations
per tick, the input poll cadence, and anything the guest can read. Every
phase is gated on the per-tick hash test from phase 1.

## 6. Phase 1: the harness and the setting

Status: Built. Code: `crates/extensions/lockstep`, `crates/platform/xapi/src/tap.rs`, `crates/apps/app/src/fps.rs`, `crates/extensions/game/src/lockstep_glue.rs`.

### The harness

- **`crates/extensions/lockstep`.** `hash_guest` hashes the physical RAM that is in use (pages mapped by any virtual range or held by a reservation), one word per MiB plus one over the lot. It reads RAM from Rust through `Guest::physical_ram`, so it is uncharged and writes nothing. `Recorder` collects `Record`s (kind tick or present, sequence, logical clock, the guest's tick count from `game_time + 8`, the hash) into a `Trace`, which has a text form (`# h2 lockstep trace v1`, one line per record). `compare` names the first differing record and the MiB of RAM that differ, or says one run is a prefix of the other.
- **Tick observer.** `install_tick_observer` post-hooks the tick function (`TickHook::HALO2`: `game_tick` and the game-time pointer from `engine::points`), the same way Live telemetry wraps it. The original runs unchanged; the hash is taken when it returns. The hook adds dispatch transfers and holds a continuation token in the return slot during the tick, so **compare runs that all have it on**. It refuses a tick that something else already overrides (telemetry, when Live is on). Present records are taken between scheduler slices and change nothing in the run.
- **Input, keyed by read.** `xapi::tap` puts an `InputTap` on every `XInputGetState`: `Recorder` logs the status and the 22 packet bytes the guest got (stamped with the tick count), `Replayer` answers read *n* with the log's read *n*. Keying by read rather than by time makes replay exact whenever the host pumped input. The log (`# h2 input log v1`) is button and stick values, not game data. Only `XInputGetState` is tapped, so record and replay use the same kind of always-connected pad.
- **Fake display clock.** `app::fps::FakeDisplayClock` steps a fixed amount per read, so a presenter "at 120" is reproducible in tests.

### Using it on a headless run

```sh
H2_INPUT_RECORD=$TMPDIR/in.log H2_LOCKSTEP=$TMPDIR/a.trace h2 run <disc> ...
H2_INPUT_REPLAY=$TMPDIR/in.log H2_LOCKSTEP=$TMPDIR/b.trace h2 run <disc> ...
h2 lockstep $TMPDIR/a.trace $TMPDIR/b.trace   # exit 0 identical, 1 differ
```

`H2_LOCKSTEP_TICKS=0` keeps only the present records. Record and replay are exclusive. The wiring is `crates/extensions/game/src/lockstep_glue.rs`; the report prints one `lockstep:` line (records written, reads replayed, reads past the end, handle mismatches). `h2 play` takes the same variables.

### The setting

`H2_FPS=30|60|120|uncapped`, or `h2 play --fps`, sets `AppConfig::fps` (`app::fps::FpsTarget`). With neither, the Video screen's Frame Rate row decides (saved as `[graphics] fps` in `<data>/settings.toml`; [settings-menu.md](settings-menu.md), "Video screen"). A press there applies at the window's next turn: `app::fps::FpsControl` carries it, the `DisplaySchedule` is retargeted and, to or from uncapped, the surface's vsync is configured again (`app::fps::follow`). The window and any enabled render-thread replay read the setting.

| setting | window |
|---|---|
| `30` (default) | a redraw per guest present; the code path is the plain one |
| `60`, `120` | a redraw per publish (guest frames and in-between frames alike), plus a repeat of the newest picture from its `DisplaySchedule` whenever no frame was published for one interval at that rate, with `vsync` as set |
| `uncapped` | redraws every event-loop turn, vsync off |

Between guest presents a redraw shows the newest guest frame again, so **above 30 nothing looks smoother** unless a paced frame owner draws display frames between scenes (section 7), which it publishes for those redraws to show. `app::fps::alpha` is the same fraction on the window's side, tested but not used; the render thread computes its own (`frame-scene::pace::alpha`) from the scenes' publish stamps.

### Tests

- `cargo test -p lockstep`: the hash sees any changed word and a swap of two, names the changed MiB; traces round-trip through text; compare finds the first divergence and truncation; the clock counts as guest state. On the toy guest (`lockstep::synthetic`: a generated tick handler, a game-time record, an `XINPUT_STATE` input and a crouched-landing-on-a-seam "super bounce"): equal inputs give equal traces; a replay reproduces the recording whatever the live pad does; releasing crouch on the landing read diverges exactly at that tick and loses the bounce; changing an ignored packet byte is still caught; the observer refuses a wrapped tick.
- `cargo test -p xapi`: the tap's log holds exactly the bytes the guest got, replay gives them back over a different live pad, and a recorded failure status comes back as the call's error.
- `crates/apps/app/tests/fps_lockstep.rs` replays the same input at all four display rates. On a fake wall clock each guest frame costs 16.67 ms of work plus the sleep requested by the shell pacer. The checks require 30 ticks in each wall second, identical guest traces, and 30, 60 or 120 window draws per second (every event-loop turn at Uncapped). `app::fps` tests that every publish is drawn once at any phase, at 60 and 120, and that the grid only fills gaps. Explicitly unpaced play takes only the simulated work time.
- `crates/apps/boot/src/idle_audio.rs` checks that a pending sound advances the kernel clock by one quantum and that the host sees each new deadline before another slice can run.
- `cargo test -p boot --lib lockstep`: the toy guest through boot's wiring writes traces that compare identical, or diverge at the changed tick.

### Next

1. With the disc: the two-run control on the Heretic script, ticks on and off, then the same at `H2_FPS=120` under `h2 play` (the trace must match). A mismatch with the mode off is a runtime determinism bug to fix first.
2. A per-present hash on the render-thread side, to show in a real run that display frames leave `Device` state unchanged.

## 7. Display frames from two scenes

Status: Built, tested with fakes; not run in the game.
Code: `crates/rewrites/frame-scene/src/pace.rs`, `crates/rewrites/frame-scene/src/display.rs`, `crates/rewrites/native-render/src/frame_graph/display_rate.rs`, `crates/platform/d3d/src/scene_sink/retarget.rs`, `crates/platform/d3d/src/threaded/owner.rs` (`DisplayRate`).

Above 30 fps the frames between the guest's are drawn by the native
renderer's frame owner from the last two sealed scenes, with each game
view's camera blended between their ticks
([framescene.md](../graphics/framescene.md), section 7 and slice 8).
The rate is the Frame Rate setting's (60, or 120 while display frames
fit the render thread, `frame-scene::pace::Headroom`, else 60). It needs
a paced frame owner: a display with a render thread, on unless
`frame-scene.sink=off`. Frames the owner declines (exact readbacks for
network play, films and lockstep) are shown once. `render.stats`'
`display_rate` counts the frames
([agent-api.md](../runtime/agent-api.md), "render.stats"). There is no
D3D-side replay or reprojection, and no switch: the setting drives it.

## 8. Reprojection

Not built. A rotation-only warp of the world image could come back on the
native frame as a cheap mode if a display frame does not fit at some
scale ([framescene.md](../graphics/framescene.md), section 13).

## 9. World marks on made frames

Status: Built and tested on synthetic cameras.
Code: `crates/platform/d3d/src/present_camera.rs`, `crates/rewrites/native-render/src/frame_graph/display_rate.rs` (`marks_present`), `crates/apps/cli/src/play_cmd.rs` (`mark_frame`, `mark_cameras`), `crates/apps/app/src/overlay/marks.rs`, `crates/extensions/game/src/superbounce_world.rs`.

The window draws world marks (the editor's marks, the superbounce figures) over the finished frame, projecting each world point with a camera ([agent-api.md](../runtime/agent-api.md), "Drawing world-space lines over the frame"). A mark is captured with a genuine guest image and that image's per-view cameras. On a made frame the world is drawn with another camera, so a mark projected with the image's own would swim against the world by up to a tick of camera motion while the player turns.

### The camera per present

- **A genuine image:** its own cameras.
- **A made frame:** per view, the camera the display frame drew it with: the blended camera, or the newest scene's own on a cut and at alpha 1.

The frame owner's display-rate loop keeps each owned present's guest image with its scene (`present_camera::guest_image`, read on the render thread inside the present's metadata). Every display frame, at a Swap or between, is presented inside `present_camera::with` as a `Present::Synthetic` (`display_rate::marks_present`, `Present::display_frame`). It carries one `ViewShift` per view drawn with another camera than the image's: `C⁻¹ B`, the source image's camera `C` for that view and the drawn one `B` (the frame's game view at the same viewport, first-person views left out), both the guest's view-projection, in world space. A view drawn with the image's own camera (the newest scene at alpha 1) has none. A view with no drawn camera at its viewport keeps the image's; `render.stats`' `display_rate.marks_moved` and `marks_held` count the two. `mark_cameras` multiplies each mark camera by its view's shift (matched by viewport, within a hundredth of the image on each side; a single view on each side pairs). The mark camera's own conventions (aspect factor, depth range) are left as they are: `M · C⁻¹ · B` projects a world point where `B` draws it, through `M`'s mapping.

The marks come from the newest image, or from the frame before while a blend is under fraction 0.5, so what a mark shows (where it is, which spots are listed) lags the world by at most half a tick. Marks fixed to a moving object step at the tick rate while the object blends.

### Motion in marks

A mark's geometry is captured once a guest frame. Anything that moves within it goes in its style, which the window runs at each present by its own clock: `Pulse`, `Chevrons` with a speed, and `Fade` by distance. The superbounce way is the style's chevrons with no line under them (`LineStyle::stroke` off). `superbounce_world::Scene::seconds` is the capture time and steps at the tick rate; nothing draws from it.

The superbounce logic counts game ticks, not presents or wall time: the mode's state, the charge tracker (38 still ticks to lose a charge, the 30 and 150 tick windows after it), the medals and the HUD values run in `GAME_TICK` observers, and the tutorial's step limits are game seconds (`superbounce_hud::tutorial_seconds`). Below 30 fps the game plays in slow motion and they slow with it.

### Tests

`cargo test -p d3d --lib present_camera`, `cargo test -p d3d --lib an_owned_draw_sees_the_guest_image_of_its_present`, `cargo test -p native-render --lib frame_graph::display_rate`, `cargo test -p cli made_frames_move_the_mark_camera`, `cargo test -p app --lib overlay::marks`, `cargo test -p gfx --lib probe`, `cargo test -p game --lib -- world_lines superbounce_world superbounce_hud`.

### Checking it in the game

`render.marks` ([agent-api.md](../runtime/agent-api.md), "render.marks") gives each present's frame kind, a made frame's fraction and `shifted`, and the first anchors' window pixels. Turning at a steady rate with a mark on screen at `H2_FPS=60`, an anchor's `x` steps by about the same amount on every present, and the made frames say `shifted: true`; with the tick camera held it would move on genuine presents only.
