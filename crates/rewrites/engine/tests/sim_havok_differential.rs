//! The island solve `0x0031c100` (`sim_havok::island`, the port
//! `HAVOK_ISLAND_SOLVE`) and the contact builder `0x0031f620`
//! (`sim_havok::jacobian::contact`) against the lifted original
//! (docs/engine/havok-step.md, "The island solve").
//!
//! Seeded islands are laid out in the image's RAM. The lifted entry runs to
//! its return, stepping into every callee; a constraint's `vt[+0x24]` is a
//! synthetic address the harness answers with `ret 8`, so the frame runs
//! with no rows. The model's writes must leave memory as the lifted entry
//! does outside the dead stack, the solver buffer and the LIFO block
//! included (the next island reads what a solve leaves there), and `eax`
//! must match; the frame-stage test, which has no solve, skips the buffer. Islands the
//! model does not cover must be declined with nothing written. Without the
//! image the tests return.
#![cfg(feature = "differential")]

use engine::points;
use engine::sim_physics::{PHYSICS_PORTS, Port, supported};
use ir::interp::{Cpu, FlatMemory};
use lifted::Code;
use lifted::cpu::from_interp as abi_cpu;
use lifted::image::{Image, Rng};
use runtime::{Guest, guest::Permissions};
use sim_havok::island_solve::{self as island, Args, Body, Fallback, Island, Layout};
use sim_havok::jacobian::{self, ContactArgs, Cursor, Fpu, Scratch};
use sim_havok::mem::{Journal, Words};
use sim_havok::motion::{self, Accumulator, SolverClass};
use sim_havok::motion_solve::{self, Solved};

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = 0xffff_fffc;
/// A constraint builder the harness answers: `ret 8`, nothing written.
const BUILD_STUB: u32 = 0x0060_0f00;
const BUFFER_OBJECT: u32 = 0x0060_0000;
const BUFFER: u32 = 0x0062_0000;
const BUFFER_BYTES: u32 = 0x8000;
const WORLD: u32 = 0x0061_0000;
const ISLAND: u32 = 0x0061_1000;
const ENTITIES: u32 = 0x0061_2000;
const CONSTRAINTS: u32 = 0x0061_2100;
const ENTITY: u32 = 0x0061_4000;
const MOTION: u32 = 0x0061_8000;
const CONSTRAINT: u32 = 0x0061_c000;
const VTABLES: u32 = 0x0061_e000;
const MONITOR_HEAD: u32 = 0x0048_0154;
const MONITOR_END: u32 = 0x0048_0158;

struct Flat<'a>(&'a FlatMemory);

impl Words for Flat<'_> {
    fn get(&self, addr: u32) -> Option<u32> {
        self.0.read_u32(addr)
    }
}

fn word(m: &mut FlatMemory, a: u32, v: u32) {
    assert!(m.load(a, &v.to_le_bytes()));
}

fn ram() -> Option<(&'static Image, FlatMemory)> {
    let image = Image::get()?;
    let mut ram = image.ram.clone();
    ram.bytes.truncate(RAM);
    ram.bytes.resize(RAM, 0);
    Some((image, ram))
}

fn original(image: &Image) -> lifted::Original {
    let mut code = image.original();
    let motions = SolverClass::ALL.into_iter().flat_map(|c| <[u32; 2]>::from(c.slots()));
    let joints = sim_havok::joint::Joint::ALL.into_iter().map(|j| j.build_address());
    for f in [0x31c100, 0x31c940, 0x31bdf0, 0x31f620].into_iter().chain(motions).chain(joints) {
        code.prime(f);
    }
    code
}

/// Run the lifted original from `cpu.eip` until it returns to `EXIT`.
/// A call to `BUILD_STUB` is answered as `ret 8`.
fn run(code: &mut lifted::Original, cpu: &mut Cpu, mem: &mut FlatMemory) {
    for _ in 0..2_000_000 {
        if cpu.eip == EXIT {
            return;
        }
        if cpu.eip == BUILD_STUB {
            let sp = cpu.gpr[4];
            cpu.eip = mem.read_u32(sp).unwrap();
            cpu.gpr[4] = sp + 12;
            continue;
        }
        lifted::code::step(code, cpu, mem).unwrap_or_else(|e| panic!("{e}"));
    }
    panic!("the lifted entry did not return");
}

fn random_bytes(r: &mut Rng, m: &mut FlatMemory, at: u32, len: u32) {
    for i in (0..len).step_by(4) {
        word(m, at + i, r.u32());
    }
}

fn lanes(v: u128) -> [u32; 4] {
    std::array::from_fn(|i| (v >> (32 * i)) as u32)
}

fn vector(m: &FlatMemory, at: u32) -> u128 {
    (0..4).fold(0, |v, i| v | u128::from(m.read_u32(at + 4 * i).unwrap()) << (32 * i))
}

/// The bytes a builder leaves at `at`: kind, flag, vectors; the rest as
/// they were.
fn lay(m: &mut FlatMemory, at: u32, a: &Accumulator) {
    m.bytes[at as usize] = motion::ACCUMULATOR_KIND;
    if let Some(flag) = a.flag {
        m.bytes[(at + 0xc) as usize] = flag;
    }
    for (i, v) in a.vectors.iter().enumerate() {
        for (k, w) in lanes(*v).into_iter().enumerate() {
            word(m, at + 0x10 + 0x10 * i as u32 + 4 * k as u32, w);
        }
    }
}

#[test]
fn the_fixed_and_keyframed_accumulators_match_the_lifted_builders() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    for seed in 0..64u64 {
        let mut r = Rng(0xacc0_0000 + seed);
        let keyframed = seed & 1 == 1;
        let mut mem = base.clone();
        random_bytes(&mut r, &mut mem, MOTION, 0x110);
        random_bytes(&mut r, &mut mem, BUFFER, 0x100);
        let mut expected = mem.clone();
        let target = if keyframed { motion::KEYFRAMED_ACCUMULATE } else { motion::FIXED_ACCUMULATE };
        let a = if keyframed {
            motion::keyframed_accumulator(vector(&mem, MOTION + 0x40), vector(&mem, MOTION + 0x50), vector(&mem, MOTION + 0x70))
        } else {
            motion::fixed_accumulator()
        };
        lay(&mut expected, BUFFER + 0x80, &a);
        for (i, v) in [EXIT, WORLD + 0x150, WORLD + 0xe0, BUFFER + 0x80].into_iter().enumerate() {
            word(&mut mem, STACK + 4 * i as u32, v);
            word(&mut expected, STACK + 4 * i as u32, v);
        }
        let mut cpu = Cpu { gpr: [0, MOTION, 0, 0x1111, STACK, 0x2222, 0x3333, 0x4444], eip: target, ..Cpu::default() };
        run(&mut code, &mut cpu, &mut mem);
        let after = abi_cpu(&cpu);
        assert_eq!(after.eax, BUFFER + 0x100, "seed {seed}: the next accumulator");
        assert_eq!(after.esp, STACK + 16, "seed {seed}: ret 0xc");
        assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444));
        // The dead stack is not modelled.
        mem.bytes[(STACK - 0x100) as usize..STACK as usize].fill(0);
        expected.bytes[(STACK - 0x100) as usize..STACK as usize].fill(0);
        let first = mem.bytes.iter().zip(&expected.bytes).position(|(a, b)| a != b);
        assert_eq!(first, None, "seed {seed}: first different byte");
    }
}

/// A seeded island of `entities` fixed bodies and `constraints`
/// constraints whose builder is `build`. Entity `entities` is outside the
/// island (the level), with a stale accumulator offset.
fn island(r: &mut Rng, m: &mut FlatMemory, entities: u32, constraints: u32, build: u32, buffer: u32) -> Args {
    word(m, island::SOLVER_BUFFER, BUFFER_OBJECT);
    word(m, BUFFER_OBJECT + island::BUFFER_BASE, BUFFER);
    word(m, BUFFER_OBJECT + island::BUFFER_SIZE, buffer);
    word(m, BUFFER_OBJECT + island::BUFFER_BUSY, 0);
    m.bytes[island::TABLES_READY as usize] = 1;
    word(m, MONITOR_END, 0x0060_0800);
    word(m, MONITOR_HEAD, 0x0060_0800);
    for (off, v) in [(0x08u32, 0), (0x0c, 0), (0x10, 0), (0x14, 0x80 * entities), (0x18, 0)] {
        word(m, ISLAND + off, v);
    }
    let fixed = VTABLES;
    word(m, fixed + island::MOTION_ACCUMULATE_SLOT, motion::FIXED_ACCUMULATE);
    word(m, fixed + island::MOTION_EXPORT_SLOT, motion::FIXED_EXPORT);
    let builder = VTABLES + 0x100;
    word(m, builder + island::CONSTRAINT_BUILD_SLOT, build);
    for i in 0..=entities {
        let (entity, motion) = (ENTITY + 0x100 * i, MOTION + 0x200 * i);
        word(m, entity + island::ENTITY_MOTION, motion);
        word(m, motion, fixed);
        // Some offsets already hold the value the entry stores.
        let stale = if r.u32() % 3 == 0 { 0x80 * (i + 1) } else { 0x80 * r.below(entities + 2) };
        word(m, motion + island::MOTION_ACCUMULATOR, stale);
        if i < entities {
            word(m, ENTITIES + 4 * i, entity);
        }
    }
    for i in 0..constraints {
        let c = CONSTRAINT + 0x100 * i;
        word(m, CONSTRAINTS + 4 * i, c);
        word(m, c, builder);
        let a = r.below(entities);
        let b = r.below(entities + 1);
        word(m, c + island::CONSTRAINT_ENTITY_A, ENTITY + 0x100 * a);
        word(m, c + island::CONSTRAINT_ENTITY_B, ENTITY + 0x100 * b);
    }
    // The solver's parameters: no iterations, unit scales.
    for off in (0..0x40).step_by(4) {
        word(m, WORLD + 0x160 + off, 1.0f32.to_bits());
    }
    word(m, WORLD + 0x160 + 0x1c, 0);
    Args {
        step: WORLD + 0x150,
        info: WORLD + 0x160,
        pairs: WORLD + 0x190,
        gravity: WORLD + 0xe0,
        island: ISLAND,
        entities: ENTITIES,
        entity_count: entities,
        constraints: CONSTRAINTS,
        constraint_count: constraints,
    }
}

fn push_args(m: &mut FlatMemory, a: &Args) {
    let words = [EXIT, a.step, a.info, a.pairs, a.gravity, a.island, a.entities, a.entity_count, a.constraints, a.constraint_count];
    for (i, v) in words.into_iter().enumerate() {
        word(m, STACK + 4 * i as u32, v);
    }
}

#[test]
fn the_frame_writes_match_the_lifted_entry() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    for seed in 0..48u64 {
        let mut r = Rng(0x31c1_0000 + seed);
        let (entities, constraints) = (1 + r.below(4), 1 + r.below(3));
        let mut mem = base.clone();
        let args = island(&mut r, &mut mem, entities, constraints, BUILD_STUB, BUFFER_BYTES);
        push_args(&mut mem, &args);
        // The model's stages, from the same bytes.
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        assert_eq!(Args::read(&j, STACK), Some(args));
        let layout = Layout::read(&j, ISLAND, BUFFER, BUFFER_BYTES).unwrap();
        assert!(layout.fits());
        let model = Island {
            buffer: BUFFER_OBJECT,
            layout,
            bodies: (0..entities)
                .map(|i| Body { motion: MOTION + 0x200 * i, class: SolverClass::Fixed })
                .collect(),
            constraints: vec![],
        };
        island::place_accumulators(&mut j, &model).unwrap();
        for i in 0..constraints {
            island::pair(&mut j, &args, BUFFER, CONSTRAINT + 0x100 * i).unwrap();
        }
        let mut expected = before.clone();
        for (a, v) in j.finish() {
            word(&mut expected, a, v);
        }
        let mut cpu = Cpu { gpr: [0, 0, 0, 0x1111, STACK, 0x2222, 0x3333, 0x4444], eip: 0x31c100, ..Cpu::default() };
        run(&mut code, &mut cpu, &mut mem);
        let after = abi_cpu(&cpu);
        assert_eq!(after.eax, 0, "seed {seed}: no LIFO block, eax 0");
        assert_eq!(after.esp, STACK + 4, "seed {seed}: cdecl");
        assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444));
        // Scratch: the solver buffer and the dead stack.
        for m in [&mut mem, &mut expected] {
            m.bytes[BUFFER as usize..(BUFFER + BUFFER_BYTES) as usize].fill(0);
            m.bytes[(STACK - 0x1000) as usize..STACK as usize].fill(0);
        }
        let first = mem.bytes.iter().zip(&expected.bytes).position(|(a, b)| a != b);
        assert_eq!(first.map(|f| format!("{f:#x}")), None, "seed {seed}: first different byte");
    }
}

fn port() -> &'static Port {
    PHYSICS_PORTS.iter().find(|p| p.point.base == points::HAVOK_ISLAND_SOLVE.base).unwrap()
}

fn guest(m: &FlatMemory) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    g.write_bytes(0, &m.bytes, 1).unwrap();
    g
}

/// An unported builder and an island past the buffer whose LIFO block
/// would come through an uncovered `vt[+0x24]`: the port declines, writes
/// nothing and counts why.
#[test]
fn islands_the_model_does_not_cover_are_declined_and_counted() {
    let Some((_, base)) = ram() else { return };
    for (build, buffer, reason) in [
        (BUILD_STUB, BUFFER_BYTES, Fallback::Constraint),
        (sim_havok::jacobian::CONTACT_BUILD, 0x100, Fallback::Overflow),
    ] {
        let mut r = Rng(0xfa11_0000 + u64::from(buffer));
        let mut mem = base.clone();
        let args = island(&mut r, &mut mem, 3, 2, build, buffer);
        seed_lifo(&mut mem, 0);
        push_args(&mut mem, &args);
        let g = guest(&mem);
        let cpu = runtime::CpuAbi { esp: STACK, ..runtime::CpuAbi::default() };
        let index = Fallback::ALL.iter().position(|f| *f == reason).unwrap();
        let before = island::fallbacks().0[index].1;
        assert!(!supported(port(), &cpu, &g), "{reason:?}");
        assert!(island::fallbacks().0[index].1 > before, "{reason:?} counted");
        let start = g.mappings()[0].physical_start as usize;
        assert!(g.physical_ram()[start..start + RAM] == mem.bytes[..], "{reason:?}: nothing written");
    }
}

// ---- the contact builder `0x0031f620` and contact islands -----------------

const POINTS: u32 = 0x0061_5000;
const PROPS: u32 = 0x0061_6000;
const CURSOR: u32 = 0x0061_7000;

fn float(r: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + (r.u32() >> 8) as f32 / (1u32 << 24) as f32 * (hi - lo)
}

fn bits(r: &mut Rng, lo: f32, hi: f32) -> u32 {
    float(r, lo, hi).to_bits()
}

fn put_lanes(m: &mut FlatMemory, at: u32, lanes: [f32; 4]) {
    for (i, v) in lanes.into_iter().enumerate() {
        word(m, at + 4 * i as u32, v.to_bits());
    }
}

/// The float state of a seed: each MXCSR rounding mode, each x87
/// rounding mode at 53-bit precision.
fn fpu(seed: u64) -> Fpu {
    Fpu { mxcsr: 0x1f80 | ((seed % 4) as u32) << 13, control: 0x027f | (((seed / 4) % 4) as u16) << 10 }
}

/// One contact constraint at `c` with `n` points: the points, their
/// properties and the friction block, shaped to reach each branch of the
/// builder (parallel, cancelling and vanishing normals; a first tangent
/// along the normal; the twist radius flag; pairs of points).
fn contact(r: &mut Rng, m: &mut FlatMemory, c: u32, points: u32, props: u32, n: u32) {
    word(m, c + jacobian::CONTACT_POINTS, points);
    word(m, c + jacobian::CONTACT_COUNT, n);
    word(m, c + jacobian::CONTACT_PROPERTIES, props);
    let shape = r.below(5);
    let unit = {
        let v = [float(r, -1.0, 1.0), float(r, -1.0, 1.0), float(r, -1.0, 1.0)];
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-3);
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let no_impulse = r.below(8) == 0;
    let mut paired = false;
    for i in 0..n {
        let p = points + 0x20 * i;
        let pos = [float(r, -2.0, 2.0), float(r, -2.0, 2.0), float(r, -2.0, 2.0), float(r, -0.2, 0.1)];
        let normal = match shape {
            0 => [float(r, -1.0, 1.0), float(r, -1.0, 1.0), float(r, -1.0, 1.0), float(r, -0.2, 0.1)],
            1 => [unit[0], unit[1], unit[2], 0.0],
            2 => {
                let s = if i % 2 == 0 { 1.0 } else { -1.0 };
                [s * unit[0], s * unit[1], s * unit[2], 0.0]
            }
            3 => [unit[0] * 1e-3, unit[1] * 1e-3, unit[2] * 1e-3, 0.0],
            _ => [unit[0] + float(r, -0.05, 0.05), unit[1], unit[2], 0.0],
        };
        put_lanes(m, p, pos);
        put_lanes(m, p + 0x10, normal);
        let q = props + jacobian::PROPERTY_BYTES * i;
        let impulse = if no_impulse { 0.0 } else { float(r, 0.0, 3.0) };
        for (off, v) in [(0, impulse), (4, float(r, -0.5, 0.5)), (8, float(r, 0.0, 1.0)), (0x0c, float(r, -1.0, 1.0)), (0x10, float(r, -1.0, 1.0)), (0x14, float(r, -0.1, 0.0))] {
            word(m, q + off, v.to_bits());
        }
        let pair = i > 0 && !paired && r.below(3) == 0;
        paired = pair;
        word(m, q + 0x18, r.u32() & 0xffff_0000 | u32::from(pair));
    }
    let f = c + jacobian::CONTACT_FRICTION;
    let measure = if r.below(2) == 0 { jacobian::FRICTION_MEASURE } else { 0 };
    word(m, f, r.u32() & !jacobian::FRICTION_MEASURE | measure);
    for off in [4, 0x0c, 0x14] {
        word(m, f + off, bits(r, -1.0, 1.0));
    }
    for off in [8, 0x10, 0x18] {
        word(m, f + off, bits(r, -0.5, 0.5));
    }
    word(m, f + jacobian::FRICTION_RADIUS, bits(r, 0.0, 0.3));
    let u = match r.below(4) {
        0 => [unit[0], unit[1], unit[2], 0.0],
        1 => [0.0; 4],
        _ => [float(r, -1.0, 1.0), float(r, -1.0, 1.0), float(r, -1.0, 1.0), float(r, -1.0, 1.0)],
    };
    put_lanes(m, f + jacobian::FRICTION_U, u);
    put_lanes(m, f + jacobian::FRICTION_V, [float(r, -1.0, 1.0); 4]);
}

