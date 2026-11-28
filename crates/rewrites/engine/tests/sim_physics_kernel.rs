//! The solver kernel (`sim_havok::solver::Stream::solve`) against the
//! register-level port of the island solver (`0x0031c940`, the oracle
//! `engine::sim_physics::ORACLES`' `HAVOK_SOLVER`), bit for bit
//! (docs/engine/sim-rewrite-havok.md section 9.3).
//!
//! A seeded generator builds a typed `Stream`. The test encodes the same
//! stream as the guest's schema, rows, accumulators and impulse array in
//! the image's RAM, runs the oracle to its return (the two row-advance
//! callees answered as the guest's own), and reads the working set back.
//! Results compare as words, never as floats, so a NaN payload and a
//! signed zero count. The owner's image supplies the dispatch tables the
//! oracle reads; without it the tests return.
#![cfg(feature = "differential")]

use guest_layout::points;
use engine::sim_physics::{ORACLES, PORTS, Port, supported};
use lifted::image::{Image, Rng};
use runtime::{CpuAbi, Guest, abi, guest::Permissions};
use sim_havok::solver::{Body, Command, Constraint, Limit, Row, Stream, Vector};

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = 0xffff_fffc;
const PARAMS: u32 = 0x0060_5000;
const SCHEMA: u32 = 0x0060_5100;
const ROWS: u32 = 0x0060_6000;
const BODIES: u32 = 0x0061_0000;
const IMPULSES: u32 = 0x0061_1000;
const OUT: u32 = 0x0061_2000;
/// The image's zero constant, the solver's comparand.
const ZERO: u32 = 0x0045_dbd8;
const READY: u32 = 0x0048_0c60;
/// The x87 control word `game_tick` leaves: 53-bit precision, nearest.
const CONTROL: u16 = 0x027f;

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// Every constraint form.
    All,
    /// Bodies with no inverse mass: an impulse changes no velocity, so the
    /// per-iteration sums and the right-hand-side correction are what moves.
    Correction,
}

struct Plan {
    stream: Stream,
    iterations: u32,
    rhs: u32,
    velocity: u32,
    bias: u32,
    export: u32,
}

fn between(r: &mut Rng, lo: f32, hi: f32) -> f32 {
    lo + (r.u32() >> 8) as f32 / (1u32 << 24) as f32 * (hi - lo)
}

fn bits(r: &mut Rng, lo: f32, hi: f32) -> u32 {
    between(r, lo, hi).to_bits()
}

/// A value, with signed zeros and denormals mixed in.
fn val(r: &mut Rng, lo: f32, hi: f32) -> f32 {
    match r.u32() % 16 {
        0 => 0.0,
        1 => -0.0,
        2 => 1.0e-40,
        3 => -1.0e-40,
        _ => between(r, lo, hi),
    }
}

fn pack(lanes: [u32; 4]) -> Vector {
    lanes.iter().rev().fold(0, |p, &x| p << 32 | u128::from(x))
}

fn vec4(r: &mut Rng, w: f32) -> Vector {
    let lanes = [val(r, -1.0, 1.0), val(r, -1.0, 1.0), val(r, -1.0, 1.0), w];
    pack(lanes.map(f32::to_bits))
}

fn linear_row(r: &mut Rng) -> Row {
    let rhs = val(r, -3.0, 6.0);
    let inverse = between(r, 0.05, 2.0);
    let diagonal = between(r, 0.05, 2.0);
    Row {
        normal: Some(vec4(r, rhs)),
        angular_a: vec4(r, inverse),
        angular_b: vec4(r, diagonal),
        impulse: val(r, -1.0, 1.0).to_bits(),
    }
}

fn angular_row(r: &mut Rng) -> Row {
    let rhs = val(r, -3.0, 6.0);
    let inverse = between(r, 0.05, 2.0);
    Row {
        normal: None,
        angular_a: vec4(r, inverse),
        angular_b: vec4(r, rhs),
        impulse: val(r, -1.0, 1.0).to_bits(),
    }
}

