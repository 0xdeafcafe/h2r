# Booting the recompiled XBE natively

`h2 build` lifts the whole `default.xbe` and compiles it to a host dylib.
`h2 run` loads that dylib into the runtime with the kernel, VFS, XAPI, D3D,
DSound and Bink HLE registered, then starts the guest at the XBE entry
point. It started as a tracer bullet to find, in order, what stopped Halo 2
on its way to the main menu. It runs headless; `h2 play` ([app.md](app.md))
runs the same boot in a window with live input and sound.

## Running it

```sh
# lift + emit + clang the whole XBE (about 6-7 min of clang, about 400 MB of C)
cargo h2 build /abs/path/to/disc/halo2.iso

# boot it (runs `build` first if the dylib is missing or stale)
cargo h2 run /abs/path/to/disc/halo2.iso
```

Pass the disc as an absolute path to the real ISO, with no symlinks or
copies. Nothing is ever written next to it.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--out DIR` | `$TMPDIR/h2boot` | generated C (`src/`), objects, `libh2game.dylib` and its `.stamp` (image hash, generator source hash, IR passes, `H2_ACCESS_COUNT`, ABI, compiler flags: `run` rebuilds a dylib stamped for another image or generator), `kernel-trace.tsv` |
| `--data DIR` | `$TMPDIR/h2boot-data` | VFS host storage (`disk/PartitionN`, `metadata/PartitionN`) |
| `--jobs N` | cores | clang parallelism |
| `--slices N` | 100000 | cooperative scheduler slices before giving up |
| `--transfers N` | 50M | runner transfers per slice |
| `--no-stopgaps` | | run on the bare kernel: no boot stopgaps or wrappers, and no fallback interpreter (an unsupported instruction stops the run where generated code reported it) |
| `--fresh-data` | | empty `<data>/disk` and `<data>/metadata` first; refused, deleting nothing, if the disc, image or `--out` is inside either |
| `--no-d3d`, `--no-dsound`, `--no-bink`, `--no-xapi` | | leave that HLE crate unregistered |

| Environment | Effect |
| --- | --- |
| `H2_FASTMEM=0` | **On by default.** Generated code does inline RAM checks/copies; unsupported accesses use Rust. `0` compiles without them; set it for both `h2 build` and `h2 run`/`play`, since changing compiler flags invalidates the dylib stamp. Requires the current boot bridge. This is the gated inline path, not fault-driven fastmem. |
| `H2_BOOT_FASTMEM=0` | run a fast-memory image through checked Rust accessors, without rebuilding; use for A/B comparisons. Leave `H2_FASTMEM` unset (or `1`) to match that image's stamp. |
| `H2_PERF=1` | compile generated loop/x87 counters; forwarded by boot's versioned bridge to contexts with profiling enabled. Part of the dylib stamp. |
| `H2_X87_NATIVE=0` | **On by default.** Generated code runs range-guarded x87 arithmetic in native binary64 (see [x87-native.md](x87-native.md)). `0` keeps every operation on the `h2x87_v1_*` helpers. A compiler flag: part of the dylib stamp and object cache keys, so set it for both build and run. |
| `H2_ACCESS_COUNT=0` | **On by default.** Generated handlers count their own guest memory accesses and charge them in bulk (see crates/emit/RUNTIME-CONTRACT.md). `0` lets the runtime charge each accessor call. It changes the C: part of the dylib stamp and the emit cache key, as `H2_PASSES` is. |
| `H2_BOOT_VERBOSE=1` | log every boot note (stopgap decisions, fallback entries) |
| `H2_BOOT_MAPS=1` | dump the guest memory map in the report |
| `H2_BOOT_OLD_GLUE=1` | re-enable the boot glue master now covers (the wrappers for alertable/root -3, EEPROM, mutants, MEM_TOP_DOWN, allocation, full attributes and MEM_RELEASE; the 0x55810 voice-off) |
| `H2_BOOT_VOICE=1` | with `H2_BOOT_OLD_GLUE`, keep voice on (skip the 0x55810 override) |
| `H2_BOOT_GFX=<dir>` | run D3D on a headless `GfxBackend` with the nv2a shaders and write presented frames to `<dir>/frame-NNNNN.png` |
| `H2_BOOT_UPLOAD_ALWAYS=1` | D3D re-reads every resource on every use, instead of trusting the write barrier (`UploadPolicy::Tracked`) |
| `H2_BOOT_GFX_EVERY=N` | with `H2_BOOT_GFX`, keep the first frame and every Nth (default 30); `never` renders on the GPU but writes no PNGs |
| `H2_GFX_MERGE_PASSES=0` | with `H2_BOOT_GFX` or in `h2 play`, one render pass per draw instead of the default merged passes (`GfxBackend::set_merge_passes`); about 2.5x slower; the exact reference, which merged passes matched on every menu frame checked |
| `H2_GFX_MERGE_CHECK=N` | with `H2_BOOT_GFX`, render each pending pass of frame N both merged and one pass per draw and print what differs, down to the draws involved (`GfxBackend::set_merge_check`; slow) |
| `H2_BOOT_STOP_AT_PRESENT=N` | end the run once N frames are presented |
| `H2_PIPELINE_CACHE=<file>` | the persistent pipeline cache (docs/graphics.md "Pipeline compiles"): pipelines recorded there are built on worker threads at startup; `off` disables it. Default `~/Library/Caches/h2recomp/pipelines-v1.jsonl` |
| `H2_GFX_COMPILE_THREADS=N` | pipeline compile workers (default half the cores, 2-4; `0` builds each pipeline on the game thread when its pass is encoded) |
| `H2_FRAME_STATS=1` | frame-time stats by phase: a line a second on stderr and the worst frames at the end (docs/app.md "Frame stats", docs/frame-budget.md); `H2_FRAME_STATS_CSV=<file>` writes every frame, `H2_FRAME_STATS_FROM=N` starts the end report at frame N |
| `H2_PERF_FROM=N` | with an `h2` built with `--features perf`: open `perf::Session::from_env()` (`H2_PROFILE`, `H2_TIMELINE`) at present N, finished at the end of the run (docs/perf.md "Boot hook") |
| `H2_BOOT_FPS_FROM=N` | measure the report's fps window from present N (default: the last 100) |
| `H2_BOOT_REALTIME=1` | pace the scheduler to wall-clock time (`Pacer::real_time`); off by default, so runs go as fast as the host allows |
| `H2_BOOT_CACHE=<dir>` or `off` | the boot cache (default `$TMPDIR/h2boot-cache`), shared by every `--out`: the fallback's unread jump-table targets, and the image's analysis (`analysis-<hash>-<size>-<analyzer>.h2a`), keyed on the XBE hash and size and on `ANALYSIS_SOURCE_HASH` (xbe and analyze sources, `Cargo.lock`, rustc version). `h2 build` writes the analysis and the fallback reads it at run time; `H2_TRACE` skips it. `off` caches nothing |
| `H2_BOOT_INPUT=BUTTONS@WHEN,...` | scripted pad timeline through the keyboard pad and XInputGetState. BUTTONS: `A B X Y BLACK WHITE LT RT UP DOWN LEFT RIGHT START BACK LS RS LUP LDOWN LLEFT LRIGHT` or a key name, joined with `\|`. WHEN: `T`, `T+D` or `T-U`, a slice or (`f` prefix) a present count; held 4 presents or 100 slices by default. Each press is printed. |
| `H2_BOOT_INPUT_FILE=<file>` | the same entries, one per line, `#` comments; merged with `H2_BOOT_INPUT` |
| `H2_BOOT_CAPTURE=<dir>`, `H2_BOOT_CAPTURE_FRAME=N` | record frame N for offline replay (see `crates/boot/src/d3d_diag.rs`): every texture upload, surface, guest write, and the frame's `ResolvedDraw`s with their streams |
| `H2_BOOT_CAPTURE_SNAP=K[-L],...` | with a capture, read the colour target back after those draws of frame N (`snap-K.bin`, guest BGRA bytes) |
| `H2_BOOT_CAPTURE_SKIP=K[-L],...` | with a capture, drop those draws of frame N |
| `H2_BOOT_CAPTURE_LITE=1` | with a capture, record only the frame's draws, surfaces and snapshots (no uploads or guest writes); for snapshots |
| `H2_BOOT_CAPTURE_TEX=K[-L],...` | with a lite capture, still keep these texture uploads' bytes (`tex-K-L.bin`; ids from `textures.txt` and `frame.txt`, stable across `--fresh-data` runs) |
| `H2_BOOT_DRAWLOG=<file>` | log every backend call of the frames in `H2_BOOT_DRAWLOG_FRAMES` (present counts, default `240`; frame N matches `frame-N.png`) |
| `H2_BOOT_DRAWLOG_VERTS=1` | with `H2_BOOT_DRAWLOG`, add the vertex layout and the first 8 vertices of each stream |
| `H2_BOOT_DUMP=addr:len,...` | print guest words at the end of the run (hex) |
| `H2_BOOT_DUMP_RAW=addr:len:file;...` | save raw guest bytes at the end of the run (hex address and length) |

