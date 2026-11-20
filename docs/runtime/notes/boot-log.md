# Boot log

Evidence for [BOOT.md](../BOOT.md): how far the boot got and what stopped it, in the order it was found.

## 2026-11-20: the milestone log, the menu benchmark and the retired stopgaps

Read at commit bc48d2053 from the page this note was split out of. Entries are as they were written on that date; the numbers are from the master commits they name (fd3e969 and earlier), not from the current tree. To reproduce a run, use the recipe in [BOOT.md](../BOOT.md) "Measuring the idle menu".

### Current state

**The main menu renders and takes input, and the campaign starts.** On
master, with no overlay:

- the loading screen, the 3D title scene (the Covenant ship over New
  Mombasa), the HALO 2 logo and "PRESS START TO CONTINUE" render, with the
  menu's blue grade;
- scripted input takes START, CHOOSE PROFILE, the main menu, CAMPAIGN and
  SELECT LEVEL, and loads The Heretic (`00a_introduction`);
- the level renders past 8,000 presents, shadow maps included: one run
  there took 50,207 shadow-map samples and skipped none.

Preemption makes slices short, so raise `--slices` for long runs. The
present numbers below are from master fd3e969 with the default script
timing: the title screen is at about present 300, CHOOSE PROFILE about
400, the main menu about 480 (21,000 slices).

Screenshots are kept in the issue threads linked from the README, not in
the repo.

Rendering notes from the menu frame (present 480):

- dotted dark horizontal lines across the screen at a few heights are the
  menu's own scrolling glyph strips, drawn as authored. In frame 480 they
  are draws 1439-1481 of 1578 (after the full-screen grade at 1437): 43
  `Begin/End` quads in pairs (the second of each pair wraps the scroll),
  each a 1024-wide DXT3 strip 4, 8 or 16 texels tall, `SrcAlpha/InvSrcAlpha`,
  with dark blue texels at alpha 2/15 to 4/15. With the blue grade (d5f82c6)
  they sit on blue;
- the CAMPAIGN OPTIONS highlight bar draws as horizontal stripes, not
  noise: draws 985-986 of frame 570, a 200x21 `LinA8R8G8B8` texture
  (pitch 832) written once by the game when the screen opens, light blue
  with alpha alternating 52 and 36 every two rows. The same with
  `H2_BOOT_UPLOAD_ALWAYS` set, so no CPU write is missed.

Unguarded switch targets the analyzer still doesn't read go through the
fallback (blocker 1): about 45k fallback entries by the main menu.

#### Milestones, from the entry point

1. Entry `0x2d0aee` and the XAPI startup (`xapi::register`).
2. XMountUtilityDrive (Partition 5 cache, `CACHE_ROW`).
3. CRT static init (its x87 is lifted now; it ran in the fallback first).
4. Game main:
   - AV pack, video and the push buffer;
   - `Direct3D_CreateDevice`, with the NV2A base stand-in and the
     back/depth backing from the boot;
   - the first Swap.
5. The world pool at physical `0x61000` (`LoadOptions::halo2()`, mapped at
   `0x80061000` and `0xf0061000`).
6. The second thread, the mutants and the fonts.
7. Cache files on Z: and N: (Partition4), the save games and `sgf`.
8. XcSHA, XInitDevices and the D3D state setup.
9. XHV voice engine passes `0x33337d` (DSound round 2).
10. Network: PhyInitialize and the XNet 20 ms timer DPC on thread 0.
11. `cpuid` at `0x34881d` (lifted now; emulated in the fallback first).
12. The `d:\bink\intro_60.bik` probe and the movie loop `0x1639c2`.
    SwitchToThread hands off since kernel round 5.
13. `d:\maps\mainmenu.map` is opened on thread 3. The stream purge
    callback `0x213fa0` waits for the read (preempted since backedge).
