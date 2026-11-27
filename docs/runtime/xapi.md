# XAPI boundary

`crates/platform/xapi` overrides services that need native host integration. Ordinary
XAPI routines stay generated over the kernel HLE. [storage.md](storage.md)
describes the VFS shared namespace and ownership.

No game bytes, executable excerpts, keys, signature patterns or assets are
included. `adapter.rs` is runtime-neutral. `register.rs` binds it to the
runtime (see [Runtime registration](#runtime-registration)).

## Pages

| Page | Holds |
|---|---|
| [XAPI: saves and the N: volume](xapi-saves.md) | Generated saves and config, opt-in native save games, the second cache volume. |
| [XAPI: runtime registration and inventory](xapi-inventory.md) | How handlers register, boot integration, and the inventory of matched and inferred entries. |

## Evidence and ABI

The supplied retail 5849 XBE has **43 Xapi signature matches**. The inventory
adds **99 named entries, thunks and inferred helpers** from the existing Ghidra
project: **142 entries**. Inferred names are descriptions of inspected behaviour,
not extra signature matches. All observed behaviour below is anchored to its
function/instruction address. Emulator bounds and host policies are labelled
as such; they are not claims about retail limits.

The table gives all identified entries, their compiled ABI, route, and static
`C/J/D` counts: calls / tail jumps / other references including callback/data
pointers. Counts include linked SDK callers; they are not execution frequencies
or proof that every branch executes. Anonymous helpers, indirect calls and
inlining prevent a claim of exhaustive identification of every XAPI byte.
There is no independently identified out-of-line XGetDevices, XInputPoll,
five-argument GetDiskFreeSpace, fiber API or multimedia timer API. Do not invent
an address for an unmatched function.

Reproduce matching with the pinned database from
[xdk-libraries.md](../lifter/xdk-libraries.md) and your XBE:

```sh
H2_XBSYMDB=<pinned-database-checkout> cargo run -p cli --release -- symbols disc/default.xbe --list
```

Use the main checkout's `scripts/ghidra --json x-ref to <address> --limit 0`
to inspect references, and `h2 disasm <absolute-xbe> <address> <count>` to check
an epilogue and its callers. Ghidra's stored `undefined name()` signatures and
`unknown` conventions do not establish an ABI. Exception-filter subroutines
can contain plain returns inside a stdcall function; a function's Ghidra byte
count need not describe one contiguous address interval.

All guest pointers/handles/stack slots are 32 bits; WCHAR is UTF-16LE. `S<n>`
is the x86 stack convention whose public return removes `n` argument bytes,
plus the four-byte return PC. Counts are decimal bytes. Results use EAX (or AL
for BOOLEAN); void results are unspecified. Internal register conventions are
explicit. `tests/routing.rs::VERIFIED_RET_SIZES` pins the **32 reviewed hook
address/ret-size pairs**, including the init thunk, independently of the adapter's
`stack_words()` calculation. It includes routines routed to generated code.

## Override decisions

`Function::is_replacement()` is the executable policy. Generated routes return
`Outcome::Original` before reading even the guest stack and leave CPU, memory
and last error untouched. There are **16 identified override addresses**;
XGetDevices and XInputPoll remain unbound adapters until an address is verified.

| Addresses | Keep in Rust | Why the kernel path is insufficient |
| --- | --- | --- |
| `00408c57`, thunk `004098c0` | XInitDevices | Original enters USB/XPP initialisation; native input backends replace that hardware stack. |
| `004098c5`, `00409884` | XGetDeviceChanges, XPeekDevices | Cur/Prev/Chg belong to the native input manager; both views must share that state. |
| `00409932`, `00409988` | XInputOpen, XInputClose | Native handles retain a host pad generation rather than a guest USB object. |
| `00409994`, `00409b6c`, `00409bdf` | Capabilities, GetState, SetState | Translate packed reports, disconnected snapshots and asynchronous host feedback. |
| `0041136f` | XReadMUMetaData | No memory units are exposed; return 1167 without entering the guest MU driver. |
| `002d5b77`, `002d5c7c` | Mount/format utility drive | Retail cache selection/formatting goes below ordinary files; delegate explicit utility-volume operations to the shared VFS. |
| `002d5d0a` | Mount the second cache volume, N: (inferred name XMountCacheDrive) | Retail formats a raw partition through disk IOCTLs and FATX sector writes (`002d6c04`); the native override keeps the database selection, slot and BOOL contract and empties a host-backed volume instead. See [The second cache volume](#the-second-cache-volume-n). |
| `002d6031`, `002d6004` | Power-down init/reset | Native application policy disables the SMC timer/DPC. Install/remove both hooks in one dispatch-table edit using POWER_DOWN_OVERRIDES. |
| `002d07f2` | XLaunchNewImageA | A successful title transition must stop/reload the host runner; a kernel wrapper alone cannot arrange that transition. |

By default no save or config replacement is retained (the opt-in native save
games below are the exception). This removes SaveRecord,
Disposition, DiskSpace, save cursors, Rust metadata packing, and the duplicate
incoming LaunchInfo store. The Storage contract contains only utility
mount/format. XFindClose therefore always remains generated: there is no token
range heuristic (the opt-in save games keep a small host table keyed by the
search's kernel handle, below). Error 18 and name termination belong
to the generated save/file path, not to a second implementation.

| Addresses | Generated route | Required kernel behaviour |
| --- | --- | --- |
| `00330df2`, `00331055`, `00331125`, `0033122f`, `00331276` | Create/Delete/FindFirst/FindNext/FindClose save APIs | Ordinary file/directory calls through shared VFS and RtlNtStatusToDosError (301). Preserve the original hash, metadata, cursor and close behaviour. |
| `0033039a`, `00330310`, `00330339`, `003321bd`, `00331385`, `002d5813` | Language, video standard/flags, AV pack, region, parental control | ExQueryNonVolatileSetting (24), plus coherent HalBootSMCVideoMode data (356) for AV pack. No Rust query overrides. |
| `002d5838`, `002d5f65` | DiskFreeSpaceEx and cluster-size helper | NtQueryVolumeInformationFile (218) and ordinary file open/close. These are wrappers, not hardware services. |
| `002d1d3e`, `002d1d66` | GetLastError, SetLastError | Guest FS/TLS access, without a parallel host/process last-error store. |
| `002d0915` | XGetLaunchInfo | LaunchDataPage (164) storage and MmFreeContiguousMemory (171); use the original one-shot consumption and title/kind checks. |

## Input behavior and sources

Device types are guest `XPP_DEVICE_TYPE*` identities, not numeric XID types:
`004086f8` is gamepad and `004087a8` is memory unit (arguments observed in
`00124aa0`). Only gamepads are exposed by this host policy; unknown type masks
are zero and unsupported opens fail. `00409932` supplies port, slot and optional
polling parameters; the adapter accepts ports 0..3 and gamepad slot zero.
Preallocation entries are two guest words at `00408c57`; the adapter validates
them without starting USB. Both init entry addresses above reach that same ABI.

The direct call to `XInputGetState` is at `0x00124c34`, inside the controller
input routine at `0x00124c00` (see the input phase in [engine-map.md](../engine/engine-map.md)).

`004098c5` is the consuming XGetDeviceChanges function. With Chg=0, write two
zero masks and return false. Otherwise compute:

```text
reinserted = Chg & Prev & Cur
ins = (Cur & ~Prev) | reinserted
rem = (Prev & ~Cur) | reinserted
Chg = 0
Prev = Cur
return (ins | rem) != 0
```

This drops a plug/unplug between polls when Prev=Cur=0. Insert/remove/insert
with Prev=0 reports insertion only; remove/reinsert with Prev=Cur=1 reports
both. These are the formulas at `004098c5`, not accumulated host edge masks.
The native DeviceChanges struct records current/changed/previous separately.

**XPeekDevices is different in the image.** At `00409884` it returns Cur and
optionally writes Prev and `Prev & Chg & Cur`; its return is at `004098bd`.
It does not clear Chg or update Prev, including when Chg=0. The adapter preserves
that compiled behaviour while sharing the corrected state. No BOOL/consuming
contract from `004098c5` is imposed on this three-slot peek API. The descriptor
publication used by both routines is 12 bytes: Cur, Chg, Prev.

`00409b6c` copies packet number and report even on a disconnected open handle,
then returns 1167. Halo accepts 1167 at `00124c3b`. Each native open handle keeps
its own final published State, refreshed before/after host sync and on explicit
poll/read, so a replug cannot replace an old handle's report. Closing at
`00409988` removes that handle; invalid closed handles return error 6 without
writing a report (host error policy).

The state/capabilities/feedback boundaries are `00409b6c` / `00409994` /
`00409bdf`; their packed layouts are 22 / 25 / 70 bytes, cross-checked in
[input.md](../input/input.md). Polling parameters at `00409932` are four bytes: flags in
byte 0, intervals in bytes 1/2, reserved byte 3. Native policy rejects reserved
bits. Capabilities::standard supplies input masks only; the device manager adds
motor masks only for a backend with supports_rumble. Unsupported feedback
returns 50 without queuing a request and, like retail `0040a923`, writes 50 to
Header.dwStatus; a disconnected open handle writes 1167 (`0040aa0a`). Halo's
rumble loop at `00124f31` reads that status and ignores EAX. This models host capability at the
`00409bdf` service boundary; macOS gilrs rumble is unavailable.

**Host source.** `InputManager::with_keyboard_pad(bindings)` is the default:
the keyboard+mouse pad is the first backend, so it is on port 0 and connected
from the first sync. It returns the `KeyboardMouse` handle the platform layer
feeds (the app uses the shared player-one fallback described below). That pad
advertises both motors and accepts feedback like a standard controller.
Requests complete with status 0, and `KeyboardMouse::rumble()` holds the
newest motor speeds. A plain `KeyboardMouse::new` keeps the no-motor policy
(50). App's `KeyboardFallback` always accepts feedback and forwards it to the first
physical pad when present. Standalone pad capabilities are fixed at insertion.
Halo's loop at `00124f31` submits with `hEvent = 0` whenever the status is 0,
skips while it is 997 and clears any other status, so a pad without motors
alternates 50/0 every frame. Halo accepts capabilities SubType 1 or 2
(`00124b3d`). No memory units are ever reported: changes/peek for
`004087a8` are zero and XReadMUMetaData returns 1167.

Feedback accepted through `00409bdf` writes pending status 997. sync_input
writes completion statuses under the memory guard and queues owed event
signals; take_signals returns them as a SignalBatch that holds no guard, which
is delivered through Events and passed back to acknowledge_signals. A buffer
stays pending (reuse returns 170) until its signal is acknowledged; a failed
signal is requeued without rewriting the status. The host must retain the event
object at submission, even if its guest handle is later closed. Guest buffers
remain owned by the guest until completion. Each sync attempts every pending
completion once, retaining failures and returning the first error only after
processing the remainder. A permanently invalid event or unwritable buffer
cannot block later/new completions. Retrying successful requests cannot signal
twice. This retry policy belongs to the native adapter, not a retail USB claim.

Native safety bounds are 4096 open handles, 4096 pending feedback buffers and
64 preallocation entries. Input handles are opaque monotonic tokens, not
memory-backed guest objects. Their range starts at 0x58000000; reserve it from
other handle families. Reusing a pending feedback address returns 170. Host
backends/cadence are supplied by the application; call sync_input before each
guest frame. Deliver SignalBatch only after dropping the guest memory and XAPI
guards: kernel DispatcherReference::signal_event relocks guest memory.

## Generated saves and config

The ignored [checkpoint acceptance harness](checkpoints.md) executes these generated save APIs, signatures and the game's checkpoint writer/revert/restore against synthetic state and private host storage, including a fresh Runtime. Detail: [XAPI: saves and the N: volume](xapi-saves.md).

### Damaged save metadata

XCreateSaveGame opens `SaveMeta.xbx` with FILE_OVERWRITE_IF and then writes it, so a process killed in between leaves it empty or torn. Detail: [XAPI: saves and the N: volume](xapi-saves.md).

## Opt-in native save games

`Config::save_games` (default false) makes `register` also install four overrides, `register::SAVE_OVERRIDES`. Detail: [XAPI: saves and the N: volume](xapi-saves.md).

## Launch, utility and startup

XLaunchNewImageA at `002d07f2` produces an owned host request: optional ANSI
path and optional 3072-byte launch payload. Null path requests the dashboard.
The runner must validate the guest image through its loader/VFS, stop the
current guest on success, and arrange reload outside the guest memory lock.
The Rust adapter returns Outcome::Launch with CPU/stack unchanged. Invalid
memory must not queue a request. This host lifecycle request is the reason
for the override; it never executes a host path/program directly.

XGetLaunchInfo at `002d0915` stays generated. Its payload lives at launch page
+0x400; kinds 2/3 bypass the certificate title-ID check, while other kinds must
match. Success copies 3072 bytes, clears LaunchDataPage, and frees the old page
through ordinal 171. No valid page returns 1168. Kernel and runner must share
that page; there is no second Rust incoming launch-data owner.

Utility mounting at `002d5b77` and formatting at `002d5c7c` use the Storage
interface over the same committed VFS as kernel file operations. Native policy
selects partition 3..5 from Config instead of reproducing retail cache LRU.
Nonformatting mounts preserve data; formatting can clear only the selected
utility volume/current Z:, never U:/T: or an arbitrary host path. Halo's
XapiInitProcess passes `fFormatClean = FALSE`: the XBE init flags are
`0x80000005`, and `002d1c3c` pushes `flags & 2`. Retail then formats only when
the database has no valid row for the title, and the kernel seeds one.

XapiInitProcess (`002d1b4e`), mainCRTStartup (`002d0aee`), mainXapiStartup
(`002d0a7a`), XapiThreadStartup (`002d212e`), `_rtinit` (`002d1d15`) and `_cinit`
(`002d1cbd`) remain generated. The initialiser creates the heap at `0055f148`,
verifies data-volume cluster size through `002d15f0`, and maps T:/U: through
`002d1a83`. The kernel/VFS must agree on 16 KiB FATX clusters and title roots.
Do not stub out heap, guest FS/TLS, exception setup, notifications or constructors.

The power-down initialiser at `002d6031` sets up a timer/DPC whose reset routine
is `002d6004`; the DPC at `002d601f` can request shutdown. Native policy disables
all of it. POWER_DOWN_OVERRIDES groups both hook functions: install/remove them
in the same dispatch edit, never register only the initialiser or reset routine.

The title-update path at `002d129f` can call `002d111e` directly, bypassing the
public launch hook; `002d0979` also reaches firmware reboot. Preserve generated
private paths and support the kernel LaunchDataPage/HalReturnToFirmware handoff.
The public launch request does not replace that contract. No multimedia timer
or fiber API was identified; GetTickCount (`003314b0`), performance counters
(`00331d2b` / `00331d3c`), Sleep/Ex (`002d6426` / `002d63a8`), ordinary waits,
timezone queries (`003317d9`) and audio flags (`003739fa`) remain generated.

## The second cache volume, N:

Halo has two HDD cache volumes. Detail: [XAPI: saves and the N: volume](xapi-saves.md).

## Runtime registration

See [XAPI: runtime registration and inventory](xapi-inventory.md).

### Boot integration

`crates/extensions/game/src/xapi_glue.rs` is this integration, in place. Detail: [XAPI: runtime registration and inventory](xapi-inventory.md).

## Validation

`cargo test -p xapi` uses synthetic guest memory and host pads. It covers the
32 pinned ABIs, every retained adapter's return cleanup, generated routing with
an invalid stack, Cur/Prev/Chg transitions, peek semantics, final disconnected
reports, generation lifetime, packed layouts, manual polling, unsupported
rumble, feedback completion/retry without head-of-line blocking, utility
forwarding, outgoing launch ownership, and prevalidation before effects.

`tests/cache.rs` tests the N: adapter. It checks the S0/BOOL ABI, row 2 and
Partition4 from the kernel seed, and that Z:'s row and database row are
unchanged. It checks the -1-untouched path while Z: is unmounted, and the
-1 path for a read failure, a bad version or marker, a format failure (after
the database write), an out-of-table Z: row and a single partition. It also
covers faults leaving the slot alone, the selection rule, the Fixed
conflicts, and a Storage without the new methods.

`tests/host_boot.rs` runs on the real Runner with the kernel's own
`rawdisk::RawDisk` and a VFS on host directories in `$TMPDIR`. It mounts Z:
then N:, checks the slots and links, that N: was emptied while Z:'s file
survived, the rewritten database row, a sharing-violation failure, a corrupt
database, and that XFormatUtilityDrive never empties N:. The default
keyboard pad appears through XInitDevices / XGetDeviceChanges / XInputOpen
on port 0 (port 1 fails with 1167) with SubType 1 and motor masks. Key
presses reach XInputGetState only after a pump, with a higher packet number.
Halo-style rumble (`hEvent = 0`) goes 997 then 0 with the motors forwarded,
and an evented request is signalled. There are no MU changes, and unplugging
gives 1167 plus a removal.

`tests/register.rs` goes through the real `runtime::Runner` and `Dispatch`:

- The installed set equals 16 literal address/ret-size pairs, written
  independently of `OVERRIDES` and `stack_words()`.
- Each address is called with HARNESS_EXIT as its return. The test checks
  argument decoding (output pointers, port/handle/format arguments, the last
  error written to a mapped TLS cell, the cache row) and that ESP rises by
  exactly `4 + ret N`.
- XLaunchNewImageA stops with the whole stack mapping byte-identical and
  CPU unchanged, and leaves the request queued.
- After `unregister`, every address goes to the runner's fallback
  (UnsupportedTarget, since no generated original exists) without touching
  the stack or the services.
- The cache row follows `GuestLayout::cache_row`.
- A guest memory fault becomes a runtime FAULT with its real reason. An
  unmapped TLS block faults before the service runs.
- `service_input` delivers the event with no guard held. The test's `Events`
  relocks guest memory on another thread with a timeout. Moving delivery
  inside the guard makes it fail.
- Registering twice, registering over a foreign override, and registering
  after `unregister` are all reported, and nothing is installed.

`tests/xbe_pins.rs` is ignored by default. It walks each of the 16 override
addresses and each of the 32 pinned addresses in the real XBE (iced-x86,
following direct jumps and the `004098c0` thunk). It requires the reachable
`ret` immediates to be exactly the pinned size. It also checks the thunk
target, the `push 004086f8` / `call XGetDeviceChanges` at `00124aae`, the
`_tls_index` and `fs:[4]` / `+8` reads in GetLastError, and the cache-row
store, its `row * 12` use for the partition index, and the -1 check. For
`002d5d0a` it checks the `0x12003` call, the title, count-import and
certificate reads, the 41-row clamp, the signature/marker/version constants,
STATUS_INTERNAL_DB_CORRUPTION, the store to `00461d84` and both -1 stores, the
formatter call, the link call and the plain `ret`. All of
these are checked as decoded mnemonics and operands. The repository holds
no instruction bytes copied from the XBE:

```sh
H2_XBE="$PWD/disc/default.xbe" \
  cargo test -p xapi --test xbe_pins -- --ignored
```

Run `cargo clippy -p xapi --no-deps --all-targets -- -D warnings` and
`cargo fmt -p xapi --check`.

## Campaign input acceptance and scripted runners

`input` supplies externally clocked timelines and `app::AppConfig` can
replace live input with one. [input.md](../input/input.md#deterministic-timelines-for-boot-and-headless-runners)
documents the integration: retain a `ScriptedInput` clone, call
`advance_to(relative_guest_ticks)` before `service_input`/`pump_input`, and
keep the usual status-write / release / signal / acknowledge sequence.
No new guest entry addresses or overrides are needed. XGetDevices and
XInputPoll remain available as adapters, without an invented retail address;
the generated device readers consume the published Cur/Chg/Prev object.

Synthetic `tests/input_semantics.rs` exercises the stdcall entries for every
button's press/hold/release, all pressure values, both sticks, packet changes,
manual polling, capabilities, feedback completion and insertion/removal with
stale handles. `tests/register.rs` also drives the actual registered
state/capabilities/feedback entries at 0x409b6c/0x409994/0x409bdf with a timeline.
`tests/mouse_look.rs` runs `sync_input` every slice and `XInputGetState` every
200 slices with the keyboard pad: each game-rate read must see the mouse
motion since the last one. `sync_input` refreshes every handle's last state
with `InputManager::peek_state`, not `get_state`, so that bookkeeping does not
count as the game reading the pad (docs/input/input.md).
The existing retry, disconnected snapshot, event pinning and memory-fault
tests remain applicable. These tests prove service semantics; observing
0x124c36's consumer and 0x2a9470..0x2a9550's Armory pitch gates during a retail
boot is left to the boot runner's integration checks.

## References

Behavioural addresses above refer to the user's supplied image and inspected
callers. Packed layout cross-checks use the pinned
[Cxbx XAPI declarations](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/585c49a50af1255ab155099e06f24505f9c5a800/src/core/hle/XAPI/Xapi.h).
See [input.md](../input/input.md), [storage.md](storage.md),
[architecture](../ARCHITECTURE.md) and [kernel-imports.md](kernel-imports.md) for
host and kernel contracts. External signatures and game disassembly stay out
of the repository.
