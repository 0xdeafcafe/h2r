use super::*;
use engine::memory::FlatMemory;

const TABLE: u32 = 0x004f_0000;
const DEF: u32 = 0x0051_0000;
const GEO: u32 = 0x0050_7000;
const INDICES: u32 = 0x0050_8000;
const SHADER_TAG: u32 = 1;
const BLOCK: u32 = 0x0050_6100;
const BITMAP: u32 = 0x0050_6b00;
const IMAGES: u32 = 0x0050_6c00;
const IMPL: u32 = 0x0050_6a00;

/// A definition whose shader `1` reaches implementation flags `flags`
/// (template `2`, implementation `3`), one bitmap (`4`) of two images.
fn mem(flags: u32) -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
    let mut set = |at: u32, v: u32| m.set_u32(at, v).unwrap();
    set(TAGS, TABLE);
    let tag = |n: u32| TABLE + 16 * n;
    set(tag(1), u32::from_be_bytes(*b"shad"));
    set(tag(1) + 8, 0x0050_6000);
    set(0x0050_6000 + 0x24, BLOCK);
    set(BLOCK, 2);
    set(BLOCK + 4, 1);
    set(BLOCK + 8, 0x0050_6200);
    set(0x0050_6200, 4);
    set(tag(2) + 8, 0x0050_6300);
    set(0x0050_6300 + 0x5c, 0x0050_6400);
    set(0x0050_6400 + 4, 0x0050_6500);
    set(0x0050_6400 + 0xc, 0x0050_6600);
    set(0x0050_6400 + 0x14, 0x0050_6700);
    set(0x0050_6500, 0xfe03); // option select 3 (& 0x1ff)
    set(0x0050_6600 + 3 * 2 + 6, 5);
    set(0x0050_6700 + 5 * 10 + 4, 0xabcd_0003);
    set(tag(3) + 8, 0x0050_6800);
    set(0x0050_6800 + 0x20, 0x0050_6900);
    set(0x0050_6900 + 4, IMPL);
    set(IMPL + 0x100, 0xabc);
    set(IMPL + 0x118, flags);
    set(tag(4) + 8, BITMAP);
    set(BITMAP + 0x44, 2);
    set(BITMAP + 0x48, IMAGES);
    set(DEF + SHADER, SHADER_TAG);
    set(DEF + GEOMETRY, GEO);
    set(DEF + PASS_WORD, 0x1234);
    set(GEO + GEOMETRY_STATE, 0x77);
    set(GEO + QUAD_COUNT, 8);
    set(GEO + QUAD_INDICES, INDICES);
    set(STATE_BLOCK, 0xdead);
    set(STATE_BLOCK + 4 * (STATE_BLOCK_WORDS - 1), 0xdead);
    m
}

/// What the stage did, in order: a guest call's target, or the device's.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Guest(u32),
    Clear,
    Draw { indices: u32, count: u32 },
}

#[derive(Default)]
struct Recorder {
    events: Vec<Event>,
    refuse_draw: bool,
}

impl Device for Recorder {
    fn clear(&mut self, _mem: &mut dyn GuestMemory) -> Result<(), Cause> {
        self.events.push(Event::Clear);
        Ok(())
    }
    fn draw(&mut self, _mem: &mut dyn GuestMemory, indices: u32, count: u32) -> Result<(), Cause> {
        if self.refuse_draw {
            return Err("draw");
        }
        self.events.push(Event::Draw { indices, count });
        Ok(())
    }
}

/// Steps the ocean to its return, each helper leaving AL from `al` (by
/// target, 1 otherwise). The calls and the return.
fn run(m: &mut FlatMemory, clear: bool, al: &dyn Fn(u32) -> u32) -> (Vec<Call>, Recorder, Next) {
    let mut ocean = Ocean::new(DEF, clear, Regs { ebx: 0xb, esi: 0x5, edi: 0xd, ..Regs::default() });
    let mut device = Recorder::default();
    let mut calls = Vec::new();
    let mut returned = Regs::default();
    loop {
        match ocean.next(m, &mut device, returned).unwrap() {
            Next::Call(c) => {
                device.events.push(Event::Guest(c.target));
                returned = Regs { eax: al(c.target), ..c.regs };
                calls.push(c);
            }
            end @ Next::Return { .. } => return (calls, device, end),
        }
    }
}

