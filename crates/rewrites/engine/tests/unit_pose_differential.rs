//! The resumed unit pose against the original, one guest call or return at a time.
use super::*;
use engine::sim_native::unit;
use lifted::cpu::from_interp;
use lifted::image::Rng;
use runtime::guest::Access;
use std::collections::BTreeSet;

impl unit::PoseMemory for Ram<'_> {
    fn checked_offset(&self, addr: u32, len: usize, _: Access, alignment: u32) -> Option<u32> {
        let end = (addr as usize).checked_add(len)?;
        (end <= self.image.len() && addr % alignment == 0).then_some(addr)
    }
}

const MANAGER: u32 = OBJECT + 0x600;
const ANIMATION: u32 = OBJECT + 0x400;
const LIMITS: u32 = A + 0x8000;
const BASIS: u32 = A + 0x9000;

fn number(rng: &mut Rng) -> f32 {
    (rng.below(1001) as i32 - 500) as f32 / 137.0
}

fn vector(m: &mut Ram<'_>, at: u32, v: [f32; 3]) {
    for (i, value) in v.into_iter().enumerate() {
        m.word(at + 4 * i as u32, value.to_bits());
    }
}

fn fixture(m: &mut Ram<'_>, case: usize) {
    let mut rng = Rng(0x1cab_005e + case as u64);
    objects(m);
    for (i, arg) in [HARNESS_EXIT, HANDLE, 3, OUT, OUT + 160]
        .into_iter()
        .enumerate()
    {
        m.word(STACK + 4 * i as u32, arg);
    }
    // Dirty stack scratch detects skipped stores and reads from the wrong local.
    for offset in (4..=0x100).step_by(4) {
        m.word(STACK - offset, rng.u32());
    }
    m.store(OBJECT + 0x33e, &((MANAGER - OBJECT) as u16).to_le_bytes());
    m.store(OBJECT + 0x12a, &((ANIMATION - OBJECT) as u16).to_le_bytes());
    m.word(ANIMATION + 0x68, 2);
    m.word(
        DEFINITION + 0xbc,
        match case {
            6 => 1 << 10,
            7 => 1 << 11,
            _ => 0,
        },
    );
    let animation_name = match case % 5 {
        0 => image_word(m, 0x112783), // First name with an extra overlay gate.
        1 => image_word(m, 0x11278a), // Second name.
        2 => image_word(m, 0x1123c4), // Suppresses the aim layers.
        _ => rng.u32(),
    };
    m.word(ANIMATION + 0x7c, animation_name);
    m.store(MANAGER, &[if case == 5 { 0 } else { 2 }]);
    m.store(MANAGER + 0x36, &u16::MAX.to_le_bytes());
    m.word(OBJECT + 0x208, 0.8f32.to_bits());
    m.word(OBJECT + 0x13c, if case == 9 { u32::MAX } else { HANDLE });
    m.store(
        OBJECT + 0x212,
        if case % 2 == 0 { &[3, 4] } else { &[255, 4] },
    );

    for offset in [0x9c, 0xbc, 0xdc] {
        m.word(
            MANAGER + offset,
            if case == 4 { u32::MAX } else { rng.below(600) },
        );
        m.store(MANAGER + offset + 6, &(rng.below(40) as u16).to_le_bytes());
    }
    for offset in (0xfc..=0x120).step_by(4) {
        m.word(MANAGER + offset, rng.below(500));
    }
    match case {
        15 => m.word(MANAGER + 0xfc, u32::MAX),
        16 => m.word(MANAGER + 0x100, u32::MAX),
        18 => {
            m.word(MANAGER + 0xfc, u32::MAX);
            m.word(MANAGER + 0x100, u32::MAX);
        }
        _ => {}
    }
    for offset in [0x81, 0x8d, 0x91] {
        m.store(MANAGER + offset, &[u8::from(case != 14)]);
    }
    for (i, value) in [-1.25f32, 0.0, 1.5].into_iter().enumerate() {
        m.word(OBJECT + 0x288 + 4 * i as u32, value.to_bits());
    }
    for offset in [0x168, 0x18c] {
        let direction = match case {
            13 => [0.0, -0.0, 0.0],
            20 => [f32::from_bits(0x7f80_4321), -0.0, f32::from_bits(1)],
            _ => std::array::from_fn(|_| number(&mut rng)),
        };
        vector(m, OBJECT + offset, direction);
    }
    m.word(0x4687c4, BASIS);
    m.word(BASIS, 0.125f32.to_bits());
    m.word(BASIS + 4, (-0.25f32).to_bits());
    for limits in [LIMITS, LIMITS + 0x40, LIMITS + 0x80] {
        for offset in [0, 4, 12, 16] {
            m.word(limits + offset, number(&mut rng).to_bits());
        }
        for offset in [8, 10, 20, 22] {
            m.store(limits + offset, &(rng.u32() as i16).to_le_bytes());
        }
    }
    m.word(LIMITS + 0x1c, 0.3125f32.to_bits());
    m.writes.clear();
}

