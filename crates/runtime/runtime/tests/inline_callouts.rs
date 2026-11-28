//! `ExecCtx::call_inline` (docs/perf/sim-10x-callouts.md): a Rust body
//! that calls a lifted function in place ends a run in the state the
//! segment path (return `CALL`, be entered again at the site) leaves, with
//! the same transfers, and every fallback is named.
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, ExecCtxOpaque, Fallback, GuestFault, H2GenEntry,
    H2GenEntryModule, H2GenModule, H2GenResume, Inline, InlineCensus, Runner, Runtime, TransferAbi,
    ffi::{h2abi_v1_read, h2abi_v1_write},
    generated::FEATURE_ENTRY_TABLE,
    guest::{Guest, Permissions},
    runner::HARNESS_EXIT,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Lifted code: the module's text.
const LEAF: u32 = 0x1000;
/// An interior entry (kind 1): never in the entry table.
const INTERIOR: u32 = 0x1004;
/// Tail-jumps to `LEAF`.
const TAIL: u32 = 0x1008;
/// Writes an unmapped page.
const FAULTS: u32 = 0x100c;
/// Stops, as a blocking call does.
const STOPS: u32 = 0x1010;
const TEXT: (u32, u32) = (0x1000, 0x1011);
/// The Rust body, outside the module, and its one return site.
const BODY: u32 = 0x8000;
const SITE: u32 = 0x8005;
const DATA: u32 = 0x5000;
const STACK_TOP: u32 = 0x11_0000;

extern "C" fn probe() -> u32 {
    runtime::cpu::ABI_VERSION
}

/// What the lifted leaf does: EAX = 3 EAX + 1, stored at DATA; `ret`.
///
/// # Safety
/// The handler ABI.
unsafe extern "C" fn leaf(cpu: *mut CpuAbi, exec: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
    // SAFETY: the handler ABI.
    let cpu = unsafe { &mut *cpu };
    cpu.eax = cpu.eax.wrapping_mul(3).wrapping_add(1);
    let v = cpu.eax.to_le_bytes();
    let mut ret = [0u8; 4];
    // SAFETY: the live pair the handler was given; local buffers.
    unsafe {
        assert_eq!(h2abi_v1_write(exec, LEAF, DATA, 4, 1, v.as_ptr()), 0);
        assert_eq!(
            h2abi_v1_read(exec, LEAF, cpu.esp, 4, 1, ret.as_mut_ptr()),
            0
        );
    }
    cpu.esp += 4;
    cpu.eip = u32::from_le_bytes(ret);
    TransferAbi::ret(cpu.eip)
}

unsafe extern "C" fn tail(cpu: *mut CpuAbi, _: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
    // SAFETY: the handler ABI.
    unsafe { (*cpu).eip = LEAF };
    TransferAbi::jump(LEAF)
}

unsafe extern "C" fn faults(_: *mut CpuAbi, exec: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
    let v = [0u8; 4];
    // SAFETY: the handler ABI; a local buffer.
    let e = unsafe { h2abi_v1_write(exec, FAULTS, 0x7000_0000, 4, 1, v.as_ptr()) };
    assert_ne!(e, 0);
    TransferAbi::fault(e)
}

unsafe extern "C" fn stops(_: *mut CpuAbi, _: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
    TransferAbi::stop(0)
}

type Handler = unsafe extern "C" fn(*mut CpuAbi, *mut ExecCtxOpaque, u32) -> TransferAbi;

/// The module, with an entry table over `TEXT` (leaked, as a loaded
/// module's tables outlive it).
fn register_module(d: &mut Dispatch) {
    let rows: [(u32, u32, Handler); 5] = [
        (LEAF, 0, leaf),
        (INTERIOR, 1, leaf),
        (TAIL, 0, tail),
        (FAULTS, 0, faults),
        (STOPS, 0, stops),
    ];
    let mut entries: Vec<H2GenEntry> = rows
        .iter()
        .map(|&(addr, kind, h)| H2GenEntry {
            addr,
            kind,
            handler: Some(h),
            abi_version: Some(probe),
        })
        .collect();
    entries.push(H2GenEntry {
        addr: 0,
        kind: 0,
        handler: None,
        abi_version: None,
    });
    let mut pcmap = vec![0u16; (TEXT.1 - TEXT.0) as usize];
    for (i, &(addr, ..)) in rows.iter().enumerate() {
        pcmap[(addr - TEXT.0) as usize] = i as u16 + 1;
    }
    let entries = entries.leak();
    let resumes = vec![H2GenResume {
        pc: 0,
        owner: 0,
        start: 0,
    }]
    .leak();
    let module = Box::leak(Box::new(H2GenEntryModule {
        module: H2GenModule {
            abi_version: runtime::cpu::ABI_VERSION,
            entry_count: rows.len() as u32,
            resume_count: 0,
            features: FEATURE_ENTRY_TABLE,
            entries: entries.as_ptr(),
            resumes: resumes.as_ptr(),
        },
        pcmap: pcmap.leak().as_ptr(),
        text_lo: TEXT.0,
        text_hi: TEXT.1,
    }));
    // SAFETY: each table holds count + 1 rows, pcmap covers the text, the
    // handlers follow the handler ABI, and all of it is leaked.
    unsafe { d.register_generated_module(&module.module, probe) }.unwrap();
}

