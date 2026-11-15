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
| `H2_BOOT_GFX_EVERY=N` | with `H2_BOOT_GFX`, keep the first frame and every Nth (default 30) |

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

## Current state (2026-11-15, origin/master 2a2b197)

**Last stop:** thread 2, in `D3DDevice_SetRenderState_CullMode`
(`0x3f53d0`, from `0x5206c`). The push-buffer sync there hits

> push buffer: unimplemented NV2A method 0x1760

which is the vertex data array format. `CDevice_SetStateVB` (`0x401030`,
run as the retail body) pushed it for an inline draw that `0x4b220` begins
with BeginPush (`0x3fae10`). The run takes about 10.3k kernel calls, 753
slices and about 22 s (about 52 s on GfxBackend).

**First real frames.** With `H2_BOOT_GFX`, the loading screen renders
through `GfxBackend` and the nv2a shaders: Halo's own "LOADING 36%" ...
"LOADING 89%" over its dark blue background. There are 2986 draws (2889
inline), 110k push words and 3106 texture uploads, and render-to-texture
writes back 6 times. Screenshots are kept outside the repo, as
`boot-loading-36.png` and `boot-loading-89.png` in the session's keep
directory.

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
    callback `0x213fa0` waits for the read (stopgap).
14. Loading screen: the unbounded switch tables `0x216a54` and `0x216b01`
    run through the fallback. The "LOADING nn%" frames render; `t:\$C\`
    and `u:\` are probed.
15. Offscreen render targets: render-to-texture through crates/d3d, and
    the same-size SetRenderTarget fast path `0x3f7b30` (stopgap).
16. An inline draw through BeginPush and SetStateVB, then the `0x1760`
    stop.

The main menu still needs, past this point:

- the vertex array methods in the push parser;
- whatever the menu's UI draws reach next.

### Ranked blockers

Sizes are S (under a day), M (a few days) or L (a week or more).

| # | Blocker | Owner | What's needed | Size |
| --- | --- | --- | --- | --- |
| 1 | **NV2A vertex array methods** | d3d | The push parser must decode `0x1760..0x179c` (`SET_VERTEX_DATA_ARRAY_FORMAT`) and the matching offsets (`0x1720..`). `CDevice_SetStateVB` pushes them before inline and indexed draws. This is the current stop. | S-M |
| 2 | **SetRenderTarget fast path** `0x3f7b30` | d3d (r6) | `[esp+4]` = target, EBX = depth, `ret 4`. It is the same as `0x3f7870` without the viewport reset. The boot forwards it to `0x3f7870`. | S |
| 3 | **BeginPush `0x3fae10`, SetStateVB `0x401030`, Get2DSurfaceDesc `0x3fdb20`** | d3d | Stopping entries that the boot runs as retail bodies (`RUN_NATIVE`). Move them to `abi::NATIVE` or implement them. | S |
| 4 | **NV2A register base** device+0x934, back/depth surface backing, `0xf0000000` alias | d3d (r6) / runtime | Boot glue at CreateDevice (`install_gpu_registers`). | S-M |
| 5 | **Spin-waits in generated code** | backedge lane (8f72ddb, in review) | Retires the `0x213fa0` override. Preemptions use `Machine::run` slices, so `--slices` may need raising. | in review |
| 6 | **x87 not lowered** | lift / emit (x87 crate on master) | About 10.5k fallback entries and 72k x87 instructions by the loading screen. The boot's fallback still uses its own `x87.rs`. | L |
| 7 | **Lift leftovers** | lift-leftovers lane | `cpuid`, `rdtsc` and jump tables with null holes (`0x216a54`, `0x216b01`). The fallback covers them. | S |
| 8 | **Write tracking** | boot + d3d | The boot still uses `UploadPolicy::Always`. Wiring `Guest::subscribe_writes` and `take_dirty` to `Device::note_write` (docs/d3d.md Integration) would allow `Tracked`. | S-M |

### Boot stopgaps in force

| Stopgap | Where | Retire when |
| --- | --- | --- |
| `0x213fa0` stream purge waits 1 ms per spin | game_glue.rs `install_stream_purge` | backedge lands |
| unknown switch target: interpret from the target | fallback.rs `trampoline` | lift-leftovers |
| `cpuid` and `rdtsc` emulated | fallback.rs | lift-leftovers |
| SetRenderTarget fast path `0x3f7b30` via `0x3f7870` | d3d_glue.rs `install_render_target_fast` | d3d (#2) |
| Get2DSurfaceDesc, BeginPush and SetStateVB as retail bodies | d3d_glue.rs `RUN_NATIVE` | d3d (#3) |
| device+0x934 NV2A stand-in, back/depth backing, `0xf0000000` alias | d3d_glue.rs `install_gpu_registers`, lib.rs | d3d-r6 (#4) |
| `--no-xapi` only: XMountUtilityDrive `0x2d5b77` | xapi_glue.rs | n/a |

### Retired

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
