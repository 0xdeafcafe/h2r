# 60 and 120 fps (design; phase 1 built)

**State.** Phase 1, the determinism harness, is built (section 6). The
setting `H2_FPS=30|60|120|uncapped` (`h2 play --fps`) exists, but above 30
it only makes the window present the newest guest frame more often. The
picture is no smoother until phase 2's interpolated replay exists.

This is the design for an opt-in presentation mode at 60 or 120 fps, or
whatever a variable-refresh (VRR) display wants. The hard requirement is that
**super bouncing keeps working at 120 fps**. Super bounces are a physics and
collision quirk of the 30 Hz game tick, so the simulation has to stay
bit-identical to 30 Hz play. Only what is drawn may run faster.

In short:

- **Keep the guest exactly as it is.** Same 60 Hz logical vblanks, same
  two-vblank frame interval, same one tick per frame, same input polls. No
  guest code, guest global or guest timing changes.
- **Make the in-between frames on the host.** Every guest frame is already
  a list of resolved draws that hold no guest pointers (d3d.md, "Draws").
  The host keeps the last two guest frames and re-renders the newest one at
  the display's rate, with camera and object matrices interpolated between
  the two ticks. The guest never sees these frames.
- **Prove it with the guest's own state.** Because the guest is untouched,
  the test can be the strongest one there is: the same input recording gives
  the same guest-memory hash at every present and every tick, with the mode
  off, at 60 and at 120.
- **Every community attempt that raised the tick lost super bounces.** The
  ones that kept a 30 Hz tick and interpolated only the picture kept them,
  and their leftover bugs were all on the rendering side.

Nothing here is useful until a level holds a steady 30 fps. It doesn't today
(about 8 fps in The Heretic, [frame-budget.md](frame-budget.md)). This mode
comes after that work, not instead of it.

## 1. How the tick, the frame and the vblank are coupled

### In the guest

Addresses and names come from [engine-map.md](engine-map.md) ("Startup and
the main loop", "Frame phases"), [d3d.md](d3d.md) ("Vertical blank") and a
read of the disassembly for this page. Anything marked "read" below is from
that disassembly and wants a check before anyone builds on it.

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
  [engine/hs_vm.md](engine/hs_vm.md)), and the budget carries a leftover
  fraction. That is what Halo 2 Vista and MCC changed to run a 60 Hz tick,
  and it is what this design must not touch.
- **Several things happen once per frame, not once per tick:** the input
  poll, frame timing, director and observer, rumble, render and sound. Any
  approach that changes how many main-loop iterations run per tick changes
  how often those run and when input is sampled relative to the tick.
- **A guest-native 60 fps exists in the data** (the interval word). Setting
  it to 1 would run the main loop at 60 Hz with every other frame running no
  tick. It is tempting and cheap, and it is rejected below: it changes the
  input-poll and per-frame cadence, it would re-render identical state unless
  the renderer reads the leftover fraction (unknown; an xref search for
  readers of `game_time + 0x10` would answer it), and it doubles the guest's
  render cost.

### In our runtime

