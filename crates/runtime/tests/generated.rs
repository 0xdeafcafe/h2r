//! RUNTIME-CONTRACT.md end to end through the real runner: a C module in the
//! emitter's shape (tests/generated_module.c) registered from its tables,
//! then call, return, resume, fault, stop and unsupported paths.
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, ExecCtxOpaque, GuestFrame, H2GenEntry, H2GenModule,
    H2GenResume, RegisteredModule, Runner, Runtime, TransferAbi, abi,
    dispatch::{ModuleTable, RegistrationError},
    exec::{GuestException, StopReason},
    ffi::GeneratedAbiVersion,
    guest::{Access, Guest, Permissions},
    runner::HARNESS_EXIT,
};
use runtime_test_probe::{h2fn_00401000, h2gen_fixture_module, h2gen_fixture_module_abi_version};
use std::sync::Arc;

const MAIN: u32 = 0x40_1000;
const MAIN_RESUME: u32 = 0x40_1005;
const MAIN_LABEL: u32 = 0x40_1010;
const MAIN_REP: u32 = 0x40_1020;
const LEAF: u32 = 0x40_2000;
const FA: u32 = 0x40_3000;
const FB: u32 = 0x40_3005;
const FC: u32 = 0x40_4000;
const STACK_TOP: u32 = 0x11_0000;
const DATA: u32 = 0x50_0000;

fn fixture() -> &'static H2GenModule {
    // SAFETY: an immutable C static that lives for the whole process.
    unsafe { &h2gen_fixture_module }
}
fn register(d: &mut Dispatch) -> Result<RegisteredModule, RegistrationError> {
    // SAFETY: the fixture's tables, handlers and probes follow ABI v1.
    unsafe { d.register_generated_module(fixture(), h2gen_fixture_module_abi_version) }
}

