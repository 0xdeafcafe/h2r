mod common;
use common::*;
use kernel::{Config, ConsoleConfig, Machine, console::*, dos, launch::LaunchData, ord, status};
use runtime::Event;
use std::sync::{Arc, Mutex};

#[test]
fn status_conversion_covers_every_kernel_and_vfs_status_and_stops_unknown() {
    let mut h = Harness::new();
    // Expected public mappings, including Xbox timeout rather than desktop's
    // ERROR_TIMEOUT, and every variant of the storage boundary's Error enum.
    let cases = [
        (status::SUCCESS, 0),
        (status::TIMEOUT, 258),
        (status::INVALID_HANDLE, 6),
        (status::INVALID_PARAMETER, 87),
        (status::NO_MEMORY, 8),
        (status::CONFLICTING_ADDRESSES, 487),
        (status::UNABLE_TO_FREE_VM, 87),
        (status::BUFFER_TOO_SMALL, 122),
        (status::OBJECT_TYPE_MISMATCH, 6),
        (status::SEMAPHORE_LIMIT_EXCEEDED, 298),
        (status::INSUFFICIENT_RESOURCES, 1450),
        (status::MEMORY_NOT_ALLOCATED, 487),
        (status::INVALID_PAGE_PROTECTION, 87),
        (status::INVALID_PARAMETER_MIX, 87),
        (status::NO_YIELD_PERFORMED, 721),
        (vfs::Error::InvalidHandle.ntstatus(), 6),
        (vfs::Error::InvalidParameter.ntstatus(), 87),
        (vfs::Error::NoSuchFile.ntstatus(), 2),
        (vfs::Error::EndOfFile.ntstatus(), 38),
        (vfs::Error::AccessDenied.ntstatus(), 5),
        (vfs::Error::ObjectNameInvalid.ntstatus(), 123),
        (vfs::Error::ObjectNameNotFound.ntstatus(), 2),
        (vfs::Error::ObjectNameCollision.ntstatus(), 183),
        (vfs::Error::ObjectPathNotFound.ntstatus(), 3),
        (vfs::Error::SharingViolation.ntstatus(), 32),
        (vfs::Error::DeletePending.ntstatus(), 5),
        (vfs::Error::DiskFull.ntstatus(), 112),
        (vfs::Error::MediaWriteProtected.ntstatus(), 19),
        (vfs::Error::FileIsADirectory.ntstatus(), 5),
        (vfs::Error::NotSupported.ntstatus(), 50),
        (vfs::Error::DirectoryNotEmpty.ntstatus(), 145),
        (vfs::Error::NotADirectory.ntstatus(), 267),
        (vfs::Error::ReparsePointNotResolved.ntstatus(), 1921),
        (vfs::Error::FileTooLarge.ntstatus(), 223),
        (vfs::Error::FileCorrupt.ntstatus(), 1392),
        (vfs::Error::IoDeviceError.ntstatus(), 1117),
        (vfs::Error::NoMoreFiles.ntstatus(), 18),
    ];
    for (nt, expected) in cases {
        assert_eq!(
            h.success(ord::RTL_NT_STATUS_TO_DOS_ERROR, &[nt]),
            expected,
            "{nt:#x}"
        );
        assert_eq!(h.cpu.edx, 0x12345678);
    }
    for n in 0..64 {
        let expected = match n {
            0 => 0,
            1 => 731,
            2 => 732,
            3 => 733,
            63 => 734,
            _ => 317,
        };
        assert_eq!(h.success(ord::RTL_NT_STATUS_TO_DOS_ERROR, &[n]), expected);
    }
    for nt in [0xdeadbeef, 0xc0000001, 0x20000001] {
        h.stopped(ord::RTL_NT_STATUS_TO_DOS_ERROR, &[nt]);
        assert_eq!(dos::from_ntstatus(nt), None);
    }
}