/// The pair words past the two accumulators: the right-hand-side scale,
/// the recovery cap, the pair coupling scale and the friction scale.
fn pair_scales(r: &mut Rng, m: &mut FlatMemory, pairs: u32) {
    for (off, (lo, hi)) in [(0x0c, (0.2, 1.0)), (0x10, (0.0, 0.05)), (0x18, (0.5, 1.0)), (0x1c, (0.0, 1.0))] {
        word(m, pairs + off, bits(r, lo, hi));
    }
}

fn run_cpu(code: &mut lifted::Original, mem: &mut FlatMemory, eip: u32, fpu: Fpu) -> runtime::CpuAbi {
    let mut cpu = Cpu { gpr: [0, 0, 0, 0x1111, STACK, 0x2222, 0x3333, 0x4444], eip, ..Cpu::default() };
    cpu.mxcsr = fpu.mxcsr;
    cpu.x87.control = fpu.control;
    run(code, &mut cpu, mem);
    abi_cpu(&cpu)
}

/// The first byte where `a` (the lifted run) and `b` (the model) differ,
/// with the aligned word on each side.
fn first_difference(a: &FlatMemory, b: &FlatMemory) -> Option<String> {
    a.bytes.iter().zip(&b.bytes).position(|(x, y)| x != y).map(|f| {
        let at = f as u32 & !3;
        format!("{f:#x} (word {at:#x}: lifted {:08x?}, model {:08x?})", a.read_u32(at), b.read_u32(at))
    })
}

/// Seeded accumulators, points, properties and friction blocks: the
/// model's rows, schemas, cursor and persistent words equal the lifted
/// builder's, word for word, under every rounding mode.
#[test]
fn the_contact_builder_matches_the_lifted_builder() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    let (mut built, mut declined) = (0, 0);
    for seed in 0..600u64 {
        let mut r = Rng(0x31f6_0000 + seed);
        let mut mem = base.clone();
        random_bytes(&mut r, &mut mem, BUFFER, 0x800);
        // Two accumulators after the leading one; sometimes one body twice.
        let (acc_a, acc_b) = match r.below(6) {
            0 => (BUFFER + 0x80, BUFFER + 0x80),
            1 => (BUFFER + 0x80, BUFFER),
            _ => (BUFFER + 0x80, BUFFER + 0x100),
        };
        for acc in [BUFFER, BUFFER + 0x80, BUFFER + 0x100] {
            mem.bytes[(acc + 0x0c) as usize] = if r.below(2) == 0 { 0 } else { r.u32() as u8 | 1 };
            put_lanes(&mut mem, acc + 0x30, [float(&mut r, 0.0, 2.0), float(&mut r, 0.0, 2.0), float(&mut r, 0.0, 2.0), float(&mut r, 0.0, 1.0)]);
            for v in [0x40, 0x50, 0x60, 0x70] {
                put_lanes(&mut mem, acc + v, [float(&mut r, -2.0, 2.0), float(&mut r, -1.0, 1.0), float(&mut r, -1.0, 1.0), float(&mut r, -1.0, 1.0)]);
            }
        }
        let n = if r.below(10) == 0 { 0 } else { 1 + r.below(6) };
        let c = CONSTRAINT;
        contact(&mut r, &mut mem, c, POINTS, PROPS, n);
        let pairs = WORLD + 0x190;
        word(&mut mem, pairs, acc_a);
        word(&mut mem, pairs + 4, acc_b);
        pair_scales(&mut r, &mut mem, pairs);
        let (rows, schemas) = (BUFFER + 0x1000, BUFFER + 0x4000);
        word(&mut mem, CURSOR, rows);
        word(&mut mem, CURSOR + 4, schemas);
        let f = c + jacobian::CONTACT_FRICTION;
        for (i, v) in [EXIT, POINTS, n, PROPS, f, pairs, CURSOR].into_iter().enumerate() {
            word(&mut mem, STACK + 4 * i as u32, v);
        }
        let fpu = fpu(seed);

        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let mut s = Scratch::new(BUFFER, BUFFER + BUFFER_BYTES).unwrap();
        for at in (BUFFER..BUFFER + 0x180).step_by(4) {
            s.put(at, before.read_u32(at).unwrap()).unwrap();
        }
        let mut cursor = Cursor { rows, schemas, limit: u32::MAX, overflow: 0 };
        let args = ContactArgs { points: POINTS, count: n, properties: PROPS, friction: f, pairs };
        if let Err(reason) = jacobian::contact(&mut j, &mut s, fpu, &args, &mut cursor) {
            assert!(matches!(reason, Fallback::Constraint | Fallback::Float), "seed {seed}: {reason:?}");
            declined += 1;
            continue;
        }
        built += 1;
        let mut expected = before.clone();
        for (a, v) in j.finish() {
            word(&mut expected, a, v);
        }
        let after = run_cpu(&mut code, &mut mem, 0x31f620, fpu);
        assert_eq!(after.esp, STACK + 4, "seed {seed}: cdecl");
        assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444));
        assert_eq!((mem.read_u32(CURSOR), mem.read_u32(CURSOR + 4)), (Some(cursor.rows), Some(cursor.schemas)), "seed {seed}: cursor");
        word(&mut expected, CURSOR, cursor.rows);
        word(&mut expected, CURSOR + 4, cursor.schemas);
        for (from, to) in [(rows, cursor.rows), (schemas, cursor.schemas)] {
            for at in (from..to).step_by(4) {
                assert_eq!(mem.read_u32(at), s.get(at), "seed {seed} (n {n}): buffer word {at:#x}");
            }
        }
        for m in [&mut mem, &mut expected] {
            m.bytes[BUFFER as usize..(BUFFER + BUFFER_BYTES) as usize].fill(0);
            m.bytes[(STACK - 0x1000) as usize..STACK as usize].fill(0);
        }
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} (n {n}): first different byte");
    }
    assert!(declined * 8 <= built, "the builder declined {declined} of {} seeds", built + declined);
}

/// Islands of fixed bodies joined by contact constraints, run whole: the
/// builder, the schema solve over one to three iterations and its
/// write-back pass, the exports and the busy byte. Memory outside the dead
/// stack, the solver buffer included, equals the lifted entry's.
#[test]
fn contact_islands_match_the_lifted_entry() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    let (mut solved, mut declined) = (0, 0);
    for seed in 0..160u64 {
        let mut r = Rng(0x31c9_0000 + seed);
        let (entities, constraints) = (1 + r.below(3), 1 + r.below(3));
        let mut mem = base.clone();
        random_bytes(&mut r, &mut mem, BUFFER, 0x400);
        let args = island(&mut r, &mut mem, entities, constraints, jacobian::CONTACT_BUILD, BUFFER_BYTES);
        let (mut rows, mut schemas, mut impulses, mut widest) = (0, 0, 0, 0);
        for i in 0..constraints {
            let n = r.below(5);
            contact(&mut r, &mut mem, CONSTRAINT + 0x100 * i, POINTS + 0x100 * i, PROPS + 0x100 * i, n);
            rows += 0x30 * n + 0x80;
            schemas += 0x30 + 4 * n;
            impulses += n + 3;
            widest = widest.max(0x30 * n + 0x80);
        }
        for (off, v) in [(0x08u32, widest), (0x0c, rows), (0x10, schemas), (0x18, impulses)] {
            word(&mut mem, ISLAND + off, v);
        }
        pair_scales(&mut r, &mut mem, args.pairs);
        let info = args.info;
        for (off, (lo, hi)) in [(4, (0.5, 1.5)), (8, (0.05, 1.0)), (0x0c, (0.1, 1.0)), (0x10, (0.5, 1.5))] {
            word(&mut mem, info + off, bits(&mut r, lo, hi));
        }
        word(&mut mem, info + 0x1c, r.below(4));
        push_args(&mut mem, &args);
        let fpu = fpu(seed);

        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let eax = match island::solve(&mut j, &args, fpu) {
            Ok(eax) => eax,
            Err(reason) => {
                assert!(!matches!(reason, Fallback::Motion | Fallback::Tables), "seed {seed}: {reason:?}");
                declined += 1;
                continue;
            }
        };
        solved += 1;
        let mut expected = before.clone();
        for (a, v) in j.finish() {
            word(&mut expected, a, v);
        }
        let after = run_cpu(&mut code, &mut mem, 0x31c100, fpu);
        assert_eq!(after.eax, eax, "seed {seed}: eax");
        assert_eq!(after.esp, STACK + 4, "seed {seed}: cdecl");
        assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444));
        for m in [&mut mem, &mut expected] {
            m.bytes[(STACK - 0x1000) as usize..STACK as usize].fill(0);
        }
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed}: first different byte");
    }
    // A constraint on the level whose stale offset points past the
    // island's accumulators declines; the rest solve.
    assert!(declined <= solved, "{declined} of {} islands declined", solved + declined);
}

/// The port answers a contact island between two bodies of the island.
#[test]
fn the_port_solves_a_contact_island() {
    let Some((_, base)) = ram() else { return };
    let mut mem = base.clone();
    let mut r = Rng(0xc0_47ac);
    let args = island(&mut r, &mut mem, 2, 1, jacobian::CONTACT_BUILD, BUFFER_BYTES);
    // Both sides in the island, so no stale offset is read.
    word(&mut mem, CONSTRAINT + island::CONSTRAINT_ENTITY_A, ENTITY);
    word(&mut mem, CONSTRAINT + island::CONSTRAINT_ENTITY_B, ENTITY + 0x100);
    contact(&mut r, &mut mem, CONSTRAINT, POINTS, PROPS, 3);
    for (off, v) in [(0x08u32, 0x110), (0x0c, 0x110), (0x10, 0x3c), (0x18, 6)] {
        word(&mut mem, ISLAND + off, v);
    }
    pair_scales(&mut r, &mut mem, args.pairs);
    word(&mut mem, args.info + 0x1c, 2);
    push_args(&mut mem, &args);
    let g = guest(&mem);
    let mut cpu = runtime::CpuAbi { esp: STACK, mxcsr: 0x1f80, ..runtime::CpuAbi::default() };
    cpu.x87.control = 0x027f;
    assert!(supported(port(), &cpu, &g));
}

// ---- the motion virtuals `vt[+0x0c]` and `vt[+0x10]` ------------------------

const STEP: u32 = WORLD + 0x150;
const GRAVITY: u32 = WORLD + 0xe0;

/// A motion of `class` at `at`: every field the virtuals read, in ranges
/// a body holds, some damping past `1 / dt` (the clamp), the box torque
/// pending or not, the rest of the object random.
fn seed_motion(r: &mut Rng, m: &mut FlatMemory, at: u32) {
    use motion_solve::field::*;
    for off in (0..0x120).step_by(4) {
        word(m, at + off, bits(r, -2.0, 2.0));
    }
    word(m, at + SWEEP, bits(r, 0.0, 1.0));
    word(m, at + INVERSE_INERTIA, bits(r, 0.0, 2.0));
    for off in [LINEAR_DAMPING, ANGULAR_DAMPING] {
        let d = if r.below(4) == 0 { float(r, 20.0, 60.0) } else { float(r, 0.0, 0.5) };
        word(m, at + off, d.to_bits());
    }
    word(m, at + INVERSE_MASS, bits(r, 0.0, 2.0));
    let q = [float(r, -1.0, 1.0), float(r, -1.0, 1.0), float(r, -1.0, 1.0), float(r, -1.0, 1.0)];
    let l = (q.iter().map(|v| v * v).sum::<f32>()).sqrt().max(1e-3);
    put_lanes(m, at + ORIENTATION, q.map(|v| v / l));
    for off in [LINEAR, ANGULAR] {
        put_lanes(m, at + off, [float(r, -3.0, 3.0), float(r, -3.0, 3.0), float(r, -3.0, 3.0), float(r, -0.1, 0.1)]);
    }
    put_lanes(m, at + CENTER, [float(r, -10.0, 10.0), float(r, -10.0, 10.0), float(r, -10.0, 10.0), 1.0]);
    for off in [SLOT_C0, SLOT_D0, SLOT_F0] {
        put_lanes(m, at + off, [float(r, -5.0, 5.0), float(r, -5.0, 5.0), float(r, -5.0, 5.0), float(r, -1.0, 1.0)]);
    }
    put_lanes(m, at + SLOT_E0, [float(r, 0.0, 2.0), float(r, 0.0, 2.0), float(r, 0.0, 2.0), float(r, 0.0, 2.0)]);
    let pending = if r.below(2) == 0 { 0.0 } else { 1.0 };
    put_lanes(m, at + BOX_TORQUE, [float(r, -5.0, 5.0), float(r, -5.0, 5.0), float(r, -5.0, 5.0), pending]);
}

fn seed_world(r: &mut Rng, m: &mut FlatMemory) {
    let dt = [1.0 / 30.0, 1.0 / 60.0, float(r, 0.001, 0.1)][r.below(3) as usize];
    word(m, STEP, dt.to_bits());
    word(m, STEP + 4, (1.0 / dt).to_bits());
    put_lanes(m, GRAVITY, [float(r, -1.0, 1.0), float(r, -10.0, -5.0), float(r, -1.0, 1.0), 0.0]);
}

/// Run a `thiscall` virtual with three stack arguments.
fn run_virtual(code: &mut lifted::Original, mem: &mut FlatMemory, eip: u32, this: u32, args: [u32; 3], fpu: Fpu) -> runtime::CpuAbi {
    for (i, v) in [EXIT, args[0], args[1], args[2]].into_iter().enumerate() {
        word(mem, STACK + 4 * i as u32, v);
    }
    let mut cpu = Cpu { gpr: [0, this, 0, 0x1111, STACK, 0x2222, 0x3333, 0x4444], eip, ..Cpu::default() };
    cpu.mxcsr = fpu.mxcsr;
    cpu.x87.control = fpu.control;
    run(code, &mut cpu, mem);
    abi_cpu(&cpu)
}

fn check_virtual(after: &runtime::CpuAbi, seed: u64, class: SolverClass) {
    assert_eq!(after.eax, BUFFER + 0x100, "seed {seed} {class:?}: the next accumulator");
    assert_eq!(after.esp, STACK + 16, "seed {seed} {class:?}: ret 0xc");
    assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444));
}

fn clear_dead_stack(ms: [&mut FlatMemory; 2]) {
    for m in ms {
        m.bytes[(STACK - 0x1000) as usize..STACK as usize].fill(0);
    }
}

/// Every class's `vt[+0x0c]`: the motion's force integration and the
/// accumulator equal the lifted builder's, byte for byte, under every
/// rounding mode.
#[test]
fn the_motion_accumulators_match_the_lifted_builders() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    for seed in 0..384u64 {
        let class = SolverClass::ALL[seed as usize % 6];
        let mut r = Rng(0x0acc_0000 + seed);
        let mut mem = base.clone();
        seed_motion(&mut r, &mut mem, MOTION);
        seed_world(&mut r, &mut mem);
        random_bytes(&mut r, &mut mem, BUFFER, 0x200);
        let fpu = fpu(seed / 6);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let a = motion_solve::accumulate(&mut j, class, MOTION, STEP, GRAVITY, fpu)
            .unwrap_or_else(|f| panic!("seed {seed} {class:?}: declined {f:?}"));
        let mut expected = before.clone();
        for (at, v) in j.finish() {
            word(&mut expected, at, v);
        }
        lay(&mut expected, BUFFER + 0x80, &a);
        let after = run_virtual(&mut code, &mut mem, class.slots().0, MOTION, [STEP, GRAVITY, BUFFER + 0x80], fpu);
        check_virtual(&after, seed, class);
        for (i, v) in [EXIT, STEP, GRAVITY, BUFFER + 0x80].into_iter().enumerate() {
            word(&mut expected, STACK + 4 * i as u32, v);
        }
        clear_dead_stack([&mut mem, &mut expected]);
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} {class:?}: first different byte");
    }
}

/// Every class's `vt[+0x10]`: the velocities from the solved accumulator
/// and the pose integration equal the lifted export's, byte for byte,
/// under every rounding mode.
#[test]
fn the_motion_exports_match_the_lifted_exports() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    for seed in 0..384u64 {
        let class = SolverClass::ALL[seed as usize % 6];
        let mut r = Rng(0x0e8b_0000 + seed);
        let mut mem = base.clone();
        seed_motion(&mut r, &mut mem, MOTION);
        seed_world(&mut r, &mut mem);
        random_bytes(&mut r, &mut mem, BUFFER, 0x200);
        for off in [0x10, 0x20, 0x40, 0x50] {
            put_lanes(&mut mem, BUFFER + 0x80 + off, [float(&mut r, -3.0, 3.0), float(&mut r, -3.0, 3.0), float(&mut r, -3.0, 3.0), float(&mut r, -0.1, 0.1)]);
        }
        let scale = bits(&mut r, 0.5, 1.5);
        let fpu = fpu(seed / 6);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let solved = Solved::read(|at| before.read_u32(at), BUFFER + 0x80).unwrap();
        motion_solve::export(&mut j, class, MOTION, STEP, scale, &solved, fpu)
            .unwrap_or_else(|f| panic!("seed {seed} {class:?}: declined {f:?}"));
        let mut expected = before.clone();
        for (at, v) in j.finish() {
            word(&mut expected, at, v);
        }
        let after = run_virtual(&mut code, &mut mem, class.slots().1, MOTION, [STEP, scale, BUFFER + 0x80], fpu);
        check_virtual(&after, seed, class);
        for (i, v) in [EXIT, STEP, scale, BUFFER + 0x80].into_iter().enumerate() {
            word(&mut expected, STACK + 4 * i as u32, v);
        }
        clear_dead_stack([&mut mem, &mut expected]);
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} {class:?}: first different byte");
    }
}

