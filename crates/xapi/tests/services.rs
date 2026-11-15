mod common;

use common::*;
use xapi::{
    Error, LAUNCH_DATA_SIZE, ThreadState,
    todo_register::{Function, Outcome},
};

#[test]
fn launch_copies_owned_data_and_returns_a_stop_outcome() {
    let (mut xapi, mut memory, _) = setup();
    let mut thread = ThreadState::default();
    memory.put(0x400, b"D:\\default.xbe\0");
    memory.put(0x2000, &[3; LAUNCH_DATA_SIZE]);
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XLaunchNewImage,
            &[0x400, 0x2000]
        )
        .unwrap()
        .1,
        Outcome::Launch
    );
    memory.put(0x2000, &[9; LAUNCH_DATA_SIZE]);
    let request = xapi.take_launch_request().unwrap();
    assert_eq!(request.path.as_deref(), Some("D:\\default.xbe"));
    assert_eq!(*request.data.unwrap(), [3; LAUNCH_DATA_SIZE]);
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XLaunchNewImage,
            &[0, 0]
        )
        .unwrap()
        .1,
        Outcome::Launch
    );
    assert_eq!(xapi.take_launch_request().unwrap().path, None);
}

#[test]
fn invalid_launch_does_not_queue_a_transition() {
    let (mut xapi, mut memory, _) = setup();
    let mut thread = ThreadState { last_error: 12 };
    assert!(matches!(
        xapi.launch_new_image(&memory, 0, 0x7fff),
        Err(Error::Memory(_))
    ));
    assert!(xapi.take_launch_request().is_none());
    memory.put(0x400, b"\0");
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XLaunchNewImage,
            &[0x400, 0]
        )
        .unwrap()
        .0,
        87
    );
    assert!(xapi.take_launch_request().is_none());
    assert_eq!(thread.last_error, 12);
}

#[test]
fn utility_operations_are_explicit_and_use_the_storage_interface() {
    let (mut xapi, mut memory, _) = setup();
    let mut thread = ThreadState::default();
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XFormatUtilityDrive,
            &[]
        )
        .unwrap()
        .0,
        0
    );
    assert_eq!(thread.last_error, 3);
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XMountUtilityDrive,
            &[0]
        )
        .unwrap()
        .0,
        1
    );
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XFormatUtilityDrive,
            &[]
        )
        .unwrap()
        .0,
        1
    );
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XMountUtilityDrive,
            &[1]
        )
        .unwrap()
        .0,
        1
    );
    assert_eq!(
        xapi.storage.calls,
        ["mount 5 false", "format", "mount 5 true"]
    );
    xapi.storage.failure = Some(Error::Win32(5));
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XMountUtilityDrive,
            &[0]
        )
        .unwrap()
        .0,
        0
    );
    assert_eq!(thread.last_error, 5);
}

#[test]
fn absent_memory_unit_policy_returns_status_without_writing_output() {
    let (mut xapi, mut memory, _) = setup();
    let mut thread = ThreadState { last_error: 77 };
    assert_eq!(
        call(
            &mut xapi,
            &mut memory,
            &mut thread,
            Function::XReadMUMetaData,
            &[0, 0, 0x400, 0, 32]
        )
        .unwrap()
        .0,
        1167
    );
    assert_eq!(memory.word(0x400), 0xcccc_cccc);
    assert_eq!(thread.last_error, 77);
}