#[test]
fn configured_nonvolatile_dwords_and_encoder_readout_use_the_same_profile() {
    let profile = ConsoleConfig {
        language: 2,
        video_standard: AV_STANDARD_PAL_I,
        video_flags: AV_FLAGS_480P | 0x10000,
        game_region: 4,
        parental_games: 5,
        parental_password: 0x1234,
        parental_movies: 3,
        misc_flags: XC_MISC_FLAG_DISABLE_DST,
        ..ConsoleConfig::default()
    };
    let mut h = Harness::config(Config {
        console: profile.clone(),
        ..Config::default()
    });
    for (index, value) in [
        (XC_LANGUAGE, 2),
        (XC_VIDEO, AV_FLAGS_480P | 0x10000),
        (XC_FACTORY_AV_REGION, AV_STANDARD_PAL_I | AV_FLAGS_60HZ),
        (XC_FACTORY_GAME_REGION, 4),
        (XC_P_CONTROL_GAMES, 5),
        (XC_P_CONTROL_PASSWORD, 0x1234),
        (XC_P_CONTROL_MOVIES, 3),
        (XC_MISC, XC_MISC_FLAG_DISABLE_DST),
    ] {
        h.bytes(DATA + 16, &[0xcc; 8]);
        assert_eq!(
            h.success(24, &[index, DATA, DATA + 16, 8, DATA + 4]),
            status::SUCCESS
        );
        assert_eq!(h.get(DATA), 4);
        assert_eq!(h.get(DATA + 4), 4);
        assert_eq!(h.get(DATA + 16), value);
        assert_eq!(h.get(DATA + 20), 0);
    }
    assert_eq!(h.success(2, &[0xfd000000, 6, 0, DATA + 32]), 0xfeedface); // VOID
    assert_eq!(h.get(DATA + 32), profile.encoder_settings());
    assert_eq!(ConsoleConfig::default().language, 1);
    assert_ne!(ConsoleConfig::default().video_flags & AV_FLAGS_480P, 0);
    // XAPI's auto power-down setup (0x2d6f79) reads XC_MISC: default is off.
    assert_eq!(ConsoleConfig::default().setting(XC_MISC), Some(0));
    // XC_MAX_ALL (whole EEPROM) with no image configured: not found, and
    // nothing written. XQueryValue (0x2d6f62) maps it to a DOS error.
    h.bytes(DATA + 40, &[0xcc; 12]);
    assert_eq!(
        h.success(24, &[0xffff, DATA + 40, DATA + 44, 256, DATA + 48]),
        status::OBJECT_NAME_NOT_FOUND
    );
    assert_eq!(
        [h.get(DATA + 40), h.get(DATA + 44), h.get(DATA + 48)],
        [0xcccccccc; 3]
    );
    h.stopped(2, &[0, 6, 1, DATA]);
}

/// The options Halo sends: D3D's SetFlickerFilter/SetSoftDisplayFilter
/// (0x3f8c53, 0x3f8c9c, game code 0x14157), the unblank/blank pair around
/// a mode set (0x3f9cb1, 0x3fc73e) and the mode-set flicker reset (0x3f9c34).
#[test]
fn tv_encoder_options_halo_sends_keep_state_and_write_no_result() {
    use kernel::console::EncoderState;
    let mut h = Harness::new();
    const NV2A: u32 = 0xfd00_0000;
    assert_eq!(h.kernel.encoder_state(), EncoderState::default());
    h.set(DATA + 32, 0xcccc_cccc);
    // Flicker filter: every documented level 0..=5, last one wins.
    for level in 0..=5 {
        assert_eq!(h.success(2, &[NV2A, 11, level, 0]), 0xfeedface); // VOID
        assert_eq!(h.kernel.encoder_state().flicker_filter, Some(level));
    }
    // Luma (soften) filter is a BOOL, passed unnormalized.
    h.success(2, &[NV2A, 14, 7, 0]);
    assert_eq!(h.kernel.encoder_state().luma_filter, Some(true));
    h.success(2, &[NV2A, 14, 0, 0]);
    h.success(2, &[NV2A, 9, 1, 0]);
    assert_eq!(
        h.kernel.encoder_state(),
        EncoderState {
            blanked: Some(true),
            flicker_filter: Some(5),
            luma_filter: Some(false),
        }
    );
    h.success(2, &[NV2A, 9, 0, DATA + 32]);
    assert_eq!(h.kernel.encoder_state().blanked, Some(false));
    // A setting option leaves Result alone.
    assert_eq!(h.get(DATA + 32), 0xcccc_cccc);
    // Unseen levels, Params and options stop with the frame and state intact.
    let before = h.kernel.encoder_state();
    h.stopped(2, &[NV2A, 11, 6, 0]);
    h.stopped(2, &[NV2A, 9, 2, 0]);
    // AV_OPTION_GUESS_FIELD (0x3f9c4f) has no documented result.
    h.stopped(2, &[NV2A, 15, 0, DATA + 32]);
    h.stopped(2, &[NV2A, 1, 0, 0]); // Macrovision
    h.stopped(2, &[NV2A, 0x13, 1, 0]); // widescreen
    assert_eq!(h.kernel.encoder_state(), before);
    assert_eq!(h.get(DATA + 32), 0xcccc_cccc);
}

