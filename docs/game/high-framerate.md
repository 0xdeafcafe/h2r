# 60 and 120 fps

Status: Partly built. The setting, the determinism harness, the interpolated replay (D) and rotation-only reprojection (E1) are built and tested on synthetic draw lists; none has been run in the game. Object tagging, bundles, 120 and variable refresh are design.
Code: `crates/extensions/lockstep`, `crates/platform/xapi/src/tap.rs`, `crates/apps/app/src/fps.rs`, `crates/platform/d3d/src/interp/`, `crates/extensions/game/src/d3d_glue.rs`, `crates/extensions/game/src/lockstep_glue.rs`. Tests: sections 6 to 8.
Switch: `H2_FPS`, `impl.render.interpolate`, `impl.render.reproject`, `impl.render.reproject_ahead`, `H2_LOCKSTEP*`, `H2_INPUT_RECORD`, `H2_INPUT_REPLAY`, listed in [switches.md](../runtime/switches.md).

The rule: **the guest tick stays 30 Hz, and 60 or 120 fps interpolates the picture.** Super bounces are a physics and collision quirk of the 30 Hz tick, so the simulation stays bit-identical to 30 Hz play and only what is drawn runs faster. Every community attempt that raised the tick lost super bounces; the ones that kept a 30 Hz tick and interpolated the picture kept them ([notes/high-framerate.md](notes/high-framerate.md) has the record). The planned home of 60 and 120 is the frame-scene renderer ([framescene.md](../graphics/framescene.md)); the rule carries over to it.

In short:

