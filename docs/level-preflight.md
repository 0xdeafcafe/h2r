# Campaign level preflight

Static audit, 2026-11-15, branch `level-preflight`, source baseline
`4620ba0d985d0c5c9cc44f4b6e3f31ea491a9dfa`. Retail XBE SHA-256:
`03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`.
Addresses below are guest virtual addresses. Names are tool/catalog names or
explicitly descriptive engine names; no retail bytes, instructions, scripts,
callback tables or assets are reproduced.

The supplied boot observation is **8,000+ presents in 00a_introduction, with
shadow-compare draws skipped**. This audit did not reproduce that run. A present
count does not establish elapsed simulation ticks, cinematic completion, input
acceptance or a successful level transition. The boot integration lives outside
this baseline's crate set; its overrides/configuration must be checked by its
owner. Do not treat its absence from this checkout as a newly discovered blocker.

The earlier campaign preflight was read and rechecked. Its blanket x87,
visibility, render-target, vblank, WMA-registration and checkpoint-storage gaps
are obsolete here. **No new, unconditionally required missing kernel/XAPI entry
was established on the ordinary 00a → Armory → Cairo route.** The useful next
work is proving live lifecycle/progress contracts and closing specific remaining
rendering/audio limitations, not replacing the generated campaign engine.

## Ranked likely blockers and misbehaviour

Ranking is triage priority, not a predicted execution order. **Gap** means the
current implementation demonstrably lacks the behavior; activation may still be
conditional. **Risk** means an address-backed dependency whose integrated
execution remains unproved, not a demonstrated defect. The existing
shadow-compare skip belongs to the other lane and is not counted again.

