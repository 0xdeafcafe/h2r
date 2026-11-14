//! Import metadata transcribed from docs/kernel-imports.md. Stops inspect
//! the individual guest stack contract; they never execute host kernel APIs.

use crate::{CpuAbi, Event, ExecCtx, GuestFault, TransferAbi, exec::KernelCall};

pub const KERNEL_TOKEN_BASE: u32 = 0xfff0_0000;
pub const KERNEL_TOKEN_END: u32 = 0xfff1_0000;
/// Tokens are recognized by dispatch only; they have no RAM mapping.
pub const fn function_token(ordinal: u32) -> u32 {
    assert!(ordinal < (KERNEL_TOKEN_END - KERNEL_TOKEN_BASE) / 4);
    KERNEL_TOKEN_BASE + ordinal * 4
}

#[derive(Debug, Clone, Copy)]
pub struct FunctionImport {
    pub ordinal: u32,
    pub name: &'static str,
    pub stack_words: usize,
    pub variadic: bool,
}

const fn f(ordinal: u32, name: &'static str, stack_words: usize, variadic: bool) -> FunctionImport {
    FunctionImport {
        ordinal,
        name,
        stack_words,
        variadic,
    }
}
pub const FUNCTION_IMPORTS: &[FunctionImport] = &[
    f(1, "AvGetSavedDataAddress", 0, false),
    f(2, "AvSendTVEncoderOption", 4, false),
    f(3, "AvSetDisplayMode", 6, false),
    f(4, "AvSetSavedDataAddress", 1, false),
    f(8, "DbgPrint", 1, true),
    f(15, "ExAllocatePoolWithTag", 2, false),
    f(17, "ExFreePool", 1, false),
    f(23, "ExQueryPoolBlockSize", 1, false),
    f(24, "ExQueryNonVolatileSetting", 5, false),
    f(44, "HalGetInterruptVector", 2, false),
    f(46, "HalReadWritePCISpace", 6, false),
    f(47, "HalRegisterShutdownNotification", 2, false),
    f(49, "HalReturnToFirmware", 1, false),
    f(62, "IoBuildSynchronousFsdRequest", 7, false),
    f(65, "IoCreateDevice", 6, false),
    f(67, "IoCreateSymbolicLink", 2, false),
    f(69, "IoDeleteSymbolicLink", 1, false),
    f(74, "IoInvalidDeviceRequest", 2, false),
    f(81, "IoStartNextPacket", 1, false),
    f(83, "IoStartPacket", 3, false),
    f(84, "IoSynchronousDeviceIoControlRequest", 8, false),
    f(85, "IoSynchronousFsdRequest", 5, false),
    f(86, "IofCallDriver", 0, false),
    f(87, "IofCompleteRequest", 0, false),
    f(95, "KeBugCheck", 1, false),
    f(97, "KeCancelTimer", 1, false),
    f(98, "KeConnectInterrupt", 1, false),
    f(99, "KeDelayExecutionThread", 3, false),
    f(100, "KeDisconnectInterrupt", 1, false),
    f(107, "KeInitializeDpc", 3, false),
    f(109, "KeInitializeInterrupt", 7, false),
    f(113, "KeInitializeTimerEx", 2, false),
    f(119, "KeInsertQueueDpc", 3, false),
    f(126, "KeQueryPerformanceCounter", 0, false),
    f(127, "KeQueryPerformanceFrequency", 0, false),
    f(128, "KeQuerySystemTime", 1, false),
    f(129, "KeRaiseIrqlToDpcLevel", 0, false),
    f(137, "KeRemoveQueueDpc", 1, false),
    f(139, "KeRestoreFloatingPointState", 1, false),
    f(142, "KeSaveFloatingPointState", 1, false),
    f(143, "KeSetBasePriorityThread", 2, false),
    f(145, "KeSetEvent", 3, false),
    f(149, "KeSetTimer", 4, false),
    f(150, "KeSetTimerEx", 5, false),
    f(151, "KeStallExecutionProcessor", 1, false),
    f(153, "KeSynchronizeExecution", 3, false),
    f(158, "KeWaitForMultipleObjects", 8, false),
    f(159, "KeWaitForSingleObject", 5, false),
    f(160, "KfRaiseIrql", 0, false),
    f(161, "KfLowerIrql", 0, false),
    f(165, "MmAllocateContiguousMemory", 1, false),
    f(166, "MmAllocateContiguousMemoryEx", 5, false),
    f(167, "MmAllocateSystemMemory", 2, false),
    f(168, "MmClaimGpuInstanceMemory", 2, false),
    f(169, "MmCreateKernelStack", 2, false),
    f(170, "MmDeleteKernelStack", 2, false),
    f(171, "MmFreeContiguousMemory", 1, false),
    f(172, "MmFreeSystemMemory", 2, false),
    f(173, "MmGetPhysicalAddress", 1, false),
    f(175, "MmLockUnlockBufferPages", 3, false),
    f(176, "MmLockUnlockPhysicalPage", 2, false),
    f(178, "MmPersistContiguousMemory", 3, false),
    f(179, "MmQueryAddressProtect", 1, false),
    f(180, "MmQueryAllocationSize", 1, false),
    f(181, "MmQueryStatistics", 1, false),
    f(182, "MmSetAddressProtect", 3, false),
    f(184, "NtAllocateVirtualMemory", 5, false),
    f(187, "NtClose", 1, false),
    f(189, "NtCreateEvent", 4, false),
    f(190, "NtCreateFile", 9, false),
    f(192, "NtCreateMutant", 3, false),
    f(193, "NtCreateSemaphore", 4, false),
    f(195, "NtDeleteFile", 1, false),
    f(196, "NtDeviceIoControlFile", 10, false),
    f(197, "NtDuplicateObject", 3, false),
    f(198, "NtFlushBuffersFile", 2, false),
    f(199, "NtFreeVirtualMemory", 3, false),
    f(200, "NtFsControlFile", 10, false),
    f(202, "NtOpenFile", 6, false),
    f(203, "NtOpenSymbolicLinkObject", 2, false),
    f(207, "NtQueryDirectoryFile", 10, false),
    f(210, "NtQueryFullAttributesFile", 2, false),
    f(211, "NtQueryInformationFile", 5, false),
    f(215, "NtQuerySymbolicLinkObject", 3, false),
    f(217, "NtQueryVirtualMemory", 2, false),
    f(218, "NtQueryVolumeInformationFile", 5, false),
    f(219, "NtReadFile", 8, false),
    f(221, "NtReleaseMutant", 2, false),
    f(222, "NtReleaseSemaphore", 3, false),
    f(224, "NtResumeThread", 2, false),
    f(225, "NtSetEvent", 2, false),
    f(226, "NtSetInformationFile", 5, false),
    f(228, "NtSetSystemTime", 2, false),
    f(233, "NtWaitForSingleObject", 3, false),
    f(234, "NtWaitForSingleObjectEx", 4, false),
    f(235, "NtWaitForMultipleObjectsEx", 6, false),
    f(236, "NtWriteFile", 8, false),
    f(238, "NtYieldExecution", 0, false),
    f(246, "ObReferenceObjectByHandle", 3, false),
    f(247, "ObReferenceObjectByName", 5, false),
    f(250, "ObfDereferenceObject", 0, false),
    f(252, "PhyGetLinkState", 1, false),
    f(253, "PhyInitialize", 2, false),
    f(255, "PsCreateSystemThreadEx", 10, false),
    f(258, "PsTerminateSystemThread", 1, false),
    f(260, "RtlAnsiStringToUnicodeString", 3, false),
    f(269, "RtlCompareMemoryUlong", 3, false),
    f(277, "RtlEnterCriticalSection", 1, false),
    f(279, "RtlEqualString", 3, false),
    f(289, "RtlInitAnsiString", 2, false),
    f(291, "RtlInitializeCriticalSection", 1, false),
    f(294, "RtlLeaveCriticalSection", 1, false),
    f(301, "RtlNtStatusToDosError", 1, false),
    f(302, "RtlRaiseException", 1, false),
    f(304, "RtlTimeFieldsToTime", 2, false),
    f(305, "RtlTimeToTimeFields", 2, false),
    f(308, "RtlUnicodeStringToAnsiString", 3, false),
    f(312, "RtlUnwind", 4, false),
    f(327, "XeLoadSection", 1, false),
    f(328, "XeUnloadSection", 1, false),
    f(335, "XcSHAInit", 1, false),
    f(336, "XcSHAUpdate", 3, false),
    f(337, "XcSHAFinal", 2, false),
    f(338, "XcRC4Key", 3, false),
    f(339, "XcRC4Crypt", 3, false),
    f(340, "XcHMAC", 7, false),
    f(343, "XcPKGetKeyLen", 1, false),
    f(344, "XcVerifyPKCS1Signature", 3, false),
    f(345, "XcModExp", 5, false),
    f(346, "XcDESKeyParity", 2, false),
    f(347, "XcKeyTable", 3, false),
    f(349, "XcBlockCryptCBC", 7, false),
    f(358, "HalIsResetOrShutdownPending", 0, false),
    f(359, "IoMarkIrpMustComplete", 1, false),
    f(360, "HalInitiateShutdown", 0, false),
    // Not imported by Halo 2, but a recognized Xbox noreturn diagnostic.
    f(96, "KeBugCheckEx", 5, false),
];