// Apply the same callee contract independently to each side's own registers and
// stack. Real out-parameters exercise frame aliasing across the return boundary.
fn answer(c: &mut CpuAbi, m: &mut Ram<'_>, transfer: TransferAbi, case: usize) {
    let site = image_word(m, c.esp);
    assert_eq!(site, transfer.resume_pc);
    let mut rng = Rng((case as u64) << 32 | u64::from(site));
    let mut result = rng.u32();
    let pop = match transfer.target_pc {
        0xb9fc0 => {
            vector(m, c.esi, [0.625, -0.25, 0.875]);
            vector(m, c.edi, [-0.375, 0.8125, 0.25]);
            0
        }
        0xc8fc0 => {
            result = if case % 2 == 0 { LIMITS } else { 0 };
            0
        }
        0x10f890 | 0x10ee20 => {
            result = u32::from(case % 4 != 3);
            0
        }
        0x111590 => {
            result = u32::from(case % 3 != 2);
            4
        }
        0x11df30 => {
            // In the full-weight case both direction deltas are zero. The
            // original checks both x87 magnitudes and omits the evaluator.
            for offset in [0, 4] {
                let angle = if case == 4 {
                    image_word(m, c.eax + 8 + offset)
                } else {
                    number(&mut rng).to_bits()
                };
                m.word(c.eax + offset, angle);
            }
            0
        }
        0x1c62f0 => {
            m.word(c.ecx, u32::MAX);
            0
        }
        0x1c6470 => {
            m.word(c.eax, rng.below(64));
            result = u32::from(case % 4 != 2);
            20
        }
        0x1c6f60 | 0x1c7200 => 16,
        0x1c7100 | 0x1cb410 => {
            result = u32::from(case != 11);
            24
        }
        0x1cb490 => {
            let descriptor = image_word(m, c.esp + 4);
            let handle = image_word(m, c.esp + 8);
            let found = case != 8;
            m.word(descriptor, if found { handle } else { u32::MAX });
            m.store(
                descriptor + 6,
                &(if found { handle as u16 } else { u16::MAX }).to_le_bytes(),
            );
            result = u32::from(found);
            8
        }
        0x1cc150 => {
            m.word(c.esi, if case != 10 { 13 } else { u32::MAX });
            0
        }
        0x1d9240 => 4,
        0x1d9370 => {
            let weight = match (site, case % 6) {
                (0x1127f5, 1) => 1.0,
                (0x1127f5, 2) => 0.0,
                (0x1127e4, 3) => 0.0,
                (0x112bbb, 4) => 1.0,
                (0x112ca2, _) => -0.25,
                _ => 0.625f32,
            };
            c.xmm[0][..4].copy_from_slice(&weight.to_bits().to_le_bytes());
            0
        }
        0x1daf30 => {
            result = match site {
                0x11259f if case % 4 == 1 || case % 4 == 3 => 0,
                0x1125c8 if case % 4 >= 2 => 0,
                0x11259f => LIMITS,
                0x1125c8 => LIMITS + 0x40,
                0x112a56 if case == 10 => 0,
                0x112a56 => LIMITS + 0x80,
                _ => panic!("unexpected limit lookup at {site:#x}"),
            };
            4
        }
        target => panic!("unstubbed pose callee {target:#x} at {site:#x}"),
    };
    // The basis, track lookup and weight callbacks leave distinct high
    // lanes. MOVSS from memory clears them, while scalar SSE arithmetic
    // and CVTSI2SS must retain the destination's other lanes.
    if matches!(transfer.target_pc, 0xb9fc0 | 0x1daf30 | 0x1d9370) {
        for reg in &mut c.xmm[..7] {
            for lane in reg[4..].chunks_exact_mut(4) {
                lane.copy_from_slice(&rng.u32().to_le_bytes());
            }
        }
    }
    c.eax = result;
    c.esp += 4 + pop;
    c.eip = site;
}

