//! The sim-tier experiment (`docs/engine/sim-tier-experiment.md`): the
//! four leaf families with Rust ports and the Havok solver, compiled by
//! `emit::sim` from the same lifted IR as the reference tier, checked
//! against the IR interpreter and timed against the reference tier and the
//! hand ports.
//!
//! The user's XBE is the oracle; inputs are synthetic and seeded. A case
//! the interpreter cannot finish (a fixture pointer it cannot follow) is
//! skipped and counted. A case the tier hands back (a rare event) is
//! counted. Every case the tier completes must leave the 8 MiB of guest RAM
//! and every CPU field but MXCSR exactly as the interpreter does.
//!
//! Correctness:
//! `H2_XBE=<xbe> cargo test -p engine --features differential --test it sim_tier_differential::`
//! Bench (results table rows on stdout):
//! `H2_XBE=<xbe> cargo test -p engine --release --features differential --test it sim_tier_differential:: -- --ignored --nocapture`
#![cfg(feature = "differential")]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use emit::sim::{self, Closure};
use guest_layout::points::{self, Point};
use engine::sim_native as native;
use ir::interp::{self, Cpu, FlatMemory};
use runtime::guest::{Access, FaultReason, Permissions};
use runtime::runner::HARNESS_EXIT as EXIT;
use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, GuestFault, Runner, Runtime, TransferAbi};

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
// Native leaf fixtures (as tests/sim_native_differential.rs).
const A: u32 = 0x0062_0000;
const B: u32 = A + 0x100;
const OUT: u32 = B + 0x100;
const TAGS: u32 = A + 0x3800;
const DEFINITION: u32 = A + 0x4000;
const MODEL: u32 = A + 0x4800;
// Solver fixture (as tests/sim_physics_differential.rs).
const DATA: u32 = 0x0060_5000;
const RECORDS: u32 = 0x0061_0000;
const SOLVER_CHILD: u32 = 0x006e_0020;
/// A synthetic `ret`: what the reference runner costs around nothing.
const EMPTY: u32 = 0x007e_0000;
/// The solver's "ready" byte: zero sends it through its setup call.
const SOLVER_READY: u32 = 0x0048_0c60;

const CASES: u32 = 48;
/// Calls per timing trial, and trials (the minimum is kept).
const BATCH: u32 = 200;
const TRIALS: usize = 5;

type Boundary = unsafe extern "C" fn(*mut CpuAbi, *mut u8, *mut u32) -> u32;

// ---- memory for the hand ports -------------------------------------------

struct Flat(Vec<u8>);

fn fault(addr: u32, access: Access) -> GuestFault {
    GuestFault { addr, len: 1, access, reason: FaultReason::Unmapped }
}