fn command(r: &mut Rng, rows: &mut Vec<Row>, bodies: [usize; 2], rhs: u32, velocity: u32) -> Command {
    let first_row = rows.len();
    let kind = r.u32() % 12;
    let constraint = match kind {
        0 => Constraint::Single(Limit::Positive),
        1 | 2 => Constraint::Single(Limit::Free),
        3 | 4 => Constraint::Single(Limit::Capped { magnitude: bits(r, 0.05, 5.0) }),
        5 | 6 => Constraint::Single(Limit::Bounded { high: bits(r, 0.02, 9.0), low: bits(r, -9.0, -0.02) }),
        7 | 8 => Constraint::Single(Limit::Range {
            high: bits(r, -1.0, 2.0),
            low: bits(r, -2.0, 1.0),
            angular_scale: if kind == 8 { bits(r, 0.2, 1.5) } else { 0 },
        }),
        9 => Constraint::PositivePair { cross: bits(r, -0.9, 0.9) },
        10 => Constraint::Friction { cross: bits(r, -0.9, 0.9), limit: bits(r, 0.05, 6.0), twist_scale: None },
        _ => Constraint::Friction {
            cross: bits(r, -0.9, 0.9),
            limit: bits(r, 0.05, 6.0),
            twist_scale: Some(bits(r, 0.2, 1.5)),
        },
    };
    match kind {
        0 | 1 | 3 | 5 | 7 => rows.push(linear_row(r)),
        2 | 4 | 6 | 8 => rows.push(angular_row(r)),
        9 | 10 => rows.extend([linear_row(r), linear_row(r)]),
        _ => rows.extend([linear_row(r), linear_row(r), angular_row(r)]),
    }
    Command { constraint, first_row, bodies, rhs_scale: rhs, velocity_scale: velocity }
}

fn plan(r: &mut Rng, shape: Shape) -> Plan {
    let count = 2 + r.u32() as usize % 3;
    let mut stream = Stream::default();
    for _ in 0..count {
        let inverse = if shape == Shape::Correction {
            pack([0; 4])
        } else {
            pack([bits(r, 0.05, 1.0), bits(r, 0.05, 1.0), bits(r, 0.05, 1.0), bits(r, 0.05, 1.0)])
        };
        let (wl, wa) = (val(r, -1.0, 1.0), val(r, -1.0, 1.0));
        let (linear, angular) = (vec4(r, wl), vec4(r, wa));
        stream.bodies.push(Body { linear, angular, inverse_mass: inverse, linear_sum: 0, angular_sum: 0 });
    }
    let iterations = match shape {
        Shape::All => 1 + r.u32() % 3,
        Shape::Correction => 2 + r.u32() % 3,
    };
    let (rhs, velocity) = (bits(r, 0.5, 1.5), bits(r, 0.05, 1.0));
    let mut previous = [0, 1];
    for i in 0..1 + r.u32() % 6 {
        // Consecutive commands on one pair of bodies share a header, and a
        // second one-sided pair takes the guest's alternate path.
        let bodies = if i > 0 && r.u32() % 2 == 0 {
            previous
        } else {
            [r.u32() as usize % count, r.u32() as usize % count]
        };
        previous = bodies;
        let own = r.u32() % 4 == 0;
        let (c_rhs, c_velocity) = if own { (bits(r, 0.3, 1.7), bits(r, 0.0, 1.2)) } else { (rhs, velocity) };
        let cmd = command(r, &mut stream.rows, bodies, c_rhs, c_velocity);
        stream.commands.push(cmd);
    }
    Plan { stream, iterations, rhs, velocity, bias: bits(r, 0.1, 1.0), export: bits(r, 0.5, 1.5) }
}

fn image_ram() -> Option<&'static [u8]> {
    let image = Image::get()?;
    Some(&image.ram.bytes[..RAM.min(image.ram.bytes.len())])
}

fn rd(ram: &[u8], at: u32) -> u32 {
    u32::from_le_bytes(ram[at as usize..at as usize + 4].try_into().unwrap())
}

/// Schema ids and correction passes come from the image's own tables.
fn handler_id(ram: &[u8], handler: u32) -> u32 {
    (0..=16u32).find(|&i| rd(ram, 0x31e6a4 + 4 * i) == handler).unwrap_or_else(|| panic!("no id for {handler:#x}"))
}

fn pass_id(ram: &[u8], form: u32) -> u32 {
    (0..=6u32).find(|&i| rd(ram, 0x31e6e8 + 4 * i) == form).unwrap_or_else(|| panic!("no pass for {form:#x}"))
}

fn done_id(ram: &[u8]) -> u32 {
    (0..=16u32)
        .find(|&i| rd(ram, 0x31e704 + 4 * u32::from(ram[(0x31e724 + i) as usize])) == 0x31e696)
        .expect("terminator id")
}