/// How the body reaches its callee.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Return the `CALL` and be entered again at `SITE`.
    Segment,
    /// `call_inline`.
    Inline,
}

/// The body: ECX = 9, EAX = 5, `call target`; at the site EBX = EAX + 100,
/// `ret`. With `observe`, it watches its own return first.
fn body(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut pc: u32,
    mode: Mode,
    target: u32,
    observe: Option<Arc<AtomicU32>>,
) -> Result<TransferAbi, GuestFault> {
    loop {
        if pc == BODY {
            if let Some(seen) = observe.clone() {
                assert!(ctx.observe_guest_return(cpu, 0, move |_, _| {
                    seen.fetch_add(1, Ordering::Relaxed);
                }));
            }
            cpu.ecx = 9;
            cpu.eax = 5;
            let call = ctx.memory().call(cpu, target, SITE)?;
            if mode == Mode::Segment {
                return Ok(call);
            }
            match ctx.call_inline(cpu, target, SITE) {
                Inline::Returned => pc = SITE,
                Inline::Leave { transfer, .. } => return Ok(transfer),
            }
        } else {
            assert_eq!(pc, SITE);
            cpu.ebx = cpu.eax + 100;
            return ctx.memory().ret(cpu, 0);
        }
    }
}

/// One arm's setup.
#[derive(Clone)]
struct Arm {
    target: u32,
    observe: bool,
    /// Override `LEAF` with a Rust leaf (the callee then runs through the
    /// host-call thunk); with `disable`, it also disables the body.
    rust_leaf: bool,
    disable: bool,
    direct: bool,
    depth: Option<u32>,
    max_transfers: u64,
    profile: bool,
}

impl Default for Arm {
    fn default() -> Self {
        Self {
            target: LEAF,
            observe: false,
            rust_leaf: false,
            disable: false,
            direct: true,
            depth: None,
            max_transfers: 1_000,
            profile: false,
        }
    }
}

/// Everything a run leaves that the guest or the runner can see.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    kind: u32,
    event: Option<String>,
    regs: [u32; 4],
    data: u32,
    observed: u32,
    transfers: Option<u64>,
}

fn run(arm: &Arm, mode: Mode) -> (Outcome, InlineCensus, Option<u64>) {
    let mut g = Guest::new();
    g.map_zeroed(0x1000, 0x1000, Permissions::RX).unwrap();
    g.guard_code(0x1000, 0x1000).unwrap();
    g.map_zeroed(0x8000, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x10_0000, 0x1_0000, Permissions::RW).unwrap();
    g.set_u32(STACK_TOP - 4, HARNESS_EXIT).unwrap();
    let mut d = Dispatch::default();
    register_module(&mut d);
    let seen = Arc::new(AtomicU32::new(0));
    let observe = arm.observe.then(|| Arc::clone(&seen));
    let target = arm.target;
    // The guest's own body at BODY, resumed at SITE: the segment path.
    let original = observe.clone();
    d.register_generated_rust(BODY, move |cpu, ctx, pc| {
        body(cpu, ctx, pc, Mode::Segment, target, original.clone())
    })
    .unwrap();
    d.register_resume(SITE, BODY, SITE).unwrap();
    let ours = observe.clone();
    let over = move |cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32| {
        body(cpu, ctx, pc, mode, target, ours.clone())
    };
    d.register_override(BODY, "body", over.clone());
    d.implement_resumes(BODY, &[SITE], over);
    if arm.rust_leaf {
        let disable = arm.disable;
        d.register_override(LEAF, "leaf in Rust", move |cpu, ctx, _| {
            if disable {
                assert!(ctx.runtime().edit_dispatch(|d| d.set_enabled(BODY, false)));
            }
            cpu.eax = cpu.eax * 3 + 1;
            let mut mem = ctx.memory();
            mem.set_u32(DATA, cpu.eax)?;
            mem.ret(cpu, 0)
        });
    }
    let rt = Runtime::new(g, d);
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.enable_direct_calls(arm.direct);
    if let Some(depth) = arm.depth {
        ctx.set_direct_call_depth(depth);
    }
    ctx.enable_profiling(arm.profile);
    let mut cpu = CpuAbi {
        esp: STACK_TOP - 4,
        ..CpuAbi::default()
    };
    let t = Runner {
        max_transfers: arm.max_transfers,
    }
    .run(&mut cpu, &mut ctx, BODY);
    let profile = ctx.profile().map(|p| (p.transfers, p.direct_transfers));
    let outcome = Outcome {
        kind: t.kind,
        event: ctx.event(t.event_id).map(|e| format!("{e:?}")),
        regs: [cpu.eax, cpu.ebx, cpu.ecx, cpu.esp],
        data: rt.memory().u32(DATA).unwrap(),
        observed: seen.load(Ordering::Relaxed),
        transfers: profile.map(|p| p.0),
    };
    (outcome, ctx.inline_census(), profile.map(|p| p.1))
}