/// Code is executable, the stack and one data page are writable, and the
/// stack's top slot returns to the harness.
fn setup(edit: impl FnOnce(&mut Dispatch)) -> (Arc<Runtime>, CpuAbi, ExecCtx) {
    let mut guest = Guest::new();
    guest
        .map_zeroed(0x40_0000, 0x1_0000, Permissions::RX)
        .unwrap();
    guest
        .map_zeroed(0x10_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    guest.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
    guest.set_u32(STACK_TOP - 4, HARNESS_EXIT).unwrap();
    let mut dispatch = Dispatch::default();
    assert_eq!(
        register(&mut dispatch),
        Ok(RegisteredModule {
            entries: 7,
            resumes: 2
        })
    );
    edit(&mut dispatch);
    let runtime = Runtime::new(guest, dispatch);
    let cpu = CpuAbi {
        esp: STACK_TOP - 4,
        ..CpuAbi::default()
    };
    let ctx = ExecCtx::new(runtime.clone(), 1);
    (runtime, cpu, ctx)
}

fn run(cpu: &mut CpuAbi, ctx: &mut ExecCtx, entry: u32) -> (TransferAbi, Event) {
    let t = Runner::default().run(cpu, ctx, entry);
    let event = ctx.event(t.event_id).cloned().expect("a retained event");
    (t, event)
}

#[test]
fn registration_adds_every_entry_row_then_every_resume_row() {
    let mut d = Dispatch::default();
    register(&mut d).unwrap();
    for addr in [MAIN, MAIN_LABEL, MAIN_REP, LEAF, FA, FB, FC] {
        assert!(d.has_generated(addr), "{addr:#x}");
    }
    assert!(d.has_resume(MAIN_RESUME) && d.has_resume(FB));
    assert!(!d.has_resume(MAIN));
    // A second load of the same module is a duplicate and adds nothing.
    let again = register(&mut d).unwrap_err();
    assert_eq!(
        again,
        RegistrationError::ModuleRow {
            table: ModuleTable::Entries,
            index: 0,
            error: Box::new(RegistrationError::DuplicateGenerated(MAIN)),
        }
    );
}

#[test]
fn call_then_return_lands_on_the_registered_resume() {
    let (_, mut cpu, mut ctx) = setup(|_| {});
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!((t.kind, event), (abi::STOP, Event::HarnessExit));
    assert_eq!(cpu.eax, 42, "LEAF set 41, MAIN's continuation added 1");
    assert_eq!(cpu.esp, STACK_TOP);
    assert_eq!(cpu.eip, HARNESS_EXIT);
}

#[test]
fn a_return_into_an_overridden_entry_still_resumes_the_caller() {
    let (_, mut cpu, mut ctx) = setup(|d| {
        let _ = d.register_override(FB, "override of B", |cpu, _, _| {
            cpu.eax = 999;
            Ok(TransferAbi::stop(0))
        });
    });
    // A calls C; C returns to FB, which is both B's entry and A's resume.
    let (t, event) = run(&mut cpu, &mut ctx, FA);
    assert_eq!((t.kind, event, cpu.eax), (abi::STOP, Event::HarnessExit, 1));
    // Entry dispatch to the same address runs the override.
    cpu.esp = STACK_TOP - 4;
    let t = Runner::default().run(&mut cpu, &mut ctx, FB);
    assert_eq!((t.kind, t.event_id, cpu.eax), (abi::STOP, 0, 999));
}

#[test]
fn an_interior_label_is_entered_with_its_own_selector() {
    let (_, mut cpu, mut ctx) = setup(|_| {});
    let (_, event) = run(&mut cpu, &mut ctx, MAIN_LABEL);
    assert_eq!((event, cpu.ecx), (Event::HarnessExit, 0x10));
}

#[test]
fn an_unaccepted_start_selector_is_a_bad_entry_stop() {
    const STRAY: u32 = 0x40_1030;
    let (_, mut cpu, mut ctx) = setup(|d| d.register_resume(STRAY, MAIN, STRAY).unwrap());
    let t = Runner::default().run_from(&mut cpu, &mut ctx, TransferAbi::resume(STRAY));
    assert_eq!(t.kind, abi::STOP);
    let Some(Event::GeneratedStop { pc, reason, .. }) = ctx.event(t.event_id) else {
        panic!("{:?}", ctx.event(t.event_id));
    };
    assert_eq!(
        (*pc, *reason, cpu.eip),
        (STRAY, StopReason::BadEntry { selector: STRAY }, STRAY)
    );
}

#[test]
fn a_nested_guest_exception_faults_with_pc_and_backtrace() {
    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.edx = 1;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!(t.kind, abi::FAULT);
    let frames = vec![
        GuestFrame {
            function: LEAF,
            pc: LEAF + 3,
        },
        GuestFrame {
            function: MAIN,
            pc: MAIN_RESUME,
        },
    ];
    assert_eq!(
        event,
        Event::GuestException {
            pc: LEAF + 3,
            saved_eip: LEAF + 3,
            exception: GuestException::DivideError,
            backtrace: frames.clone(),
        }
    );
    assert_eq!(cpu.eip, LEAF + 3);
    assert_eq!(ctx.backtrace(cpu.eip), frames, "the chain at the stop");
    let report = ctx.report(t.event_id).unwrap().to_string();
    for needle in [
        "#DE divide error",
        "pc 0x00402003",
        "instruction div/idiv",
        "#0  at 0x00402003 in 0x00402000",
        "#1  returns to 0x00401005 in 0x00401000",
    ] {
        assert!(report.contains(needle), "{needle:?} in\n{report}");
    }
}

#[test]
fn traps_report_the_next_instruction_and_halt_is_a_stop() {
    let pc = MAIN_RESUME + 1;
    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.ebx = 3;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!(t.kind, abi::FAULT);
    let Event::GuestException {
        saved_eip,
        exception,
        ..
    } = event
    else {
        panic!("{event:?}");
    };
    assert_eq!(
        (saved_eip, exception),
        (pc + 2, GuestException::Interrupt { vector: 0x2d })
    );
    assert!(exception.is_trap());

    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.ebx = 2;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!(t.kind, abi::STOP);
    assert!(matches!(
        event,
        Event::GeneratedStop {
            reason: StopReason::Halt,
            pc: p,
            ..
        } if p == pc
    ));
}

#[test]
fn an_unmasked_sse_exception_is_a_guest_fault_at_its_pc() {
    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.ebx = 8;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!((t.kind, cpu.eip), (abi::FAULT, MAIN_RESUME + 1));
    assert!(matches!(
        event,
        Event::GuestException {
            pc,
            saved_eip,
            exception: GuestException::SimdException,
            ..
        } if pc == MAIN_RESUME + 1 && saved_eip == pc
    ));
    assert!(!GuestException::SimdException.is_trap());
}

#[test]
fn a_pending_x87_exception_is_a_guest_fault_with_the_status_word() {
    // ES, B, IE and SF set by an earlier unmasked stack fault; TOP = 7.
    let status = 0x8000 | 0x3800 | 0x80 | 0x41;
    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.ebx = 9;
    cpu.x87.status = status;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!((t.kind, cpu.eip), (abi::FAULT, MAIN_RESUME + 1));
    let exception = GuestException::FpError { status };
    assert!(matches!(
        event,
        Event::GuestException { pc, saved_eip, exception: e, .. }
            if pc == MAIN_RESUME + 1 && saved_eip == pc && e == exception
    ));
    assert!(!exception.is_trap());
    assert_eq!(
        exception.to_string(),
        "#MF x87 floating-point error (status word 0xb8c1)"
    );
}

#[test]
fn every_stop_code_decodes_and_stays_a_stop() {
    for (code, arg, reason) in [
        (8, 0, StopReason::Halt),
        (9, 0x1234, StopReason::BadEntry { selector: 0x1234 }),
        (10, 0x5678, StopReason::UnknownTarget { target: 0x5678 }),
        (11, 0, StopReason::DescentError),
        (12, 0, StopReason::Unmodelled),
    ] {
        let (_, mut cpu, mut ctx) = setup(|_| {});
        (cpu.ebx, cpu.ecx, cpu.edx) = (6, code, arg);
        let (t, event) = run(&mut cpu, &mut ctx, MAIN);
        assert_eq!(t.kind, abi::STOP, "code {code}");
        assert!(
            matches!(event, Event::GeneratedStop { reason: r, .. } if r == reason),
            "code {code}: {event:?}"
        );
    }
}

#[test]
fn an_ignored_exception_id_still_faults_and_unknown_codes_fault() {
    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.ebx = 4;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!(t.kind, abi::FAULT, "the handler returned a JUMP");
    assert!(matches!(
        event,
        Event::GuestException {
            exception: GuestException::InvalidOpcode,
            ..
        }
    ));

    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.ebx = 5;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!(t.kind, abi::FAULT);
    assert_eq!(
        event,
        Event::UnknownShimEvent {
            pc: MAIN_RESUME + 1,
            saved_eip: MAIN_RESUME + 1,
            code: 99,
            arg: 7
        }
    );
}

#[test]
fn unsupported_instructions_stop_with_mnemonic_detail_and_backtrace() {
    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.ebx = 7;
    let (t, event) = run(&mut cpu, &mut ctx, MAIN);
    assert_eq!(t.kind, abi::STOP);
    assert_eq!(
        event,
        Event::Unsupported {
            pc: MAIN_RESUME + 1,
            mnemonic: "fxsave".into(),
            detail: "x87: fxsave is not lowered".into(),
            backtrace: vec![GuestFrame {
                function: MAIN,
                pc: MAIN_RESUME + 1
            }],
        }
    );

    let (_, mut cpu, mut ctx) = setup(|_| {});
    cpu.edx = 2;
    let (t, _) = run(&mut cpu, &mut ctx, MAIN);
    let report = ctx.report(t.event_id).unwrap();
    assert_eq!(report.mnemonic.as_deref(), Some("aaa"));
    assert_eq!(report.backtrace.len(), 2);
    assert!(
        report
            .to_string()
            .contains("unsupported instruction `aaa`: integer: BCD adjust")
    );
}

#[test]
fn a_faulting_rep_keeps_progress_and_restarts_at_its_eip() {
    let (runtime, mut cpu, mut ctx) = setup(|_| {});
    let src = DATA;
    let dst = DATA + 0x1000 - 3; // three bytes fit before the unmapped page
    runtime
        .memory()
        .write(src, *b"ABCDEFGH")
        .expect("source is mapped");
    (cpu.esi, cpu.edi, cpu.ecx) = (src, dst, 8);
    (cpu.eflags, cpu.eflags_defined) = (0x2 | 0x40 | 0x1, 0x8d5);
    let (t, event) = run(&mut cpu, &mut ctx, MAIN_REP);
    assert_eq!(t.kind, abi::FAULT);
    let Event::MemoryFault { pc, fault } = event else {
        panic!("{event:?}");
    };
    assert_eq!(
        (pc, fault.addr, fault.access),
        (MAIN_REP, dst + 3, Access::Write)
    );
    assert_eq!((cpu.esi, cpu.edi, cpu.ecx), (src + 3, dst + 3, 5));
    assert_eq!((cpu.eflags, cpu.eflags_defined), (0x43, 0x8d5));
    assert_eq!(cpu.eip, MAIN_REP);
    let report = ctx.report(t.event_id).unwrap();
    assert_eq!(
        report.backtrace[0],
        GuestFrame {
            function: MAIN_REP,
            pc: MAIN_REP
        }
    );

    // Make the destination writable and restart the instruction at EIP.
    runtime
        .memory()
        .map_zeroed(DATA + 0x1000, 0x1000, Permissions::RW)
        .unwrap();
    let restart = cpu.eip;
    let (t, event) = run(&mut cpu, &mut ctx, restart);
    assert_eq!((t.kind, event), (abi::STOP, Event::HarnessExit));
    assert_eq!(cpu.ecx, 0);
    let copied = runtime.memory().read::<8>(dst).unwrap();
    assert_eq!(&copied, b"ABCDEFGH");
}

// ---- registration failures, on tables built here ------------------------

extern "C" fn good_probe() -> u32 {
    runtime::cpu::ABI_VERSION
}
extern "C" fn stale_probe() -> u32 {
    runtime::cpu::ABI_VERSION + 1
}

fn entry(addr: u32, kind: u32, probe: GeneratedAbiVersion) -> H2GenEntry {
    H2GenEntry {
        addr,
        kind,
        handler: Some(
            h2fn_00401000
                as unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32) -> TransferAbi,
        ),
        abi_version: Some(probe),
    }
}
const ZERO_ENTRY: H2GenEntry = H2GenEntry {
    addr: 0,
    kind: 0,
    handler: None,
    abi_version: None,
};
const ZERO_RESUME: H2GenResume = H2GenResume {
    pc: 0,
    owner: 0,
    start: 0,
};