| Rank | Candidate and evidence | Owner | Suggested fix scope / acceptance |
|---:|---|---|---|
| 1 | **Risk: intro finishes rendering but never advances scripts/level.** `game_tick` `0x00137fe0` gates simulation before HS `0x00138046 → 0x00209310`; HS compares wake times against game time `0x00510c54`. Intro reaches `game_won` `0x002a99b0 → 0x001388e0 → 0x0018e960`. Presents alone cannot show those gates pass. | `runtime`, `app`, boot integration; generated HS | Add bounded tick/HS wake/transition observations. Check time policy, pause and cinematic state before changing code. Require the ordinary ending and cinematic skip to reach the queued tutorial options. Do not implement a replacement HS scheduler or force `game_won`. |
| 2 | **Risk: next-map/BSP teardown leaves stale host resources or exhausts memory.** Disposal `0x0018ed00`, acquire `0x0018eaa0`, activation `0x0018f000`; BSP switch `0x0011c1b0` calls prepare callbacks at `0x0011c21b` and tag/BSP loading at `0x0011c2e4`. Cairo reuses campaign cache slot 4 after intro and adds six BSPs. Existing cache fixtures omit asset-backed lifecycle and GPU/audio disposal. | `runtime`, `kernel`, `d3d`, `dsound`; generated loader | Observe handles, physical usage, texture/surface/voice counts through 00a → 01a → 01b and a Cairo BSP change. Verify alias-aware write tracking when guest arenas are reused. Repair the first retained owner/stale cache; preserve generated dispose/init ordering. |
| 3 | **Risk: Armory waits forever for look/movement input.** `XInputGetState` call `0x00124c34 → 0x00409b6c`; tutorial look gates `0x002a9470`, `0x002a94b0`, `0x002a9520`, `0x002a9550`; movement enable `0x002a8f30`. `training_look`, `training_move`, `training_shield`, `training_tram` gate `mission_01a`. | `input`, `xapi`, `app`, boot integration | Use existing full analog bindings and per-frame pump. Exercise forward/backward lookstick, inversion acceptance/cancel, held/released movement, trigger and use-button edges. Keep the generated action-test reset/latch behavior. A Start/A-only scripted boot cannot pass this test. |
| 4 | **Gap: previous-frame cinematic effects sample the current frame.** Texture selector `0x00014f80` selects previous parity at `0x00014fc1` / `0x000150a7`, current at `0x00014fe9`. `Device::create` publishes the same back header for back/display surfaces. `frame_texture` snapshots the current render surface. Intro reaches crossfade evaluator `0x002ab270`. | `d3d`, `gfx` | Give previous/current frame resources distinct identities and lifetimes; retain the previous presented image across Swap. Test two frames with distinguishable colors, same-frame feedback, parity and transition reset. The mismatch is proved; the exact first visible effect is not. |
| 5 | **Gap: depth-buffer-as-color effects are skipped.** `0x00014600` constructs aliases; selectors `0x00015020` / `0x00015032` bind `0x00509368` / `0x0050936c`. `frame_texture` explicitly skips sampled depth-buffer aliases. Intro reaches depth-of-field evaluator `0x002ab210`; the specific material/alias selection is conditional. | `d3d`, `gfx`, `shaders` | Implement/test the required depth-to-color interpretation and resource hazards, including the A8R8G8B8/A8B8G8R8 views. This is distinct from shadow comparison sampling. Capture which alias/material activates before choosing a conversion; do not turn the skip into an arbitrary white texture. |
| 6 | **Risk: checkpoint requests never commit, or revert restores bytes without a coherent world.** `game_save` `0x002a9c30`; safety service `0x00226050`, predicate `0x00226190`; request publication `0x0022613d → 0x0012b790`; snapshot `0x00123e20`; full restore `0x00124380`. Existing tests substitute scheduling and synthetic scenario/BSP hooks. | `kernel`, `vfs`, `runtime`; generated gameplay | Observe a safe tutorial save through policy → pending flag → worker → valid slot, mutate position/state, revert, then cross a BSP before another revert. Preserve protection, exact completion counts and the older valid slot. Fix the failing policy/scheduling/lifecycle seam, not the already-tested storage writer. |
| 7 | **Risk: streamed sound stops progressing or completes into freed level state.** Engine DoWork `0x0021ec37 → 0x0037b844`; stream creation `0x002ae68d → 0x0037d4e2`; Process `0x0037ad25`; FlushEx `0x002ae4e2 → 0x0037b827`; WMA creator `0x0038a0c5`. DoWork does not consume samples. | `dsound`, `audio`, `wma`, `app`, boot integration | Verify exactly one mixer consumer stays active in loading loops, WMA registration is installed, and status/size publication precedes callbacks/events. Trace cancel/release at map change and WMA reset/destruction. Use existing consumer-lease and output APIs; do not add a second pump. |
| 8 | **Risk: first active physics/AI/weapon callback escapes known dispatch or exposes state errors.** Objects `0x000b7b40`, type update/post-physics `0x00108c60` / `0x00108cd0`, AI `0x001c83c0`, command scripts `0x00257d00`, Havok `0x001c3ddd → 0x00311100`. Table targets are covered, but Havok alone has 146 unassociated dynamic sites. | `analyze`, `lift`, `emit`, `runtime`; `x87`, `simd` if differential evidence warrants | Preserve unsupported-target PCs/targets/transfer kinds, emitted revision and per-thread FP state. Validate grounded movement, contact/door/tram behavior, AI pathing, weapon pickup/fire/reload and damage in Cairo. Fix the first missing target or proven semantic discrepancy; do not stub physics/AI. |
| 9 | **Risk: persistent save-and-quit/resume depends on unverified signing/profile policy.** Generated signing `0x002d6765`, `0x002d6779`, `0x002d67c7` consumes key import `0x00411660`; runtime key DATA exports are unconfigured placeholders. The checkpoint restart fixture explicitly supplies synthetic keys and a separate U: payload. | `runtime`, `kernel`, `xapi`, `vfs`, boot integration | Confirm stable per-title/console key provisioning and persistent UDATA/TDATA paths before signing. Exercise real save-and-quit, process exit, profile selection and resume; reject damaged signatures without replacing a good save. Stable zero placeholders are not evidence of a crash, but neither are they the configured save contract. |
| 10 | **Gap: campaign audio lacks environmental/effect behavior.** Positional source `0x00220ea1 → 0x0037c668`; stream filter/LFO/envelope sites `0x002203e9`, `0x00220508`, `0x002ae812`; effects setup `0x001913a4`. The DSP image is dry; filters, envelopes, doppler and several routing/headroom controls are stored/ignored. | `dsound`, `audio` | Implement the particular audible effect/routing requirements, starting with dialogue intelligibility and room/airlock transitions. Test synthetic signal response and retain packet completion behavior. This is expected fidelity loss, not proof of a campaign hang. |