/// With both copy bits and the clear byte, the ocean makes the body's
/// steps in its order: the grid, the gates, the copies, the clear, the
/// state, the quads, then a stamp per image.
#[test]
fn the_ocean_makes_the_bodys_steps_in_order() {
    let mut m = mem(1 << COPY_BIT | 1 << PASS_BIT);
    let (calls, device, end) = run(&mut m, true, &|t| if t == PASS_SOURCE { 0x0099_0000 } else { 1 });
    use Event::*;
    assert_eq!(device.events, vec![
        Guest(GRID), Guest(VISIBLE), Guest(TEXTURE), Guest(COPY_TARGET), Guest(PASS_SOURCE), Guest(PASS_TARGET), Clear,
        Guest(STATE_A), Guest(STATE_B), Guest(STATE_C), Guest(STATE_D), Guest(STATE_E), Guest(STATE_F),
        Guest(SHADER_BEGIN), Guest(SHADER_PASS), Guest(CONSTANTS), Guest(COMMIT),
        Draw { indices: INDICES, count: 8 }, Guest(STAMP), Guest(STAMP),
    ]);
    let by = |t: u32| calls.iter().find(|c| c.target == t).unwrap();
    assert_eq!(by(GRID).regs.eax, DEF);
    assert_eq!(by(VISIBLE).args, vec![DEF]);
    assert_eq!((by(TEXTURE).regs.eax, by(TEXTURE).regs.ecx), (3, DEF + TEXTURE_BLOCK));
    assert_eq!(by(PASS_SOURCE).args, vec![DEF + PASS_BLOCK]);
    // 0x0003bd00(definition + 0x84's word, 0x00131ed0's result).
    assert_eq!(by(PASS_TARGET).args, vec![0x1234, 0x0099_0000]);
    assert_eq!(by(STATE_A).args, vec![1]);
    assert_eq!(by(STATE_A).regs.esi, GEO);
    assert_eq!(by(STATE_C).regs.ecx, 0x77);
    assert_eq!(by(STATE_D).args, vec![0x0051_f0f0]);
    assert_eq!(by(SHADER_BEGIN).regs.edi, SHADER_TAG);
    assert_eq!(by(SHADER_BEGIN).args, vec![0, 3, 0, 0, 0x47c3_5000]);
    assert_eq!(by(CONSTANTS).regs.eax, DEF);
    // The body's EBP is the definition up to the stamps.
    assert!(calls.iter().filter(|c| c.target != STAMP).all(|c| c.regs.ebp == DEF));
    let stamps: Vec<_> = calls.iter().filter(|c| c.target == STAMP).map(|c| (c.regs.eax, c.regs.ebp)).collect();
    assert_eq!(stamps, vec![(IMAGES, BLOCK), (IMAGES + 0x74, BLOCK)]);
    // The block 0x0001bd50 reads is zeroed before it.
    assert_eq!((m.u32(STATE_BLOCK).unwrap(), m.u32(STATE_BLOCK + 4 * (STATE_BLOCK_WORDS - 1)).unwrap()), (0, 0));
    // AL 1 over the bitmap count the loop ends on, ECX the count.
    assert_eq!(end, Next::Return { eax: 1, ecx: 1 });
}

/// The implementation's bits pick the copies; no clear byte, no clear.
#[test]
fn the_shader_flags_pick_the_copies() {
    let targets = |flags: u32, clear: bool| {
        let (_, device, _) = run(&mut mem(flags), clear, &|_| 1);
        device.events.into_iter().take_while(|e| *e != Event::Guest(STATE_A)).collect::<Vec<_>>()
    };
    use Event::*;
    let gates = [Guest(GRID), Guest(VISIBLE), Guest(TEXTURE)];
    assert_eq!(targets(0, false), gates);
    assert_eq!(targets(1 << COPY_BIT, false), [&gates[..], &[Guest(COPY_TARGET)]].concat());
    assert_eq!(targets(1 << PASS_BIT, true), [&gates[..], &[Guest(PASS_SOURCE), Guest(PASS_TARGET), Clear]].concat());
}

/// A gate's AL 0 skips to the stamps; a definition with no shader returns
/// after the gates with EAX -1 under AL 1.
#[test]
fn a_closed_gate_goes_to_the_stamps() {
    use Event::*;
    for gate in [VISIBLE, TEXTURE] {
        let (_, device, _) = run(&mut mem(0), true, &|t| u32::from(t != gate));
        let upto: Vec<_> = device.events.iter().take_while(|e| **e != Guest(STAMP)).collect();
        assert_eq!(upto.last(), Some(&&Guest(gate)), "{gate:#x}");
        assert_eq!(device.events.iter().filter(|e| **e == Guest(STAMP)).count(), 2);
    }
    let mut m = mem(0);
    m.set_u32(DEF + SHADER, u32::MAX).unwrap();
    let (_, device, end) = run(&mut m, true, &|_| 1);
    assert_eq!(device.events, vec![Guest(GRID), Guest(VISIBLE), Guest(TEXTURE)]);
    assert!(matches!(end, Next::Return { eax: 0xffff_ff01, .. }));
}

/// A bitmap index of -1 and a null image array are skipped as the guest
/// skips them.
#[test]
fn the_stamps_skip_what_the_guest_skips() {
    let mut m = mem(0);
    m.set_u32(BLOCK + 4, 2).unwrap();
    m.set_u32(0x0050_6200 + 0xc, u32::MAX).unwrap();
    let (s, count) = stamps(&m, SHADER_TAG).unwrap();
    assert_eq!((s.len(), count), (2, 2));
    m.set_u32(BITMAP + 0x48, 0).unwrap();
    // Image 0 of a null array is null; image 1 is not.
    assert_eq!(stamps(&m, SHADER_TAG).unwrap().0.iter().map(|s| s.image).collect::<Vec<_>>(), vec![0x74]);
}

/// The steps the stage cannot make are named.
#[test]
fn unmade_steps_are_named() {
    let mut m = mem(0);
    m.set_u32(TABLE + 16, GROUP_MTRP).unwrap();
    assert_eq!(shader(&m, SHADER_TAG), Err("shader_group"));
    let mut ocean = Ocean::new(DEF, false, Regs::default());
    let mut device = Recorder { refuse_draw: true, ..Recorder::default() };
    let mut m = mem(0);
    let cause = loop {
        match ocean.next(&mut m, &mut device, Regs { eax: 1, ..Regs::default() }) {
            Ok(Next::Call(_)) => continue,
            Ok(end) => panic!("{end:?}"),
            Err(cause) => break cause,
        }
    };
    assert_eq!(cause, "draw");
    m.set_u32(TAGS, 0x0010_0000).unwrap();
    assert_eq!(shader(&m, SHADER_TAG), Err("unread"));
}
