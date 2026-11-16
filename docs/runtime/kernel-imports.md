# Halo 2 kernel imports

Work list for the Rust kernel replacement: retail Halo 2, title `4D530064`,
linked against XBOXKRNL `1.0.5849`. The supplied XBE imports **152 exports:
135 functions and 17 data symbols**. All appear below, sorted by decimal
ordinal. No game bytes, keys or executable contents are reproduced.

## Evidence and ABI

`./target/release/h2 info` reports the thunk table at `0x00411520`.
Ordinals were read from its 152 nonzero 32-bit entries with the import bit
masked off; the next entry is zero. Names were matched by slot address to
`scripts/ghidra --json symbol list --limit 0 --filter "source=IMPORTED"`
and checked against the public [Cxbx ordinal table][ordinals]. In this
Ghidra project, `EXTERNAL:00000001` through `EXTERNAL:00000098` are synthetic
external IDs, **not kernel ordinals**: external ID 1 names
`MmPersistContiguousMemory`, ordinal 178. The slot addresses, rather than
external IDs, tie the sources together.

For every slot, `scripts/ghidra --json x-ref to <slot> --limit 0` supplied
references. Each function reference instruction was decoded with
`./target/release/h2 disasm disc/default.xbe <from-address> 1`.
The function table gives **C/J/R**: direct thunk `call` sites / thunk tail
`jmp` sites / all slot references, including pointer loads. These are static
counts across the analyzed image, including linked SDK code, not execution
frequencies or proof of reachability. Calls through a previously loaded
register and references missed by Ghidra are not counted as C/J. Data rows
show references only. Totals: 1,005 calls, 25 tail jumps and 1,152 references
(1,074 function-slot references and 78 data-slot references). Zero means none
found, not that an import can be omitted.

Signatures use the public [nxdk Xbox kernel header][nxdk], cross-checked
against the linked Cxbx declarations; parameter direction annotations are
omitted. `NTAPI` means guest x86 stdcall; `FASTCALL` uses ECX/EDX for its first
two eligible arguments; `CDECL` leaves stack cleanup to the caller.
`DbgPrint` is a variadic cdecl call despite the wiki table saying stdcall.
All pointers, handles, `SIZE_T`, `ULONG_PTR` and ordinary integer enums here
are **guest 32-bit** values; `BOOLEAN` and `KIRQL` are unsigned 8-bit,
`KPROCESSOR_MODE` is signed 8-bit, `WCHAR` is 16-bit, and
`LARGE_INTEGER`/`ULONGLONG` are 64-bit. Do not use macOS pointer/long/usize
sizes for guest structures. A `P...` type is a guest pointer, not a host pointer.
Xbox NT calls have fewer arguments than some Windows equivalents.

Difficulty estimates cover correct behavior in this runtime, not a success
stub. **† marks a public-source disagreement**, detailed below. Research
checked 2026-11-14; hardware details still need title-specific validation.

## Imported functions (135)

