# XAPI: runtime registration and inventory

Status: Built. How handlers register, boot integration, and the inventory of matched and inferred entries. Part of [XAPI boundary](xapi.md).

## Runtime registration

`xapi::register` (`crates/platform/xapi/src/register.rs`) installs all 16 override
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
  `004098c0` thunk) are installed, and so are both power-down hooks and
  the N: mount at `002d5d0a`.
  XGetDevices and XInputPoll are not installed because they have no
  verified address.
- **Conflicts.** If any of the 16 addresses already has an override,
  `register` installs nothing and returns `RegisterError::Occupied` with
  every conflict (`addr`, `existing`, `ours`). Calling `register` twice gives
  16 conflicts with `existing == ours`. `Dispatch` cannot remove an override,
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
  XGetDeviceChanges, XPeekDevices, XInputOpen, XInputClose, Mount/Format,
  the N: mount)
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
  `cache_row = None` to skip publishing the row. The N: mount reads this
  row and never writes it. It publishes its own row at `00461d84`, and its
  addresses come from `Config::cache_drive.globals`, not from GuestLayout.
- **Input pump.** `service_input` runs the deferred signal flow:
  `sync_input` under the memory guard, `take_signals`, drop both guards,
  `SignalBatch::deliver(events)`, then lock XAPI again for
  `acknowledge_signals`. It returns the first error only after every step
  has run. Call it between guest slices, with no guard held and no runner
  active on the calling thread.

`Events` is the trait boundary to the kernel, and this crate does not
depend on `crates/runtime/kernel`. `fn signal(&mut self, guest_handle: u32) ->
xapi::Result<()>` is only ever called with no guest memory or XAPI guard
held.

### Boot integration

`crates/extensions/game/src/xapi_glue.rs` is this integration, in place. The kernel is
attached with its VFS first (`k.set_vfs(v)`, which also mounts the raw
disk), then:

1. `register_with` builds `xapi::VfsStorage::new(k.vfs()...)`, which formats
   through `Vfs::format_device` and not by deleting host directories under a
   live `HostBackend`, and `xapi::Config::for_gamepad`, then `xapi::register`
   with `GuestLayout::default()`. The VFS is captured once, with no guard
   held; never call `Kernel::vfs()` from inside an override.
2. The `InputManager` comes from the host: `register` makes a headless one
   with the keyboard pad (`H2_BOOT_INPUT` presses go through it), and
   `h2 play` passes the window's (`GuestHost::take_input`, player one's pad
   already on port 0).
3. Between slices the run loop calls
   `xapi::service_input(&loaded.runtime, x, &mut xapi_glue::KernelEvents(..))`
   with no guard held and no runner active. It runs the same sequence as
   `app::pump_input`, but works on a `runtime::Runtime`:
   `app::pump_input` needs a `GuestMemory` that owns its guard, and
   `register::RuntimeMemory` only borrows one, so the guard would still be
   held during delivery.
4. `--no-xapi` leaves XAPI unregistered and brings back the boot's own
   `0x2d5b77` XMountUtilityDrive override, which writes the Z: row at
   `xapi::register::addr::CACHE_ROW` (`0x461d80`). Without `--no-xapi` the
   boot registers no override at `0x2d5d0a`: `xapi::register` refuses
   everything with `Occupied` there.

What to expect: `0x2d5b77` returns 1 with `[0x461d80] = 0`. `0x2d5d0a`
returns 1 with `[0x461d84] = 2`, `\??\N:` linked to Partition4 (emptied),
and database row 2 = (title, 1, 0). The `n:\` savegame probes and the cache
slot 2-5 copies then go to a real volume. If `[0x461d84]` is -1 after
`0x12003`, read the guest last error: 31 means Z: is not mounted or the table
geometry is wrong, 1358 means the raw disk database is missing or corrupt,
and 32 means a file was open on the target volume.

`Config::utility_partition` defaults to 5, which matches the kernel default.
Keep the two equal.

### Remaining integration

- Pinning the kernel event when feedback is submitted (`00409bdf`), with
  `Kernel::reference_dispatcher`, is not wired. `Events::signal` resolves
  the handle at completion, so a handle that the guest closes in between
  fails the signal. The failure is requeued, not lost.
- Outgoing `LaunchRequest` and reload are host work. Incoming launch info is
  only the kernel page.
- A controller replug or title relaunch has not been run against these
  registrations. The boot itself runs them on every launch; the synthetic
  Runner tests are described below.
- N: is formatted on every boot (as retail). Its 64 KiB retail cluster size
  is not reproduced. If a generated `002d5f65` query on `n:\` must report
  64 KiB, the VFS volume needs a per-partition cluster size.

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
| `002d5d0a` | XMountCacheDrive, the N: mount (inferred) | S0 | R | 1/0/0 |
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
