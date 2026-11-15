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
| `H2_BOOT_RTT=1` | SetRenderTarget to a texture level keeps the HLE target (see Current state) |

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
- **`fallback.rs`, `x87.rs`, `sse.rs`**: mixed-mode fallback.
  - When generated code stops on an instruction the lifter doesn't lower,
    the trampoline interprets from there with `ir::interp`.
  - x87 and leftover SSE instructions go to the emulators in `x87.rs`
    (ext80 storage, f64 arithmetic per docs/FLOAT.md) and `sse.rs`.
  - Control goes back to generated code at the next registered entry or
    resume, call or return.
- **`stopgaps.rs`, `wrappers.rs`**: boot-only kernel behaviour, each a
  named blocker below.
  - A stopgap replaces a kernel export the kernel stops on. It is only
    installed if the ordinal is in `Kernel::stopped_ordinals()`.
  - A wrapper parks the kernel's handler on a shadow page (`0xa0ff1000`),
    fixes up the arguments, then jumps to it.
- **`trace.rs`**: wraps every kernel import token to record the TSV.
- **`d3d_glue.rs`**: `d3d::register` on a headless `RecordingBackend`,
  with `UploadPolicy::Always`.
- **`dsound_glue.rs`**: `dsound::register`. Completion events go to
  `Kernel::signal_event`.
- **`xapi_glue.rs`**: `xapi::register` with a kernel-VFS `Storage` and a
  headless `InputManager`.
  - `xapi::service_input` runs between scheduler slices.
  - With `--no-xapi`, the boot keeps its own XMountUtilityDrive override
    (`0x2d5b77`, which writes `xapi::register::addr::CACHE_ROW`).
- **`game_glue.rs`**: boot-only overrides of Halo functions (the frame
  wait, the stream purge callback, and the retired voice-off), each with
  its ABI and the retail path it reproduces.

## Current state (2026-11-15, master b309045)

**Last stop:** thread 2, `D3DDevice_SetRenderTarget` (`0x3f7870`), from
`0x1cd71` <- `0x14d47` (`0x14bc0` selects a render target by index):

> render target 0x004b56b8 is a level of texture 0x004b56a4 (render-to-texture)

That's after about 9.7k kernel calls, 565 slices and about 30 s, with no
kernel stopgap installed. By then the loading screen has drawn 2880 inline
Begin/End draws (about 105k push words and 2880 texture uploads), and
`mainmenu.map` is open on thread 3.

With `H2_BOOT_RTT=1` the run gets one call further. It then stops on
the next SetRenderTarget's push sync, because the offscreen pass now lands
on the back buffer (a stopgap artefact): "viewport 320x240 at (0, 0) on a
640x480 target (gfx draws the whole target)".

### Milestones, from the entry point

1. Entry `0x2d0aee` and the XAPI startup (`xapi::register`): the `\??\D:`
   link, T:/U: and TitleMeta.
2. XMountUtilityDrive through `xapi::register`. Partition 5 is the cache,
   and the cache DB row is `CACHE_ROW`.
3. CRT static init: x87 in the fallback interpreter.
4. Game main:
   - AV pack and video;
   - the push buffer;
   - `Direct3D_CreateDevice` (NV2A base stand-in and back/depth surfaces
     from the boot);
   - the first Swap.
5. The world pool: MmAllocateContiguousMemoryEx `0x30e4000` at physical
   `0x61000`, mapped at `0x80061000` by `LoadOptions::halo2()`. There's
   also a write-combined alias at `0xf0061000`.
6. The second thread, the mutants and the fonts.
7. Cache files `z:\cache000/001.map`. The N: partition is mounted
   (Partition4), and `n:\cache002..005`, `savegame0/1` and `sgf` are
   created.
8. XcSHA, XInitDevices and the D3D state setup.
9. XHV voice engine: it passes `0x33337d` with DSound round 2 (no
   `0x55810` override).
10. Network: PhyInitialize, then the XNet 20 ms timer DPC `0x332f4c`
    runs on thread 0 (kernel round 3 DPCs).
11. `cpuid` at `0x34881d`, emulated in the fallback.
12. The `d:\bink\intro_60.bik` probe and the movie loop `0x1639c2`
    (SwitchToThread). Needs the NtYieldExecution stopgap.
13. `d:\maps\mainmenu.map` is opened (thread 3). The stream-block purge
    callback `0x213fa0` waits on the async read (stopgap).
