//! Entry I, `0x00301800`: the general MOPP ray's parity arm against the
//! lifted guest. The setup and the float interpreter run on the IR
//! interpreter with nothing stubbed; the callbacks they reach are invented
//! and answered by the run loop: the collection's child method (the
//! instanced one at `0x00244bb0`, whose keys the arm observes, or one at an
//! invented address, whose keys it cannot see), the child shape's ray
//! method, which lowers the collector's fraction for some keys, and an
//! optional filter that rejects some keys. The arm's observers run where
//! the installer puts them, and each case checks:
//!
//! - the arm's outcome: matched, or unverified for a stated reason, never a
//!   mismatch;
//! - the keys the guest passed to the collection against a native-only
//!   run of the walk with the same callbacks.
//!
//! Programs are `sim_collision::synth::random_mopp` and hand ones for every
//! opcode family, with rays on and across their thresholds. A last test
//! changes the bytecode under a warm world (same header, same revision) so
//! the walk follows the old program, and the arm must report the mismatch.
use super::*;
use engine::sim_collision::layout::mopp_ray as l;
use engine::sim_collision::mopp_ray as arm;
use mopp::Code;
use sim_collision::mopp::ray::{Constants, Event, Precision, Ray, RayWalk};
use sim_collision::{Bail, RayParity};

const VM: u32 = 0x0063_6000;
const INPUT: u32 = 0x0063_6100;
const COLLECTOR: u32 = 0x0063_6200;
const PARENT: u32 = 0x0063_6300;
const COLLECTION: u32 = 0x0063_6400;
const CHILD: u32 = 0x0063_6500;
const FILTER: u32 = 0x0063_6600;
/// The collection's, the child's and the filter's vtables, 0x100 apart.
const VTABLES: u32 = 0x0063_6800;
const CODE: u32 = 0x0066_0000;
/// Invented callees: the run loop answers them; nothing runs there.
const FAKE_COLLECTION_CHILD: u32 = 0x0063_7000;
const FAKE_SHAPE_RAY: u32 = 0x0063_7010;
const FAKE_FILTER: u32 = 0x0063_7020;

/// One query.
#[derive(Clone, Debug)]
struct Case {
    code: Code,
    from: [f32; 4],
    to: [f32; 4],
    /// The collection's child method is the instanced one (keys observed).
    instanced: bool,
    filter: bool,
    fraction: f32,
    control: u16,
}

/// The child shape's answer: some keys hit, at a fraction below the
/// current one.
fn hit(key: u32, fraction: u32) -> Option<u32> {
    if !key.is_multiple_of(3) {
        return None;
    }
    let f = [0.875f32, 0.625, 0.375, 0.125][(key as usize >> 2) & 3];
    (f < f32::from_bits(fraction)).then_some(f.to_bits())
}

/// The filter's answer.
fn accepted(key: u32) -> bool {
    key % 5 != 1
}

fn image_word(image: &[u8], at: u32) -> u32 {
    u32::from_le_bytes(image[at as usize..at as usize + 4].try_into().unwrap())
}

fn ray(image: &[u8], case: &Case) -> Result<Ray, Bail> {
    let o = case.code.offset.map(f32::to_bits);
    Ok(Ray {
        origin: [o[0], o[1], o[2], case.code.scale.to_bits()],
        from: case.from.map(f32::to_bits),
        to: case.to.map(f32::to_bits),
        precision: Precision::from_control(case.control)?,
        constants: Constants {
            zero: image_word(image, layout::ZERO),
            one: image_word(image, layout::ONE),
            unit: std::array::from_fn(|k| image_word(image, l::UNIT + 4 * k as u32)),
            root_scale: image_word(image, l::ROOT_SCALE),
        },
    })
}