## Cutscene and mission route

The disc's Bink inventory is boot intro, attract and credits. The campaign
introduction is **in-engine**: `h2 hs 00a_introduction --builtins` reaches
`camera_set_animation_relative` (`0x002a8aa0`),
`custom_animation_relative` (`0x002a3600`),
`scenery_animation_start_relative` (`0x002a3070`),
`cinematic_start` / `cinematic_stop` (`0x002a9740` / `0x002a9760`),
`sound_impulse_start` (`0x002a9e00`) and cinematic lighting/postprocessing.
The startup selects `high_0`, runs the intro subject to the skip predicate,
ends the skip scope, then invokes `game_won`. No Bink decode is needed to
render these scenes. See [movies](movies.md) and [Bink boundaries](bink-hle.md).

The verified next-level chain is:

- `game_won 0x002a99b0` → campaign completion `0x001388e0`.
- Next-level options `0x0018e960` resolves campaign/level through
  `0x0019c440` and `0x0019c270`, then queues at `0x0018ea83 → 0x0018e790`.
- The main loop services pending transition `0x0018e810`, entering
  `0x0018e8b0`, teardown `0x0018eb80`, acquire `0x0018eaa0`, readiness
  `0x0018f100 → 0x00163890`, then activation `0x0018f000`.
- Activation calls new-map `0x00137ca0` / `0x00137e40` and BSP selection
  `0x0011c1b0`. The selected options must retain campaign/level identity;
  manually loading a scenario path does not prove next-level table lookup.

| Map | Tags | BSPs | First meaningful acceptance |
|---|---:|---:|---|
| `00a_introduction` | 2,863 | 1 (`high_0`) | Normal cinematic completion and separate skip route queue tutorial |
| `01a_tutorial` | 2,830 | 1 (`01_bsp_0`) | Armory look/inversion, movement, shield, tram; real checkpoint and `game_won` |
| `01b_spacestation` | 8,827 | 6 | First combat; BSP lifecycle; checkpoint/revert after gameplay |

Tutorial startup `mission_01a` disables movement/camera input while setting
up training, then restores it through its scripted phases. Treat these gates
as game behavior. The co-op branch can complete the tutorial early, so use
single player for acceptance. Cairo startup is `mission_spacestation`.
The Cairo map includes the tutorial BSPs, Cairo BSPs, outro and High Charity;
a count of six does not mean six new map-file loads.

## Map loading, BSPs and checkpoint boundaries

Campaign cache slots are **4/5**, with shared slot 0 and single-player-shared
slot 1; tutorial is not a multiplayer slot merely because it is small.
Selector `0x00214b80`, copy owner `0x002141f0`, raw-resource router
`0x00213760` and job owner `0x001a1100` remain generated. The router selects
current/menu/shared/SP-shared resources. Map loading must retain these
selectors rather than treat encoded resource addresses as guest pointers.

[Cache integration](cache-slots.md) already tests the real sequence
menu/shared/SP-shared/intro/tutorial/Cairo, cold and warm copies, exact
payloads, checksums, slot publication, short I/O and active-slot preservation.
It runs job callbacks through a deterministic adapter. It does **not** test
real concurrent cancellation, `0x0018f000`, asset-backed BSP callbacks or
renderer/sound disposal. These are the rank-2 delta, not a request to redo
cache mounts or rewrite the loader.

`switch_bsp_by_name 0x002a9580` queues through `0x0012b850` (call
`0x002a95ac`). The actual switch at `0x0011c1b0` performs preparation,
old-BSP disposal, loads new BSP/tag data through `0x00122b40`, publishes
BSP/lightmap globals `0x004e0348` / `0x004e0344`, then initializes callbacks.
It is not a single pointer assignment. Physics world/BSP state, render
resources and AI spatial state must change together. Sound, geometry,
textures, animation and tags are unwound by `0x0018ed00` on map disposal.