#[test]
fn nonvolatile_short_buffer_and_bad_pointers_preserve_unwritten_outputs() {
    let mut h = Harness::new();
    h.set(DATA, 0xaaaaaaaa);
    h.set(DATA + 4, 0xbbbbbbbb);
    assert_eq!(
        h.success(24, &[XC_LANGUAGE, DATA, 0, 3, DATA + 4]),
        status::BUFFER_TOO_SMALL
    );
    assert_eq!(h.get(DATA), 0xaaaaaaaa);
    assert_eq!(h.get(DATA + 4), 4);
    assert_eq!(h.success(24, &[XC_LANGUAGE, DATA, DATA + 16, 4, 0]), 0);
    h.set(DATA, 0xaaaaaaaa);
    h.set(DATA + 4, 0xbbbbbbbb);
    let t = h.call(24, &[XC_LANGUAGE, DATA, 0, 4, DATA + 4]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.get(DATA), 0xaaaaaaaa);
    assert_eq!(h.get(DATA + 4), 0xbbbbbbbb);
}

#[test]
fn launch_data_defaults_to_null_and_explicit_page_has_the_public_layout() {
    let h = Harness::new();
    let cell = h.kernel.data_export(ord::LAUNCH_DATA_PAGE).unwrap();
    assert_eq!(h.get(cell), 0);
    let launch = LaunchData {
        kind: 2,
        title_id: 0x4d530064,
        path: b"D:\\default.xbe".to_vec(),
        flags: 3,
        data: vec![0x12, 0x34],
    };
    h.kernel.set_launch_data(Some(&launch)).unwrap();
    let page = h.get(cell);
    assert_ne!(page, 0);
    assert_eq!(page & 4095, 0);
    assert_eq!(h.get(page), 2);
    assert_eq!(h.get(page + 4), launch.title_id);
    assert_eq!(h.get(page + 528), 3);
    let m = h.runtime.memory();
    assert_eq!(m.read::<16>(page + 8).unwrap(), *b"D:\\default.xbe\0\0");
    assert_eq!(m.read::<4>(page + 1024).unwrap(), [0x12, 0x34, 0, 0]);
    assert_eq!(m.u32(page + 1020).unwrap(), 0);
    drop(m);
    let bad = LaunchData {
        data: vec![0; 3073],
        ..launch.clone()
    };
    assert!(h.kernel.set_launch_data(Some(&bad)).is_err());
    assert_eq!(h.get(cell), page);
    h.kernel.set_launch_data(None).unwrap();
    assert_eq!(h.get(cell), 0);
    h.kernel.set_launch_data(Some(&launch)).unwrap();
    assert_eq!(h.get(cell), page);
    h.set(cell, DATA);
    assert!(h.kernel.set_launch_data(None).is_err());
}

#[test]
fn shared_vfs_is_exactly_the_attached_instance_and_cannot_be_replaced() {
    let h = Harness::new();
    assert!(h.kernel.vfs().is_none());
    let shared = Arc::new(Mutex::new(vfs::Vfs::new()));
    h.kernel.set_shared_vfs(shared.clone()).unwrap();
    assert!(Arc::ptr_eq(&shared, &h.kernel.vfs().unwrap()));
    assert!(h.kernel.set_vfs(vfs::Vfs::new()).is_err());
}

