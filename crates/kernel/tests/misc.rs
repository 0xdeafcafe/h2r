mod common;
use common::*;
use kernel::{clock::Clock, handles::HandleTable, status};
use runtime::Event;

#[test]
fn fastcall_atomics_wrap_and_compare_under_shared_alias_identity() {
    let mut h = Harness::new();
    let physical = h
        .runtime
        .memory()
        .mappings()
        .iter()
        .find(|m| m.virtual_start == DATA)
        .unwrap()
        .physical_start;
    h.runtime
        .memory()
        .map(0x20000000, 4096, physical, runtime::guest::Permissions::RW)
        .unwrap();
    h.set(DATA, u32::MAX);
    assert_eq!(h.success(53, &[0x20000000]), 0);
    assert_eq!(h.get(DATA), 0);
    assert_eq!(h.success(52, &[DATA]), u32::MAX);
    assert_eq!(h.success(54, &[DATA, 99]), u32::MAX);
    assert_eq!(h.get(DATA), 99);
    assert_eq!(h.success(55, &[DATA, 3]), 99);
    assert_eq!(h.get(DATA), 102);
    assert_eq!(h.success(51, &[DATA, 7, 102]), 102);
    assert_eq!(h.get(DATA), 7);
    assert_eq!(h.success(51, &[DATA, 8, 102]), 7);
    assert_eq!(h.get(DATA), 7);
    let before = h.get(DATA);
    let t = h.call(51, &[0xdead0000, 8, 7]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.get(DATA), before);
    assert_eq!(h.cpu.esp, STACK_TOP - 0x100);
}

#[test]
fn av_pointer_round_trip_is_guest_storage() {
    let mut h = Harness::new();
    assert_eq!(h.success(1, &[]), 0);
    assert_eq!(h.success(4, &[DATA]), 0xfeedface);
    assert_eq!(h.success(1, &[]), DATA);
}

#[test]
fn deterministic_clock_replays_wraps_ticks_and_returns_edx_eax() {
    let mut config = kernel::Config {
        epoch_100ns: 123456789,
        ..kernel::Config::default()
    };
    config.data_exports.insert(156, DATA);
    let mut h = Harness::config(config);
    let ticks = (u32::MAX as u64 + 2) * 10_000;
    h.kernel.advance_time(&h.runtime, ticks).unwrap();
    assert_eq!(h.get(DATA), 1);
    h.success(126, &[]);
    assert_eq!((h.cpu.edx as u64) << 32 | h.cpu.eax as u64, ticks);
    h.success(127, &[]);
    assert_eq!(
        (h.cpu.edx as u64) << 32 | h.cpu.eax as u64,
        Clock::FREQUENCY
    );
    h.success(128, &[DATA + 8]);
    assert_eq!(h.runtime.memory().u64(DATA + 8).unwrap(), ticks + 123456789);
    let snapshot = h.kernel.clock();
    assert_eq!(
        Clock::replay(123456789, snapshot.advances()).unwrap(),
        snapshot
    );
    assert!(Clock::replay(0, &[9, 8]).is_err());
    assert!(Clock::new(u64::MAX).advance(1).is_err());
    let c = Clock::new(500);
    assert_eq!(c.deadline(-10).unwrap(), 10);
    assert_eq!(c.deadline(510).unwrap(), 10);
    assert_eq!(c.deadline(499).unwrap(), 0);
}

#[test]
fn handles_retain_identity_across_duplication_and_reject_stale_values() {
    let mut handles = HandleTable::default();
    let a = handles.insert(0x12340000).unwrap();
    let b = handles.duplicate(a).unwrap();
    assert_ne!(a, b);
    assert_eq!(handles.get(b), Ok(0x12340000));
    assert_eq!(handles.close(a), Ok(0x12340000));
    assert_eq!(handles.get(a), Err(status::INVALID_HANDLE));
    assert!(handles.insert(42).unwrap() > b);
    assert!(handles.duplicate(a).is_err());
    assert_eq!(handles.len(), 2);
    assert!(!status::nt_success(status::INVALID_HANDLE));
    assert!(status::nt_success(status::TIMEOUT));
}

#[test]
fn debug_print_formats_guest_varargs_and_stops_before_partial_logging() {
    let mut h = Harness::new();
    h.bytes(DATA, b"[%s] %+06d %#x %08X %p %% %.3s %c\n\0");
    h.bytes(DATA + 0x100, b"halo\0");
    assert_eq!(
        h.success(
            8,
            &[
                DATA,
                DATA + 0x100,
                (-3i32) as u32,
                42,
                7,
                0x1234,
                DATA + 0x100,
                b'!' as u32
            ]
        ),
        0
    );
    assert_eq!(
        h.kernel.log(),
        vec!["[halo] -00003 0x2a 00000007 00001234 % hal !\n"]
    );
    h.bytes(DATA, b"prefix %f\0");
    h.stopped(8, &[DATA, 0]);
    assert_eq!(h.kernel.log().len(), 1);
    h.bytes(DATA, b"%s\0");
    let t = h.call(8, &[DATA, 0xdead0000]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.kernel.log().len(), 1);
    h.bytes(DATA, b"%*.*s %ld %#o\0");
    h.bytes(DATA + 0x100, b"abcde\0");
    h.success(8, &[DATA, (-5i32) as u32, 2, DATA + 0x100, 42, 8]);
    assert_eq!(h.kernel.log()[1], "ab    42 010");
}