/// Bodies of every motion class joined by contacts, the island's sizes
/// their bounds, random solver parameters.
fn contact_island(r: &mut Rng, mem: &mut FlatMemory, entities: u32, constraints: u32) -> Args {
    random_bytes(r, mem, BUFFER, 0x400);
    let args = island(r, mem, entities, constraints, jacobian::CONTACT_BUILD, BUFFER_BYTES);
    seed_world(r, mem);
    for (k, class) in SolverClass::ALL.into_iter().enumerate() {
        let vt = VTABLES + 0x200 + 0x40 * k as u32;
        let (accumulate, export) = class.slots();
        word(mem, vt + island::MOTION_ACCUMULATE_SLOT, accumulate);
        word(mem, vt + island::MOTION_EXPORT_SLOT, export);
    }
    for i in 0..entities {
        let at = MOTION + 0x200 * i;
        let offset = mem.read_u32(at + island::MOTION_ACCUMULATOR).unwrap();
        seed_motion(r, mem, at);
        word(mem, at, VTABLES + 0x200 + 0x40 * r.below(6));
        word(mem, at + island::MOTION_ACCUMULATOR, offset);
    }
    let (mut rows, mut schemas, mut impulses, mut widest) = (0, 0, 0, 0);
    for i in 0..constraints {
        let n = r.below(5);
        contact(r, mem, CONSTRAINT + 0x100 * i, POINTS + 0x100 * i, PROPS + 0x100 * i, n);
        rows += 0x30 * n + 0x80;
        schemas += 0x30 + 4 * n;
        impulses += n + 3;
        widest = widest.max(0x30 * n + 0x80);
    }
    for (off, v) in [(0x08u32, widest), (0x0c, rows), (0x10, schemas), (0x18, impulses)] {
        word(mem, ISLAND + off, v);
    }
    pair_scales(r, mem, args.pairs);
    let info = args.info;
    for (off, (lo, hi)) in [(4, (0.5, 1.5)), (8, (0.05, 1.0)), (0x0c, (0.1, 1.0)), (0x10, (0.5, 1.5)), (0x20, (0.5, 1.5))] {
        word(mem, info + off, bits(r, lo, hi));
    }
    word(mem, info + 0x1c, r.below(4));
    args
}

/// Islands of bodies of every motion class joined by contacts, run whole:
/// the force integration, the builder, the solve, the write-back and the
/// exports. Memory outside the dead stack, the solver buffer included, equals the
/// lifted entry's, and no island declines for its motions.
#[test]
fn dynamic_contact_islands_match_the_lifted_entry() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    let (mut solved, mut declined) = (0, 0);
    for seed in 0..192u64 {
        let mut r = Rng(0xd1a0_0000 + seed);
        let (entities, constraints) = (1 + r.below(3), 1 + r.below(3));
        let mut mem = base.clone();
        let args = contact_island(&mut r, &mut mem, entities, constraints);
        push_args(&mut mem, &args);
        let fpu = fpu(seed);

        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let eax = match island::solve(&mut j, &args, fpu) {
            Ok(eax) => eax,
            Err(reason) => {
                assert!(!matches!(reason, Fallback::Motion | Fallback::Tables), "seed {seed}: {reason:?}");
                declined += 1;
                continue;
            }
        };
        solved += 1;
        let mut expected = before.clone();
        for (a, v) in j.finish() {
            word(&mut expected, a, v);
        }
        let after = run_cpu(&mut code, &mut mem, 0x31c100, fpu);
        assert_eq!(after.eax, eax, "seed {seed}: eax");
        assert_eq!(after.esp, STACK + 4, "seed {seed}: cdecl");
        for m in [&mut mem, &mut expected] {
            m.bytes[(STACK - 0x1000) as usize..STACK as usize].fill(0);
        }
        let classes: Vec<_> = (0..entities)
            .map(|i| (before.read_u32(MOTION + 0x200 * i).unwrap() - VTABLES - 0x200) / 0x40)
            .map(|k| SolverClass::ALL[k as usize])
            .collect();
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} {classes:?} {fpu:x?}: first different byte");
    }
    assert!(declined <= solved, "{declined} of {} islands declined", solved + declined);
}

/// The memory system's LIFO descriptor and the block it hands out.
const LIFO_DESCRIPTOR: u32 = 0x0060_1000;
const LIFO_BLOCK: u32 = 0x0064_0000;
const LIFO_BYTES: u32 = 0x1_0000;

/// `[0x00480118]` with `remaining` inline bytes at [`LIFO_BLOCK`] in a
/// chunk whose payload lies before it, and no vtable.
fn seed_lifo(m: &mut FlatMemory, remaining: u32) {
    use sim_havok::layout as l;
    word(m, l::MEMORY_SYSTEM, LIFO_DESCRIPTOR);
    word(m, LIFO_DESCRIPTOR, 0);
    word(m, LIFO_DESCRIPTOR + l::POOL_LIFO_CURSOR, LIFO_BLOCK);
    word(m, LIFO_DESCRIPTOR + l::POOL_LIFO_REMAINING, remaining);
    word(m, LIFO_DESCRIPTOR + l::POOL_LIFO_BLOCK, LIFO_BLOCK - 0x110);
    word(m, LIFO_DESCRIPTOR + l::POOL_LIFO_PAYLOAD, LIFO_BLOCK - 0x100);
}

/// Contact islands past the solver buffer's end, run whole: the rows
/// starting past it (every area in the block), only the impulses and
/// schemas past it, and the rows' tail past it (the cursor moving to the
/// block). The block is taken and given back inline; memory outside the
/// dead stack, the buffer and the block included, equals the lifted
/// entry's, and
/// `eax` is the block.
#[test]
fn islands_past_the_buffer_spill_into_a_lifo_block_as_the_lifted_entry_does() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    spill_islands(&mut code, &base, 0x5b11_0000, false);
}

// The class pool's builder and the directory pools, as the allocator's
// differential (`sim_physics_differential.rs`) lays them out.
const HEAP_DIR: u32 = 0x0020_0000;
const HEAP_CLASSES: u32 = 0x0021_5040;
const HEAP_BORROWED: u32 = 0x0040_0000;

fn directory_pool(m: &mut FlatMemory, at: u32) {
    for (off, v) in [(0, 0x4576a4), (4, 1), (8, 0), (0xc, 0), (0x10, 0), (0x14, u32::MAX), (0x18, at + 0x30), (0x1c, 0x15000), (0x20, 0)] {
        word(m, at + off, v);
    }
}

/// The memory system with its pools built by the lifted `0x0022c3e0`, in
/// `mode` (1: the directory pool alone; else the class pool first), and
/// an empty LIFO descriptor, so a block is pushed and popped through
/// `0x002d7020` and `0x002d7090`.
fn heap(code: &mut lifted::Original, m: &mut FlatMemory, mode: u32) {
    use sim_havok::layout as l;
    directory_pool(m, HEAP_DIR);
    directory_pool(m, HEAP_BORROWED);
    let mut cpu = Cpu { gpr: [0, 0, 0, 0, STACK, 0, HEAP_CLASSES, 0], eip: 0x22c3e0, eflags: 0xa93, ..Cpu::default() };
    word(m, STACK, EXIT);
    run(code, &mut cpu, m);
    for (a, v) in [(l::POOL_MODE, mode), (l::DIRECTORY_POOL, HEAP_DIR), (l::CLASS_POOL, HEAP_CLASSES), (l::BORROWED_POOL, HEAP_BORROWED), (l::MEMORY_SYSTEM, l::DISPATCH_OBJECT)] {
        word(m, a, v);
    }
    // The dispatcher's vtable: allocate `+0x10`, free `+0x14`, the LIFO
    // push `+0x24` and pop `+0x28`.
    word(m, l::DISPATCH_OBJECT, 0x0045_378c);
    for off in [l::POOL_LIFO_CURSOR, l::POOL_LIFO_REMAINING, l::POOL_LIFO_BLOCK, l::POOL_LIFO_PAYLOAD] {
        word(m, l::DISPATCH_OBJECT + off, 0);
    }
}

/// Spilled islands whose block comes from the allocator: the descriptor
/// has no inline bytes, so the entry calls `vt[+0x24]` for a new chunk and,
/// the block being that chunk's payload, `vt[+0x28]` to give it back, in
/// both pool modes.
#[test]
fn spilled_islands_take_their_block_through_the_allocator_as_the_lifted_entry_does() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    for f in [0x2d7020, 0x2d7090, 0x1472c0, 0x147320, 0x22c3e0] {
        code.prime(f);
    }
    for mode in [1, 2] {
        let mut heaped = base.clone();
        heap(&mut code, &mut heaped, mode);
        spill_islands(&mut code, &heaped, 0x5b12_0000 + 0x1000 * u64::from(mode), true);
    }
}

/// Seeded contact islands past the buffer in each layout, model against
/// the lifted entry. `from_heap`: the block comes from the built pools;
/// else inline from [`seed_lifo`]'s descriptor.
fn spill_islands(code: &mut lifted::Original, base: &FlatMemory, salt: u64, from_heap: bool) {
    let (mut solved, mut declined, mut why) = ([0; 3], 0, Vec::new());
    for seed in 0..96u64 {
        let mut r = Rng(salt + seed);
        let (entities, constraints) = (1 + r.below(3), 1 + r.below(3));
        let mut mem = base.clone();
        let args = contact_island(&mut r, &mut mem, entities, constraints);
        if !from_heap {
            random_bytes(&mut r, &mut mem, LIFO_BLOCK, 0x400);
            seed_lifo(&mut mem, LIFO_BYTES);
        }
        let at = |off: u32| mem.read_u32(ISLAND + off).unwrap();
        let rows = 0x80 * entities + 0x90;
        let impulses = rows + at(island::ISLAND_ROWS);
        let end = impulses + 4 * at(island::ISLAND_IMPULSES) + 8 + at(island::ISLAND_SCHEMAS) + 4;
        let case = (seed % 3) as usize;
        let size = match case {
            0 => r.below(rows + 1),
            1 => impulses + 1 + r.below(end - impulses - 1),
            _ => rows + 1 + r.below(impulses - rows),
        };
        word(&mut mem, BUFFER_OBJECT + island::BUFFER_SIZE, size);
        push_args(&mut mem, &args);
        let fpu = fpu(seed);

        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let eax = match island::solve(&mut j, &args, fpu) {
            Ok(eax) => eax,
            Err(reason) => {
                assert!(
                    !matches!(reason, Fallback::Motion | Fallback::Tables | Fallback::Overflow),
                    "seed {seed} case {case}: {reason:?}"
                );
                why.push((case, reason));
                declined += 1;
                continue;
            }
        };
        solved[case] += 1;
        if !from_heap {
            // The inline release leaves the descriptor's remaining bytes.
            assert_eq!(eax, LIFO_BYTES, "seed {seed} case {case}: eax");
        }
        let mut expected = before.clone();
        for (a, v) in j.finish() {
            word(&mut expected, a, v);
        }
        let after = run_cpu(code, &mut mem, 0x31c100, fpu);
        if !from_heap {
            assert_eq!(after.eax, eax, "seed {seed} case {case}: eax");
        }
        assert_eq!(after.esp, STACK + 4, "seed {seed}: cdecl");
        // The buffer and the block hold what the lifted solve left there:
        // compared with the rest.
        clear_dead_stack([&mut mem, &mut expected]);
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} case {case}: first different byte");
    }
    assert!(solved.iter().all(|&n| n > 0), "solved by case {solved:?}, declined {why:?}");
    assert!(declined <= solved.iter().sum(), "{declined} declined, solved {solved:?}");
}

// ---- the joint builders `vt[+0x24]` ---------------------------------------

use sim_havok::joint::Joint;

/// A joint at `c` between entities A and B: random pivots and axes in the
/// bodies' frames, limits, friction (zero or not, to reach each branch), a
/// last angle within a turn or two, and each body's transform
/// (`motion+0x80`).
fn seed_joint(r: &mut Rng, m: &mut FlatMemory, c: u32) {
    word(m, c + island::CONSTRAINT_ENTITY_A, ENTITY);
    word(m, c + island::CONSTRAINT_ENTITY_B, ENTITY + 0x100);
    for off in (0x20..0xf0).step_by(4) {
        word(m, c + off, bits(r, -1.0, 1.0));
    }
    word(m, c + 0x40, bits(r, 0.0, 2.0));
    // Friction: limited hinge `+0xd0`, ragdoll `+0x94`, prismatic `+0xa8`.
    for off in [0xd0, 0x94, 0xa8] {
        let f = if r.below(3) == 0 { 0.0 } else { float(r, -0.2, 1.0) };
        word(m, c + off, f.to_bits());
    }
    word(m, c + 0xd4, bits(r, -9.0, 9.0));
    // Sometimes the ragdoll's twist axes coincide, so a cone is skipped.
    if r.below(4) == 0 {
        for i in 0..4 {
            let v = m.read_u32(c + 0x40 + 4 * i).unwrap();
            word(m, c + 0x70 + 4 * i, v);
        }
    }
    for i in 0..2 {
        let (entity, motion) = (ENTITY + 0x100 * i, MOTION + 0x200 * i);
        word(m, entity + island::ENTITY_MOTION, motion);
        for off in (0x80..0xc0).step_by(4) {
            word(m, motion + off, bits(r, -1.0, 1.0));
        }
        put_lanes(m, motion + 0xb0, [float(r, -5.0, 5.0), float(r, -5.0, 5.0), float(r, -5.0, 5.0), 1.0]);
    }
}

/// Each joint's `vt[+0x24]`: the header, rows, schemas and cursor equal
/// the lifted builder's, word for word, under every rounding mode, and
/// nothing outside the buffer differs.
#[test]
fn the_joint_builders_match_the_lifted_builders() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    for seed in 0..768u64 {
        let joint = Joint::ALL[seed as usize % Joint::ALL.len()];
        let mut r = Rng(0x101e_0000 + seed);
        let mut mem = base.clone();
        random_bytes(&mut r, &mut mem, BUFFER, 0x800);
        let (acc_a, acc_b) = match r.below(6) {
            0 => (BUFFER + 0x80, BUFFER),
            _ => (BUFFER + 0x80, BUFFER + 0x100),
        };
        for acc in [BUFFER, BUFFER + 0x80, BUFFER + 0x100] {
            mem.bytes[(acc + 0x0c) as usize] = if r.below(2) == 0 { 0 } else { r.u32() as u8 | 1 };
            put_lanes(&mut mem, acc + 0x30, [float(&mut r, 0.0, 2.0), float(&mut r, 0.0, 2.0), float(&mut r, 0.0, 2.0), float(&mut r, 0.0, 1.0)]);
            for v in [0x40, 0x50, 0x60, 0x70] {
                put_lanes(&mut mem, acc + v, [float(&mut r, -2.0, 2.0), float(&mut r, -1.0, 1.0), float(&mut r, -1.0, 1.0), float(&mut r, -1.0, 1.0)]);
            }
        }
        let c = CONSTRAINT;
        seed_joint(&mut r, &mut mem, c);
        let pairs = WORLD + 0x190;
        word(&mut mem, pairs, acc_a);
        word(&mut mem, pairs + 4, acc_b);
        pair_scales(&mut r, &mut mem, pairs);
        word(&mut mem, pairs + 8, bits(&mut r, 0.0, 1.0));
        let (rows, schemas) = (BUFFER + 0x1000, BUFFER + 0x4000);
        word(&mut mem, CURSOR, rows);
        word(&mut mem, CURSOR + 4, schemas);
        let fpu = fpu(seed / Joint::ALL.len() as u64);

        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let mut s = Scratch::new(BUFFER, BUFFER + BUFFER_BYTES).unwrap();
        for at in (BUFFER..BUFFER + 0x180).step_by(4) {
            s.put(at, before.read_u32(at).unwrap()).unwrap();
        }
        let mut cursor = Cursor { rows, schemas, limit: u32::MAX, overflow: 0 };
        sim_havok::joint::build(&mut j, &mut s, fpu, joint, c, pairs, &mut cursor)
            .unwrap_or_else(|f| panic!("seed {seed} {joint:?}: declined {f:?}"));
        let mut expected = before.clone();
        for (a, v) in j.finish() {
            word(&mut expected, a, v);
        }
        for (i, v) in [EXIT, pairs, CURSOR].into_iter().enumerate() {
            word(&mut mem, STACK + 4 * i as u32, v);
            word(&mut expected, STACK + 4 * i as u32, v);
        }
        let mut cpu = Cpu { gpr: [0, c, 0, 0x1111, STACK, 0x2222, 0x3333, 0x4444], eip: joint.build_address(), ..Cpu::default() };
        cpu.mxcsr = fpu.mxcsr;
        cpu.x87.control = fpu.control;
        run(&mut code, &mut cpu, &mut mem);
        let after = abi_cpu(&cpu);
        assert_eq!(after.esp, STACK + 12, "seed {seed} {joint:?}: ret 8");
        assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444));
        assert_eq!((mem.read_u32(CURSOR), mem.read_u32(CURSOR + 4)), (Some(cursor.rows), Some(cursor.schemas)), "seed {seed} {joint:?}: cursor");
        for (from, to) in [(rows, cursor.rows), (schemas, cursor.schemas)] {
            for at in (from..to).step_by(4) {
                assert_eq!(mem.read_u32(at), s.get(at), "seed {seed} {joint:?}: buffer word {at:#x}");
            }
        }
        for m in [&mut mem, &mut expected] {
            m.bytes[BUFFER as usize..(BUFFER + BUFFER_BYTES) as usize].fill(0);
            m.bytes[(STACK - 0x1000) as usize..STACK as usize].fill(0);
        }
        word(&mut expected, CURSOR, cursor.rows);
        word(&mut expected, CURSOR + 4, cursor.schemas);
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} {joint:?}: first different byte");
    }
}

