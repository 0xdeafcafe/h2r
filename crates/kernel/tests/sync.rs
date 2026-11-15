mod common;
use common::*;
use kernel::status;
use runtime::Event;

#[test]
fn event_waits_preserve_notification_and_consume_synchronization_state() {
    let mut h = Harness::new();
    h.set(DATA + 0x100, 0);
    h.set(DATA + 0x104, 0); // zero LARGE_INTEGER timeout
    let timeout = DATA + 0x100;
    for kind in [0, 1] {
        h.success(108, &[DATA, kind, 0]);
        assert_eq!(h.success(159, &[DATA, 0, 0, 0, timeout]), status::TIMEOUT);
        assert_eq!(h.success(145, &[DATA, 0, 0]), 0);
        assert_eq!(h.success(159, &[DATA, 0, 0, 0, timeout]), 0);
        assert_eq!(h.get(DATA + 4), if kind == 0 { 1 } else { 0 });
        let previous = h.success(138, &[DATA]);
        assert_eq!(previous, if kind == 0 { 1 } else { 0 });
        h.stopped(145, &[DATA, 0, 1]);
        assert_eq!(h.get(DATA + 4), 0);
        h.stopped(159, &[DATA, 0, 0, 1, timeout]);
        // Immediate polling works without Machine; an actual block does not.
        h.stopped(159, &[DATA, 0, 0, 0, 0]);
    }
}

#[test]
fn semaphores_enforce_limits_and_wait_all_is_atomic() {
    let mut h = Harness::new();
    h.success(112, &[DATA, 1, 3]);
    h.success(108, &[DATA + 0x20, 1, 0]);
    h.set(DATA + 0x100, DATA);
    h.set(DATA + 0x104, DATA + 0x20);
    h.runtime.memory().set_u64(DATA + 0x110, 0).unwrap();
    let args = [2, DATA + 0x100, 0, 0, 0, 0, DATA + 0x110, 0];
    assert_eq!(h.success(158, &args), status::TIMEOUT);
    assert_eq!(h.get(DATA + 4), 1);
    h.success(145, &[DATA + 0x20, 0, 0]);
    assert_eq!(h.success(158, &args), 0);
    assert_eq!(h.get(DATA + 4), 0);
    assert_eq!(h.get(DATA + 0x24), 0);
    assert_eq!(h.success(132, &[DATA, 0, 2, 0]), 0);
    assert_eq!(h.get(DATA + 4), 2);
    h.stopped(132, &[DATA, 0, 2, 0]);
    assert_eq!(h.get(DATA + 4), 2);
    h.stopped(132, &[DATA, 0, 1, 1]);
    assert_eq!(h.get(DATA + 4), 2);
    assert_eq!(h.success(159, &[DATA, 0, 0, 0, DATA + 0x110]), 0);
    assert_eq!(h.get(DATA + 4), 1);
    h.set(DATA + 0x104, DATA);
    assert_eq!(h.success(158, &args), status::INVALID_PARAMETER_MIX);
    assert_eq!(h.get(DATA + 4), 1);
    assert_eq!(
        h.success(158, &[0, DATA, 0, 0, 0, 0, 0, 0]),
        status::INVALID_PARAMETER
    );
}