/// Both arms; the outcomes must match. The inline arm's census.
fn both(arm: &Arm) -> (Outcome, InlineCensus) {
    let (segment, census, _) = run(arm, Mode::Segment);
    assert_eq!(
        census,
        InlineCensus::default(),
        "the segment arm never calls in place"
    );
    let (inline, census, _) = run(arm, Mode::Inline);
    assert_eq!(
        inline,
        segment,
        "{arm:?}",
        arm = (arm.target, arm.max_transfers)
    );
    (inline, census)
}

fn only(why: Fallback) -> InlineCensus {
    let mut c = InlineCensus::default();
    c.left[why.index()] = 1;
    c
}

fn exited(o: &Outcome) -> bool {
    o.event.as_deref() == Some(format!("{:?}", Event::HarnessExit).as_str())
}

#[test]
fn a_body_calling_a_lifted_function_in_place_ends_as_the_segment_path_does() {
    for profile in [false, true] {
        let arm = Arm {
            profile,
            ..Arm::default()
        };
        let (out, census) = both(&arm);
        assert!(exited(&out), "{out:?}");
        assert_eq!(out.regs, [16, 116, 9, STACK_TOP]);
        assert_eq!(out.data, 16);
        assert_eq!(
            census,
            InlineCensus {
                returned: 1,
                ..InlineCensus::default()
            }
        );
    }
    // The call and the return are direct transfers, not the runner's.
    let arm = Arm {
        profile: true,
        ..Arm::default()
    };
    assert_eq!(run(&arm, Mode::Segment).2, Some(0));
    assert_eq!(run(&arm, Mode::Inline).2, Some(2));
}

#[test]
fn a_body_calling_an_overridden_entry_goes_through_the_thunk_in_place() {
    let arm = Arm {
        rust_leaf: true,
        ..Arm::default()
    };
    let (out, census) = both(&arm);
    assert!(exited(&out));
    assert_eq!((out.regs, out.data), ([16, 116, 9, STACK_TOP], 16));
    assert_eq!(census.returned, 1);
}

/// Every limit gives the same result on the same transfer in both arms:
/// the budget is spent as the runner spends it.
#[test]
fn the_transfer_limit_falls_on_the_same_transfer() {
    for max_transfers in 1..=5 {
        let (out, census) = both(&Arm {
            max_transfers,
            ..Arm::default()
        });
        let limit = format!("{:?}", Event::TransferLimit);
        match max_transfers {
            // The body's own entry leaves no transfer for the call.
            1 => assert_eq!(
                (out.event.as_deref(), census),
                (Some(&*limit), only(Fallback::Busy))
            ),
            // None for the return.
            2 => assert_eq!(
                (out.event.as_deref(), census),
                (Some(&*limit), only(Fallback::ResumeBusy))
            ),
            _ => assert!(exited(&out) && census.returned == 1),
        }
    }
}

#[test]
fn each_fallback_is_named_and_ends_as_the_segment_path_does() {
    let cases = [
        (
            Arm {
                direct: false,
                ..Arm::default()
            },
            Fallback::Off,
        ),
        (
            Arm {
                observe: true,
                ..Arm::default()
            },
            Fallback::Observer,
        ),
        (
            Arm {
                depth: Some(0),
                ..Arm::default()
            },
            Fallback::Depth,
        ),
        (
            Arm {
                target: INTERIOR,
                ..Arm::default()
            },
            Fallback::NotLive,
        ),
        (
            Arm {
                target: STOPS,
                ..Arm::default()
            },
            Fallback::Yield,
        ),
        (
            Arm {
                target: FAULTS,
                ..Arm::default()
            },
            Fallback::Fault,
        ),
        (
            Arm {
                target: TAIL,
                ..Arm::default()
            },
            Fallback::Handed,
        ),
        (
            Arm {
                rust_leaf: true,
                disable: true,
                ..Arm::default()
            },
            Fallback::Dispatch,
        ),
    ];
    for (arm, why) in cases {
        let (out, census) = both(&arm);
        assert_eq!(census, only(why), "{}", why.name());
        match why {
            Fallback::Yield => assert_eq!((out.kind, out.event), (runtime::abi::STOP, None)),
            Fallback::Fault => assert_eq!(out.kind, runtime::abi::FAULT),
            Fallback::Observer => assert!(exited(&out) && out.observed == 1),
            _ => assert!(exited(&out), "{}: {out:?}", why.name()),
        }
    }
}

#[test]
fn fallback_names_are_distinct_and_in_order() {
    let mut names: Vec<_> = Fallback::ALL.iter().map(|f| f.name()).collect();
    for (i, f) in Fallback::ALL.iter().enumerate() {
        assert_eq!(f.index(), i);
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), Fallback::ALL.len());
}