[Checkpoint acceptance](checkpoints.md) already covers generated alternating
N: snapshots, protected state, exact I/O counts, fallback staging, malformed
headers, generated save metadata, independent reopen and synthetic signing.
The remaining live policy is substantial: `0x00226050` requires successive
safe evaluations through `0x00226190`, whose callers include AI/unit/gameplay
predicates, before publishing the main-loop request. Cinematic-skip save
`0x002ac3a0` has a separate policy mode. A failure to commit may be a stalled
simulation or legitimately unsafe state rather than failed file I/O.

Full restore `0x00124380` validates identity, can reload the scenario through
`0x0018e8b0`, selects the BSP through `0x0011c1b0`, and restores state. Test
that world objects and external HLE resources agree afterward. Reopening
N: scratch files is not sufficient evidence for a persistent save under U:.

## HS, physics, AI and weapons

The generated HS VM is the campaign execution path. `crates/hs::vm` is a
separate native implementation with an engine trait and is not wired into
this boot; missing host implementations of that trait are not missing guest
builtins. The following fresh inventories include all selected definitions
and both sides of conditionals, not just startup-reachable runtime branches:

| Map | Builtin IDs | Distinct evaluator addresses |
|---|---:|---:|
| Introduction | 95 | 88 |
| Tutorial | 132 | 123 |
| Cairo | 196 | 181 |

HS update `0x00209310` checks enable state `0x004f9388`, iterates the thread
array `0x004f9384`, skips externally driven category 4 and sleeping/finished
threads, then calls thread execution `0x00209850`. `sleep` `0x002087e0`,
`sleep_until` `0x002089b0`, `wake` `0x00208c40`, argument evaluation
`0x00209d50` and return `0x00209ae0` must keep their generated semantics.
A sleeping tutorial thread may be waiting for an input latch, trigger-volume
entry, command-script result or game tick; report which, rather than waking
all threads. Shared evaluator `0x002ab130` is a retail no-op for several
debug/cache builtins, not an HLE stub this project introduced.

Tutorial already demands AI: `ai_place 0x002a4fd0`,
`cs_run_command_script 0x002a6680`, `cs_go_to 0x002a6e90`, device-position
set/get `0x002a47f0` / `0x002a4850`, and trigger volumes
`0x002a0ec0` / `0x002a0f20`. Cairo expands this into combat. Merely moving
the camera does not establish collision, an actor reaching its destination,
weapon possession or damage.

Object construction `0x000b7b40` uses the descriptor family at `0x00468630`;
weapon/projectile behavior flows through generated type initialization
`0x00108a90`, update `0x00108c60` and post-physics `0x00108cd0`, including
inherited callbacks. See [objects](objects.md). Preserve those callbacks,
animation action lookup `0x001db170`, projectile collisions and damage;
`crates/tags` decoders are inspection tools, not gameplay replacements.

The tick calls AI at `0x0013802d`, HS at `0x00138046`, physics at
`0x00138055`, and save policy at `0x0013808c`. Physics
`0x001c3d30 → 0x00311100` changes x87/MXCSR state around the step.
The old x87 blocker is fixed: current `lift/src/lower/x87.rs` handles unit
angle/trig, square root and exponential instructions, and `lower/mod.rs`
handles RDTSC. The remaining concern is correct per-thread/callback state
and dynamically selected targets, not absence of these instructions.

A fresh build of the baseline analyzer produced the following census:

| Root | Conservative functions | Indirect sites | Sites without a table association |
|---|---:|---:|---:|
| New map `0x00137ca0` | 6,770 | 2,202 | 1,408 |
| BSP `0x0011c1b0` | 3,932 | 1,680 | 1,150 |
| HS `0x00209310` | 5,697 | 1,931 | 1,242 |
| Objects `0x000b7b40` | 3,772 | 1,649 | 1,133 |
| AI `0x001c83c0` | 5,925 | 1,950 | 1,246 |
| Havok `0x00311100` | 181 | 190 | 146 |

18,691 functions + 207 address-taken labels = **18,898 callable entries**;
five rounds, converged, zero analysis conflicts and zero CFG errors in
these closures. The catalog has 453 tables, 165 in the union of closures,
zero non-null target misses, and 1,420 unique unassociated dynamic sites.
These sites are unresolved provenance, **not 1,420 missing functions**.
Known lifecycle, HS, object and animation-codec callbacks are present.
[Dispatch coverage](dispatch-coverage.md) explains the conservative expansion
and unknown heap receivers. Its older numerical census is superseded by
this measurement for this baseline. An emitted dylib must still correspond
to the analyzed revision; a successful analyzer does not validate emitted
instruction semantics or dynamic execution.