fn try_module(
    mut entries: Vec<H2GenEntry>,
    mut resumes: Vec<H2GenResume>,
    probe: GeneratedAbiVersion,
) -> (Result<RegisteredModule, RegistrationError>, Dispatch) {
    let counts = (entries.len() as u32, resumes.len() as u32);
    entries.push(ZERO_ENTRY);
    resumes.push(ZERO_RESUME);
    let module = H2GenModule {
        abi_version: runtime::cpu::ABI_VERSION,
        entry_count: counts.0,
        resume_count: counts.1,
        reserved: 0,
        entries: entries.as_ptr(),
        resumes: resumes.as_ptr(),
    };
    let mut d = Dispatch::default();
    // SAFETY: the tables live for this call; handlers follow ABI v1.
    let result = unsafe { d.register_generated_module(&module, probe) };
    (result, d)
}

fn row_error(table: ModuleTable, index: usize, error: RegistrationError) -> RegistrationError {
    RegistrationError::ModuleRow {
        table,
        index,
        error: Box::new(error),
    }
}

#[test]
fn a_bad_module_registers_nothing_and_names_the_row() {
    let ok = || entry(0x1000, 0, good_probe);
    let resume = |pc, owner| H2GenResume {
        pc,
        owner,
        start: pc,
    };
    let (r, d) = try_module(vec![ok()], vec![], stale_probe);
    assert_eq!(
        r,
        Err(RegistrationError::ModuleAbiVersion {
            probe: runtime::cpu::ABI_VERSION + 1,
            table: runtime::cpu::ABI_VERSION,
            runtime: runtime::cpu::ABI_VERSION
        })
    );
    assert!(!d.has_generated(0x1000));

    let cases = [
        (
            vec![ok(), entry(0x2000, 0, stale_probe)],
            vec![],
            row_error(
                ModuleTable::Entries,
                1,
                RegistrationError::AbiVersion(runtime::cpu::ABI_VERSION + 1),
            ),
        ),
        (
            vec![ok(), entry(0x1000, 1, good_probe)],
            vec![],
            row_error(
                ModuleTable::Entries,
                1,
                RegistrationError::DuplicateGenerated(0x1000),
            ),
        ),
        (
            vec![ok(), entry(0x2000, 2, good_probe)],
            vec![],
            row_error(
                ModuleTable::Entries,
                1,
                RegistrationError::EntryKind {
                    addr: 0x2000,
                    kind: 2,
                },
            ),
        ),
        (
            vec![ok(), entry(0x1010, 1, good_probe)],
            vec![resume(0x1005, 0x1000), resume(0x1015, 0x1010)],
            row_error(
                ModuleTable::Resumes,
                1,
                RegistrationError::OwnerNotFunction(0x1010),
            ),
        ),
        (
            vec![ok()],
            vec![resume(0x1005, 0x1000), resume(0x1005, 0x1000)],
            row_error(
                ModuleTable::Resumes,
                1,
                RegistrationError::DuplicateResume(0x1005),
            ),
        ),
        (
            vec![
                ok(),
                H2GenEntry {
                    handler: None,
                    ..entry(0x2000, 0, good_probe)
                },
            ],
            vec![],
            row_error(
                ModuleTable::Entries,
                1,
                RegistrationError::NullEntry(0x2000),
            ),
        ),
    ];
    for (entries, resumes, expected) in cases {
        let (r, d) = try_module(entries, resumes, good_probe);
        assert_eq!(r, Err(expected));
        assert!(!d.has_generated(0x1000), "nothing is registered on error");
    }

    // A count that runs past the table's end, and a null table.
    let rows = [ok(), ok()];
    let module = H2GenModule {
        abi_version: runtime::cpu::ABI_VERSION,
        entry_count: 1,
        resume_count: 3,
        reserved: 0,
        entries: rows.as_ptr(),
        resumes: std::ptr::null(),
    };
    let mut d = Dispatch::default();
    // SAFETY: the entry table has count + 1 rows; the resume table is null.
    let r = unsafe { d.register_generated_module(&module, good_probe) };
    assert_eq!(r, Err(RegistrationError::NullTable(ModuleTable::Resumes)));
    let module = H2GenModule {
        resume_count: 0,
        ..module
    };
    // SAFETY: as above.
    let r = unsafe { d.register_generated_module(&module, good_probe) };
    assert_eq!(
        r,
        Err(RegistrationError::MissingTerminator(ModuleTable::Entries))
    );
}

