# Booting the recompiled XBE natively

`h2 build` lifts the whole `default.xbe` and compiles it to a host dylib.
`h2 run` loads that dylib into the runtime with the kernel, VFS, XAPI, D3D,
DSound and Bink HLE registered, then starts the guest at the XBE entry
point. It's a tracer bullet: the aim is to find, in order, what stops Halo 2
on its way to the main menu. It does not play the game.

## Running it

```sh
# host binary (build output stays in $TMPDIR)
CARGO_TARGET_DIR=$TMPDIR/boot-target2 /opt/homebrew/bin/cargo build --release -p cli

# lift + emit + clang the whole XBE (about 6-7 min of clang, about 400 MB of C)
$TMPDIR/boot-target2/release/h2 build /abs/path/to/disc/halo2.iso

# boot it (runs `build` first if the dylib is missing)
$TMPDIR/boot-target2/release/h2 run /abs/path/to/disc/halo2.iso
```

Pass the disc as an absolute path to the real ISO, with no symlinks or
copies. Nothing is ever written next to it.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--out DIR` | `$TMPDIR/h2boot` | generated C (`src/`), objects, `libh2game.dylib`, `kernel-trace.tsv` |
| `--data DIR` | `$TMPDIR/h2boot-data` | VFS host storage (`disk/PartitionN`, `metadata/PartitionN`) |
| `--jobs N` | cores | clang parallelism |
| `--slices N` | 100000 | cooperative scheduler slices before giving up |
| `--transfers N` | 50M | runner transfers per slice |
| `--no-stopgaps` | | run on the bare kernel: no boot stopgaps or wrappers |
| `--no-d3d`, `--no-dsound`, `--no-bink`, `--no-xapi` | | leave that HLE crate unregistered |

| Environment | Effect |
| --- | --- |
| `H2_BOOT_VERBOSE=1` | log every boot note (stopgap decisions, fallback entries) |
| `H2_BOOT_MAPS=1` | dump the guest memory map in the report |
| `H2_BOOT_OLD_GLUE=1` | re-enable the boot glue master now covers (the wrappers for alertable/root -3, EEPROM, mutants, MEM_TOP_DOWN, allocation, full attributes and MEM_RELEASE; the 0x55810 voice-off) |
| `H2_BOOT_VOICE=1` | with `H2_BOOT_OLD_GLUE`, keep voice on (skip the 0x55810 override) |
| `H2_BOOT_GFX=<dir>` | run D3D on a headless `GfxBackend` with the nv2a shaders and write presented frames to `<dir>/frame-NNNNN.png` |
| `H2_BOOT_UPLOAD_ALWAYS=1` | D3D re-reads every resource on every use, instead of trusting the write barrier (`UploadPolicy::Tracked`) |
| `H2_BOOT_GFX_EVERY=N` | with `H2_BOOT_GFX`, keep the first frame and every Nth (default 30) |
| `H2_BOOT_INPUT=BUTTONS@WHEN,...` | scripted pad timeline through the keyboard pad and XInputGetState. BUTTONS: `A B X Y BLACK WHITE LT RT UP DOWN LEFT RIGHT START BACK LS RS LUP LDOWN LLEFT LRIGHT` or a key name, joined with `\|`. WHEN: `T`, `T+D` or `T-U`, a slice or (`f` prefix) a present count; held 4 presents or 100 slices by default. Each press is printed. |
| `H2_BOOT_INPUT_FILE=<file>` | the same entries, one per line, `#` comments; merged with `H2_BOOT_INPUT` |
| `H2_BOOT_CAPTURE=<dir>`, `H2_BOOT_CAPTURE_FRAME=N` | record frame N for offline replay (see `crates/boot/src/d3d_diag.rs`): every texture upload, surface, guest write, and the frame's `ResolvedDraw`s with their streams |
| `H2_BOOT_CAPTURE_SNAP=K[-L],...` | with a capture, read the colour target back after those draws of frame N (`snap-K.bin`, guest BGRA bytes) |
| `H2_BOOT_CAPTURE_SKIP=K[-L],...` | with a capture, drop those draws of frame N |
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
    the trampoline interprets from there with `ir::interp`.
  - x87 and leftover SSE instructions go to the emulators in `x87.rs`
    (a decoder over crates/x87's `abi` state machine, so stack faults,
    source-format exceptions and condition codes match what generated
    code will get) and `sse.rs`.
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
  `Device::note_write`. Nothing is drained on return: other contexts can
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