impl native::Memory for Flat {
    fn read_bytes(&self, a: u32, dst: &mut [u8], _: u32) -> Result<(), GuestFault> {
        let s = self.0.get(a as usize..a as usize + dst.len()).ok_or(fault(a, Access::Read))?;
        dst.copy_from_slice(s);
        Ok(())
    }
    fn write_bytes(&mut self, a: u32, src: &[u8], _: u32) -> Result<(), GuestFault> {
        let n = src.len();
        self.0.get_mut(a as usize..a as usize + n).ok_or(fault(a, Access::Write))?.copy_from_slice(src);
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Port {
    Native(fn(&mut CpuAbi, &mut Flat) -> native::Result),
    Physics(&'static engine::sim_physics::Port),
}

struct Root {
    point: &'static Point,
    fixture: fn(&mut FlatMemory, &mut Cpu, u32, &mut u64),
    port: Port,
}

fn roots() -> Vec<Root> {
    let solver = engine::sim_physics::PORTS.iter().chain(engine::sim_physics::ORACLES).find(|p| p.point.base == points::HAVOK_SOLVER.base).expect("solver port");
    vec![
        Root { point: &points::OBJECT_TRANSFORM_COMPOSE, fixture: compose_fixture, port: Port::Native(|c, m| native::compose(c, m)) },
        Root { point: &points::OBJECT_POSE_MATRIX, fixture: pose_fixture, port: Port::Native(|c, m| native::pose_matrix(c, m)) },
        Root { point: &points::SIM_MOPP_BOX, fixture: mopp_fixture, port: Port::Native(|c, m| native::mopp_query::box_query(c, m)) },
        Root { point: &points::COLLISION_BSP_SEGMENT, fixture: bsp_fixture, port: Port::Native(|c, m| native::bsp::segment(c, m)) },
        Root { point: &points::HAVOK_SOLVER, fixture: solver_fixture, port: Port::Physics(solver) },
    ]
}

// ---- seeded inputs ---------------------------------------------------------

fn next(r: &mut u64) -> u64 {
    *r ^= *r << 13;
    *r ^= *r >> 7;
    *r ^= *r << 17;
    *r
}

/// Mostly magnitudes within 2^+-12 of one; one in 16 is a signed zero.
fn float(r: &mut u64) -> u32 {
    let x = next(r);
    if x & 15 == 0 {
        return (x as u32) & 0x8000_0000;
    }
    let exp = 127 - 12 + (x >> 8) % 25;
    ((x >> 4) as u32 & 0x8000_0000) | (exp as u32) << 23 | (x >> 32) as u32 & 0x7f_ffff
}

fn word(m: &mut FlatMemory, a: u32, v: u32) {
    assert!(m.load(a, &v.to_le_bytes()));
}

fn compose_fixture(m: &mut FlatMemory, c: &mut Cpu, n: u32, r: &mut u64) {
    for base in [A, B] {
        for i in 0..13 {
            word(m, base + 4 * i, float(r));
        }
    }
    c.gpr[1] = A;
    c.gpr[2] = B;
    word(m, STACK + 4, [OUT, A, B, A + 4][(n % 4) as usize]);
}

fn pose_fixture(m: &mut FlatMemory, _: &mut Cpu, n: u32, r: &mut u64) {
    for i in 0..8 {
        let v = if n % 5 == 0 { 0 } else { float(r) };
        word(m, A + 4 * i, v);
    }
    word(m, native::pose::POSE_INIT_FLAGS, (n % 2) | 0x9000_0000);
    word(m, native::pose::POSE_NORM_THRESHOLD, 1.0e-5f32.to_bits());
    word(m, STACK + 4, [OUT, A, A + 8][(n / 3 % 3) as usize]);
    word(m, STACK + 8, A);
}

fn bsp_fixture(m: &mut FlatMemory, _: &mut Cpu, n: u32, r: &mut u64) {
    let (bsp, nodes, planes, leaves, pos, delta) = (B, MODEL, MODEL + 48, TAGS, A + 160, A + 176);
    word(m, A, n % 8);
    word(m, A + 4, bsp);
    word(m, A + 16, pos);
    word(m, A + 20, delta);
    word(m, A + 24, OUT);
    word(m, A + 28, 2);
    assert!(m.load(A + 32, &[(n % 4) as u8]));
    word(m, A + 36, 0);
    word(m, OUT, 1.0f32.to_bits());
    word(m, bsp + 4, nodes);
    word(m, bsp + 12, planes);
    word(m, bsp + 20, leaves);
    word(m, bsp + 44, DEFINITION);
    assert!(m.load(nodes + 2, &[0, 0, 0x80]));
    assert!(m.load(nodes + 5, &[1, 0, 0x80]));
    for a in [planes, planes + 4, planes + 8, planes + 12, pos, pos + 4, pos + 8, delta, delta + 4, delta + 8] {
        word(m, a, float(r));
    }
    assert!(m.load(leaves, &[1]));
    assert!(m.load(leaves + 4, &[0]));
    let root = match n % 4 {
        0 => u32::MAX,
        1 => 0xff80_0000,
        _ => 0,
    };
    word(m, STACK + 4, A);
    word(m, STACK + 8, root);
    word(m, STACK + 12, 0);
    word(m, STACK + 16, if n % 4 == 3 { 1.0f32.to_bits() } else { 0 });
}

fn mopp_fixture(m: &mut FlatMemory, c: &mut Cpu, n: u32, r: &mut u64) {
    let (state, code, keys) = (MODEL, TAGS, OUT + 64);
    c.gpr[1] = A;
    word(m, A, OUT);
    word(m, OUT, keys);
    word(m, OUT + 8, 32);
    word(m, state, if n % 3 == 0 { 1 } else { 130 });
    word(m, state + 16, if n % 3 == 1 { 124 } else { 3 });
    word(m, state + 44, 97);
    for i in 1..4 {
        word(m, state + 4 * i, next(r) as u32 & 0xff);
    }
    word(m, STACK + 4, state);
    word(m, STACK + 8, code);
    let stream: &[u8] = match n % 12 {
        0 => &[0x10, 100, 120, 1, 0x32, 0x33],
        1 => &[0x20, 95, 1, 0x33, 0x37],
        2 => &[0x26, 10, 100, 0x50, 27],
        3 => &[0x05, 1, 0, 0x51, 1, 7],
        4 => &[0x07, 0, 0, 0, 0x52, 1, 2, 3],
        5 => &[0x29, 0, 0, 0, 0, 2, 0, 0x53, 1, 2, 3, 4],
        6 => &[0x10, 16, 11, 1, 0x33, 0x36],
        7 => &[0x09, 3, 0x13, 100, 120, 1, 0x32, 0x33],
        8 => &[0x02, 1, 2, 3, 0x0a, 1, 9, 0x50, 1],
        9 => &[0x23, 100, 10, 0, 0, 0, 1, 0x32, 0x33],
        10 => &[0x60, 7, 0x64, 0, 15, 0x68, 0, 0, 0, 31, 0x38],
        _ => &[0x0b, 0, 1, 2, 3, 0x1c, 178, 220, 1, 0x37, 0x38],
    };
    assert!(m.load(code, stream));
}

/// The solver's command stream, built from handler identities in the
/// image's own dispatch tables (tests/sim_physics_differential.rs).
fn solver_fixture(m: &mut FlatMemory, _: &mut Cpu, case: u32, r: &mut u64) {
    let case = case % 23;
    let (param, schema, rows, out, acc, imp) = (DATA, DATA + 128, DATA + 512, DATA + 1024, RECORDS, RECORDS + 1024);
    m.bytes[DATA as usize..(DATA + 2048) as usize].fill(0);
    for (i, p) in [param, schema, acc, imp].into_iter().enumerate() {
        word(m, STACK + 4 + 4 * i as u32, p);
    }
    m.bytes[SOLVER_READY as usize] = u8::from(case != 0);
    word(m, param + 28, if case >= 6 { 2 } else { u32::from(case >= 4) });
    for off in [4, 8, 12, 16] {
        word(m, param + off, float(r) & 0x7fff_ffff);
    }
    for (off, v) in [(4, rows), (8, out), (12, acc), (16, acc + 128), (20, 12)] {
        word(m, schema + off, v);
    }
    let lookup = |m: &FlatMemory, form: u32| -> u8 {
        (0..=16u32)
            .find(|i| {
                let class = m.bytes[(0x31e724 + i) as usize];
                m.read_u32(0x31e704 + 4 * u32::from(class)) == Some(form)
            })
            .expect("schema class") as u8
    };
    let done = lookup(m, 0x31e696);
    let mut p = schema + 24;
    if (1..4).contains(&case) {
        let forms: &[u32] = match case {
            1 => &[0x31e567, 0x31e567, 0x31e5aa],
            2 => &[0x31e52a, 0x31e5ed],
            _ => &[0x31e631],
        };
        for &form in forms {
            let size = match form {
                0x31e52a => 8,
                0x31e5ed => 20,
                0x31e631 => 24,
                _ => 4,
            };
            let id = lookup(m, form);
            m.bytes[p as usize] = id;
            m.bytes[(p + 1) as usize] = size as u8;
            if size >= 20 {
                word(m, p + 4, out + 128);
                word(m, p + 16, 12);
            }
            p += size;
        }
    } else if case >= 4 {
        let instructions: &[u32] = match case {
            4 | 5 => &[0x31cf97, 0x31cf97],
            6 => &[0x31da55, 0x31db21],
            7 => &[0x31d982, 0x31df3e],
            8 => &[0x31dc3f, 0x31d796],
            9 => &[0x31d88e, 0x31dd80],
            10..=13 => &[0x31c9c6, 0x31c9c6],
            14..=16 => &[0x31d532],
            17..=19 => &[0x31d1fb],
            20 => &[0x31e068, 0x31cf97, 0x31e08d],
            21 => &[0x31e0a6],
            _ => &[0x31c9a0, 0x31df3e, 0x31df3e],
        };
        for &handler in instructions {
            let (size, correction) = match handler {
                0x31cf97 | 0x31db21 => (4, 0x31e400),
                0x31da55 => (4, 0x31e472),
                0x31d982 | 0x31d88e => (16, 0x31e472),
                0x31df3e => (16, 0x31e400),
                0x31dc3f => (8, 0x31e400),
                0x31d796 => (8, 0x31e472),
                0x31dd80 => (12, 0x31e400),
                0x31c9c6 => (8, 0x31e161),
                0x31d532 => (20, 0x31e161),
                0x31d1fb => (24, 0x31e2e9),
                0x31c9a0 => (24, 0x31e0e1),
                0x31e068 => (12, 0x31e4cf),
                0x31e08d => (4, 0x31e4cf),
                _ => (12, 0x31e4cf),
            };
            let id = (0..=16u32).find(|i| m.read_u32(0x31e6a4 + 4 * i) == Some(handler)).unwrap();
            let pass = (0..=6u32).find(|i| m.read_u32(0x31e6e8 + 4 * i) == Some(correction)).unwrap();
            word(m, p, id | (size << 8) | (pass << 16));
            if size >= 8 {
                word(m, p + 4, float(r));
            }
            if size >= 12 {
                word(m, p + 8, float(r));
            }
            if size >= 16 {
                word(m, p + 12, if case % 2 == 0 { 0.02f32 } else { 9.0 }.to_bits());
            }
            if size >= 20 {
                word(m, p + 4, out + 128);
                word(m, p + 16, 12);
            }
            if size == 24 {
                word(m, p + 20, 1.125f32.to_bits());
            }
            if handler == 0x31e0a6 {
                word(m, p + 4, SOLVER_CHILD);
            }
            if handler == 0x31c9a0 {
                for (off, v) in [(4, rows), (8, out), (12, acc), (16, acc + 128), (20, 12)] {
                    word(m, p + off, v);
                }
            }
            p += size;
        }
    }
    let end = (0..=6u32).find(|i| m.read_u32(0x31e6e8 + 4 * i) == Some(0x31e4da)).unwrap();
    word(m, p, u32::from(done) | (end << 16));
    for i in 0..64 {
        word(m, rows + 4 * i, float(r));
        word(m, acc + 4 * i, float(r));
        word(m, imp + 4 * i, float(r));
    }
    m.bytes[acc as usize] = 2;
    m.bytes[(acc + 128) as usize] = 2;
    m.bytes[(acc + 256) as usize] = 3;
}

// ---- the oracle ------------------------------------------------------------

struct Image {
    bytes: Vec<u8>,
    analysis: analyze::Analysis,
    ram: FlatMemory,
}

fn image() -> Option<&'static Image> {
    static IMAGE: OnceLock<Option<Image>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let Some(path) = common::xbe_path() else {
                assert!(std::env::var_os("H2_XBE").is_none(), "H2_XBE does not name a readable file");
                eprintln!("SKIPPED sim tier differential: set H2_XBE");
                return None;
            };
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256);
            let xbe = xbe::Xbe::parse(&bytes).unwrap();
            let analysis = analyze::analyze(&xbe).unwrap();
            let mut ram = lift::harness::load_image(&xbe).unwrap();
            ram.bytes.truncate(RAM);
            Some(Image { bytes, analysis, ram })
        })
        .as_ref()
}

