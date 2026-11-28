//! Private XBE as the oracle, synthetic mutable data. Every store is
//! compared in order (including spills and writes subsequently overwritten).
//! The lifted original, the image and the register conversion are
//! `lifted`'s (docs/engine/shadow-exec.md).
#![cfg(feature = "differential")]

use guest_layout::points;
use engine::sim_native as sim;
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use lifted::Code;
use lifted::cpu::to_interp as ir_cpu;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use std::collections::BTreeMap;

const STACK: u32 = 0x007f_f000;
const A: u32 = 0x0062_0000;
const B: u32 = A + 0x100;
const OUT: u32 = B + 0x100;

#[derive(Clone)]
struct Ram<'a> {
    image: &'a [u8],
    overlay: BTreeMap<u32, u8>,
    writes: Vec<(u32, Vec<u8>)>,
    callbacks: BTreeMap<u32, (u32, u32)>,
    calls: Vec<Cpu>,
}

#[path = "sim_native_differential/unit_pose.rs"]
mod full_pose;

#[path = "sim_native_differential/manager_differential.rs"]
mod manager_differential;

impl<'a> Ram<'a> {
    fn new(image: &'a [u8]) -> Self {
        Self {
            image,
            overlay: BTreeMap::new(),
            writes: Vec::new(),
            callbacks: BTreeMap::new(),
            calls: Vec::new(),
        }
    }

    fn read(&self, a: u32, dst: &mut [u8]) {
        for (i, byte) in dst.iter_mut().enumerate() {
            let p = a + i as u32;
            *byte = self
                .overlay
                .get(&p)
                .copied()
                .unwrap_or(self.image[p as usize]);
        }
    }

    fn store(&mut self, a: u32, data: &[u8]) {
        self.writes.push((a, data.to_vec()));
        for (i, &byte) in data.iter().enumerate() {
            let p = a + i as u32;
            assert!((p as usize) < self.image.len());
            self.overlay.insert(p, byte);
        }
    }

    fn word(&mut self, a: u32, v: u32) {
        self.store(a, &v.to_le_bytes());
    }
}

impl sim::Memory for Ram<'_> {
    fn read_bytes(&self, a: u32, dst: &mut [u8], align: u32) -> Result<(), GuestFault> {
        assert_eq!(a % align, 0);
        self.read(a, dst);
        Ok(())
    }
    fn write_bytes(&mut self, a: u32, data: &[u8], align: u32) -> Result<(), GuestFault> {
        assert_eq!(a % align, 0);
        self.store(a, data);
        Ok(())
    }
}

impl interp::Memory for Ram<'_> {
    fn read(&mut self, a: u32, dst: &mut [u8]) -> Result<(), interp::MemFault> {
        Ram::read(self, a, dst);
        Ok(())
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<(), interp::MemFault> {
        self.store(a, data);
        Ok(())
    }
}

struct Oracle<'a> {
    point: u32,
    image: &'a [u8],
    original: lifted::Original,
}

fn seeded_cpu(entry: u32, mode: usize) -> CpuAbi {
    let mut c = CpuAbi {
        eax: 0x1234_5678,
        ecx: 0x3456_789a,
        edx: 0x2345_6789,
        ebx: 0x4567_89ab,
        esi: 0x1773_5566,
        edi: 0x6257_1918,
        ebp: STACK - 0x80,
        esp: STACK,
        eip: entry,
        eflags: 0x293,
        mxcsr: [0x1f80, 0x1fa0, 0x3f80, 0x5f80, 0x7f80, 0x9fa0][mode % 6],
        ..CpuAbi::default()
    };
    c.x87.control = [0x007f, 0x027f, 0x037f, 0x077f, 0x0b7f, 0x0f7f][mode % 6];
    c.x87.status = ((mode % 8) as u16) << 11;
    c.x87.st = std::array::from_fn(|i| [(i + 1) as u8; 10]);
    c.xmm = std::array::from_fn(|i| {
        let lane = [0x3f80_1000u32, 0x7f80_2001, 0x8000_0000, 0xffc0_1234][i % 4];
        ((u128::from(lane) << 96) | (u128::from(lane) << 64) | 13).to_le_bytes()
    });
    c
}

