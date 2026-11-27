//! The island solve `0x0031c100` (`sim_havok::island`, the port
//! `HAVOK_ISLAND_SOLVE`) against the lifted original
//! (docs/engine/havok-step.md, "The island solve").
//!
//! Seeded islands are laid out in the image's RAM. The lifted entry runs to
//! its return, stepping into every callee; a constraint's `vt[+0x24]` is a
//! synthetic address the harness answers with `ret 8`, so the frame runs
//! with no rows. The model's persistent writes (`motion+0x08`, the pair
//! words) must leave memory as the lifted entry does, outside the solver
//! buffer (scratch) and the dead stack, and `eax` must match. Islands the
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
use sim_havok::mem::{Journal, Words};
use sim_havok::motion::{self, Accumulator, SolverClass};

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
    for f in [0x31c100, 0x31c940, 0x31a770, 0x31a6a0, 0x319430] {
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
                .map(|i| Body { motion: MOTION + 0x200 * i, class: SolverClass::Fixed, accumulator: motion::fixed_accumulator() })
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

/// An unported builder and an island too large for the buffer: the port
/// declines, writes nothing and counts why.
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