#[test]
fn unit_pose_segments() {
    let image = lifted::image::Image::require();
    let entry = points::SIM_UNIT_POSE.base;
    let mut code = image.original();
    code.prime(entry);
    let mut covered = BTreeSet::new();
    for case in 0..24 {
        let mut native_mem = Ram::new(&image.ram.bytes);
        fixture(&mut native_mem, case);
        let mut guest_mem = native_mem.clone();
        let mut native = seeded_cpu(entry, case);
        native.x87.control = [0x007f, 0x027f, 0x037f][case % 3];
        let mut guest = ir_cpu(&native);
        for boundary in 0..100 {
            let pc = native.eip;
            let label = format!("case {case} boundary {boundary} from {pc:#x}");
            if pc != entry {
                assert!(
                    unit::SITES.contains(&pc),
                    "unregistered return site {label}"
                );
                covered.insert(pc);
            }
            native_mem.writes.clear();
            guest_mem.writes.clear();
            if case == 0 && pc == 0x1125c8 {
                // The interpreter's FPATAN models nearest rounding. The
                // native segment must decline before spilling the dot products.
                let mut directed = native;
                directed.x87.control |= 0x0400;
                let saved = directed;
                assert!(
                    unit::run(&mut directed, &mut native_mem, pc)
                        .unwrap()
                        .is_none()
                );
                assert_eq!(directed, saved);
                assert!(native_mem.writes.is_empty());
            }
            let actual = unit::run(&mut native, &mut native_mem, pc)
                .unwrap()
                .unwrap_or_else(|| panic!("native segment declined {label}"));
            let expected = lifted::code::segment(&mut code, &mut guest, &mut guest_mem, 1000)
                .unwrap_or_else(|e| panic!("original {label}: {e}"));
            assert_eq!(actual, expected, "transfer {label}");
            native.eip = actual.target_pc;
            assert_eq!(native, from_interp(&guest), "CPU {label}");
            assert_eq!(
                native_mem.writes, guest_mem.writes,
                "store sequence {label}"
            );
            assert_eq!(native_mem.overlay, guest_mem.overlay, "RAM {label}");
            if actual == TransferAbi::ret(HARNESS_EXIT) {
                break;
            }
            assert_eq!(actual.kind, runtime::abi::CALL, "{label}");
            assert!(boundary < 99, "unbounded callbacks {label}");
            let mut regs = from_interp(&guest);
            answer(&mut regs, &mut guest_mem, expected, case);
            guest = ir_cpu(&regs);
            answer(&mut native, &mut native_mem, actual, case);
        }
    }
    assert_eq!(
        covered,
        unit::SITES.iter().copied().collect(),
        "missing pose return sites"
    );
}