#[test]
fn owned_dispatcher_references_keep_closed_events_alive_until_completion() {
    let mut h = Harness::new();
    h.success(189, &[DATA, 0, 0, 0]);
    let handle = h.get(DATA);
    let first = h.kernel.reference_dispatcher(handle).unwrap();
    let second = h.kernel.reference_dispatcher(handle).unwrap();
    let object = first.address();
    assert_eq!(second.address(), object);
    assert_eq!(h.kernel.signal_event(handle).unwrap(), 0);
    assert_eq!(h.kernel.signal_event(handle).unwrap(), 1);
    h.success(187, &[handle]);
    assert!(h.kernel.signal_event(handle).is_err());
    first.release();
    assert_eq!(second.signal_event().unwrap(), 1);
    // A live pin prevents pool object reuse after the final handle closes.
    let other = h.success(14, &[20]);
    assert_ne!(other, object);
    second.release();
    let reused = h.success(14, &[20]);
    assert_eq!(reused, object);
    assert!(h.kernel.reference_dispatcher(handle).is_err());
    h.success(193, &[DATA, 0, 0, 2]);
    assert!(h.kernel.signal_event(h.get(DATA)).is_err());
}

#[test]
fn completion_event_signaling_wakes_a_parked_guest_thread() {
    let mut h = Harness::new();
    let mut machine = Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    h.success(189, &[DATA, 0, 1, 0]);
    let handle = h.get(DATA);
    h.code(0, move |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(233),
            &[handle, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    assert!(matches!(
        machine.run(5).unwrap(),
        kernel::RunOutcome::Deadlock { .. }
    ));
    let state = machine.thread(1).unwrap().object + KTHREAD_STATE;
    assert_eq!(h.runtime.memory().u8(state).unwrap(), WAITING);
    h.kernel.signal_event(handle).unwrap();
    // The signal itself readies the waiter; Machine::run's own wake pass must
    // not be what makes this observable.
    assert_eq!(h.runtime.memory().u8(state).unwrap(), READY);
    assert!(matches!(
        machine.run(5).unwrap(),
        kernel::RunOutcome::Complete { .. }
    ));
}

const KTHREAD_STATE: u32 = 0x2c;
const READY: u8 = 1;
const WAITING: u8 = 5;

/// One guest thread parked in NtWaitForSingleObject on a fresh event.
fn parked_waiter(h: &mut Harness) -> (Machine, u32, u32) {
    let machine = Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    h.success(189, &[DATA, 0, 1, 0]);
    let handle = h.get(DATA);
    h.code(0, move |cpu, ctx, _| {
        guest_call(
            cpu,
            ctx,
            runtime::kernel::function_token(233),
            &[handle, 0, 0],
            CODE + 0x100,
        )
    });
    h.code(0x100, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    let mut machine = machine;
    assert!(matches!(
        machine.run(5).unwrap(),
        kernel::RunOutcome::Deadlock { .. }
    ));
    let state = machine.thread(1).unwrap().object + KTHREAD_STATE;
    assert_eq!(h.runtime.memory().u8(state).unwrap(), WAITING);
    (machine, handle, state)
}

/// Run `f` on another thread and fail (rather than hang) if it blocks.
fn bounded<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    let value = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("completion deadlocked");
    worker.join().unwrap();
    value
}

#[test]
fn deferred_completion_under_the_guest_guard_returns_and_drains_after_release() {
    let mut h = Harness::new();
    let (mut machine, handle, state) = parked_waiter(&mut h);
    let pin = h.kernel.reference_dispatcher(handle).unwrap();
    let object = pin.address();
    // The guest closes its handle while the completion is outstanding.
    assert_eq!(h.success(187, &[handle]), 0);
    let runtime = h.runtime.clone();
    let signalled_under_guard = bounded(move || {
        let memory = runtime.memory();
        pin.defer_signal();
        // Nothing has been applied while the guard is still held.
        memory.u32(object + 4).unwrap()
    });
    assert_eq!(signalled_under_guard, 0);
    assert_eq!(h.kernel.pending_deferred_signals(), 1);
    assert_eq!(h.runtime.memory().u8(state).unwrap(), WAITING);
    let kernel = h.kernel.clone();
    let drained = bounded(move || kernel.drain_deferred_signals().unwrap());
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].address, object);
    assert_eq!(drained[0].result.as_ref().unwrap(), &0);
    assert_eq!(h.kernel.pending_deferred_signals(), 0);
    assert_eq!(h.runtime.memory().u8(state).unwrap(), READY);
    assert!(matches!(
        machine.run(5).unwrap(),
        kernel::RunOutcome::Complete { .. }
    ));
    // The queue released the pin: with no handle left the pool block is reused.
    assert_eq!(h.success(14, &[20]), object);
    assert!(h.kernel.take_deferred_signal_failures().is_empty());
}