| Ordinal | Name | C signature | Category | Call sites C/J/R | What a Rust HLE needs to do | Difficulty |
| ---: | --- | --- | --- | ---: | --- | --- |
| 1 | [AvGetSavedDataAddress][xbox] | `PVOID NTAPI AvGetSavedDataAddress(void);` | hardware/HAL | 4/0/4 | Return the saved guest AV-data pointer. | trivial |
| 2 | [AvSendTVEncoderOption][xbox] | `VOID NTAPI AvSendTVEncoderOption(PVOID RegisterBase, ULONG Option, ULONG Param, PULONG Result);` | hardware/HAL | 7/0/8 | Decode encoder queries/options and keep video capabilities consistent with the renderer. | hard |
| 3 | [AvSetDisplayMode][xbox] | `ULONG NTAPI AvSetDisplayMode(PVOID RegisterBase, ULONG Step, ULONG DisplayMode, ULONG SourceColorFormat, ULONG Pitch, ULONG FrameBuffer);` | hardware/HAL | 2/0/2 | Translate staged mode, framebuffer, pitch and format changes to the video backend. | hard |
| 4 | [AvSetSavedDataAddress][xbox] | `VOID NTAPI AvSetSavedDataAddress(PVOID Address);` | hardware/HAL | 3/0/3 | Store the guest AV-data pointer. | trivial |
| 8 | [DbgPrint][dbg] | `ULONG CDECL DbgPrint(PCSTR Format, ...);` | debug/misc | 0/1/1 | Format guest varargs and strings into the runtime log; return the debug-print status. | moderate |
| 15 | [ExAllocatePoolWithTag][ex] | `PVOID NTAPI ExAllocatePoolWithTag(SIZE_T NumberOfBytes, ULONG Tag);` | memory | 14/0/16 | Allocate aligned guest pool memory, tracking size and tag; return null on failure. | moderate |
| 17 | [ExFreePool][ex] | `VOID NTAPI ExFreePool(PVOID P);` | memory | 10/1/13 | Release a tracked guest pool allocation. | moderate |
| 23 | [ExQueryPoolBlockSize][ex] | `ULONG NTAPI ExQueryPoolBlockSize(PVOID PoolBlock);` | memory | 2/0/2 | Return the actual tracked size of a guest pool block. | trivial |
| 24 | [ExQueryNonVolatileSetting][ex] | `NTSTATUS NTAPI ExQueryNonVolatileSetting(ULONG ValueIndex, PULONG Type, PVOID Value, ULONG ValueLength, PULONG ResultLength);` | hardware/HAL | 0/1/1 | Serve typed EEPROM-style settings with buffer-length checks and required-length output. | moderate |
| 44 | [HalGetInterruptVector][hal] | `ULONG NTAPI HalGetInterruptVector(ULONG BusInterruptLevel, PKIRQL Irql);` | hardware/HAL | 5/0/5 | Map an Xbox IRQ to a virtual interrupt vector and IRQL. | moderate |
| 46 | [HalReadWritePCISpace][hal] | `VOID NTAPI HalReadWritePCISpace(ULONG BusNumber, ULONG SlotNumber, ULONG RegisterNumber, PVOID Buffer, ULONG Length, BOOLEAN WritePCISpace);` | hardware/HAL | 0/0/1 | Implement the device PCI configuration fields the guest reads/writes through a virtual device model. | hard |
| 47 | [HalRegisterShutdownNotification][hal] | `VOID NTAPI HalRegisterShutdownNotification(PHAL_SHUTDOWN_REGISTRATION ShutdownRegistration, BOOLEAN Register);` | hardware/HAL | 10/0/11 | Register/remove guest shutdown callbacks and preserve their ordering. | moderate |
| 49 | [HalReturnToFirmware][hal] | `VOID DECLSPEC_NORETURN NTAPI HalReturnToFirmware(FIRMWARE_REENTRY Routine);` | hardware/HAL | 4/0/4 | End guest execution or relaunch through the runtime, preserving launch state; never return. | moderate |
| 62 | [IoBuildSynchronousFsdRequest][io] | `PIRP NTAPI IoBuildSynchronousFsdRequest(ULONG MajorFunction, PDEVICE_OBJECT DeviceObject, PVOID Buffer, ULONG Length, PLARGE_INTEGER StartingOffset, PKEVENT Event, PIO_STATUS_BLOCK IoStatusBlock);` | file I/O | 1/0/1 | Build a guest IRP with buffer, offset, event and completion status for the device stack. | hard |
| 65 | [IoCreateDevice][io] | `NTSTATUS NTAPI IoCreateDevice(PDRIVER_OBJECT DriverObject, ULONG DeviceExtensionSize, POBJECT_STRING DeviceName, DEVICE_TYPE DeviceType, BOOLEAN Exclusive, PDEVICE_OBJECT *DeviceObject);` | file I/O | 0/0/0 | Allocate a guest device object/extension and attach it to its driver and namespace. | hard |
| 67 | [IoCreateSymbolicLink][io] | `NTSTATUS NTAPI IoCreateSymbolicLink(POBJECT_STRING SymbolicLinkName, POBJECT_STRING DeviceName);` | file I/O | 6/0/6 | Add an Xbox device-path alias to the runtime namespace. | moderate |
| 69 | [IoDeleteSymbolicLink][io] | `NTSTATUS NTAPI IoDeleteSymbolicLink(POBJECT_STRING SymbolicLinkName);` | file I/O | 2/0/2 | Remove an Xbox device-path alias. | moderate |
| 74 | [IoInvalidDeviceRequest][io] | `NTSTATUS NTAPI IoInvalidDeviceRequest(PDEVICE_OBJECT DeviceObject, PIRP Irp);` | file I/O | 0/0/1 | Reject an unsupported device request and complete its IRP with the correct status. | moderate |
| 81 | [IoStartNextPacket][io] | `VOID NTAPI IoStartNextPacket(PDEVICE_OBJECT DeviceObject);` | file I/O | 8/0/8 | Advance the device packet queue and invoke its guest StartIo callback. | hard |
| 83 | [IoStartPacket][io] | `VOID NTAPI IoStartPacket(PDEVICE_OBJECT DeviceObject, PIRP Irp, PULONG Key);` | file I/O | 1/0/1 | Queue an IRP, preserving key order, and start device processing when idle. | hard |
| 84 | [IoSynchronousDeviceIoControlRequest][io] | `NTSTATUS NTAPI IoSynchronousDeviceIoControlRequest(ULONG IoControlCode, PDEVICE_OBJECT DeviceObject, PVOID InputBuffer, ULONG InputBufferLength, PVOID OutputBuffer, ULONG OutputBufferLength, PULONG ReturnedOutputBufferLength, BOOLEAN InternalDeviceIoControl);` | file I/O | 0/0/1 | Build/submit a device-control IRP and wait for completion, returning status and output length. | hard |
| 85 | [IoSynchronousFsdRequest][io] | `NTSTATUS NTAPI IoSynchronousFsdRequest(ULONG MajorFunction, PDEVICE_OBJECT DeviceObject, PVOID Buffer, ULONG Length, PLARGE_INTEGER StartingOffset);` | file I/O | 1/0/1 | Submit a synchronous filesystem request to the guest device stack. | hard |
| 86 | [IofCallDriver][io] | `NTSTATUS FASTCALL IofCallDriver(PDEVICE_OBJECT DeviceObject, PIRP Irp);` | file I/O | 1/0/1 | Dispatch an IRP to the correct guest driver routine and preserve stack-location rules. | hard |
| 87 | [IofCompleteRequest][io] | `VOID FASTCALL IofCompleteRequest(PIRP Irp, CCHAR PriorityBoost);` | file I/O | 9/0/9 | Unwind IRP completion routines, signal events/APCs and release request resources. | hard |
| 95 | [KeBugCheck][kernel] | `VOID NTAPI DECLSPEC_NORETURN KeBugCheck(ULONG BugCheckCode);` | debug/misc | 4/0/4 | Capture the bugcheck code and stop the guest permanently. | trivial |
| 97 | [KeCancelTimer][kernel] | `BOOLEAN NTAPI KeCancelTimer(PKTIMER Timer);` | time | 26/0/26 | Cancel an armed guest timer and return whether it was queued. | moderate |
| 98 | [KeConnectInterrupt][kernel] | `BOOLEAN NTAPI KeConnectInterrupt(PKINTERRUPT Interrupt);` | hardware/HAL | 5/0/5 | Register a guest ISR with the virtual interrupt source and return connection status. | hard |
| 99 | [KeDelayExecutionThread][kernel] | `NTSTATUS NTAPI KeDelayExecutionThread(KPROCESSOR_MODE WaitMode, BOOLEAN Alertable, PLARGE_INTEGER Interval);` | time | 3/0/3 | Suspend only the calling guest thread until its deadline or alert/APC. | hard |
| 100 | [KeDisconnectInterrupt][kernel] † | `BOOLEAN NTAPI KeDisconnectInterrupt(PKINTERRUPT Interrupt);` | hardware/HAL | 4/0/4 | Disconnect a guest ISR safely against in-flight delivery; return type disputed (†). | hard |
| 107 | [KeInitializeDpc][kernel] | `VOID NTAPI KeInitializeDpc(KDPC *Dpc, PKDEFERRED_ROUTINE DeferredRoutine, PVOID DeferredContext);` | threads/sync | 12/0/13 | Initialize guest DPC callback, context and queue state. | moderate |
| 109 | [KeInitializeInterrupt][kernel] | `VOID NTAPI KeInitializeInterrupt(PKINTERRUPT Interrupt, PKSERVICE_ROUTINE ServiceRoutine, PVOID ServiceContext, ULONG Vector, KIRQL Irql, KINTERRUPT_MODE InterruptMode, BOOLEAN ShareVector);` | hardware/HAL | 5/0/5 | Initialize the guest interrupt object, callback, vector, IRQL and sharing mode. | hard |
| 113 | [KeInitializeTimerEx][kernel] | `VOID NTAPI KeInitializeTimerEx(PKTIMER Timer, TIMER_TYPE Type);` | time | 9/0/9 | Initialize notification/synchronization timer state in guest memory. | moderate |
| 119 | [KeInsertQueueDpc][kernel] | `BOOLEAN NTAPI KeInsertQueueDpc(PRKDPC Dpc, PVOID SystemArgument1, PVOID SystemArgument2);` | threads/sync | 7/0/7 | Queue a DPC once with its two guest arguments and dispatch at emulated DISPATCH_LEVEL. | hard |
| 126 | [KeQueryPerformanceCounter][kernel] | `ULONGLONG NTAPI KeQueryPerformanceCounter(void);` | time | 1/0/1 | Return a monotonic 64-bit guest performance counter in EDX:EAX. | moderate |
| 127 | [KeQueryPerformanceFrequency][kernel] | `ULONGLONG NTAPI KeQueryPerformanceFrequency(void);` | time | 1/0/1 | Return the matching counter frequency, independent of the host CPU frequency. | trivial |
| 128 | [KeQuerySystemTime][kernel] | `VOID NTAPI KeQuerySystemTime(PLARGE_INTEGER CurrentTime);` | time | 17/1/18 | Write NT system time in 100 ns units since 1601, using the guest clock offset. | moderate |
| 129 | [KeRaiseIrqlToDpcLevel][kernel] | `KIRQL NTAPI KeRaiseIrqlToDpcLevel(void);` | threads/sync | 119/0/121 | Raise guest IRQL to DISPATCH_LEVEL and return its previous value. | hard |
| 137 | [KeRemoveQueueDpc][kernel] | `BOOLEAN NTAPI KeRemoveQueueDpc(PRKDPC Dpc);` | threads/sync | 5/0/6 | Remove a queued DPC atomically and report whether it was present. | hard |
| 139 | [KeRestoreFloatingPointState][kernel] | `NTSTATUS NTAPI KeRestoreFloatingPointState(PKFLOATING_SAVE FloatSave);` | debug/misc | 7/0/7 | Restore the saved guest x87/SSE state and control words. | moderate |
| 142 | [KeSaveFloatingPointState][kernel] | `NTSTATUS NTAPI KeSaveFloatingPointState(PKFLOATING_SAVE FloatSave);` | debug/misc | 7/0/7 | Save guest x87/SSE state and establish the kernel floating-point environment. | moderate |
| 143 | [KeSetBasePriorityThread][kernel] | `LONG NTAPI KeSetBasePriorityThread(PKTHREAD Thread, LONG Increment);` | threads/sync | 1/0/1 | Adjust a guest thread base priority and return the previous increment. | moderate |
| 145 | [KeSetEvent][kernel] | `LONG NTAPI KeSetEvent(PRKEVENT Event, KPRIORITY Increment, BOOLEAN Wait);` | threads/sync | 27/0/29 | Signal the dispatcher event, return its previous state, and honor wake/reset/Wait semantics. | hard |
| 149 | [KeSetTimer][kernel] | `BOOLEAN NTAPI KeSetTimer(PKTIMER Timer, LARGE_INTEGER DueTime, PKDPC Dpc);` | time | 20/0/20 | Arm a one-shot guest timer with optional DPC; return its previous queued state. | hard |
| 150 | [KeSetTimerEx][kernel] | `BOOLEAN NTAPI KeSetTimerEx(PKTIMER Timer, LARGE_INTEGER DueTime, LONG Period, PKDPC Dpc);` | time | 1/0/1 | Arm/rearm a guest timer with a millisecond period and optional DPC. | hard |
| 151 | [KeStallExecutionProcessor][kernel] | `VOID NTAPI KeStallExecutionProcessor(ULONG MicroSeconds);` | time | 9/0/12 | Advance or wait for the requested microsecond interval without losing device timing. | moderate |
| 153 | [KeSynchronizeExecution][kernel] | `BOOLEAN NTAPI KeSynchronizeExecution(PKINTERRUPT Interrupt, PKSYNCHRONIZE_ROUTINE SynchronizeRoutine, PVOID SynchronizeContext);` | hardware/HAL | 1/0/2 | Run a guest callback while excluding its ISR at the interrupt synchronization level. | hard |
| 158 | [KeWaitForMultipleObjects][kernel] | `NTSTATUS NTAPI KeWaitForMultipleObjects(ULONG Count, PVOID Object[], WAIT_TYPE WaitType, KWAIT_REASON WaitReason, KPROCESSOR_MODE WaitMode, BOOLEAN Alertable, PLARGE_INTEGER Timeout, PKWAIT_BLOCK WaitBlockArray);` | threads/sync | 1/0/1 | Wait on guest objects with WaitAny/WaitAll, timeout, alert/APC and consumption rules. | hard |
| 159 | [KeWaitForSingleObject][kernel] | `NTSTATUS NTAPI KeWaitForSingleObject(PVOID Object, KWAIT_REASON WaitReason, KPROCESSOR_MODE WaitMode, BOOLEAN Alertable, PLARGE_INTEGER Timeout);` | threads/sync | 9/0/11 | Wait on a guest dispatcher object with timeout and alert/APC handling. | hard |
| 160 | [KfRaiseIrql][xbox] | `KIRQL FASTCALL KfRaiseIrql(KIRQL NewIrql);` | threads/sync | 2/0/2 | Raise the emulated IRQL, return its previous level, and defer masked work. | hard |
| 161 | [KfLowerIrql][xbox] | `VOID FASTCALL KfLowerIrql(KIRQL NewIrql);` | threads/sync | 136/4/140 | Lower emulated IRQL and deliver newly eligible interrupt/DPC work. | hard |
| 165 | [MmAllocateContiguousMemory][mm] | `PVOID NTAPI MmAllocateContiguousMemory(SIZE_T NumberOfBytes);` | memory | 2/0/2 | Allocate physically contiguous guest pages with Xbox alignment and address constraints. | hard |
| 166 | [MmAllocateContiguousMemoryEx][mm] | `PVOID NTAPI MmAllocateContiguousMemoryEx(SIZE_T NumberOfBytes, ULONG_PTR LowestAcceptableAddress, ULONG_PTR HighestAcceptableAddress, ULONG_PTR Alignment, ULONG Protect);` | memory | 5/0/5 | Honor guest physical low/high bounds, alignment, protection and allocation failure. | hard |
| 167 | [MmAllocateSystemMemory][mm] | `PVOID NTAPI MmAllocateSystemMemory(SIZE_T NumberOfBytes, ULONG Protect);` | memory | 1/0/1 | Allocate page-backed guest system memory with the requested protection. | moderate |
| 168 | [MmClaimGpuInstanceMemory][mm] | `PVOID NTAPI MmClaimGpuInstanceMemory(SIZE_T NumberOfBytes, SIZE_T *NumberOfPaddingBytes);` | memory | 1/0/1 | Reserve GPU instance memory and report its padding consistently with GPU addresses. | hard |
| 169 | [MmCreateKernelStack][mm] | `PVOID NTAPI MmCreateKernelStack(SIZE_T NumberOfBytes, BOOLEAN DebuggerThread);` | memory | 1/0/1 | Allocate a guest kernel stack with correct bounds and debugger-thread policy. | moderate |
| 170 | [MmDeleteKernelStack][mm] | `VOID NTAPI MmDeleteKernelStack(PVOID KernelStackBase, PVOID KernelStackLimit);` | memory | 2/0/2 | Free the guest stack identified by its base and limit. | moderate |
| 171 | [MmFreeContiguousMemory][mm] | `VOID NTAPI MmFreeContiguousMemory(PVOID BaseAddress);` | memory | 5/2/7 | Release contiguous guest pages and all related virtual/physical bookkeeping. | hard |
| 172 | [MmFreeSystemMemory][mm] | `ULONG NTAPI MmFreeSystemMemory(PVOID BaseAddress, SIZE_T NumberOfBytes);` | memory | 2/0/2 | Release guest system pages and return the number of pages freed. | moderate |
| 173 | [MmGetPhysicalAddress][mm] | `ULONG_PTR NTAPI MmGetPhysicalAddress(PVOID BaseAddress);` | memory | 13/0/13 | Translate a guest virtual address to the guest physical address used by devices. | hard |
| 175 | [MmLockUnlockBufferPages][mm] | `VOID NTAPI MmLockUnlockBufferPages(PVOID BaseAddress, SIZE_T NumberOfBytes, BOOLEAN UnlockPages);` | memory | 12/0/13 | Pin/unpin guest buffer pages and preserve DMA lifetime and address mappings. | hard |
| 176 | [MmLockUnlockPhysicalPage][mm] | `VOID NTAPI MmLockUnlockPhysicalPage(ULONG_PTR PhysicalAddress, BOOLEAN UnlockPage);` | memory | 2/0/3 | Pin/unpin the guest physical page and track lock counts. | hard |
| 178 | [MmPersistContiguousMemory][mm] | `VOID NTAPI MmPersistContiguousMemory(PVOID BaseAddress, SIZE_T NumberOfBytes, BOOLEAN Persist);` | memory | 3/0/3 | Mark contiguous guest memory for retention across a quick reboot. | hard |
| 179 | [MmQueryAddressProtect][mm] | `ULONG NTAPI MmQueryAddressProtect(PVOID VirtualAddress);` | memory | 2/0/2 | Return the tracked Xbox protection flags for a guest address. | trivial |
| 180 | [MmQueryAllocationSize][mm] | `SIZE_T NTAPI MmQueryAllocationSize(PVOID BaseAddress);` | memory | 4/0/4 | Return the tracked allocation size containing the supplied guest address. | trivial |
| 181 | [MmQueryStatistics][mm] | `NTSTATUS NTAPI MmQueryStatistics(PMM_STATISTICS MemoryStatistics);` | memory | 2/0/2 | Validate the statistics structure length and report consistent guest page usage. | moderate |
| 182 | [MmSetAddressProtect][mm] | `VOID NTAPI MmSetAddressProtect(PVOID BaseAddress, ULONG NumberOfBytes, ULONG NewProtect);` | memory | 2/1/4 | Update guest page protection metadata and enforce access rules in Guest. | moderate |
| 184 | [NtAllocateVirtualMemory][nt] | `NTSTATUS NTAPI NtAllocateVirtualMemory(PVOID *BaseAddress, ULONG_PTR ZeroBits, PSIZE_T RegionSize, ULONG AllocationType, ULONG Protect);` | memory | 7/0/9 | Implement guest reserve/commit, rounding, placement, protection and in/out region fields. | hard |
| 187 | [NtClose][nt] | `NTSTATUS NTAPI NtClose(HANDLE Handle);` | threads/sync | 33/0/33 | Close a guest handle, release its reference and perform last-handle cleanup. | moderate |
| 189 | [NtCreateEvent][nt] | `NTSTATUS NTAPI NtCreateEvent(PHANDLE EventHandle, POBJECT_ATTRIBUTES ObjectAttributes, EVENT_TYPE EventType, BOOLEAN InitialState);` | threads/sync | 1/0/1 | Create/open a named or unnamed guest event with its type and initial signal state. | moderate |
| 190 | [NtCreateFile][nt] | `NTSTATUS NTAPI NtCreateFile(PHANDLE FileHandle, ACCESS_MASK DesiredAccess, POBJECT_ATTRIBUTES ObjectAttributes, PIO_STATUS_BLOCK IoStatusBlock, PLARGE_INTEGER AllocationSize, ULONG FileAttributes, ULONG ShareAccess, ULONG CreateDisposition, ULONG CreateOptions);` | file I/O | 9/0/10 | Resolve Xbox paths and implement file disposition, sharing, options and I/O status. | hard |
| 192 | [NtCreateMutant][nt] | `NTSTATUS NTAPI NtCreateMutant(PHANDLE MutantHandle, POBJECT_ATTRIBUTES ObjectAttributes, BOOLEAN InitialOwner);` | threads/sync | 1/0/1 | Create a guest recursive owner-tracked mutant, including initial ownership. | moderate |
| 193 | [NtCreateSemaphore][nt] | `NTSTATUS NTAPI NtCreateSemaphore(PHANDLE SemaphoreHandle, POBJECT_ATTRIBUTES ObjectAttributes, LONG InitialCount, LONG MaximumCount);` | threads/sync | 1/0/1 | Create a guest semaphore with validated initial/maximum counts. | moderate |
| 195 | [NtDeleteFile][nt] | `NTSTATUS NTAPI NtDeleteFile(POBJECT_ATTRIBUTES ObjectAttributes);` | file I/O | 1/0/1 | Resolve the Xbox path and apply delete/share semantics to the writable backing store. | moderate |
| 196 | [NtDeviceIoControlFile][nt] | `NTSTATUS NTAPI NtDeviceIoControlFile(HANDLE FileHandle, HANDLE Event, PIO_APC_ROUTINE ApcRoutine, PVOID ApcContext, PIO_STATUS_BLOCK IoStatusBlock, ULONG IoControlCode, PVOID InputBuffer, ULONG InputBufferLength, PVOID OutputBuffer, ULONG OutputBufferLength);` | file I/O | 0/0/1 | Decode supported device IOCTLs and complete status, event and APC outputs correctly. | hard |
| 197 | [NtDuplicateObject][nt] | `NTSTATUS NTAPI NtDuplicateObject(HANDLE SourceHandle, PHANDLE TargetHandle, ULONG Options);` | threads/sync | 1/0/1 | Duplicate a guest handle with the requested close-source/same-access semantics. | moderate |
| 198 | [NtFlushBuffersFile][nt] | `NTSTATUS NTAPI NtFlushBuffersFile(HANDLE FileHandle, PIO_STATUS_BLOCK IoStatusBlock);` | file I/O | 1/0/1 | Flush the backing file and fill the guest I/O status block. | moderate |
| 199 | [NtFreeVirtualMemory][nt] | `NTSTATUS NTAPI NtFreeVirtualMemory(PVOID *BaseAddress, PSIZE_T RegionSize, ULONG FreeType);` | memory | 10/0/10 | Implement guest decommit/release rules and update base/size outputs. | hard |
| 200 | [NtFsControlFile][nt] | `NTSTATUS NTAPI NtFsControlFile(HANDLE FileHandle, HANDLE Event, PIO_APC_ROUTINE ApcRoutine, PVOID ApcContext, PIO_STATUS_BLOCK IoStatusBlock, ULONG FsControlCode, PVOID InputBuffer, ULONG InputBufferLength, PVOID OutputBuffer, ULONG OutputBufferLength);` | file I/O | 1/0/1 | Implement requested filesystem controls and their asynchronous completion contract. | hard |
| 202 | [NtOpenFile][nt] | `NTSTATUS NTAPI NtOpenFile(PHANDLE FileHandle, ACCESS_MASK DesiredAccess, POBJECT_ATTRIBUTES ObjectAttributes, PIO_STATUS_BLOCK IoStatusBlock, ULONG ShareAccess, ULONG OpenOptions);` | file I/O | 13/0/14 | Open an existing Xbox path with access/share checks and guest I/O status. | moderate |
| 203 | [NtOpenSymbolicLinkObject][nt] | `NTSTATUS NTAPI NtOpenSymbolicLinkObject(PHANDLE LinkHandle, POBJECT_ATTRIBUTES ObjectAttributes);` | file I/O | 4/0/4 | Open a guest symbolic-link object by Xbox namespace attributes. | moderate |
| 207 | [NtQueryDirectoryFile][nt] | `NTSTATUS NTAPI NtQueryDirectoryFile(HANDLE FileHandle, HANDLE Event, PIO_APC_ROUTINE ApcRoutine, PVOID ApcContext, PIO_STATUS_BLOCK IoStatusBlock, PVOID FileInformation, ULONG Length, FILE_INFORMATION_CLASS FileInformationClass, POBJECT_STRING FileName, BOOLEAN RestartScan);` | file I/O | 3/0/3 | Enumerate with Xbox masks, restart state and packed guest directory records. | moderate |
| 210 | [NtQueryFullAttributesFile][nt] | `NTSTATUS NTAPI NtQueryFullAttributesFile(POBJECT_ATTRIBUTES ObjectAttributes, PFILE_NETWORK_OPEN_INFORMATION FileInformation);` | file I/O | 2/0/2 | Return guest file sizes, attributes and NT timestamps without opening a persistent handle. | moderate |
| 211 | [NtQueryInformationFile][nt] | `NTSTATUS NTAPI NtQueryInformationFile(HANDLE FileHandle, PIO_STATUS_BLOCK IoStatusBlock, PVOID FileInformation, ULONG Length, FILE_INFORMATION_CLASS FileInformationClass);` | file I/O | 9/0/9 | Encode the requested Xbox FILE_INFORMATION_CLASS into guest memory. | moderate |
| 215 | [NtQuerySymbolicLinkObject][nt] | `NTSTATUS NTAPI NtQuerySymbolicLinkObject(HANDLE LinkHandle, POBJECT_STRING LinkTarget, PULONG ReturnedLength);` | file I/O | 4/0/4 | Copy a symbolic-link target to the counted guest string and return required length. | moderate |
| 217 | [NtQueryVirtualMemory][nt] | `NTSTATUS NTAPI NtQueryVirtualMemory(PVOID BaseAddress, PMEMORY_BASIC_INFORMATION MemoryInformation);` | memory | 0/0/1 | Report guest region base, size, state, type and protection. | moderate |
| 218 | [NtQueryVolumeInformationFile][nt] | `NTSTATUS NTAPI NtQueryVolumeInformationFile(HANDLE FileHandle, PIO_STATUS_BLOCK IoStatusBlock, PVOID FsInformation, ULONG Length, FS_INFORMATION_CLASS FsInformationClass);` | file I/O | 3/0/3 | Encode requested guest volume size/device/filesystem information. | moderate |
| 219 | [NtReadFile][nt] | `NTSTATUS NTAPI NtReadFile(HANDLE FileHandle, HANDLE Event, PIO_APC_ROUTINE ApcRoutine, PVOID ApcContext, PIO_STATUS_BLOCK IoStatusBlock, PVOID Buffer, ULONG Length, PLARGE_INTEGER ByteOffset);` | file I/O | 7/0/8 | Read disc/backing-file bytes and implement offsets, pending I/O, event/APC and status outputs. | hard |
| 221 | [NtReleaseMutant][nt] | `NTSTATUS NTAPI NtReleaseMutant(HANDLE MutantHandle, PLONG PreviousCount);` | threads/sync | 1/0/1 | Release the calling thread ownership/recursion and wake an eligible mutant waiter. | moderate |
| 222 | [NtReleaseSemaphore][nt] | `NTSTATUS NTAPI NtReleaseSemaphore(HANDLE SemaphoreHandle, LONG ReleaseCount, PLONG PreviousCount);` | threads/sync | 1/0/1 | Validate and add semaphore units atomically, returning the previous count. | moderate |
| 224 | [NtResumeThread][nt] | `NTSTATUS NTAPI NtResumeThread(HANDLE ThreadHandle, PULONG PreviousSuspendCount);` | threads/sync | 1/0/1 | Decrement the guest suspend count, return its old value and schedule when runnable. | hard |
| 225 | [NtSetEvent][nt] | `NTSTATUS NTAPI NtSetEvent(HANDLE EventHandle, PLONG PreviousState);` | threads/sync | 4/0/4 | Resolve an event handle, signal it and return the previous state. | moderate |
| 226 | [NtSetInformationFile][nt] | `NTSTATUS NTAPI NtSetInformationFile(HANDLE FileHandle, PIO_STATUS_BLOCK IoStatusBlock, PVOID FileInformation, ULONG Length, FILE_INFORMATION_CLASS FileInformationClass);` | file I/O | 12/0/14 | Implement requested file position, EOF, rename and disposition information classes. | moderate |
| 228 | [NtSetSystemTime][nt] | `NTSTATUS NTAPI NtSetSystemTime(PLARGE_INTEGER SystemTime, PLARGE_INTEGER PreviousTime);` | time | 1/0/1 | Update the guest wall-clock offset and optionally return the previous NT time. | moderate |
| 233 | [NtWaitForSingleObject][nt] | `NTSTATUS NTAPI NtWaitForSingleObject(HANDLE Handle, BOOLEAN Alertable, PLARGE_INTEGER Timeout);` | threads/sync | 2/0/2 | Resolve a handle and wait with alertability and relative/absolute timeout semantics. | hard |
| 234 | [NtWaitForSingleObjectEx][nt] | `NTSTATUS NTAPI NtWaitForSingleObjectEx(HANDLE Handle, KPROCESSOR_MODE WaitMode, BOOLEAN Alertable, PLARGE_INTEGER Timeout);` | threads/sync | 1/0/1 | Implement handle waiting with explicit guest processor mode and alertability. | hard |
| 235 | [NtWaitForMultipleObjectsEx][nt] | `NTSTATUS NTAPI NtWaitForMultipleObjectsEx(ULONG Count, CONST HANDLE Handles[], WAIT_TYPE WaitType, KPROCESSOR_MODE WaitMode, BOOLEAN Alertable, PLARGE_INTEGER Timeout);` | threads/sync | 1/0/1 | Implement atomic handle WaitAny/WaitAll with timeout and alert/APC handling. | hard |
| 236 | [NtWriteFile][nt] | `NTSTATUS NTAPI NtWriteFile(HANDLE FileHandle, HANDLE Event, PIO_APC_ROUTINE ApcRoutine, PVOID ApcContext, PIO_STATUS_BLOCK IoStatusBlock, PVOID Buffer, ULONG Length, PLARGE_INTEGER ByteOffset);` | file I/O | 13/0/13 | Write to guest writable storage with offsets, sharing and event/APC completion semantics. | hard |
| 238 | [NtYieldExecution][nt] † | `NTSTATUS NTAPI NtYieldExecution(void);` | threads/sync | 1/0/1 | Yield the guest scheduler; return type disputed (†). | moderate |
| 246 | [ObReferenceObjectByHandle][ob] | `NTSTATUS NTAPI ObReferenceObjectByHandle(HANDLE Handle, POBJECT_TYPE ObjectType, PVOID *ReturnedObject);` | threads/sync | 9/0/9 | Validate a guest handle/type, retain the object and return its guest address. | moderate |
| 247 | [ObReferenceObjectByName][ob] | `NTSTATUS NTAPI ObReferenceObjectByName(POBJECT_STRING ObjectName, ULONG Attributes, POBJECT_TYPE ObjectType, PVOID ParseContext, PVOID *Object);` | threads/sync | 1/0/1 | Resolve a named guest object, applying type checks and parse callbacks. | hard |
| 250 | [ObfDereferenceObject][ob] | `VOID FASTCALL ObfDereferenceObject(PVOID Object);` | threads/sync | 14/1/15 | Release a guest object reference and destroy it only after all references expire. | moderate |
| 252 | [PhyGetLinkState][xbox] † | `DWORD NTAPI PhyGetLinkState(BOOLEAN update);` | hardware/HAL | 3/0/3 | Report virtual Ethernet link/speed/duplex state; argument width disputed (†). | moderate |
| 253 | [PhyInitialize][xbox] † | `NTSTATUS NTAPI PhyInitialize(BOOLEAN forceReset, PVOID param);` | hardware/HAL | 2/0/2 | Initialize the virtual PHY and honor reset intent; argument width disputed (†). | moderate |
| 255 | [PsCreateSystemThreadEx][ps] | `NTSTATUS NTAPI PsCreateSystemThreadEx(PHANDLE ThreadHandle, SIZE_T ThreadExtensionSize, SIZE_T KernelStackSize, SIZE_T TlsDataSize, PHANDLE ThreadId, PKSTART_ROUTINE StartRoutine, PVOID StartContext, BOOLEAN CreateSuspended, BOOLEAN DebuggerThread, PKSYSTEM_ROUTINE SystemRoutine);` | threads/sync | 1/0/1 | Create a schedulable guest thread with stack, TLS, startup callbacks and suspended state. | hard |
| 258 | [PsTerminateSystemThread][ps] | `VOID NTAPI DECLSPEC_NORETURN PsTerminateSystemThread(NTSTATUS ExitStatus);` | threads/sync | 1/0/1 | Terminate only the current guest thread, signal its object and release resources; never return. | moderate |
| 260 | [RtlAnsiStringToUnicodeString][rtl] | `NTSTATUS NTAPI RtlAnsiStringToUnicodeString(PUNICODE_STRING DestinationString, PSTRING SourceString, BOOLEAN AllocateDestinationString);` | debug/misc | 1/0/1 | Convert counted ANSI to guest UTF-16, checking capacity and optional pool allocation. | moderate |
| 269 | [RtlCompareMemoryUlong][rtl] | `SIZE_T NTAPI RtlCompareMemoryUlong(PVOID Source, SIZE_T Length, ULONG Pattern);` | memory | 7/0/7 | Count the initial bytes matching the repeated 32-bit pattern. | trivial |
| 277 | [RtlEnterCriticalSection][rtl] | `VOID NTAPI RtlEnterCriticalSection(PRTL_CRITICAL_SECTION CriticalSection);` | threads/sync | 17/0/17 | Enter a recursive guest critical section with ownership, contention and wakeup handling. | hard |
| 279 | [RtlEqualString][rtl] | `BOOLEAN NTAPI RtlEqualString(CONST PSTRING String1, CONST PSTRING String2, BOOLEAN CaseInSensitive);` | debug/misc | 1/0/1 | Compare counted byte strings with the Xbox case-insensitive option. | trivial |
| 289 | [RtlInitAnsiString][rtl] | `VOID NTAPI RtlInitAnsiString(PANSI_STRING DestinationString, PCSZ SourceString);` | debug/misc | 19/0/25 | Initialize the counted ANSI descriptor over the supplied guest NUL-terminated string. | trivial |
| 291 | [RtlInitializeCriticalSection][rtl] | `VOID NTAPI RtlInitializeCriticalSection(PRTL_CRITICAL_SECTION CriticalSection);` | threads/sync | 3/0/3 | Initialize the guest critical-section fields and dispatcher state. | moderate |
| 294 | [RtlLeaveCriticalSection][rtl] | `VOID NTAPI RtlLeaveCriticalSection(PRTL_CRITICAL_SECTION CriticalSection);` | threads/sync | 150/0/150 | Release recursive ownership and wake the correct critical-section waiter. | hard |
| 301 | [RtlNtStatusToDosError][rtl] | `ULONG NTAPI RtlNtStatusToDosError(NTSTATUS Status);` | debug/misc | 23/0/27 | Map NTSTATUS values to the corresponding DOS error numbers. | moderate |
| 302 | [RtlRaiseException][rtl] | `VOID NTAPI RtlRaiseException(PEXCEPTION_RECORD ExceptionRecord);` | debug/misc | 3/0/3 | Dispatch the guest exception through x86 SEH and implement continuation/unhandled behavior. | hard |
| 304 | [RtlTimeFieldsToTime][rtl] | `BOOLEAN NTAPI RtlTimeFieldsToTime(PTIME_FIELDS TimeFields, PLARGE_INTEGER Time);` | time | 2/0/3 | Validate calendar fields and convert to NT 100 ns time. | moderate |
| 305 | [RtlTimeToTimeFields][rtl] | `VOID NTAPI RtlTimeToTimeFields(PLARGE_INTEGER Time, PTIME_FIELDS TimeFields);` | time | 5/0/5 | Convert NT 100 ns time to calendar fields with Xbox epoch and rounding. | moderate |
| 308 | [RtlUnicodeStringToAnsiString][rtl] | `NTSTATUS NTAPI RtlUnicodeStringToAnsiString(PSTRING DestinationString, PUNICODE_STRING SourceString, BOOLEAN AllocateDestinationString);` | debug/misc | 1/0/1 | Convert counted guest UTF-16 to ANSI with capacity and optional allocation handling. | moderate |
| 312 | [RtlUnwind][rtl] | `VOID NTAPI RtlUnwind(PVOID TargetFrame, PVOID TargetIp, PEXCEPTION_RECORD ExceptionRecord, PVOID ReturnValue);` | debug/misc | 0/1/1 | Walk guest x86 SEH frames, run unwind handlers and transfer to the requested target. | hard |
| 327 | [XeLoadSection][xbox] | `NTSTATUS NTAPI XeLoadSection(PXBE_SECTION_HEADER Section);` | memory | 1/0/1 | Load/reference a supplied-XBE section, honoring shared pages and section reference counts. | hard |
| 328 | [XeUnloadSection][xbox] | `NTSTATUS NTAPI XeUnloadSection(PXBE_SECTION_HEADER Section);` | memory | 1/0/1 | Decrement XBE section references and release only pages no longer shared or needed. | hard |
| 335 | [XcSHAInit][xbox] | `VOID NTAPI XcSHAInit(PUCHAR pbSHAContext);` | crypto | 0/1/1 | Initialize the guest-compatible SHA-1 context layout. | moderate |
| 336 | [XcSHAUpdate][xbox] | `VOID NTAPI XcSHAUpdate(PUCHAR pbSHAContext, PUCHAR pbInput, ULONG dwInputLength);` | crypto | 0/1/1 | Update the guest SHA-1 context with exactly the requested bytes. | moderate |
| 337 | [XcSHAFinal][xbox] | `VOID NTAPI XcSHAFinal(PUCHAR pbSHAContext, PUCHAR pbDigest);` | crypto | 0/1/1 | Finalize SHA-1 and write the 20-byte digest with compatible context mutation. | moderate |
| 338 | [XcRC4Key][xbox] | `VOID NTAPI XcRC4Key(PUCHAR pbKeyStruct, ULONG dwKeyLength, PUCHAR pbKey);` | crypto | 0/1/1 | Initialize the guest RC4 key schedule and stream-state layout. | moderate |
| 339 | [XcRC4Crypt][xbox] | `VOID NTAPI XcRC4Crypt(PUCHAR pbKeyStruct, ULONG dwInputLength, PUCHAR pbInput);` | crypto | 0/1/1 | Apply RC4 in place and update the guest stream state. | moderate |
| 340 | [XcHMAC][xbox] | `VOID NTAPI XcHMAC(PUCHAR pbKey, ULONG dwKeyLength, PUCHAR pbInput, ULONG dwInputLength, PUCHAR pbInput2, ULONG dwInputLength2, PUCHAR pbDigest);` | crypto | 0/1/1 | Compute Xbox-compatible HMAC-SHA1 over both input spans, preserving key-length behavior. | moderate |
| 343 | [XcPKGetKeyLen][xbox] | `ULONG NTAPI XcPKGetKeyLen(PUCHAR pbPubKey);` | crypto | 0/1/1 | Parse the Xbox RSA public-key blob and return its key length. | moderate |
| 344 | [XcVerifyPKCS1Signature][xbox] | `BOOLEAN NTAPI XcVerifyPKCS1Signature(PUCHAR pbSig, PUCHAR pbPubKey, PUCHAR pbDigest);` | crypto | 0/1/1 | Verify the Xbox RSA/PKCS#1 SHA-1 signature format and its byte order. | hard |
| 345 | [XcModExp][xbox] | `ULONG NTAPI XcModExp(PULONG pA, PULONG pB, PULONG pC, PULONG pD, ULONG dwN);` | crypto | 0/1/1 | Compute modular exponentiation using the Xbox limb order, buffer sizes and return convention. | hard |
| 346 | [XcDESKeyParity][xbox] | `VOID NTAPI XcDESKeyParity(PUCHAR pbKey, ULONG dwKeyLength);` | crypto | 0/1/1 | Set each DES key byte to odd parity. | trivial |
| 347 | [XcKeyTable][xbox] | `VOID NTAPI XcKeyTable(ULONG dwCipher, PUCHAR pbKeyTable, PUCHAR pbKey);` | crypto | 0/1/1 | Build the guest DES/3DES key-table layout for the selected cipher. | moderate |
| 349 | [XcBlockCryptCBC][xbox] | `VOID NTAPI XcBlockCryptCBC(ULONG dwCipher, ULONG dwInputLength, PUCHAR pbOutput, PUCHAR pbInput, PUCHAR pbKeyTable, ULONG dwOp, PUCHAR pbFeedback);` | crypto | 0/1/1 | Encrypt/decrypt DES/3DES CBC, honoring operation, in-place buffers and feedback mutation. | moderate |
| 358 | [HalIsResetOrShutdownPending][hal] | `BOOLEAN NTAPI HalIsResetOrShutdownPending(void);` | hardware/HAL | 1/0/1 | Return whether the guest reset/shutdown state machine has a pending request. | trivial |
| 359 | [IoMarkIrpMustComplete][io] † | `VOID NTAPI IoMarkIrpMustComplete(PIRP Irp);` | file I/O | 1/0/1 | Mark IRP completion requirements consistently with pending I/O; return type disputed (†). | hard |
| 360 | [HalInitiateShutdown][hal] † | `VOID NTAPI HalInitiateShutdown(void);` | hardware/HAL | 1/0/1 | Request guest shutdown through the virtual SMC/runtime; return type disputed (†). | moderate |