14. Loading screen: unbounded switch tables `0x216a54` and `0x216b01`
    (fallback). It draws 2880 inline draws. `t:\$C\` and `u:\` are probed,
    and `d:\launch.txt` is missing, as it should be.
15. The resource busy check `0x3fb710`, then `0x3f8cc0` (method `0x1480`),
    then the render-to-texture stop.

The main menu still needs, past this point:

- render-to-texture and partial viewports;
- then the menu's own UI draws.

GfxBackend + nv2a (gfx-shaders) is not switched on yet. The trace is
still in the loading screen, on RecordingBackend.

### Ranked blockers

Sizes are S (under a day), M (a few days) or L (a week or more).

| # | Blocker | Owner | What's needed | Size |
| --- | --- | --- | --- | --- |
| 1 | **Render-to-texture** at `0x3f7870` | d3d | The offscreen targets `0x14850` builds (`0x3fb7a0` sets `Parent`) must be real host surfaces whose results reach the texture's guest bytes, or the texture's sampling. The current stop. | M |
| 2 | **Partial viewports** | d3d / gfx | `0x14bc0` sets the viewport to the target's extents: 320x240 for the offscreen targets, with split screen later. gfx draws the whole target, so it needs a viewport/scissor. This is the stop after #1 (seen with `H2_BOOT_RTT`). | S-M |
| 3 | **Spin-waits in generated code** | emit + runtime (backedge lane) | Generated loops never give the runner control back, so a guest busy-wait on another thread hangs natively, with no stop and no transfer: `0x12b2e0` (vblank) and `0x213fa0` (async read). It needs a preemption point on loop back-edges (a budget check that ends the slice). | M |
| 4 | **NtYieldExecution starvation** | kernel (kernel-r5 lane) | `Machine::run` advances the clock to the next deadline and runs DPCs while every ready thread is `polling`. With a periodic DPC (XNet's timer re-arms itself from the DPC) there is always another DPC, so a yielding thread never gets a slice. A polling thread must run after one clock advance. | S |
| 5 | **vblank delivery** | d3d round 4 + runtime-watch | The vblank callback at device+0x1db8 (`0x12b2a0`) is never called. The boot's 0x12b2e0 override calls it from the frame wait; it hasn't been reached yet. Retire it when frame-wait B and `raise_interrupt` land. | S |
| 6 | **D3D entries still stopgapped** | d3d (rounds 4/5) | `0x3f6190`, `0x3f6390`, `0x3f6840`, `0x3f6f40` (fragment and transform pushes), SetSoftDisplayFilter `0x3f8c70` (round 4); `0x3fb710` resource busy and `0x3f8cc0` / NV2A method `0x1480` (round 5); Get2DSurfaceDesc `0x3fdb20` (retail body); device+0x934 NV2A base and the back/depth surface backing at CreateDevice. | M total |
| 7 | **x87 not lowered** | lift / emit | About 10.5k fallback entries and 72k x87 instructions by the loading screen (fmul, fld, fstp, fistp, fsqrt, fptan, fpatan, ...). Correct, but slow. Lower x87 per docs/FLOAT.md. | L |
| 8 | **Lift leftovers** | lift-leftovers lane | `cpuid` (emulated in the fallback as the Pentium III, family 6 model 8 stepping 10). The analyzer stops reading a jump table at its first null entry: `0x216a54` and `0x216b01` are unchecked `dec eax; jmp [eax*4+tbl]` with holes. The fallback now interprets from any unknown switch target. | S |
| 9 | **AvSendTVEncoderOption 0xb** | kernel (kernel-r5 lane) | The flicker-filter option is ignored in the boot (`*Result = 0`). Kernel round 4 still stops on it. | S |
| 10 | **Async file I/O, write barrier, analysis of pointer-only functions, build time** | kernel/vfs, d3d+runtime-watch, analyze, emit | Unchanged from the previous round. None is the current stop. | M each |

### Boot stopgaps in force

All of these are in crates/boot and listed in the run report under
"stopgap used".

| Stopgap | Where | Retire when |
| --- | --- | --- |
| AvSendTVEncoderOption 0xb ignored | wrappers.rs `install_tv_encoder` | kernel-r5 |
| NtYieldExecution as a 1 ms KeDelayExecutionThread | wrappers.rs `install_yield` | kernel-r5 (#4) |
| `0x12b2e0` frame wait delivers vblanks | game_glue.rs `install_frame_wait` | d3d round 4 frame-wait B |
| `0x213fa0` stream purge waits 1 ms per spin | game_glue.rs `install_stream_purge` | backedge lane (#3) |
| unknown switch target: interpret from the target | fallback.rs `trampoline` | lift-leftovers (#8) |
| `cpuid` emulated | fallback.rs | lift-leftovers (#8) |
| D3D no-ops `0x3f6190`, `0x3f6390`, `0x3f6840`, `0x3f6f40`; SetSoftDisplayFilter | d3d_glue.rs | d3d round 4 |
| `0x3fb710` resource busy (GPU idle) | d3d_glue.rs `install_resource_busy` | d3d round 5 |
| `0x3f8cc0` cache only, method `0x1480` not pushed | d3d_glue.rs `install_ramp` | d3d round 5 |
| Get2DSurfaceDesc as the retail body | d3d_glue.rs `RUN_NATIVE` | d3d |
| device+0x934 NV2A stand-in, back/depth surface backing, `0xf0000000` alias | d3d_glue.rs `install_gpu_registers`, lib.rs | d3d / runtime-watch |
| SetRenderTarget to a texture level, opt-in `H2_BOOT_RTT=1` | d3d_glue.rs `install_render_to_texture` | d3d (#1) |
| `--no-xapi` only: XMountUtilityDrive `0x2d5b77` | xapi_glue.rs | n/a |

### Retired

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
