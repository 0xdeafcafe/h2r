//! Entry I, `0x00301800`: the general MOPP ray in Rust
//! ([`arm::CollectorCast`]) against the lifted guest. The setup and the
//! float interpreter run on the IR interpreter with nothing stubbed; the
//! callbacks they reach are invented and answered by the run loop: the
//! collection's child method (the instanced one at `0x00244bb0`, or one at
//! an invented address), the child shape's ray method, which lowers the
//! collector's fraction for some keys, and an optional filter that rejects
//! some keys. Each case drives the cast with the same callbacks and checks
//! every callback (target, registers, arguments, the temporary body), the
//! keys the collection got and every byte the guest left above its stack.
//!
//! Programs are `sim_collision::synth::random_mopp` and hand ones for every
//! opcode family, with rays on and across their thresholds.
use super::*;
use engine::sim_collision::layout::mopp_ray as l;
use engine::sim_collision::mopp_ray as arm;
use mopp::Code;
use sim_collision::mopp::ray::{Constants, Event, Precision, Ray, RayWalk};
use sim_collision::Bail;

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
/// The interpreter's unknown-opcode report: it calls Havok's error object
/// and then writes address 0, so a program reaching it is one the guest
/// asserts on.
const OPCODE_TRAP: u32 = 0x0030_0d86;

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
        mxcsr: 0x1fa0,
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
    /// The guest reached [`OPCODE_TRAP`].
    trapped: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Callback {
    target: u32,
    this: u32,
    sp: u32,
    args: Vec<u32>,
    /// All words of the temporary body as the child sees it.
    body: Option<[u32; 4]>,
}

impl Callback {
    fn new(target: u32, this: u32, sp: u32, args: Vec<u32>, m: &Ram<'_>) -> Self {
        let body = (target == FAKE_SHAPE_RAY)
            .then(|| std::array::from_fn(|i| word(m, args[1] + i as u32 * 4)));
        Self {
            target,
            this,
            sp,
            args,
            body,
        }
    }

    fn argc(target: u32) -> Option<u32> {
        match target {
            l::INSTANCE_CHILD | FAKE_COLLECTION_CHILD => Some(2),
            FAKE_SHAPE_RAY => Some(3),
            FAKE_FILTER => Some(4),
            _ => None,
        }
    }

    fn answer(&self, m: &mut Ram<'_>, keys: &mut Vec<u32>, change_inputs: bool) -> u32 {
        match self.target {
            l::INSTANCE_CHILD | FAKE_COLLECTION_CHILD => {
                assert_eq!(self.this, COLLECTION);
                keys.push(self.args[0]);
                if change_inputs {
                    // The lookup can change the body metadata before the VM
                    // builds its temporary collision body.
                    m.word(PARENT + 8, self.args[0].wrapping_add(17));
                }
                CHILD
            }
            FAKE_SHAPE_RAY => {
                assert_eq!(self.this, CHILD);
                assert_eq!(self.args[0], VM + 0x20);
                let [shape, key, info, parent] = self.body.unwrap();
                assert_eq!((shape, info, parent), (CHILD, word(m, PARENT + 8), PARENT));
                let mut collector = self.args[2];
                if change_inputs {
                    // The interpreter re-loads the collector pointer after
                    // dispatch. Leave the old collector's fraction intact.
                    let saved = word(m, collector + l::COLLECTOR_FRACTION);
                    collector = COLLECTOR + 0x40;
                    m.word(VM + l::COLLECTOR, collector);
                    m.word(collector + l::COLLECTOR_FRACTION, saved);
                }
                let found = hit(key, word(m, collector + l::COLLECTOR_FRACTION));
                if let Some(f) = found {
                    m.word(collector + l::COLLECTOR_FRACTION, f);
                }
                u32::from(found.is_some())
            }
            FAKE_FILTER => {
                assert_eq!(self.this, FILTER);
                assert_eq!(&self.args[1..3], &[VM + 0x20, COLLECTION]);
                // Check that the VM reads EAX, which need not be its stack
                // result pointer.
                let result = if change_inputs {
                    FILTER + 0x10
                } else {
                    self.args[0]
                };
                m.store(result, &[u8::from(accepted(self.args[3]))]);
                result
            }
            _ => panic!("unexpected callback {:#x}", self.target),
        }
    }
}