## Imported data (17)

Each thunk must contain the **guest address of the exported storage**.
An exported pointer needs a separate 4-byte pointer cell: for example,
`LaunchDataPage` is a pointer variable, while `ExEventObjectType` and
`PsThreadObjectType` are actual `OBJECT_TYPE` objects, not pointer variables.
Nested pointers must also be guest addresses. Key material comes from user
configuration or a documented compatibility policy, never from this document.
The declarations and [XboxDev export list][wiki] identify these as data;
Ghidra label/function classifications alone are insufficient.

| Ordinal | Name | C declaration | Category | References | What a Rust HLE needs to do | Difficulty |
| ---: | --- | --- | --- | ---: | --- | --- |
| 16 | [ExEventObjectType][ex] | `OBJECT_TYPE ExEventObjectType;` | threads/sync | 8 | Provide a guest OBJECT_TYPE descriptor used for event type checks and lifetime operations. | moderate |
| 40 | [HalDiskCachePartitionCount][hal] | `ULONG HalDiskCachePartitionCount;` | hardware/HAL | 3 | Expose the configured cache-partition count, consistent with mounted devices. | trivial |
| 41 | [HalDiskModelNumber][hal] † | `STRING HalDiskModelNumber;` | hardware/HAL | 1 | Expose a disk-model counted string and guest buffer; indirection remains disputed (†). | moderate |
| 42 | [HalDiskSerialNumber][hal] † | `STRING HalDiskSerialNumber;` | hardware/HAL | 1 | Expose a disk-serial counted string and guest buffer; indirection remains disputed (†). | moderate |
| 156 | [KeTickCount][kernel] | `volatile DWORD KeTickCount;` | time | 13 | Maintain a readable, wrapping 32-bit millisecond boot counter in guest memory. | moderate |
| 164 | [LaunchDataPage][xbox] | `PLAUNCH_DATA_PAGE LaunchDataPage;` | memory | 14 | Expose a writable guest pointer cell to a persistent launch-data page, possibly null. | moderate |
| 259 | [PsThreadObjectType][ps] | `OBJECT_TYPE PsThreadObjectType;` | threads/sync | 1 | Provide a guest OBJECT_TYPE descriptor for thread type checks and lifetime operations. | moderate |
| 322 | [XboxHardwareInfo][xbox] | `XBOX_HARDWARE_INFO XboxHardwareInfo;` | hardware/HAL | 8 | Expose the 8-byte hardware flags/revision structure for the chosen virtual console. | trivial |
| 323 | [XboxHDKey][xbox] | `UCHAR XboxHDKey[16];` | crypto | 15 | Provide a 16-byte per-console HDD key from runtime configuration. | moderate |
| 324 | [XboxKrnlVersion][xbox] | `XBOX_KRNL_VERSION XboxKrnlVersion;` | hardware/HAL | 2 | Expose the 8-byte kernel-version structure for the selected compatibility profile. | trivial |
| 325 | [XboxSignatureKey][xbox] | `BYTE XboxSignatureKey[16];` | crypto | 1 | Provide a 16-byte active-title signature key consistent with save signing. | moderate |
| 326 | [XeImageFileName][xbox] | `OBJECT_STRING XeImageFileName;` | file I/O | 3 | Expose the counted guest image filename and its backing path buffer. | trivial |
| 353 | [XboxLANKey][xbox] | `XBOX_KEY_DATA XboxLANKey;` | crypto | 3 | Provide a 16-byte LAN key consistent across the chosen compatibility profile. | moderate |
| 354 | [XboxAlternateSignatureKeys][xbox] | `XBOX_KEY_DATA XboxAlternateSignatureKeys[16];` | crypto | 1 | Provide 16 alternate 16-byte title-signature keys with the correct active-title derivation. | moderate |
| 355 | [XePublicKeyData][xbox] | `UCHAR XePublicKeyData[284];` | crypto | 1 | Expose a 284-byte Xbox RSA public-key blob from configured compatible public material. | moderate |
| 356 | [HalBootSMCVideoMode][hal] | `DWORD HalBootSMCVideoMode;` | hardware/HAL | 1 | Expose the configured boot AV-pack/video-mode word, consistent with AV queries. | trivial |
| 357 | [IdexChannelObject][xbox] | `IDE_CHANNEL_OBJECT IdexChannelObject;` | hardware/HAL | 2 | Provide a guest IDE channel object with coherent device/interrupt/queue fields used by drivers. | hard |