/// The keys the collection should see, from the walk alone.
fn simulate(image: &[u8], case: &Case) -> Result<Vec<u32>, Bail> {
    let ray = ray(image, case)?;
    let root = ray.root()?;
    let mut walk = RayWalk::new(case.code.bytecode.as_slice(), ray, &root);
    let mut fraction = case.fraction.to_bits();
    let mut keys = Vec::new();
    while let Some(event) = walk.next(fraction)? {
        if let Event::Key(key) = event {
            if case.filter && !accepted(key) {
                continue;
            }
            keys.push(key);
            if let Some(f) = hit(key, fraction) {
                fraction = f;
            }
        }
    }
    Ok(keys)
}

fn setup<'a>(image: &'a [u8], rng: &mut Rng, case: &Case) -> (CpuAbi, Ram<'a>) {
    let (cpu, mut ram) = setup_at(image, rng, case, CODE);
    ram.store(CODE, &case.code.to_block());
    (cpu, ram)
}

fn setup_at<'a>(image: &'a [u8], rng: &mut Rng, case: &Case, code_ptr: u32) -> (CpuAbi, Ram<'a>) {
    let mut m = fresh(image);
    m.store(VM, &[0xab; 0x70]);
    for k in 0..4u32 {
        m.float(INPUT + 4 * k, case.from[k as usize]);
        m.float(INPUT + l::INPUT_TO + 4 * k, case.to[k as usize]);
    }
    m.word(INPUT + 0x20, 0x5a5a_5a5a);
    m.word(
        INPUT + l::INPUT_FILTER,
        if case.filter { FILTER } else { 0 },
    );
    m.word(COLLECTOR, 0x1111_2222);
    m.float(COLLECTOR + l::COLLECTOR_FRACTION, case.fraction);
    m.word(PARENT + 8, 0x7777_0008);
    m.word(COLLECTION, VTABLES);
    let child = if case.instanced {
        l::INSTANCE_CHILD
    } else {
        FAKE_COLLECTION_CHILD
    };
    m.word(VTABLES + l::COLLECTION_CHILD, child);
    m.word(CHILD, VTABLES + 0x100);
    m.word(VTABLES + 0x100 + 0x24, FAKE_SHAPE_RAY);
    m.word(FILTER, VTABLES + 0x200);
    m.word(VTABLES + 0x200, FAKE_FILTER);
    for (k, arg) in [COLLECTION, code_ptr, INPUT, PARENT, COLLECTOR]
        .into_iter()
        .enumerate()
    {
        m.word(STACK + 4 + 4 * k as u32, arg);
    }
    let mut c = entry_cpu(rng, points::COLLISION_MOPP_RAY.base);
    c.ecx = VM;
    (c.mxcsr, c.x87.control, c.x87.status, c.x87.tag) = (0x1fa0, case.control, 0, 0xffff);
    (c, m)
}

fn word(m: &Ram<'_>, at: u32) -> u32 {
    let mut b = [0; 4];
    m.read(at, &mut b).unwrap();
    u32::from_le_bytes(b)
}

/// Return from an answered callee: `eax`, then pop the return address and
/// `pop` bytes of arguments.
fn ret(c: &mut Cpu, m: &Ram<'_>, pop: u32, eax: u32) -> u32 {
    let sp = c.gpr[4];
    c.gpr[0] = eax;
    c.gpr[4] = sp + 4 + pop;
    word(m, sp)
}

#[derive(Default)]
struct Run {
    /// Keys the guest passed to the collection.
    keys: Vec<u32>,
    enters: u32,
}