/// Islands of dynamic bodies joined by joints and contacts, run whole:
/// the solve over the joints' free rows, the write-back to each joint's
/// outputs and the exports equal the lifted entry's.
#[test]
fn joint_islands_match_the_lifted_entry() {
    let Some((image, base)) = ram() else { return };
    let mut code = original(image);
    let (mut solved, mut declined) = (0, 0);
    for seed in 0..384u64 {
        let mut r = Rng(0x101f_0000 + seed);
        let joint = Joint::ALL[seed as usize % Joint::ALL.len()];
        let mut mem = base.clone();
        random_bytes(&mut r, &mut mem, BUFFER, 0x400);
        let args = island(&mut r, &mut mem, 2, 1, joint.build_address(), BUFFER_BYTES);
        seed_world(&mut r, &mut mem);
        for (k, class) in SolverClass::ALL.into_iter().enumerate() {
            let vt = VTABLES + 0x200 + 0x40 * k as u32;
            let (accumulate, export) = class.slots();
            word(&mut mem, vt + island::MOTION_ACCUMULATE_SLOT, accumulate);
            word(&mut mem, vt + island::MOTION_EXPORT_SLOT, export);
        }
        for i in 0..2 {
            let at = MOTION + 0x200 * i;
            let offset = mem.read_u32(at + island::MOTION_ACCUMULATOR).unwrap();
            seed_motion(&mut r, &mut mem, at);
            word(&mut mem, at, VTABLES + 0x200 + 0x40 * r.below(6));
            word(&mut mem, at + island::MOTION_ACCUMULATOR, offset);
        }
        let builder = mem.read_u32(CONSTRAINT).unwrap();
        seed_joint(&mut r, &mut mem, CONSTRAINT);
        word(&mut mem, CONSTRAINT, builder);
        // Room for the largest joint (prismatic and ragdoll: 0x150 bytes,
        // nine rows, 0x5c bytes of schemas).
        for (off, v) in [(0x08u32, 0x180), (0x0c, 0x180), (0x10, 0x80), (0x18, 12)] {
            word(&mut mem, ISLAND + off, v);
        }
        pair_scales(&mut r, &mut mem, args.pairs);
        word(&mut mem, args.pairs + 8, bits(&mut r, 0.0, 1.0));
        let info = args.info;
        for (off, (lo, hi)) in [(4, (0.5, 1.5)), (8, (0.05, 1.0)), (0x0c, (0.1, 1.0)), (0x10, (0.5, 1.5)), (0x20, (0.5, 1.5))] {
            word(&mut mem, info + off, bits(&mut r, lo, hi));
        }
        word(&mut mem, info + 0x1c, 1 + r.below(3));
        push_args(&mut mem, &args);
        let fpu = fpu(seed / Joint::ALL.len() as u64);

        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let eax = match island::solve(&mut j, &args, fpu) {
            Ok(eax) => eax,
            Err(reason) => {
                assert!(!matches!(reason, Fallback::Motion | Fallback::Constraint | Fallback::Tables), "seed {seed} {joint:?}: {reason:?}");
                declined += 1;
                continue;
            }
        };
        solved += 1;
        let mut expected = before.clone();
        for (a, v) in j.finish() {
            word(&mut expected, a, v);
        }
        let after = run_cpu(&mut code, &mut mem, 0x31c100, fpu);
        assert_eq!(after.eax, eax, "seed {seed}: eax");
        for m in [&mut mem, &mut expected] {
            m.bytes[(STACK - 0x1000) as usize..STACK as usize].fill(0);
        }
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} {joint:?}: first different byte");
    }
    assert!(declined <= solved, "{declined} of {} islands declined", solved + declined);
}

// ---- the island integrate `0x00316a20` -------------------------------------

use engine::sim_physics::island_integrate as arm;
use engine::sim_physics::parity::{self, Step};
use sim_havok::island_integrate::{self as integrate, Fallback as Decline};
use sim_havok::motion_solve::FreeClass;

const INTEGRATE: u32 = 0x0031_6a20;
/// The world's step input: `{dt, 1/dt}` at `+0x10`, the solver's
/// parameters at `+0x20`, the pair words at `+0x50`.
const INPUT: u32 = WORLD + 0x140;

fn integrate_code(image: &Image) -> lifted::Original {
    let mut code = original(image);
    let free = FreeClass::ALL.into_iter().flat_map(|c| <[u32; 2]>::from(c.slots()));
    for f in std::iter::once(INTEGRATE).chain(free) {
        code.prime(f);
    }
    code
}

/// The unconstrained virtuals of a solver class's motion type.
fn free_class(class: SolverClass) -> FreeClass {
    match class {
        SolverClass::Sphere | SolverClass::StabilizedSphere => FreeClass::Sphere,
        SolverClass::Box | SolverClass::StabilizedBox => FreeClass::Box,
        SolverClass::Keyframed => FreeClass::Keyframed,
        SolverClass::Fixed => FreeClass::Fixed,
    }
}

/// Run a `thiscall` on `this` with `args` on the stack, under `fpu`.
fn run_this(code: &mut lifted::Original, mem: &mut FlatMemory, eip: u32, this: u32, args: &[u32], fpu: Fpu) -> runtime::CpuAbi {
    for (i, v) in std::iter::once(EXIT).chain(args.iter().copied()).enumerate() {
        word(mem, STACK + 4 * i as u32, v);
    }
    let mut cpu = Cpu { gpr: [0, this, 0, 0x1111, STACK, 0x2222, 0x3333, 0x4444], eip, ..Cpu::default() };
    cpu.mxcsr = fpu.mxcsr;
    cpu.x87.control = fpu.control;
    run(code, &mut cpu, mem);
    abi_cpu(&cpu)
}

fn apply(m: &mut FlatMemory, j: Journal) {
    for (a, v) in j.finish() {
        word(m, a, v);
    }
}

/// Each class's `vt[+0x08]` (forces) and `vt[+0x04]` (pose), one at a
/// time: the motion equals the lifted virtual's, byte for byte, under
/// every rounding mode.
#[test]
fn the_unconstrained_motion_virtuals_match_the_lifted_virtuals() {
    let Some((image, base)) = ram() else { return };
    let mut code = integrate_code(image);
    for seed in 0..512u64 {
        let class = FreeClass::ALL[seed as usize % 4];
        let pose = seed / 4 % 2 == 1;
        let mut r = Rng(0x0f2e_0000 + seed);
        let mut mem = base.clone();
        seed_motion(&mut r, &mut mem, MOTION);
        seed_world(&mut r, &mut mem);
        let fpu = fpu(seed / 8);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let (forces_at, pose_at) = class.slots();
        let (eip, args): (u32, &[u32]) = if pose {
            motion_solve::pose(&mut j, class, MOTION, STEP, fpu)
                .unwrap_or_else(|f| panic!("seed {seed} {class:?}: pose declined {f:?}"));
            (pose_at, &[STEP])
        } else {
            motion_solve::forces(&mut j, class, MOTION, STEP, GRAVITY, fpu)
                .unwrap_or_else(|f| panic!("seed {seed} {class:?}: forces declined {f:?}"));
            (forces_at, &[STEP, GRAVITY])
        };
        let mut expected = before.clone();
        apply(&mut expected, j);
        let after = run_this(&mut code, &mut mem, eip, MOTION, args, fpu);
        assert_eq!(after.esp, STACK + 4 + 4 * args.len() as u32, "seed {seed} {class:?}: ret");
        assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444));
        for (i, v) in std::iter::once(EXIT).chain(args.iter().copied()).enumerate() {
            word(&mut expected, STACK + 4 * i as u32, v);
        }
        clear_dead_stack([&mut mem, &mut expected]);
        assert_eq!(first_difference(&mem, &expected), None, "seed {seed} {class:?} pose {pose} {fpu:x?}: first different byte");
    }
}

/// The island words the integrate reads, over the arrays `island()` lays.
fn integrate_words(m: &mut FlatMemory, entities: u32, constraints: u32) {
    for (off, v) in [
        (integrate::ISLAND_WORLD, WORLD),
        (integrate::ISLAND_ENTITIES, ENTITIES),
        (integrate::ISLAND_ENTITY_COUNT, entities),
        (integrate::ISLAND_ACTIONS, 0x0061_7f00),
        (integrate::ISLAND_ACTION_COUNT, 0),
        (integrate::ISLAND_CONSTRAINTS, CONSTRAINTS),
        (integrate::ISLAND_CONSTRAINT_COUNT, constraints),
    ] {
        word(m, ISLAND + off, v);
    }
}

/// Vtables for every solver class at `VTABLES + 0x200`, with both the
/// solver slots and the unconstrained ones, and the fixed one `island()`
/// lays at `VTABLES`.
fn motion_vtables(m: &mut FlatMemory) {
    for (k, class) in SolverClass::ALL.into_iter().enumerate() {
        let vt = VTABLES + 0x200 + 0x40 * k as u32;
        let (accumulate, export) = class.slots();
        let (forces, pose) = free_class(class).slots();
        for (slot, f) in [
            (island::MOTION_ACCUMULATE_SLOT, accumulate),
            (island::MOTION_EXPORT_SLOT, export),
            (integrate::MOTION_FORCES_SLOT, forces),
            (integrate::MOTION_POSE_SLOT, pose),
        ] {
            word(m, vt + slot, f);
        }
    }
    let (forces, pose) = FreeClass::Fixed.slots();
    word(m, VTABLES + integrate::MOTION_FORCES_SLOT, forces);
    word(m, VTABLES + integrate::MOTION_POSE_SLOT, pose);
}

/// An island of `entities` motions of random classes and no constraint.
fn free_island(r: &mut Rng, mem: &mut FlatMemory, entities: u32) {
    island(r, mem, entities, 0, BUILD_STUB, BUFFER_BYTES);
    seed_world(r, mem);
    motion_vtables(mem);
    for i in 0..entities {
        let at = MOTION + 0x200 * i;
        seed_motion(r, mem, at);
        word(mem, at, VTABLES + 0x200 + 0x40 * r.below(6));
    }
    integrate_words(mem, entities, 0);
}

/// Run the lifted integrate on `ISLAND` and check its frame: `ret 4`, ecx
/// and the callee-saved registers as on entry.
fn run_integrate(code: &mut lifted::Original, mem: &mut FlatMemory, fpu: Fpu, why: &str) {
    let after = run_this(code, mem, INTEGRATE, ISLAND, &[INPUT], fpu);
    assert_eq!(after.esp, STACK + 8, "{why}: ret 4");
    assert_eq!((after.ecx, after.ebx, after.ebp, after.esi, after.edi), (ISLAND, 0x1111, 0x2222, 0x3333, 0x4444), "{why}");
}

/// The dead stack only: the solver buffer is compared too, since the
/// next island reads the bytes a solve leaves there.
fn clear_scratch(ms: [&mut FlatMemory; 2]) {
    clear_dead_stack(ms);
}

/// Islands with no constraint, one to five bodies of every motion type:
/// each entity's forces and pose, last to first. Memory outside the dead
/// stack equals the lifted entry's.
#[test]
fn unconstrained_islands_match_the_lifted_integrate() {
    let Some((image, base)) = ram() else { return };
    let mut code = integrate_code(image);
    for seed in 0..256u64 {
        let mut r = Rng(0x16a2_0000 + seed);
        let entities = r.below(6);
        let mut mem = base.clone();
        free_island(&mut r, &mut mem, entities);
        word(&mut mem, STACK, EXIT);
        word(&mut mem, STACK + 4, INPUT);
        let fpu = fpu(seed);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let out = integrate::integrate(&mut j, ISLAND, INPUT, fpu).unwrap_or_else(|f| panic!("seed {seed}: declined {f:?}"));
        assert!(!out.solved);
        let mut expected = before.clone();
        apply(&mut expected, j);
        let why = format!("seed {seed} ({entities} bodies) {fpu:x?}");
        run_integrate(&mut code, &mut mem, fpu, &why);
        clear_scratch([&mut mem, &mut expected]);
        assert_eq!(first_difference(&mem, &expected), None, "{why}: first different byte");
    }
}

/// Islands of every motion type joined by contacts: the integrate passes
/// the solve its nine arguments, and memory outside the dead stack, the
/// solver buffer included, equals the lifted entry's.
#[test]
fn contact_islands_match_the_lifted_integrate() {
    let Some((image, base)) = ram() else { return };
    let mut code = integrate_code(image);
    let (mut solved, mut declined) = (0, 0);
    for seed in 0..192u64 {
        let mut r = Rng(0x16a3_0000 + seed);
        let (entities, constraints) = (1 + r.below(3), 1 + r.below(3));
        let mut mem = base.clone();
        contact_island(&mut r, &mut mem, entities, constraints);
        motion_vtables(&mut mem);
        integrate_words(&mut mem, entities, constraints);
        word(&mut mem, STACK, EXIT);
        word(&mut mem, STACK + 4, INPUT);
        let fpu = fpu(seed);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        match integrate::integrate(&mut j, ISLAND, INPUT, fpu) {
            Ok(out) => assert!(out.solved),
            Err(reason) => {
                assert_eq!(reason, Decline::Solve, "seed {seed}");
                declined += 1;
                continue;
            }
        }
        solved += 1;
        let mut expected = before.clone();
        apply(&mut expected, j);
        let why = format!("seed {seed} {fpu:x?}");
        run_integrate(&mut code, &mut mem, fpu, &why);
        clear_scratch([&mut mem, &mut expected]);
        assert_eq!(first_difference(&mem, &expected), None, "{why}: first different byte");
    }
    assert!(declined <= solved, "{declined} of {} islands declined", solved + declined);
}

fn integrate_cpu(fpu: Fpu) -> runtime::CpuAbi {
    let mut cpu = runtime::CpuAbi { ecx: ISLAND, ebx: 0x1111, ebp: 0x2222, esi: 0x3333, edi: 0x4444, esp: STACK, mxcsr: fpu.mxcsr, ..runtime::CpuAbi::default() };
    cpu.x87.control = fpu.control;
    cpu
}

/// The arm end to end on islands of both paths: an audited call's
/// prediction matches the lifted answer, and an applied prediction leaves
/// memory and the registers as the lifted entry does.
#[test]
fn the_integrate_arm_answers_as_the_lifted_entry() {
    let Some((image, base)) = ram() else { return };
    let mut code = integrate_code(image);
    for seed in 0..32u64 {
        let mut r = Rng(0x16a4_0000 + seed);
        let mut mem = base.clone();
        if seed % 2 == 0 {
            let entities = 1 + r.below(4);
            free_island(&mut r, &mut mem, entities);
        } else {
            let (entities, constraints) = (2, 1 + r.below(2));
            contact_island(&mut r, &mut mem, entities, constraints);
            motion_vtables(&mut mem);
            integrate_words(&mut mem, entities, constraints);
        }
        word(&mut mem, STACK, EXIT);
        word(&mut mem, STACK + 4, INPUT);
        // The arm's environment: nearest x87 rounding, any SSE rounding.
        let fpu = Fpu { mxcsr: 0x1f80 | ((seed % 4) as u32) << 13, control: 0x027f };
        let cpu = integrate_cpu(fpu);
        let g = guest(&mem);
        let prediction = match arm::capture(&cpu, &g) {
            Ok(p) => p,
            Err(reason) => {
                assert_eq!(reason, Decline::Solve, "seed {seed}");
                continue;
            }
        };
        let mut lifted = mem.clone();
        run_integrate(&mut code, &mut lifted, fpu, &format!("seed {seed}"));
        let mut answered = cpu;
        answered.esp = STACK + 8;
        assert!(prediction.matches(&answered, &guest(&lifted)), "seed {seed}: the audit matches the lifted answer");
        let mut applied = guest(&mem);
        let mut regs = cpu;
        assert!(prediction.apply(&mut regs, &mut applied), "seed {seed}");
        assert_eq!((regs.esp, regs.ecx, regs.ebx, regs.ebp, regs.esi, regs.edi), (STACK + 8, ISLAND, 0x1111, 0x2222, 0x3333, 0x4444));
        let start = applied.mappings()[0].physical_start as usize;
        let mut model = FlatMemory { bytes: applied.physical_ram()[start..start + RAM].to_vec(), ..base.clone() };
        clear_scratch([&mut lifted, &mut model]);
        assert_eq!(first_difference(&lifted, &model), None, "seed {seed}: first different byte");
    }
}

/// An island with an action, one with a motion whose unconstrained slots
/// are unported, one whose solve declines and one under the monitor
/// stream: the arm declines with nothing written and counts each reason.
#[test]
fn integrates_the_model_does_not_cover_are_declined_and_counted() {
    let Some((_, base)) = ram() else { return };
    let cases: [(Decline, fn(&mut FlatMemory)); 4] = [
        (Decline::Action, |m| word(m, ISLAND + integrate::ISLAND_ACTION_COUNT, 1)),
        (Decline::Motion, |m| word(m, VTABLES + 0x200 + integrate::MOTION_POSE_SLOT, 0x0012_3456)),
        (Decline::Solve, |m| {
            word(m, ISLAND + integrate::ISLAND_CONSTRAINT_COUNT, 1);
            word(m, island::TABLES_READY & !3, 0);
        }),
        (Decline::Monitor, |m| word(m, MONITOR_END, 0x0060_0900)),
    ];
    for (reason, spoil) in cases {
        let mut r = Rng(0xdec1_0000 + reason as u64);
        let mut mem = base.clone();
        free_island(&mut r, &mut mem, 3);
        for i in 0..3 {
            word(&mut mem, MOTION + 0x200 * i, VTABLES + 0x200);
        }
        word(&mut mem, STACK, EXIT);
        word(&mut mem, STACK + 4, INPUT);
        spoil(&mut mem);
        let mut cpu = integrate_cpu(Fpu { mxcsr: 0x1f80, control: 0x027f });
        let mut g = guest(&mem);
        let index = Decline::ALL.iter().position(|f| *f == reason).unwrap();
        let before = integrate::fallbacks()[index].1;
        let declined = parity::status()[4].3;
        assert!(matches!(parity::integrate_enter(&mut cpu, &mut g), Step::Guest), "{reason:?}");
        assert!(integrate::fallbacks()[index].1 > before, "{reason:?} counted");
        assert!(parity::status()[4].3 > declined, "{reason:?} counted on the arm");
        assert_eq!(cpu.esp, STACK, "{reason:?}: the frame is the guest's");
        let start = g.mappings()[0].physical_start as usize;
        assert!(g.physical_ram()[start..start + RAM] == mem.bytes[..], "{reason:?}: nothing written");
    }
}

// ---- the deactivation test `0x00316b30` and the integrate phase `0x0030fef0`

use engine::sim_physics::{deactivation as test_arm, integrate_phase as phase_arm};
use sim_havok::deactivation::{self as deact, Deactivator, Fallback as TestDecline};
use sim_havok::integrate_phase::{self as phase, Fallback as PhaseDecline};

const DEACTIVATION: u32 = 0x0031_6b30;
const PHASE: u32 = 0x0030_fef0;
/// The out byte's slot, and the step `{dt, 1/dt}` the phase's caller passes.
const OUT: u32 = 0x0067_0000;
const STEP_IN: u32 = 0x0067_0010;
const ISLAND_LIST: u32 = 0x0067_0100;
/// Deactivator objects, 0x100 apart; their vtables after the motions'.
const DEACTIVATORS: u32 = 0x0068_0000;
const DEACTIVATOR_VTABLES: u32 = VTABLES + 0x600;
/// Halo's object tables behind `0x001d1910`.
const HALO_HEADERS: u32 = 0x0069_0000;
const HALO_DEFINITIONS: u32 = 0x0069_0800;
const HALO_TAGS: u32 = 0x0069_2000;
const HALO_NODES: u32 = 0x0069_3000;
const HALO_TAG_DATA: u32 = 0x0069_8000;
const HALO_OBJECTS: u32 = 0x006a_0000;
/// Islands past the first, 0x100 apart, with their own arrays.
const MORE_ISLANDS: u32 = 0x0063_0000;

fn phase_code(image: &Image) -> lifted::Original {
    let mut code = integrate_code(image);
    let deactivators = [Deactivator::NEVER, Deactivator::HAVOK_A, Deactivator::HAVOK_B, Deactivator::HALO_A, Deactivator::HALO_B];
    for f in [DEACTIVATION, PHASE, 0x1d1910, 0xe5240, 0xefb60, 0x175f40, 0x318700, 0x30c180, 0x30e720].into_iter().chain(deactivators) {
        code.prime(f);
    }
    code
}