14. Loading screen: the unbounded switch tables `0x216a54` and `0x216b01`
    run through the fallback. The "LOADING nn%" frames render; `t:\$C\`
    and `u:\` are probed.
15. Offscreen render targets: render-to-texture and the same-size
    SetRenderTarget fast path `0x3f7b30` through crates/d3d.
16. Inline draws through BeginPush and SetStateVB (vertex array formats,
    d3d-r6).
17. Unaligned memcpy/memmove and the zero-holed table at `0x336f0`
    through the fallback (blocker 1).
18. The title screen, then START (scripted), CHOOSE PROFILE, A on
    Default, and the main menu.

19. With a frame-timed script (`START@f310,DOWN@f408,A@f419,A@f524,A@f585,
    A@f632`): CAMPAIGN, CAMPAIGN OPTIONS (New Campaign), then PROFILE NAME
    (the Default profile cannot save, so the game asks for a new one), DONE.
    Saving the new profile used to stop on `NtCreateFile` of
    `u:\<id>\SaveMeta.xbx` with an AllocationSize; the kernel now treats
    it as a hint on backends that cannot reserve (b906763).

    With fresh partitions the Default profile is accepted and no profile
    name is asked for: `START@f310,DOWN@f408,A@f419,A@f524,A@f585,A@f632,
    A@f700,A@f760` goes CAMPAIGN, CAMPAIGN OPTIONS, SELECT LEVEL (The
    Heretic), difficulty, then loads `maps\00a_introduction.map`
    (loading screen from present ~830). About present 4210 the first
    level frame needs depth render-to-texture, which d3d has had since
    850fe2f; the level then renders past 8,000 presents.

When this was written the main menu ran at about 5-6 fps on an unloaded
M-series Mac (1.9 fps at fd3e969); current in-level numbers are in
[gameplay-budget.md](../../perf/notes/gameplay-budget.md). Audio runs on logical time through a
`NullSink` (its frames are counted as dropped, by design), vblanks on
the 60 Hz logical clock. When every thread waits with nothing timed but
audio is still playing, the runner moves logical time on one audio
quantum (5.3 ms) at a time (`dsound_glue::wake_idle`, at most 10 s with
no thread running) instead of stopping with a deadlock.

#### Menu idle benchmark

For performance work: the main menu, idle. Fresh data
makes every run take the same path; the script picks the Default profile
and leaves the main menu up, then the run stops on its own.

```
T=$TMPDIR/h2bench; OUT=$TMPDIR/h2boot
cargo build --release -p cli            # target/release/h2
# Once per generator (h2 run rebuilds a dylib whose .stamp doesn't match):
target/release/h2 build disc/halo2.iso --out $OUT
H2_BOOT_GFX=$T/gfx H2_BOOT_GFX_EVERY=never \
H2_BOOT_INPUT="START@f310,DOWN@f408,A@f419" \
H2_BOOT_FPS_FROM=440 H2_BOOT_STOP_AT_PRESENT=560 \
target/release/h2 run disc/halo2.iso --out $OUT --data $T/data --fresh-data
```

- Window: presents 440-560, the main menu after it has faded in (the
  profile is chosen at 419). The run report's `frames:` line gives fps,
  mean, median and worst frame time over that window, and every 100
  presents the run prints `present N: last 100 at X fps`.
- `H2_BOOT_GFX_EVERY=never` keeps the GPU path but writes no PNGs; drop
  it for a screenshot every N.
- The fallback loads or computes the image analysis on a background
  thread. It reuses the cache from `h2 build` when available, and reads
  the unread-target cache for the current image and analyzer. Boot
  continues on a cache miss. An unsupported instruction or a transfer
  that needs the fallback can still wait for the analysis. The run
  reports waits over 100 ms, and `H2_BOOT_VERBOSE=1` shows when analysis
  finishes. Compare warm runs to avoid competition from cold analysis.
- Numbers are only meaningful on a quiet machine: with heavy builds
  running alongside (load average 170-430) the same run fell to a
  fraction of its quiet-machine rate, and the analysis went from 15 s to
  190 s.
- Profile with `sample <pid> 10` or Instruments once `present 400`
  appears.

Runs compare frame for frame only from the same data directory state:
use `--fresh-data` (or a new `--data`), and never share one between two
runs at once. On master a run that shared the data directory with another
stopped with a divide error at `0x223cff` (a zero stride in loaded data).

#### Process CPU

Inline fast memory won its A/B against the checked accessors; the run-time
switch for that comparison (`H2_BOOT_FASTMEM`) was removed in the
2026-11-20 flag cleanup, and `tests/bridge.rs` still checks both bridges
on one image. Measure with the default merged passes,
`H2_BOOT_GFX_EVERY=never`, input `START@f310,DOWN@f408,A@f419`, and a new
data directory for each run.

`cargo build --release -p boot --example menu_cpu` builds a bounded driver:
`menu_cpu DISC OUT FRESH_DATA_DIR`. It runs boot's normal headless backend,
keeps 4096 trace rows, and stops after present 560. `MENU_CPU` reports wall
FPS, process user+system CPU milliseconds per frame for presents 480–560, and
`guest_thread_cpu_ms_per_frame`, the runner thread's own CPU time (every guest
thread; the backend's work runs on the render thread unless `H2_RENDER_THREAD=0`).
Process CPU includes every process thread; GPU execution and time descheduled are
not CPU time. Samples are taken between scheduler slices at those presents,
so boundaries can differ slightly from the backend's frame timestamps.
Compare CPU as well as FPS when shared-machine load changes. For pixel
fidelity, `H2_GFX_MERGE_CHECK=N` renders frame N both merged and one pass
per draw and reports the differences (docs/graphics/graphics.md, "Merged
passes").

#### Ranked blockers

Sizes are S (under a day), M (a few days) or L (a week or more).

| # | Blocker | Owner | What's needed | Size |
| --- | --- | --- | --- | --- |
| 1 | ~~Holed jump tables~~ | analyze | Read on master. The CRT copy's masked and negative-index tables since bceed11; the unguarded enum tables with long runs of nulls (`0x336f0`, five in a row; `0x33980`, seven; `0x22dcd9`, eight) since an unguarded table placed after its own code may skip up to 16 nulls. `xbe_pins::unguarded_tables_hold_every_case_the_boot_fallback_would_find` pins that the boot's run-time read finds no target the analyzer lacks. | done |
| 2 | **Fog and sky on the title scene** | gfx / shaders | Seen at fd3e969: brown fog over everything, no sky colour, 8x8 blocky patterns. Fog has followed the NV2A fog unit since 5c2845d and 5b8ec7d; recheck before working on it. | ? |
| 3 | **RTT write-back replayed as a CPU write** | d3d (runtime_glue) or runtime | The barrier reports the device's own write-back pages at the next entry, so an unchanged render texture uploads again. Boot cannot tell them apart from other contexts' stores during the call: d3d's handlers would need to run under `Guest::writes_unreported(id)` for the boot's watch, or the device would need to report its own write ranges. Performance only. | S |
| 4 | ~~x87 not lowered~~ | lift / emit | Lowered on master (e5c4251). The boot dylib links emit's x87 bridge, and the fallback's x87 runs on the same crates/x87 abi helpers. | done |

#### Retired

- backedge (a7ca4f5) and d3d-r6 (a41dae4): the `0x213fa0` stream purge
  wait; the SetRenderTarget fast path `0x3f7b30`; Get2DSurfaceDesc,
  BeginPush and SetStateVB as retail bodies (`RUN_NATIVE`); the
  device+0x934 NV2A stand-in (now `DeviceConfig::nv2a_window`).

- Lift leftovers (0775c97): the fallback's `cpuid` and `rdtsc` emulation
  and the unknown-switch-target path. `0x216a54` and `0x216b01` are lifted
  now. The boot bridge forwards `h2shim_v1_rdtsc` to the runtime's guest
  TSC, and the interpreter's TSC is seeded from it at each fallback entry.

- Kernel round 5 (2a2b197):
  - NtYieldExecution as a 1 ms delay (the yield starvation is fixed, and
    the movie loop `0x1639c2` still progresses);
  - AvSendTVEncoderOption 0xb.
- D3D round 4 (f5f7390 and earlier):
  - the fragment no-ops `0x3f6190`, `0x3f6390`, `0x3f6840` and `0x3f6f40`;
  - SetSoftDisplayFilter `0x3f8c70`;
  - the boot's `0x12b2e0` frame wait (now `d3d::register_frame_wait`);
  - the `0x3fb710` resource busy check and `0x3f8cc0` (SetStipple,
    method `0x1480`), both implemented by crates/d3d;
  - the SetRenderTarget stopgap (render-to-texture is in crates/d3d; its
    `H2_BOOT_RTT` switch is gone).

Master covers these now. The kernel stopgap table installs nothing,
because `Kernel::stopped_ordinals()` is empty for every entry.

- Kernel rounds 3 and 4 retired every entry in the stopgap table:
  - ExQueryNonVolatileSetting;
  - HalRegisterShutdownNotification;
  - KeInitializeDpc, KeInitializeTimerEx, KeSetTimer(Ex) and
    KeCancelTimer (DPCs now run);
  - KeStallExecutionProcessor and KeSetBasePriorityThread;
  - Ke Save/Restore FloatingPointState;
  - RtlCompareMemoryUlong;
  - PhyGetLinkState and PhyInitialize (252/253);
  - MmCreate/DeleteKernelStack;
  - ObReferenceObjectByHandle and ObfDereferenceObject;
  - MmSetAddressProtect (182);
  - XcSHAInit, XcSHAUpdate and XcSHAFinal (335-337);
  - MmPersistContiguousMemory and NtOpenSymbolicLinkObject.
- Also retired by kernel round 3 (the code, kept for a while behind
  `H2_BOOT_OLD_GLUE`, was deleted in the 2026-11-20 flag cleanup):
  - the mutant semaphores (192/221);
  - the root -3 and FILE_SYNCHRONOUS_IO_ALERT rewrite;
  - EEPROM XC_MAX_ALL;
  - MEM_TOP_DOWN;
  - FileAllocationInformation and NtQueryFullAttributesFile;
  - MEM_RELEASE.
- The widened arena: `LoadOptions::halo2()` reserves
  [0x61000, 0x3145000).
- XAPI round 2: the N: interim override and `0x2d5d0a`
  (VfsStorage + `CACHE_ROW`; N: mounts as Partition4).
- DSound round 2: the `0x55810` voice-off. XHV now passes `0x33337d`.
  The override (`game_glue.rs`) was deleted with the rest of the retired
  glue in the 2026-11-20 flag cleanup.
- Before these rounds: the class-34 wrapper, XeLoad/UnloadSection, the
  boot XMountUtilityDrive (back with `--no-xapi`), and the deletion of the
  pre-created T:/U:/Z: links.