// ---- review regressions --------------------------------------------------

/// Register `entries` and `resumes` exactly as given (terminators included)
/// into `d`, under a header claiming `abi_version` and the given counts.
fn register_raw(
    d: &mut Dispatch,
    abi_version: u32,
    entries: &[H2GenEntry],
    entry_count: u32,
    resumes: &[H2GenResume],
    resume_count: u32,
) -> Result<RegisteredModule, RegistrationError> {
    let module = H2GenModule {
        abi_version,
        entry_count,
        resume_count,
        reserved: 0,
        entries: entries.as_ptr(),
        resumes: resumes.as_ptr(),
    };
    // SAFETY: each table holds count + 1 readable rows; handlers follow ABI v1.
    unsafe { d.register_generated_module(&module, good_probe) }
}

/// A dispatch that already holds one generated original and one resume, so
/// late rejections can be checked to leave both alone and add nothing.
fn preloaded() -> Dispatch {
    let mut d = Dispatch::default();
    // SAFETY: the fixture handler follows ABI v1.
    unsafe { d.register_generated(0x9000, runtime::cpu::ABI_VERSION, h2fn_00401000) }.unwrap();
    d.register_resume(0x9005, 0x9000, 0x9005).unwrap();
    d
}
fn assert_untouched(d: &Dispatch) {
    assert!(d.has_generated(0x9000) && d.has_resume(0x9005));
    for addr in [0x1000, 0x2000] {
        assert!(!d.has_generated(addr), "{addr:#x} registered");
    }
    assert!(!d.has_resume(0x1005));
}