/// The guest query with the arm's observers where the installer puts them
/// and the invented callees answered here.
fn run(oracle: &mut Oracle<'_>, c: &CpuAbi, m: &mut Ram<'_>, world: &CollisionWorld) -> Run {
    let mut cpu = ir_cpu(c);
    let mut pc = c.eip;
    let mut out = Run::default();
    let arg = |cpu: &Cpu, m: &Ram<'_>, k: u32| word(m, cpu.gpr[4] + 4 + 4 * k);
    for _ in 0..4_000_000 {
        cpu.eip = pc;
        if pc == HARNESS_EXIT {
            arm::end(1, &*m, world);
            return out;
        }
        if pc == points::COLLISION_MOPP_RAY.base {
            arm::begin(1, &lifted::cpu::from_interp(&cpu), &mut *m, world);
        } else if pc == points::SIM_MOPP_RAY.base {
            out.enters += 1;
            arm::enter(1, &lifted::cpu::from_interp(&cpu), &*m);
        } else if pc == l::INSTANCE_CHILD || pc == FAKE_COLLECTION_CHILD {
            // `this` the collection; stack: key, buffer; `ret 8`.
            if pc == l::INSTANCE_CHILD {
                arm::key(1, &lifted::cpu::from_interp(&cpu), &*m);
            }
            assert_eq!(cpu.gpr[1], COLLECTION, "the collection's `this`");
            out.keys.push(arg(&cpu, &*m, 0));
            pc = ret(&mut cpu, m, 8, CHILD);
            continue;
        } else if pc == FAKE_SHAPE_RAY {
            // `this` the child; stack: input, body, collector; `ret 0xc`.
            assert_eq!(cpu.gpr[1], CHILD, "the child shape's `this`");
            let (body, collector) = (arg(&cpu, &*m, 1), arg(&cpu, &*m, 2));
            assert_eq!(word(m, body), CHILD, "the body's shape");
            assert_eq!(word(m, body + 12), PARENT, "the body's parent");
            let fraction = word(m, collector + l::COLLECTOR_FRACTION);
            let found = hit(word(m, body + 4), fraction);
            if let Some(f) = found {
                m.word(collector + l::COLLECTOR_FRACTION, f);
            }
            pc = ret(&mut cpu, m, 12, u32::from(found.is_some()));
            continue;
        } else if pc == FAKE_FILTER {
            // stack: out, input, collection, key; `ret 0x10`, `eax` = out.
            let (result, key) = (arg(&cpu, &*m, 0), arg(&cpu, &*m, 3));
            m.store(result, &[u8::from(accepted(key))]);
            pc = ret(&mut cpu, m, 16, result);
            continue;
        }
        oracle.prime(pc);
        pc = interp::step(&mut cpu, m, &oracle.cache[&pc])
            .unwrap_or_else(|e| panic!("at {pc:#x}: {e:?}"))
            .target();
    }
    panic!("step limit at {pc:#x}");
}

/// What one case's arm recorded.
fn outcome(world: &CollisionWorld) -> RayParity {
    world.stats().ray
}

/// Run `case` on a fresh world and check it. `Some(true)`:
/// compared to the end; `Some(false)`: unverified; `None`: skipped
/// (a trap the guest would assert on).
fn check_case(
    oracle: &mut Oracle<'_>,
    image: &[u8],
    rng: &mut Rng,
    case: &Case,
    label: &str,
) -> Option<bool> {
    static EPOCH: Epoch = Epoch::new();
    let expected = simulate(image, case);
    if expected == Err(Bail::Data) {
        return None;
    }
    let world = CollisionWorld::new(&EPOCH);
    let (c, mut m) = setup(image, rng, case);
    let got = run(oracle, &c, &mut m, &world);
    let r = outcome(&world);
    assert_eq!(arm::open_queries(), 0, "{label}: a query stayed open");
    assert_eq!(
        r.mismatched,
        0,
        "{label}: {}, control {:#x}, {} program bytes",
        r.first_mismatch.as_deref().unwrap_or(""),
        case.control,
        case.code.bytecode.len()
    );
    assert!(got.enters > 0, "{label}: the interpreter never ran");
    let unverified: u64 = r.unverified.iter().sum();
    assert_eq!(
        r.matched + r.fraction_moved + unverified,
        1,
        "{label}: one query"
    );
    match expected {
        Ok(keys) => {
            assert_eq!(
                got.keys, keys,
                "{label}: keys, from {:?}, to {:?}",
                case.from, case.to
            );
            if case.instanced && !case.filter {
                assert_eq!(r.matched, 1, "{label}: observed keys, not matched: {r:?}");
            }
            Some(r.matched == 1)
        }
        Err(why) => {
            if why == Bail::Env {
                assert_eq!(r.unverified[Bail::Env as usize], 1, "{label}");
            }
            Some(false)
        }
    }
}

