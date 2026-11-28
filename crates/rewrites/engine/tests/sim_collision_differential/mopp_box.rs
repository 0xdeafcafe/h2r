//! Entry F, `0x00300150`, against the lifted guest: random programs
//! (`sim_collision::synth::random_mopp`) and compiled ones (`mopp::build`
//! over invented surfaces), boxes on frame-cell edges, and the decline
//! paths (precision flag clear, extended or directed x87 control, a NaN,
//! inverted or far box, a full array, an output over the VM, the code or
//! the dead stack). The comparison excludes the declared clobbers: `eax`,
//! `ecx`, `edx`, the x87 condition codes and the dead stack below the
//! entry. The ignored sweep runs every structure program on the owner's
//! disc against `mopp::boxvm` and, for one box in ten, against the guest.
use super::*;
use mopp::{Aabb, Code};
use sim_collision::mopp::{Corners, Source, Tree};

const VM: u32 = 0x0063_6000;
const AABB: u32 = 0x0063_6100;
const ARRAY: u32 = 0x0063_6200;
const DATA: u32 = 0x0063_7000;
const CODE: u32 = 0x0066_0000;

const MOPP_BOX: Entry = Entry {
    point: &points::COLLISION_MOPP_BOX,
    eax: 0,
    dead: (layout::mopp_box::DEAD_BELOW, 0),
};

/// Masked, nearest single, double and extended: the controls F answers in
/// (extended on the x87 model).
const SUPPORTED: [u16; 3] = [0x027f, 0x007f, 0x037f];
/// Round up, an unmasked invalid: the environment F declines.
const DECLINED: [u16; 2] = [0x0a7f, 0x027e];

/// A program and where its full-precision space sits in the world.
fn program(rng: &mut Rng, n: u32) -> Code {
    if n % 5 == 4 {
        let surfaces: Vec<mopp::geom::Surface> = (0..1 + rng.below(30))
            .map(|k| {
                let base = [rng.eighths(64), rng.eighths(64), rng.eighths(16)];
                let lift = rng.eighths(4).abs();
                mopp::geom::Surface {
                    key: mopp::geom::structure_key(k as usize),
                    vertices: vec![
                        base,
                        [base[0] + 1.0, base[1] + lift, base[2] + 0.5],
                        [base[0] + 3.0, base[1], base[2] + 1.5],
                    ],
                }
            })
            .collect();
        return mopp::build(&surfaces);
    }
    let (offset, scale) = match rng.below(4) {
        0 => ([0.0; 3], 1.0),
        1 => ([-512.0, 64.0, 0.25], 4096.0),
        2 => ([1.5, -2.25, 8.0], 0.75),
        _ => ([0.0; 3], 0.5),
    };
    Code {
        offset,
        scale,
        info: mopp::code::RETAIL_INFO,
        bytecode: synth::random_mopp(rng, 1 + n % 7),
    }
}

/// A full-precision coordinate near the frame cells a random program
/// tests: whole root cells, finer cells, exact edges.
fn full(rng: &mut Rng) -> f64 {
    let cell = f64::from(rng.below(256)) * 65536.0;
    cell + match rng.below(4) {
        0 => 0.0,
        1 => f64::from(rng.below(16) * 4096),
        2 => f64::from(rng.below(65536)),
        _ => 65535.0,
    }
}

/// A box for `code`: random cells mapped back to the world, or near a
/// compiled program's surfaces; now and then NaN, inverted or far away.
fn random_aabb(rng: &mut Rng, code: &Code) -> Aabb {
    let mut b = frame_box(rng, code);
    match rng.below(40) {
        0 => b.min[rng.below(3) as usize] = f32::NAN,
        1 => b.max.swap(0, 1),
        2 => b.max[0] = 3.0e9 / code.scale,
        3 => b.min[2] = b.max[2] + 100.0,
        _ => {}
    }
    b
}