## Kernel and XAPI coverage: what is actually missing

The import work list is a research inventory, not the current coverage truth.
Compare `kernel/src/exports.rs`, its handlers, `xapi/src/register.rs` and
runtime registration. File create/open/read/write/flush/close, file metadata,
virtual/physical allocation and protection, worker synchronization and SHA-1
are implemented. In particular **RtlTimeFieldsToTime / RtlTimeToTimeFields
(304/305) are implemented**, unlike the earlier report. Generated
`FileTimeToSystemTime 0x003314b8` calls the latter at `0x003314d4`.

XInput's 16 standard override entries and the six WMA codec entries exist.
Save APIs `XCreateSaveGame 0x00330df2`, `XDeleteSaveGame 0x00331055`,
`XFindFirstSaveGame 0x00331125`, `XFindNextSaveGame 0x0033122f`,
`XFindClose 0x00331276` can remain generated. Four opt-in native save
adapters are separate; their absence from a registration is not evidence
that saves are missing.

A temporary audit used `dispatch::coverage` on transition, BSP, HS, save,
input, object, AI and Havok roots and compared direct kernel sites with
`kernel::EXPORTS`. This graph is **not cut at HLE overrides**. Its missing
imports therefore include original SDK internals that should never run:

| Missing/conditional boundary | Evidence | Classification and scope |
|---|---|---|
| `MmQueryStatistics` (181) | `GlobalMemoryStatus 0x00331336`, import call `0x00331347`; five direct calls at `0x00155e80`, `0x001560cd`, `0x00156746`, `0x00156840`, `0x001568aa` | The five callers belong to movie-wrapper memory reporting/allocation. Optional Bink path, not evidence of an in-engine campaign dependency. If enabling that path, implement consistent guest-memory accounting and length validation in `kernel`; returning arbitrary free RAM is unsafe. |
| `KeSaveFloatingPointState` / `KeRestoreFloatingPointState` (142/139) | `0x00379ce7` / `0x00379d05` in DSound wrappers | Original sound internals behind the HLE boundary. Do not infer that Havok needs these kernel functions merely because it saves FP state itself. Investigate escaped DSound paths before adding kernel emulation. |
| `KeInsertQueueDpc` (119), `KeStallExecutionProcessor` (151) | `0x003810b1`, `0x003dc750`; `0x0037ea6c`, `0x00384abd`, `0x00384ae2` | SDK device/driver paths; not proved mandatory once HLE is installed. If reached, capture the HLE escape and preserve the actual callback/wait contract. |
| `MmLockUnlockBufferPages` (175) | `0x0037f871`, `0x00380431`, `0x00380494`, `0x0040fbc7`, `0x00410088` | DSound/device paths in the conservative graph. Conditional, not a new generic map-loading API requirement. |
| `RtlRaiseException` (302), `KeBugCheck` (95) | `0x002d2079`, `0x002d4da7`, `0x002d5673`; `0x00324390`, `0x00326087` | Error/exception paths. A stop needs its original cause diagnosed; treating a raise/bugcheck as success would conceal it. |
| Crypto 338/339/340/345/346/347/349 | Thunks `0x0036d1ee`, `0x0036d1e8`, `0x002d6f96`, `0x0036d206`, `0x0036d200`, `0x0036d1fa`, `0x0036d1f4` | Present in broad alternate SDK paths. Generated profile signatures use SHA-1 335–337 and guest keyed hashing; no new ordinary checkpoint need for these missing exports was proved. |
| `NtSetInformationFile` rename class | `kernel/src/files.rs::set_information` has no class-10 handler | Conditional save-management operation; ordinary tested checkpoint/create/delete paths do not establish a rename need. Implement only with a traced caller and atomic VFS rename semantics. |

The pure Havok root's conservative import set is 95, 128, 184, 199, 269,
277, 294 and 302. Outside error paths, time, allocation/free and critical
sections are already present. This is useful negative evidence against a
large new “physics kernel API” implementation, not proof every heap dispatch
is resolved.