/// Run from `cpu.eip` to [`EXIT`] on the interpreter; `None` if it cannot
/// (an instruction that does not lift, a fault, a step limit).
fn oracle(lifter: &lift::Lifter<'_>, cache: &mut BTreeMap<u32, ir::IrInsn>, cpu: &mut Cpu, mem: &mut FlatMemory) -> Option<()> {
    for _ in 0..2_000_000 {
        let pc = cpu.eip;
        if pc == EXIT {
            return Some(());
        }
        if !cache.contains_key(&pc) {
            let f = lifter.lift(pc)?;
            for i in f.blocks.into_values().flat_map(|b| b.insns) {
                cache.entry(i.pc).or_insert(i);
            }
        }
        let insn = cache.get(&pc)?;
        cpu.eip = interp::step(cpu, mem, insn).ok()?.target();
    }
    None
}

fn abi(c: &Cpu) -> CpuAbi {
    let [eax, ecx, edx, ebx, esp, ebp, esi, edi] = c.gpr;
    let x = &c.x87;
    CpuAbi {
        eax,
        ecx,
        edx,
        ebx,
        esp,
        ebp,
        esi,
        edi,
        eip: c.eip,
        fs_base: c.fs_base,
        mxcsr: c.mxcsr,
        eflags: c.eflags,
        eflags_defined: c.flags_valid,
        xmm: c.xmm.map(u128::to_le_bytes),
        x87: runtime::cpu::X87Abi {
            st: x.st,
            control: x.control,
            status: x.status,
            tag: x.tag,
            opcode: x.opcode,
            instruction_pointer: x.instruction_pointer,
            data_pointer: x.data_pointer,
            instruction_selector: x.instruction_selector,
            data_selector: x.data_selector,
        },
        ..Default::default()
    }
}