The run prints a report when it ends. It covers:

- the outcome and the stop, with the last EIP;
- every thread's registers, EBP chain and raw stack;
- the stopgaps that fired, with counts;
- D3D, DSound and XAPI registration and their stats;
- fallback interpreter stats;
- the last 40 kernel calls, decoded.

`kernel-trace.tsv` has one row per kernel import call: seq, thread, ordinal,
name, return address, args, EAX and decoded detail (object names for the
file APIs). Use it to diff against a golden trace (docs/golden-trace.md).

## Hosted runs

`run` is `run_hosted(xbe, dylib, opts, Host::default())`. A host (today
`h2 play`, docs/app.md "Playing") fills in `boot::Host`:

| Field | Instead of | Goes to |
| --- | --- | --- |
| `gfx: Option<d3d::GfxBackend>` | `RecordingBackend` or `H2_BOOT_GFX` | `d3d_glue::install_with` (present hook already attached) |
| `audio_sink: Option<dsound_glue::SinkOpener>` | a `NullSink` | `dsound_glue::install_with_sink`: the same logical-time output, advanced after every slice, playing into the sink the opener returns (called on the output thread; `DeviceSink` is not `Send`) |
| `input: Option<input::InputManager>` | a headless keyboard pad | `xapi_glue::register_with` |
| `keyboard: Option<KeyboardMouse>` | that pad | what `H2_BOOT_INPUT` presses |
| `trace_limit: Option<usize>` | every call in memory | `trace::install_bounded`: the last N calls, and at most N older ones still open (calls that never return, such as thread exits, age out), stay in memory; the TSV still gets every completed call |
| `between_slices: Option<BetweenSlices>` | nothing | called after every slice, input pump and audio advance with the kernel; `false` ends the run with outcome `HostStop` (the output is halted as on any end) |