fn tag(ram: &[u8], handler: u32, size: u32, correction: u32) -> u32 {
    handler_id(ram, handler) | size << 8 | pass_id(ram, correction) << 16
}

fn get(g: &Guest, at: u32) -> Vector {
    (0..4).fold(0, |v, i| v | u128::from(g.u32(at + 4 * i).unwrap()) << (32 * i))
}

fn put(g: &mut Guest, at: u32, v: Vector) {
    for i in 0..4 {
        g.set_u32(at + 4 * i, (v >> (32 * i)) as u32).unwrap();
    }
}

fn zero(g: &mut Guest, at: u32, len: usize) {
    g.write_bytes(at, &vec![0; len], 1).unwrap();
}

fn header(g: &mut Guest, at: u32, tag: Option<u32>, rows: u32, out: u32, a: u32, b: u32) {
    if let Some(tag) = tag {
        g.set_u32(at, tag).unwrap();
    }
    for (off, v) in [(4, rows), (8, out), (12, a), (16, b), (20, 12)] {
        g.set_u32(at + off, v).unwrap();
    }
}

/// The schema entry for one command: its handler, size, correction pass
/// and operand words from `+4`.
fn entry(cmd: &Command, linear: bool, out: u32) -> (u32, u32, u32, Vec<u32>) {
    let on = |l: (u32, u32, u32), a: (u32, u32, u32)| if linear { l } else { a };
    match cmd.constraint {
        Constraint::Single(Limit::Positive) => (0x31cf97, 4, 0x31e400, vec![]),
        Constraint::Single(Limit::Free) => {
            let (h, s, p) = on((0x31db21, 4, 0x31e400), (0x31da55, 4, 0x31e472));
            (h, s, p, vec![])
        }
        Constraint::Single(Limit::Capped { magnitude }) => {
            let (h, s, p) = on((0x31dc3f, 8, 0x31e400), (0x31d796, 8, 0x31e472));
            (h, s, p, vec![magnitude])
        }
        Constraint::Single(Limit::Bounded { high, low }) => {
            let (h, s, p) = on((0x31df3e, 16, 0x31e400), (0x31d982, 16, 0x31e472));
            (h, s, p, vec![high, low, 0.02f32.to_bits()])
        }
        Constraint::Single(Limit::Range { high, low, angular_scale }) => {
            let (h, s, p) = on((0x31dd80, 12, 0x31e400), (0x31d88e, 16, 0x31e472));
            (h, s, p, if linear { vec![high, low] } else { vec![high, low, angular_scale] })
        }
        Constraint::PositivePair { cross } => (0x31c9c6, 8, 0x31e161, vec![cross]),
        Constraint::Friction { cross, limit, twist_scale: None } => {
            (0x31d532, 20, 0x31e161, vec![out, cross, limit, 12])
        }
        Constraint::Friction { cross, limit, twist_scale: Some(twist) } => {
            (0x31d1fb, 24, 0x31e2e9, vec![out, cross, limit, 12, twist])
        }
    }
}