/// MXCSR is outside the tier's contract (its sticky flags are not kept).
fn comparable(mut c: CpuAbi) -> CpuAbi {
    c.mxcsr = 0;
    c
}

/// A seeded start state for case `n` of `root`.
fn start(img: &Image, root: &Root, n: u32) -> (Cpu, FlatMemory) {
    let mut r = (u64::from(root.point.base) << 16 | u64::from(n)).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut mem = img.ram.clone();
    let mut cpu = Cpu::default();
    for g in &mut cpu.gpr {
        *g = next(&mut r) as u32;
    }
    cpu.gpr[4] = STACK;
    cpu.eip = root.point.base;
    cpu.eflags = 2 | (next(&mut r) as u32 & 0x8d5);
    for x in &mut cpu.xmm {
        *x = (0..4).fold(0u128, |v, i| v | u128::from(float(&mut r)) << (32 * i));
    }
    // The tick's FP environment: PC=53, round to nearest, all masked.
    cpu.x87.control = 0x027f;
    cpu.mxcsr = 0x1f80;
    word(&mut mem, STACK, EXIT);
    (root.fixture)(&mut mem, &mut cpu, n, &mut r);
    (cpu, mem)
}

// ---- the three tiers -------------------------------------------------------

struct Built {
    generated: emit::host::Generated,
    tier: sim::Module,
    ctx: ExecCtx,
}

