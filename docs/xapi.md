# XAPI boundary

`crates/xapi` overrides services that need native host integration. Ordinary
XAPI routines stay generated over the kernel HLE. VFS is committed at
`50f4a95`; [storage.md](storage.md) describes its shared namespace and ownership.
The input lint was fixed at `983437d`. This lane rebased onto `f01debc`, later
than the requested `8e4dc8c`, and remains uncommitted.

No game bytes, executable excerpts, keys, signature patterns or assets are
included. `adapter.rs` is runtime-neutral. `register.rs` binds it to the
runtime (see [Runtime registration](#runtime-registration)).

## Evidence and ABI

The supplied retail 5849 XBE has **43 Xapi signature matches**. The inventory
adds **99 named entries, thunks and inferred helpers** from the existing Ghidra
project: **142 entries**. Inferred names are descriptions of inspected behavior,
not extra signature matches. All observed behavior below is anchored to its
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
[xdk-libraries.md](xdk-libraries.md) and the absolute disc path:

```sh
H2_XBSYMDB=<pinned-database-checkout> cargo run -p cli --release -- symbols /Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/default.xbe --list
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
explicit. `tests/routing.rs::VERIFIED_RET_SIZES` pins the **31 reviewed hook
address/ret-size pairs**, including the init thunk, independently of the adapter's
`stack_words()` calculation. It includes routines now routed to generated code.

## Override decisions

`Function::is_replacement()` is the executable policy. Generated routes return
`Outcome::Original` before reading even the guest stack and leave CPU, memory
and last error untouched. There are **15 identified override addresses**;
XGetDevices and XInputPoll remain unbound adapters until an address is verified.

| Addresses | Keep in Rust | Why the kernel path is insufficient |
| --- | --- | --- |
| `00408c57`, thunk `004098c0` | XInitDevices | Original enters USB/XPP initialization; native input backends replace that hardware stack. |
| `004098c5`, `00409884` | XGetDeviceChanges, XPeekDevices | Cur/Prev/Chg belong to the native input manager; both views must share that state. |
| `00409932`, `00409988` | XInputOpen, XInputClose | Native handles retain a host pad generation rather than a guest USB object. |
| `00409994`, `00409b6c`, `00409bdf` | Capabilities, GetState, SetState | Translate packed reports, disconnected snapshots and asynchronous host feedback. |
| `0041136f` | XReadMUMetaData | No memory units are exposed; return 1167 without entering the guest MU driver. |
| `002d5b77`, `002d5c7c` | Mount/format utility drive | Retail cache selection/formatting goes below ordinary files; delegate explicit utility-volume operations to the shared VFS. |
| `002d6031`, `002d6004` | Power-down init/reset | Native application policy disables the SMC timer/DPC. Install/remove both hooks in one dispatch-table edit using POWER_DOWN_OVERRIDES. |
| `002d07f2` | XLaunchNewImageA | A successful title transition must stop/reload the host runner; a kernel wrapper alone cannot arrange that transition. |

No save or config replacement is retained. This removes SaveRecord,
Disposition, DiskSpace, save cursors, Rust metadata packing, and the duplicate
incoming LaunchInfo store. The Storage contract now contains only utility
mount/format. XFindClose therefore always remains generated: there is no token
range heuristic or Rust `searches` table. Error 18 and name termination belong
to the generated save/file path, not to a second implementation.

| Addresses | Generated route | Required kernel behavior |
| --- | --- | --- |
| `00330df2`, `00331055`, `00331125`, `0033122f`, `00331276` | Create/Delete/FindFirst/FindNext/FindClose save APIs | Ordinary file/directory calls through shared VFS and RtlNtStatusToDosError (301). Preserve the original hash, metadata, cursor and close behavior. |
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
that compiled behavior while sharing the corrected state. No BOOL/consuming
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
[input.md](input.md). Polling parameters at `00409932` are four bytes: flags in
byte 0, intervals in bytes 1/2, reserved byte 3. Native policy rejects reserved
bits. Capabilities::standard supplies input masks only; the device manager adds
motor masks only for a backend with supports_rumble. Unsupported feedback
returns 50 without queuing a request and, like retail `0040a923`, writes 50 to
Header.dwStatus; a disconnected open handle writes 1167 (`0040aa0a`). Halo's
rumble loop at `00124f31` reads that status and ignores EAX. This models host capability at the
`00409bdf` service boundary; macOS gilrs rumble is unavailable.

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

XCreateSaveGame at `00330df2` retains the original UTF-16 name hash and ordinary
file operations. Metadata emission at `00330f8f` writes UTF-16LE: code unit FEFF
(bytes FF FE), then `Name=<name>\r\n`, followed by `NoCopy=1\r\n` when bit 0 is
set. The relevant strings are at `00416fb8`, `00417020`, `00417070`, `00417078`.
No Rust Storage contract duplicates this format now that saves are generated.

The scanner at `00330c15`, first/next at `00331125` / `0033122f`, and shared
close at `00331276` own save metadata, search lifetime and buffer packing.
XGAME_FIND_DATA has a 320-byte WIN32_FIND_DATAA prefix, an ANSI directory at
+320 (260 bytes), and UTF-16 name at +580 (128 units): 836 bytes total. Preserve
NUL termination for both find-data name arrays and the added strings; no raw
Rust prefix is accepted anymore. Empty/end enumeration must return error 18
through the generated file/search path. Retain generated XCalculateSignature*
and the kernel crypto HLE; this lane supplies no signing keys.

XGetLanguage at `0033039a` queries nonvolatile index 7, returns zero on query
failure, preserves values 0..9, and maps values above 9 to 1 (`003303b9`).
XGetVideoStandard at `00330310` queries index 0x103 and returns bits 8..15.
XGetVideoFlags at `00330339` queries index 8 and starts with
`(value >> 16) & 0x5f`; only AV pack 1 with standard !=3 retains every flag,
otherwise apply `flags & ~0x0e`. XGetAVPack at `003321bd` dereferences imported
HalBootSMCVideoMode; XGetGameRegion at `00331385` queries index 0x104 and
XGetParentalControlSetting at `002d5813` queries index 10. These derivations
remain in the generated originals; Config has no competing locale/video fields.

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
utility volume/current Z:, never U:/T: or an arbitrary host path.

XapiInitProcess (`002d1b4e`), mainCRTStartup (`002d0aee`), mainXapiStartup
(`002d0a7a`), XapiThreadStartup (`002d212e`), `_rtinit` (`002d1d15`) and `_cinit`
(`002d1cbd`) remain generated. The initializer creates the heap at `0055f148`,
verifies data-volume cluster size through `002d15f0`, and maps T:/U: through
`002d1a83`. The kernel/VFS must agree on 16 KiB FATX clusters and title roots.
Do not stub out heap, guest FS/TLS, exception setup, notifications or constructors.

The power-down initializer at `002d6031` sets up a timer/DPC whose reset routine
is `002d6004`; the DPC at `002d601f` can request shutdown. Native policy disables
all of it. POWER_DOWN_OVERRIDES groups both hook functions: install/remove them
in the same dispatch edit, never register only the initializer or reset routine.

The title-update path at `002d129f` can call `002d111e` directly, bypassing the
public launch hook; `002d0979` also reaches firmware reboot. Preserve generated
private paths and support the kernel LaunchDataPage/HalReturnToFirmware handoff.
The public launch request does not replace that contract. No multimedia timer
or fiber API was identified; GetTickCount (`003314b0`), performance counters
(`00331d2b` / `00331d3c`), Sleep/Ex (`002d6426` / `002d63a8`), ordinary waits,
timezone queries (`003317d9`) and audio flags (`003739fa`) remain generated.

## Kernel API requests

This is the exact integration surface requested from crates/kernel. Existing
NT file/event/clock wrappers remain dependencies, not additional XAPI overrides.

| Kernel surface | Required contract and guest consumer |
| --- | --- |
| RtlNtStatusToDosError, ordinal 301 | Correct NTSTATUS→Win32 mapping for generated file/save/disk wrappers and XapiSetLastNTError (`002d1d94`). No success fallback for unmapped errors. |
| ExQueryNonVolatileSetting, ordinal 24 | Typed DWORD settings (type 4, length 4, checked optional result length): indices 7/language, 8/video flags, 10/parental control, 0x103/video standard and 0x104/game region. Match generated derivations above and renderer/certificate settings. |
| HalBootSMCVideoMode, ordinal 356 data | Coherent guest value used by generated XGetAVPack (`003321bd`), consistent with video settings. |
| LaunchDataPage, ordinal 164 data; MmFreeContiguousMemory, ordinal 171 | Export a guest pointer cell to the shared persistent launch page, null when absent. Honor the original `002d0915` consume/free behavior exactly once; loader/runner must seed this same page. |
| HalReturnToFirmware, ordinal 49 | Stop/relaunch transfer and preserve launch state for private reboot paths (`002d111e`, `002d0979`), without returning to a dead guest. |
| Shared VFS access | A safe borrow/callback or shared synchronized handle to the kernel's one Vfs, never a second instance over the same roots. Needed to implement only Storage::mount_utility / format_utility for `002d5b77` / `002d5c7c`; explicit utility formatting must enforce open-object and share ownership. |
| Event handle reference/signal | Resolve and retain an event object by guest handle at feedback submission (`00409bdf`), then signal that retained object at completion even after handle close. Return typed guest failures; no guest execution/reentrant memory locking while signalling. The wrapper releases its reference after successful completion or cancellation. |

Generated save/disk acceptance also depends on the existing NtCreateFile,
NtOpenFile, NtReadFile, NtWriteFile, NtClose, NtQueryDirectoryFile,
NtQuery/SetInformationFile and NtQueryVolumeInformationFile paths being complete
on shared VFS. See [kernel-imports.md](kernel-imports.md) for their Xbox ABIs.
A failure in one of these is a kernel/VFS fix, not a reason to restore the
removed save/config overrides. GetLastError/SetLastError at `002d1d3e` /
`002d1d66` use guest FS:[4], `_tls_index`, then +8: the native adapter's per-thread
ThreadState only mirrors that guest cell for native BOOL/handle failures.

## Runtime registration

`xapi::register` (`crates/xapi/src/register.rs`) installs all 15 override
addresses in one dispatch edit, or none of them:

```rust
pub fn register<S: Storage + Send + 'static>(
    dispatch: &mut runtime::Dispatch,
    xapi: &Arc<Mutex<Xapi<S>>>,
    layout: GuestLayout,            // GuestLayout::default() for retail 5849
) -> Result<Registration, RegisterError>;
pub fn unregister(dispatch: &mut runtime::Dispatch, registration: &Registration);
pub fn service_input<S: Storage>(
    runtime: &runtime::Runtime,
    xapi: &Mutex<Xapi<S>>,
    events: &mut impl Events,
) -> xapi::Result<()>;
```

- **Addresses.** `register::OVERRIDES` lists address, `Function` and dispatch
  name (`xapi::<name>`). Each entry is stdcall, and the adapter pops
  `4 * stack_words()`, which equals the `ret N` pinned in
  `VERIFIED_RET_SIZES`. Both XInitDevices entries (`00408c57` and the
  `004098c0` thunk) are installed, and so are both power-down hooks.
  XGetDevices and XInputPoll are not installed because they have no
  verified address.
- **Conflicts.** If any of the 15 addresses already has an override,
  `register` installs nothing and returns `RegisterError::Occupied` with
  every conflict (`addr`, `existing`, `ours`). Calling `register` twice gives
  15 conflicts with `existing == ours`. `Dispatch` cannot remove an override,
  so `unregister` only disables the entries, and a later `register` reports
  them too.
- **One call.** The adapter takes `ctx.memory()` first and the XAPI mutex
  second, then runs `Xapi::invoke` over `register::RuntimeMemory`. That
  wrapper is a `GuestMemory` over the held guard. It uses alignment 1 and
  keeps the runtime's `GuestFault`, so a bad guest pointer becomes a runtime
  FAULT with its real reason. On `Outcome::Return` the adapter copies
  EAX/ESP back and returns `TransferAbi::ret(pc)`. On `Outcome::Launch` the
  CPU and stack stay untouched and the runner stops on an
  `Event::Unsupported { mnemonic: "xapi::XLaunchNewImageA", detail: "hle: ..." }`.
  The host then calls `Xapi::take_launch_request` and reloads outside every
  guard. (The runtime has no HLE-specific event yet. Use `ctx.report(id)` to
  render the stop.)
- **Last error.** The adapters that can set it (XInitDevices,
  XGetDeviceChanges, XPeekDevices, XInputOpen, XInputClose, Mount/Format)
  resolve the guest cell the same way GetLastError at `002d1d3e` does:
  `[[fs:4] + 4 * [_tls_index]] + 8`, with `_tls_index` at `00480100`. The
  cell is write-checked before the service runs and written only when the
  value changes. The status-returning APIs never touch it.
- **Cache row.** `00461d80` holds a row index into the utility-cache
  database (`\Device\Harddisk0\Partition0` + 0x800, 12-byte rows). It is
  not a partition number. The retail mount path stores the row at
  `002d5ac8`, then from `002d5b22` writes the partition index
  (`partition - 3`) into that row. `002d5d0a` re-reads the database and
  indexes it with the row, and fails with STATUS_UNSUCCESSFUL while the row
  is still -1. After a successful XMountUtilityDrive the adapter publishes
  `GuestLayout::cache_row` (`CacheRow { global, row }`) and does not rewrite
  the database. The row must name the entry the kernel already holds for
  the title. `kernel::rawdisk` (master `715b5ec`) seeds exactly one entry,
  the title with the kernel's `utility_partition`, in row 0, so the default
  is row 0. Keep `Config::utility_partition` equal to the kernel's. Set
  `cache_row = None` to skip publishing the row.
- **Input pump.** `service_input` runs the deferred signal flow:
  `sync_input` under the memory guard, `take_signals`, drop both guards,
  `SignalBatch::deliver(events)`, then lock XAPI again for
  `acknowledge_signals`. It returns the first error only after every step
  has run. Call it between guest slices, with no guard held and no runner
  active on the calling thread.

`Events` is the trait boundary to the kernel, and this crate does not
depend on `crates/kernel`. `fn signal(&mut self, guest_handle: u32) ->
xapi::Result<()>` is only ever called with no guest memory or XAPI guard
held.

### Boot integration

For `.claude/worktrees/boot`, this replaces `xapi_glue::install`. Remove that
override first, or `register` reports `002d5b77` as occupied. The kernel
must already be attached with its VFS set:

```rust
use std::sync::{Arc, Mutex};

/// xapi::Events over the kernel. Kernel::signal_event takes kernel state
/// and then guest memory, so it is only safe here, where service_input has
/// already dropped every guard.
struct KernelEvents(Arc<kernel::Kernel>);
impl xapi::Events for KernelEvents {
    fn signal(&mut self, handle: u32) -> xapi::Result<()> {
        self.0.signal_event(handle).map(drop).map_err(|e| match e {
            kernel::Error::Fault(f) => xapi::Error::Memory(xapi::MemoryFault {
                address: f.addr,
                length: f.len,
                write: true,
            }),
            _ => xapi::Error::INVALID_HANDLE,
        })
    }
}

/// xapi::Storage over the kernel's one VFS (never a second instance).
/// It is called with the guest memory and XAPI guards held, so it must not
/// touch kernel state: the kernel's order is kernel state, then guest memory,
/// then VFS, and `Kernel::vfs()` locks kernel state. Capture the shared VFS
/// `Arc` once at setup and take only its lock (memory, XAPI, VFS).
struct KernelStorage(Arc<Mutex<vfs::Vfs>>);
impl xapi::Storage for KernelStorage {
    fn mount_utility(&mut self, partition: u8, format: bool) -> xapi::Result<()> {
        let mut v = self.0.lock().unwrap();
        let _ = v.namespace.delete_symbolic_link("\\??\\Z:");
        v.namespace
            .create_symbolic_link("\\??\\Z:", &format!("\\Device\\Harddisk0\\Partition{partition}"))
            .map_err(|_| xapi::Error::Win32(31))?;
        if format {
            // Boot policy today: keep the cache. The Storage contract wants
            // only Z: cleared here; do that through the VFS when it can.
        }
        Ok(())
    }
    fn format_utility(&mut self) -> xapi::Result<()> {
        Ok(()) // same policy as above
    }
}

// After Kernel::for_image + set_vfs, before Machine::new:
let mut input = input::InputManager::new(input::host::ConvertConfig::default())?;
// input.add_backend(Box::new(...));   // gilrs/keyboard backend of choice
let xapi = Arc::new(Mutex::new(
    xapi::Xapi::new(
        xapi::Config::for_gamepad(xapi::register::addr::XDEVICE_TYPE_GAMEPAD),
        input,
        // Captured here, with no guest guard held; never Kernel::vfs() later.
        KernelStorage(k.vfs().context("VFS not attached")?),
    )
    .map_err(|e| anyhow::anyhow!("xapi: {e}"))?,
));
loaded
    .runtime
    .edit_dispatch(|d| xapi::register(d, &xapi, xapi::GuestLayout::default()))?;

// Between Machine::run slices (no guard held, no runner active):
let mut events = KernelEvents(Arc::clone(&k));
xapi::service_input(&loaded.runtime, &xapi, &mut events)?;

// On a stop whose event is Unsupported with mnemonic "xapi::XLaunchNewImageA":
if let Some(request) = xapi.lock().unwrap().take_launch_request() { /* reload */ }
```

`Config::utility_partition` defaults to 5, which matches the boot glue.
Keep it equal to the kernel's `utility_partition`.

### Remaining integration

- Pinning the kernel event when feedback is submitted (`00409bdf`), with
  `Kernel::reference_dispatcher`, is not wired. `Events::signal` resolves
  the handle at completion, so a handle that the guest closes in between
  fails the signal. The failure is requeued, not lost.
- Outgoing `LaunchRequest` and reload are host work. Incoming launch info is
  only the kernel page.
- No real boot, controller replug or title relaunch was run against these
  registrations. The synthetic Runner tests are described below.

## Inventory

`R`: native override. `K`: generated kernel wrapper. `G`: generated CRT/pure or
stateful glue. `B`: internal stack bypassed by public service replacement.
mainCRTStartup returns after creating the initial thread. mainXapiStartup has
an S4 epilogue after its dashboard path; XapiThreadStartup terminates its thread.
The weak ResetEvent signature at `002d1f3a` is actually named CloseHandle by
Ghidra and calls NtClose. Keep it generated; never install a ResetEvent hook
there. Internal GetTypeInformation naming differs between sources, so preserve
its observed register convention.

### 43 signature matches

| Address | Name | Compiled ABI | Route | C/J/D |
| --- | --- | --- | --- | --- |
| `002d07f2` | XLaunchNewImageA | S8 | R | 7/0/1 |
| `002d0915` | XGetLaunchInfo | S8 | K | 1/0/1 |
| `002d0a7a` | mainXapiStartup | S4; startup callback | G | 0/0/1 |
| `002d0aee` | mainCRTStartup | S0; entry | G | 0/0/1 |
| `002d1ad8` | XapiBootToDash | S12 | G | 4/0/0 |
| `002d1b4e` | XapiInitProcess | S0 | G | 1/0/0 |
| `002d1cbd` | _cinit | S0 | G | 1/0/0 |
| `002d1d15` | _rtinit | S0 | G | 1/0/0 |
| `002d1d3e` | GetLastError | S0 | G | 211/1/1 |
| `002d1d66` | SetLastError | S4 | G | 112/0/1 |
| `002d1d94` | XapiSetLastNTError | S4 | K | 58/0/0 |
| `002d1dab` | GetOverlappedResult | S16 | K | 3/0/1 |
| `002d1f3a` | ResetEvent | S4 | K | 105/0/1 |
| `002d1f7e` | XapiCallThreadNotifyRoutines | S4 | G | 2/0/0 |
| `002d1fb6` | SetThreadPriority | S8 | K | 4/0/1 |
| `002d202e` | RaiseException | S16 | K | 3/0/2 |
| `002d2084` | SwitchToThread | S0 | K | 70/0/1 |
| `002d20a9` | XRegisterThreadNotifyRoutine | S8 | G | 1/0/1 |
| `002d20f2` | UnhandledExceptionFilter | S4 | G | 0/0/1 |
| `002d212e` | XapiThreadStartup | stack callback, no return | G | 0/0/1 |
| `002d21c6` | CreateThread | S24 | K | 3/0/1 |
| `002d592c` | XapiSelectCachePartition | S12 | G | 1/0/0 |
| `002d5b77` | XMountUtilityDrive | S4 | R | 1/0/1 |
| `002d5c7c` | XFormatUtilityDrive | S0 | R | 1/0/1 |
| `002d6004` | XAutoPowerDownResetTimer | S0 | R | 2/0/1 |
| `002d6114` | XapiFormatObjectAttributes | S12 | G | 3/0/0 |
| `002d616a` | CreateEventA | S16 | K | 6/0/0 |
| `002d626c` | CreateMutexA | S12 | K | 4/0/0 |
| `002d6765` | XCalculateSignatureBegin | S4 | G | 9/0/1 |
| `003317d9` | GetTimeZoneInformation | S4 | K | 1/0/1 |
| `00331cf9` | OutputDebugStringA | S4 | K | 4/0/1 |
| `00331d2b` | QueryPerformanceCounter | S4 | K | 8/0/1 |
| `00408c57` | XInitDevices | S8 | R | 0/1/0 |
| `00408d4d` | IUsbInit_GetMaxDeviceTypeCount | S4 | B | 4/0/0 |
| `00409800` | GetTypeInformation2 | register ECX/EDX; ret | B | 3/0/0 |
| `004098c5` | XGetDeviceChanges | S12 | R | 2/0/1 |
| `00409932` | XInputOpen | S16 | R | 1/0/1 |
| `00409988` | XInputClose | S4 | R | 2/0/1 |
| `00409994` | XInputGetCapabilities | S8 | R | 1/0/1 |
| `00409b6c` | XInputGetState | S8 | R | 1/0/1 |
| `00409bdf` | XInputSetState | S8 | R | 2/0/1 |
| `0040a85e` | XID_fCloseDevice | register ECX; ret | B | 2/0/0 |
| `0041136f` | XReadMUMetaData | S20 | R | 1/0/1 |

### Additional named entries and inferred helpers

| Address | Name | Compiled ABI | Route | C/J/D |
| --- | --- | --- | --- | --- |
| `002d09c0` | XMemAllocDefault | S8 | G | 19/0/1 |
| `002d0a60` | XMemFreeDefault | S8 | G | 15/0/1 |
| `002d1404` | lstrcpynA | S12 | G | 20/0/1 |
| `002d1469` | lstrlenA | S4 | G | 6/0/1 |
| `002d14a7` | HeapCreate | S12 | K | 1/0/1 |
| `002d14f6` | HeapDestroy | S4 | K | 1/0/1 |
| `002d1512` | HeapFree | S12 | K | 2/0/1 |
| `002d152f` | LocalAlloc | S8 | K | 9/0/1 |
| `002d154c` | LocalFree | S4 | K | 8/0/1 |
| `002d156a` | XPhysicalAlloc | S16 | K | 2/0/1 |
| `002d15da` | XPhysicalProtect | S12 | K | 30/0/1 |
| `002d15ea` | XPhysicalFree | S4; kernel tail | K | 2/0/1 |
| `002d174f` | XapiMapLetterToDirectory | S24 | G | 2/0/0 |
| `002d1f58` | DuplicateHandle | S28 | K | 1/0/1 |
| `002d2008` | ResumeThread | S4 | K | 5/0/1 |
| `002d2097` | SetUnhandledExceptionFilter | S4 | K | 2/0/1 |
| `002d2234` | GetCurrentThreadId | S0 | K | 70/0/1 |
| `002d2241` | ReadFile | S20 | K | 36/0/1 |
| `002d232e` | WriteFile | S20 | K | 22/0/1 |
| `002d2404` | SetEndOfFile | S4 | K | 7/0/1 |
| `002d2488` | SetFilePointer | S16 | K | 41/0/1 |
| `002d2582` | SetFilePointerEx | S20 | K | 3/0/1 |
| `002d264a` | GetFileTime | S16 | K | 2/0/1 |
| `002d26b0` | FlushFileBuffers | S4 | K | 4/0/1 |
| `002d26d7` | GetFileSizeEx | S8 | K | 5/0/1 |
| `002d2715` | GetFileSize | S8 | K | 10/0/1 |
| `002d2919` | XComputeContentSignatureKey | S8 | G | 1/0/1 |
| `002d2c8f` | XGetContentInstallLocationFromIDs | S16 | G | 1/0/1 |
| `002d2ce1` | XInstallContentSignaturesWithFileName | S16 | G | 2/0/1 |
| `002d2ef9` | XInstallContentSignaturesEx | S16 | G | 2/0/1 |
| `002d2f2a` | XRemoveContent | S4 | G | 1/0/1 |
| `002d2f6d` | XLoadContentSignaturesWithFileName | S8 | G | 1/0/1 |
| `002d30d5` | XLoadContentSignaturesEx | S8 | G | 2/0/1 |
| `002d3100` | XCloseContentSignatures | S4 | G | 2/0/1 |
| `002d5794` | XLoadSectionByHandle | S4 | K | 4/0/1 |
| `002d57b5` | XFreeSectionByHandle | S4 | K | 4/0/1 |
| `002d5809` | XGetSectionSize | S4 | G | 3/0/1 |
| `002d5813` | XGetParentalControlSetting | S0 | K | 2/0/1 |
| `002d5838` | DiskFreeSpaceEx (inferred) | S16 | K | 5/0/0 |
| `002d5f65` | DiskClusterSize (inferred) | S4 | K | 1/0/0 |
| `002d6031` | XapiInitAutoPowerDown (inferred) | S0 | R | 1/0/0 |
| `002d61cb` | SetEvent | S4 | K | 57/0/1 |
| `002d6246` | ReleaseSemaphore | S12 | K | 5/0/1 |
| `002d62c4` | ReleaseMutex | S4 | K | 24/0/1 |
| `002d62e4` | WaitForSingleObjectEx | S12 | K | 1/0/1 |
| `002d6328` | WaitForMultipleObjectsEx | S20 | K | 1/0/1 |
| `002d63a8` | SleepEx | S8 | K | 1/0/1 |
| `002d63fa` | WaitForSingleObject | S8 | K | 36/0/1 |
| `002d640c` | WaitForMultipleObjects | S16 | K | 3/0/1 |
| `002d6426` | Sleep | S4 | K | 11/0/1 |
| `002d64bb` | XcSHAUpdate | S12; kernel tail | K | 6/0/0 |
| `002d6724` | XCalculateSignatureBeginEx | S8 | G | 1/0/1 |
| `002d6779` | XCalculateSignatureUpdate | S12 | G | 12/0/1 |
| `002d67c7` | XCalculateSignatureEnd | S8 | G | 10/0/1 |
| `002d6f28` | lstrlenW | S4 | G | 8/0/1 |
| `002d6f62` | XQueryValue | S20 | K | 3/0/1 |
| `00330310` | XGetVideoStandard | S0 | K | 2/0/1 |
| `00330339` | XGetVideoFlags | S0 | K | 1/0/1 |
| `00330382` | XSaveFloatingPointStateForDpc | S0 | K | 1/0/1 |
| `0033038e` | XRestoreFloatingPointStateForDpc | S0 | K | 1/0/1 |
| `0033039a` | XGetLanguage | S0 | K | 32/0/1 |
| `003303f3` | GetSystemTimeAsFileTime | S4; kernel tail | K | 23/0/1 |
| `0033080a` | XFindFirstContent | S12 | G | 1/0/1 |
| `0033095a` | XFindNextContent | S8 | G | 1/0/1 |
| `00330df2` | XCreateSaveGame | S24 | G | 4/0/1 |
| `00331055` | XDeleteSaveGame | S8 | G | 4/0/1 |
| `00331125` | XFindFirstSaveGame | S8 | G | 2/0/1 |
| `0033122f` | XFindNextSaveGame | S8 | G | 2/0/1 |
| `00331276` | XFindClose | S4 | G | 3/0/1 |
| `003312cc` | VirtualAlloc | S16 | K | 22/0/1 |
| `003312fa` | VirtualFree | S12 | K | 17/0/1 |
| `00331336` | GlobalMemoryStatus | S4 | K | 5/0/1 |
| `00331385` | XGetGameRegion | S0 | K | 2/0/1 |
| `00331406` | XapipQueryTimeZoneInformation | S8 | K | 1/0/1 |
| `003314b0` | GetTickCount | S0 | K | 328/0/1 |
| `003314b8` | FileTimeToSystemTime | S8 | K | 3/0/1 |
| `00331523` | SystemTimeToFileTime | S8 | K | 2/0/1 |
| `00331598` | CompareFileTime | S8 | K | 10/0/1 |
| `0033198b` | GetLocalTime | S4 | K | 1/0/1 |
| `00331a0a` | FileTimeToLocalFileTime | S8 | K | 2/0/1 |
| `00331d3c` | QueryPerformanceFrequency | S4 | K | 5/0/1 |
| `00331f5e` | MultiByteToWideChar | S24 | K | 15/0/1 |
| `00332019` | WideCharToMultiByte | S32 | K | 19/0/1 |
| `003320e1` | IsBadReadPtr | S8 | K | 2/0/1 |
| `00332146` | IsBadWritePtr | S8 | K | 1/0/1 |
| `003321af` | IsBadCodePtr | S4 | K | 1/0/1 |
| `003321bd` | XGetAVPack | S0 | K | 2/0/1 |
| `00409837` | GetTypeInformation_4 | register ECX; ret | B | 1/0/0 |
| `00409884` | XPeekDevices | S12 | R | 3/0/1 |
| `004098c0` | XInitDevices | jmp to S8 | R | 1/0/1 |
| `00012280` | XMemAlloc | S8 | G | 12/0/1 |
| `000122c0` | XMemFree | S8 | G | 19/0/1 |
| `002d0979` | RebootWithLaunchData (inferred) | S20 | G | 3/0/0 |
| `002d111e` | LaunchImageInternal (inferred) | S16 | G | 2/0/0 |
| `002d129f` | ApplyTitleUpdate (inferred) | S0 | G | 1/0/0 |
| `002d15f0` | VerifyVolumeClusterSize (inferred) | S8 | K | 4/0/0 |
| `002d1a83` | MapTitleDrives (inferred) | S8 | G | 1/0/0 |
| `002d601f` | AutoPowerDownDpc (inferred) | S16 | B | 0/0/1 |
| `003739fa` | XGetAudioFlags | S0 | K | 1/0/1 |

## Validation

`cargo test -p xapi` uses synthetic guest memory and host pads. It covers the
31 pinned ABIs, every retained adapter's return cleanup, generated routing with
an invalid stack, Cur/Prev/Chg transitions, peek semantics, final disconnected
reports, generation lifetime, packed layouts, manual polling, unsupported
rumble, feedback completion/retry without head-of-line blocking, utility
forwarding, outgoing launch ownership, and prevalidation before effects.

`tests/register.rs` goes through the real `runtime::Runner` and `Dispatch`:

- The installed set equals 15 literal address/ret-size pairs, written
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

`tests/xbe_pins.rs` is ignored by default. It walks each of the 15 override
addresses and each of the 31 pinned addresses in the real XBE (iced-x86,
following direct jumps and the `004098c0` thunk). It requires the reachable
`ret` immediates to be exactly the pinned size. It also checks the thunk
target, the `push 004086f8` / `call XGetDeviceChanges` at `00124aae`, the
`_tls_index` and `fs:[4]` / `+8` reads in GetLastError, and the cache-row
store, its `row * 12` use for the partition index, and the -1 check. All of
these are checked as decoded mnemonics and operands. The repository holds
no instruction bytes copied from the XBE:

```sh
H2_XBE=/Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/default.xbe \
  cargo test -p xapi --test xbe_pins -- --ignored
```

Run `cargo clippy -p xapi --no-deps --all-targets -- -D warnings` and
`cargo fmt -p xapi --check`.

## References

Behavioral addresses above refer to the user's supplied image and inspected
callers. Packed layout cross-checks use the pinned
[Cxbx XAPI declarations](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/585c49a50af1255ab155099e06f24505f9c5a800/src/core/hle/XAPI/Xapi.h).
See [input.md](input.md), [storage.md](storage.md),
[architecture](ARCHITECTURE.md) and [kernel-imports.md](kernel-imports.md) for
host and kernel contracts. External signatures and game disassembly stay out
of the repository.