- **The guest is untouched.** Same 60 Hz logical vblanks, same two-vblank frame interval, same one tick per frame, same input polls. No guest code, guest global or guest timing changes.
- **The in-between frames are made on the host.** Every guest frame is a list of resolved draws that hold no guest pointers ([d3d.md](../graphics/d3d.md), "Draws"). The host keeps the last two guest frames and draws the newest one at the display's rate, with camera and object matrices blended between the two ticks. The guest never sees these frames.
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
  never change the schedule ("schedule, history and guest state are the same
  paced or not"). A slow host plays in slow motion.
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
| E1. Rotation-only reprojection of frame N's world colour | unchanged | kept | as D, one full-screen pass per display frame | camera turns smooth (most of what players see at 30), translation and objects step at 30 Hz; needs overscan for the edges | **built as D's comparison** (section 8) |
| F. Extrapolation (render ahead of the newest tick) | unchanged | kept, but drawn wrong | as D | no added latency, but overshoots on every sudden change: landings, collisions, the bounce itself | camera aim only |

**E1, the cheap comparison.** Capture frame N's world colour before the
first-person weapon and HUD passes (the pass tag below says where), render
the view with some overscan, and at each display frame draw that image
rotated by the camera's rotation between the tick and the in-between
camera (a homography, no depth needed for a pure rotation). The weapon and
HUD go on top unchanged. Its cost is one full-screen pass per display
frame, independent of the draw count, so it fits 60 and 120 where D's
geometry replay (below) does not yet. It does nothing for strafing or
moving objects. D (section 7) and E1 (section 8) are two modes of the same wrapper,
behind separate switches, so they can be compared.

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
| the frame becoming visible | the window redraws on the publish, then the display's vsync (0-16.7 ms at 60 Hz) | the first in-between frame is drawn at the frame's arrival (alpha 0, frame N-1's pose), then on the 60 Hz grid; the window's own redraw timer re-shows the newest publish |
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
  `H2_FPS` / `h2 play --fps` (section 6). 30 means off: no replay.
  The replay itself is behind `impl.render.interpolate` or
  `impl.render.reproject`, off by default (sections 7 and 8).
- **Pass tags.** A pass tag is what this design wants: which draws are the
  world, which the first-person weapon, which the HUD, and where E1
  captures the world colour. None is wired to the device; the first slice
  tells views apart by viewport and target size, and draws by their
  constants (section 7), and E1 finds the overlay by a rule on the
  classified draws (section 8, "The pass tag").

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
| 2 | **Partly built** (sections 7 and 8). Camera interpolation: two-frame store, alpha from measured arrivals, view constants rebuilt and verified, cut detection, replay on the render thread, 60 Hz pacing, the setting (built); E1 for comparison (built); then bundles, 120/VRR once measured | 2-4 weeks | identical hashes with the replay on and off; replay cost under 16 ms at 60 (8 ms for 120) |
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

`H2_FPS=30|60|120|uncapped`, or `h2 play --fps`, sets `AppConfig::fps` (`app::fps::FpsTarget`). With neither, the Video screen's Frame Rate row decides (saved as `[graphics] fps` in `<data>/settings.toml`; [settings-menu.md](settings-menu.md), "Video screen"). A press there applies at the window's next turn: `app::fps::FpsControl` carries it, the `DisplaySchedule` is retargeted and, to or from uncapped, the surface's vsync is configured again (`app::fps::follow`). It reaches only the window:

| setting | window |
|---|---|
| `30` (default) | a redraw per guest present; the code path is the plain one |
| `60`, `120` | also redraws on its own `DisplaySchedule` at that rate (late redraws skip slots, never burst); `vsync` as set |
| `uncapped` | redraws every event-loop turn, vsync off |

Between guest presents a redraw shows the newest guest frame again, so **above 30 nothing looks smoother** unless the replay is switched on (`impl.render.interpolate` or `impl.render.reproject`, sections 7 and 8), which publishes in-between frames for those redraws to show. `app::fps::alpha` is the same fraction on the window's side, tested but not used; the replay computes its own (`d3d::interp::pacing::alpha`) on the render thread, from the frames' arrivals.

### Tests

- `cargo test -p lockstep`: the hash sees any changed word and a swap of two, names the changed MiB; traces round-trip through text; compare finds the first divergence and truncation; the clock counts as guest state. On the toy guest (`lockstep::synthetic`: a generated tick handler, a game-time record, an `XINPUT_STATE` input and a crouched-landing-on-a-seam "super bounce"): equal inputs give equal traces; a replay reproduces the recording whatever the live pad does; releasing crouch on the landing read diverges exactly at that tick and loses the bounce; changing an ignored packet byte is still caught; the observer refuses a wrapped tick.
- `cargo test -p xapi`: the tap's log holds exactly the bytes the guest got, replay gives them back over a different live pad, and a recorded failure status comes back as the call's error.
- `cargo test -p app`: `H2_FPS` parsing and its default; 30 is never due on the schedule; 60 and 120 redraw 60 and 120 times a second on the fake clock; a late window skips slots; alpha. `crates/apps/app/tests/fps_lockstep.rs` runs the toy guest through the shell's triple buffer with a presenter on the fake clock at all four settings: every guest frame reaches the screen, redraws per guest frame are 1, 2, 4 and over 30, and the guest's trace is identical at every setting. A control shows the check catches a guest that ran one extra tick.
- `cargo test -p boot --lib lockstep`: the toy guest through boot's wiring writes traces that compare identical, or diverge at the changed tick.

### Next

1. With the disc: the two-run control on the Heretic script, ticks on and off, then the same at `H2_FPS=120` under `h2 play` (the trace must match). A mismatch with the mode off is a runtime determinism bug to fix first.
2. A per-present hash on the render-thread side, to show in a real run that the replay path leaves `Device` state unchanged (the synthetic test in section 7 shows it for the backend calls).

## 7. Phase 2: the interpolated replay

Status: Built, tested on synthetic draw lists only. Code: `crates/platform/d3d/src/interp/` (`backend.rs`, `model.rs`, `camera.rs`, `pacing.rs`, `epilogue.rs`), `crates/extensions/game/src/d3d_glue.rs`. Tests: `crates/platform/d3d/tests/interp.rs`, `crates/platform/d3d/tests/interp_gpu.rs`.

Design D's geometry replay at 60 Hz, for a single gameplay view, behind a switch.

### Switches

Listed in [switches.md](../runtime/switches.md). This section's behaviour:

| switch | effect |
|---|---|
| `impl.render.interpolate` (in `H2_FEATURES`) | wraps the window's backend in `d3d::InterpBackend`, inside the render thread. Only this exact switch: `impl.render=on` does not turn it on |
| Frame Rate row / `H2_FPS` / `h2 play --fps` | 30: the replay records nothing and forwards every call. 60, 120 and uncapped: in-between frames at 60 (`interp::pacing::MAX_REPLAY_HZ`; 120 waits on measured headroom) |
| `H2_RENDER_THREAD=0` | the replay stays off (it runs only on the render thread) |
| `impl.render.pipelined_frame=on` | the replay stays off: the lockstep harness cannot certify the combination |

It is Rust in `d3d` and `boot`; no image rebuild.

```sh
H2_FEATURES=impl.render.interpolate h2 play --fps 60 ...
```

At exit (and every 5 s while running) the replay prints one line: `d3d: interpolation: frames F blended B held H replays R substituted draws D snapped S deferred P size_mismatch Z (extent Ze mapping Zm screen Zs overlay Zo) unsure U (camera Uc unpinned Up) unpaired Q (gap Qg camera Qc views Qv counting Qn) dropped X; replay mode: N frames, A ms avg, M ms max`.

- `Z`, `U` and `X` count frames shown as the guest drew them by the guards below; `Z` and `U` split by the guard that held them.
- `Q` counts frames with nothing to blend: no previous frame close enough (`gap`: the first, or after a load, a pause or a hitch over 45 ms), no camera (`camera`: a 2D menu), views other than the previous frame's or, for E1, not one view with a world (`views`), or a pixel count open at the present (`counting`).
- Every held frame is in exactly one of `Z`, `U`, `Q` and `X`.
- `N` counts every frame the replay made (blended presents and in-between frames) and the times are the render thread's wall time for each: its draws, plus `present_replay` for in-between frames. The guest's own present is not counted. It is encode and submit; the GPU's own time is not in it. E1 prints its own part (section 8).

### How it works

- **`d3d::interp::InterpBackend<B>`** (`crates/platform/d3d/src/interp/backend.rs`) wraps the backend the render thread drives (`ThreadedBackend<InterpBackend<GfxBackend>>`, or around the `ScaledBackend` with a render scale). It passes every call through. With the setting above 30 it also keeps, for each guest frame, the clears, partial clears, copies and drawn draws whose colour target is a surface the guest has presented (the swap chain), with the draws' textures pinned (`Arc`), so a texture the device frees later still replays. Draws into other targets (shadow maps, render-to-texture, reflections) are not kept: in-between frames sample frame N's.
- **Classification** (`interp::model`), once per frame. Nothing in it knows Halo 2's register map; it searches each draw's 192 vertex constants (once per distinct constant block) for:
  - perspective blocks: four registers, as rows or columns, that split into a projection and a rigid camera (`interp::camera::Persp::split`) and rebuild within 1e-5 of their size. Orthographic HUD transforms do not split;
  - the view's camera: per view (viewport and target size), the perspective value most draws compute their clip position from, bit for bit, used by at least two. Which block a draw computes it from is read from its program (`interp::epilogue::position`): the four constant rows `oPos.x` to `oPos.w` are each a `dp4`/`dph` with, followed through temporaries and moves, as they stand before the epilogue reads them back. A program that passes its vertices through (`mov oPos, v0`) is `Fixed`: no constant places them. Anything else (columns, products of blocks) is `Unknown` and does not vote. A rule that counted every perspective-looking block is wrong: see [the note](notes/high-framerate.md) ("Camera pick");
  - objects drawn with the camera: perspective blocks that are the camera times a rotation, uniform scale and translation (the world part recovered with the camera's inverse, accepted when it rebuilds within 1e-4: the guest's matrices come through x87 emulation, so matches are by tolerance, not bits);
  - draws with a projection of their own (the first-person weapon and hands): blended in camera space;
  - in draws that use the camera, node matrices (three registers forming a rotation, uniform scale and translation), and the eye position (a register whose `xyz` is the camera's position, for fog, specular and sky).
- **Pairing** (`model::match_draws`). A draw continues the previous frame's draw with the same signature (view, vertex program, primitive, vertex and index counts, stream strides and sizes, texture headers); among several (instances of one mesh) the nearest world position first. There is no object datum: a pass tag and an object tag from the render loop would replace the signature (section 3, "Pass tags").
- **Substitution** (`model::substitute`). At alpha `a`: camera blocks get the in-between camera (eye lerped in a straight line, orientation slerped, projection lerped); object blocks get the in-between camera times the blended world; own-projection blocks blend in camera space; nodes blend; the eye register follows the camera. A draw with no match (spawned) keeps its own object matrices with the in-between camera, so it stays put in the world. At `a >= 1` nothing is substituted: the replay is frame N exactly. A camera that has not moved keeps its uploaded value.
- **Snaps** (`model::Tuning` defaults). A camera that moved more than 3 world units, turned more than 1 rad, or changed its projection by more than 5% (scope zoom, vehicle entry, a cinematic's lens) is not blended; neither is an object that moved more than 2 units or turned more than 1.6 rad. The pair is dropped altogether when the frames arrived more than 45 ms apart (`pacing::HOLD_GAP_NS`: the guest below about 22 fps, a load, a pause, a pacer rebase: the newest frame is held as drawn), when either frame has no camera, or when the set of views changed (a split-screen layout change).
- **Pacing** (`interp::pacing`). Each present is stamped on the game's thread at the `Swap` entry (`Backend::stamp_present`, passed along by `ThreadedBackend`, which reads the `Instant` in `present_with_gamma`), not when the render thread reaches it. Frame N is shown from alpha 0 at its arrival to alpha 1 one measured interval later (when N+1 is due); nothing is extrapolated and a late frame holds at 1. A catch-up frame blends over its longer gap. In-between frames are due on a 60 Hz grid that skips late slots and restarts at each guest frame; a slot within a quarter period of the next frame's expected arrival waits for it (drawing alpha near 1 there would duplicate the next frame's alpha 0), and shows alpha 1 if the frame is late.
- **The guest's present** shows the replay at its alpha (about 0: frame N-1's poses on frame N's draws) in place of the guest's surface, so the real frame is never shown ahead of the blend. It is still the guest's one present for the backend (frame count, shader upkeep, the `D3DSWAP_FINISH` wait). Frames with nothing to blend show the guest's surface as drawn.
- **In-between frames** come from `Backend::idle`, which the render thread calls only when its queue is empty (a zero-timeout receive takes any waiting batch first), and are shown with `Backend::present_replay` (`GfxBackend`: submit and the present hook only; `ScaledBackend`: the view's surface, no frame boundary). The guest's commands always go first.

### Rules and limits

- **What the replay may touch.** It draws only into surfaces of its own (a private copy of the presented surface and of each depth surface its draws use), never reads anything back, writes no guest memory, and never runs while a pixel count is open (cleared, not yet reported), so no replay draw lands in a visibility count. A replay error turns the replay off for the rest of the run instead of stopping the game.
- **Shared GPU.** A pixel count submitted after a replay lands later, so a polled visibility test can answer `D3DERR_TESTINCOMPLETE` where it would not have. That is the timing class [render-thread.md](../graphics/render-thread.md) accepts; `Device::set_visibility_poll(false)` makes it exact.
- **Pinned inputs.** Textures are pinned by `Arc`, targets are private, uniforms go through the backend's normal per-draw path.
- **Resets.** Gaps over 45 ms, no camera and view-set changes reset the history; cuts snap. A checkpoint revert is caught only as a camera cut.
- **The window's own schedule.** The replay's 60 Hz grid decides when in-between frames are drawn, but the window's `DisplaySchedule` (`app::fps`) re-shows the newest publish at its own rate; the two are not one scheduler.
- **Fog and sky** parameters other than the eye position are not found.
- **Render targets a frame samples are copied as the draw read them.** A recorded draw that samples a render target through a live view (`surface_texture`) keeps a snapshot taken right after the draw, so a replay never sees what the guest drew into the target since. A surface copied into the frame cannot be kept: the frame is held (`unsure`, `unpinned`). Shadow maps need nothing: the texture a depth level is sampled through (`Backend::depth_level_texture`) is the backend's copy of its texels, made once each time the level is drawn.
- **Only centred perspective cameras** (`Persp::centred`: no shear, the axis within 5% of the screen's middle, a sane focal length). A perspective into pixels (screen-space UI, menus) is not a camera to move: held (`unsure`, `camera`).
- **A made frame shows at the guest frame's host size** (`Backend::surface_extent`, the view's scaled copy through `ScaledBackend`), else it is not shown (`size_mismatch`, `extent`).
- **One made frame on the GPU at a time** (`Backend::gpu_fence`): while the last one is unfinished, idle slots are dropped and the guest's frames are shown as drawn (`dropped`), so replay work never queues up ahead of the guest's frames.
- **Cost.** A replay re-records and re-encodes every kept draw through the normal backend path (no bundles), so one replay costs about what the back-buffer share of the guest frame's encode costs (the figures are in [the note](notes/high-framerate.md), "Encode budget"), twice that through `ScaledBackend`, plus classification. At 60 there are two replays per guest frame. In heavy scenes the render thread cannot keep up, the frame-in-flight limit holds the guest, and the game slows down.

### Tests

`cargo test -p d3d --lib interp` (matrix and quaternion helpers, perspective split and rebuild, camera blending, cuts, pacing: 60 shows two frames per tick, a catch-up frame blends over its longer gap, a gap holds the newest frame, a late frame holds at 1, settings map to rates) and `cargo test -p d3d --test it interp::` (synthetic frames through the recording backend):

- in-between frames at alpha 0 (frame N's draws with frame N-1's camera and nodes), 0.5 (camera half way along a straight eye path, node and eye register half way) and 1 (frame N bit for bit), the slot before the next frame waiting for it, then nothing more;
- instances of one mesh pair by nearest position even when drawn in another order; a spawned object keeps its own matrix; a despawned one is dropped;
- the HUD (orthographic, pre-transformed) replays with its constants untouched;
- objects drawn with world-view-projection (column layout) blend in the world; a weapon with its own projection blends in camera space;
- a zoom and a teleport snap; a 200 ms gap shows the guest's frame as drawn; no replay while a pixel count is open;
- the guest's own calls (surfaces, clears, draws, presents and their flags) are the same at 30, 60, 120 and uncapped as through a plain recording backend, the replay never reads back, and at 30 every call is identical;
- turning the setting to 30 mid-run goes back to plain presents;
- through `ThreadedBackend`, the render thread's idle step draws in-between frames;
- menu frames (a screen-space perspective shared by the panels, orthographic text, a pre-transformed background) are shown as drawn in D and both E1 variants; a made frame of another host size is not shown; slots are dropped while the GPU is busy; a sampled render target replays from its snapshot, and a copied-in surface holds.

`cargo test -p d3d --test it interp_gpu::` runs D and E1 on the GPU through `ScaledBackend` at 2x and 3x: every presented frame is the guest's size at that scale, D's made frames have the guest's pixels exactly and E1's (a still camera) agree within filtering on over 98% of pixels. `cargo test -p boot --lib interpolation` checks the switch's decision (off by default, needs the render thread, off with `impl.render.pipelined_frame`) and `H2_FPS` parsing.

### Not verified

- A game run at 60: does the camera classify in gameplay (the stats line's `blended` against `held`), do objects blend, what pops. Then the phase 1 harness with the switch on: the per-tick trace must match a run with it off.
- What a player should see (S, from the design): camera turns and movement at 60, about one tick behind the 30 fps picture; bipeds, vehicles and other objects whose matrices classify move at 60; the first-person weapon and hands blend with the camera; snaps instead of blends on cuts, scope zoom, teleports and respawns; anything whose matrices do not classify draws as frame N, never wrong. Still at 30 Hz: particles, contrails, beams and muzzle flashes, shadows, reflections and other render-to-texture effects, the HUD, motion sensor and text, and world-anchored markers (the care list in section 3).
- Replay cost against 16.7 ms at 60 and 8.3 ms at 120 (the exit line measures render-thread time).
- Not built: bundles or reused passes so a replay costs uniform writes, not a re-encode; a pass tag and an object tag (section 3, "Pass tags"); one display scheduler driven by the replay; shadows, reflections, projected lights and particles (the care list).

## 8. E1: rotation-only reprojection

Status: Built, tested on synthetic draw lists only. Code: `crates/platform/d3d/src/interp/reproject.rs`, `epilogue.rs`, `screen_space.rs`, `holds.rs`, `crates/platform/gfx/shaders/warp.wgsl`. Tests: `crates/platform/d3d/tests/interp.rs`, `crates/platform/d3d/tests/interp_gpu.rs`, `crates/apps/boot/tests/headless_interp.rs`.

The comparison section 3 asks for, as a second mode of the same `InterpBackend`: same recording, classification, pacing, private surfaces and rules as section 7; only how an in-between frame is made differs.

### Switches

Listed in [switches.md](../runtime/switches.md).

| switch | effect |
|---|---|
| `impl.render.reproject` | E1, the display camera's rotation blended between frames N-1 and N (one frame behind, like D) |
| `impl.render.reproject_ahead` | E1 with the rotation carried on past N by alpha of the last turn (F, extrapolation, for aim only): no added latency for turning, overshoots when a turn stops |
| `impl.render.interpolate` | D (section 7). With a reproject switch also on, E1 wins |

Off by default; exact switches only (`impl.render=on` turns none of them on). Same rules as D: render thread only, off with `impl.render.pipelined_frame`, the Frame Rate row above 30 (in-between frames at 60). Rust in `d3d`, `gfx` and `boot`; no image rebuild.

```sh
H2_FEATURES=impl.render.reproject h2 play --fps 60 ...
H2_FEATURES=impl.render.reproject_ahead h2 play --fps 60 ...
H2_FEATURES=impl.render.interpolate h2 play --fps 60 ...   # D, to compare
```

At exit, after the common counters: `; reproject mode: C world images A ms avg M ms max, W warped frames A ms avg M ms max`. A world image is the per-guest-frame draw described below; a warped frame is one shown frame (the warp, the overlay draws, and for in-between frames their present), at the guest's present and in between. Times are render-thread wall time measured the same way as D's `replay mode` part, so the two compare directly: D's per-frame cost against E1's world image per guest frame plus its warped frames.

### Headless (an off-screen serve)

The same switches work with no window, on the headless GPU backend `H2_BOOT_GFX=<dir>` makes (`h2 run`, `h2 serve`), so an agent can read E1's (or D's) counters and hold reasons through `h2 agent render` without anyone playing:

```sh
H2_OPT=2 H2_BOOT_GFX=$TMPDIR/h2-gfx H2_BOOT_GFX_EVERY=never \
  H2_FPS=60 H2_FEATURES=impl.render.reproject \
  h2 serve disc/halo2.iso --out <the image you built>
h2 agent render          # "interp": frames, warps, the held split
```

- **What runs.** `boot::d3d_glue::install_headless` puts the GPU backend (at 1x, or `ScaledBackend` under `H2_RENDER_SCALE`) behind `InterpBackend` on the render thread, as a window does; which host has a GPU backend is `d3d_glue::interp_render_thread`. Without `H2_BOOT_GFX` (the recorder draws nothing) and under `H2_BOOT_GFX_LIGHTING_AB` there are no in-between frames. `H2_RENDER_THREAD=0` and `impl.render.pipelined_frame` turn them off.
- **Pacing.** No display, so no vsync: the in-between frames are paced on the wall clock at the frame rate, the same schedule a window uses (`pacing::Schedule`). At real-time speed a guest frame at 30 gets one in-between frame at 60; a serve sped up past that shows fewer, since a frame arriving early takes the slot. Each is made for real (the world image warped, the overlay drawn, presented through `GfxBackend::on_present_replay`) and every guest frame is classified, so `blended_frames`, `replays`, `warps` and the hold reasons are the game's own. A guest frame more than 45 ms after the last is `unpaired_gap`, as in a window.
- **Pictures.** `H2_BOOT_GFX_BETWEEN=N` writes the first N in-between frames to the directory as `frame-NNNNN-between-KK.png` (the KK-th after guest swap NNNNN), and each one's neighbours as `frame-NNNNN.png`: the guest frame shown before it and the one shown after it (as shown: a blended present is a warped frame too). At most N in-between and 2N guest frames are written. While captures are left, each guest frame is copied on the GPU (no read back) and the copy is read back only once an in-between frame follows it, so a long run does not read back every frame. In-between frames go to their own hook, so the guest's swap numbering (`H2_BOOT_GFX_EVERY`, `H2_BOOT_GFX_SHADOWS`) is unchanged.
- **Held frames, draw by draw.** `interp.holds` in `render.stats` names the draws that keep gameplay at 30 (next section).

### Held frames, draw by draw

The `mismatch_*` counters say which guard held a frame; `interp.holds` in `render.stats` (`d3d::interp::holds::HoldLog`, kept by `InterpBackend`, registered by `boot::render_probe::set_interp_holds`) says which draws. For every frame the screen guard (`world_screen`) refuses, with a camera, every world draw or clear it refuses is listed, not only the first:

- `last`: the last such frame's first offending draw in full.
- `kinds`: each kind of offending draw (`reason`: `screen`, `mapping` or `overlay`; `why`: the detail, below; the vertex program; the primitive), with the held frames it was first in (`frames_first`), the held frames it was in at all (`frames`), its draws over all of them, and an example. At most 48 kinds; more are counted in `other_kinds`.
- `programs`: the kinds' vertex programs, disassembled, by id (at most 24).

Each draw says: its number in the guest frame and its pass, as `render.frame` numbers them (draws over every target since the present; a pass is a run of work on one colour and depth surface), so `h2 agent frame draws` shows the same draw; its number among the presented surface's draws, how many of those were world draws, and where the overlay started; the vertex program's id (`pipeline_store::program_id` of its microcode, the id `pipelines-v2.jsonl` records it under); entry, primitive, vertex count, texture formats and sizes, blend and depth write; where its clip position comes from (`rows c[N]`, `pass-through`, `unknown`) and what the block there is (`camera`, `world`, `local centred`, `local off-centre`, `none`), the block's four rows as uploaded and the projection split from them; and the bounding box in window pixels where its first 64 vertices land after its program (run through the shader interpreter), next to the target's size, so a full-target pass and a sprite tell apart.

| `why` | reason | what the draw is |
|---|---|---|
| `pretransformed` | screen | `XYZRHW` vertices, already in pixels |
| `fixed_function` | screen | fixed-function vertex processing |
| `pass_through` | screen | a program that moves a vertex to `oPos` (`mov oPos, v0`) |
| `position_unknown` | screen | a clip position not from one block's rows |
| `not_perspective` | screen | the block there is no perspective (orthographic, or no matrix) |
| `off_centre` | screen | a perspective of its own, not centred (into pixels, sheared) |
| `no_camera_draw`, `no_epilogue`, `camera_not_covering`, `partial_clear`, `other_mapping`, `scissored` | mapping | where the world lands is not one mapping the warp can follow |
| `camera_after_overlay` | overlay | a draw the camera places after the overlay starts |

A frame's first offence is its hold reason, so the counters are unchanged. Frames with no camera (menus, loads) are not logged: they are held as `unpaired` with nothing to warp.

### Screen-space draws in the world

Halo 2's gameplay frames hold pass-through world draws (`mov oPos, v0`: their vertices are window pixels) that a warp has to handle ([the note](notes/high-framerate.md), "Hold log of a gameplay serve", has the counts). Three kinds:

| kind | what it is |
|---|---|
| full-target layer | one blended triangle fan over the whole target among the world draws, with four textures (the first a full-size target drawn that frame, looked up per pixel) |
| sprites | blended small fans with one small texture among the opaque world draws, with the depth surface bound: points of the world the game projected to the screen with the frame's camera |
| orthographic quads | an untextured full-target fan and a thin strip at the top: a fill and letterbox bars, in short frames (cinematic edges) |

The rules (`interp::screen_space`, used by `world_screen` and the world image):

- **Sprites are warped with the world.** The game put each on the screen by projecting a world point with frame N's camera, so in the world image its positions are scaled by the overscan about the screen's centre exactly as the world's are (each program's viewport scale times `s` scales about the same centre), its depth and texture coordinates kept. After the warp at frame N's rotation it is where the guest drew it (to float rounding); in between it turns with the point it marks. Drawn unwarped on top it would slide against the world, and since it is drawn among the opaque world draws with their depth, moving it after them would change what covers what.
- **The full-target layer is warped with the world and kept over the whole image.** It is blended over the world drawn so far and the particles are drawn over it, so it can be neither moved on top (blend order) nor left out. Its first texture is a target drawn that frame and looked up per pixel, so inside the guest's view the world image must show at each pixel what the guest showed at the pixel it maps to. Shrinking it with the overscan would do that but leave the band the overscan adds (the only pixels a turn reveals) without the layer, a strip at the leading edge on every turn. So it keeps covering the whole image and every other input is moved to its value at the guest's pixel the image shows there (`q = c + (p - c) / s`): the same texels inside, its own coordinates carried on in the band (clamped or wrapped as its samplers say). That is exact only when the program's outputs are its inputs moved (no constant, no arithmetic) and the inputs are affine over the quad's pixels with one `w`; both are checked (`Fit::Extend`), otherwise it is shrunk (`Fit::Shrink`).
- **Orthographic quads are held.** A fill and letterbox bars are fixed to the screen, not to the world, and they sit among the world draws: warped they would turn, moved on top they would change the blend order. Those frames stay at 30.
- **Still held:** a pass-through draw whose program also reads its position elsewhere (a texture looked up from it would read another texel once moved), whose position is not a float attribute at register 0 (nothing to rewrite), or that is scissored; pre-transformed and fixed-function draws; unknown positions; off-centre perspectives.

`InterpStats` counts the moved draws per world image: `screen_shrunk` and `screen_extended` (the exit line's `screen-space draws S shrunk E extended`).

### How it works

- **The overlay.** Without a pass tag, the draws that go on top unwarped are found on the classified frame (`interp::reproject::overlay_start`): from the first draw with a projection of its own (the first-person weapon and hands) after the world has started, or from the first draw after the last one that uses the camera (the HUD), whichever comes first. Everything before it is the world.
- **The world image, once per guest frame.** At the guest's present, the world's ops (clears, copies, draws) are drawn again into a world copy of the presented surface (and of its depth surfaces) with **overscan**: each draw's own viewport scale (the `x` and `y` components its program's epilogue multiplies by) is scaled by `1 / 1.1` (`reproject::OVERSCAN`), so the picture shrinks about the screen's centre and the image covers 10% more of the view in each direction (the host clips in its own clip space, after the viewport transform, so what lands outside the guest's frustum but inside the target is drawn). The clip position is the guest's, so whatever else a program computes from it is unchanged: many Halo 2 programs look a screen-space texture up from it (`dp4 oT0.x, r12, c[8]`), which scaling the camera's clip `x` and `y` instead would read at the wrong texel. The guest's own frame cannot be used: it is not overscanned, and with no pass tag there is no point mid-frame to copy it at.
- **The viewport transform** (`interp::epilogue`). The scale and offset are found in each program's microcode, once per program: a symbolic run that follows `oPos` from the clip position through `clip * k`, `1 / w` and `+ o` with `k`, `o` constant components, and accepts the program only when `oPos.xy` ends as `clip.xy / w * scale + offset`, `oPos.w` as the clip `w`, and the scale components are read and flow nowhere else (relative reads only above them, as skinning's are). D3D's epilogue (`c-38`/`c-37`), Halo 2's (`c[10]`/`c[11]`, hardware numbering) and a divide-first form with scale and offset in one register are read; a pass-through (`mov oPos, v0`) or a program that reads its window position back is not. The push-buffer registers (`ResolvedDraw::hw`) hold D3D's numbering, not what Halo 2's programs apply, so the mapping is read from the program.
- **The warp, per shown frame** (`Backend::warp_surface`, `gfx::blit::Warper`, `crates/platform/gfx/shaders/warp.wgsl`). One full-screen pass draws the world image into the private copy of the presented surface through a homography `H = M_w R_N R_dᵀ M_d⁻¹` (`reproject::homography`), where `M_d` is frame N's projection acting on directions, `M_w` the same with the overscan, `R_N` frame N's camera rotation and `R_d` the display camera's (`reproject::display_rotation`: slerped from N-1 to N at alpha, or from N on by alpha with `_ahead`; a cut gives N's). It is mapped to texture coordinates through the camera draw's viewport transform (the one every world draw shares), sampled linear, clamped at the edge. No depth: a pure rotation needs none.
- **The overlay on top.** The depth surfaces the overlay uses get private copies cleared to far depth and zero stencil (the world under the overlay is a picture now), then the overlay's ops replay unchanged: weapon, hands and HUD exactly as the guest drew them in frame N.
- **When.** The guest's present shows the warped frame at its alpha (as D shows its replay); idle slots show further warps on the same pacing grid. Only frames with exactly one camera view and a world blend: split-screen, menus and loads hold the guest's frame as drawn, as do gaps, open pixel counts and frames without a camera. `warp_surface` needs a single-sample target, which every surface made through `Backend::create_surface` is; a backend that cannot warp answers so, and E1 turns itself off for the run with a message, like any replay error.

### Rules and limits

- **E1 warps only a world it can show lands where the guest drew it** (`interp::backend::world_screen`), else the frame is held (`size_mismatch`):
  - `mapping`: every world draw's program must have a viewport transform the replay can read (`interp::epilogue`), the same as the camera draw's, covering the whole target, unscissored; partial clears in the world hold too.
  - `screen`: a world draw in screen space (pre-transformed, fixed function, a pass-through program, or whose position block is not the camera, an object drawn with it or a centred perspective) is not part of the turning world: it would be zoomed, or shrunk by the overscan and turned. Only the block the program computes its position from counts; the rest of a draw's constants may hold anything. A draw whose position is `Unknown` holds. The exceptions are the moved pass-through draws above.
  - `overlay`: a draw the camera places after the overlay starts (a level draw after the first-person weapon) would go on top of the turned world unturned. The HUD's pass-through quads carry the frame's constants, camera included, but nothing places them: they are overlay. A draw whose position is `Unknown` counts as placed by the camera if any of its blocks is the camera.
- One camera view and a world, else held (split screen, menus, loads).
- Not seen in Halo 2's world: pre-transformed draws before the overlay, level draws after the first-person weapon, programs that read their window position back (each holds E1).

### The pass tag

No pass tag is wired. The pass in progress is the return address of the call the render loop's scene-pass stage is making, on the game's thread. Getting it to the replay needs a hook that reads it, a per-draw tag captured at `Device` draw time (a new `ResolvedDraw` field, set at every place one is built), and the map from return addresses to world, weapon and HUD passes, which a game run has to read off. The overlay rule above stands in until then and gives the same split on the synthetic scenes.

### Tests

`cargo test -p d3d --lib interp` also covers `epilogue::position` (rows straight to `oPos` or through a temporary are read, a pass-through is `Fixed`, a product, rows of two blocks, rows out of order or a negated move are `Unknown`), `epilogue` (D3D's and Halo 2's epilogues and a divide-first one are read; screen-space lookups from the clip position are allowed; pass-throughs, missing divides, a scale or window position read elsewhere, a second clip `w` and a relative read below the scale are not; through the shader interpreter the overscan moves the position about the centre and leaves the rest alone), `reproject` (the warp samples the world image where it drew a point for a turned display camera; no turn gives the overscan zoom; display rotations blend, run ahead and snap on cuts) and `interp::screen_space` (which programs move only their position, and the affine fit).

`cargo test -p d3d --test it interp::`, through the recording backend with a real program using Halo 2's epilogue:

- at the guest's present: the guest's calls, then the world image (clear and level draws, no weapon or HUD) into a surface of its own with the program's viewport scale times the overscan, then a warp into the shown surface, a depth-only clear, the weapon and HUD with their own constants, and the present with the guest's flags;
- in between: only a warp, the clear, the overlay and a present; at alpha 1 the warp is exactly the overscan zoom, then nothing more;
- `_ahead`: the present shows N's rotation, and half a frame on it turns the opposite way from the blended mode's alpha 0, half as far;
- two views hold the guest's frame; D's line carries its own frame count and cost and E1's carries world images and warps;
- the guest's own calls are the same as through a plain recording backend in both E1 variants, and nothing is read back;
- a shadow map the world samples (`depth_level_texture`) blends in D and both E1 variants;
- world draws mapped elsewhere than D3D's registers say, pre-transformed, orthographic, fixed-function, unknown-mapping, pass-through (a full-target quad among the level's draws) or scissored world draws hold; a level draw after the weapon holds; a loading screen is shown as drawn;
- a gameplay-like frame blends in D and both E1 variants: a shadow pass, the level with the centred camera, the first-person weapon, then HUD pass-through quads carrying the camera in their constants, every draw carrying a packed block that splits as an off-centre perspective;
- every held frame is counted under one reason (a menu in perspective into pixels, a 2D menu, a loading screen, split screen in E1, gameplay after a pause);
- `the_hold_log_names_every_draw_that_held_a_frame` and `the_hold_log_says_off_centre_and_after_the_overlay`: the log has each kind with the guest draw and pass numbers, program id and disassembly, and a window covering the target; a frame with no camera is not logged;
- `screen_space_sprites_and_layers_in_the_world_turn_with_it`, `screen_space_world_draws_e1_cannot_move_still_hold` and `screen_space_draws_turn_with_the_world_through_the_interpreter`: sprites and layers are shrunk or extended with texture coordinates and depth kept, the counters say 2 shrunk and 1 extended, a scissored sprite, a layer that looks a texture up from its own position and an orthographic quad hold, and a moved sprite lands where the level's program puts its world point.

`cargo test -p d3d --test it interp_gpu::` draws E1 through the XDK's own epilogue and on one plain `GfxBackend` as a headless run has it: the in-between frame goes to the replay hook (flags 0), never the present hook, and is the guest's picture within filtering with the camera still; a 100 ms gap is counted as `unpaired_gap` and makes nothing after it. `cargo test -p boot --test headless_interp` (GPU; skips without an adapter) checks that with `H2_BOOT_GFX`, `impl.render.reproject` and `H2_FPS=60`, `install_headless` builds the interp and the agent's `render.stats` reads it, every hold reason and `interp.holds` present. `cargo test -p gfx --lib warp` validates the warp shader with naga. `cargo test -p boot --lib interpolation` checks the switches (off by default; reproject wins over interpolate, `_ahead` over plain) and which host has a GPU backend for them; `d3d_glue::tests::in_between_captures_stop_at_their_limit` and `in_between_captures_write_only_their_neighbours` check the `H2_BOOT_GFX_BETWEEN` names and that N slots write at most N in-between and 2N guest frames.

### Not verified

- What a player should see (S, from the design): camera turns at 60 (with `_ahead`, without the extra frame of lag on turns but with a small overshoot when a turn stops); strafing, walking, vehicles and every moving object at 30 Hz, since the image only rotates, with near things juddering against the turn more than far ones (no parallax); the first-person weapon, hands and HUD at 30 Hz, steady on screen while the world turns under them; a slightly softer world (the overscanned image is magnified 1.1x with linear filtering) and smeared edge pixels on flicks beyond 10% of the view in one frame; transparent effects and particles drawn before the overlay turning with the world.
- A game run of E1 next to D: smoothness of turns, the softness from the overscan, the `reproject mode` costs against D's `replay mode`. With the screen-space rules, `mismatch_screen` should be near 0 in gameplay (only the orthographic frames and anything new), `blended_frames` and `captures` rising, `screen_shrunk` about 144 times `screen_extended` on the level of the note's hold log, and `H2_BOOT_GFX_BETWEEN=12` pictures with no edge band on the layer and sprites staying on what they mark.
- Whether a loading screen is held by one of the unseen draw kinds above or is a centred camera scene the warp shows correctly.
- A full-target quad that copies the screen made that frame (seen in a converted Halo 1 level, [the note](notes/high-framerate.md), "Camera pick") still holds: showing such frames needs either that pass on top of the warped world (only if nothing after it in the world depends on it) or the copy made from the overscanned world image; neither is proven.
- Not built: drawing the world image by copying the guest's frame at a pass boundary with a wider projection set by the guest instead of a second world draw (halves the cost), the pass tag, and 120 Hz (a warp per display frame is cheap).