/// A synthetic `ret` at [`EMPTY`], as IR.
fn empty() -> ir::IrFunction {
    use ir::{Addr, BinOp, Control, Expr, Gpr, IrBlock, IrInsn, Segment, Slice, Stmt, Temp, Value, Width};
    let t = |id| Temp { id, width: Width::W32 };
    let ret = IrInsn {
        pc: EMPTY,
        len: 1,
        atomic: None,
        stmts: vec![
            Stmt::ReadReg { dst: t(0), reg: Gpr::Esp, slice: Slice::Full },
            Stmt::Load { dst: t(1), addr: Addr { seg: Segment::Flat, offset: t(0).into() } },
            Stmt::Assign { dst: t(2), expr: Expr::Binary { op: BinOp::Add, a: t(0).into(), b: Value::u32(4) } },
            Stmt::WriteReg { reg: Gpr::Esp, slice: Slice::Full, value: t(2).into() },
        ],
        control: Control::Return { target: t(1).into(), cleanup: 0 },
    };
    ir::IrFunction {
        entry: EMPTY,
        blocks: BTreeMap::from([(EMPTY, IrBlock { start: EMPTY, insns: vec![ret] })]),
        interior_entries: vec![],
        errors: vec![],
    }
}

/// Compile the closure of every root twice into one dylib: the reference
/// tier (`emit::write_module_with`, run by the production runtime) and the
/// sim tier (`emit::sim::write`, called through its boundaries).
fn build(img: &Image, lifter: &lift::Lifter<'_>, roots: &[u32], name: &str) -> Built {
    let closure = Closure::build(roots, 4096, |pc| lifter.lift(pc));
    let tier = sim::write(&closure, roots);
    eprintln!(
        "closure: {} functions, {} missing, {} compiled by the tier, {} refused",
        closure.functions.len(),
        closure.missing.len(),
        tier.tier.signatures.len(),
        tier.tier.refused.len()
    );
    for (pc, why) in &tier.tier.refused {
        eprintln!("  refused {pc:#010x}: {why}");
    }
    let mut funcs: Vec<ir::IrFunction> = closure.functions.into_values().collect();
    funcs.push(empty());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let src = dir.join("src");
    emit::write_module_with(&emit::plan(funcs), &src, emit::Codegen::default()).unwrap();
    let header = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/runtime/include/h2abi.h");
    emit::compile::install_header(&src, &header).unwrap();
    std::fs::write(dir.join("tier.c"), &tier.c).unwrap();
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let b = emit::compile::build(&src, &dir, emit::compile::Arch::host(), jobs).unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join(format!("lib{name}.dylib"));
    let extras = [("bridge.c", emit::host::BRIDGE), ("tier.c", tier.c.as_str())];
    emit::compile::host_dylib(&b, &src, &extras, &dylib).unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now by this crate's emitter for this host.
    let generated = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    let mut dispatch = Dispatch::default();
    // The production runtime's bridge, as tests in crates/recompiler/emit/tests/perf.rs
    // install it.
    unsafe {
        let sym = |n: &str| generated.symbol(n).unwrap_or_else(|| panic!("{n}"));
        let install: unsafe extern "C" fn(*const *const (), usize) -> u32 = std::mem::transmute(sym("h2bridge_install_v3"));
        let layout: unsafe extern "C" fn(u32) -> usize = std::mem::transmute(sym("h2bridge_layout_v3"));
        let callbacks = [
            runtime::ffi::h2abi_v1_read as *const (),
            runtime::ffi::h2abi_v1_write as *const (),
            runtime::generated::h2shim_v1_event as *const (),
            runtime::generated::h2shim_v1_unsupported as *const (),
            runtime::generated::h2shim_v1_rdtsc as *const (),
            runtime::ffi::h2abi_v1_atomic_rmw as *const (),
            runtime::ffi::h2abi_v1_xchg as *const (),
            runtime::ffi::h2abi_v1_cmpxchg as *const (),
            runtime::ffi::h2abi_v1_profile as *const (),
            runtime::ffi::h2abi_v1_fast_acquire as *const (),
            runtime::ffi::h2abi_v1_fast_release as *const (),
            runtime::ffi::h2abi_v1_read_counted as *const (),
            runtime::ffi::h2abi_v1_write_counted as *const (),
            runtime::ffi::h2abi_v1_charge as *const (),
            runtime::ffi::h2abi_v1_budget as *const (),
            runtime::ffi::h2abi_v1_exit as *const (),
        ];
        assert_eq!(layout(callbacks.len() as u32), size_of_val(&callbacks));
        assert_eq!(install(std::ptr::null(), 0), 0);
        assert_eq!(install(callbacks.as_ptr(), size_of_val(&callbacks)), 1);
        let module = sym("h2gen_module") as *const runtime::H2GenModule;
        let version: runtime::ffi::GeneratedAbiVersion = std::mem::transmute(sym("h2gen_module_abi_version"));
        dispatch.register_generated_module(&*module, version).unwrap();
    }
    let mut guest = Guest::new();
    guest.map_zeroed(0, RAM as u32, Permissions::RWX).unwrap();
    guest.write_bytes(0, &img.ram.bytes, 1).unwrap();
    let ctx = ExecCtx::new(Runtime::new(guest, dispatch), 1);
    Built { generated, tier, ctx }
}