Everything else (stopgaps, fallback, vblanks, report) is the same in both
modes. `tests/hosted.rs` (ignored; needs `$H2_XBE`, `$H2_DISC`,
`$H2_BOOT_DYLIB`) checks the sink is opened once off the runner thread and
fed on the guest's clock, the callback count, a monotonic clock and
`HostStop`; `trace.rs` tests the bounded trace.

## How it fits together

- **`crates/boot/src/lib.rs`**: `build` and `run`.
  - `build` runs analyze, lifts each function, then `emit::plan` writes
    C11 shards and `h2gen.h`.
  - clang builds the shards for the host architecture, and they are linked
    with `c/boot_bridge.c` into `libh2game.dylib`.
  - The bridge forwards the `h2abi_v1_*` imports to `runtime::ffi`. Rust
    implements the `h2shim_*` imports in `shim.rs`.
  - `run` reads the module's `h2gen_module` tables. Every generated entry
    and resume is registered with one trampoline (`fallback.rs`).
- **`fallback.rs`, `x87.rs`, `sse.rs`**: mixed-mode fallback. `x87.rs`
  does all of its arithmetic, conversions and transcendentals in
  crates/x87 (exact ext80).
  - When generated code stops on an instruction the lifter doesn't lower,
    the trampoline interprets from there with `ir::interp`. This is a
    stopgap like the others: `--no-stopgaps` turns it off.
  - x87 and leftover SSE instructions go to the emulators in `x87.rs`
    (a decoder over crates/x87's `abi` state machine, so stack faults,
    source-format exceptions and condition codes match what generated
    code gets) and `sse.rs`.
  - Control goes back to generated code at the next registered entry or
    resume, call or return.