#[test]
fn counted_string_exports_initialize_convert_compare_and_free() {
    let mut h = Harness::new();
    h.bytes(DATA + 0x100, b"Halo\0");
    h.success(289, &[DATA, DATA + 0x100]);
    assert_eq!(h.get(DATA), 0x00050004);
    assert_eq!(h.get(DATA + 4), DATA + 0x100);
    h.success(289, &[DATA + 8, 0]);
    assert_eq!(h.get(DATA + 8), 0);
    assert_eq!(h.get(DATA + 12), 0);
    h.bytes(DATA + 0x110, b"halo\0");
    h.success(289, &[DATA + 8, DATA + 0x110]);
    assert_eq!(h.success(279, &[DATA, DATA + 8, 0]), 0);
    assert_eq!(h.success(279, &[DATA, DATA + 8, 1]), 1);
    assert_eq!(h.success(260, &[DATA + 16, DATA, 1]), 0);
    let unicode = h.get(DATA + 20);
    assert_eq!(h.get(DATA + 16), 0x000a0008);
    assert_eq!(
        h.runtime.memory().read::<10>(unicode).unwrap(),
        [b'H', 0, b'a', 0, b'l', 0, b'o', 0, 0, 0]
    );
    h.success(290, &[DATA + 24, unicode]);
    assert_eq!(h.get(DATA + 24), 0x000a0008);
    assert_eq!(h.success(308, &[DATA + 32, DATA + 16, 1]), 0);
    assert_eq!(h.get(DATA + 32), 0x00050004);
    assert_eq!(h.success(279, &[DATA, DATA + 32, 0]), 1);
    h.success(286, &[DATA + 32]);
    h.success(287, &[DATA + 16]);
    assert_eq!(h.get(DATA + 32), 0);
    assert_eq!(h.get(DATA + 36), 0);
    h.stopped(286, &[DATA]); // Descriptor points at caller storage, not an allocated string.
    h.set(DATA + 40, 0x00010000);
    h.set(DATA + 44, DATA + 0x200);
    h.stopped(260, &[DATA + 40, DATA, 0]);
    h.bytes(DATA + 0x100, &[0xe9, 0]);
    h.success(289, &[DATA, DATA + 0x100]);
    h.stopped(260, &[DATA + 16, DATA, 1]);
    let t = h.call(289, &[DATA, 0xdead0000]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}

#[test]
fn irql_exports_preserve_low_byte_fastcall_and_validate_transitions() {
    let mut h = Harness::new();
    assert_eq!(h.success(103, &[]), 0);
    assert_eq!(h.success(160, &[0x1002]), 0);
    assert_eq!(h.success(103, &[]), 2);
    assert_eq!(h.success(129, &[]), 2);
    h.stopped(160, &[1]);
    assert_eq!(h.success(103, &[]), 2);
    h.stopped(161, &[3]);
    assert_eq!(h.success(103, &[]), 2);
    assert_eq!(h.success(161, &[0]), 0xfeedface);
    assert_eq!(h.success(129, &[]), 0);
    h.success(161, &[0]);
}

#[test]
fn dbgprint_log_and_stop_history_have_lifetime_budgets() {
    let mut h = Harness::new();
    h.bytes(DATA + 0x100, b"line %d\0");
    let total = kernel::limits::MAX_LOG_MESSAGES + 10;
    for i in 0..total as u32 {
        assert_eq!(h.success(8, &[DATA + 0x100, i]), 0);
    }
    let log = h.kernel.log_bytes();
    assert!(log.len() <= kernel::limits::MAX_LOG_MESSAGES);
    assert!(log.iter().map(Vec::len).sum::<usize>() <= kernel::limits::MAX_LOG_BYTES);
    assert_eq!(h.kernel.dropped_log_messages() as usize + log.len(), total);
    assert_eq!(
        log.last().unwrap(),
        format!("line {}", total - 1).as_bytes()
    );

    let stops = kernel::limits::MAX_STOP_REASONS + 5;
    for _ in 0..stops {
        h.stopped(171, &[DATA]);
    }
    let reasons = h.kernel.stop_reasons();
    assert!(reasons.len() <= kernel::limits::MAX_STOP_REASONS);
    assert_eq!(
        h.kernel.dropped_stop_reasons() as usize + reasons.len(),
        stops
    );
}

#[test]
fn handle_table_has_a_live_handle_budget() {
    let mut table = HandleTable::default();
    for _ in 0..kernel::limits::MAX_HANDLES {
        table.insert(0x1000).unwrap();
    }
    assert_eq!(table.insert(0x1000), Err(status::INSUFFICIENT_RESOURCES));
    assert_eq!(table.duplicate(4), Err(status::INSUFFICIENT_RESOURCES));
    table.close(4).unwrap();
    assert!(table.insert(0x1000).is_ok());
}
