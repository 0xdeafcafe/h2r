mod common;

use common::*;
use xapi::{
    Error, ThreadState, memory,
    todo_register::{Frame, Function, Outcome, POWER_DOWN_OVERRIDES},
};

// Pinned retail disassembly ret sizes, independently confirmed by the Opus review.
// Bytes after RET exclude the return PC. The init thunk shares its target's ABI.
// Metadata only: no executable bytes or signature patterns are embedded.
const VERIFIED_RET_SIZES: &[(u32, Function, u32)] = &[
    (0x002d07f2, Function::XLaunchNewImage, 8),
    (0x002d0915, Function::XGetLaunchInfo, 8),
    (0x002d1d3e, Function::GetLastError, 0),
    (0x002d1d66, Function::SetLastError, 4),
    (0x002d5813, Function::XGetParentalControlSetting, 0),
    (0x002d5838, Function::DiskFreeSpaceEx, 16),
    (0x002d5b77, Function::XMountUtilityDrive, 4),
    (0x002d5c7c, Function::XFormatUtilityDrive, 0),
    (0x002d5f65, Function::DiskClusterSize, 4),
    (0x002d6004, Function::XAutoPowerDownResetTimer, 0),
    (0x002d6031, Function::XapiInitAutoPowerDown, 0),
    (0x00330310, Function::XGetVideoStandard, 0),
    (0x00330339, Function::XGetVideoFlags, 0),
    (0x0033039a, Function::XGetLanguage, 0),
    (0x00330df2, Function::XCreateSaveGame, 24),
    (0x00331055, Function::XDeleteSaveGame, 8),
    (0x00331125, Function::XFindFirstSaveGame, 8),
    (0x0033122f, Function::XFindNextSaveGame, 8),
    (0x00331276, Function::XFindClose, 4),
    (0x00331385, Function::XGetGameRegion, 0),
    (0x003321bd, Function::XGetAVPack, 0),
    (0x00408c57, Function::XInitDevices, 8),
    (0x00409884, Function::XPeekDevices, 12),
    (0x004098c0, Function::XInitDevices, 8),
    (0x004098c5, Function::XGetDeviceChanges, 12),
    (0x00409932, Function::XInputOpen, 16),
    (0x00409988, Function::XInputClose, 4),
    (0x00409994, Function::XInputGetCapabilities, 8),
    (0x00409b6c, Function::XInputGetState, 8),
    (0x00409bdf, Function::XInputSetState, 8),
    (0x0041136f, Function::XReadMUMetaData, 20),
];

#[test]
fn stack_abis_match_the_31_pinned_ret_sizes() {
    for &(address, function, cleanup_bytes) in VERIFIED_RET_SIZES {
        assert_eq!(
            function.stack_words() as u32 * 4,
            cleanup_bytes,
            "{function:?} at {address:#010x}"
        );
    }
}

#[test]
fn kernel_capable_functions_return_to_original_before_reading_stack() {
    let (mut xapi, mut memory, _) = setup();
    let before_memory = memory.bytes.clone();
    let mut thread = ThreadState { last_error: 123 };
    let generated = [
        Function::XGetLaunchInfo,
        Function::GetLastError,
        Function::SetLastError,
        Function::DiskFreeSpace,
        Function::DiskFreeSpaceEx,
        Function::DiskClusterSize,
        Function::XCreateSaveGame,
        Function::XFindFirstSaveGame,
        Function::XFindNextSaveGame,
        Function::XFindClose,
        Function::XDeleteSaveGame,
        Function::XGetLanguage,
        Function::XGetVideoFlags,
        Function::XGetVideoStandard,
        Function::XGetAVPack,
        Function::XGetGameRegion,
        Function::XGetParentalControlSetting,
    ];
    for function in generated {
        assert!(!function.is_replacement());
        let mut frame = Frame {
            eax: 0xdead_beef,
            esp: u32::MAX,
        };
        let before_frame = frame;
        assert_eq!(
            xapi.invoke(function, &mut frame, &mut memory, &mut thread),
            Ok(Outcome::Original)
        );
        assert_eq!(frame, before_frame);
        assert_eq!(thread.last_error, 123);
        assert_eq!(memory.bytes, before_memory);
        assert!(xapi.storage.calls.is_empty());
    }
}