## Current state (2026-11-15, origin/master a41dae4)

**The main menu renders and takes input.** On plain master (backedge and
d3d-r6 landed), with no overlay:

- no stop: the run ends at the slice limit. Preemption makes slices short,
  so `--slices` must be raised: the title screen is at about slice 9000
  (present ~250), 12000 slices take about 106 s on GfxBackend;
- the 3D title scene (the Covenant ship over New Mombasa, the bridge),
  the HALO 2 logo and "PRESS START TO CONTINUE" render (present ~300);
- with `H2_BOOT_INPUT="Escape@10000-10100"` the game takes START and
  shows CHOOSE PROFILE (present ~400), text, buttons and frame complete;
- with `ArrowDown@15600-15700,Space@16200-16300` added (pick Default,
  press A) it reaches the main menu: CAMPAIGN, XBOX LIVE, SPLIT SCREEN,
  SYSTEM LINK, SETTINGS over the 3D scene (present ~480). 21000 slices,
  184 s, 19k preemptions.

The earlier "menu backdrop without text" was the transition into the
title screen, with the UI still faded out (its quads draw with vertex
alpha 0); the game was waiting, not failing to draw.

Screenshots are kept outside the repo, in the session's keep directory:
`boot-title-300.png`, `boot-choose-profile-400.png`,
`boot-main-menu-480.png`, `boot-main-menu-520.png` (and the loading
screen, `boot-loading-36.png`, `boot-loading-89.png`).

Rendering differences still visible (owners to confirm):

- the scene is washed out in a dense brown fog with no sky colour, and the
  fog shows 8x8-pixel blocky patterns (gfx/shaders: atmospheric fog or
  the P8/palette textures the scene binds);
- dotted dark horizontal lines across the screen at a few heights in the
  pre-title frames (gfx);
- some 3D textures look low-detail early on (8x8 tag-data bitmaps until
  the texture cache streams the full ones; expected).

Until the analyzer fix lands, the CRT copy and unguarded switch targets
go through the fallback (blocker 1): about 33k fallback entries by the
profile screen.

### Milestones, from the entry point

1. Entry `0x2d0aee` and the XAPI startup (`xapi::register`).
2. XMountUtilityDrive (Partition 5 cache, `CACHE_ROW`).
3. CRT static init: x87 in the fallback interpreter.
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
11. `cpuid` at `0x34881d` (emulated in the fallback).
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

Next: drive the menu further (CAMPAIGN), longer runs, and the rendering
differences above.

### Ranked blockers

Sizes are S (under a day), M (a few days) or L (a week or more).

| # | Blocker | Owner | What's needed | Size |
| --- | --- | --- | --- | --- |
| 1 | **Masked and holed jump tables** | analyze (1254e53 in review) | `jump_table()` sizes a switch only from a cmp/ja guard. The CRT copy (`0x320890`) indexes its tables with `and r,3` (slot 0 overlaps code) or a negated count (entries below the base), and `0x336f0`'s table (caller-checked index) has zero entries for impossible cases, where an unguarded read stops. The boot reads these tables from the image and interprets from their targets meanwhile. | S |
| 2 | **Fog and sky on the title scene** | gfx / shaders | Brown fog over everything, no sky colour, 8x8 blocky patterns. Not yet narrowed to a shader or texture format. | ? |
| 3 | **RTT write-back replayed as a CPU write** | d3d (runtime_glue) or runtime | The barrier reports the device's own write-back pages at the next entry, so an unchanged render texture uploads again. Boot cannot tell them apart from other contexts' stores during the call: d3d's handlers would need to run under `Guest::writes_unreported(id)` for the boot's watch, or the device would need to report its own write ranges. Performance only. | S |
| 4 | ~~x87 not lowered~~ | lift / emit | Lowered on master (377b675). The boot dylib links emit's x87 bridge, and the fallback's x87 runs on the same crates/x87 abi helpers. | done |

### Boot stopgaps in force

| Stopgap | Where | Retire when |
| --- | --- | --- |
| Back/depth surface backing at CreateDevice, `0xf0000000` write-combined alias | d3d_glue.rs `install_back_surfaces`, lib.rs | d3d / runtime |
| Fallback from unread jump-table targets (the CRT copy's and every unguarded switch's, read from the image), and the unknown-switch stops they cause, allowed only for the site's own table | fallback.rs `unread_targets`, `install_unread_targets` | analyze (#1) |
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