fn put_byte(m: &mut FlatMemory, at: u32, v: u8) {
    m.bytes[at as usize] = v;
}

/// The deactivator vtables: never, Havok's, Halo's (slots `+0x14` and
/// `+0x18`), at `DEACTIVATOR_VTABLES + 0x20 k`.
fn deactivator_vtables(m: &mut FlatMemory) {
    let pairs = [
        (Deactivator::NEVER, Deactivator::NEVER),
        (Deactivator::HAVOK_A, Deactivator::HAVOK_B),
        (Deactivator::HALO_A, Deactivator::HALO_B),
    ];
    for (k, (a, b)) in pairs.into_iter().enumerate() {
        let vt = DEACTIVATOR_VTABLES + 0x20 * k as u32;
        word(m, vt + deact::SLOT_A, a);
        word(m, vt + deact::SLOT_B, b);
    }
    word(m, deact::OBJECT_HEADERS, HALO_HEADERS);
    word(m, HALO_HEADERS + 0x44, HALO_HEADERS + 0x100);
    word(m, deact::OBJECT_DEFINITIONS, HALO_DEFINITIONS);
    word(m, HALO_DEFINITIONS + 0x44, HALO_DEFINITIONS + 0x100);
    word(m, deact::TAG_INSTANCES, HALO_TAGS);
}

/// A float near `x`: within `near` or past `far`.
fn nudge(r: &mut Rng, x: f32, near: f32, far: f32) -> f32 {
    let d = if r.below(2) == 0 { float(r, -near, near) } else { far * if r.below(2) == 0 { 1.0 } else { -1.0 } + float(r, -near, near) };
    x + d
}

/// Entity `n`'s deactivator (`kind`: 0 none, 1 never, 2 Havok, 3 Halo),
/// its references near the motion's state or away from it, and for Halo's
/// the object, definition, node and tag words `0x001d1910` reads.
fn seed_deactivator(r: &mut Rng, m: &mut FlatMemory, entity: u32, motion: u32, n: u32, kind: u32) {
    let d = DEACTIVATORS + 0x100 * n;
    if kind == 0 {
        word(m, entity + deact::ENTITY_DEACTIVATOR, 0);
        return;
    }
    word(m, entity + deact::ENTITY_DEACTIVATOR, d);
    word(m, d, DEACTIVATOR_VTABLES + 0x20 * (kind - 1));
    word(m, d + deact::DEACTIVATOR_SCALE, bits(r, 0.5, 2.0));
    let center: [f32; 4] = std::array::from_fn(|i| f32::from_bits(m.read_u32(motion + deact::MOTION_CENTER + 4 * i as u32).unwrap()));
    let q: [f32; 4] = std::array::from_fn(|i| f32::from_bits(m.read_u32(motion + deact::MOTION_ORIENTATION + 4 * i as u32).unwrap()));
    for (c, o) in [(0x10, 0x20), (0x30, 0x40)] {
        let near = if r.below(2) == 0 { 0.01 } else { 0.3 };
        put_lanes(m, d + c, center.map(|x| nudge(r, x, near, 0.5)));
        put_lanes(m, d + o, q.map(|x| nudge(r, x, near / 4.0, 0.3)));
    }
    for off in [0x50, 0x54, 0x58, 0x5c] {
        word(m, d + off, bits(r, 0.005, 0.2));
    }
    if kind != 3 {
        return;
    }
    let node = r.below(4);
    word(m, d + 0x60, (r.u32() & 0xffff_0000) | n);
    word(m, d + 0x64, node);
    let header = HALO_HEADERS + 0x100 + 12 * n;
    let object = HALO_OBJECTS + 0x400 * n;
    put_byte(m, header + 3, r.below(3) as u8);
    word(m, header + 8, object);
    word(m, object, (r.u32() & 0xffff_0000) | n);
    word(m, object + 0xb4, (r.u32() & 0xffff_0000) | n);
    put_byte(m, object + 0x34b, u8::from(r.below(4) == 0));
    put_byte(m, object + 0x3dc, 0);
    put_byte(m, object + 0x38b, if r.below(2) == 0 { 0xff } else { r.below(8) as u8 });
    for off in [0x354, 0x35c, 0x25c, 0x260, 0x358] {
        let v = match r.below(4) {
            0 => bits(r, -1.0, 1.0),
            1 => 0x8000_0000,
            2 => 1,
            _ => 0,
        };
        word(m, object + off, v);
    }
    let nodes = HALO_NODES + 0x200 * n;
    word(m, HALO_DEFINITIONS + 0x100 + 0xa0 * n + 0x70, nodes);
    put_byte(m, nodes + 0x60 * node + 0x44, [0, 1, 2, 1][r.below(4) as usize]);
    let data = HALO_TAG_DATA + 0x200 * n;
    word(m, HALO_TAGS + 16 * n + 8, data);
    word(m, data + 0x1ec, r.u32() & 0x2f);
}

/// The deactivation words of the island at `at` with `entities` entities
/// at `entity(i)` and motions at `motion(i)`: timers that make either
/// period fire, both or neither, a counter on either side of the limit,
/// and a deactivator of every kind.
fn seed_deactivation(r: &mut Rng, m: &mut FlatMemory, at: u32, entities: u32, entity: impl Fn(u32) -> u32, motion: impl Fn(u32) -> u32, base: u32) {
    let dt = f32::from_bits(m.read_u32(STEP_IN).unwrap());
    let (pa, pb) = (f32::from_bits(m.read_u32(WORLD + deact::WORLD_PERIOD_A).unwrap()), f32::from_bits(m.read_u32(WORLD + deact::WORLD_PERIOD_B).unwrap()));
    let path = r.below(4);
    let a = if path & 1 == 1 { pa - dt * float(r, 0.0, 0.9) } else { float(r, 0.0, pa - 2.0 * dt) };
    let b = if path & 2 == 2 { pb - dt * float(r, 0.0, 0.9) } else { float(r, 0.0, pb - 2.0 * dt) };
    word(m, at + deact::ISLAND_TIMER_A, a.to_bits());
    word(m, at + deact::ISLAND_TIMER_B, b.to_bits());
    put_byte(m, at + deact::ISLAND_COUNTER, [0, 3, 4, 5, 6, 0x80][r.below(6) as usize]);
    // Most islands rest or keep only by Havok's test; some have an entity
    // that never rests.
    let awake = r.below(3) == 0;
    for i in 0..entities {
        let kind = if awake { r.below(4) } else { 2 + r.below(2) };
        seed_deactivator(r, m, entity(i), motion(i), base + i, kind);
    }
}

fn seed_periods(r: &mut Rng, m: &mut FlatMemory) {
    word(m, WORLD + deact::WORLD_PERIOD_A, bits(r, 0.2, 1.0));
    word(m, WORLD + deact::WORLD_PERIOD_B, bits(r, 1.0, 4.0));
    word(m, STEP_IN, m.read_u32(STEP).unwrap());
    word(m, STEP_IN + 4, m.read_u32(STEP + 4).unwrap());
}

/// The float state of a seed, with the DAZ bit on some, which the
/// Pentium III ignores.
fn test_fpu(seed: u64) -> Fpu {
    let f = fpu(seed);
    Fpu { mxcsr: f.mxcsr | if seed % 5 == 0 { 0x40 } else { 0 }, ..f }
}

/// The dead stack and the argument slots, which the entries use as scratch.
fn clear_frame(ms: [&mut FlatMemory; 2], args: u32) {
    for m in ms {
        m.bytes[(STACK - 0x1000) as usize..(STACK + 4 + args) as usize].fill(0);
    }
}

fn deactivation_island(r: &mut Rng, mem: &mut FlatMemory) -> u32 {
    let entities = r.below(5);
    free_island(r, mem, entities);
    deactivator_vtables(mem);
    seed_periods(r, mem);
    seed_deactivation(r, mem, ISLAND, entities, |i| ENTITY + 0x100 * i, |i| MOTION + 0x200 * i, 0);
    entities
}

/// Islands of zero to four entities with deactivators of every kind, each
/// period firing or not: the timers, the counter, the references and the
/// out byte equal the lifted entry's, and eax is the out pointer.
#[test]
fn the_deactivation_test_matches_the_lifted_test() {
    let Some((image, base)) = ram() else { return };
    let mut code = phase_code(image);
    let (mut deactivated, mut kept) = (0, 0);
    for seed in 0..512u64 {
        let mut r = Rng(0x316b_0000 + seed);
        let mut mem = base.clone();
        let entities = deactivation_island(&mut r, &mut mem);
        let fpu = test_fpu(seed);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let out = deact::test(&mut j, ISLAND, Some(OUT), STEP_IN, fpu).unwrap_or_else(|f| panic!("seed {seed}: declined {f:?}"));
        if out { deactivated += 1 } else { kept += 1 }
        let mut expected = before.clone();
        apply(&mut expected, j);
        let why = format!("seed {seed} ({entities} entities) {fpu:x?}");
        let after = run_this(&mut code, &mut mem, DEACTIVATION, ISLAND, &[OUT, STEP_IN], fpu);
        assert_eq!((after.eax, after.esp), (OUT, STACK + 12), "{why}: eax and ret 8");
        assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444), "{why}");
        assert_eq!(mem.bytes[OUT as usize], u8::from(out), "{why}: the out byte");
        clear_frame([&mut mem, &mut expected], 8);
        assert_eq!(first_difference(&mem, &expected), None, "{why}: first different byte");
    }
    assert!(deactivated > 32 && kept > 32, "{deactivated} deactivated, {kept} kept");
}

fn test_cpu(fpu: Fpu) -> runtime::CpuAbi {
    let mut cpu = runtime::CpuAbi { ecx: ISLAND, ebx: 0x1111, ebp: 0x2222, esi: 0x3333, edi: 0x4444, esp: STACK, mxcsr: fpu.mxcsr, ..runtime::CpuAbi::default() };
    cpu.x87.control = fpu.control;
    cpu
}

/// The deactivation arm end to end: an audited call's prediction matches
/// the lifted answer, and an applied prediction leaves memory and the
/// registers as the lifted entry does.
#[test]
fn the_deactivation_arm_answers_as_the_lifted_test() {
    let Some((image, base)) = ram() else { return };
    let mut code = phase_code(image);
    for seed in 0..32u64 {
        let mut r = Rng(0x316c_0000 + seed);
        let mut mem = base.clone();
        deactivation_island(&mut r, &mut mem);
        for (i, v) in [EXIT, OUT, STEP_IN].into_iter().enumerate() {
            word(&mut mem, STACK + 4 * i as u32, v);
        }
        let fpu = Fpu { mxcsr: 0x1f80 | ((seed % 4) as u32) << 13, control: 0x027f };
        let cpu = test_cpu(fpu);
        let prediction = test_arm::capture(&cpu, &guest(&mem)).unwrap_or_else(|f| panic!("seed {seed}: declined {f:?}"));
        let mut lifted = mem.clone();
        let after = run_this(&mut code, &mut lifted, DEACTIVATION, ISLAND, &[OUT, STEP_IN], fpu);
        let mut answered = cpu;
        (answered.esp, answered.eax) = (after.esp, after.eax);
        assert!(prediction.matches(&answered, &guest(&lifted)), "seed {seed}: the audit matches the lifted answer");
        let mut applied = guest(&mem);
        let mut regs = cpu;
        assert!(prediction.apply(&mut regs, &mut applied), "seed {seed}");
        assert_eq!((regs.eax, regs.esp, regs.ebx, regs.ebp, regs.esi, regs.edi), (OUT, STACK + 12, 0x1111, 0x2222, 0x3333, 0x4444));
        let start = applied.mappings()[0].physical_start as usize;
        let mut model = FlatMemory { bytes: applied.physical_ram()[start..start + RAM].to_vec(), ..base.clone() };
        clear_frame([&mut lifted, &mut model], 8);
        assert_eq!(first_difference(&lifted, &model), None, "seed {seed}: first different byte");
    }
}

/// A deactivator slot the model does not know, Halo's object check
/// reaching `0x000e4050`, and the monitor stream: declined with nothing
/// written, each reason counted.
#[test]
fn deactivation_tests_the_model_does_not_cover_are_declined_and_counted() {
    let Some((_, base)) = ram() else { return };
    let cases: [(TestDecline, fn(&mut FlatMemory)); 3] = [
        (TestDecline::Deactivator, |m| word(m, DEACTIVATOR_VTABLES + 0x20 + deact::SLOT_A, 0x0012_3456)),
        (TestDecline::Object, |m| {
            let (header, object) = (HALO_HEADERS + 0x100, HALO_OBJECTS);
            put_byte(m, header + 3, 0);
            put_byte(m, object + 0x34b, 0);
            put_byte(m, object + 0x3dc, 1);
            let nodes = m.read_u32(HALO_DEFINITIONS + 0x100 + 0x70).unwrap();
            let node = m.read_u32(DEACTIVATORS + 0x64).unwrap();
            put_byte(m, nodes + 0x60 * node + 0x44, 0);
        }),
        (TestDecline::Monitor, |m| word(m, MONITOR_END, 0x0060_0900)),
    ];
    for (reason, spoil) in cases {
        let mut r = Rng(0xdec2_0000 + reason as u64);
        let mut mem = base.clone();
        free_island(&mut r, &mut mem, 1);
        deactivator_vtables(&mut mem);
        seed_periods(&mut r, &mut mem);
        // The first period fires on one entity with a Halo deactivator for
        // the object case and a Havok one otherwise.
        let period = mem.read_u32(WORLD + deact::WORLD_PERIOD_A).unwrap();
        word(&mut mem, ISLAND + deact::ISLAND_TIMER_A, period);
        let kind = if reason == TestDecline::Object { 3 } else { 2 };
        seed_deactivator(&mut r, &mut mem, ENTITY, MOTION, 0, kind);
        for (i, v) in [EXIT, OUT, STEP_IN].into_iter().enumerate() {
            word(&mut mem, STACK + 4 * i as u32, v);
        }
        spoil(&mut mem);
        let mut cpu = test_cpu(Fpu { mxcsr: 0x1f80, control: 0x027f });
        let mut g = guest(&mem);
        let index = TestDecline::ALL.iter().position(|f| *f == reason).unwrap();
        let before = deact::fallbacks()[index].1;
        let declined = parity::status()[5].3;
        assert!(matches!(parity::deactivation_enter(&mut cpu, &mut g), Step::Guest), "{reason:?}");
        assert!(deact::fallbacks()[index].1 > before, "{reason:?} counted");
        assert!(parity::status()[5].3 > declined, "{reason:?} counted on the arm");
        assert_eq!(cpu.esp, STACK, "{reason:?}: the frame is the guest's");
        let start = g.mappings()[0].physical_start as usize;
        assert!(g.physical_ram()[start..start + RAM] == mem.bytes[..], "{reason:?}: nothing written");
    }
}

/// Island `k` past the first: its struct, entity array, entities and
/// motions, none shared with the first island's.
fn more_island(k: u32) -> (u32, u32, impl Fn(u32) -> u32, impl Fn(u32) -> u32) {
    let at = MORE_ISLANDS + 0x100 * k;
    let entities = MORE_ISLANDS + 0x1000 + 0x100 * k;
    (at, entities, move |i| MORE_ISLANDS + 0x2000 + 0x800 * k + 0x100 * i, move |i| MORE_ISLANDS + 0x8000 + 0x1000 * k + 0x200 * i)
}

/// A world of one to four active islands in a random list order: the
/// first a contact or a free island, the rest free; the deactivation test
/// on or off with timers that never deactivate, and the split pass on or
/// off over islands with nothing to split.
fn phase_world(r: &mut Rng, mem: &mut FlatMemory) -> (u32, bool) {
    let contact = r.below(2) == 0;
    let first = if contact {
        let (entities, constraints) = (1 + r.below(3), 1 + r.below(3));
        contact_island(r, mem, entities, constraints);
        motion_vtables(mem);
        integrate_words(mem, entities, constraints);
        entities
    } else {
        let entities = 1 + r.below(3);
        free_island(r, mem, entities);
        entities
    };
    deactivator_vtables(mem);
    seed_periods(r, mem);
    let islands = 1 + r.below(4);
    let mut list = vec![(ISLAND, first, ENTITY, MOTION)];
    for k in 1..islands {
        let (at, array, entity, motion) = more_island(k);
        let n = 1 + r.below(3);
        for (off, v) in [
            (sim_havok::island_integrate::ISLAND_WORLD, WORLD),
            (sim_havok::island_integrate::ISLAND_ENTITIES, array),
            (sim_havok::island_integrate::ISLAND_ENTITY_COUNT, n),
            (sim_havok::island_integrate::ISLAND_ACTION_COUNT, 0),
            (sim_havok::island_integrate::ISLAND_CONSTRAINT_COUNT, 0),
        ] {
            word(mem, at + off, v);
        }
        for i in 0..n {
            word(mem, array + 4 * i, entity(i));
            word(mem, entity(i) + island::ENTITY_MOTION, motion(i));
            seed_motion(r, mem, motion(i));
            word(mem, motion(i), VTABLES + 0x200 + 0x40 * r.below(6));
        }
        list.push((at, n, entity(0), motion(0)));
    }
    let test = r.below(3) != 0;
    for (k, &(at, n, entity0, motion0)) in list.iter().enumerate() {
        // Timers that never reach a deactivation: the second period never
        // fires, and the first finds an entity that never rests or a counter
        // below the limit.
        seed_deactivation(r, mem, at, n, |i| entity0 + 0x100 * i, |i| motion0 + 0x200 * i, 8 * k as u32);
        word(mem, at + deact::ISLAND_TIMER_B, 0);
        put_byte(mem, at + deact::ISLAND_COUNTER, r.below(5) as u8);
        put_byte(mem, at + phase::ISLAND_SPLIT, 0);
        put_byte(mem, at + phase::ISLAND_INTEGRATED, r.u32() as u8);
    }
    let mut order: Vec<u32> = list.iter().map(|l| l.0).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, r.below(i as u32 + 1) as usize);
    }
    for (i, at) in order.into_iter().enumerate() {
        word(mem, ISLAND_LIST + 4 * i as u32, at);
    }
    for (off, v) in [
        (phase::WORLD_ISLANDS, ISLAND_LIST),
        (phase::WORLD_ISLAND_COUNT, islands),
        (phase::WORLD_PENDING, 0),
        (phase::WORLD_ISLAND_LISTENERS, 0),
        (phase::WORLD_STEP_LISTENERS, 0),
        (phase::WORLD_STEP_LISTENER, 0),
        (phase::WORLD_MERGES, 0),
        (phase::WORLD_QUEUE, 0),
        (phase::WORLD_DEACTIVATE, 0),
        (phase::WORLD_RELEASES, [0, 0, 0xffff_fff0][r.below(3) as usize]),
        (phase::WORLD_ITERATIONS, r.below(5)),
        (phase::WORLD_SCALE, bits(r, 0.5, 1.5)),
    ] {
        word(mem, WORLD + off, v);
    }
    put_byte(mem, WORLD + phase::WORLD_DEACTIVATION, u8::from(test));
    put_byte(mem, WORLD + phase::WORLD_SPLIT, r.below(2) as u8);
    put_byte(mem, WORLD + phase::WORLD_LOCK, r.u32() as u8);
    (islands, test)
}