- **The logical clock** (`kernel::Clock`, crates/kernel/README.md "Time and
  work") counts 100 ns ticks. Generated code charges one ns per guest memory
  access. The clock jumps to the next deadline when every thread waits. Rust
  overrides and DPCs are uncharged. KeTickCount, the performance counter,
  the TSC, timers and vblanks all read this one clock, so a run's schedule
  depends only on the guest's work and its inputs, never on wall time.
- **Vblanks** come from a kernel periodic source at `Rate::HZ_60`
  (`crates/boot/src/lib.rs`, `d3d::vblank::clocked_vblanks`) on that clock.
  The frame-wait override `FrameWait::HALO2_CLOCKED` yields until the target,
  so the clock jumps straight to the vblank (d3d.md). A level frame is
  33.3 ms of logical time and 2 vblanks ([frame-budget.md](frame-budget.md)).
- **Wall time is outside.** `app::pacing::Pacer` and `kernel::pacer::Pacer`
  only sleep the host so guest time doesn't run ahead of real time. They
  never change the schedule ("schedule, history and guest state are the same
  paced or not"). A slow host plays in slow motion.
- **Presentation never blocks the guest.** Swap goes through the present
  hook into a 3-slot mailbox; the window shows the newest frame
  ([app.md](app.md), "Presentation"). With the render thread (the default; `H2_RENDER_THREAD=0` turns it off) every
  `d3d::Backend` call runs on a render thread, after the device has resolved
  the draw on the guest's thread ([render-thread.md](render-thread.md)).
  The only wall-time effect on the guest is the frame-in-flight limit, and
  that also leaves the logical schedule alone.

That last pair is what makes this design possible: anything done after the
present hook, on host surfaces, can't reach the guest's schedule or memory.

## 2. Prior art

Paraphrased; no code was copied or read beyond what is needed to describe
it. Links that answered 403 during research are marked "snippet": the
finding comes from the search result text only.

### Halo 2 Vista (2007) and its community

- Vista ran the tick at **60 Hz**, against 30 on the Xbox, so its higher
  frame rate came from a faster simulation, not interpolation. Sources: the
  MCCBounceEnable README (https://github.com/Daylonz/MCCBounceEnable) and a
  Project Cartographer post on super bouncing
  (https://www.halo2.online/threads/read-regarding-super-bouncing-in-public-games.1907/,
  snippet). There is no first-party statement. Cutscenes stayed at 30
  (https://www.pcgamingwiki.com/wiki/Halo_2).
- **What broke at 60 Hz** (Cartographer post): super bouncing, button-combo
  timing, hit registration and melee. The same post says setting the tick
  back to 30 is not enough on its own, because other values were retuned for
  60 and would need resetting too.
- **Halo 2 Reclaimed** ships a 60-tick build and a 30-tick build; the 60-tick
  one carries gameplay fixes such as weapon fire rates
  (https://www.moddb.com/mods/halo-2-reclaimed).
- **Project Cartographer 0.6.9.0, "High Framerate Support"**
  (https://www.halo2.online/threads/project-cartographer-0-6-9-0-update-changelog-high-framerate-support-initial-release.4694/,
  snippet): campaign moved to a 30 Hz tick "to fix all issues associated with
  the raised tickrate"; the particle system, motion sensor and rumble were
  rewritten to look right above 30 fps; the simulation queues its updates.
- **Cartographer's source** (https://github.com/pnill/cartographer) is the
  closest design to this one, from a read of its structure:
  - campaign always ticks at 30; multiplayer stays at 60 unless the variant
    turns on a 30-tick mode;
  - an interpolator keeps a previous and a target snapshot per object
    (position, forward and up, centre of mass, every node matrix), plus per
    player the first-person weapon's nodes, the crouch amount, the camera
    offset and the aim vector;
  - an object that moves too far in one tick snaps instead of blending (a
    teleport guard);
  - effects created during a tick read the interpolated node position, so
    muzzle effects don't trail the gun;
  - bipeds hidden in vehicle seats are still interpolated, for the death
    camera;
  - there is a tick-rate-to-30 ratio for rescaling constants authored at
    30 Hz, for the 60-tick mode.

  It differs from this design in one big way: it runs the game's main loop
  at the display's rate and has the game's own renderer draw the
  interpolated state. That is why it had to rewrite particles, the motion
  sensor and rumble: they run per frame.

### MCC

- A 343 engineer, quoting the June 2020 development update: the original
  games updated at 30 Hz and MCC updates at 60 Hz; formulas tied to the tick
  had to be rescaled (his example is a projectile moving half as far per
  tick); the move exposed physics and projectile bugs that had been hidden;
  rendering can be interpolated but game systems can't
  (https://www.resetera.com/threads/halo-3-for-mcc-is-now-available-on-steam.247510/page-4).
- The April 2019 "60 Hz" change was the network send rate, not the game
  tick (https://www.gamepur.com/news/halo-mcc-update-60hz-server-tick-rate-loading-screen-types).
- **Super bounces in MCC Halo 2 Classic:** MCCBounceEnable patches it back
  to a 30 Hz tick specifically to bring them back
  (https://github.com/Daylonz/MCCBounceEnable). Its author says the
  explanation is their own reading. Players report the old bounces fail
  (https://steamcommunity.com/app/976730/discussions/0/596273816172292901,
  anecdote).
- A removed Steam Workshop mod claimed MCC's Halo 2 campaign AI perception
  and decision timers were never adjusted for the doubled tick, and that
  cutscenes ran at the wrong speed
  (https://steamcommunity.com/sharedfiles/filedetails/?id=3557052360; the
  modder's claim).
- 343's Andrew Schnickel to PC Gamer: code that meant "1/30 s" and code that
  meant "one tick" were only the same at 30 Hz; Halo 4's move produced an
  unkillable sniper jackal; animations authored at 30 Hz look wrong when
  interpolated; first-person camera animation needed its own smoothing
  (https://www.pcgamer.com/halos-developers-explain-what-can-go-wrong-with-unlocked-framerates-in-old-game-ports/,
  https://www.pcgamer.com/inside-the-journey-to-bring-halo-back-to-pc/).
  A write-up of the same interview adds a weapon that wore down faster at
  higher frame rates
  (https://www.player.one/halo-developers-explain-why-unlocked-frame-rate-issue-old-games-133478).
- MCC Halo CE cutscenes render above 60 but their animation still steps at
  30 (https://steamcommunity.com/app/976730/discussions/0/1752402219404579573/,
  anecdote).

### Halo CE PC and Chimera

- Stock Halo PC keeps a 30 Hz tick and has no interpolation. Uncapped, it
  fires checkpoints early and fades the motion sensor wrongly
  (https://www.pcgamingwiki.com/wiki/Halo:_Combat_Evolved).
- A 2017 experiment ran CE at 60 ticks: animations played at double speed,
  some biped animations broke, tick-counting scripts broke, networking broke
  above 35 ticks (https://opencarnage.net/index.php?/topic/6527-halo-in-60-fps/).
  The thread later says Chimera's interpolation made it unnecessary.
- **Chimera** (https://github.com/SnowyMouse/chimera) interpolates objects
  and some effects at a 30 Hz tick, and fixes per-frame bugs: camera shake,
  checkpoint timing, contrails, motion-sensor fade, auto look-centring, name
  and scoreboard fades. Uncapping cutscenes is a separate opt-in, and objects
  jitter in cutscenes if you uncap without interpolation. Older releases had
  levels (rotation for bipeds and vehicles, then all objects and particles)
  and cut interpolation for distant objects to save CPU
  (https://opencarnage.net/index.php?/topic/7987-update-more-optimizations-to-interpolation/).
  Antennas and similar widgets were not interpolated at one point
  (forum anecdote).

### Original Xbox Halo 2

- Grimdoomer's Halo 2 HD patch leaves the frame rate alone; he said patching
  the tick is hard because physics is tied to it
  (https://icode4.coffee/?p=738, snippet; https://github.com/grimdoomer/Halo-2-HD).
- theboy181's "Halo2 60fps" patch is Patreon-only with no write-up
  (https://x.com/theboy181/status/1396695996730462209). Videos show close to
  60 with campaign slowdowns. Whether it raises the tick, and what happens to
  super bounces, is unknown.
- XPSemu (xemu on PS5) ships frame-rate mods for Halo 1 and 2 up to 120 fps,
  method undocumented (https://github.com/ZiZc3/XPSemu/releases).
- ChupathingyCE PR 921 (Halo 1 engine on Xbox,
  https://github.com/ChupathingyCE/chupathingyce/pull/921) keeps the 30 Hz
  tick, eases only the camera toward the next tick each frame with aim read
  from the stick every frame, does not blend objects (to save memory),
  re-enables per-frame code the Xbox build had disabled (HUD timers,
  particles, contrails, camera shake), keeps held-button counting, Doppler
  and the pause lock per tick, and falls back to a steady 30 when a scene
  can't hold 60.

### General technique and emulators

- Glenn Fiedler's "Fix Your Timestep!": fixed simulation step, an
  accumulator, and rendering a blend of the last two states by the leftover
  fraction (https://gafferongames.com/post/fix_your_timestep/). Halo 2's
  budget already has the accumulator.
- RPCS3's first Demon's Souls 60 fps patch doubled game speed because of a
  fixed timestep (https://blog.rpcs3.net/2019/08/12/new-demons-souls-60fps-patch/);
  its vblank list records effects at 2x, projectiles at double speed and
  physics breaking at high rates
  (https://wiki.rpcs3.net/index.php?title=Vblank_compatible_games_list).

**What the record says.** Every approach that raised the tick (Vista, MCC,
the CE experiment) lost super bounces or broke timers, AI, weapons and
animations, and needed per-constant rescaling it never finished. Every
approach that kept 30 Hz and interpolated the picture (Cartographer
campaign, Chimera, ChupathingyCE) kept the game as authored. Their remaining
bugs were particles, contrails, camera shake, HUD and motion-sensor fades,
the first-person weapon and camera, cutscene animation, teleports and
effects spawned mid-tick. Most of those came from running per-frame guest
code more often, which this design avoids by not running it more often.

## 3. Recommended design

### Options

| option | simulation | super bounces | guest CPU | quality | verdict |
|---|---|---|---|---|---|
| **A. Faster tick** (60 or 120 Hz, the Vista/MCC way) | changes | lost (Vista, MCC) | 2-4x | native | rejected |
| **B. Guest frame interval 1** (the `0x00485aca` word) | per-tick code unchanged, but input polls and per-frame code run twice per tick | at risk: input-to-tick phase changes | ~2x render | duplicate frames unless the renderer interpolates; caps at 60 | rejected |
| **C. Guest render sub-frames** (Cartographer: write interpolated transforms into guest memory, run the guest renderer, restore) | unchanged if every side effect is undone | kept, if isolation holds | +render cost per presented frame | best: the game culls, sorts and spawns effects at the interpolated camera | fallback |
| **D. Host replay with interpolated matrices** (this design) | unchanged by construction | kept | none | camera and object motion smooth; particles move at 30 Hz | **recommended** |
| E. Image-space frame generation (reprojection with depth and motion) | unchanged | kept | none | disocclusion smears, HUD and transparency artefacts | not worth it |
| F. Extrapolation (render ahead of the newest tick) | unchanged | kept, but drawn wrong | none | no added latency, but overshoots on every sudden change: landings, collisions, the bounce itself | camera aim only |

C is what a mature PC mod does, and it renders more correctly than D. It
loses here for two reasons. Guest CPU is our bottleneck: a level frame is
82% guest-side today, and C pays the guest's render path once per presented
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
  for offline replay (`crates/boot/src/d3d_diag.rs`), so replaying a frame
  is not new.
- The presenter keeps two frames, N-1 and N, each stamped with the tick
  count `game_time + 8` read at the present hook. Reading guest memory from
  Rust is uncharged and does not touch the schedule.
- For a display frame due at wall time `t`, map `t` to guest time through
  the pacer's origin, subtract one tick (the interpolation delay), and
  compute `alpha = (that - T(N-1)) / (T(N) - T(N-1))`, clamped to [0, 1]. A
  guest frame that ran no tick (pause, load) has nothing to interpolate. One
  that ran two (catch-up) interpolates over two ticks.
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

1. **Camera** (phase 2). Read each view's camera from the view records that
   `render_game_views` `0x002234e0` builds, by observing, not writing. Find
   which constants hold that view's view and projection by rebuilding them on
   the host and checking they match the uploaded words exactly. Interpolate
   position linearly, orientation by quaternion slerp, and field of view
   linearly. Rebuild the constants and substitute them in every draw of that
   view. World-space data (BSP, decals, particles, contrails) then renders
   correctly from the in-between camera. Particles only move on their own at
   30 Hz.
2. **Objects** (phase 3). Tag each draw with the object being drawn by
   observing the guest's per-object render entry (still to be found; the
   object render cache at `0x0003d270` is the place to start). For each
   matrix constant: if it holds view-projection times world, recover the
   world part with the tick's view-projection inverse. Match it to the same
   object and node in frame N-1, interpolate translation, rotation and scale
   separately, and rebuild the constant with the in-between camera. Skinned
   draws do this per node. A hypothesis worth checking first: if the game
   passes the setter a pointer into an object's node-matrix block, the guest
   address alone identifies the object and node.
3. **First-person weapon and hands.** They are drawn with their own
   projection (commit 1a5aa8d). Interpolate their nodes in camera space, so
   they stay locked to the in-between camera instead of swimming.
4. **Shadows** (phase 3). Shadow-map passes are render-to-texture passes
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
  flares, visibility) and RTT write-backs happen once per guest frame, as
  now.

#### Care list

| thing | at 30 Hz with D | fix, if wanted |
|---|---|---|
| particles, contrails, beams (vertices built on the CPU) | world-anchored, so correct under camera motion; their own motion steps at 30 Hz; billboards face the tick camera | match particle draws across frames and interpolate positions (later, optional) |
| LOD and visibility culling | chosen for tick N's camera; things just off-screen can pop at the edges for a tick during fast turns | accept; or C for those draws |
| render-to-texture effects (water, active camo, screen effects) | sample frame N's textures, up to a tick old | accept |
| animations authored at 30 Hz | node interpolation smooths them; deliberate pops get blended (MCC's note) | snap per object on large node jumps |
| HUD, motion sensor, timers, text | 30 Hz | accept: they are 30 Hz on the Xbox |
| camera shake and screen effects | 30 Hz | accept |
| cutscene cuts | must snap | cut detection above |
| sound, rumble | unchanged: the guest runs them per guest frame | nothing to do |

#### Input latency and aim

Interpolation adds about one tick of latency (up to 33 ms, half that on
average). Aim is where players feel it. Two answers, in order:

- First ship without it. The 30 Hz Xbox already has this latency for a
  rendered tick.
- Then optionally add **visual aim prediction**. Rotate the render camera by
  the yaw and pitch the next tick will apply, computed from the live stick
  or mouse with the game's own look-rate rule. That needs reading how
  `0x001870a0` turns the stick into a turn rate, including any acceleration.
  It's a rotation of the drawn camera only. The guest still reads the same
  input at the same poll, and the next tick's real aim replaces the
  prediction. A mismatch shows as a small correction, never as a simulation
  change. Mouse look already becomes stick deflection per 1/60 s
  ([input.md](input.md)), which keeps the rule a function of input alone.

#### Render thread, frame budget, VRR

- **Where it runs.** The replay belongs on the render thread (`d3d::ThreadedBackend`),
  after the backend has recorded the guest's frame. Encoding the guest's
  frame stays as now. Replays reuse the recorded passes and pipelines and
  change only uniforms: record the frame once into render bundles (or
  equivalent pass lists) whose uniform buffers are rewritten for each
  replayed frame. The render thread is required, not optional: replays on
  the guest's thread would put host GPU work back on the critical path that
  render-thread.md took it off.
- **Budget.** At 120 Hz a replay gets 8.3 ms. Today the guest's encode of
  about 300 draws is 13.6 ms of CPU ([frame-budget.md](frame-budget.md)),
  far too slow to repeat four times per tick. So replays must not re-encode
  from scratch: per replay, about 300 uniform updates (a few hundred KB) and
  one bundle execution. That target needs measuring before phase 2 can
  promise 120.
- **Precondition.** The guest must hold 30 Hz first. Below that the pacer
  rebases and the game plays in slow motion; interpolating slow motion helps
  nothing. If the guest misses a frame, the presenter holds at alpha 1 for
  that span rather than guessing.
- **Present modes.** Today the window uses `AutoVsync` (Fifo). For a fixed
  60 or 120 Hz display: render one replay per vsync, with alpha computed for
  the predicted display time, not the submit time. "60 fps" on a 120 Hz
  panel means a replay every other vsync. For **VRR** (ProMotion,
  FreeSync, G-Sync): present as soon as each replay is ready, capped by the
  setting, with alpha for the time it is predicted to appear. wgpu doesn't
  report presentation timestamps, so predict from the last frame
  intervals; Metal's display-link timing is the better source on macOS.
  Jitter in that prediction shows as judder, so it is the thing to measure.
- **Settings.** The toggle is the `fps = 30 | 60 | 120` entry the
  settings-menu design reserves (`docs/settings-menu.md`, on the
  `doc-settings-keyboard` branch when this was written). 30 means off: no
  replay, identical to today. Built as `H2_FPS` / `h2 play --fps`
  (section 6), for now without replay.

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
element of a collision model ([collision.md](collision.md)), the Havok BSP
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

Design D changes neither. The guest runs every instruction it runs today, on
the same logical schedule, and polls input on the same frames. So the
bounce, the positions it passes through and its timing are bit-identical.
The test below checks exactly that, instead of trusting the argument.

### Regression test

**Input recording keyed to ticks.** Add a recorder next to
`input::backend::scripted` that logs every XInput packet the guest actually
read, with the logical time, vblank count and `game_time + 8` at the read.
A human does a bounce in `h2 play` with recording on. That produces a
timeline that replays deterministically, because the logical clock is the
same with the same inputs. The recording is ours (button and stick values
over time), not game data, so it can be committed. The expected game-state
values are derived from the disc and are never committed: the test compares
runs with each other.

**The test** (an ignored retail acceptance test, like
[checkpoints.md](checkpoints.md)), headless, through `GuestHost::headless`:

1. Load the map. Start the recording at its epoch.
2. Run with the mode off. At the end of every `game_tick` (an observing
   wrapper on `0x00137fe0`), record a hash of guest RAM and the local
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

**Where to run it.** Halopedia's spots are multiplayer maps, and only The
Heretic is reachable today. So the bounce test waits for a local
multiplayer game on Lockout (or the first campaign bounce someone finds once
the Armory loads). The same harness can start now, without a bounce:
step 4 on the scripted run into The Heretic (frame-budget.md's input
string) proves "mode on changes nothing the guest can see" over 300 level
frames, before any bounce exists to record.

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
| 0 | **Prerequisite, not part of this:** levels at a steady 30 fps; render thread on by default | (frame-budget.md) | worst frame under 33 ms |
| 1 | **Built** (section 6): determinism harness: per-tick guest-RAM hash, tick-keyed input recorder, run comparison, fake display clock | 1-2 weeks | two mode-off runs match; Heretic run matches with a replay that draws frame N unchanged |
| 2 | Camera interpolation: two-frame store, alpha from the pacer, view constants rebuilt and verified, cut detection, replay via bundles on the render thread, 60/120/VRR pacing, the setting | 2-4 weeks | identical hashes at 30/60/120; replay cost under 8 ms at 120 |
| 3 | Objects: draw tagging by object, world recovery, node and skinning interpolation, first-person weapon, teleport snaps, shadows | 3-6 weeks | same; visual check in a level with moving actors |
| 4 | Polish: particle and contrail matching (optional), visual aim prediction, cutscene checks | 2-3 weeks | same |
| 5 | Super-bounce regression test, once a bounce map is reachable | 1 week plus the recording | section 4's test |

Risks, most likely first:

1. **Constant classification.** If matrices reach constants in forms that are
   hard to recover (pre-multiplied with extra terms, packed, or built in the
   vertex program from other data), the verified-rebuild rule leaves those
   draws at 30 Hz. It degrades safely, but the result may look patchy until
   the guest's render paths are mapped. Phase 2's camera pass is the cheap
   probe: if view constants can't be matched, rethink before phase 3.
2. **Replay cost.** Per-replay cost has to be a fraction of today's encode.
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
   happens next.

What must not change in any phase: the vblank source rate, the refresh word,
the frame-interval word, the tick rate, the number of main-loop iterations
per tick, the input poll cadence, and anything the guest can read. Every
phase is gated on the per-tick hash test from phase 1.

## 6. What is built (phase 1, and the setting)

### The harness

- **`crates/lockstep`.** `hash_guest` hashes the physical RAM that is in use
  (pages mapped by any virtual range or held by a reservation), one word per
  MiB plus one over the lot. It reads RAM from Rust through
  `Guest::physical_ram`, so it is uncharged and writes nothing. `Recorder`
  collects `Record`s (kind tick or present, sequence, logical clock, the
  guest's tick count from `game_time + 8`, the hash) into a `Trace`, which
  has a text form (`# h2 lockstep trace v1`, one line per record).
  `compare` names the first differing record and the MiB of RAM that
  differ, or says one run is a prefix of the other.
- **Tick observer.** `install_tick_observer` post-hooks the tick function
  (`TickHook::HALO2`: `game_tick` `0x00137fe0`, game-time pointer
  `0x00510c54`, shadow page `0xa0ff9000`), the same way Live telemetry
  wraps it. The original runs unchanged; the hash is taken when it returns.
  The hook adds dispatch transfers and holds a continuation token in the
  return slot during the tick, so **compare runs that all have it on**.
  It refuses a tick that something else already overrides (telemetry, when
  Live is on). Present records are taken between scheduler slices and
  change nothing in the run.
- **Input, keyed by read.** `xapi::tap` puts an `InputTap` on every
  `XInputGetState`: `Recorder` logs the status and the 22 packet bytes the
  guest got (stamped with the tick count), `Replayer` answers read *n* with
  the log's read *n*. Keying by read rather than by time makes replay exact
  whenever the host pumped input. The log (`# h2 input log v1`) is our
  button and stick values, not game data. Only `XInputGetState` is
  tapped, so record and replay over the same kind of always-connected pad.
- **Fake display clock.** `app::fps::FakeDisplayClock` steps a fixed amount
  per read, so a presenter "at 120" is reproducible in tests.

### Using it on a headless run

```sh
H2_INPUT_RECORD=$TMPDIR/in.log H2_LOCKSTEP=$TMPDIR/a.trace h2 run <disc> ...
H2_INPUT_REPLAY=$TMPDIR/in.log H2_LOCKSTEP=$TMPDIR/b.trace h2 run <disc> ...
h2 lockstep $TMPDIR/a.trace $TMPDIR/b.trace   # exit 0 identical, 1 differ
```

`H2_LOCKSTEP_TICKS=0` keeps only the present records. Record and replay are
exclusive. Wiring is `crates/boot/src/lockstep_glue.rs`; the report prints
one `lockstep:` line (records written, reads replayed, reads past the end,
handle mismatches). `h2 play` takes the same variables. Retail runs of this
have not been made yet (no game runs in this lane): the first job for
whoever has the disc is the two-run control above on the Heretic script.

### The setting

`H2_FPS=30|60|120|uncapped`, or `h2 play --fps`, sets
`AppConfig::fps` (`app::fps::FpsTarget`). With neither, the Video screen's
Frame Rate row decides (saved as `[graphics] fps` in `<data>/settings.toml`;
docs/settings-menu.md "Video screen"). A press there applies at the window's
next turn: `app::fps::FpsControl` carries it, the `DisplaySchedule` is
retargeted and, to or from uncapped, the surface's vsync is configured
again (`app::fps::follow`). It reaches only the window:

| setting | window |
|---|---|
| `30` (default) | a redraw per guest present, as before; the code path is unchanged |
| `60`, `120` | also redraws on its own `DisplaySchedule` at that rate (late redraws skip slots, never burst); `vsync` as set |
| `uncapped` | redraws every event-loop turn, vsync off |

Between guest presents a redraw shows the newest guest frame again, so
**above 30 nothing looks smoother yet**: that is phase 2. `app::fps::alpha`
is phase 2's interpolation fraction (clamped, never extrapolates, 1 for a
frame that ran no tick), tested but not used.

### Tests

- `cargo test -p lockstep`: the hash sees any changed word and a swap of
  two, names the changed MiB; traces round-trip through text; compare finds
  the first divergence and truncation; the clock counts as guest state. On
  the toy guest (`lockstep::synthetic`: a generated tick handler, a
  game-time record, an `XINPUT_STATE` input and a crouched-landing-on-a-seam
  "super bounce"): equal inputs give equal traces; a replay reproduces the
  recording whatever the live pad does; releasing crouch on the landing
  read diverges exactly at that tick and loses the bounce; changing an
  ignored packet byte is still caught; the observer refuses a wrapped tick.
- `cargo test -p xapi`: the tap's log holds exactly the bytes the guest got,
  replay gives them back over a different live pad, and a recorded failure
  status comes back as the call's error.
- `cargo test -p app`: `H2_FPS` parsing and its default; 30 is never due on
  the schedule; 60 and 120 redraw 60 and 120 times a second on the fake
  clock; a late window skips slots; alpha. `tests/fps_lockstep.rs` runs the
  toy guest through the shell's triple buffer with a presenter on the fake
  clock at all four settings: every guest frame reaches the screen, redraws
  per guest frame are 1, 2, 4 and over 30, and the guest's trace is
  identical at every setting. A control shows the check would catch a
  guest that ran one extra tick.
- `cargo test -p boot --lib lockstep`: the toy guest through boot's wiring
  writes traces that compare identical, or diverge at the changed tick.

### Next

1. With the disc: the two-run control on the Heretic script, ticks on and
   off, then the same at `H2_FPS=120` under `h2 play` (the trace must
   match). Any mismatch with the mode off is a runtime determinism bug to
   fix before phase 2.
2. Phase 2: the two-frame store and per-view camera classification in d3d
   (draws tagged by the view constants they use, verified bit-exact against
   constants rebuilt from the observed camera), behind `FpsTarget` above 30,
   then the bundle replay on the render thread with `alpha`.
3. A per-present hash on the render-thread side, once replays exist, to
   show the replay path leaves `Device` state unchanged.
