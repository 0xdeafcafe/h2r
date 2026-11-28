//! The typed projectile kernels (`sim_objects::projectile`) against the
//! register-level bodies (`projectile`, `projectile_step`) on the same
//! inputs: the update's velocity and the arguments it hands the mover and
//! the step, and the step's timers up to its first guest call after them.
//! Synthetic memory, no game data.

use super::{Cx, Next, projectile, projectile_step};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use guest_layout::{FlatRam, Mem, MemMut};
use ::sim_objects::projectile::{Body, Def, advance_timers, update};
use runtime::guest::Permissions;
use runtime::{CpuAbi, Guest};

const STACK: u32 = 0x8000;
const ARRAY: u32 = 0x0050_1000;
const HEADERS: u32 = 0x0050_2000;
const TAGS: u32 = 0x0050_3000;
const TIME: u32 = 0x0050_4000;
const DEF: u32 = 0x0050_5000;
const BODY: u32 = 0x0060_0000;
const NONE: u32 = u32::MAX;
const MOVER: u32 = 0x0010_9a00;
/// The image's float constant one.
const ONE: u32 = 0x0045_dbc0;

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }

    fn float(&mut self) -> u32 {
        const SET: [f32; 9] = [0.0, -0.0, 0.25, 0.5, 0.75, 1.0, 1.5, -2.0, f32::NAN];
        match self.next() % 4 {
            0 => SET[(self.next() % 9) as usize].to_bits(),
            1 => self.next(),
            _ => ((self.next() % 4000) as f32 / 1000.0).to_bits(),
        }
    }
}

fn map(g: &mut Guest, addr: u32, len: u32) {
    let mut a = addr & !0xfff;
    while a < addr + len {
        if g.u32(a).is_err() {
            g.map_zeroed(a, 0x1000, Permissions::RW).unwrap();
        }
        a += 0x1000;
    }
}

/// The pools both bodies read: the object array, the tag table, the game
/// time, the stack, one projectile at slot 1 and its definition.
fn world() -> Guest {
    let mut g = Guest::new();
    for (a, n) in [
        (STACK - 0x1000, 0x2000),
        (points::OBJECTS.base, 4),
        (points::TAG_ENTRIES.base, 4),
        (points::GAME_TIME.base, 4),
        (ARRAY, 0x100),
        (HEADERS, 0x100),
        (TAGS, 0x100),
        (TIME, 0x10),
        (DEF, 0x100),
        (BODY, 0x400),
        (ONE, 4),
    ] {
        map(&mut g, a, n);
    }
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    g.set_u32(ARRAY + DATA, HEADERS).unwrap();
    // Slot 1: header offset 0xc, body pointer at `+8`.
    g.set_u32(HEADERS + 0xc + 8, BODY).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    // The body's definition is tag 2, whose entry points at `DEF`.
    g.set_u32(BODY, 2).unwrap();
    g.set_u32(TAGS + 2 * 16 + 8, DEF).unwrap();
    g.set_u32(points::GAME_TIME.base, TIME).unwrap();
    g.set_u32(ONE, 1.0f32.to_bits()).unwrap();
    g.set_u32(TIME + 4, 0.0f32.to_bits()).unwrap();
    g
}

fn cpu(g: &mut Guest) -> CpuAbi {
    g.set_u32(STACK, 0xdead).unwrap();
    g.set_u32(STACK + 4, 0x7_0001).unwrap();
    g.set_u32(STACK + 8, STACK + 0x100).unwrap();
    CpuAbi { esp: STACK, ..Default::default() }
}

/// The body's bytes as flat memory, for the typed side.
fn flat(g: &Guest, at: u32, len: usize) -> FlatRam {
    let mut b = vec![0; len];
    g.read_bytes(at, &mut b, 1).unwrap();
    FlatRam::from_bytes(at, b)
}

fn get<M: Mem>(m: &M, addr: u32) -> u32 {
    let mut b = [0; 4];
    m.read_bytes(addr, &mut b).unwrap();
    u32::from_le_bytes(b)
}