fn run_phase(code: &mut lifted::Original, mem: &mut FlatMemory, fpu: Fpu, why: &str) {
    let after = run_this(code, mem, PHASE, WORLD, &[STEP_IN], fpu);
    assert_eq!(after.esp, STACK + 8, "{why}: ret 4");
    let monitor = (mem.read_u32(MONITOR_END).unwrap(), mem.read_u32(MONITOR_HEAD).unwrap());
    assert_eq!((after.eax, after.ecx, after.edx), (monitor.0, monitor.1, u32::MAX), "{why}: eax, ecx, edx");
    assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444), "{why}");
}

/// Worlds of one to four islands: the step terms, every island's
/// deactivation test and integrate in the phase's order, the split pass
/// and the flags equal the lifted phase's, the solver buffer included.
#[test]
fn integrate_phases_match_the_lifted_phase() {
    let Some((image, base)) = ram() else { return };
    let mut code = phase_code(image);
    let (mut run, mut declined, mut tested) = (0, 0, 0);
    for seed in 0..256u64 {
        let mut r = Rng(0x30fe_0000 + seed);
        let mut mem = base.clone();
        let (islands, test) = phase_world(&mut r, &mut mem);
        let fpu = test_fpu(seed);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        match phase::phase(&mut j, WORLD, STEP_IN, fpu) {
            Ok(out) => assert_eq!(out.islands, islands, "seed {seed}"),
            Err(reason) => {
                assert_eq!(reason, PhaseDecline::Island, "seed {seed}");
                declined += 1;
                continue;
            }
        }
        run += 1;
        tested += u32::from(test);
        let mut expected = before.clone();
        apply(&mut expected, j);
        let why = format!("seed {seed} ({islands} islands, test {test}) {fpu:x?}");
        run_phase(&mut code, &mut mem, fpu, &why);
        clear_frame([&mut mem, &mut expected], 4);
        assert_eq!(first_difference(&mem, &expected), None, "{why}: first different byte");
    }
    assert!(declined <= run / 2 && tested > 32, "{run} run, {declined} declined, {tested} tested");
}

fn phase_cpu(fpu: Fpu) -> runtime::CpuAbi {
    runtime::CpuAbi { ecx: WORLD, ..test_cpu(fpu) }
}

/// The phase arm end to end, as the integrate arm's test.
#[test]
fn the_phase_arm_answers_as_the_lifted_phase() {
    let Some((image, base)) = ram() else { return };
    let mut code = phase_code(image);
    for seed in 0..32u64 {
        let mut r = Rng(0x30ff_0000 + seed);
        let mut mem = base.clone();
        phase_world(&mut r, &mut mem);
        word(&mut mem, STACK, EXIT);
        word(&mut mem, STACK + 4, STEP_IN);
        let fpu = Fpu { mxcsr: 0x1f80 | ((seed % 4) as u32) << 13, control: 0x027f };
        let cpu = phase_cpu(fpu);
        let prediction = match phase_arm::capture(&cpu, &guest(&mem)) {
            Ok(p) => p,
            Err(reason) => {
                assert_eq!(reason, PhaseDecline::Island, "seed {seed}");
                continue;
            }
        };
        let mut lifted = mem.clone();
        run_phase(&mut code, &mut lifted, fpu, &format!("seed {seed}"));
        let monitor = (mem.read_u32(MONITOR_END).unwrap(), mem.read_u32(MONITOR_HEAD).unwrap());
        let mut answered = cpu;
        (answered.esp, answered.eax, answered.ecx, answered.edx) = (STACK + 8, monitor.0, monitor.1, u32::MAX);
        assert!(prediction.matches(&answered, &guest(&lifted)), "seed {seed}: the audit matches the lifted answer");
        let mut applied = guest(&mem);
        let mut regs = cpu;
        assert!(prediction.apply(&mut regs, &mut applied), "seed {seed}");
        assert_eq!((regs.eax, regs.ecx, regs.edx, regs.esp), (monitor.0, monitor.1, u32::MAX, STACK + 8));
        assert_eq!((regs.ebx, regs.ebp, regs.esi, regs.edi), (0x1111, 0x2222, 0x3333, 0x4444));
        let start = applied.mappings()[0].physical_start as usize;
        let mut model = FlatMemory { bytes: applied.physical_ram()[start..start + RAM].to_vec(), ..base.clone() };
        clear_frame([&mut lifted, &mut model], 4);
        assert_eq!(first_difference(&lifted, &model), None, "seed {seed}: first different byte");
    }
}

/// Pending work, a step listener, a deactivating island, an island to
/// split and the monitor stream: the phase arm declines with nothing
/// written and counts each reason.
#[test]
fn phases_the_model_does_not_cover_are_declined_and_counted() {
    let Some((_, base)) = ram() else { return };
    let cases: [(PhaseDecline, fn(&mut FlatMemory)); 5] = [
        (PhaseDecline::Pending, |m| word(m, WORLD + phase::WORLD_PENDING, 1)),
        (PhaseDecline::Listener, |m| word(m, WORLD + phase::WORLD_STEP_LISTENERS, 1)),
        (PhaseDecline::Deactivate, |m| {
            // The second period fires on an island with no entity.
            put_byte(m, WORLD + phase::WORLD_DEACTIVATION, 1);
            word(m, ISLAND + deact::ISLAND_ENTITY_COUNT, 0);
            word(m, ISLAND + sim_havok::island_integrate::ISLAND_ENTITY_COUNT, 0);
            word(m, ISLAND + deact::ISLAND_TIMER_B, m.read_u32(WORLD + deact::WORLD_PERIOD_B).unwrap());
        }),
        (PhaseDecline::Split, |m| {
            put_byte(m, WORLD + phase::WORLD_SPLIT, 1);
            put_byte(m, ISLAND + phase::ISLAND_SPLIT, 1);
        }),
        (PhaseDecline::Monitor, |m| word(m, MONITOR_END, 0x0060_0900)),
    ];
    for (reason, spoil) in cases {
        let mut r = Rng(0xdec3_0000 + reason as u64);
        let mut mem = loop {
            let mut m = base.clone();
            phase_world(&mut r, &mut m);
            let view = Flat(&m);
            if phase::phase(&mut Journal::new(&view), WORLD, STEP_IN, Fpu { mxcsr: 0x1f80, control: 0x027f }).is_ok() {
                break m;
            }
        };
        word(&mut mem, STACK, EXIT);
        word(&mut mem, STACK + 4, STEP_IN);
        spoil(&mut mem);
        let mut cpu = phase_cpu(Fpu { mxcsr: 0x1f80, control: 0x027f });
        let mut g = guest(&mem);
        let index = PhaseDecline::ALL.iter().position(|f| *f == reason).unwrap();
        let before = phase::fallbacks()[index].1;
        let declined = parity::status()[6].3;
        assert!(matches!(parity::phase_enter(&mut cpu, &mut g), Step::Guest), "{reason:?}");
        assert!(phase::fallbacks()[index].1 > before, "{reason:?} counted");
        assert!(parity::status()[6].3 > declined, "{reason:?} counted on the arm");
        assert_eq!(cpu.esp, STACK, "{reason:?}: the frame is the guest's");
        let start = g.mappings()[0].physical_start as usize;
        assert!(g.physical_ram()[start..start + RAM] == mem.bytes[..], "{reason:?}: nothing written");
    }
}

// ---- the structure polygon builder `0x001edd00` ----------------------------

use engine::sim_physics::structure_polygon as polygon_arm;
use sim_havok::structure::{self as structure, Fallback as PolygonDecline};

const BUILDER: u32 = 0x001e_dd00;
const POLY_BSP: u32 = 0x0070_0000;
const POLY_RECORD: u32 = 0x0070_0100;
const POLY_EDGES: u32 = 0x0070_1000;
const POLY_VERTICES: u32 = 0x0070_8000;
const POLY_SHAPE: u32 = 0x0072_0000;
const POLY_TRANSFORM: u32 = 0x0072_1000;

/// The builder's arguments for a seeded call.
struct PolyCall {
    args: structure::Args,
    vertices: u32,
}

/// A surface whose ring of three to eight edges takes each edge from
/// either side, at scattered edge and vertex indices, with raw vertex
/// words (a NaN or a denormal now and then), and on odd seeds a transform
/// whose scale is one or not.
fn poly_surface(r: &mut Rng, m: &mut FlatMemory, transformed: bool) -> PolyCall {
    let surface = r.below(0x8000);
    word(m, POLY_BSP + structure::BSP_SURFACES, POLY_RECORD.wrapping_sub(surface.wrapping_mul(8)));
    word(m, POLY_BSP + structure::BSP_EDGES, POLY_EDGES);
    word(m, POLY_BSP + structure::BSP_VERTICES, POLY_VERTICES);
    let n = 3 + r.below(6);
    let mut edges: Vec<u32> = Vec::new();
    while edges.len() < n as usize {
        let e = r.below(2000);
        if !edges.contains(&e) {
            edges.push(e);
        }
    }
    word(m, POLY_RECORD, (edges[0] << 16) | (r.u32() & 0xffff));
    word(m, POLY_RECORD + 4, r.u32());
    for i in 0..n as usize {
        let at = POLY_EDGES + 12 * edges[i];
        let next = edges[(i + 1) % n as usize];
        let vertex = r.below(4000);
        let other = |r: &mut Rng| loop {
            let v = r.u32() & 0xffff;
            if v != surface {
                break v;
            }
        };
        let (left, right) = (other(r), other(r));
        if r.below(2) == 0 {
            // The surface on the left: start and forward.
            word(m, at, vertex | (r.u32() & 0xffff) << 16);
            word(m, at + 4, next | (r.u32() & 0xffff) << 16);
            word(m, at + 8, surface | right << 16);
        } else {
            word(m, at, (r.u32() & 0xffff) | vertex << 16);
            word(m, at + 4, (r.u32() & 0xffff) | next << 16);
            word(m, at + 8, left | surface << 16);
        }
        for k in 0..4 {
            let v = match r.below(16) {
                0 => 0x7fc0_0000 | (r.u32() & 0xffff),
                1 => r.u32() & 0x007f_ffff,
                _ => bits(r, -500.0, 500.0),
            };
            word(m, POLY_VERTICES + 16 * vertex + 4 * k, v);
        }
    }
    random_bytes(r, m, POLY_SHAPE, 0x20 + 16 * n + 0x40);
    let transform = if transformed {
        let scale = if r.below(2) == 0 { 1.0f32.to_bits() } else { bits(r, 0.5, 2.0) };
        word(m, POLY_TRANSFORM, scale);
        for i in 1..13 {
            word(m, POLY_TRANSFORM + 4 * i, bits(r, -2.0, 2.0));
        }
        POLY_TRANSFORM
    } else {
        0
    };
    PolyCall {
        args: structure::Args { shape: POLY_SHAPE, transform, bsp: POLY_BSP, flag: r.u32() as u8, owner: r.u32(), surface, tail: r.u32() },
        vertices: n,
    }
}

fn poly_stack(m: &mut FlatMemory, a: &structure::Args) {
    for (i, v) in [EXIT, a.bsp, u32::from(a.flag), a.owner, a.surface, a.tail].into_iter().enumerate() {
        word(m, STACK + 4 * i as u32, v);
    }
}

/// Run the lifted builder and check its registers.
fn run_builder(code: &mut lifted::Original, m: &mut FlatMemory, a: &structure::Args, mxcsr: u32, why: &str) {
    poly_stack(m, a);
    let mut cpu = Cpu { gpr: [0, a.shape, 0, 0x1111, STACK, 0x2222, 0x3333, a.transform], eip: BUILDER, ..Cpu::default() };
    cpu.mxcsr = mxcsr;
    run(code, &mut cpu, m);
    let after = abi_cpu(&cpu);
    assert_eq!((after.eax, after.ecx, after.edx, after.esp), (a.shape, a.shape, a.tail, STACK + 0x18), "{why}: eax, ecx, edx, ret 0x14");
    assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, a.transform), "{why}");
}

fn builder_code(image: &Image) -> lifted::Original {
    let mut code = image.original();
    code.prime(BUILDER);
    code
}

/// Seeded surfaces, untransformed and transformed under every rounding
/// mode: the shape's words equal the lifted builder's, byte for byte.
#[test]
fn structure_polygons_match_the_lifted_builder() {
    let Some((image, base)) = ram() else { return };
    let mut code = builder_code(image);
    for seed in 0..512u64 {
        let mut r = Rng(0x1edd_0000 + seed);
        let mut mem = base.clone();
        let call = poly_surface(&mut r, &mut mem, seed % 2 == 1);
        let mxcsr = 0x1f80 | ((seed / 2 % 4) as u32) << 13;
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let n = structure::build(&mut j, &call.args, mxcsr).unwrap_or_else(|f| panic!("seed {seed}: declined {f:?}"));
        assert_eq!(n, call.vertices, "seed {seed}");
        let mut expected = before.clone();
        apply(&mut expected, j);
        let why = format!("seed {seed} ({n} vertices, transform {:#x}) mxcsr {mxcsr:#x}", call.args.transform);
        run_builder(&mut code, &mut mem, &call.args, mxcsr, &why);
        clear_frame([&mut mem, &mut expected], 0x14);
        assert_eq!(first_difference(&mem, &expected), None, "{why}: first different byte");
    }
}

/// The arm end to end: an audited call's prediction matches the lifted
/// answer, and an applied one leaves memory and the registers as it does.
#[test]
fn the_polygon_arm_answers_as_the_lifted_builder() {
    let Some((image, base)) = ram() else { return };
    let mut code = builder_code(image);
    for seed in 0..32u64 {
        let mut r = Rng(0x1ede_0000 + seed);
        let mut mem = base.clone();
        let call = poly_surface(&mut r, &mut mem, seed % 2 == 1);
        poly_stack(&mut mem, &call.args);
        let cpu = runtime::CpuAbi { ecx: POLY_SHAPE, edi: call.args.transform, ebx: 0x1111, ebp: 0x2222, esi: 0x3333, esp: STACK, mxcsr: 0x1f80, ..runtime::CpuAbi::default() };
        let prediction = polygon_arm::capture(&cpu, &guest(&mem)).unwrap_or_else(|f| panic!("seed {seed}: declined {f:?}"));
        assert_eq!(prediction.vertices, call.vertices);
        let mut lifted = mem.clone();
        run_builder(&mut code, &mut lifted, &call.args, 0x1f80, &format!("seed {seed}"));
        let mut answered = cpu;
        (answered.eax, answered.edx, answered.esp) = (POLY_SHAPE, call.args.tail, STACK + 0x18);
        assert!(prediction.matches(&answered, &guest(&lifted)), "seed {seed}: the audit matches the lifted answer");
        let mut applied = guest(&mem);
        let mut regs = cpu;
        assert!(prediction.apply(&mut regs, &mut applied), "seed {seed}");
        assert_eq!((regs.eax, regs.ecx, regs.edx, regs.esp), (POLY_SHAPE, POLY_SHAPE, call.args.tail, STACK + 0x18));
        let start = applied.mappings()[0].physical_start as usize;
        let mut model = FlatMemory { bytes: applied.physical_ram()[start..start + RAM].to_vec(), ..base.clone() };
        clear_frame([&mut lifted, &mut model], 0x14);
        assert_eq!(first_difference(&lifted, &model), None, "seed {seed}: first different byte");
    }
}

/// A ring that never closes, a transform under an unmasked MXCSR and an
/// unaligned shape: declined with nothing written, each reason counted.
#[test]
fn polygons_the_model_does_not_cover_are_declined_and_counted() {
    let Some((_, base)) = ram() else { return };
    for (reason, transformed) in [(PolygonDecline::Ring, false), (PolygonDecline::Float, true), (PolygonDecline::Memory, false)] {
        let mut r = Rng(0xdec4_0000 + reason as u64);
        let mut mem = base.clone();
        let call = poly_surface(&mut r, &mut mem, transformed);
        let mut cpu = runtime::CpuAbi { ecx: POLY_SHAPE, edi: call.args.transform, esp: STACK, mxcsr: 0x1f80, ..runtime::CpuAbi::default() };
        match reason {
            // The first edge's next edge is itself, from whichever side.
            PolygonDecline::Ring => {
                let first = mem.read_u32(POLY_RECORD).unwrap() >> 16;
                let at = POLY_EDGES + 12 * first;
                let next = mem.read_u32(at + 4).unwrap();
                let second = if next & 0xffff == first { next >> 16 } else { next & 0xffff };
                word(&mut mem, POLY_EDGES + 12 * second + 4, second | second << 16);
            }
            PolygonDecline::Float => cpu.mxcsr = 0x1f00,
            _ => cpu.ecx = POLY_SHAPE + 2,
        }
        poly_stack(&mut mem, &call.args);
        let mut g = guest(&mem);
        let index = PolygonDecline::ALL.iter().position(|f| *f == reason).unwrap();
        let before = structure::fallbacks()[index].1;
        let declined = parity::status()[7].3;
        assert!(matches!(parity::polygon_enter(&mut cpu, &mut g), Step::Guest), "{reason:?}");
        assert!(structure::fallbacks()[index].1 > before, "{reason:?} counted");
        assert!(parity::status()[7].3 > declined, "{reason:?} counted on the arm");
        let start = g.mappings()[0].physical_start as usize;
        assert!(g.physical_ram()[start..start + RAM] == mem.bytes[..], "{reason:?}: nothing written");
    }
}