/// Write `plan` into the guest. Returns each row's address.
fn load(g: &mut Guest, ram: &[u8], plan: &Plan) -> Vec<u32> {
    zero(g, PARAMS, 0x2000);
    zero(g, BODIES, 0x6000);
    zero(g, STACK - 0x1000, 0x1100);
    g.set_u32(STACK, EXIT).unwrap();
    for (i, p) in [PARAMS, SCHEMA, BODIES, IMPULSES].into_iter().enumerate() {
        g.set_u32(STACK + 4 + 4 * i as u32, p).unwrap();
    }
    g.set_u8(READY, 1).unwrap();
    for (off, v) in [(4, plan.rhs), (8, plan.velocity), (12, plan.bias), (16, plan.export), (28, plan.iterations)] {
        g.set_u32(PARAMS + off, v).unwrap();
    }
    let stream = &plan.stream;
    let mut at = ROWS;
    let mut addresses = Vec::new();
    for (i, row) in stream.rows.iter().enumerate() {
        addresses.push(at);
        let mut p = at;
        if let Some(normal) = row.normal {
            put(g, p, normal);
            p += 16;
        }
        put(g, p, row.angular_a);
        put(g, p + 16, row.angular_b);
        at += if row.normal.is_some() { 48 } else { 32 };
        g.set_u32(IMPULSES + 4 * i as u32, row.impulse).unwrap();
    }
    for (i, b) in stream.bodies.iter().enumerate() {
        let at = BODIES + 128 * i as u32;
        g.set_u8(at, 2).unwrap();
        for (off, v) in [(16, b.linear), (32, b.angular), (48, b.inverse_mass), (64, b.linear_sum), (80, b.angular_sum)] {
            put(g, at + off, v);
        }
    }
    g.set_u8(BODIES + 128 * stream.bodies.len() as u32, 3).unwrap();

    let mut p = SCHEMA;
    let mut previous = None;
    let mut headers = 0;
    let (set, restore) = (tag(ram, 0x31e068, 12, 0x31e4cf), tag(ram, 0x31e08d, 4, 0x31e4cf));
    for cmd in &stream.commands {
        if previous != Some(cmd.bodies) {
            let tagged = previous.map(|_| tag(ram, 0x31c9a0, 24, 0x31e0e1));
            let [a, b] = cmd.bodies.map(|i| BODIES + 128 * i as u32);
            header(g, p, tagged, addresses[cmd.first_row], OUT + 0x400 * headers, a, b);
            p += 24;
            headers += 1;
        }
        previous = Some(cmd.bodies);
        let own = cmd.rhs_scale != plan.rhs || cmd.velocity_scale != plan.velocity;
        if own {
            g.set_u32(p, set).unwrap();
            g.set_u32(p + 4, cmd.rhs_scale).unwrap();
            g.set_u32(p + 8, cmd.velocity_scale).unwrap();
            p += 12;
        }
        let linear = stream.rows[cmd.first_row].normal.is_some();
        let (handler, size, correction, words) = entry(cmd, linear, OUT + 0x2000 + 0x80 * headers);
        g.set_u32(p, tag(ram, handler, size, correction)).unwrap();
        for (i, w) in words.into_iter().enumerate() {
            g.set_u32(p + 4 + 4 * i as u32, w).unwrap();
        }
        p += size;
        if own {
            g.set_u32(p, restore).unwrap();
            p += 4;
        }
    }
    g.set_u32(p, done_id(ram) | pass_id(ram, 0x31e4da) << 16).unwrap();
    addresses
}

fn oracle() -> &'static Port {
    PORTS
        .iter()
        .chain(ORACLES)
        .find(|p| p.point.base == points::HAVOK_SOLVER.base)
        .expect("the solver port")
}

/// Run the solver port to its return. Its two row-advance callees answer
/// as the guest's: the row pointer plus the count times the row size.
fn run_oracle(g: &mut Guest, rounding: u32) {
    let port = oracle();
    let mut regs = CpuAbi {
        eax: 0x1234_5678,
        ecx: 0x9876_5432,
        edx: 0x00fe_3420,
        ebx: 0x7b1c,
        esp: STACK,
        ebp: 0x00aa_bbcc,
        esi: 0x5151,
        edi: 0xe37,
        eip: points::HAVOK_SOLVER.base,
        eflags: 0xa93,
        ..Default::default()
    };
    regs.x87.control = CONTROL;
    regs.mxcsr = 0x1f80 | rounding << 13;
    let mut at = points::HAVOK_SOLVER.base;
    for _ in 0..4096 {
        assert!(supported(port, &regs, g), "the solver port falls back at {at:#x}");
        let t = port.invoke(&mut regs, g, at).expect("the solver port faulted");
        assert_ne!(t.route, abi::RESUME, "the solver port escaped to generated code");
        if t.kind == abi::RETURN {
            assert_eq!(t.target_pc, EXIT);
            return;
        }
        let sp = regs.esp;
        let next = g.u32(sp).unwrap();
        assert_eq!(next, t.resume_pc);
        let count = g.u32(sp + 4).unwrap();
        regs.eax = match t.target_pc {
            0x31c520 => regs.ecx.wrapping_add(count.wrapping_mul(32)),
            0x31c530 => regs.ecx.wrapping_add(count.wrapping_mul(48)),
            other => panic!("unexpected callee {other:#x}"),
        };
        regs.esp = sp + 8;
        regs.eip = next;
        at = next;
    }
    panic!("the solver port never returned");
}