fn run_callbacks(
    oracle: &mut Oracle<'_>,
    c: &CpuAbi,
    m: &mut Ram<'_>,
    trace: &mut Vec<(Callback, CpuAbi)>,
    change_inputs: bool,
) -> Run {
    let mut cpu = ir_cpu(c);
    let mut pc = c.eip;
    let mut out = Run::default();
    for _ in 0..4_000_000 {
        cpu.eip = pc;
        if pc == HARNESS_EXIT {
            return out;
        }
        if pc == OPCODE_TRAP {
            out.trapped = true;
            return out;
        }
        if pc == points::SIM_MOPP_RAY.base {
            out.enters += 1;
        } else if let Some(argc) = Callback::argc(pc) {
            let sp = cpu.gpr[4];
            let args = (0..argc).map(|i| word(m, sp + 4 + i * 4)).collect();
            let call = Callback::new(pc, cpu.gpr[1], sp, args, m);
            let eax = call.answer(m, &mut out.keys, change_inputs);
            trace.push((call, lifted::cpu::from_interp(&cpu)));
            pc = ret(&mut cpu, m, argc * 4, eax);
            continue;
        }
        oracle.prime(pc);
        pc = interp::step(&mut cpu, m, &oracle.cache[&pc])
            .unwrap_or_else(|e| panic!("at {pc:#x}: {e:?}"))
            .target();
    }
    panic!("step limit at {pc:#x}");
}

/// What one cast compared.
#[derive(Default)]
struct Cast {
    rejected: u32,
    rays: u32,
    nested: bool,
}

/// Run the guest on `guest` and the cast on a copy, with the same
/// callbacks, and compare. `None`: the walk declines (the guest would
/// assert, or the float environment or root is off the exact path).
fn check_cast(
    oracle: &mut Oracle<'_>,
    image: &[u8],
    cpu: &CpuAbi,
    mut guest: Ram<'_>,
    case: &Case,
    change_inputs: bool,
    label: &str,
) -> Option<Cast> {
    static EPOCH: Epoch = Epoch::new();
    let expected = simulate(image, case);
    let world = CollisionWorld::new(&EPOCH);
    let mut native = guest.clone();
    let mut trace = Vec::new();
    let observed = run_callbacks(oracle, cpu, &mut guest, &mut trace, change_inputs);
    if observed.trapped || expected.is_err() {
        assert!(expected.is_err(), "{label}: the guest trapped, the walk did not");
        return None;
    }
    assert!(observed.enters > 0, "{label}: the interpreter never ran");
    if !change_inputs {
        assert_eq!(Ok(&observed.keys), expected.as_ref(), "{label}: keys");
    }
    let mut cast = arm::CollectorCast::begin(cpu, &mut native, &world).unwrap();
    let (mut returned, mut keys, mut out) = (*cpu, Vec::new(), Cast::default());
    for (want, regs) in trace {
        let call = cast
            .next(&mut native, &returned)
            .unwrap()
            .expect("missing guest callback");
        let sp = call.esp(cpu.esp);
        assert_eq!(call.eax, Some(regs.eax), "{label}: callback EAX");
        assert_eq!(call.edx, Some(regs.edx), "{label}: callback vtable");
        assert_eq!(call.edi, Some(regs.edi), "{label}: callback key");
        if let Some(esi) = call.esi {
            assert_eq!(esi, regs.esi, "{label}: callback context");
        }
        let got = Callback::new(call.target, call.ecx.unwrap(), sp, call.args, &native);
        assert_eq!(want, got, "{label}: callback");
        if got.target == FAKE_FILTER && !accepted(got.args[3]) {
            out.rejected += 1;
        }
        out.rays += u32::from(got.target == FAKE_SHAPE_RAY);
        out.nested |= observed.enters > 1;
        returned.eax = got.answer(&mut native, &mut keys, change_inputs);
    }
    assert!(cast.next(&mut native, &returned).unwrap().is_none(), "{label}: extra callback");
    assert_eq!(keys, observed.keys, "{label}: keys");
    for addr in guest.touched(&native, cpu.esp, (0, cpu.esp)) {
        assert_eq!(native.get(addr), guest.get(addr), "{label}: memory {addr:#x}");
    }
    Some(out)
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
fn mopp_ray_matches_the_guest() {
    with_oracle("collision MOPP ray differential test", |oracle, image| {
        const CASES: u32 = 1500;
        let (mut compared, mut skipped) = (0u32, 0u32);
        for n in 0..CASES {
            let mut rng =
                Rng(0x6a09_e667_f3bc_c908 ^ u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
            let case = random_case(image, &mut rng, n);
            let (cpu, ram) = setup(image, &mut rng, &case);
            match check_cast(oracle, image, &cpu, ram, &case, false, &format!("case {n}")) {
                Some(_) => compared += 1,
                None => skipped += 1,
            }
        }
        eprintln!("MOPP ray: {compared} compared, {skipped} declined walks, of {CASES}");
        assert!(compared * 3 > CASES, "MOPP ray: only {compared} of {CASES} compared");
    });
}

/// Every opcode family the walk takes, with rays starting, ending and
/// running along their thresholds, in single and double precision: every
/// case must be answered and match.
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
                        let (cpu, ram) = setup(image, &mut rng, &case);
                        assert!(
                            check_cast(oracle, image, &cpu, ram, &case, false, &label).is_some(),
                            "{label}: declined"
                        );
                        n += 1;
                    }
                }
            }
        }
    });
}