/// Every surface of every collision BSP on the disc, structure and
/// ordinary tags, built as `0x001ee4d0` builds it: the shape equals the
/// lifted builder's. The BSP's surface, edge and vertex blocks are copied
/// to fresh RAM with the header's addresses rebased.
#[test]
#[ignore = "reads the owner's Halo 2 ISO (H2_DISC) and XBE (H2_XBE)"]
fn every_disc_surface_polygon_matches_the_lifted_builder() {
    use crate::sim_collision_differential::disc::{bsp_headers, for_each_tag};
    use map::FourCC;
    const BLOCKS: u32 = 0x0100_0000;
    let Some((image, base)) = ram() else { panic!("set H2_XBE to run the disc sweep") };
    let mut code = builder_code(image);
    let mut mem = base.clone();
    mem.bytes.resize(0x0400_0000, 0);
    let span = POLY_SHAPE as usize..POLY_SHAPE as usize + 0x20 + 16 * structure::MAX_RING as usize;
    let (mut surfaces, mut bsps, mut declined) = (0u64, 0u64, 0u64);
    let (coll, sbsp) = (FourCC(*b"coll"), FourCC(*b"sbsp"));
    for_each_tag(&[coll, sbsp], |label, group, view, address| {
        for header in bsp_headers(view, group, address) {
            let mut at = BLOCKS;
            let mut count = [0u32; 8];
            for (i, stride) in sim_collision::bsp::block::STRIDE.into_iter().enumerate() {
                let n = u32::from_le_bytes(header[8 * i..8 * i + 4].try_into().unwrap());
                let source = u32::from_le_bytes(header[8 * i + 4..8 * i + 8].try_into().unwrap());
                word(&mut mem, POLY_BSP + 8 * i as u32, n);
                if i < sim_collision::bsp::block::SURFACES {
                    continue;
                }
                let bytes = view.range(source, u64::from(n) * u64::from(stride)).unwrap();
                assert!((at as usize + bytes.len()) < mem.bytes.len(), "{label}: blocks fit");
                mem.bytes[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
                word(&mut mem, POLY_BSP + 8 * i as u32 + 4, at);
                count[i] = n;
                at = (at + bytes.len() as u32 + 15) & !15;
            }
            bsps += 1;
            for surface in 0..count[sim_collision::bsp::block::SURFACES] {
                let a = structure::Args { shape: POLY_SHAPE, transform: 0, bsp: POLY_BSP, flag: 0, owner: 0, surface, tail: 0 };
                mem.bytes[span.clone()].fill(0);
                let writes = {
                    let view = Flat(&mem);
                    let mut j = Journal::new(&view);
                    if structure::build(&mut j, &a, 0x1f80).is_err() {
                        declined += 1;
                        continue;
                    }
                    j.finish()
                };
                let mut expected = vec![0u8; span.len()];
                for (&w, &v) in &writes {
                    let off = (w as usize).checked_sub(span.start).filter(|&o| o + 4 <= span.len());
                    let off = off.unwrap_or_else(|| panic!("{label} surface {surface}: a store outside the shape at {w:#x}"));
                    expected[off..off + 4].copy_from_slice(&v.to_le_bytes());
                }
                let why = format!("{label} surface {surface}");
                run_builder(&mut code, &mut mem, &a, 0x1f80, &why);
                assert!(mem.bytes[span.clone()] == expected[..], "{why}: the shape differs");
                surfaces += 1;
            }
            mem.bytes[BLOCKS as usize..at as usize].fill(0);
        }
    });
    eprintln!("structure polygons: {surfaces} surfaces over {bsps} BSPs equal the lifted builder; {declined} declined");
    assert!(surfaces > 0 && declined == 0, "{declined} surfaces declined");
}

// The convex agent's process call `0x002eeb40` (sim_havok::convex,
// docs/engine/havok-convex.md): the separated-pair path against the
// lifted call.

use engine::sim_physics::convex_agent as convex_arm;
use sim_havok::convex::{self as convex, Fallback as ConvexDecline};

const CONVEX: u32 = 0x002e_eb40;
const CONVEX_AGENT: u32 = 0x0075_0000;
const CONVEX_A: u32 = 0x0075_1000;
const CONVEX_B: u32 = 0x0075_1100;
const CONVEX_MOTION_A: u32 = 0x0075_2000;
const CONVEX_MOTION_B: u32 = 0x0075_2100;
const CONVEX_MANAGER: u32 = 0x0075_3000;
const CONVEX_INPUT: u32 = 0x0075_4000;
const CONVEX_OUTPUT: u32 = 0x0075_5000;
const CONVEX_ARGS: [u32; 4] = [CONVEX_A, CONVEX_B, CONVEX_INPUT, CONVEX_OUTPUT];

fn convex_code(image: &Image) -> lifted::Original {
    let mut code = image.original();
    for f in [CONVEX, 0x3029d0] {
        code.prime(f);
    }
    code
}

/// One pair: both collidables' motions (two positions, an allowance in the
/// first's `w`), the agent's axis and bound, no manifold points. Bounds
/// near the reach land on both sides of zero.
fn convex_pair(r: &mut Rng, m: &mut FlatMemory) {
    word(m, CONVEX_A, 0x0076_0000 + r.below(0x100) * 16);
    word(m, CONVEX_B, 0x0076_8000 + r.below(0x100) * 16);
    for (collidable, motion) in [(CONVEX_A, CONVEX_MOTION_A), (CONVEX_B, CONVEX_MOTION_B)] {
        word(m, collidable + convex::COLLIDABLE_MOTION, motion);
        let from: [f32; 3] = std::array::from_fn(|_| float(r, -10.0, 10.0));
        put_lanes(m, motion + convex::MOTION_FROM, [from[0], from[1], from[2], float(r, 0.0, 0.2)]);
        let to: [f32; 3] = std::array::from_fn(|i| from[i] + float(r, -0.3, 0.3));
        put_lanes(m, motion + convex::MOTION_TO, [to[0], to[1], to[2], float(r, -1.0, 1.0)]);
    }
    let axis: [f32; 3] = std::array::from_fn(|_| float(r, -1.0, 1.0));
    put_lanes(m, CONVEX_AGENT + convex::field::AXIS, [axis[0], axis[1], axis[2], float(r, -1.0, 1.0)]);
    let bound = if r.below(4) == 0 { float(r, 0.0, 0.8) } else { float(r, 0.5, 3.0) };
    word(m, CONVEX_AGENT + convex::field::BOUND, bound.to_bits());
    word(m, CONVEX_AGENT + convex::field::MANAGER, CONVEX_MANAGER);
    word(m, CONVEX_AGENT + convex::field::POINTS, 0);
}

fn check_convex_regs(after: &runtime::CpuAbi, head: u32, why: &str) {
    assert_eq!(
        (after.eax, after.ecx, after.edx, after.esp),
        (CONVEX_AGENT + convex::field::POINTS, head, CONVEX_MANAGER, STACK + 0x14),
        "{why}: eax, ecx, edx, ret 0x10"
    );
    assert_eq!((after.ebx, after.ebp, after.esi, after.edi), (0x1111, 0x2222, 0x3333, 0x4444), "{why}");
}

/// Seeded separated pairs under every rounding mode: the bound the model
/// stores equals the lifted call's, byte for byte, and the registers too.
/// Pairs whose bound reaches zero are the closest-feature query's.
#[test]
fn separated_convex_pairs_match_the_lifted_process_call() {
    let Some((image, base)) = ram() else { return };
    let mut code = convex_code(image);
    let head = base.read_u32(MONITOR_HEAD).unwrap();
    let (mut answered, mut closest) = (0, 0);
    for seed in 0..512u64 {
        let mut r = Rng(0x2eeb_0000 + seed);
        let mut mem = base.clone();
        convex_pair(&mut r, &mut mem);
        let fpu = test_fpu(seed);
        let before = mem.clone();
        let view = Flat(&before);
        let mut j = Journal::new(&view);
        let args = convex::Args { agent: CONVEX_AGENT, a: CONVEX_A, b: CONVEX_B };
        match convex::process(&mut j, &args, fpu) {
            Ok(_) => answered += 1,
            Err(ConvexDecline::Closest) => {
                closest += 1;
                continue;
            }
            Err(f) => panic!("seed {seed}: declined {f:?}"),
        }
        let mut expected = before.clone();
        apply(&mut expected, j);
        let why = format!("seed {seed} {fpu:x?}");
        let after = run_this(&mut code, &mut mem, CONVEX, CONVEX_AGENT, &CONVEX_ARGS, fpu);
        check_convex_regs(&after, head, &why);
        clear_frame([&mut mem, &mut expected], 0x10);
        assert_eq!(first_difference(&mem, &expected), None, "{why}: first different byte");
    }
    assert!(answered > 256 && closest > 16, "{answered} answered, {closest} closest");
}

fn convex_cpu(fpu: Fpu) -> runtime::CpuAbi {
    let mut cpu = runtime::CpuAbi { ecx: CONVEX_AGENT, ebx: 0x1111, ebp: 0x2222, esi: 0x3333, edi: 0x4444, esp: STACK, mxcsr: fpu.mxcsr, ..runtime::CpuAbi::default() };
    cpu.x87.control = fpu.control;
    cpu
}

fn convex_stack(m: &mut FlatMemory) {
    for (i, v) in std::iter::once(EXIT).chain(CONVEX_ARGS).enumerate() {
        word(m, STACK + 4 * i as u32, v);
    }
}

/// The arm end to end: an audited call's prediction matches the lifted
/// answer, and an applied one leaves memory and the registers as it does.
#[test]
fn the_convex_arm_answers_as_the_lifted_process_call() {
    let Some((image, base)) = ram() else { return };
    let mut code = convex_code(image);
    let head = base.read_u32(MONITOR_HEAD).unwrap();
    let mut checked = 0;
    for seed in 0..64u64 {
        let mut r = Rng(0x2eec_0000 + seed);
        let mut mem = base.clone();
        convex_pair(&mut r, &mut mem);
        convex_stack(&mut mem);
        let fpu = Fpu { mxcsr: 0x1f80 | ((seed % 4) as u32) << 13, control: 0x027f };
        let cpu = convex_cpu(fpu);
        let prediction = match convex_arm::capture(&cpu, &guest(&mem)) {
            Ok(p) => p,
            Err(ConvexDecline::Closest) => continue,
            Err(f) => panic!("seed {seed}: declined {f:?}"),
        };
        let mut lifted = mem.clone();
        let after = run_this(&mut code, &mut lifted, CONVEX, CONVEX_AGENT, &CONVEX_ARGS, fpu);
        assert_eq!(lifted.read_u32(CONVEX_AGENT + convex::field::BOUND), Some(prediction.bound), "seed {seed}");
        assert!(prediction.matches(&after, &guest(&lifted)), "seed {seed}: the audit matches the lifted answer");
        let mut applied = guest(&mem);
        let mut regs = cpu;
        assert!(prediction.apply(&mut regs, &mut applied), "seed {seed}");
        check_convex_regs(&regs, head, &format!("seed {seed}"));
        let start = applied.mappings()[0].physical_start as usize;
        let mut model = FlatMemory { bytes: applied.physical_ram()[start..start + RAM].to_vec(), ..base.clone() };
        clear_frame([&mut lifted, &mut model], 0x10);
        assert_eq!(first_difference(&lifted, &model), None, "seed {seed}: first different byte");
        checked += 1;
    }
    assert!(checked > 24, "{checked} answered");
}

/// A bound at zero, a manifold with points, an unmasked MXCSR, the monitor
/// stream and an unaligned motion: declined with nothing written, each
/// reason counted.
#[test]
fn convex_calls_the_model_does_not_cover_are_declined_and_counted() {
    let Some((_, base)) = ram() else { return };
    let cases: [(ConvexDecline, fn(&mut FlatMemory, &mut runtime::CpuAbi)); 5] = [
        (ConvexDecline::Closest, |m, _| word(m, CONVEX_AGENT + convex::field::BOUND, 0)),
        (ConvexDecline::Points, |m, _| word(m, CONVEX_AGENT + convex::field::POINTS, 2)),
        (ConvexDecline::Float, |_, cpu| cpu.mxcsr = 0x1f00),
        (ConvexDecline::Monitor, |m, _| word(m, MONITOR_END, 0x0060_0900)),
        (ConvexDecline::Memory, |m, _| word(m, CONVEX_A + convex::COLLIDABLE_MOTION, CONVEX_MOTION_A + 4)),
    ];
    for (reason, spoil) in cases {
        let mut r = Rng(0xdec5_0000 + reason as u64);
        let mut mem = base.clone();
        convex_pair(&mut r, &mut mem);
        // Far apart: separated whatever the motions.
        word(&mut mem, CONVEX_AGENT + convex::field::BOUND, 100.0f32.to_bits());
        convex_stack(&mut mem);
        let mut cpu = convex_cpu(Fpu { mxcsr: 0x1f80, control: 0x027f });
        spoil(&mut mem, &mut cpu);
        let mut g = guest(&mem);
        let index = ConvexDecline::ALL.iter().position(|f| *f == reason).unwrap();
        let before = convex::fallbacks()[index].1;
        let declined = parity::status()[8].3;
        assert!(matches!(parity::convex_enter(&mut cpu, &mut g), Step::Guest), "{reason:?}");
        assert!(convex::fallbacks()[index].1 > before, "{reason:?} counted");
        assert!(parity::status()[8].3 > declined, "{reason:?} counted on the arm");
        assert_eq!(cpu.esp, STACK, "{reason:?}: the frame is the guest's");
        let start = g.mappings()[0].physical_start as usize;
        assert!(g.physical_ram()[start..start + RAM] == mem.bytes[..], "{reason:?}: nothing written");
    }
}

// The closest-feature query's leaves (sim_havok::gjk,
// docs/engine/havok-convex.md, "The query path") against the lifted
// functions, each seeded under every rounding mode.

use sim_havok::gjk;

const GJK_STATE: u32 = 0x0077_0000;
const GJK_IN: u32 = 0x0077_1000;
const GJK_OUT: u32 = 0x0077_2000;

fn gjk_code(image: &Image) -> lifted::Original {
    let mut code = image.original();
    for f in [0x2da4d0, 0x2da6e0, 0x303360, 0x308590, 0x3032c0, 0x303540, 0x303110, 0x3037a0] {
        code.prime(f);
    }
    code
}

/// A float that is sometimes zero or an exact small integer, so equal,
/// parallel and degenerate cases come up.
fn coord(r: &mut Rng) -> f32 {
    match r.below(8) {
        0 => 0.0,
        1 => (r.below(5) as f32) - 2.0,
        _ => float(r, -4.0, 4.0),
    }
}

fn seed_vector(r: &mut Rng, m: &mut FlatMemory, at: u32) -> [f32; 4] {
    let v = [coord(r), coord(r), coord(r), coord(r)];
    put_lanes(m, at, v);
    v
}

/// Run the model on a journal over `before`, then the lifted function;
/// the bytes outside the frame must agree. Returns the lifted registers.
fn gjk_compare(
    code: &mut lifted::Original,
    mem: &mut FlatMemory,
    eip: u32,
    this: u32,
    args: &[u32],
    fpu: Fpu,
    model: impl FnOnce(&mut Journal) -> Result<(), sim_havok::convex::Fallback>,
    why: &str,
) -> runtime::CpuAbi {
    let before = mem.clone();
    let view = Flat(&before);
    let mut j = Journal::new(&view);
    model(&mut j).unwrap_or_else(|f| panic!("{why}: declined {f:?}"));
    let mut expected = before.clone();
    apply(&mut expected, j);
    let after = run_this(code, mem, eip, this, args, fpu);
    clear_frame([&mut *mem, &mut expected], 4 * args.len() as u32);
    assert_eq!(first_difference(mem, &expected), None, "{why}: first different byte");
    after
}

#[test]
fn the_query_transforms_match_the_lifted_transforms() {
    let Some((image, base)) = ram() else { return };
    let mut code = gjk_code(image);
    for seed in 0..256u64 {
        let mut r = Rng(0x2da4_0000 + seed);
        let mut mem = base.clone();
        for i in 0..8 {
            seed_vector(&mut r, &mut mem, GJK_IN + 16 * i);
        }
        let fpu = test_fpu(seed);
        let (a, b) = (GJK_IN, GJK_IN + 0x40);
        // Seed 3 in 16 writes over its own A, as an in-place caller would.
        let out = if seed % 16 == 3 { a } else { GJK_OUT };
        let why = format!("relative seed {seed} {fpu:x?}");
        let after = gjk_compare(&mut code, &mut mem, 0x2da4d0, out, &[a, b], fpu, |j| gjk::relative(j, out, a, b, fpu), &why);
        assert_eq!(after.esp, STACK + 12, "{why}: ret 8");
        let why = format!("rotate seed {seed} {fpu:x?}");
        let after = gjk_compare(&mut code, &mut mem, 0x2da6e0, GJK_OUT + 0x100, &[a, b], fpu, |j| gjk::rotate(j, GJK_OUT + 0x100, a, b, fpu), &why);
        assert_eq!(after.esp, STACK + 12, "{why}: ret 8");
    }
}

#[test]
fn the_triangle_test_matches_the_lifted_test() {
    let Some((image, base)) = ram() else { return };
    let mut code = gjk_code(image);
    let mut masks = [0u32; 8];
    let mut flipped = 0;
    for seed in 0..512u64 {
        let mut r = Rng(0x3033_0000 + seed);
        let mut mem = base.clone();
        let (p, tri) = (GJK_IN, GJK_IN + 0x100);
        seed_vector(&mut r, &mut mem, p);
        for i in 0..6 {
            seed_vector(&mut r, &mut mem, tri + 16 * i);
        }
        for off in [gjk::state::SUPPORT_A, gjk::state::SUPPORT_B, gjk::state::NORMAL, gjk::state::SIDES] {
            seed_vector(&mut r, &mut mem, GJK_STATE + off);
        }
        let orient = if seed % 4 == 0 { 0 } else { 1 + r.below(3) };
        let fpu = test_fpu(seed);
        let why = format!("seed {seed} orient {orient} {fpu:x?}");
        let mut sides = None;
        let after = gjk_compare(&mut code, &mut mem, 0x303360, GJK_STATE, &[p, tri, orient], fpu, |j| {
            sides = Some(gjk::triangle(j, GJK_STATE, p, tri, orient, fpu)?);
            Ok(())
        }, &why);
        let sides = sides.unwrap();
        assert_eq!((after.eax, after.edx, after.esp), (sides.mask, sides.edx, STACK + 16), "{why}: eax, edx, ret 0xc");
        masks[sides.mask as usize] += 1;
        if sides.edx != tri {
            flipped += 1;
        }
    }
    assert!(masks.iter().filter(|&&n| n > 0).count() >= 6 && flipped > 4, "{masks:?}, {flipped} flipped");
}

#[test]
fn closest_segment_points_match_the_lifted_solve() {
    let Some((image, base)) = ram() else { return };
    let mut code = gjk_code(image);
    let mut seen = std::collections::BTreeMap::new();
    for seed in 0..1024u64 {
        let mut r = Rng(0x3085_0000 + seed);
        let mut mem = base.clone();
        let [p1, d1, p2, d2] = [0, 1, 2, 3].map(|i| GJK_IN + 16 * i);
        seed_vector(&mut r, &mut mem, p1);
        let dir = seed_vector(&mut r, &mut mem, d1);
        seed_vector(&mut r, &mut mem, p2);
        seed_vector(&mut r, &mut mem, d2);
        match seed % 8 {
            // Parallel, antiparallel and degenerate second segments.
            1 => put_lanes(&mut mem, d2, dir.map(|x| 2.0 * x)),
            2 => put_lanes(&mut mem, d2, dir.map(|x| -x)),
            3 => put_lanes(&mut mem, d2, [0.0; 4]),
            _ => {}
        }
        let fpu = test_fpu(seed);
        let why = format!("seed {seed} {fpu:x?}");
        let mut closest = None;
        let after = gjk_compare(&mut code, &mut mem, 0x308590, 0, &[p1, d1, p2, d2, GJK_OUT], fpu, |j| {
            closest = Some(gjk::segments(j, p1, d1, p2, d2, GJK_OUT, fpu)?);
            Ok(())
        }, &why);
        let closest = closest.unwrap();
        assert_eq!((after.eax, after.ecx, after.edx, after.esp), (closest.flags, closest.flags, closest.distance, STACK + 4), "{why}: eax, ecx, edx, ret");
        *seen.entry(closest.flags).or_insert(0) += 1;
    }
    assert!(seen.len() >= 6, "{seen:?}");
}

#[test]
fn the_query_contact_matches_the_lifted_contact() {
    let Some((image, base)) = ram() else { return };
    let mut code = gjk_code(image);
    let mut cases = [0u32; 3];
    for seed in 0..256u64 {
        let mut r = Rng(0x3032_0000 + seed);
        let mut mem = base.clone();
        let sep = GJK_IN;
        seed_vector(&mut r, &mut mem, sep);
        for off in [gjk::state::SUPPORT_A, gjk::state::SUPPORT_B, gjk::state::MIDPOINT] {
            seed_vector(&mut r, &mut mem, GJK_STATE + off);
        }
        let case = r.below(3);
        let (na, nb) = [(1, 1 + r.below(3)), (2 + r.below(2), 1), (2 + r.below(2), 2 + r.below(2))][case as usize];
        word(&mut mem, GJK_STATE + gjk::state::COUNT_A, na);
        word(&mut mem, GJK_STATE + gjk::state::COUNT_B, nb);
        let fpu = test_fpu(seed);
        let why = format!("seed {seed} ({na}, {nb}) {fpu:x?}");
        let mut edx = None;
        let after = gjk_compare(&mut code, &mut mem, 0x3032c0, GJK_STATE, &[0x0077_3000, sep, GJK_OUT], fpu, |j| {
            edx = Some(gjk::contact(j, GJK_STATE, sep, GJK_OUT, fpu)?);
            Ok(())
        }, &why);
        assert_eq!((after.eax, after.edx, after.esp), (GJK_OUT, edx.unwrap(), STACK + 16), "{why}: eax, edx, ret 0xc");
        cases[case as usize] += 1;
    }
    assert!(cases.iter().all(|&n| n > 32), "{cases:?}");
}

/// Seeded tetrahedra and points, some at the newest vertex and some far
/// past one face: the face `0x00303540` names (through `0x00303110` when
/// no face dominates) and ecx equal the lifted function's, and it writes
/// nothing outside its frame.
#[test]
fn the_tetrahedron_face_matches_the_lifted_choice() {
    let Some((image, base)) = ram() else { return };
    let mut code = gjk_code(image);
    let mut seen = std::collections::BTreeMap::new();
    for seed in 0..1024u64 {
        let mut r = Rng(0x3035_0000 + seed);
        let mut mem = base.clone();
        let (verts, p) = (GJK_IN, GJK_IN + 0x100);
        for i in 0..4 {
            seed_vector(&mut r, &mut mem, verts + 16 * i);
        }
        let point = seed_vector(&mut r, &mut mem, p);
        match seed % 8 {
            0 => {
                let v3: [f32; 4] = std::array::from_fn(|i| f32::from_bits(mem.read_u32(verts + 0x30 + 4 * i as u32).unwrap()));
                put_lanes(&mut mem, p, v3);
            }
            1 => put_lanes(&mut mem, p, point.map(|x| 8.0 * x)),
            _ => {}
        }
        let s = [1.0f32, -1.0, float(&mut r, -2.0, 2.0)][r.below(3) as usize].to_bits();
        word(&mut mem, STACK, EXIT);
        word(&mut mem, STACK + 4, s);
        let fpu = test_fpu(seed);
        let why = format!("seed {seed} {fpu:x?}");
        let face = gjk::tetrahedron(&Journal::new(&Flat(&mem)), verts, p, s, fpu).unwrap_or_else(|f| panic!("{why}: declined {f:?}"));
        let before = mem.clone();
        let mut cpu = Cpu { gpr: [verts, 0, 0, p, STACK, 0x2222, 0x3333, 0x4444], eip: 0x303540, ..Cpu::default() };
        cpu.mxcsr = fpu.mxcsr;
        cpu.x87.control = fpu.control;
        run(&mut code, &mut cpu, &mut mem);
        let after = abi_cpu(&cpu);
        assert_eq!((after.eax, after.ecx, after.esp), (face.eax, face.ecx, STACK + 4), "{why}: eax, ecx, ret");
        assert_eq!((after.ebx, after.edi), (p, 0x4444), "{why}");
        let mut expected = before;
        clear_frame([&mut mem, &mut expected], 4);
        assert_eq!(first_difference(&mem, &expected), None, "{why}: first different byte");
        *seen.entry((face.eax, face.ecx == verts + 0x30)).or_insert(0) += 1;
    }
    assert!(seen.len() >= 6, "{seen:?}");
}

/// Seeded segments against seeded triangles, fresh (the triangle's own
/// normal) or not (the state's normal and sides, the fresh byte sometimes
/// only above the low one), a quarter of them through the triangle's
/// middle: eax, ecx and edx equal the lifted function's, and the vertices,
/// counts and state words it moves match byte for byte outside its frame.
#[test]
fn the_edge_case_matches_the_lifted_case() {
    let Some((image, base)) = ram() else { return };
    let mut code = gjk_code(image);
    let mut seen = std::collections::BTreeMap::new();
    for seed in 0..1024u64 {
        let mut r = Rng(0x3037_0000 + seed);
        let mut mem = base.clone();
        let (seg, tri, counts) = (GJK_IN, GJK_IN + 0x100, GJK_IN + 0x200);
        for i in 0..6 {
            seed_vector(&mut r, &mut mem, seg + 16 * i);
        }
        let t: Vec<[f32; 4]> = (0..7).map(|i| seed_vector(&mut r, &mut mem, tri + 16 * i)).collect();
        for off in [gjk::state::SUPPORT_A, gjk::state::SUPPORT_B, gjk::state::NORMAL, gjk::state::SIDES] {
            seed_vector(&mut r, &mut mem, GJK_STATE + off);
        }
        if seed % 4 == 1 {
            // Through the middle, with the state's normal the triangle's own.
            let mid: [f32; 4] = std::array::from_fn(|k| (t[0][k] + t[1][k] + t[2][k]) / 3.0);
            let dir = [coord(&mut r), coord(&mut r), 1.0 + coord(&mut r).abs(), 0.0];
            put_lanes(&mut mem, seg, std::array::from_fn(|k| mid[k] + dir[k]));
            put_lanes(&mut mem, seg + 0x10, std::array::from_fn(|k| mid[k] - dir[k]));
            let sub = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] { std::array::from_fn(|k| a[k] - b[k]) };
            let (u, w) = (sub(t[1], t[2]), sub(t[0], t[2]));
            put_lanes(&mut mem, GJK_STATE + gjk::state::NORMAL, [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0], 0.0]);
        }
        word(&mut mem, counts, 2);
        word(&mut mem, counts + 4, 3);
        let fresh = [0, 0x100, 1, 0x5a][(seed % 8 / 2) as usize];
        let feature = r.u32();
        let args = [seg, tri, counts, counts + 4, fresh, feature];
        let fpu = test_fpu(seed);
        let why = format!("seed {seed} fresh {fresh:#x} {fpu:x?}");
        let mut answer = None;
        let after = gjk_compare(&mut code, &mut mem, 0x3037a0, GJK_STATE, &args, fpu, |j| {
            answer = Some(gjk::edge(j, GJK_STATE, STACK, seg, tri, counts, counts + 4, fresh, feature, fpu)?);
            Ok(())
        }, &why);
        let edge = answer.unwrap();
        assert_eq!((after.eax, after.ecx, after.edx, after.esp), (edge.eax, edge.ecx, edge.edx, STACK + 0x1c), "{why}: eax, ecx, edx, ret 0x18");
        assert_eq!((after.ebx, after.esi, after.edi), (0x1111, 0x3333, 0x4444), "{why}");
        *seen.entry((fresh & 0xff != 0, edge.eax)).or_insert(0) += 1;
    }
    assert!(seen.len() >= 5, "{seen:?}");