`XboxHDKey`, `XboxSignatureKey` and `XboxLANKey` each occupy 16 bytes;
`XboxAlternateSignatureKeys` occupies 256 bytes; `XePublicKeyData` is 284
bytes. `XboxHardwareInfo` and `XboxKrnlVersion` each occupy 8 bytes, and
`KeTickCount` is a 4-byte counter (not Windows’ larger tick-count structure).
`XeImageFileName` is an 8-byte counted ANSI descriptor plus a separate buffer.
Use the published [guest structure definitions][types] for object and IDE
channel layouts rather than allocating host Rust equivalents.

## Hard work and why

- **Guest memory and image lifetime:** contiguous/physical allocation and
  DMA pinning (165, 166, 171, 173, 175, 176, 178), GPU instance memory (168),
  virtual reserve/commit/decommit (184, 199), and section loading (327, 328)
  share one page/address model. Physical bounds, aliases, fragmentation,
  shared section pages and quick-reboot persistence must agree. See
  [Cxbx memory HLE][hle-mm] and [section loading][hle-xe].
- **AV, PCI and interrupts:** encoder/display mode (2, 3), PCI space (46),
  IRQ objects/delivery (98, 100, 109, 153), IRQL (129, 160, 161), DPC queues
  (119, 137) and the IDE channel object (357) connect guest-visible hardware
  state to rendering, audio, networking and storage. macOS cannot execute
  these as host privileged operations. IRQL must control guest delivery,
  not merely return a constant. See [AV HLE][hle-av], [HAL HLE][hle-hal],
  [kernel HLE][hle-ke] and [xemu’s Xbox device models][xemu].