## D3D entry points, methods and formats

At this baseline `d3d::abi::UNIMPLEMENTED` has **Reset `0x003f77c0`**.
The additional miniport port-I/O guards are deliberate. Reset's observed
caller is device reinitialization, not ordinary map/BSP loading; the latter
retains the device. Do not assign Reset as a mandatory tutorial transition
fix without a trace. `SetBackBufferScale 0x003f9df0` accepts only unit scale;
its two direct calls are in `0x00013f10`. Non-unit use remains a conditional
stop, not something demonstrated by the campaign script inventory.

| Campaign demand / evidence | Current coverage or limit |
|---|---|
| Visibility `0x00020cbb → 0x003f8630`, `0x00020d0f → 0x003f85d0`; inline method `0x17c8` at `0x00020cec` | Implemented report/count path. Only ZPASS report type is supported. Count pass does not run the original fragment shader's alpha/discard logic; known flare query draws do not depend on it. |
| Constant upload `0x00040d01 → 0x003f7410`; vertex selection `0x003f6cf0` | Implemented. Constant-writing vertex programs still produce a shader-translation skip; ordinary read-only programs translate. No first-three-map requirement for such a program was established. |
| Targets `0x00014bc0 → 0x003f7870` / `0x003f7b30`; viewport `0x003f8100`; clear `0x003f9fc0`; scissors `0x003f8760` | RTT, same-size target switching, masked/partial clears and multiple scissor rectangles exist. Compressed/depth RTT write-back and rewritten dirty target identities still have explicit limits. |
| Inline vertex arrays `0x0004b220`; `BEGIN_END 0x17fc`, `INLINE_ARRAY 0x1818`; stipple `0x1480`; callback `0x0002460e`; vertex-cache break `0x00043a1a` | Decoder/drain support exists; no newly mandatory unknown method was found. Unknown methods still stop with the header PC and method, which should be retained in the boot report. |
| Color/normal/lightmap texture formats | `gfx::format::D3DFormat` and texture conversion cover DXT1/3/5, P8/palette, ordinary linear/swizzled color/luminance, signed bump formats, cube and volume layouts. Do not report these families wholesale as missing. Tag decoding alone does not establish runtime header/alias correctness. |
| Frame color `LIN_A8R8G8B8`, `LIN_X8R8G8B8`; depth aliases `LIN_A8R8G8B8`, `LIN_A8B8G8R8` | Same-size current color snapshots work. History identity and depth-as-color sampling are ranks 4/5. Other frame-sentinel offsets, dimensions or formats can skip. |
| Pixel texture modes `DOT_ZW`, `BRDF` | `shaders/src/psh/wgsl.rs` warns: depth replacement is omitted; BRDF reads zero. These are real partial semantics, but this audit did not establish their selection by a particular campaign material. Keep separate from the known shadow-compare work. |

Check `Event::DrawSkipped` **and** shader warnings. An absence of hard stops
or even skipped draws does not imply correct pixels: history aliases and
accepted approximate shader modes can render the wrong result silently.
Resource destruction, texture dirty tracking and RTT write-back are already
implemented; rank 2 concerns their live cross-map ownership, not their absence.

## Streaming sound boundaries

The main ADPCM route builds its format at `0x0021e410`; WMA has a separate
container/codec route. Do not classify all dialogue as WMA or all codec-encoded
sounds as ordinary ADPCM. `dsound::xwma::register` installs creator/reset/
destroy/begin/decode/PCM boundaries from `0x0038a0c5` through `0x0038a1cc`
while leaving container reads and XMO packets generated. The earlier report's
“WMA is not wired” is fixed at the library seam; verify the boot uses it.

Source/listener position, ordinary pitch, stream Process/status, immediate
StopEx/FlushEx, voice properties, effect-data storage and mixer consumers
exist. Rank 7 concerns the runner's progress and ownership across transition.
Tutorial uses `sound_impulse_language_time 0x002a9fe0` in script sleeps;
this computes script timing, not proof that the corresponding audio packet
has played. Correct script timing can coexist with inaudible/starved sound.