- **`stopgaps.rs`, `wrappers.rs`**: boot-only kernel behaviour, each a
  named blocker below.
  - A stopgap replaces a kernel export the kernel stops on. It is only
    installed if the ordinal is in `Kernel::stopped_ordinals()`.
  - A wrapper parks the kernel's handler on a shadow page (`0xa0ff1000`),
    fixes up the arguments, then jumps to it.
- **`trace.rs`**: wraps every kernel import token to record the TSV.
- **`d3d_glue.rs`**: `d3d::register` on a headless `RecordingBackend`
  (or `GfxBackend` with `H2_BOOT_GFX`), with `UploadPolicy::Tracked`.
  The write barrier subscribes to the runtime's write watch over all of
  RAM and wraps every d3d entry: on entry, `take_dirty` goes to
  `Device::note_written_pages`. Nothing is drained on return: other contexts can
  store while the call is in flight, so the device's own stores come back
  to it at the next entry instead.
- **`dsound_glue.rs`**: `dsound::register`. Completion events go to
  `Kernel::signal_event`.
- **`xapi_glue.rs`**: `xapi::register` with a kernel-VFS `Storage` and a
  headless `InputManager`.
  - `xapi::service_input` runs between scheduler slices.
  - With `--no-xapi`, the boot keeps its own XMountUtilityDrive override
    (`0x2d5b77`, which writes `xapi::register::addr::CACHE_ROW`).
- **`game_glue.rs`**: boot-only overrides of Halo functions (only the
  retired voice-off, under `H2_BOOT_OLD_GLUE`).
- **`d3d_diag.rs`**: a backend wrapper; the report gains draw outcomes by
  target, the texture formats drawn and device events by reason, and
  `H2_BOOT_DRAWLOG` logs whole frames.

## Current state

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
  `H2_BOOT_UPLOAD_ALWAYS=1`, so no CPU write is missed.

Unguarded switch targets the analyzer still doesn't read go through the
fallback (blocker 1): about 45k fallback entries by the main menu.

### Milestones, from the entry point

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

On current master the main menu runs at about 5-6 fps on an unloaded
M-series Mac (it was 1.9 fps at fd3e969). Audio runs on logical time through a
`NullSink` (its frames are counted as dropped, by design), vblanks on
the 60 Hz logical clock. When every thread waits with nothing timed but
audio is still playing, the runner moves logical time on one audio
quantum (5.3 ms) at a time (`dsound_glue::wake_idle`, at most 10 s with
no thread running) instead of stopping with a deadlock.

### Menu idle benchmark

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
- The fallback reads the image's analysis from the boot cache, where
  `h2 build` left it, so a run after a build doesn't analyze again. Only a
  run whose analyzer has no cached analysis yet (a run with
  `H2_BOOT_CACHE=off`, or a dylib built elsewhere) analyzes the image for
  the fallback (15-20 s alone); later runs read the unread-target cache
  and analyze in the background, so compare second runs.
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

### Accessor A/B with process CPU