- **Threads, waits and timers:** thread creation/resume (255, 224), waits
  (99, 158, 159, 233–235), event dispatch (145), critical sections (277, 294)
  and timer/DPC delivery (149, 150) need a common scheduler. Preserve atomic
  WaitAll, event reset rules, recursive ownership, suspended startup, guest
  TLS, alert/APC completion and wake ordering. Timer due times use signed
  100 ns intervals: negative is relative, nonnegative is absolute NT time;
  periodic intervals are milliseconds. See [thread HLE][hle-ps],
  [kernel HLE][hle-ke] and [native-call HLE][hle-nt].
- **Files and guest drivers:** open/create (190), read/write (219, 236),
  controls (196, 200), IRP/device/packet handling (62, 65, 81, 83–87, 359)
  and named-object parsing (247) involve asynchronous completion and guest
  callbacks as well as host files. Device paths, sharing, pending status,
  status-block byte counts and completion order must agree. Inventory the
  actual IOCTL/FSCTL codes before choosing implementations. See
  [I/O HLE][hle-io], [native-call HLE][hle-nt] and [object HLE][hle-ob].
- **Guest exceptions and crypto formats:** exception dispatch/unwind (302,
  312) must walk guest x86 SEH, not the host Rust stack. RSA verification and
  modular exponentiation (344, 345) need Xbox key/blob/limb byte order and
  failure behavior. SHA/RC4/DES contexts remain guest-readable across calls.
  See [RTL HLE][hle-rtl] and [crypto HLE][hle-xc].