fn boundary(b: &Built, root: u32) -> Option<Boundary> {
    let p = b.generated.symbol(&sim::entry_symbol(root))?;
    // SAFETY: `emit::sim` exports every boundary with this signature.
    Some(unsafe { std::mem::transmute::<*const std::ffi::c_void, Boundary>(p) })
}

fn guest_ram(ctx: &mut ExecCtx) -> Vec<u8> {
    let g = ctx.memory();
    let at = g.mappings()[0].physical_start as usize;
    g.physical_ram()[at..at + RAM].to_vec()
}

/// Byte ranges where `a` and `b` differ, joined across small gaps.
fn dirty(a: &[u8], b: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, _) in a.iter().zip(b).enumerate().filter(|(_, (x, y))| x != y) {
        match out.last_mut() {
            Some(r) if i <= r.end + 64 => r.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out
}

/// Nanoseconds per call of `f`: the minimum over [`TRIALS`] of [`BATCH`].
fn time(mut f: impl FnMut()) -> f64 {
    (0..TRIALS)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..BATCH {
                f();
            }
            t.elapsed().as_secs_f64() * 1e9 / f64::from(BATCH)
        })
        .fold(f64::INFINITY, f64::min)
}

#[derive(Default, Debug)]
struct Tally {
    cases: u32,
    oracle_skipped: u32,
    tier_ok: u32,
    rare: BTreeMap<u32, u32>,
    reference_ok: u32,
    port_ok: u32,
    /// Per timed case: (tier, reference, port) ns per call, net of restore.
    times: Vec<(f64, f64, Option<f64>)>,
}

