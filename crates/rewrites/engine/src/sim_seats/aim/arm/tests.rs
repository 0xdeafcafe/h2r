use super::*;
use runtime::guest::Permissions;
use std::sync::Mutex;

/// The counters are process-wide: tests of the arm run one at a time.
static ARM: Mutex<()> = Mutex::new(());

const HEADER: u32 = 0x0060_0000;
const DATA: u32 = 0x0060_1000;
const UNIT: u32 = 0x0060_2000;
const WEAPON: u32 = 0x0060_3000;
const ENTRIES: u32 = 0x0060_4000;
const DEF: u32 = 0x0060_5000;
const OUT: u32 = 0x0060_6000;
const STACK: u32 = 0x007f_0000;
const RETURN: u32 = 0x0012_3456;

/// A unit holding one weapon with no zoom levels (magnification 1, no
/// x87), its cone fields and one trigger.
fn call() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0x0040_0000, 0x0010_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(0x0060_0000, 0x0001_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW)
        .unwrap();
    g.set_u32(ONE, 1.0f32.to_bits()).unwrap();
    g.set_u32(points::OBJECTS.base, HEADER).unwrap();
    g.set_u32(HEADER + 0x44, DATA).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, ENTRIES).unwrap();
    // Unit datum 2, weapon datum 6; the weapon's tag index 3.
    g.set_u32(DATA + 2 * 12 + 8, UNIT).unwrap();
    g.set_u32(DATA + 6 * 12 + 8, WEAPON).unwrap();
    g.set_u8(UNIT + 0x212, 0).unwrap();
    g.set_u8(UNIT + 0x213, 0xff).unwrap();
    g.set_u32(UNIT + 0x218, 0x8300_0006).unwrap();
    g.set_u32(WEAPON, 3).unwrap();
    g.set_u32(ENTRIES + 3 * 16 + 8, DEF).unwrap();
    for (o, v) in [
        (0x208, 0.05f32),
        (0x20c, 12.0),
        (0x210, 0.09),
        (0x214, 8.0),
        (0x218, 0.12),
    ] {
        g.set_u32(DEF + o, v.to_bits()).unwrap();
    }
    g.set_u16(DEF + 0x294, 1).unwrap();
    g.set_u32(DEF + 0x2c8, 1).unwrap();
    g.set_u32(DEF + 0x2cc, DEF + 0x800).unwrap();
    g.set_u16(DEF + 0x800 + 6, 5).unwrap();
    g.set_u32(STACK, RETURN).unwrap();
    g.set_u32(STACK + 4, 0xffff).unwrap();
    let mut cpu = CpuAbi {
        esp: STACK,
        ecx: 0x8200_0002,
        esi: OUT,
        eax: 0xaabb_ccdd,
        edx: 0x1122_3344,
        mxcsr: 0x1fa0,
        ..Default::default()
    };
    cpu.x87.control = 0x027f;
    (cpu, g)
}

/// The guest's answer to the call: the prediction itself. The record it
/// writes is the same on every call of the fixture.
fn guest_answer(p: &Prediction, cpu: &CpuAbi, g: &mut Guest) -> CpuAbi {
    let mut c = *cpu;
    apply(p, &mut c, g).unwrap();
    c
}

#[test]
fn a_held_weapon_answers_the_record_and_retails_registers() {
    let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
    let (cpu, g) = call();
    let p = predict(&cpu, &g).unwrap();
    let (_, words) = p.record.expect("armed");
    let f = |v: f32| v.to_bits();
    // Magnification 1; an idle kind-5 trigger on a class-1 weapon tracks.
    assert_eq!(
        words,
        [
            f(0.05),
            f(12.0),
            f(0.09),
            f(8.0),
            f(0.12),
            0x20,
            0,
            0,
            f(12.0)
        ]
    );
    assert_eq!(p.cpu.esp, STACK + 8);
    assert_eq!(
        p.cpu.eax,
        (DATA & 0xffff_0000) | 0xff01,
        "the second slot's byte, then al"
    );
    assert_eq!(
        p.cpu.ecx,
        f(0.12),
        "the first bend, popped from push ecx's slot"
    );
    assert_eq!(p.cpu.edx, HEADER);
    assert_eq!(p.cpu.xmm[0], [0; 16]);
    assert_eq!(p.cpu.xmm[1], u128::from(f(1.0)).to_le_bytes());
    assert_eq!(p.cpu.x87, cpu.x87, "no zoom level: no x87");
}