// The convex shapes' support virtuals (sim_havok::shape_support,
// docs/engine/havok-convex.md, "Support virtuals") against the lifted
// functions: 1024 seeds each under every rounding mode, the x87 state
// sometimes holding a value and a sticky flag at the entry.

use sim_havok::shape_support::{self as shape, Exit};
use x87::abi::X87Abi as X87State;

const SHAPE: u32 = 0x0077_3000;
const SHAPE_CHILD: u32 = 0x0077_3100;
const SHAPE_DIR: u32 = 0x0077_3200;
const SHAPE_OUT: u32 = 0x0077_3300;
/// Halo's tag capsule vtable (type 7), the pill a biped wrapper holds.
const CAPSULE_VTABLE: u32 = 0x0045_5500;
const SUPPORT_EDX: u32 = 0x5555;

fn support_code(image: &Image) -> lifted::Original {
    let mut code = image.original();
    for f in [shape::CAPSULE_SUPPORT, shape::BIPED_SUPPORT, shape::POLYGON_SUPPORT] {
        code.prime(f);
    }
    code
}

fn support_x87(r: &mut Rng, seed: u64) -> X87State {
    let mut x = X87State::default();
    x.control = test_fpu(seed).control;
    if seed % 3 == 1 {
        x87::abi::ld_f32(&mut x, float(r, -2.0, 2.0).to_bits());
    }
    if seed % 7 == 2 {
        x.status |= 0x0020;
    }
    x
}

/// A capsule at `at`: the vtable, two ends with their radius in `w`; in
/// one seed of 8 both ends the same, so every direction is perpendicular.
fn seed_capsule(r: &mut Rng, m: &mut FlatMemory, at: u32, seed: u64) {
    word(m, at, CAPSULE_VTABLE);
    let a = seed_vector(r, m, at + 0x10);
    let b = seed_vector(r, m, at + 0x20);
    put_lanes(m, at + 0x20, if seed % 8 == 2 { a } else { b });
}

/// A direction: sometimes zero, sometimes with a NaN lane.
fn seed_direction(r: &mut Rng, m: &mut FlatMemory, seed: u64) {
    let mut d = seed_vector(r, m, SHAPE_DIR);
    match seed % 8 {
        0 => d = [0.0, 0.0, 0.0, d[3]],
        1 => d[r.below(3) as usize] = f32::NAN,
        _ => {}
    }
    put_lanes(m, SHAPE_DIR, d);
}

/// Run the model on a journal over `mem`, then the lifted function at
/// `eip` (ecx `this`, the direction and the output on the stack): the
/// registers it leaves, the MXCSR, the x87 registers, status and tags,
/// and every byte outside its frame must agree.
fn support_compare(
    code: &mut lifted::Original,
    mem: &mut FlatMemory,
    eip: u32,
    seed: u64,
    x87: X87State,
    model: impl FnOnce(&mut Journal, &mut u32, &mut X87State) -> Result<Exit, sim_havok::convex::Fallback>,
    why: &str,
) -> FlatMemory {
    let fpu = test_fpu(seed);
    let before = mem.clone();
    let view = Flat(&before);
    let mut j = Journal::new(&view);
    let (mut mxcsr, mut x) = (fpu.mxcsr, x87);
    let exit = model(&mut j, &mut mxcsr, &mut x).unwrap_or_else(|f| panic!("{why}: declined {f:?}"));
    let mut expected = before.clone();
    apply(&mut expected, j);
    for (i, v) in [EXIT, SHAPE_DIR, SHAPE_OUT].into_iter().enumerate() {
        word(mem, STACK + 4 * i as u32, v);
    }
    let mut cpu = Cpu { gpr: [0, SHAPE, SUPPORT_EDX, 0x1111, STACK, 0x2222, 0x3333, 0x4444], eip, ..Cpu::default() };
    cpu.mxcsr = fpu.mxcsr;
    cpu.x87 = ir::interp::X87 {
        st: x87.st,
        control: x87.control,
        status: x87.status,
        tag: x87.tag,
        opcode: x87.opcode,
        instruction_pointer: x87.instruction_pointer,
        data_pointer: x87.data_pointer,
        instruction_selector: x87.instruction_selector,
        data_selector: x87.data_selector,
    };
    run(code, &mut cpu, mem);
    let after = abi_cpu(&cpu);
    assert_eq!((after.eax, after.ecx, after.edx), (exit.eax, exit.ecx, exit.edx), "{why}: eax, ecx, edx");
    assert_eq!((after.ebx, after.ebp, after.esi, after.edi, after.esp), (0x1111, 0x2222, 0x3333, 0x4444, STACK + 12), "{why}: kept, ret 8");
    assert_eq!(after.mxcsr, mxcsr, "{why}: mxcsr");
    assert_eq!((after.x87.st, after.x87.status, after.x87.tag), (x.st, x.status, x.tag), "{why}: x87");
    clear_frame([&mut *mem, &mut expected], 8);
    assert_eq!(first_difference(mem, &expected), None, "{why}: first different byte");
    expected
}

fn capsule_model(j: &mut Journal, this: u32, m: &mut u32, x: &mut X87State) -> Result<Exit, sim_havok::convex::Fallback> {
    shape::capsule(j, this, SHAPE_DIR, SHAPE_OUT, m, x).map(|eax| Exit { eax, ecx: SHAPE_OUT, edx: SUPPORT_EDX })
}

#[test]
fn the_capsule_support_matches_the_lifted_support() {
    let Some((image, base)) = ram() else { return };
    let mut code = support_code(image);
    let mut ends = [0u32; 2];
    for seed in 0..1024u64 {
        let mut r = Rng(0x2ddd_0000 + seed);
        let mut mem = base.clone();
        seed_capsule(&mut r, &mut mem, SHAPE, seed);
        seed_direction(&mut r, &mut mem, seed);
        let x87 = support_x87(&mut r, seed);
        let why = format!("capsule seed {seed}");
        let out = support_compare(&mut code, &mut mem, shape::CAPSULE_SUPPORT, seed, x87, |j, m, x| capsule_model(j, SHAPE, m, x), &why);
        ends[(out.read_u32(SHAPE_OUT + 0xc).unwrap() & 0x10 != 0) as usize] += 1;
    }
    assert!(ends.iter().all(|&n| n > 200), "{ends:?}");
}

#[test]
fn the_biped_support_matches_the_lifted_support() {
    let Some((image, base)) = ram() else { return };
    let mut code = support_code(image);
    let mut moved = [0u32; 2];
    for seed in 0..1024u64 {
        let mut r = Rng(0x2dbd_0000 + seed);
        let mut mem = base.clone();
        seed_capsule(&mut r, &mut mem, SHAPE_CHILD, seed / 8);
        word(&mut mem, SHAPE + shape::biped_field::CHILD, SHAPE_CHILD);
        let offset = seed_vector(&mut r, &mut mem, SHAPE + shape::biped_field::OFFSET);
        if seed % 16 == 5 {
            put_lanes(&mut mem, SHAPE + shape::biped_field::OFFSET, [0.0, 0.0, 0.0, offset[3]]);
        }
        seed_direction(&mut r, &mut mem, seed);
        let x87 = support_x87(&mut r, seed);
        let why = format!("biped seed {seed}");
        let child = mem.clone();
        let out = support_compare(
            &mut code,
            &mut mem,
            shape::BIPED_SUPPORT,
            seed,
            x87,
            |j, m, x| shape::biped(j, SHAPE, SHAPE_DIR, SHAPE_OUT, STACK, m, x, |j, at, _, m, x| capsule_model(j, at, m, x)),
            &why,
        );
        let end = SHAPE_CHILD + 0x10 + (out.read_u32(SHAPE_OUT + 0xc).unwrap() & 0x10);
        let xyz = |m: &FlatMemory, at: u32| [0, 4, 8].map(|k| m.read_u32(at + k).unwrap());
        moved[(xyz(&out, SHAPE_OUT) != xyz(&child, end)) as usize] += 1;
    }
    assert!(moved.iter().all(|&n| n > 200), "{moved:?}");
}

#[test]
fn the_polygon_support_matches_the_lifted_support() {
    let Some((image, base)) = ram() else { return };
    let mut code = support_code(image);
    let mut chosen = std::collections::BTreeMap::new();
    for seed in 0..1024u64 {
        let mut r = Rng(0x1ee3_0000 + seed);
        let mut mem = base.clone();
        let count = match seed % 8 {
            0 => 0,
            1 => u32::MAX,
            _ => 1 + r.below(8) as u32,
        };
        word(&mut mem, SHAPE + 0x14, count);
        let vertex = |i: u32| SHAPE + 0x20 + 16 * i;
        let first = seed_vector(&mut r, &mut mem, vertex(0));
        for i in 1..9 {
            seed_vector(&mut r, &mut mem, vertex(i));
        }
        // A tie with vertex 0 at the end of the ring.
        if seed % 8 == 2 {
            put_lanes(&mut mem, vertex(count - 1), first);
        }
        seed_direction(&mut r, &mut mem, seed);
        let x87 = support_x87(&mut r, seed);
        let why = format!("polygon seed {seed}");
        let out = support_compare(&mut code, &mut mem, shape::POLYGON_SUPPORT, seed, x87, |j, m, _| shape::polygon(j, SHAPE, SHAPE_DIR, SHAPE_OUT, m), &why);
        let words = |at: u32| [0, 4, 8, 12].map(|k| out.read_u32(at + k).unwrap());
        let index = (0..9).position(|i| words(vertex(i)) == words(SHAPE_OUT));
        *chosen.entry(index).or_insert(0) += 1;
    }
    assert!(chosen.len() >= 6, "{chosen:?}");
}