#[test]
fn machine_run_applies_deferred_completions_at_the_next_slice() {
    let mut h = Harness::new();
    let (mut machine, handle, _) = parked_waiter(&mut h);
    let pin = h.kernel.reference_dispatcher(handle).unwrap();
    {
        let _guest = h.runtime.memory();
        pin.defer_signal();
    }
    assert!(matches!(
        machine.run(5).unwrap(),
        kernel::RunOutcome::Complete { .. }
    ));
    assert_eq!(h.kernel.pending_deferred_signals(), 0);
    // A completion whose guest header changed is reported, not dropped silently.
    h.success(189, &[DATA, 0, 0, 0]);
    let pin = h.kernel.reference_dispatcher(h.get(DATA)).unwrap();
    let object = pin.address();
    pin.defer_signal();
    h.bytes(object, &[7, 0, 4, 0]);
    assert!(matches!(
        machine.run(1).unwrap(),
        kernel::RunOutcome::Complete { .. }
    ));
    let failures = h.kernel.take_deferred_signal_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].address, object);
    assert!(failures[0].result.is_err());
}

/// The XAPI crate's `Events::signal(guest_handle)` contract: called only after
/// XAPI dropped its Guest guard, with the event pinned at submission.
struct KernelEvents {
    pinned: std::collections::HashMap<u32, kernel::DispatcherReference>,
}
impl KernelEvents {
    fn signal(&mut self, guest_handle: u32) -> Result<(), kernel::Error> {
        let pin = self
            .pinned
            .remove(&guest_handle)
            .ok_or(kernel::Error::Resource(status::INVALID_HANDLE))?;
        pin.signal_event()?;
        pin.release();
        Ok(())
    }
}

#[test]
fn xapi_signal_after_dropping_the_guest_guard_wakes_immediately() {
    let mut h = Harness::new();
    let (mut machine, handle, state) = parked_waiter(&mut h);
    let mut events = KernelEvents {
        pinned: [(handle, h.kernel.reference_dispatcher(handle).unwrap())].into(),
    };
    let runtime = h.runtime.clone();
    let delivered = bounded(move || {
        {
            // XAPI writes the completion status under its guard, then drops it.
            let mut guest = runtime.memory();
            guest.set_u32(DATA + 0x40, 0).unwrap();
        }
        let result = events.signal(handle);
        (result.is_ok(), events.pinned.is_empty())
    });
    assert_eq!(delivered, (true, true));
    assert_eq!(h.runtime.memory().u8(state).unwrap(), READY);
    assert!(matches!(
        machine.run(5).unwrap(),
        kernel::RunOutcome::Complete { .. }
    ));
}

#[test]
fn tls_last_error_is_per_thread_and_uses_the_signed_guest_index() {
    let tls = xbe::TlsDirectory {
        raw_start: DATA + 0x100,
        raw_end: DATA + 0x104,
        index_addr: DATA + 0x108,
        callbacks_addr: 0,
        zero_fill: 0x8c,
        characteristics: 0,
    };
    let mut h = Harness::config(Config {
        tls: Some(tls),
        ..Config::default()
    });
    let machine = Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    h.success(255, &[DATA, 0, 0x4000, 0x94, 0, CODE + 0x200, 0, 1, 0, 0]);
    let first = h.kernel.last_error_cell(1, -37).unwrap();
    let second = h.kernel.last_error_cell(2, -37).unwrap();
    assert_eq!(first, machine.thread(1).unwrap().tls + 12);
    assert_ne!(first, second);
    h.set(first, 123);
    h.set(second, 456);
    assert_eq!(h.get(first), 123);
    assert_eq!(h.get(second), 456);
    assert!(h.kernel.last_error_cell(1, 37).is_err());
    assert!(h.kernel.last_error_cell(1, i32::MIN).is_err());
    assert!(h.kernel.last_error_cell(999, -37).is_err());
    h.set(machine.thread(1).unwrap().tls, DATA);
    assert!(h.kernel.last_error_cell(1, -37).is_err());
}