/// A frame coordinate: whole cells (on thresholds), eighths, and a little
/// outside the root range.
fn frame_coordinate(rng: &mut Rng) -> f64 {
    match rng.below(5) {
        0 | 1 => f64::from(rng.below(256)),
        2 => f64::from(rng.below(2048)) / 8.0,
        3 => -f64::from(rng.below(16)),
        _ => 255.0 + f64::from(rng.below(16)),
    }
}

/// World-space endpoints for frame-space ones, through the code's origin
/// and scale and the guest's root factor.
fn world_point(image: &[u8], code: &Code, frame: [f64; 3], w: f32) -> [f32; 4] {
    let root = f64::from(f32::from_bits(image_word(image, l::ROOT_SCALE)));
    let s = f64::from(code.scale) * root;
    let p = |a: usize| (frame[a] / s + f64::from(code.offset[a])) as f32;
    [p(0), p(1), p(2), w]
}

fn random_case(image: &[u8], rng: &mut Rng, n: u32) -> Case {
    let (offset, scale) = match rng.below(4) {
        0 => ([0.0; 3], 1.0),
        1 => ([-512.0, 64.0, 0.25], 4096.0),
        2 => ([1.5, -2.25, 8.0], 0.75),
        _ => ([0.0; 3], 0.5),
    };
    let code = Code {
        offset,
        scale,
        info: mopp::code::RETAIL_INFO,
        bytecode: synth::random_mopp(rng, 1 + n % 7),
    };
    let a = [(); 3].map(|_| frame_coordinate(rng));
    let b = if rng.chance(16) {
        a
    } else {
        [(); 3].map(|_| frame_coordinate(rng))
    };
    let w = if rng.chance(4) { 1.0 } else { 0.0 };
    let control = match rng.below(10) {
        0 => 0x037f,
        1 | 2 => 0x007f,
        _ => 0x027f,
    };
    Case {
        from: world_point(image, &code, a, w),
        to: world_point(image, &code, b, w),
        code,
        instanced: !rng.chance(4),
        filter: rng.chance(5),
        fraction: [1.0, 1.0, 1.0, 0.5, 0.0][rng.below(5) as usize],
        control,
    }
}