#[test]
fn the_table_header_version_is_checked_on_its_own() {
    let mut d = preloaded();
    let r = register_raw(
        &mut d,
        runtime::cpu::ABI_VERSION + 1,
        &[entry(0x1000, 0, good_probe), ZERO_ENTRY],
        1,
        &[ZERO_RESUME],
        0,
    );
    assert_eq!(
        r,
        Err(RegistrationError::ModuleAbiVersion {
            probe: runtime::cpu::ABI_VERSION,
            table: runtime::cpu::ABI_VERSION + 1,
            runtime: runtime::cpu::ABI_VERSION
        })
    );
    assert_untouched(&d);
}

#[test]
fn a_resume_owner_from_outside_the_module_is_rejected() {
    let mut d = preloaded();
    let r = register_raw(
        &mut d,
        runtime::cpu::ABI_VERSION,
        &[entry(0x1000, 0, good_probe), ZERO_ENTRY],
        1,
        &[
            H2GenResume {
                pc: 0x1005,
                owner: 0x1000,
                start: 0x1005,
            },
            H2GenResume {
                pc: 0x900a,
                owner: 0x9000,
                start: 0x900a,
            },
            ZERO_RESUME,
        ],
        2,
    );
    assert_eq!(
        r,
        Err(row_error(
            ModuleTable::Resumes,
            1,
            RegistrationError::OwnerNotFunction(0x9000)
        ))
    );
    assert_untouched(&d);
    assert!(!d.has_resume(0x900a));
}