#[test]
fn completion_references_revalidate_headers_and_bad_guest_pointers_fault() {
    let mut h = Harness::new();
    h.success(189, &[DATA, 0, 0, 0]);
    let handle = h.get(DATA);
    let pin = h.kernel.reference_dispatcher(handle).unwrap();
    h.bytes(pin.address(), &[0, 0, 3, 0]);
    assert!(pin.signal_event().is_err());
    assert!(h.kernel.signal_event(handle).is_err());
    assert!(h.kernel.reference_dispatcher(handle).is_err());
    assert_eq!(h.get(pin.address() + 4), 0);
    let t = h.call(2, &[0, 6, 0, 0]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    let mut profile = h.kernel.console_config();
    profile.language = 6;
    h.kernel.set_console_config(profile);
    assert_eq!(h.success(24, &[XC_LANGUAGE, DATA, DATA + 16, 4, 0]), 0);
    assert_eq!(h.get(DATA + 16), 6);
}

#[test]
fn boot_data_exports_describe_the_cache_partitions_and_launched_image() {
    let h = Harness::config(Config {
        data_exports: [(40, DATA + 0x900), (326, DATA + 0x910)].into(),
        ..Config::default()
    });
    assert_eq!(h.get(DATA + 0x900), 3);
    let m = h.runtime.memory();
    let length = m.u16(DATA + 0x910).unwrap() as usize;
    assert_eq!(m.u16(DATA + 0x912).unwrap() as usize, length + 1);
    let mut name = vec![0; length + 1];
    m.read_bytes(m.u32(DATA + 0x914).unwrap(), &mut name, 1)
        .unwrap();
    assert_eq!(&name[..length], b"\\Device\\CdRom0\\default.xbe");
    assert_eq!(name[length], 0);
}

#[test]
fn shutdown_notifications_are_recorded_in_priority_order() {
    let mut h = Harness::new();
    for (i, priority) in [(0u32, 1i32), (1, 5), (2, 1)] {
        h.set(DATA + i * 16, CODE + i);
        h.set(DATA + i * 16 + 4, priority as u32);
        assert_eq!(h.success(47, &[DATA + i * 16, 1]), 0xfeedface); // VOID
    }
    let order: Vec<_> = h
        .kernel
        .shutdown_notifications()
        .iter()
        .map(|n| (n.registration, n.routine, n.priority))
        .collect();
    assert_eq!(
        order,
        [
            (DATA + 16, CODE + 1, 5),
            (DATA, CODE, 1),
            (DATA + 32, CODE + 2, 1)
        ]
    );
    h.success(47, &[DATA, 0]);
    assert_eq!(h.kernel.shutdown_notifications().len(), 2);
    let t = h.call(47, &[0xdead_0000, 1]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}

#[test]
fn persistent_contiguous_pages_are_recorded_and_other_ranges_stop() {
    let mut h = Harness::new();
    let va = h.success(165, &[0x3000]);
    assert_ne!(va, 0);
    h.success(178, &[va + 0x100, 0x1000, 1]);
    assert_eq!(h.kernel.persistent_pages(), [va, va + 0x1000]);
    h.success(178, &[va, 0x1000, 0]);
    assert_eq!(h.kernel.persistent_pages(), [va + 0x1000]);
    h.success(178, &[va, 0, 1]);
    h.stopped(178, &[va + 0x2000, 0x2000, 1]);
    h.stopped(178, &[DATA, 0x1000, 1]);
}