Remaining hard-stop variants are nonzero StopEx/FlushEx timestamps,
Pause(3) on inactive voices, Lock(FROMWRITECURSOR), unsupported format/image
requests. Inspected ordinary game StopEx/FlushEx sites use zero timestamps;
do not promote hypothetical future scheduling into a confirmed campaign
blocker. Nonzero packet status, callback ordering and sound cancellation
need checks even when output is intentionally discarded for headless runs.

## Source ledger

Implementation checks used these baseline files, with source taking precedence
over older coverage prose:

- [Kernel exports](../crates/kernel/src/exports.rs),
  [file operations](../crates/kernel/src/files.rs),
  [XAPI registration](../crates/xapi/src/register.rs) and
  [runtime import metadata](../crates/runtime/src/kernel.rs).
- [D3D adapters](../crates/d3d/src/abi.rs),
  [device identities and draw handling](../crates/d3d/src/device.rs),
  [frame aliases and RTT](../crates/d3d/src/device/raster.rs),
  [push decoder](../crates/d3d/src/pushbuf.rs),
  [shader preparation](../crates/d3d/src/nv2a.rs),
  [texture formats](../crates/gfx/src/format.rs) and
  [pixel-mode translation](../crates/shaders/src/psh/wgsl.rs).
- [DSound behavior](../crates/dsound/src/state.rs),
  [consumer/callback integration](../crates/dsound/src/register.rs),
  [WMA registration](../crates/dsound/src/xwma.rs),
  [input bindings](input-default.toml),
  [x87 lowering](../crates/lift/src/lower/x87.rs) and
  [dispatch analysis](../crates/analyze/src/dispatch.rs).

## Validation and next handoff

Performed here: `h2 analyze`, targeted `h2 disasm`, `h2 hs` inventories and
startup-path inspection, `h2 tags` BSP/group inventories, and `h2 ui --all`
on each of the first three maps. **All three UI validations passed**: each
had 30 skin, 38 wgit, one wgtz and one wigl tag, with zero failures.
These are asset-reader checks, not gameplay tests. A private release build
of `analyze` reproduced the dispatch census; an address-only temporary
analysis probe supplied kernel-boundary and direct-caller cross-checks.
No production behavior changed, no new behavior tests were appropriate,
and no game run, full-image emit or generated-game clang build was needed.

Reproduction from a checkout with its built `h2` available:

```sh
export H2_XBE=/Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/default.xbe
export H2_DISC=/Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/halo2.iso
export RUSTC_WRAPPER= CARGO_BUILD_JOBS=4
export CARGO_TARGET_DIR="$TMPDIR/level-preflight-target"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export H2_OBJ_CACHE="$TMPDIR/shared-objcache"
h2 analyze "$H2_XBE"
h2 disasm "$H2_XBE" 0x18e960 100
h2 disasm "$H2_XBE" 0x11c1b0 110
h2 disasm "$H2_XBE" 0x137fe0 65
h2 disasm "$H2_XBE" 0x226050 100
h2 hs 00a_introduction --builtins --disc "$H2_DISC"
h2 hs 01a_tutorial --builtins --disc "$H2_DISC"
h2 hs 01b_spacestation --builtins --disc "$H2_DISC"
h2 tags 01b_spacestation sbsp --disc "$H2_DISC"
h2 ui --all 01a_tutorial --disc "$H2_DISC"
cargo run -p analyze --release --example dispatch_coverage
```

For the boot owner, the smallest useful next observation is **tick + HS thread
wake/frame + pending level options** while the introduction advances. Then
accept tutorial input, reach Cairo, change BSP, revert and restart from a
persistent save, in that order. Log the generated dylib revision, HLE override
set, upload policy, audio consumer and input mapping with each result. Use
bounded runs through `gate.sh game` and the shared dylib under
`$TMPDIR/shared-gen/<master short hash>/`; do not emit another full image for
this report. Follow the lane rule's background completion notifications,
not sleep/poll loops. The frame budget and first failing callback/material
remain open questions; this static audit supplies no FPS or gameplay-success
claim.

No isolated production patch was justified: the proven remaining gaps span
resource identity/conversion or audio processing, while the potential hard
stops require a live activation witness or fuller contract work. This avoids
adding speculative success stubs to already-working generated systems.