#[test]
fn zero_addresses_are_rejected_in_both_tables() {
    let mut d = preloaded();
    // A zero entry address inside the counted rows (with a handler, so it
    // is not mistaken for the terminator).
    let r = register_raw(
        &mut d,
        runtime::cpu::ABI_VERSION,
        &[
            entry(0x1000, 0, good_probe),
            entry(0, 0, good_probe),
            ZERO_ENTRY,
        ],
        2,
        &[ZERO_RESUME],
        0,
    );
    assert_eq!(
        r,
        Err(row_error(
            ModuleTable::Entries,
            1,
            RegistrationError::ZeroAddress
        ))
    );
    assert_untouched(&d);
    let r = register_raw(
        &mut d,
        runtime::cpu::ABI_VERSION,
        &[entry(0x1000, 0, good_probe), ZERO_ENTRY],
        1,
        &[
            H2GenResume {
                pc: 0,
                owner: 0x1000,
                start: 0,
            },
            ZERO_RESUME,
        ],
        1,
    );
    assert_eq!(
        r,
        Err(row_error(
            ModuleTable::Resumes,
            0,
            RegistrationError::ZeroAddress
        ))
    );
    assert_untouched(&d);
}

#[test]
fn a_resume_count_short_of_the_table_is_a_missing_terminator() {
    let mut d = preloaded();
    let row = H2GenResume {
        pc: 0x1005,
        owner: 0x1000,
        start: 0x1005,
    };
    let r = register_raw(
        &mut d,
        runtime::cpu::ABI_VERSION,
        &[entry(0x1000, 0, good_probe), ZERO_ENTRY],
        1,
        &[row, row, ZERO_RESUME],
        1,
    );
    assert_eq!(
        r,
        Err(RegistrationError::MissingTerminator(ModuleTable::Resumes))
    );
    assert_untouched(&d);
    // The same tables with the right count register both rows... except
    // that they are duplicates, which names the second row.
    let r = register_raw(
        &mut d,
        runtime::cpu::ABI_VERSION,
        &[entry(0x1000, 0, good_probe), ZERO_ENTRY],
        1,
        &[row, row, ZERO_RESUME],
        2,
    );
    assert_eq!(
        r,
        Err(row_error(
            ModuleTable::Resumes,
            1,
            RegistrationError::DuplicateResume(0x1005)
        ))
    );
    assert_untouched(&d);
}

fn shim(ctx: &mut ExecCtx) -> *mut ExecCtxOpaque {
    (ctx as *mut ExecCtx).cast()
}