These bullets cover every row rated hard. Replacing a linked SDK subsystem
may bypass some of its kernel calls later; import presence alone does not
establish which subset the first-frame milestone will execute.

## Calls that do not return

Keep the existing noreturn set **49, 95, 96, 258** for Xbox guest execution.
The [nxdk declarations][nxdk] explicitly mark all four noreturn:

| Ordinal | Export | Imported here? | Runtime behavior |
| ---: | --- | --- | --- |
| 49 | HalReturnToFirmware | yes | Halt/reboot/relaunch transfers control out of the current guest execution. |
| 95 | KeBugCheck | yes | Fatal stop; no guest continuation. |
| 96 | KeBugCheckEx | **no** | Fatal stop with four diagnostic arguments; retain recognition for other binaries. |
| 258 | PsTerminateSystemThread | yes | Ends only the calling guest thread and signals its thread object. |

Cxbx offers a debugger “continue” path for [bugchecks][hle-ke]; that emulator
convenience does not change the Xbox noreturn contract. Windows’
[PsTerminateSystemThread documentation][nt-terminate] allows error returns,
but the Xbox header declares `VOID` and noreturn. For this runtime, invalid
use should produce a guest fault rather than resume a supposedly terminated
thread. `HalInitiateShutdown` (360) only requests shutdown and **can return**.
`RtlRaiseException` (302) can resume after a handler; `RtlUnwind` (312)
transfers control in some paths but is not an unconditional noreturn export.