Build one image with the defaults (`H2_FASTMEM` on) in a private output
directory. Run the same image twice, one run after the other, setting
`H2_BOOT_FASTMEM=0` for checked access and `1` for inline access. Both use
the default merged passes, `H2_BOOT_GFX_EVERY=never`, and input
`START@f310,DOWN@f408,A@f419`. Leave `H2_FASTMEM` unset during both runs so
the module stamp matches. Use a new data directory for each run. To compare
against an image without the default speedups, build a second one with
`H2_FASTMEM=0 H2_X87_NATIVE=0 H2_ACCESS_COUNT=0` and keep those set for its
run.

`cargo build --release -p boot --example menu_cpu` builds a bounded driver:
`menu_cpu DISC OUT FRESH_DATA_DIR`. It runs boot's normal headless backend,
keeps 4096 trace rows, and stops after present 560. `MENU_CPU` reports wall
FPS and process user+system CPU milliseconds per frame for presents 480–560.
CPU includes every process thread; GPU execution and time descheduled are
not CPU time. Samples are taken between scheduler slices at those presents,
so boundaries can differ slightly from the backend's frame timestamps.
Compare CPU as well as FPS when shared-machine load changes. Pixel-fidelity
acceptance tests can set `H2_GFX_MERGE_PASSES=0` for the exact per-draw
reference; merged passes matched it on every menu frame checked
(docs/graphics.md, "Merged passes").

### Ranked blockers

Sizes are S (under a day), M (a few days) or L (a week or more).

| # | Blocker | Owner | What's needed | Size |
| --- | --- | --- | --- | --- |
| 1 | **Holed jump tables** | analyze | The CRT copy's masked and negative-index tables are read on master (bceed11), and the boot no longer scans them. Still unread: tables with zero entries for impossible cases, where an unguarded read stops (`0x336f0`, caller-checked index; `0x33980`; `0x22dcd9`: 65 site/target pairs on master). The boot reads these from the image and interprets from their targets meanwhile. | S |
| 2 | **Fog and sky on the title scene** | gfx / shaders | Seen at fd3e969: brown fog over everything, no sky colour, 8x8 blocky patterns. Fog has followed the NV2A fog unit since 5c2845d and 5b8ec7d; recheck before working on it. | ? |
| 3 | **RTT write-back replayed as a CPU write** | d3d (runtime_glue) or runtime | The barrier reports the device's own write-back pages at the next entry, so an unchanged render texture uploads again. Boot cannot tell them apart from other contexts' stores during the call: d3d's handlers would need to run under `Guest::writes_unreported(id)` for the boot's watch, or the device would need to report its own write ranges. Performance only. | S |
| 4 | ~~x87 not lowered~~ | lift / emit | Lowered on master (e5c4251). The boot dylib links emit's x87 bridge, and the fallback's x87 runs on the same crates/x87 abi helpers. | done |

### Boot stopgaps in force

| Stopgap | Where | Retire when |
| --- | --- | --- |
| Back/depth surface backing at CreateDevice, `0xf0000000` write-combined alias | d3d_glue.rs `install_back_surfaces`, lib.rs | d3d / runtime |
| Fallback from unread jump-table targets (every unguarded switch's, read from the image), and the unknown-switch stops they cause, allowed only for the site's own table | fallback.rs `unread_targets`, `install_unread_targets` | analyze (#1) |
| Interpreting past instructions the lifter doesn't lower | fallback.rs `trampoline`, `set_recovery` | lift |
| `--no-xapi` only: XMountUtilityDrive `0x2d5b77` | xapi_glue.rs | n/a |

### Retired

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
  - the `H2_BOOT_RTT` SetRenderTarget stopgap (render-to-texture is in
    crates/d3d).

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
- Also retired by kernel round 3, now behind `H2_BOOT_OLD_GLUE`:
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
  The override is kept behind `H2_BOOT_OLD_GLUE`; the proof that it is
  safe is in game_glue.rs.
- Before these rounds: the class-34 wrapper, XeLoad/UnloadSection, the
  boot XMountUtilityDrive (back with `--no-xapi`), and the deletion of the
  pre-created T:/U:/Z: links.