pub fn function(ordinal: u32) -> Option<&'static FunctionImport> {
    FUNCTION_IMPORTS.iter().find(|f| f.ordinal == ordinal)
}

pub(crate) fn stop(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    import: FunctionImport,
) -> Result<TransferAbi, GuestFault> {
    // One guard captures a coherent stack snapshot. Partial diagnostics retain
    // individual access faults instead of inventing missing argument values.
    let call = {
        let memory = ctx.memory();
        let stack_args = (0..import.stack_words)
            .map(|i| memory.u32(cpu.esp.wrapping_add(4 + i as u32 * 4)))
            .collect();
        KernelCall {
            ordinal: import.ordinal,
            name: import.name,
            return_pc: memory.u32(cpu.esp),
            stack_args,
            variadic: import.variadic,
            cpu: *cpu,
        }
    };
    Ok(TransferAbi::stop(
        ctx.emit(Event::KernelCall(Box::new(call))),
    ))
}

/// Shapes are guest bytes, never native Rust structures. Unimplemented object
/// fields remain zero placeholders until HLE supplies their actual behavior.
/// Opaque OBJECT_TYPE/IDE objects reserve a page; this is capacity, not a
/// claim about their as-yet-unimplemented internal structure layout.
#[derive(Debug, Clone, Copy)]
pub struct DataImport {
    pub ordinal: u32,
    pub name: &'static str,
    pub size: u32,
}
const fn d(ordinal: u32, name: &'static str, size: u32) -> DataImport {
    DataImport {
        ordinal,
        name,
        size,
    }
}
pub const DATA_IMPORTS: &[DataImport] = &[
    d(16, "ExEventObjectType", 4096),
    d(40, "HalDiskCachePartitionCount", 4),
    d(41, "HalDiskModelNumber", 8),
    d(42, "HalDiskSerialNumber", 8),
    d(156, "KeTickCount", 4),
    d(164, "LaunchDataPage", 4),
    d(259, "PsThreadObjectType", 4096),
    d(322, "XboxHardwareInfo", 8),
    d(323, "XboxHDKey", 16),
    d(324, "XboxKrnlVersion", 8),
    d(325, "XboxSignatureKey", 16),
    d(326, "XeImageFileName", 8),
    d(353, "XboxLANKey", 16),
    d(354, "XboxAlternateSignatureKeys", 256),
    d(355, "XePublicKeyData", 284),
    d(356, "HalBootSMCVideoMode", 4),
    d(357, "IdexChannelObject", 4096),
];
pub fn data(ordinal: u32) -> Option<&'static DataImport> {
    DATA_IMPORTS.iter().find(|d| d.ordinal == ordinal)
}