## Unverified details and source disagreements

All 152 ordinal/name pairs and their function/data split were verified
against the XBE and public ordinal table. Retail behavior was not observed
on hardware. The following are unresolved implementation inputs, not grounds
for inventing a host ABI:

| Export(s) | Disagreement or missing evidence | Working entry above |
| --- | --- | --- |
| 41, 42 HalDiskModelNumber/HalDiskSerialNumber | [Cxbx HAL declarations][hal] and implementation use `PANSI_STRING` variables; [nxdk][nxdk] declares inline `STRING` objects. Pointer depth changes guest dereferencing. | nxdk inline descriptor; verify guest consumers before implementing. |
| 100 KeDisconnectInterrupt | [Cxbx][kernel] returns `void`; [nxdk][nxdk] returns `BOOLEAN`. | nxdk `BOOLEAN`; verify whether guest consumers test AL. |
| 238 NtYieldExecution | [Cxbx][nt] declares/implements `void`; [nxdk][nxdk] returns `NTSTATUS`. | nxdk `NTSTATUS`; status behavior needs confirmation. |
| 252 PhyGetLinkState; 253 PhyInitialize | [Cxbx][xbox] uses a 32-bit first argument; [nxdk][nxdk] uses `BOOLEAN`. Both still occupy a guest stack argument slot. | nxdk `BOOLEAN`; confirm low-byte versus full-word interpretation. |
| 359 IoMarkIrpMustComplete | [Cxbx][io] returns `CCHAR`; [nxdk][nxdk] returns `VOID`. | nxdk `VOID`; verify return-value use and completion bookkeeping. |
| 360 HalInitiateShutdown | [Cxbx HAL HLE][hle-hal] returns `NTSTATUS`; [nxdk][nxdk] returns `VOID`. | nxdk `VOID`; this is returning, not noreturn. |
| Hardware/driver controls and crypto contexts | Public HLE contains incomplete paths; required AV options, PCI fields, IOCTL/FSCTL codes, exact callback ordering and guest crypto context layouts were not exhaustively traced. | Work items describe required contracts; do not treat stubs as specifications. |
| Imported functions with no direct calls | Address loads, callbacks and unrecognized code can hide later calls. Ghidra labels `IoCreateDevice` and `IoInvalidDeviceRequest` as external Labels even though both are functions. | Keep both in the function work list, including the zero-reference import. |