#[test]
fn every_shim_code_decodes_to_its_own_event() {
    use GuestException as X;
    use StopReason as S;
    let exceptions = [
        (1, 0, X::DivideError),
        (2, 0, X::Breakpoint),
        (3, 0, X::InvalidOpcode),
        (4, 0x21, X::Interrupt { vector: 0x21 }),
        (5, 0, X::Overflow),
        (6, 0, X::BoundRange),
        (7, 0, X::GeneralProtection),
        (13, 0, X::SimdException),
        (14, 0x80c1, X::FpError { status: 0x80c1 }),
    ];
    let (_, _, mut ctx) = setup(|_| {});
    for (code, arg, expected) in exceptions {
        // SAFETY: an exclusive, live context; no handler is running.
        let id = unsafe {
            runtime::generated::h2shim_v1_event(shim(&mut ctx), 0x1234, 0x1236, code, arg)
        };
        assert_ne!(id, 0);
        assert_eq!(
            ctx.event(id),
            Some(&Event::GuestException {
                pc: 0x1234,
                saved_eip: 0x1236,
                exception: expected,
                backtrace: vec![GuestFrame {
                    function: 0,
                    pc: 0x1234
                }],
            }),
            "code {code}"
        );
        assert_eq!(expected.is_trap(), matches!(code, 2 | 4 | 5), "code {code}");
    }
    let stops = [
        (8, 0, S::Halt),
        (9, 0x77, S::BadEntry { selector: 0x77 }),
        (10, 0x88, S::UnknownTarget { target: 0x88 }),
        (11, 0, S::DescentError),
        (12, 0, S::Unmodelled),
    ];
    for (code, arg, expected) in stops {
        // SAFETY: as above.
        let id =
            unsafe { runtime::generated::h2shim_v1_event(shim(&mut ctx), 0x99, 0x9a, code, arg) };
        assert!(
            matches!(
                ctx.event(id),
                Some(Event::GeneratedStop { pc: 0x99, saved_eip: 0x9a, reason, .. }) if *reason == expected
            ),
            "code {code}: {:?}",
            ctx.event(id)
        );
    }
    // Unused codes, and #MF with an argument no status word can have.
    for (code, arg) in [(0, 3), (0xff, 3), (u32::MAX, 3), (14, 0x1_0000)] {
        // SAFETY: as above.
        let id = unsafe { runtime::generated::h2shim_v1_event(shim(&mut ctx), 1, 2, code, arg) };
        assert_eq!(
            ctx.event(id),
            Some(&Event::UnknownShimEvent {
                pc: 1,
                saved_eip: 2,
                code,
                arg
            })
        );
    }
}

#[test]
fn diagnostic_strings_are_owned_and_read_no_further_than_the_cap() {
    use runtime::generated::{MAX_DIAGNOSTIC, h2shim_v1_unsupported};
    let (_, _, mut ctx) = setup(|_| {});
    let strings = |ctx: &ExecCtx, id| match ctx.event(id) {
        Some(Event::Unsupported {
            mnemonic, detail, ..
        }) => (mnemonic.clone(), detail.clone()),
        other => panic!("{other:?}"),
    };
    // Long but terminated: truncated to exactly the cap.
    let mut long = vec![b'm'; 3 * MAX_DIAGNOSTIC];
    long.push(0);
    // Exactly the cap and unterminated: the scan must stop at its end. The
    // buffer is followed by a byte that is never part of the string.
    let mut exact = vec![b'd'; MAX_DIAGNOSTIC];
    exact.push(b'X');
    // SAFETY: `long` is terminated; `exact` has MAX_DIAGNOSTIC readable bytes.
    let id = unsafe {
        h2shim_v1_unsupported(
            shim(&mut ctx),
            0x10,
            long.as_ptr().cast(),
            exact.as_ptr().cast(),
        )
    };
    // Changing the buffers afterwards does not change the retained event.
    long.fill(b'z');
    exact.fill(b'z');
    let (mnemonic, detail) = strings(&ctx, id);
    assert_eq!(mnemonic, "m".repeat(MAX_DIAGNOSTIC));
    assert_eq!(detail, "d".repeat(MAX_DIAGNOSTIC));
    assert_eq!(MAX_DIAGNOSTIC, 512);
    // Short strings and nulls.
    // SAFETY: terminated literal and a null pointer.
    let id =
        unsafe { h2shim_v1_unsupported(shim(&mut ctx), 0x11, c"aaa".as_ptr(), std::ptr::null()) };
    assert_eq!(strings(&ctx, id), ("aaa".into(), "<null>".into()));
}