fn frame_box(rng: &mut Rng, code: &Code) -> Aabb {
    let world = |a: usize, x: f64| (x / f64::from(code.scale) + f64::from(code.offset[a])) as f32;
    let mut b = Aabb {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    for a in 0..3 {
        let lo = full(rng);
        let hi = if rng.chance(4) {
            lo
        } else {
            lo + f64::from(rng.below(1 << 21))
        };
        b.min[a] = world(a, lo);
        b.max[a] = world(a, hi);
    }
    b
}

fn put_aabb(m: &mut Ram<'_>, b: &Aabb) {
    put_vector(m, AABB, b.min);
    m.word(AABB + 12, 0x5a5a_5a5a);
    put_vector(m, AABB + layout::mopp_box::AABB_MAX, b.max);
    m.word(AABB + 0x1c, 0x5a5a_5a5a);
}

/// Memory and CPU for one call: the VM and the key area pre-filled so
/// stray writes show, an array whose room is sometimes too small, and a
/// key area that sometimes overlaps another input.
fn setup<'a>(image: &'a [u8], rng: &mut Rng, code: &Code, b: &Aabb) -> (CpuAbi, Ram<'a>) {
    let mut m = fresh(image);
    m.store(CODE, &code.to_block());
    m.store(VM, &[0xab; 0x40]);
    m.store(DATA, &vec![0xcd; 0x4000]);
    put_aabb(&mut m, b);
    let sp = STACK - 4 * rng.below(4);
    let data = match rng.below(24) {
        0 => VM + 8,
        1 => CODE,
        2 => sp - 0x100,
        _ => DATA,
    };
    let size = rng.below(4);
    let room = [0u32, 1, 2, 8, 64, 1024, 1024, 4096][rng.below(8) as usize];
    let flags = if rng.chance(2) { 0x8000_0000 } else { 0 };
    m.word(ARRAY + layout::mopp_box::DATA, data);
    m.word(ARRAY + layout::mopp_box::SIZE, size);
    m.word(ARRAY + layout::mopp_box::CAPACITY, (size + room) | flags);
    m.word(sp, lift::harness::HARNESS_EXIT);
    m.word(sp + 4, CODE);
    m.word(sp + 8, AABB);
    m.word(sp + 12, ARRAY);
    let mut c = entry_cpu(rng, MOPP_BOX.point.base);
    c.esp = sp;
    c.ecx = VM;
    c.x87.control = if rng.chance(5) {
        DECLINED[rng.below(2) as usize]
    } else {
        SUPPORTED[rng.below(3) as usize]
    };
    // Half enter with PE clear, as the live game does: the setup's
    // conversions set it as the guest's would.
    if rng.chance(2) {
        c.x87.status |= layout::mopp_box::X87_PE;
    }
    (c, m)
}