fn run(root: &Root, img: &Image, lifter: &lift::Lifter<'_>, built: &mut Built, bench: bool) -> Tally {
    let entry = root.point.base;
    let call = boundary(built, entry).unwrap_or_else(|| panic!("{} refused by the tier", root.point.name));
    let mut cache = BTreeMap::new();
    let mut t = Tally::default();
    for n in 0..CASES {
        t.cases += 1;
        let (cpu0, mem0) = start(img, root, n);
        let (mut want, mut want_mem) = (cpu0.clone(), mem0.clone());
        if oracle(lifter, &mut cache, &mut want, &mut want_mem).is_none() {
            t.oracle_skipped += 1;
            continue;
        }
        let want_abi = comparable(abi(&want));
        let ranges = dirty(&mem0.bytes, &want_mem.bytes);
        let restore = |dst: &mut [u8]| {
            for r in &ranges {
                dst[r.clone()].copy_from_slice(&mem0.bytes[r.clone()]);
            }
        };

        // The sim tier. Run only after the interpreter finished: its memory
        // is unchecked, and the interpreter has proved every access lands
        // inside the 8 MiB.
        let mut mem = mem0.bytes.clone();
        let mut cpu = abi(&cpu0);
        let mut at = 0;
        // SAFETY: `mem` is the whole guest space from address 0.
        let rc = unsafe { call(&mut cpu, mem.as_mut_ptr(), &mut at) };
        if rc != 0 {
            *t.rare.entry(rc).or_default() += 1;
            continue;
        }
        let label = format!("{} case {n}", root.point.name);
        assert_eq!(comparable(cpu), want_abi, "{label}: CPU");
        if let Some(i) = (0..RAM).find(|&i| mem[i] != want_mem.bytes[i]) {
            panic!("{label}: memory differs first at {i:#x}");
        }
        t.tier_ok += 1;

        // The reference tier, on the production runtime.
        built.ctx.memory().write_bytes(0, &mem0.bytes, 1).unwrap();
        let mut rcpu = abi(&cpu0);
        let tr = Runner { max_transfers: 1_000_000 }.run(&mut rcpu, &mut built.ctx, entry);
        let reference_ok = tr.kind == runtime::abi::STOP
            && rcpu.eip == EXIT
            && comparable(rcpu) == want_abi
            && guest_ram(&mut built.ctx) == want_mem.bytes;
        if !reference_ok {
            eprintln!("{label}: the reference tier disagrees with the interpreter ({tr:?}); not timed");
        }
        t.reference_ok += u32::from(reference_ok);

        // The hand port, where it finishes the call without a transfer.
        let port_done = match root.port {
            Port::Native(f) => {
                let mut c = abi(&cpu0);
                let mut m = Flat(mem0.bytes.clone());
                native::supported(&c)
                    && f(&mut c, &mut m).is_ok_and(|x| x == TransferAbi::ret(EXIT))
                    && comparable(c) == want_abi
                    && m.0 == want_mem.bytes
            }
            Port::Physics(p) => {
                let mut c = abi(&cpu0);
                let mut g = Guest::new();
                g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
                g.write_bytes(0, &mem0.bytes, 1).unwrap();
                let at = g.mappings()[0].physical_start as usize;
                engine::sim_physics::supported(p, &c, &g)
                    && p.invoke(&mut c, &mut g, entry).is_ok_and(|x| x == TransferAbi::ret(EXIT))
                    && comparable(c) == want_abi
                    && g.physical_ram()[at..at + RAM] == want_mem.bytes[..]
            }
        };
        t.port_ok += u32::from(port_done);

        if !bench || !reference_ok {
            continue;
        }
        let tier_ns = {
            let mut buf = mem0.bytes.clone();
            let full = time(|| {
                restore(&mut buf);
                let mut c = abi(&cpu0);
                // SAFETY: as above.
                unsafe { call(&mut c, buf.as_mut_ptr(), std::ptr::null_mut()) };
            });
            let only = time(|| {
                restore(&mut buf);
                std::hint::black_box(abi(&cpu0));
            });
            full - only
        };
        let reference_ns = {
            let ctx = &mut built.ctx;
            let full = time(|| {
                {
                    let mut g = ctx.memory();
                    for r in &ranges {
                        g.write_bytes(r.start as u32, &mem0.bytes[r.clone()], 1).unwrap();
                    }
                }
                let mut c = abi(&cpu0);
                Runner { max_transfers: 1_000_000 }.run(&mut c, ctx, entry);
            });
            let only = time(|| {
                let mut g = ctx.memory();
                for r in &ranges {
                    g.write_bytes(r.start as u32, &mem0.bytes[r.clone()], 1).unwrap();
                }
            });
            full - only
        };
        let port_ns = port_done.then(|| match root.port {
            Port::Native(f) => {
                let mut m = Flat(mem0.bytes.clone());
                let full = time(|| {
                    restore(&mut m.0);
                    let mut c = abi(&cpu0);
                    let _ = f(&mut c, &mut m);
                });
                let only = time(|| restore(&mut m.0));
                full - only
            }
            Port::Physics(p) => {
                let mut g = Guest::new();
                g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
                g.write_bytes(0, &mem0.bytes, 1).unwrap();
                let back = |g: &mut Guest| {
                    for r in &ranges {
                        g.write_bytes(r.start as u32, &mem0.bytes[r.clone()], 1).unwrap();
                    }
                };
                let full = time(|| {
                    back(&mut g);
                    let mut c = abi(&cpu0);
                    let _ = p.invoke(&mut c, &mut g, entry);
                });
                let only = time(|| back(&mut g));
                full - only
            }
        });
        t.times.push((tier_ns, reference_ns, port_ns));
    }
    t
}