#[test]
fn handle_events_semaphores_duplicates_and_type_errors() {
    let mut h = Harness::new();
    h.runtime.memory().set_u64(DATA + 0x100, 0).unwrap();
    assert_eq!(h.success(189, &[DATA, 0, 1, 0]), 0);
    let event = h.get(DATA);
    assert_eq!(h.success(225, &[event, DATA + 0x20]), 0);
    assert_eq!(h.get(DATA + 0x20), 0);
    assert_eq!(h.success(233, &[event, 0, DATA + 0x100]), 0);
    assert_eq!(
        h.success(234, &[event, 0, 0, DATA + 0x100]),
        status::TIMEOUT
    );
    h.success(225, &[event, 0]);
    assert_eq!(h.success(197, &[event, DATA + 4, 2]), 0);
    let duplicate = h.get(DATA + 4);
    assert_ne!(event, duplicate);
    assert_eq!(h.success(187, &[event]), 0);
    assert_eq!(h.success(233, &[event, 0, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(233, &[duplicate, 0, DATA + 0x100]), 0);
    assert_eq!(h.success(193, &[DATA + 8, 0, 1, 3]), 0);
    let semaphore = h.get(DATA + 8);
    assert_eq!(
        h.success(225, &[semaphore, 0]),
        status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(
        h.success(222, &[duplicate, 1, 0]),
        status::OBJECT_TYPE_MISMATCH
    );
    assert_eq!(h.success(222, &[semaphore, 2, DATA + 0x20]), 0);
    assert_eq!(h.get(DATA + 0x20), 1);
    assert_eq!(
        h.success(222, &[semaphore, 1, 0]),
        status::SEMAPHORE_LIMIT_EXCEEDED
    );
    assert_eq!(
        h.success(222, &[semaphore, 0, 0]),
        status::INVALID_PARAMETER
    );
    h.set(DATA + 0x80, duplicate);
    h.set(DATA + 0x84, semaphore);
    assert_eq!(h.success(235, &[2, DATA + 0x80, 1, 0, 0, DATA + 0x100]), 1);
    assert_eq!(h.success(197, &[duplicate, DATA + 12, 1]), 0);
    assert_eq!(h.success(187, &[duplicate]), status::INVALID_HANDLE);
    assert_eq!(h.success(187, &[h.get(DATA + 12)]), 0);
    assert_eq!(h.success(187, &[semaphore]), 0);
    assert_eq!(h.success(224, &[semaphore, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(225, &[event, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(197, &[event, DATA, 0]), status::INVALID_HANDLE);
    assert_eq!(h.success(189, &[DATA, 0, 9, 0]), status::INVALID_PARAMETER);
    assert_eq!(h.success(193, &[DATA, 0, 4, 3]), status::INVALID_PARAMETER);
    h.set(DATA + 0x50, 0);
    h.set(DATA + 0x54, DATA + 0x60);
    h.set(DATA + 0x58, 0);
    h.stopped(189, &[DATA, DATA + 0x50, 0, 0]);
}

#[test]
fn critical_sections_mirror_guest_layout_and_enforce_recursive_ownership() {
    let mut h = Harness::new();
    let machine = kernel::Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    h.success(291, &[DATA]);
    assert_eq!(h.get(DATA + 16), u32::MAX);
    h.success(277, &[DATA]);
    h.success(277, &[DATA]);
    assert_eq!(h.get(DATA + 16), 1);
    assert_eq!(h.get(DATA + 20), 2);
    assert_eq!(h.get(DATA + 24), machine.thread(1).unwrap().object);
    h.ctx.thread_id = 2;
    assert_eq!(h.success(306, &[DATA]), 0);
    h.stopped(294, &[DATA]);
    h.stopped(291, &[DATA]);
    h.ctx.thread_id = 1;
    h.success(294, &[DATA]);
    assert_eq!(h.get(DATA + 20), 1);
    h.success(294, &[DATA]);
    assert_eq!(h.get(DATA + 16), u32::MAX);
    assert_eq!(h.get(DATA + 24), 0);
    assert_eq!(h.success(306, &[DATA]), 1);
    h.success(294, &[DATA]);
    let before = h.get(DATA + 16);
    let t = h.call(291, &[0xdead0000]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.get(DATA + 16), before);
}

#[test]
fn closing_last_object_handle_reuses_backing_without_reusing_handle_identity() {
    let mut h = Harness::new();
    let mut last = 0;
    h.success(189, &[DATA, 0, 0, 0]);
    let first = h.get(DATA);
    h.success(187, &[first]);
    let mapping_count = h.runtime.memory().mappings().len();
    for _ in 0..100 {
        assert_eq!(h.success(193, &[DATA, 0, 0, 1]), 0);
        let handle = h.get(DATA);
        assert!(handle > last);
        last = handle;
        assert_eq!(h.success(187, &[handle]), 0);
    }
    assert_eq!(h.runtime.memory().mappings().len(), mapping_count);
    assert_eq!(h.success(187, &[first]), status::INVALID_HANDLE);
}

#[test]
fn semaphore_header_at_the_end_of_the_address_space_faults_instead_of_overflowing() {
    let mut h = Harness::new();
    h.runtime
        .memory()
        .map(0xffff_f000, 4096, 0x100000, runtime::guest::Permissions::RW)
        .unwrap();
    // A readable 16-byte semaphore header whose limit word would be at 2^32.
    h.bytes(0xffff_fff0, &[5, 0, 5, 0]);
    let t = h.call(132, &[0xffff_fff0, 0, 1, 0]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    let t = h.call(159, &[0xffff_fff0, 0, 0, 0, 0]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}