#[test]
fn mopp_box_matches_the_guest() {
    with_oracle("collision MOPP box differential test", |oracle, image| {
        const CASES: u32 = 1500;
        let mut native = 0;
        for n in 0..CASES {
            let mut rng =
                Rng(0x510e_527f_ade6_82d1 ^ u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
            let code = program(&mut rng, n);
            let b = if n % 5 == 4 {
                let lo = [rng.eighths(70), rng.eighths(70), rng.eighths(20)];
                let ext = [rng.eighths(6).abs(), rng.eighths(6).abs(), rng.eighths(6).abs()];
                Aabb {
                    min: lo,
                    max: [lo[0] + ext[0], lo[1] + ext[1], lo[2] + ext[2]],
                }
            } else {
                random_aabb(&mut rng, &code)
            };
            let (c, m) = setup(image, &mut rng, &code, &b);
            let ok = check(
                oracle,
                &MOPP_BOX,
                &c,
                &m,
                |c, g, w| adapter::mopp_box(c, g, w),
                None,
                &format!("case {n}"),
            );
            native += u32::from(ok);
        }
        report("MOPP box", native, CASES);
    });
}

/// Boxes whose corners sit exactly on frame-cell edges, through programs
/// of every opcode class, in the supported environment: every call must
/// answer natively and match.
#[test]
fn mopp_box_edges_match_the_guest() {
    with_oracle("collision MOPP box edges", |oracle, image| {
        let programs: [&[u8]; 10] = [
            // Both splits visit both sides. The duplicate surrounds a distinct
            // key, so sorting, uniquing, or reversing a sibling pair differs.
            &[0x10, 255, 0, 6, 0x10, 255, 0, 1, 0x37, 0x32, 0x37],
            &[0x10, 10, 20, 1, 0x31, 0x32],
            &[0x23, 10, 20, 0, 0, 0, 1, 0x31, 0x32],
            &[0x20, 10, 1, 0x31, 0x32],
            &[0x26, 10, 20, 0x31],
            &[0x29, 0, 0x0a, 0, 0x14, 0, 0, 0x31],
            &[0x04, 1, 0, 0, 0x26, 2, 5, 0x31],
            &[0x17, 10, 12, 1, 0x31, 0x32],
            &[0x01, 3, 5, 7, 0x1c, 140, 150, 1, 0x31, 0x32],
            &[0x0b, 0, 0, 1, 0, 0x60, 9, 0x10, 255, 0, 1, 0x00, 0x51, 1, 2],
        ];
        let mut n = 0u32;
        for (p, bytecode) in programs.iter().enumerate() {
            let code = Code {
                offset: [0.0; 3],
                scale: 1.0,
                info: mopp::code::RETAIL_INFO,
                bytecode: bytecode.to_vec(),
            };
            for lo_cell in [0u32, 8, 9, 10, 11, 19, 20, 21] {
                for (lo_in, hi_in) in [(0.0f32, 0.0f32), (0.0, 65535.0), (65535.0, 65536.0), (1.0, 1.0)] {
                    for span in [0u32, 1, 2, 10] {
                        let lo = (lo_cell * 65536) as f32 + lo_in;
                        let hi = ((lo_cell + span) * 65536) as f32 + hi_in;
                        let b = Aabb {
                            min: [lo, lo, 0.0],
                            max: [hi.max(lo), hi.max(lo), 1.0],
                        };
                        let mut rng = Rng(u64::from(n) | 1);
                        let (mut c, mut m) = setup(image, &mut rng, &code, &b);
                        (c.mxcsr, c.x87.control, c.x87.tag) = (0x1f80, 0x027f, 0xffff);
                        // PE clear on odd boxes: the edge inputs are exact
                        // and inexact by turns.
                        if n % 2 == 0 {
                            c.x87.status |= layout::mopp_box::X87_PE;
                        }
                        m.word(ARRAY + layout::mopp_box::DATA, DATA);
                        m.word(ARRAY + layout::mopp_box::CAPACITY, 1024);
                        let label = format!("program {p} box {b:?}");
                        assert!(
                            check(oracle, &MOPP_BOX, &c, &m, |c, g, w| adapter::mopp_box(c, g, w), None, &label),
                            "{label}: declined"
                        );
                        n += 1;
                    }
                }
            }
        }
    });
}

/// Boxes around a structure: vertices of its collision BSP, half extents
/// from 2 cm to 22 m (the mopp.md sampling).
fn structure_box(samples: &super::disc::Samples<'_>, rng: &mut Rng, i: u32) -> Aabb {
    fn unit(rng: &mut Rng) -> f64 {
        f64::from(rng.next()) / f64::from(u32::MAX)
    }
    let centre = samples.point(rng, i);
    let half = if i.is_multiple_of(16) {
        2.0 + 20.0 * unit(rng)
    } else {
        0.02 * 400f64.powf(unit(rng))
    };
    let mut b = Aabb {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    for a in 0..3 {
        let c = f64::from(centre[a]) + (unit(rng) - 0.5) * 2.0 * half;
        let h = half * (0.25 + unit(rng));
        b.min[a] = (c - h) as f32;
        b.max[a] = (c + h) as f32;
    }
    b
}

#[test]
#[ignore = "reads the owner's Halo 2 ISO (H2_DISC) and XBE (H2_XBE)"]
fn every_disc_mopp_box_matches_the_guest() {
    use super::disc::programs::{self, Kind::*};
    const BOXES: u32 = 400;
    const GUEST_EVERY: u32 = 10;
    assert!(common::xbe_path().is_some(), "set H2_XBE to run the disc sweep");
    with_oracle("collision MOPP box disc sweep", |oracle, image| {
        // Input bytes live below the sparse overlay, so cloning the guest for
        // a comparison copies only the handful of words each case writes.
        let mut fixture = image.to_vec();
        fixture[VM as usize..VM as usize + 0x40].fill(0xab);
        fixture[DATA as usize..DATA as usize + 0x4000].fill(0xcd);
        let mut programs = [0u32; 4];
        let mut compared = [0u64; 4];
        let mut guest_runs = [0u64; 4];
        let mut bails = [0u64; sim_collision::BAILS];
        programs::for_each(&[Structure, Cluster, CollisionModel, PhysicsModel], |tag, kind, view, at, code, samples| {
            let family = kind as usize;
            let tree = Tree::decode(
                at,
                Source {
                    origin: code.offset.map(f32::to_bits),
                    scale: code.scale.to_bits(),
                    bytecode: code.bytecode.clone(),
                },
            );
            programs[family] += 1;
            let label = format!("{tag} {kind:?} {at:#x}");
            eprintln!(
                "{label}: {} bytecode bytes, {} nodes, guest_only {}",
                code.bytecode.len(),
                tree.nodes().len(),
                tree.guest_only()
            );
            let mut rng = Rng(u64::from(at) << 1 | 1);
            for i in 0..BOXES {
                let b = match (kind, samples) {
                    (Structure, Some(samples)) => structure_box(samples, &mut rng, i),
                    _ => frame_box(&mut rng, &code),
                };
                let model = mopp::query_box(&code, &b);
                let ours = Corners::convert(
                    b.min.map(f32::to_bits),
                    b.max.map(f32::to_bits),
                    code.offset.map(f32::to_bits),
                    code.scale.to_bits(),
                    0x027f,
                )
                .and_then(|q| tree.query_box(&q, usize::MAX));
                match ours {
                    Ok(found) => {
                        assert_eq!(Ok(found.keys), model, "{label} box {i}: {b:?}");
                        compared[family] += 1;
                    }
                    Err(why) => bails[why as usize] += 1,
                }
                if i.is_multiple_of(GUEST_EVERY) {
                    let mut m = fresh(&fixture);
                    m.tag = Some((view.address(), view.bytes(), TableVersion::fresh()));
                    put_aabb(&mut m, &b);
                    m.word(ARRAY + layout::mopp_box::DATA, DATA);
                    m.word(ARRAY + layout::mopp_box::SIZE, 0);
                    m.word(ARRAY + layout::mopp_box::CAPACITY, 0x1000);
                    m.word(STACK + 4, at);
                    m.word(STACK + 8, AABB);
                    m.word(STACK + 12, ARRAY);
                    let mut c = entry_cpu(&mut rng, MOPP_BOX.point.base);
                    (c.mxcsr, c.x87.control, c.x87.tag) = (0x1fa0, 0x027f, 0xffff);
                    c.x87.status |= layout::mopp_box::X87_PE;
                    c.ecx = VM;
                    let ok = check(
                        oracle,
                        &MOPP_BOX,
                        &c,
                        &m,
                        |c, g, w| adapter::mopp_box(c, g, w),
                        None,
                        &format!("{label} box {i} guest"),
                    );
                    guest_runs[family] += u64::from(ok);
                }
            }
        });
        for kind in [Structure, Cluster, CollisionModel, PhysicsModel] {
            let k = kind as usize;
            eprintln!("{kind:?}: {} programs, {} boxes compared with mopp::boxvm, {} with the guest", programs[k], compared[k], guest_runs[k]);
            assert!(programs[k] > 0 && compared[k] > 0 && guest_runs[k] > 0, "{kind:?} never compared");
        }
        for why in sim_collision::Bail::ALL {
            if bails[why as usize] > 0 {
                eprintln!("  {}: {}", why.name(), bails[why as usize]);
            }
        }
    });
}