#[test]
fn the_unarmed_exits_leave_retails_registers() {
    let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cpu, mut g) = call();
    cpu.ecx = u32::MAX;
    let p = predict(&cpu, &g).unwrap();
    assert_eq!(
        (p.record, p.cpu.eax, p.cpu.ecx, p.cpu.edx),
        (None, 0xaabb_cc00, u32::MAX, cpu.edx)
    );
    cpu.ecx = 0x8200_0002;
    g.set_u8(UNIT + 0x212, 0xff).unwrap();
    assert_eq!(predict(&cpu, &g).unwrap().cpu.edx, 0x1122_ffff);
    g.set_u8(UNIT + 0x212, 1).unwrap();
    g.set_u32(UNIT + 0x21c, u32::MAX).unwrap();
    assert_eq!(predict(&cpu, &g).unwrap().cpu.edx, 1);
    g.set_u8(UNIT + 0x212, 0).unwrap();
    g.set_u32(DEF + 0x12c, 0x20).unwrap();
    let p = predict(&cpu, &g).unwrap();
    assert_eq!(
        (p.record, p.cpu.edx),
        (None, 1),
        "no zoom asked of a weapon that needs it"
    );
}

#[test]
fn declines_are_counted_by_reason() {
    let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
    reset();
    let (cpu, mut g) = call();
    let mut env = cpu;
    env.mxcsr = 0x1f80;
    assert_eq!(enter(&mut env, &mut g), Step::Guest);
    g.set_u32(DEF + 0x2c8, MAX_TRIGGERS as u32 + 1).unwrap();
    assert_eq!(enter(&mut { cpu }, &mut g), Step::Guest);
    g.set_u32(DEF + 0x2c8, 1).unwrap();
    g.set_u32(DATA + 2 * 12 + 8, 0x0100_0000).unwrap();
    assert_eq!(enter(&mut { cpu }, &mut g), Step::Guest);
    assert_eq!(status().declined, [1, 1, 1]);
    assert_eq!(status().answered, 0);
}

/// The kernel answers every call from the first, with nothing proven
/// first, and a reset clears the counters.
#[test]
fn the_kernel_answers_from_the_first_call() {
    let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
    reset();
    let (cpu, mut g) = call();
    for _ in 0..3 {
        g.set_u32(OUT + 0x14, 0).unwrap();
        let mut entering = cpu;
        assert_eq!(enter(&mut entering, &mut g), Step::Answered { target: RETURN });
        assert_eq!(entering.esp, STACK + 8);
        assert_eq!(g.u32(OUT + 0x14).unwrap(), 0x20, "the answer writes the record");
    }
    assert_eq!(status(), Status { answered: 3, declined: [0; 3] });
    reset();
    assert_eq!(status(), Status::default());
}

#[test]
fn a_different_record_word_is_a_mismatch() {
    let (cpu, mut g) = call();
    let p = predict(&cpu, &g).unwrap();
    let after = guest_answer(&p, &cpu, &mut g);
    let words = |ram: &Guest| std::array::from_fn(|i| ram.u32(OUT + 4 * i as u32).unwrap());
    assert_eq!(p.differs(&after, Some(words(&g))), None);
    g.set_u32(OUT + 0x20, 0).unwrap();
    assert_eq!(p.differs(&after, Some(words(&g))), Some("record"));
    g.set_u32(OUT + 0x20, 12.0f32.to_bits()).unwrap();
    let mut x = after;
    x.xmm[2][15] ^= 1;
    assert_eq!(p.differs(&x, Some(words(&g))), Some("xmm2"));
}