#[test]
fn rdtsc_follows_the_guest_clock_and_never_repeats() {
    use runtime::generated::h2shim_v1_rdtsc;
    use runtime::tsc::{TSC_HZ, TSC_MIN_STEP};
    let (_, _, mut ctx) = setup(|_| {});
    // SAFETY: `ctx` is this thread's exclusive context.
    let mut read = || unsafe { h2shim_v1_rdtsc(shim(&mut ctx), 0x10) };
    let (a, b) = (read(), read());
    assert_eq!(b - a, TSC_MIN_STEP, "no clock installed: the step alone");
    let ticks = Arc::new(std::sync::atomic::AtomicU64::new(30_000_000));
    let t = ticks.clone();
    let clock = Arc::new(move || Some(t.load(std::sync::atomic::Ordering::SeqCst)));
    let (_, _, mut ctx) = setup(|_| {});
    ctx.runtime().set_guest_clock(Some(clock));
    // SAFETY: as above.
    let mut read = || unsafe { h2shim_v1_rdtsc(shim(&mut ctx), 0x10) };
    assert_eq!(read(), 3 * TSC_HZ, "three seconds of guest time");
    assert_eq!(read(), 3 * TSC_HZ + TSC_MIN_STEP);
    ticks.store(40_000_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(read(), 4 * TSC_HZ);
    assert!(ctx.events().is_empty(), "{:?}", ctx.events());
}

/// Rust overrides at three code addresses: MAIN calls LEAF, LEAF calls
/// THIRD, and THIRD either restores MAIN's ESP and jumps back into MAIN (a
/// longjmp) or tail-jumps to TAIL with ESP at its own return slot.
fn nonlocal(longjmp: bool) -> (Vec<GuestFrame>, Vec<GuestFrame>) {
    const M: u32 = 0x40_a000;
    const L: u32 = 0x40_b000;
    const T3: u32 = 0x40_c000;
    const TAIL: u32 = 0x40_d000;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let at_tail = seen.clone();
    let (_, mut cpu, mut ctx) = setup(|d| {
        let _ = d.register_override(M, "main", |cpu, ctx, _| {
            if cpu.ebx == 0 {
                cpu.ebx = 1;
                cpu.edi = cpu.esp; // setjmp
                let mut memory = ctx.memory();
                return memory.call(cpu, L, M + 5);
            }
            Ok(TransferAbi::stop(0))
        });
        let _ = d.register_override(L, "leaf", |cpu, ctx, _| {
            let mut memory = ctx.memory();
            memory.call(cpu, T3, L + 5)
        });
        let _ = d.register_override(T3, "third", move |cpu, _, _| {
            if longjmp {
                cpu.esp = cpu.edi; // longjmp to MAIN's saved stack
                Ok(TransferAbi::jump(M))
            } else {
                Ok(TransferAbi::jump(TAIL))
            }
        });
        let _ = d.register_override(TAIL, "tail", move |cpu, ctx, _| {
            *at_tail.lock().unwrap() = ctx.backtrace(cpu.eip);
            Ok(TransferAbi::stop(0))
        });
    });
    let t = Runner::default().run(&mut cpu, &mut ctx, M);
    assert_eq!((t.kind, t.event_id), (abi::STOP, 0));
    let after = ctx.backtrace(cpu.eip);
    let tail = seen.lock().unwrap().clone();
    (after, tail)
}

#[test]
fn a_longjmp_jump_drops_the_frames_it_abandoned() {
    let (after, _) = nonlocal(true);
    assert_eq!(
        after,
        vec![GuestFrame {
            function: 0x40_a000,
            pc: 0x40_a000
        }]
    );
}

#[test]
fn a_tail_jump_keeps_its_callers() {
    let (_, tail) = nonlocal(false);
    assert_eq!(
        tail,
        vec![
            GuestFrame {
                function: 0x40_d000,
                pc: 0x40_d000
            },
            GuestFrame {
                function: 0x40_b000,
                pc: 0x40_b005
            },
            GuestFrame {
                function: 0x40_a000,
                pc: 0x40_a005
            },
        ]
    );
}
