# Kernel stop inventory

With VFS attached, the following Halo imports always stop. Ordinal 96 is also
retained as a diagnostic for other images (Halo does not import it). A stop
keeps the guest frame and canonical CPU; no success or argument cleanup occurs.
The inventory is checked against Runtime by `tests/stops.rs`.

| Ordinal | Export |
| ---: | --- |
| 3 | AvSetDisplayMode |
| 44 | HalGetInterruptVector |
| 46 | HalReadWritePCISpace |
| 49 | HalReturnToFirmware |
| 62 | IoBuildSynchronousFsdRequest |
| 65 | IoCreateDevice |
| 74 | IoInvalidDeviceRequest |
| 81 | IoStartNextPacket |
| 83 | IoStartPacket |
| 84 | IoSynchronousDeviceIoControlRequest |
| 85 | IoSynchronousFsdRequest |
| 86 | IofCallDriver |
| 87 | IofCompleteRequest |
| 95 | KeBugCheck |
| 96 | KeBugCheckEx |
| 98 | KeConnectInterrupt |
| 100 | KeDisconnectInterrupt |
| 109 | KeInitializeInterrupt |
| 119 | KeInsertQueueDpc |
| 137 | KeRemoveQueueDpc |
| 139 | KeRestoreFloatingPointState |
| 142 | KeSaveFloatingPointState |
| 151 | KeStallExecutionProcessor |
| 153 | KeSynchronizeExecution |
| 167 | MmAllocateSystemMemory |
| 168 | MmClaimGpuInstanceMemory |
| 172 | MmFreeSystemMemory |
| 175 | MmLockUnlockBufferPages |
| 176 | MmLockUnlockPhysicalPage |
| 195 | NtDeleteFile |
| 196 | NtDeviceIoControlFile |
| 200 | NtFsControlFile |
| 228 | NtSetSystemTime |
| 247 | ObReferenceObjectByName |
| 302 | RtlRaiseException |
| 312 | RtlUnwind |
| 338 | XcRC4Key |
| 339 | XcRC4Crypt |
| 343 | XcPKGetKeyLen |
| 344 | XcVerifyPKCS1Signature |
| 345 | XcModExp |
| 346 | XcDESKeyParity |
| 347 | XcKeyTable |
| 349 | XcBlockCryptCBC |
| 358 | HalIsResetOrShutdownPending |
| 359 | IoMarkIrpMustComplete |
| 360 | HalInitiateShutdown |

Supplemental exports **56, 57, 58** (SLIST atomics) and **177** (MmMapIoSpace)
also have explicit stop adapters. They are outside Runtime's current Halo import
inventory; use `Kernel::entry` for their mapped adapters in synthetic code.

Without `Kernel::set_vfs`, **67, 69, 190, 198, 202, 203, 207, 210, 211, 215,
218, 219, 226, 236** stop as well. `Kernel::stopped_ordinals()` includes this configuration
difference. File handles use the existing 187/197 close/duplicate adapters.

Partly supported exports also stop on unsupported inputs; see README.md.
HLE initializes DATA 156 (KeTickCount), 164 (LaunchDataPage), 40
(HalDiskCachePartitionCount = 3), 323 (XboxHDKey = `crypto::XBOX_HD_KEY`,
all zero) and 326 (XeImageFileName). The other DATA
ordinals (**16, 41, 42, 259, 322, 324, 325, 353, 354, 355, 356, 357**)
are not initialized by HLE. Runtime still exposes
their original zero placeholders. Reads cannot currently be intercepted to
produce a named kernel stop: these values must not be treated as a console
profile, real object types, configured keys, or a working IDE device.


Partly supported exports 2/24/301 stop on unknown encoder options, setting
indices and status codes respectively; their supported subsets are in XAPI.md.
164 (LaunchDataPage DATA) is explicitly null by default, with an opt-in setter.
Timers and DPCs (97, 107, 113, 149, 150) are implemented on
logical time (README.md); KeInsertQueueDpc/KeRemoveQueueDpc (119/137) and
interrupt objects still stop. Inside a DPC routine, blocking waits, delays,
yields and thread-only services stop (no KTHREAD), and such a stop cannot be
retried with `retry_kernel_call`.

MEM_NOZERO variants of 184 stop (MEM_TOP_DOWN is implemented). 199 stops for
a decommit range past its allocation and release at an interior address. 182
stops outside contiguous or committed memory. 182, 171 and 199 stop, changing
nothing, when they would remove Execute from or unmap lifted code
(`MapError::LiftedCode`; the loader guards executable sections). 199 and 170
also stop, changing nothing, over a page inside an HLE-owned virtual claim
(`MapError::Claimed`); 199 likewise over a committed page no longer mapped. 166 stops for a reservation
without a 0x80000000 alias. Named mutants, file-object references (246), APC
routines on file I/O and FILE_USE_FILE_POINTER_POSITION on asynchronous handles
stop. PAGE_WRITECOMBINE and
PAGE_NOCACHE use coherent RAM while preserving protection query bits; those
modifiers alone no longer force a stop. Invalid/mismatched dispatcher headers
and KeReleaseSemaphore type/limit exceptions stop rather than returning counts.