/// The guest's working set after the run, in the stream's shape.
fn read_back(g: &Guest, plan: &Plan, addresses: &[u32]) -> Stream {
    let mut s = plan.stream.clone();
    for (i, b) in s.bodies.iter_mut().enumerate() {
        let at = BODIES + 128 * i as u32;
        b.linear = get(g, at + 16);
        b.angular = get(g, at + 32);
        b.linear_sum = get(g, at + 64);
        b.angular_sum = get(g, at + 80);
    }
    for (i, row) in s.rows.iter_mut().enumerate() {
        let mut p = addresses[i];
        if row.normal.is_some() {
            row.normal = Some(get(g, p));
            p += 16;
        }
        row.angular_a = get(g, p);
        row.angular_b = get(g, p + 16);
        row.impulse = g.u32(IMPULSES + 4 * i as u32).unwrap();
    }
    s
}

fn flat(s: &Stream) -> Vec<(String, u32)> {
    let mut v = Vec::new();
    let mut lanes = |name: String, x: Vector| {
        for i in 0..4 {
            v.push((format!("{name}.{i}"), (x >> (32 * i)) as u32));
        }
    };
    for (i, b) in s.bodies.iter().enumerate() {
        for (n, x) in [("linear", b.linear), ("angular", b.angular), ("linear_sum", b.linear_sum), ("angular_sum", b.angular_sum)] {
            lanes(format!("body {i} {n}"), x);
        }
    }
    for (i, r) in s.rows.iter().enumerate() {
        if let Some(n) = r.normal {
            lanes(format!("row {i} normal"), n);
        }
        lanes(format!("row {i} angular_a"), r.angular_a);
        lanes(format!("row {i} angular_b"), r.angular_b);
    }
    for (i, r) in s.rows.iter().enumerate() {
        v.push((format!("row {i} impulse"), r.impulse));
    }
    v
}

/// Seeded cases, each under one MXCSR rounding mode, equal as words. The
/// kernel may decline a stream it does not cover; that must stay rare.
fn sweep(seed: u64, cases: u32, shape: Shape) {
    let Some(ram) = image_ram() else { return };
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    g.write_bytes(0, ram, 1).unwrap();
    let zero_bits = rd(ram, ZERO);
    let mut declined = 0;
    for case in 0..cases {
        let rounding = case % 4;
        let mut r = Rng(seed + u64::from(case));
        let plan = plan(&mut r, shape);
        let mxcsr = 0x1f80 | rounding << 13;
        let Some(kernel) = plan.stream.clone().solve(plan.iterations as usize, plan.bias, zero_bits, mxcsr, CONTROL) else {
            declined += 1;
            continue;
        };
        let addresses = load(&mut g, ram, &plan);
        run_oracle(&mut g, rounding);
        let guest = read_back(&g, &plan, &addresses);
        let (kernel, guest) = (flat(&kernel), flat(&guest));
        if let Some(((name, k), (_, o))) = kernel.iter().zip(&guest).find(|(k, o)| k.1 != o.1) {
            panic!("case {case} (seed {seed:#x}, rounding {rounding}): {name}: kernel {k:#010x}, port {o:#010x}");
        }
    }
    assert!(declined * 20 <= cases, "the kernel declined {declined} of {cases} finite streams");
}

#[test]
fn rows_equal_register_port() {
    sweep(0x5eed_0001_0000, 10_000, Shape::All);
}

#[test]
fn correct_velocities_equal_register_port() {
    sweep(0x5eed_0002_0000, 2_000, Shape::Correction);
}

/// A non-finite impulse is a declined stream: the guest keeps it, so the
/// port's behaviour is the one retail has. The port still runs it through.
#[test]
fn rows_equal_on_non_finite_impulses() {
    let Some(ram) = image_ram() else { return };
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    g.write_bytes(0, ram, 1).unwrap();
    let zero_bits = rd(ram, ZERO);
    let specials = [f32::INFINITY.to_bits(), f32::NEG_INFINITY.to_bits(), 0x7fc0_0000, 0xffc1_2345];
    for case in 0..400u32 {
        let mut r = Rng(0x5eed_0003_0000 + u64::from(case));
        let mut plan = plan(&mut r, Shape::All);
        let row = r.u32() as usize % plan.stream.rows.len();
        plan.stream.rows[row].impulse = specials[case as usize % 4];
        let mxcsr = 0x1f80 | (case % 4) << 13;
        assert!(
            plan.stream.clone().solve(plan.iterations as usize, plan.bias, zero_bits, mxcsr, CONTROL).is_none(),
            "case {case}: the kernel took a non-finite impulse"
        );
        load(&mut g, ram, &plan);
        run_oracle(&mut g, case % 4);
    }
}