/// Compare the full sequence, including rejected terminals, arbitrary
/// collection methods, callbacks that change the VM's inputs, the guest
/// scratch addresses, and every visible VM byte. Both trials start with
/// their own copy of the fixture.
#[test]
fn native_collector_callbacks_match_the_guest() {
    with_oracle("general ray collector callbacks", |oracle, image| {
        let (mut compared, mut rejected, mut rays, mut nested) = (0, 0, 0, false);
        let mut rng = Rng(0x6134_9875);
        for n in 0..120 {
            let mut case = random_case(image, &mut rng, n);
            case.control = if n % 2 == 0 { 0x027f } else { 0x007f };
            if n < 8 {
                // Cross a split that calls both terminals. Key 1 is
                // rejected by the filter and key 3 can shorten the ray.
                case.code = Code {
                    offset: [0.; 3],
                    scale: 1.,
                    info: mopp::code::RETAIL_INFO,
                    bytecode: vec![0x10, 10, 20, 1, 0x31, 0x33],
                };
                case.from = world_point(image, &case.code, [0., 0., 0.], 0.);
                case.to = world_point(image, &case.code, [30., 0., 0.], 0.);
                case.filter = n & 1 != 0;
                case.instanced = n & 2 != 0;
                case.fraction = 1.;
            }
            let (cpu, ram) = setup(image, &mut rng, &case);
            let label = format!("case {n}");
            if let Some(cast) = check_cast(oracle, image, &cpu, ram, &case, n % 3 != 0, &label) {
                compared += 1;
                rejected += cast.rejected;
                rays += cast.rays;
                nested |= cast.nested;
            }
        }
        assert!(compared >= 8 && rejected > 0 && rays > 0 && nested);
    });
}

#[test]
fn collector_declines_a_callback_edit_to_its_cached_ray() {
    with_oracle("general ray callback changes an endpoint", |_, image| {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let case = Case {
            code: Code {
                offset: [0.; 3],
                scale: 1.,
                info: mopp::code::RETAIL_INFO,
                bytecode: vec![0x31],
            },
            from: [0.; 4],
            to: [1.; 4],
            filter: true,
            instanced: true,
            fraction: 1.,
            control: 0x027f,
        };
        let (mut cpu, mut ram) = setup(image, &mut Rng(37), &case);
        let mut cast = arm::CollectorCast::begin(&cpu, &mut ram, &world).unwrap();
        let call = cast.next(&mut ram, &cpu).unwrap().unwrap();
        assert_eq!(call.target, FAKE_FILTER);
        cpu.eax = call.args[0];
        ram.store(cpu.eax, &[0]);
        ram.float(VM + 0x20, 12.);
        assert_eq!(cast.next(&mut ram, &cpu), Err(Bail::Data));
    });
}

#[path = "mopp_ray/disc_rays.rs"]
mod disc_rays;