impl Oracle<'_> {
    fn case(
        &mut self,
        point: &points::Point,
        mode: usize,
        init: impl FnOnce(&mut CpuAbi, &mut Ram<'_>),
        native: impl Fn(&mut CpuAbi, &mut Ram<'_>) -> sim::Result,
    ) {
        if self.point != point.base {
            return;
        }
        let mut c = seeded_cpu(point.base, mode);
        let mut mem = Ram::new(self.image);
        mem.word(STACK, HARNESS_EXIT);
        init(&mut c, &mut mem);
        mem.writes.clear();
        let mut reference = mem.clone();
        let mut guest = ir_cpu(&c);
        self.finish(&mut guest, &mut reference, point.base);
        // Host vector lanes and the scalar loop must each match the guest.
        for simd in [true, false] {
            let (mut c, mut mem) = (c, mem.clone());
            let transfer = sim::with_simd(simd, || native(&mut c, &mut mem)).unwrap();
            if transfer.target_pc == HARNESS_EXIT {
                assert_eq!(transfer, TransferAbi::ret(HARNESS_EXIT));
            }
            let mut got = ir_cpu(&c);
            self.finish(&mut got, &mut mem, transfer.target_pc);
            let label = (point.name, mode, if simd { "simd" } else { "scalar" });
            assert_eq!(mem.calls, reference.calls, "callback arguments {label:?}");
            assert_eq!(got, guest, "architectural state {label:?}");
            assert_eq!(mem.writes, reference.writes, "{label:?}");
            assert_eq!(mem.overlay, reference.overlay, "{label:?}");
        }
    }

    fn finish(&mut self, c: &mut Cpu, mem: &mut Ram<'_>, mut pc: u32) {
        for _ in 0..100_000 {
            c.eip = pc;
            if pc == HARNESS_EXIT {
                return;
            }
            if let Some(&(pop, result)) = mem.callbacks.get(&pc) {
                // Synthetic shape, animation resource or codec behind a
                // call boundary. Include every incoming GPR and XMM in
                // the callback transcript, even registers it ignores.
                mem.calls.push(c.clone());
                let mut bytes = [0; 4];
                mem.read(c.gpr[4], &mut bytes);
                pc = u32::from_le_bytes(bytes);
                c.gpr[4] += 4 + pop;
                c.gpr[0] = result;
                continue;
            }
            let insn = self
                .original
                .insn(pc)
                .unwrap_or_else(|| panic!("function at {pc:#x}"));
            pc = interp::step(c, mem, insn)
                .unwrap_or_else(|e| panic!("at {pc:#x}: {e:?}"))
                .target();
        }
        panic!("step limit at {pc:#x}");
    }
}

fn exercise(point: &points::Point, cases: fn(&mut Oracle<'_>)) {
    let image = lifted::image::Image::require();
    let mut oracle = Oracle {
        point: point.base,
        image: &image.ram.bytes,
        original: image.original(),
    };
    cases(&mut oracle);
}

macro_rules! differential {
    ($name:ident,$point:ident,$cases:ident) => {
        #[test]
        fn $name() {
            exercise(&points::$point, $cases);
        }
    };
}

differential!(transform_compose, OBJECT_TRANSFORM_COMPOSE, pose_cases);
differential!(pose_matrix, OBJECT_POSE_MATRIX, pose_cases);
differential!(rotation_codec, ANIM_ROTATION_CODEC, pose_cases);
differential!(root_transform, OBJECT_ROOT_TRANSFORM, root_cases);
differential!(static_rotation, ANIM_STATIC_ROTATION, codec_cases);
differential!(frame_rotation, ANIM_FRAME_ROTATION, codec_cases);
differential!(blend_rotation, ANIM_BLEND_ROTATION, codec_cases);
differential!(keyed_translation, ANIM_KEYED_TRANSLATION, codec_cases);
differential!(unit_pose, SIM_UNIT_POSE, animation_cases);
differential!(animation_stream, SIM_ANIMATION_STREAM, animation_cases);
differential!(graph_resolve, SIM_GRAPH_RESOLVE, animation_cases);
differential!(marker_transforms, SIM_MARKER_TRANSFORMS, animation_cases);
differential!(mopp_ray, SIM_MOPP_RAY, traversal_cases);
differential!(mopp_box, SIM_MOPP_BOX, traversal_cases);
differential!(bsp_segment, COLLISION_BSP_SEGMENT, traversal_cases);
differential!(point_to_leaf, COLLISION_POINT_TO_LEAF, collision_cases);
differential!(collision_query, SIM_COLLISION_QUERY, traversal_cases);
differential!(geometry_test, COLLISION_GEOMETRY_TEST, collision_cases);
differential!(object_value, OBJECT_FUNCTION_VALUE, object_cases);
differential!(object_import, OBJECT_FUNCTION_IMPORT, object_cases);
differential!(base_object_import, BASE_OBJECT_IMPORT, object_cases);
differential!(compose_lanes, OBJECT_TRANSFORM_COMPOSE, lane_cases);
differential!(pose_matrix_lanes, OBJECT_POSE_MATRIX, lane_cases);
differential!(rotation_codec_lanes, ANIM_ROTATION_CODEC, lane_cases);
differential!(mopp_ray_ties, SIM_MOPP_RAY, tie_cases);
differential!(mopp_box_ties, SIM_MOPP_BOX, tie_cases);

fn sample(i: usize, seed: u32) -> u32 {
    let mixed = (i as u32 ^ seed).wrapping_mul(0x735a_2d97).rotate_left(9);
    match i % 17 {
        0 => 0,
        1 => 1,
        2 => 0x8000_0000,
        3 => 0x7f80_0023,
        4 => 0xffc0_0a01,
        5 => 0x7f80_0000,
        6 => 0x7f7f_ffff,
        _ => (mixed as i32 as f32 / 1.0e8).to_bits(),
    }
}

fn pose_cases(o: &mut Oracle<'_>) {
    for n in 0..36 {
        o.case(
            &points::OBJECT_TRANSFORM_COMPOSE,
            n,
            |c, m| {
                for (base, seed) in [(A, 3), (B, 81)] {
                    for i in 0..13 {
                        m.word(base + 4 * i as u32, sample(n + i, seed));
                    }
                }
                (c.ecx, c.edx) = (A, B);
                m.word(STACK + 4, [OUT, A, B, B - 4, B + 4, A + 4][(n / 6 + n) % 6]);
            },
            |c, m| sim::compose(c, m),
        );
        o.case(
            &points::OBJECT_POSE_MATRIX,
            n,
            |_, m| {
                for i in 0..8 {
                    let v = if n % 3 == 0 {
                        0
                    } else if n % 3 == 1 {
                        (i as f32 - 2.25).to_bits()
                    } else {
                        sample(i + n, 8)
                    };
                    m.word(A + 4 * i as u32, v);
                }
                m.word(sim::pose::POSE_INIT_FLAGS, (n % 2) as u32 | 0x9000_0000);
                m.word(sim::pose::POSE_NORM_THRESHOLD, 1.0e-5f32.to_bits());
                m.word(STACK + 4, [OUT, A, A + 8][(n / 3) % 3]);
                m.word(STACK + 8, A);
            },
            |c, m| sim::pose_matrix(c, m),
        );
        o.case(
            &points::ANIM_ROTATION_CODEC,
            n,
            |_, m| {
                use sim::pose::*;
                m.word(ROTATION_BLOCK, A);
                m.word(ROTATION_TRACK, 2);
                m.word(A + 20, 64);
                m.word(FRAME_A, 1);
                m.word(FRAME_B, 2);
                let first = A + 32 + 2 * 64 + 16;
                for i in 0..8 {
                    m.word(first + 4 * i as u32, sample(n + i, 17));
                }
                m.word(
                    ROTATION_WEIGHT,
                    [0.0f32, 1.0, 0.371, -0.22][n % 4].to_bits(),
                );
                m.word(ROTATION_OUT, [OUT, first, first + 16][n % 3]);
            },
            |c, m| sim::rotation_codec(c, m),
        );
    }
}

/// Both axes, scale and parent cover unit and non-unit scales, a
/// mirrored parent, no parent, and output over the parent.
fn root_cases(o: &mut Oracle<'_>) {
    for n in 0..36 {
        o.case(
            &points::OBJECT_ROOT_TRANSFORM,
            n,
            |c, m| {
                (c.eax, c.ecx) = (A, A + 0x10);
                c.ebx = [0x1700, 0x1701, 0x17ff][n % 3];
                for i in 0..3 {
                    m.word(A + 4 * i as u32, sample(n + i, 7));
                    m.word(A + 0x10 + 4 * i as u32, sample(n + i, 19));
                    m.word(A + 0x20 + 4 * i as u32, ((i as f32 - 1.0) * 2.5).to_bits());
                }
                let unit = 1.0f32.to_bits();
                for i in 0..13 {
                    let v = if i == 0 && n % 4 != 3 { [unit, 0.5f32.to_bits(), unit][n % 3] } else { sample(n + i, 31) };
                    m.word(B + 4 * i as u32, v);
                }
                let scale = [unit, 2.0f32.to_bits(), unit, sample(n, 3)][(n / 3) % 4];
                let parent = if n % 5 == 4 { 0 } else { B };
                let out = [OUT, B, OUT + 4][(n / 2) % 3];
                for (i, v) in [A + 0x20, scale, (n % 2) as u32 * 0x101, parent, out].into_iter().enumerate() {
                    m.word(STACK + 4 + 4 * i as u32, v);
                }
            },
            |c, m| sim::root::transform(c, m),
        );
    }
}

/// Packed int16 quaternion lanes, including both extremes and zero.
fn half(i: usize, seed: u32) -> u16 {
    match (i + seed as usize) % 11 {
        0 => 0,
        1 => 0x8000,
        2 => 0x7fff,
        3 => 0xffff,
        _ => (sample(i, seed) >> 9) as u16,
    }
}

fn codec_cases(o: &mut Oracle<'_>) {
    use sim::pose::*;
    const WEIGHTS: [f32; 6] = [0.0, 1.0, 0.371, -0.22, 0.5, 1.0e-30];
    for n in 0..36 {
        // Rotation records: a stride per track, eight bytes per frame.
        let rotations = |m: &mut Ram<'_>| {
            m.word(ROTATION_BLOCK, A);
            m.word(A + 20, 48);
            m.word(ROTATION_TRACK, (n % 3) as u32);
            m.word(FRAME_A, (n % 4) as u32);
            m.word(FRAME_B, ((n + 1) % 4) as u32);
            for i in 0..80 {
                m.store(A + 32 + 2 * i as u32, &half(i * 7 + n, 5).to_le_bytes());
            }
            let weight = if n % 7 == 6 { sample(n, 29) } else { WEIGHTS[n % 6].to_bits() };
            m.word(ROTATION_WEIGHT, weight);
            m.word(ROTATION_OUT, [OUT, OUT + 16, A + 48][n % 3]);
        };
        o.case(&points::ANIM_STATIC_ROTATION, n, |_, m| rotations(m), |c, m| sim::codec::static_rotation(c, m));
        o.case(&points::ANIM_FRAME_ROTATION, n, |_, m| rotations(m), |c, m| sim::codec::frame_rotation(c, m));
        o.case(&points::ANIM_BLEND_ROTATION, n, |_, m| rotations(m), |c, m| sim::codec::blend_rotation(c, m));
        o.case(
            &points::ANIM_KEYED_TRANSLATION,
            n,
            |_, m| {
                // Track table at +0x40: key count | first key << 12.
                // Frame bytes at +0x60, 12-byte keys at +0x80.
                m.word(ROTATION_BLOCK, A);
                m.word(A + 0xc, 0x40);
                m.word(A + 0x18, 0x60);
                m.word(A + 0x24, 0x80);
                let count = [1u32, 2, 5, 6, 3, 4][n % 6];
                let first = (n % 2) as u32;
                m.word(sim::codec::TRANSLATION_TRACK, (n % 2) as u32);
                m.word(A + 0x40 + 4 * first, count | first << 12);
                for (i, f) in [0u8, 3, 7, 7, 12, 20, 31].into_iter().enumerate() {
                    m.store(A + 0x60 + i as u32, &[f]);
                }
                for i in 0..21 {
                    m.word(A + 0x80 + 4 * i as u32, sample(i + n, 13));
                }
                m.word(FRAME_A, [0u32, 2, 3, 7, 11, 30, 40, 255][n % 8]);
                m.word(ROTATION_WEIGHT, WEIGHTS[n % 6].to_bits());
                m.word(ROTATION_OUT, [OUT, A + 0x80, OUT + 4][n % 3]);
                for i in 0..3 {
                    m.word(OUT + 0x10 + 4 * i, sample(i as usize + n, 3));
                }
            },
            |c, m| sim::codec::keyed_translation(c, m),
        );
    }
}

fn collision_cases(o: &mut Oracle<'_>) {
    for n in 0..36 {
        o.case(
            &points::COLLISION_POINT_TO_LEAF,
            n,
            |c, m| {
                (c.ecx, c.eax, c.edx) = (A, 0xff00_0000, OUT);
                m.word(A + 4, B);
                m.word(A + 12, B + 96);
                for i in 0..4u32 {
                    let node = B + 8 * i;
                    m.store(node, &(i as i16 - 1).to_le_bytes());
                    let low = if i == 3 { 0xff_ffff } else { i + 1 };
                    m.store(node + 2, &low.to_le_bytes()[..3]);
                    m.store(node + 5, &(0x80_0010 + i).to_le_bytes()[..3]);
                    for j in 0..4 {
                        m.word(
                            B + 80 + 16 * i + 4 * j,
                            [1.0f32, 0.0, -1.0, 0.5][j as usize].to_bits(),
                        );
                    }
                }
                for i in 0..3 {
                    m.word(OUT + 4 * i as u32, sample(n + i, 8));
                }
            },
            |c, m| sim::point_to_leaf(c, m),
        );
        o.case(
            &points::COLLISION_GEOMETRY_TEST,
            n,
            |c, m| {
                (c.eax, c.ecx, c.edx) = (A, B, OUT);
                for i in 0..3 {
                    m.word(A + 4 * i, 0);
                    m.word(B + 4 * i, if i == 0 { 1.0f32.to_bits() } else { 0 });
                    let dx = if i == 0 {
                        [-0.2f32, -2.0, 0.0, 1.0][n % 4].to_bits()
                    } else {
                        0
                    };
                    m.word(
                        OUT + 4 * i,
                        if n % 5 == 4 {
                            sample(n + i as usize, 1)
                        } else {
                            dx
                        },
                    );
                }
                m.word(STACK + 4, [0.5f32, 1.0, 2.0][(n / 4) % 3].to_bits());
            },
            |c, m| sim::geometry_test(c, m),
        );
    }
}

const OBJECT: u32 = A + 0x2000;
const TAGS: u32 = A + 0x3800;
const DEFINITION: u32 = A + 0x4000;
const MODEL: u32 = A + 0x4800;
const DESCRIPTOR: u32 = A + 0x5000;
const HANDLER: u32 = A + 0x6000;
const HANDLE: u32 = 0x8567_0003;

fn objects(m: &mut Ram<'_>) {
    m.word(points::OBJECTS.base, A);
    m.word(A + 0x44, B);
    m.word(B + 3 * 12 + 8, OBJECT);
    m.word(OBJECT, 0x1234_0002);
    m.word(OBJECT + 20, u32::MAX);
    m.word(points::TAG_ENTRIES.base, TAGS);
    m.word(TAGS + 2 * 16 + 8, DEFINITION);
    m.word(DEFINITION + 56, 3);
    m.word(TAGS + 3 * 16 + 8, MODEL);
    m.word(MODEL + 4, 4);
    m.word(TAGS + 4 * 16 + 8, MODEL + 0x200);
    m.store(OBJECT + 0xaa, &[2]);
    m.word(sim::object::DESCRIPTORS + 8, DESCRIPTOR);
}

fn image_word(m: &Ram<'_>, addr: u32) -> u32 {
    let mut bytes = [0; 4];
    m.read(addr, &mut bytes);
    u32::from_le_bytes(bytes)
}

fn object_cases(o: &mut Oracle<'_>) {
    for n in 0..36 {
        o.case(
            &points::OBJECT_FUNCTION_IMPORT,
            n,
            |c, m| {
                objects(m);
                c.ebx = OUT;
                c.edi = OUT + 12;
                m.word(STACK + 4, HANDLE);
                m.word(STACK + 8, 17);
                m.word(OUT, 0xface_feed);
                m.word(OUT + 12, 0xdead_beef);
                if n % 3 != 0 {
                    // First descriptor has no callback, second one does.
                    m.word(DESCRIPTOR + 0x84, B + 64);
                    m.word(DESCRIPTOR + 0x88, B + 160);
                    m.word(B + 160 + 0x4c, HANDLER);
                    m.callbacks.insert(HANDLER, (16, (n % 2) as u32));
                }
            },
            |c, m| sim::object::import(c, m),
        );
        o.case(
            &points::OBJECT_FUNCTION_VALUE,
            n,
            |_, m| {
                objects(m);
                m.word(STACK + 4, HANDLE);
                // Keep the original case 1 and 2 inputs: words starting at
                // ModR/M are ordinary names. Also exercise both real names.
                let key = match n % 4 {
                    0 => 0,
                    1 if n == 1 => image_word(m, 0x000b_abaa),
                    2 if n == 2 => image_word(m, 0x000b_abb6),
                    1 => image_word(m, 0x000b_abab),
                    2 => image_word(m, 0x000b_abb7),
                    3 if n % 12 == 11 => image_word(m, 0x000b_cdb3),
                    _ => 13,
                };
                m.word(STACK + 8, key);
                m.word(STACK + 12, OUT);
                if n % 4 == 3 {
                    m.word(DESCRIPTOR + 0x84, B + 128);
                    m.word(B + 128 + 0x4c, points::BASE_OBJECT_IMPORT.base);
                    m.word(OBJECT + 0xec, 0.75f32.to_bits());
                    m.word(DEFINITION + 0x64, 2);
                    m.word(DEFINITION + 0x68, MODEL + 0x800);
                    if n % 8 == 3 {
                        m.word(MODEL + 0x828, key);
                    }
                    m.callbacks.insert(0x000b_a8c0, (8, 1));
                    if n % 12 == 7 {
                        m.word(OBJECT + 4, 1 << 26);
                        m.word(OBJECT + 20, HANDLE + 1);
                        m.word(B + 4 * 12 + 8, OBJECT + 0x800);
                        m.word(OBJECT + 0x800, 2);
                        m.store(OBJECT + 0x8aa, &[2]);
                        m.word(OBJECT + 0x814, u32::MAX);
                    }
                }
                if n % 5 == 4 {
                    m.store(OBJECT + 0xc0, &(1u16 << 9).to_le_bytes());
                    m.callbacks.insert(0x0010_aac0, (12, 1));
                }
            },
            |c, m| {
                let mut transfer = sim::object::value(c, m)?;
                // Alternate the single switch with all three object
                // switches. Each hop uses the same return adapter as boot.
                if n % 8 >= 3 {
                    for _ in 0..32 {
                        transfer = if transfer.target_pc == points::OBJECT_FUNCTION_IMPORT.base {
                            let t = sim::object::import(c, m)?;
                            sim::object::complete_return(c, m, t, false, true)?
                        } else if transfer.target_pc == points::BASE_OBJECT_IMPORT.base {
                            let t = sim::object_base::base_import(c, m)?;
                            sim::object::complete_return(c, m, t, true, true)?
                        } else {
                            break;
                        };
                    }
                }
                Ok(transfer)
            },
        );
        o.case(
            &points::BASE_OBJECT_IMPORT,
            n,
            |_, m| {
                objects(m);
                let sample = [-1.0f32, 0.0, 0.375, 2.5, f32::NAN][(n / 2) % 5].to_bits();
                for offset in [0xec, 0xf0, 0xf4, 0xf8] {
                    m.word(OBJECT + offset, sample);
                }
                m.word(0x004e_6948, MODEL + 0x200);
                m.word(0x004d_e2f4, MODEL + 0x300);
                m.word(MODEL + 0x50, (n % 6) as u32);
                m.store(OBJECT + 0xb1, &[n as u8]);
                m.store(OBJECT + 0x10a, &[n as u8]);
                for addr in [0x000b_ca70, 0x000b_cbb0, 0x000b_cad0, 0x000c_18e0] {
                    m.callbacks.insert(addr, (0, 1));
                }
                m.callbacks.insert(0x000b_c970, (4, 0));
                let operand = [
                    0x000b_cdb3,
                    0x000b_ce07,
                    0x000b_cf04,
                    0x000b_ce85,
                    0x000b_ce55,
                    0x000b_cc48,
                    0x000b_cc85,
                    0x000b_cc90,
                    0x000b_cc9b,
                    0x000b_cd13,
                    0x000b_cd1a,
                    0x000b_cd21,
                    0x000b_cc67,
                    0x000b_ce66,
                    0x000b_cdfc,
                    0x000b_ce77,
                    0x000b_cf46,
                    0x000b_cf8a,
                ][n % 18];
                m.word(STACK + 4, HANDLE);
                let id = if n == 35 {
                    u32::MAX
                } else {
                    image_word(m, operand)
                };
                m.word(STACK + 8, id);
                m.word(STACK + 12, OUT);
                m.word(STACK + 16, OUT + 8);
            },
            |c, m| sim::object_base::base_import(c, m),
        );
    }
}

fn animation_cases(o: &mut Oracle<'_>) {
    for n in 0..18 {
        o.case(
            &points::SIM_GRAPH_RESOLVE,
            n,
            |c, m| {
                m.store(sim::graph::ENABLED, &[u8::from(n != 0), u8::from(n != 1)]);
                c.edx = A;
                m.word(A + 48, B);
                m.word(A + 0xb0, OUT);
                let inherited = n % 2 == 0;
                let index = if n == 2 { 0xffff } else { 0 };
                m.word(
                    STACK + 4,
                    (index << 16) | if inherited { 0 } else { 0xffff },
                );
                m.word(A + 76, 1);
                m.word(A + 80, MODEL);
                m.word(MODEL + 4, 1);
                m.word(points::TAG_ENTRIES.base, TAGS);
                m.word(TAGS + 24, A);
                for i in 0..3 {
                    let clip = B + 108 * i;
                    m.store(
                        clip + 48,
                        &(if n % 3 == 0 && i == 0 { 1u16 } else { u16::MAX }).to_le_bytes(),
                    );
                    m.store(
                        clip + 50,
                        &(if i == 2 { u16::MAX } else { (i + 1) as u16 }).to_le_bytes(),
                    );
                    m.store(clip + 22, &[if n % 4 != 0 { 16 } else { 0 }]);
                    m.word(clip + 36, i);
                    m.store(OUT + 20 * i + 16, &[u8::from((n + i as usize) % 2 != 0)]);
                }
                m.callbacks.insert(0x0012_36f0, (0, 17));
            },
            |c, m| sim::graph::resolve(c, m),
        );
        o.case(
            &points::SIM_UNIT_POSE,
            n,
            |_, m| {
                objects(m);
                let manager = OBJECT + 0x500;
                m.store(OBJECT + 0x33e, &0x500u16.to_le_bytes());
                m.store(OBJECT + 0x12a, &0x400u16.to_le_bytes());
                m.word(OBJECT + 0x468, 2);
                m.word(DEFINITION + 0xbc, 1 << 11);
                m.callbacks.insert(0x000c_8fc0, (0, 0));
                m.callbacks.insert(0x0010_f890, (0, 0));
                m.callbacks.insert(0x001c_6470, (20, 0));
                for offset in [0x9c, 0xbc, 0xdc] {
                    m.word(manager + offset, u32::MAX);
                }
                for offset in [0x106, 0x10a, 0x10e, 0x112] {
                    m.store(manager + offset, &u16::MAX.to_le_bytes());
                }
                match n % 6 {
                    1 => m.word(manager + 0x9c, 0),
                    2 => m.word(manager + 0xbc, 0),
                    3 => m.word(manager + 0xdc, 0),
                    4 => m.word(manager + 0x104, 0),
                    5 => m.word(manager + 0x108, 0),
                    _ => {
                        m.word(DEFINITION + 0xbc, 1 << 10);
                        m.store(manager, &[2]);
                        for offset in [0x118, 0x11c, 0x120] {
                            m.word(manager + offset, u32::MAX);
                        }
                        if n != 0 {
                            m.word(manager + 0x118, 1);
                            m.word(OBJECT + 0x288, if n == 6 { 0.75f32.to_bits() } else { 0 });
                            m.callbacks.insert(0x001c_b490, (8, 0));
                        }
                    }
                }
                for (i, v) in [HANDLE, 3, OUT, OUT + 160].into_iter().enumerate() {
                    m.word(STACK + 4 + 4 * i as u32, v);
                }
                m.callbacks.insert(0x001c_6f60, (16, 0));
            },
            |c, m| sim::unit::pose(c, m),
        );
        o.case(
            &points::SIM_MARKER_TRANSFORMS,
            n,
            |c, m| {
                objects(m);
                m.word(DEFINITION + 56, if n % 3 == 0 { u32::MAX } else { 3 });
                m.store(OBJECT + 0x116, &0x480u16.to_le_bytes());
                m.store(OBJECT + 0x114, &52u16.to_le_bytes());
                for i in 0..13 {
                    m.word(OBJECT + 0x480 + 4 * i, sample(n + i as usize, 56));
                }
                m.word(OBJECT + 4, if n % 2 == 0 { 1 << 10 } else { 0 });
                m.word(MODEL + 0x200 + 88, 2);
                m.word(MODEL + 0x200 + 92, MODEL + 0x300);
                m.word(MODEL + 0x300, 7);
                m.word(MODEL + 0x30c, 2);
                if n % 6 == 2 {
                    m.word(OBJECT + 20, HANDLE + 1);
                    m.store(DEFINITION + 2, &[16]);
                    m.word(B + 4 * 12 + 8, OBJECT + 0x800);
                    m.word(OBJECT + 0x800, 2);
                    m.word(OBJECT + 0x814, u32::MAX);
                }
                c.ecx = (n % 2) as u32;
                c.ebx = OUT;
                m.word(STACK + 4, HANDLE);
                m.word(STACK + 8, (n % 4) as u32);
                m.word(STACK + 12, 1);
                m.callbacks.insert(0x001d_8f50, (24, (n % 2) as u32));
            },
            |c, m| sim::marker::transforms(c, m),
        );
        o.case(
            &points::SIM_ANIMATION_STREAM,
            n,
            |c, m| {
                c.esi = A;
                m.word(A, B);
                m.word(A + 4, OUT);
                m.store(A + 8, &[41]);
                m.store(OUT, &[9]);
                m.store(OUT + 6, &(if n % 2 == 0 { 8u16 } else { 0 }).to_le_bytes());
                m.word(OUT + 12, if n % 3 == 0 { 0 } else { 128 });
                m.store(B, &[1]);
                m.store(B + 8, &[2]);
                m.store(MODEL + 16, &[((n / 3) % 3) as u8]);
                m.store(MODEL + 19, &[22]);
                m.store(MODEL + 20, &5u16.to_le_bytes());
                let time = [0.0f32, 0.00001, 2.625, 4.0, -1.25, 14.0][n % 6];
                c.xmm[1][..4].copy_from_slice(&time.to_le_bytes());
                c.xmm[4][..4].copy_from_slice(&[0.0f32, 0.75, 1.0][n % 3].to_le_bytes());
                for (i, v) in [
                    OUT,
                    MODEL,
                    23,
                    if n % 4 == 0 { 0 } else { DEFINITION },
                    B + 24,
                    B + 40,
                    (n % 2) as u32,
                    ((n / 2) % 2) as u32,
                    3.4f32.to_bits(),
                    0.55f32.to_bits(),
                ]
                .into_iter()
                .enumerate()
                {
                    m.word(STACK + 4 + 4 * i as u32, v);
                }
                m.callbacks.insert(0x0027_9860, (0, 31));
                m.callbacks.insert(0x0027_a100, (20, 1));
            },
            |c, m| sim::stream::select(c, m),
        );
    }
}

fn traversal_cases(o: &mut Oracle<'_>) {
    for n in 0..48 {
        o.case(
            &points::COLLISION_BSP_SEGMENT,
            n,
            |_, m| {
                let (bsp, nodes, planes, leaves, pos, delta) =
                    (B, MODEL, MODEL + 48, TAGS, A + 160, A + 176);
                m.word(A, (n % 8) as u32);
                m.word(A + 4, bsp);
                m.word(A + 16, pos);
                m.word(A + 20, delta);
                m.word(A + 24, OUT);
                m.word(A + 28, 2);
                m.store(A + 32, &[(n % 4) as u8]);
                m.word(A + 36, 0);
                m.word(OUT, 1.0f32.to_bits());
                m.word(bsp + 4, nodes);
                m.word(bsp + 12, planes);
                m.word(bsp + 20, leaves);
                m.word(bsp + 44, DEFINITION);
                m.store(nodes + 2, &[0, 0, 0x80]);
                m.store(nodes + 5, &[1, 0, 0x80]);
                for (addr, v) in [
                    (planes, 0.75f32),
                    (planes + 12, 0.22),
                    (pos, -1.5),
                    (delta, 5.5),
                ] {
                    m.word(addr, v.to_bits());
                }
                m.store(leaves, &[1]);
                m.store(leaves + 4, &[0]);
                let root = match n % 4 {
                    0 => u32::MAX,
                    1 => 0xff80_0000,
                    _ => 0,
                };
                m.word(STACK + 4, A);
                m.word(STACK + 8, root);
                m.word(STACK + 12, 0);
                m.word(STACK + 16, if n % 4 == 3 { 1.0f32.to_bits() } else { 0 });
                m.callbacks
                    .insert(0x001d_ea00, (20, if n % 5 != 0 { u32::MAX } else { 1 }));
            },
            |c, m| sim::bsp::segment(c, m),
        );
        o.case(
            &points::SIM_COLLISION_QUERY,
            n,
            |_, m| {
                let bsp = TAGS;
                m.word(0x004e_0348, DEFINITION);
                m.store(0x0046_86c4, &1u16.to_le_bytes());
                m.word(0x004e_d280, MODEL);
                m.word(0x004e_0340, bsp);
                m.word(bsp + 4, B);
                m.word(bsp + 12, A + 128);
                m.store(B + 2, &[255; 3]);
                m.store(B + 5, &[255; 3]);
                if n % 4 >= 2 {
                    m.word(bsp + 20, TAGS + 0x200);
                    m.word(DEFINITION + 0x30, MODEL + 0x600);
                    m.store(B + 2, &[0, 0, 0x80]);
                    m.store(B + 5, &[0, 0, 0x80]);
                }
                for (i, v) in [((n % 8) as u32) << 23, A, A + 64, u32::MAX, u32::MAX, OUT]
                    .into_iter()
                    .enumerate()
                {
                    m.word(STACK + 4 + 4 * i as u32, v);
                }
            },
            |c, m| {
                let transfer = sim::collision::query(c, m)?;
                if n % 2 == 0 && transfer.target_pc == points::COLLISION_BSP_SEGMENT.base {
                    let transfer = sim::bsp::segment(c, m)?;
                    sim::collision::complete_segment(c, m, transfer)
                } else {
                    Ok(transfer)
                }
            },
        );
        o.case(
            &points::SIM_MOPP_BOX,
            n,
            |c, m| {
                let (state, code, keys) = (MODEL, TAGS, OUT + 64);
                c.ecx = A;
                m.word(A, OUT);
                m.word(OUT, keys);
                m.word(OUT + 8, 32);
                m.word(state, if n % 3 == 0 { 1 } else { 130 });
                m.word(state + 16, if n % 3 == 1 { 124 } else { 3 });
                m.word(state + 44, 97);
                m.word(STACK + 4, state);
                m.word(STACK + 8, code);
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
                m.store(code, stream);
            },
            |c, m| sim::mopp_query::box_query(c, m),
        );
        o.case(
            &points::SIM_MOPP_RAY,
            n,
            |c, m| {
                c.ecx = A;
                m.word(STACK + 4, DEFINITION);
                m.word(STACK + 8, TAGS);
                m.word(STACK + 12, MODEL);
                let (start, end) = [(1.25f32, 6.5f32), (65.0, 41.25), (3.0, 36.0)][n % 3];
                m.word(MODEL, start.to_bits());
                m.word(MODEL + 16, end.to_bits());
                m.word(A + 0x54, 1.0f32.to_bits());
                m.word(A + 100, B);
                m.word(B, OUT);
                m.word(OUT + 52, HANDLER);
                m.word(HANDLER + 256, OUT + 256);
                m.word(OUT + 256 + 36, HANDLER + 16);
                m.word(A + 96, DEFINITION + 128);
                m.word(A + 92, DEFINITION + 160);
                m.callbacks.insert(HANDLER, (8, HANDLER + 256));
                m.callbacks.insert(HANDLER + 16, (12, 0));
                if n % 4 == 0 {
                    m.word(A + 68, B + 16);
                    m.word(B + 16, OUT + 128);
                    m.word(OUT + 128, HANDLER + 32);
                    m.callbacks.insert(HANDLER + 32, (16, OUT + 160));
                }
                m.word(A + 20, 1.0f32.to_bits());
                m.word(DEFINITION + 20, 1.0f32.to_bits());
                let stream: &[u8] = match n % 14 {
                    0 => &[0x05, 1, 0, 0x32],
                    1 => &[0x10, 32, 8, 1, 0x34, 0x36],
                    2 => &[0x10, 32, 8, 1, 0, 0],
                    3 => &[0x06, 0, 1, 0, 0x51, 1, 31],
                    4 => &[0x26, 7, 30, 0x33],
                    5 => &[0x29, 0, 0, 7, 0, 0, 31, 0],
                    6 => &[0x02, 1, 2, 3, 0x60, 5, 0x0b, 0, 1, 2, 3, 0x35],
                    7 => &[0x09, 7, 0x64, 1, 2, 0x0a, 1, 3, 0],
                    8 => &[0x68, 0, 2, 5, 1, 0x26, 6, 30, 0],
                    9 => &[0x26, 1, 32, 0x35],
                    10 => &[0x20, 24, 1, 0, 0],
                    11 => &[0x23, 32, 8, 0, 0, 0, 1, 0, 0],
                    12 => &[0x15, 25, 13, 1, 0x33, 0x31],
                    _ => &[0x1b, 91, 96, 1, 0, 0],
                };
                m.store(TAGS, stream);
            },
            |c, m| sim::mopp_query::ray_query(c, m),
        );
    }
}

/// Mostly magnitudes near one, so whole compositions stay on the native
/// packed fast path, with boundary patterns of its range check mixed in:
/// signed zero, subnormals, the first and last values in range, infinity
/// and both kinds of NaN.
fn dense(i: usize, seed: u32, edges: bool) -> u32 {
    const EDGES: [u32; 16] = [
        0x8000_0000,
        0x0000_0001,
        0x807f_ffff,
        0x0080_0000,
        0x1fff_ffff,
        0x2000_0000,
        0x5eff_ffff,
        0x5f00_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_0000,
        0x7f80_0001,
        0,
        0xbf80_0000,
        0x7f7f_ffff,
        0xdf00_0000,
    ];
    let mixed = (i as u32 ^ seed).wrapping_mul(0x735a_2d97).rotate_left(9);
    if edges && mixed % 11 == 0 {
        EDGES[(mixed >> 8) as usize % EDGES.len()]
    } else {
        (mixed & 0x8000_0000) | (0x3c00_0000 + (mixed >> 5) % 0x0800_0000)
    }
}

fn lane_cases(o: &mut Oracle<'_>) {
    for n in 0..60 {
        let edges = n % 4 != 0;
        o.case(
            &points::OBJECT_TRANSFORM_COMPOSE,
            n,
            |c, m| {
                for (base, seed) in [(A, 5), (B, 91)] {
                    for i in 0..13 {
                        m.word(base + 4 * i as u32, dense(n * 13 + i, seed, edges));
                    }
                }
                (c.ecx, c.edx) = (A, B);
                let out = [OUT, A, B, B - 4, B + 4, A + 4, A + 8][(n / 7 + n) % 7];
                m.word(STACK + 4, out);
            },
            |c, m| sim::compose(c, m),
        );
        o.case(
            &points::OBJECT_POSE_MATRIX,
            n,
            |_, m| {
                for i in 0..8 {
                    m.word(A + 4 * i as u32, dense(n * 8 + i, 23, edges));
                }
                m.word(sim::pose::POSE_INIT_FLAGS, (n % 2) as u32 | 0x9000_0000);
                m.word(sim::pose::POSE_NORM_THRESHOLD, 1.0e-5f32.to_bits());
                m.word(STACK + 4, [OUT, A, A + 8, A + 4][n % 4]);
                m.word(STACK + 8, A);
            },
            |c, m| sim::pose_matrix(c, m),
        );
        o.case(
            &points::ANIM_ROTATION_CODEC,
            n,
            |_, m| {
                use sim::pose::*;
                m.word(ROTATION_BLOCK, A);
                m.word(ROTATION_TRACK, 2);
                m.word(A + 20, 64);
                m.word(FRAME_A, 1);
                m.word(FRAME_B, 2);
                let first = A + 32 + 2 * 64 + 16;
                for i in 0..8 {
                    m.word(first + 4 * i as u32, dense(n * 8 + i, 41, edges));
                }
                m.word(ROTATION_WEIGHT, dense(n, 77, edges));
                m.word(ROTATION_OUT, [OUT, first, first + 16][n % 3]);
            },
            |c, m| sim::rotation_codec(c, m),
        );
    }
}

/// Query boxes and segments that land exactly on split planes and bounds
/// (and one unit either side), so every strict and non-strict comparison
/// decides a child. Visit order and terminal keys come out in the stores.
fn tie_cases(o: &mut Oracle<'_>) {
    // Box program, the tie for the low corner and the tie for the high.
    const BOX: [(&[u8], u32, u32); 6] = [
        (&[0x10, 100, 120, 1, 0x32, 0x33], 100, 120),
        (&[0x20, 95, 1, 0x33, 0x37], 95, 95),
        (&[0x26, 10, 100, 0x50, 27], 100, 10),
        (&[0x23, 100, 10, 0, 0, 0, 1, 0x32, 0x33], 100, 10),
        // Two axes summed: each corner at half the threshold.
        (&[0x13, 100, 120, 1, 0x32, 0x33], 100, 120),
        (&[0x10, 7, 7, 1, 0x34, 0x35], 7, 7),
    ];
    for n in 0..BOX.len() * 9 {
        let (stream, low, high) = BOX[n % BOX.len()];
        let low = low.wrapping_add((n / BOX.len()) as u32 % 3).wrapping_sub(1);
        let high = high
            .wrapping_add((n / BOX.len() / 3) as u32 % 3)
            .wrapping_sub(1);
        o.case(
            &points::SIM_MOPP_BOX,
            n,
            |c, m| {
                let (state, code, keys) = (MODEL, TAGS, OUT + 64);
                c.ecx = A;
                m.word(A, OUT);
                m.word(OUT, keys);
                m.word(OUT + 8, 32);
                for axis in 0..3 {
                    m.word(state + 4 * axis, high);
                    m.word(state + 16 + 4 * axis, low);
                }
                m.word(state + 44, 97);
                m.word(STACK + 4, state);
                m.word(STACK + 8, code);
                m.store(code, stream);
            },
            |c, m| sim::mopp_query::box_query(c, m),
        );
    }

    // Ray programs whose planes sit at integer coordinates, crossed by
    // endpoints placed on them. The other two lanes are dense so the
    // packed clip blends stay on the fast path.
    const RAY: [&[u8]; 8] = [
        &[0x10, 32, 8, 1, 0x34, 0x36],
        &[0x26, 7, 30, 0x33],
        &[0x29, 0, 0, 7, 0, 0, 31, 0],
        &[0x20, 24, 1, 0, 0],
        &[0x23, 32, 8, 0, 0, 0, 1, 0, 0],
        &[0x15, 25, 13, 1, 0x33, 0x31],
        &[0x1b, 91, 96, 1, 0, 0],
        &[0x02, 1, 2, 3, 0x26, 6, 30, 0],
    ];
    const ENDS: [(f32, f32); 7] = [
        (32.0, 8.0),
        (8.0, 8.0),
        (32.0, 32.0),
        (24.0, 25.0),
        (7.0, 30.0),
        (-0.0, 31.0),
        (25.0, 13.0),
    ];
    for n in 0..RAY.len() * ENDS.len() {
        let stream = RAY[n % RAY.len()];
        let (start, end) = ENDS[n / RAY.len()];
        o.case(
            &points::SIM_MOPP_RAY,
            n,
            |c, m| {
                c.ecx = A;
                m.word(STACK + 4, DEFINITION);
                m.word(STACK + 8, TAGS);
                m.word(STACK + 12, MODEL);
                m.word(MODEL, start.to_bits());
                m.word(MODEL + 16, end.to_bits());
                for lane in 1..4u32 {
                    let i = n * 8 + lane as usize;
                    m.word(MODEL + 4 * lane, dense(i, 3, false));
                    m.word(MODEL + 16 + 4 * lane, dense(i, 4, false));
                }
                m.word(A + 0x54, 1.0f32.to_bits());
                m.word(A + 100, B);
                m.word(B, OUT);
                m.word(OUT + 52, HANDLER);
                m.word(HANDLER + 256, OUT + 256);
                m.word(OUT + 256 + 36, HANDLER + 16);
                m.word(A + 96, DEFINITION + 128);
                m.word(A + 92, DEFINITION + 160);
                m.callbacks.insert(HANDLER, (8, HANDLER + 256));
                m.callbacks.insert(HANDLER + 16, (12, 0));
                m.word(A + 20, 1.0f32.to_bits());
                m.word(DEFINITION + 20, 1.0f32.to_bits());
                m.store(TAGS, stream);
            },
            |c, m| sim::mopp_query::ray_query(c, m),
        );
    }
}