#[test]
fn every_retained_adapter_uses_its_pinned_cleanup() {
    let (mut xapi, mut memory, _) = initialized();
    let mut thread = ThreadState::default();
    let handle = open(&mut xapi, &memory);
    for &(address, function, cleanup_bytes) in VERIFIED_RET_SIZES {
        if !function.is_replacement() || function == Function::XLaunchNewImage {
            continue;
        }
        let mut arguments = [0; 6];
        match function {
            Function::XGetDeviceChanges | Function::XPeekDevices => {
                arguments[..3].copy_from_slice(&[0x100, 0x400, 0x404]);
            }
            Function::XInputGetState
            | Function::XInputGetCapabilities
            | Function::XInputSetState => {
                // Invalid handle yields a Win32 status, but the pinned stack is still popped.
                arguments[1] = 0x400;
            }
            Function::XInputClose => {
                arguments[0] = handle;
            }
            _ => {}
        }
        memory.put(0x1000, &0x1234_5678u32.to_le_bytes());
        for (index, argument) in arguments
            .iter()
            .take(cleanup_bytes as usize / 4)
            .enumerate()
        {
            memory.put(0x1004 + index as u32 * 4, &argument.to_le_bytes());
        }
        let mut frame = Frame {
            eax: 9,
            esp: 0x1000,
        };
        assert_eq!(
            xapi.invoke(function, &mut frame, &mut memory, &mut thread),
            Ok(Outcome::Return { pc: 0x1234_5678 }),
            "{address:#010x}"
        );
        assert_eq!(frame.esp, 0x1004 + cleanup_bytes, "{address:#010x}");
    }
}

#[test]
fn void_powerdown_hooks_are_one_registration_group() {
    assert_eq!(
        POWER_DOWN_OVERRIDES,
        [
            Function::XapiInitAutoPowerDown,
            Function::XAutoPowerDownResetTimer
        ]
    );
    let (mut xapi, mut memory, _) = setup();
    let mut thread = ThreadState { last_error: 15 };
    for function in POWER_DOWN_OVERRIDES {
        let before_memory = memory.bytes[..0x1000].to_vec();
        assert_eq!(
            call(&mut xapi, &mut memory, &mut thread, function, &[])
                .unwrap()
                .0,
            0xdead_beef
        );
        assert_eq!(&memory.bytes[..0x1000], &before_memory);
    }
    assert!(xapi.storage.calls.is_empty());
    assert_eq!(thread.last_error, 15);
}

#[test]
fn invalid_guest_stack_precedes_service_side_effects() {
    let (mut xapi, mut memory, _) = setup();
    let mut thread = ThreadState::default();
    let mut frame = Frame {
        eax: 55,
        esp: 0x7ffc,
    };
    let before_frame = frame;
    assert!(matches!(
        xapi.invoke(
            Function::XMountUtilityDrive,
            &mut frame,
            &mut memory,
            &mut thread
        ),
        Err(Error::Memory(_))
    ));
    assert_eq!(frame, before_frame);
    assert!(xapi.storage.calls.is_empty());
    frame.esp = u32::MAX - 1;
    assert!(matches!(
        xapi.invoke(
            Function::XLaunchNewImage,
            &mut frame,
            &mut memory,
            &mut thread
        ),
        Err(Error::Memory(_))
    ));
}

#[test]
fn last_error_mirror_is_per_thread_for_native_failures() {
    let (mut xapi, mut memory, _) = setup();
    let mut first_thread = ThreadState { last_error: 10 };
    let second_thread = ThreadState { last_error: 20 };
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut first_thread,
            Function::XInputOpen,
            &[0x100, 4, 0, 0]
        )
        .unwrap()
        .0,
        0
    );
    assert_eq!(first_thread.last_error, 87);
    assert_eq!(second_thread.last_error, 20);
}

#[test]
fn unmatched_api_adapters_have_no_invented_address() {
    assert!(!VERIFIED_RET_SIZES.iter().any(|entry| matches!(
        entry.1,
        Function::XGetDevices | Function::XInputPoll | Function::DiskFreeSpace
    )));
}

#[test]
fn bounded_guest_strings_reject_unterminated_and_overflowing_inputs() {
    let mut memory = Memory::new();
    memory.put(0x100, b"abcd");
    assert_eq!(
        memory::ansi(&memory, 0x100, 4),
        Err(Error::INVALID_PARAMETER)
    );
    assert!(matches!(memory::offset(u32::MAX, 1), Err(Error::Memory(_))));
    assert_eq!(memory::ansi(&memory, 0, 260), Err(Error::INVALID_PARAMETER));
}
