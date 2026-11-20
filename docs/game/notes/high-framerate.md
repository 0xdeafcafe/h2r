# 60 and 120 fps: evidence

Evidence for [high-framerate.md](../high-framerate.md). Nothing here states what the code does; the design page does.

Newest first.

## Hold log of a gameplay serve

Measured at `b0e5f8fc3` (2026-11-18), headless `h2 serve` on level 03a_oldmombasa with `H2_FPS=60` and `impl.render.reproject`, read with `h2 agent render` (`interp.holds`).

Before the screen-space rule in [high-framerate.md](../high-framerate.md) ("Screen-space draws in the world"), 5,161 frames were held by the screen guard, 4,353 of them counted as `mismatch_screen` (the rest also unpaired). Every one held on a pass-through world draw (`mov oPos, v0`: its vertices are window pixels). Three kinds:

| kind | in held frames | what it is |
|---|---|---|
| full-target layer | first offender in 5,041 of 5,161; 1 a frame | one blended triangle fan over the whole 640x480 target among the world draws, four textures: a 640x480 target drawn that frame and three small noise sheets |
| sprites | in 1,200 frames, first in 120; about 144 a frame (172,498 draws) | blended 6x6-pixel fans with one 8x8 compressed texture, among the opaque world draws, with the depth surface bound: points of the world the game projected to the screen with the frame's camera |
| orthographic quads | 2 frames (4 and 8 draws) | an untextured full-target fan and a 640x60 strip: a fill and letterbox bars, in short frames |

Reproduce: the serve command in the design page's "Headless" section, then read `interp.holds.kinds`.

## Camera pick on a converted Halo 1 gameplay capture

Measured at `b0e5f8fc3` (2026-11-18), one captured gameplay frame of a converted Halo 1 level (307 draws into the shown surface).

Every draw carried a block of packed parameters that splits as a perspective far off centre, shared by all 307 draws, while the camera was in 300. A rule that took the most-shared perspective-looking block picked the packed block, so every gameplay frame was held (`unsure`, `camera`) and E1 blended nothing. The design reads the block each draw's program computes its position from instead (`interp::epilogue::position`). With that, the camera is `c[0]`, centred, in 300 draws and the HUD's draws are all overlay. The frame would still hold on `screen`: draw 203 of 256 world draws is a full-target quad in pixels (a pass-through program, point-sampled 1:1 from a 640x480 copy of the screen made that frame, blended over the world).

## Encode budget the replay is held against

The figures are in [gameplay-budget.md](../../perf/notes/gameplay-budget.md) ("Buffer count and per-pass bookkeeping", measured 2026-11-17): about 17.5 ms of encode and submit for about 1000 draws in 193 passes (6.3 + 11.2 ms), 27.8 ms at 2000 draws, and a render pass costs about 40 to 75 microseconds of bookkeeping on its own. A replay that re-encodes the back-buffer draws costs the same per draw. The design page's "Render thread, frame budget, VRR" says what that means for 60 and 120.

## Prior art

Read during research, 2026-11-15 to 2026-11-18, at `42a1c068e`, from the linked pages. Reproduce by opening the links; those marked "snippet" answered 403 and the finding comes from the search result text only. Paraphrased; no code was copied.

#### Halo 2 Vista (2007) and its community

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

#### MCC

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

#### Halo CE PC and Chimera

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

#### Original Xbox Halo 2

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

#### General technique and emulators

- Glenn Fiedler's "Fix Your Timestep!": fixed simulation step, an
  accumulator, and rendering a blend of the last two states by the leftover
  fraction (https://gafferongames.com/post/fix_your_timestep/). Halo 2's
  budget already has the accumulator.
- RPCS3's first Demon's Souls 60 fps patch doubled game speed because of a
  fixed timestep (https://blog.rpcs3.net/2019/08/12/new-demons-souls-60fps-patch/);
  its vblank list records effects at 2x, projectiles at double speed and
  physics breaking at high rates
  (https://wiki.rpcs3.net/index.php?title=Vblank_compatible_games_list).