#[test]
fn timers_match_the_step_up_to_its_first_call_after_them() {
    let mut rng = Lcg(7);
    let mut g = world();
    for case in 0..4000 {
        let flags = rng.next() & 0xffff_ffff;
        let kind = (rng.next() % 7) as i16 - 1;
        g.set_u32(BODY + 0x12c, flags).unwrap();
        g.set_u16(BODY + 0x130, [0xffff, 0, 1, 2, 0x8000, 0x7fff][(rng.next() % 6) as usize]).unwrap();
        for off in [0x158, 0x15c, 0x160, 0x164] {
            let v = rng.float();
            g.set_u32(BODY + off, v).unwrap();
        }
        g.set_u16(BODY + 0x1aa, [0, 0xffff, 5][(rng.next() % 3) as usize]).unwrap();
        g.set_u16(DEF + 0xc0, kind as u16).unwrap();
        g.set_u32(BODY + 0x14c, NONE).unwrap();
        g.set_u32(TIME + 4, 0.0f32.to_bits()).unwrap();

        // Typed side, on a copy.
        // The definition sits after the body in the typed side's memory.
        let mut typed = FlatRam::new(BODY, 0x1000);
        typed.write_bytes(BODY, flat(&g, BODY, 0x400).bytes()).unwrap();
        typed.write_bytes(BODY + 0x800, flat(&g, DEF, 0x100).bytes()).unwrap();
        advance_timers(&mut typed, Body::at(BODY), Def::at(BODY + 0x800)).unwrap();

        // Register-level side: attachment calls answered "none".
        let mut c = cpu(&mut g);
        let mut pc = points::PROJECTILE_STEP.base;
        for _ in 0..64 {
            match projectile_step::projectile_step(&mut Cx { cpu: &mut c, g: &mut g }, pc) {
                Ok(Next::Call { target, site }) => match target {
                    0xfd410 => {
                        c.eax = NONE;
                        pc = site;
                    }
                    0x17c880 | 0x16ba40 => pc = site,
                    0xfd460 => {
                        c.esp += 4;
                        pc = site;
                    }
                    _ => break,
                },
                _ => break,
            }
        }
        // The attachment calls precede the timers only without flags bit 1;
        // the step writes the attachment slot either way identically.
        for off in [0x12c, 0x130, 0x158, 0x15c, 0x160, 0x164, 0x1aa] {
            let want = g.u32(BODY + off).unwrap();
            let got = get(&typed, BODY + off);
            let mask = if off == 0x130 { 0xffff } else if off == 0x1aa { 0xffff } else { u32::MAX };
            assert_eq!(got & mask, want & mask, "case {case} +{off:#x} flags {flags:#x} kind {kind}");
        }
    }
}

/// `update` against `f8de0`: what the mover and the step were handed and
/// the velocity left.
#[test]
fn update_matches_f8de0() {
    let mut rng = Lcg(11);
    let mut g = world();
    for case in 0..4000 {
        let flags = (rng.next() & 0xffff) | if rng.next() % 2 == 0 { 0 } else { 0xc000 };
        let latch = (rng.next() & 0xff) as u8;
        g.set_u32(BODY + 0x12c, flags).unwrap();
        g.set_u8(BODY + 0xc1, latch).unwrap();
        let v: [u32; 3] = [rng.float(), rng.float(), rng.float()];
        let d: [u32; 3] = [rng.float(), rng.float(), rng.float()];
        let after: [u32; 3] = [rng.float(), rng.float(), rng.float()];
        let answer = rng.next() % 2 == 0;
        for i in 0..3 {
            g.set_u32(BODY + 0x88 + 4 * i, v[i as usize]).unwrap();
        }

        let mut m = flat(&g, BODY, 0x400);
        let mut typed_calls = (None, None);
        update(
            &mut m,
            Body::at(BODY),
            |_, flag| {
                typed_calls.0 = Some(flag);
                Ok(answer.then(|| d.map(f32::from_bits)))
            },
            |m, vel| {
                typed_calls.1 = Some(vel.map(f32::to_bits));
                Body::at(BODY).set(m, Body::VELOCITY, after.map(f32::from_bits))
            },
        )
        .unwrap();

        let mut c = cpu(&mut g);
        let mut old_calls = (None, None);
        let mut pc = points::PROJECTILE_UPDATE.base;
        loop {
            match projectile::projectile_update(&mut Cx { cpu: &mut c, g: &mut g }, pc).unwrap() {
                Next::Call { target: MOVER, site } => {
                    let (flag, out) = (g.u32(c.esp + 8).unwrap(), g.u32(c.esp + 4).unwrap());
                    old_calls.0 = Some(flag & 0xff != 0);
                    if answer {
                        for i in 0..3 {
                            g.set_u32(out + 4 * i, d[i as usize]).unwrap();
                        }
                    }
                    c.eax = u32::from(answer);
                    c.esp += 0xc;
                    pc = site;
                }
                Next::Call { target, site } => {
                    assert_eq!(target, points::PROJECTILE_STEP.base);
                    let ptr = g.u32(c.esp + 4).unwrap();
                    old_calls.1 = Some([0, 4, 8].map(|o| g.u32(ptr + o).unwrap()));
                    for i in 0..3 {
                        g.set_u32(BODY + 0x88 + 4 * i, after[i as usize]).unwrap();
                    }
                    c.esp += 8;
                    pc = site;
                }
                Next::Ret { pop } => {
                    assert_eq!(pop, 4);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(typed_calls, old_calls, "case {case} flags {flags:#x} latch {latch:#x}");
        for i in 0..3 {
            assert_eq!(get(&m, BODY + 0x88 + 4 * i), g.u32(BODY + 0x88 + 4 * i).unwrap(), "case {case} lane {i}");
        }
    }
}