fn with_built(name: &str, f: impl FnOnce(&Image, &lift::Lifter<'_>, &mut Built, &[Root])) {
    let Some(img) = image() else { return };
    let xbe = xbe::Xbe::parse(&img.bytes).unwrap();
    let lifter = lift::Lifter::new(&img.analysis, &xbe).unwrap();
    let roots = roots();
    let entries: Vec<u32> = roots.iter().map(|r| r.point.base).collect();
    let mut built = build(img, &lifter, &entries, name);
    assert_eq!(built.tier.roots, entries, "every root compiles in the tier");
    f(img, &lifter, &mut built, &roots);
}

#[test]
fn tier_matches_the_interpreter() {
    with_built("sim_tier_differential", |img, lifter, built, roots| {
        for root in roots {
            let t = run(root, img, lifter, built, false);
            eprintln!("{}: {t:?}", root.point.name);
            assert!(t.tier_ok > 0, "{}: the tier completed no case: {t:?}", root.point.name);
        }
    });
}

fn mean(v: impl Iterator<Item = f64>) -> Option<f64> {
    let (s, n) = v.fold((0.0, 0u32), |(s, n), x| (s + x, n + 1));
    (n > 0).then(|| s / f64::from(n))
}

#[test]
#[ignore = "benchmark; run with --release --nocapture"]
fn tier_bench() {
    with_built("sim_tier_bench", |img, lifter, built, roots| {
        // The reference runner's cost around a function that does nothing.
        let empty_ns = {
            let ctx = &mut built.ctx;
            ctx.memory().write_bytes(STACK, &EXIT.to_le_bytes(), 1).unwrap();
            time(|| {
                let mut c = CpuAbi { esp: STACK, ..CpuAbi::default() };
                Runner { max_transfers: 10 }.run(&mut c, ctx, EMPTY);
            })
        };
        println!("reference runner around an empty function: {empty_ns:.1} ns");
        println!("| function | cases | tier ok | rare | timed | tier ns | reference ns | reference - empty ns | port ns | reference / tier | port / tier |");
        println!("|---|---|---|---|---|---|---|---|---|---|---|");
        for root in roots {
            let t = run(root, img, lifter, built, true);
            let tier = mean(t.times.iter().map(|x| x.0));
            let reference = mean(t.times.iter().map(|x| x.1));
            let port = mean(t.times.iter().filter_map(|x| x.2));
            let f = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:.1}"));
            let ratio = |a: Option<f64>, b: Option<f64>| match (a, b) {
                (Some(a), Some(b)) if b > 0.0 => format!("{:.2}x", a / b),
                _ => "-".into(),
            };
            println!(
                "| {} | {} | {} | {:?} | {} | {} | {} | {} | {} | {} | {} |",
                root.point.name,
                t.cases - t.oracle_skipped,
                t.tier_ok,
                t.rare,
                t.times.len(),
                f(tier),
                f(reference),
                f(reference.map(|r| r - empty_ns)),
                f(port),
                ratio(reference, tier),
                ratio(port, tier),
            );
        }
    });
}