The [XboxDev export list][wiki] corroborates ordinal/data classifications;
[Cxbx implementations][hle-root] explain operational contracts, and
[nxdk][nxdk] supplies an independent Xbox ABI reference. NT documentation is
useful for shared semantics (for example [bugchecks][nt-bugcheck]), but its
Windows signatures must not replace the Xbox declarations.

[ordinals]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/KernelThunk.cpp
[nxdk]: https://github.com/XboxDev/nxdk/blob/master/lib/xboxkrnl/xboxkrnl.h
[wiki]: https://xboxdevwiki.net/Kernel
[types]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/types.h
[hle-root]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/tree/master/src/core/kernel/exports
[xemu]: https://github.com/xemu-project/xemu/tree/master/hw/xbox
[nt-terminate]: https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-psterminatesystemthread
[nt-bugcheck]: https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-kebugcheckex
[dbg]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/dbg.h
[ex]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/ex.h
[hal]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/hal.h
[io]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/io.h
[kernel]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/kernel.h
[mm]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/mm.h
[nt]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/nt.h
[ob]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/ob.h
[ps]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/ps.h
[rtl]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/rtl.h
[xbox]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/common/xbox.h
[hle-av]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlAv.cpp
[hle-hal]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlHal.cpp
[hle-ke]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlKe.cpp
[hle-mm]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlMm.cpp
[hle-nt]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlNt.cpp
[hle-ps]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlPs.cpp
[hle-xc]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlXc.cpp
[hle-xe]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlXe.cpp
[hle-io]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlIo.cpp
[hle-ob]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlOb.cpp
[hle-rtl]: https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/master/src/core/kernel/exports/EmuKrnlRtl.cpp