#[test]
fn mopp_ray_parity_matches_the_guest() {
    with_oracle("collision MOPP ray differential test", |oracle, image| {
        const CASES: u32 = 1500;
        let (mut compared, mut skipped) = (0u32, 0u32);
        for n in 0..CASES {
            let mut rng =
                Rng(0x6a09_e667_f3bc_c908 ^ u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
            let case = random_case(image, &mut rng, n);
            match check_case(oracle, image, &mut rng, &case, &format!("case {n}")) {
                Some(full) => compared += u32::from(full),
                None => skipped += 1,
            }
        }
        eprintln!(
            "MOPP ray: {compared} compared to the end, {skipped} trap programs skipped, of {CASES}"
        );
        assert!(
            compared * 3 > CASES,
            "MOPP ray: only {compared} of {CASES} compared"
        );
    });
}

/// Every opcode family the walk takes, with rays starting, ending and
/// running along their thresholds, in single and double precision, keys
/// observed: every case must match to the end.
#[test]
fn mopp_ray_edges_match_the_guest() {
    with_oracle("collision MOPP ray edges", |oracle, image| {
        let programs: [&[u8]; 11] = [
            &[0x10, 10, 20, 1, 0x31, 0x32],
            &[0x11, 10, 20, 1, 0x31, 0x32],
            &[0x23, 10, 20, 0, 0, 0, 1, 0x31, 0x32],
            &[0x20, 10, 1, 0x31, 0x32],
            &[0x26, 10, 20, 0x31],
            &[0x29, 0, 0x0a, 0, 0x14, 0, 0, 0x31],
            &[0x04, 1, 0, 0, 0x26, 2, 5, 0x31],
            &[0x17, 10, 12, 1, 0x31, 0x32],
            &[0x01, 3, 5, 7, 0x1c, 140, 150, 1, 0x31, 0x32],
            &[
                0x0b, 0, 0, 1, 0, 0x60, 9, 0x10, 255, 0, 2, 0x05, 1, 0x33, 0x51, 1, 2,
            ],
            &[0x12, 8, 9, 3, 0x09, 3, 0x30, 0x10, 8, 30, 1, 0x34, 0x35],
        ];
        let mut n = 0u32;
        for (p, bytecode) in programs.iter().enumerate() {
            let code = Code {
                offset: [0.0; 3],
                scale: 1.0,
                info: mopp::code::RETAIL_INFO,
                bytecode: bytecode.to_vec(),
            };
            for axis in 0..3 {
                for (a, b) in [
                    (0.0, 30.0),
                    (30.0, 0.0),
                    (10.0, 20.0),
                    (20.0, 10.0),
                    (10.0, 10.0),
                    (5.0, 10.0),
                    (20.0, 25.0),
                ] {
                    for control in [0x027f, 0x007f] {
                        let (mut from, mut to) = ([5.0; 3], [5.0; 3]);
                        from[axis] = a;
                        to[axis] = b;
                        let case = Case {
                            from: world_point(image, &code, from, 0.0),
                            to: world_point(image, &code, to, 0.0),
                            code: code.clone(),
                            instanced: true,
                            filter: false,
                            fraction: 1.0,
                            control,
                        };
                        let mut rng = Rng(u64::from(n) | 1);
                        let label =
                            format!("program {p} axis {axis} {a}..{b} control {control:#x}");
                        assert_eq!(
                            check_case(oracle, image, &mut rng, &case, &label),
                            Some(true),
                            "{label}: not compared to the end"
                        );
                        n += 1;
                    }
                }
            }
        }
    });
}

/// The arm reports a walk that does not follow the guest: the world holds
/// a program decoded from one bytecode, the guest runs another with the
/// same header (the revision cannot see it in this memory), and the guest's
/// answer is the one the collection gets.
#[test]
fn mopp_ray_reports_a_deliberate_mismatch() {
    with_oracle("collision MOPP ray mismatch", |oracle, image| {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let mut code = Code {
            offset: [0.0; 3],
            scale: 1.0,
            info: mopp::code::RETAIL_INFO,
            bytecode: vec![0x10, 10, 20, 1, 0x31, 0x32],
        };
        let mut case = Case {
            from: world_point(image, &code, [0.0, 5.0, 5.0], 0.0),
            to: world_point(image, &code, [30.0, 5.0, 5.0], 0.0),
            code: code.clone(),
            instanced: true,
            filter: false,
            fraction: 1.0,
            control: 0x027f,
        };
        let mut rng = Rng(7);
        let (c, mut m) = setup(image, &mut rng, &case);
        run(oracle, &c, &mut m, &world);
        assert_eq!(outcome(&world).matched, 1, "the first program matches");
        // The low threshold moves; the walk still has the old one.
        code.bytecode[1] = 25;
        case.code = code;
        let (c, mut m) = setup(image, &mut rng, &case);
        let got = run(oracle, &c, &mut m, &world);
        let r = outcome(&world);
        assert_eq!((r.matched, r.mismatched), (1, 1), "{r:?}");
        let first = r.first_mismatch.unwrap();
        assert!(first.contains("different step"), "{first}");
        assert_eq!(
            Ok(got.keys),
            simulate(image, &case),
            "the guest's answer stands"
        );
        assert_eq!(arm::open_queries(), 0);
    });
}

#[path = "mopp_ray/disc_rays.rs"]
mod disc_rays;
