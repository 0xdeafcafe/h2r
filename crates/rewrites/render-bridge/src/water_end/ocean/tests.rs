use super::*;
use super::fake::{Event, Fake};
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
const SCRATCH: u32 = 0x0050_9000;
const DEVICE_AT: u32 = 0x004f_8000;
const STREAM_AT: u32 = 0x0050_7100;
const VERTEX_TYPE: u32 = 5;
const VERTEX_HANDLE: u32 = 0x77aa;

/// A definition whose shader `1` reaches implementation flags `flags`
/// (template `2`, implementation `3`), one bitmap (`4`) of two images.
fn mem(flags: u32) -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0017_0000);
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
    set(IMPL + 0x100, VERTEX_TYPE);
    set(tag(VERTEX_TYPE) + 8, 0x0050_6d00);
    set(0x0050_6d00 + 8, 0x0050_6e00);
    set(0x0050_6e00 + 0x1c + 0x10, VERTEX_HANDLE);
    set(IMPL + 0x118, flags);
    set(tag(4) + 8, BITMAP);
    set(BITMAP + 0x44, 2);
    set(BITMAP + 0x48, IMAGES);
    set(DEF + SHADER, SHADER_TAG);
    set(DEF + GEOMETRY, GEO);
    set(DEF + PASS_WORD, 0x1234);
    set(GEO + GEOMETRY_STATE, STREAM_AT);
    set(STREAM_AT + 8, 0x0050_7200);
    set(state::DEVICE, DEVICE_AT);
    set(DEVICE_AT, 0x100);
    set(DEVICE_AT + 4, 0x1000);
    set(crate::native_targets::STAGE_REQUESTED, 0x1234);
    set(state::ONE, 1f32.to_bits());
    // The structure upload's inputs (engine::render_loop::structure_parameters).
    set(0x0046_87b0, 0x0050_7300);
    set(0x0050_7300 + 8, 1f32.to_bits());
    set(0x004b_9db0, 1f32.to_bits());
    set(0x0045_dbdc, 1e-4f32.to_bits());
    set(0x0045_dbbc, 0.5f32.to_bits());
    set(0x0045_dbcc, 0.25f32.to_bits());
    set(0x0045_e458, 2f32.to_bits());
    set(0x0045_dc38, 255f32.to_bits());
    set(GEO + QUAD_COUNT, 8);
    set(GEO + QUAD_INDICES, INDICES);
    set(STATE_BLOCK, 0xdead);
    set(STATE_BLOCK + 4 * (STATE_BLOCK_WORDS - 1), 0xdead);
    m.write(0x0045_e450, &0.5f64.to_le_bytes()).unwrap();
    // The texture block loaded; both images current.
    m.write(DEF + TEXTURE_BLOCK + 0x1e, &[1]).unwrap();
    m.set_u32(cache::RESOURCES, CACHE_AT).unwrap();
    m.set_u32(CACHE_AT + 0x64, CACHE_AT + 0x100).unwrap();
    m.set_u32(CACHE_AT + 0x100 + 0x44, CACHE_AT + 0x200).unwrap();
    m.set_u32(CACHE_AT + 0x38, 7).unwrap();
    for image in [IMAGES, IMAGES + 0x74] {
        m.set_u32(image + 0x70, 1).unwrap();
        m.set_u32(image + 0x50, 0x1111).unwrap();
    }
    m
}

const CACHE_AT: u32 = 0x0050_a000;

struct Recorder {
    host: Fake,
    refuse_draw: bool,
    visible: bool,
}

impl Recorder {
    fn new() -> Self {
        Self { host: Fake::default(), refuse_draw: false, visible: true }
    }
}

impl Device for Recorder {
    fn host(&self) -> &dyn Host {
        &self.host
    }
    fn scratch(&self) -> u32 {
        SCRATCH
    }
    fn draw(&mut self, _mem: &mut dyn GuestMemory, indices: u32, count: u32) -> Result<(), Cause> {
        if self.refuse_draw {
            return Err("draw");
        }
        self.host.push(Event::Draw { indices, count });
        Ok(())
    }
    fn visible(&mut self, _mem: &mut dyn GuestMemory, definition: u32) -> Result<bool, Cause> {
        assert_eq!(definition, DEF);
        self.host.push(Event::Visible);
        Ok(self.visible)
    }
}

/// An event's kind, for the order.
fn kind(e: &Event) -> String {
    match e {
        Event::Guest(t) => format!("guest {t:#x}"),
        other => format!("{other:?}").split([' ', '{', '(']).next().unwrap().to_owned(),
    }
}

/// Steps the ocean to its return, each helper leaving AL from `al` (by
/// target, 1 otherwise), the gate `visible`. The calls and the return.
fn run_seen(m: &mut FlatMemory, clear: bool, visible: bool, al: &dyn Fn(u32) -> u32) -> (Vec<Call>, Vec<Event>, Next) {
    let mut ocean = Ocean::new(DEF, clear, Regs { ebx: 0xb, esi: 0x5, edi: 0xd, ..Regs::default() });
    let mut device = Recorder { visible, ..Recorder::new() };
    let mut calls = Vec::new();
    let mut returned = Regs::default();
    loop {
        match ocean.next(m, &mut device, returned).unwrap() {
            Next::Call(c) => {
                device.host.push(Event::Guest(c.target));
                returned = Regs { eax: al(c.target), ..c.regs };
                calls.push(c);
            }
            end @ Next::Return { .. } => return (calls, device.host.take(), end),
        }
    }
}

fn run(m: &mut FlatMemory, clear: bool, al: &dyn Fn(u32) -> u32) -> (Vec<Call>, Vec<Event>, Next) {
    run_seen(m, clear, true, al)
}

/// With both copy bits and the clear byte, the ocean makes the body's
/// steps in its order: the grid, the gates, the copies, the clear, the
/// draw state and stream commit, the quads, then the stamps.
#[test]
fn the_ocean_makes_the_bodys_steps_in_order() {
    let mut m = mem(1 << COPY_BIT | 1 << PASS_BIT);
    m.write(state::DECOMPRESSION_PENDING, &[1]).unwrap();
    let (calls, events, end) = run(&mut m, true, &|t| if t == PASS_SOURCE { 0x0099_0000 } else { 1 });
    let mut order: Vec<String> = events.iter().map(kind).collect();
    order.dedup();
    let g = |t: u32| format!("guest {t:#x}");
    let want: Vec<String> = [
        g(GRID), "Visible".into(), g(COPY_TARGET), g(PASS_SOURCE), g(PASS_TARGET), "Clear".into(),
        "VertexProgram".into(), "Constants".into(), "VertexInput".into(), "SelectProgram".into(), "Constants".into(),
        g(SHADER_BEGIN), "PushViewport".into(), "StageState".into(), "Registers".into(), "RenderState".into(),
        "Registers".into(), "Texture".into(), "Draw".into(),
    ].into();
    assert_eq!(order, want);
    assert!(events.contains(&Event::Clear { flags: 0x80, colour: 0x00ff_ffff }));
    assert!(events.contains(&Event::VertexProgram { function: VERTEX_HANDLE, address: 0 }));
    assert!(events.contains(&Event::Texture { stage: 0, texture: 0x1234 }));
    assert!(events.contains(&Event::Draw { indices: INDICES, count: 8 }));
    assert_eq!(events.iter().filter(|e| matches!(e, Event::StageState { .. })).count(), 12);
    let by = |t: u32| calls.iter().find(|c| c.target == t).unwrap();
    assert_eq!(by(GRID).regs.eax, DEF);
    assert_eq!(by(PASS_SOURCE).args, vec![DEF + PASS_BLOCK]);
    // 0x0003bd00(definition + 0x84's word, 0x00131ed0's result).
    assert_eq!(by(PASS_TARGET).args, vec![0x1234, 0x0099_0000]);
    assert_eq!(by(SHADER_BEGIN).regs.esi, GEO);
    assert_eq!(by(SHADER_BEGIN).regs.edi, SHADER_TAG);
    assert_eq!(by(SHADER_BEGIN).args, vec![0, 3, 0, 0, 0x47c3_5000]);
    // The body's EBP is the definition.
    assert!(calls.iter().all(|c| c.regs.ebp == DEF));
    // The texture block's cache entry stamped with the cache's frame.
    assert_eq!(m.u32(CACHE_AT + 0x200 + 0x14).unwrap(), 7);
    // The stream committed: its one input through the static shader.
    assert!(events.contains(&Event::VertexInput { handle: state::STATIC_SHADER | 1, count: 1, inputs: state::STREAMS + state::RECORD_INPUTS }));
    // The block 0x0001bd50 reads is zeroed and cached; the commit's count cleared.
    assert_eq!((m.u32(STATE_BLOCK).unwrap(), m.u32(STATE_BLOCK + 4 * (STATE_BLOCK_WORDS - 1)).unwrap()), (0, 0));
    assert_eq!(m.u32(state::STATE_CACHED).unwrap(), STATE_BLOCK);
    assert_eq!(m.u32(state::STREAMS + 0x2d0).unwrap(), 0);
    // The viewport push's put written back.
    assert_eq!(m.u32(DEVICE_AT).unwrap(), 0x100 + super::fake::PUSH);
    // AL 1 over the bitmap count the loop ends on, ECX the count.
    assert_eq!(end, Next::Return { eax: 1, ecx: 1 });
}

/// The implementation's bits pick the copies; no clear byte, no clear.
#[test]
fn the_shader_flags_pick_the_copies() {
    let head = |flags: u32, clear: bool| {
        let (_, events, _) = run(&mut mem(flags), clear, &|_| 1);
        events.into_iter().take_while(|e| !matches!(e, Event::VertexProgram { .. })).collect::<Vec<_>>()
    };
    use Event::*;
    let gates = [Guest(GRID), Visible];
    assert_eq!(head(0, false), gates);
    assert_eq!(head(1 << COPY_BIT, false), [&gates[..], &[Guest(COPY_TARGET)]].concat());
    assert_eq!(
        head(1 << PASS_BIT, true),
        [&gates[..], &[Guest(PASS_SOURCE), Guest(PASS_TARGET), Clear { flags: 0x80, colour: 0x00ff_ffff }]].concat()
    );
}

/// A closed gate goes to the stamps: the occlusion query's, or a texture
/// block not resident; a definition with no shader returns after the
/// gates with EAX -1 under AL 1.
#[test]
fn a_closed_gate_goes_to_the_stamps() {
    use Event::*;
    let (_, events, end) = run_seen(&mut mem(0), true, false, &|_| 1);
    assert_eq!(events, vec![Guest(GRID), Visible]);
    assert_eq!(end, Next::Return { eax: 1, ecx: 1 });
    let mut m = mem(0);
    m.write(DEF + TEXTURE_BLOCK + 0x1e, &[0]).unwrap();
    let (_, events, end) = run(&mut m, true, &|_| 1);
    assert_eq!(events, vec![Guest(GRID), Visible]);
    assert_eq!(end, Next::Return { eax: 1, ecx: 1 });
    // The stamps run past a closed gate: an image the cache must load is
    // named.
    let mut m = mem(0);
    m.set_u32(IMAGES + 0x74 + 0x70, 0).unwrap();
    m.set_u32(IMAGES + 0x74 + 0x50, 0).unwrap();
    m.write(IMAGES + 0x74 + 0xf, &[2]).unwrap();
    let mut ocean = Ocean::new(DEF, true, Regs::default());
    let mut device = Recorder { visible: false, ..Recorder::new() };
    ocean.next(&mut m, &mut device, Regs::default()).unwrap();
    assert_eq!(ocean.next(&mut m, &mut device, Regs::default()), Err("texture_request"));
    let mut m = mem(0);
    m.set_u32(DEF + SHADER, u32::MAX).unwrap();
    let (_, events, end) = run(&mut m, true, &|_| 1);
    assert_eq!(events, vec![Guest(GRID), Visible]);
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
    assert_eq!(stamps(&m, SHADER_TAG).unwrap().0, vec![0x74]);
}

/// The steps the stage cannot make are named.
#[test]
fn unmade_steps_are_named() {
    let mut m = mem(0);
    m.set_u32(TABLE + 16, GROUP_MTRP).unwrap();
    assert_eq!(shader(&m, SHADER_TAG), Err("shader_group"));
    let cause = |m: &mut FlatMemory, refuse_draw: bool| {
        let mut ocean = Ocean::new(DEF, false, Regs::default());
        let mut device = Recorder { refuse_draw, ..Recorder::new() };
        loop {
            match ocean.next(m, &mut device, Regs { eax: 1, ..Regs::default() }) {
                Ok(Next::Call(_)) => continue,
                Ok(end) => panic!("{end:?}"),
                Err(cause) => break cause,
            }
        }
    };
    assert_eq!(cause(&mut mem(0), true), "draw");
    // A stream record that does not read.
    let mut m = mem(0);
    m.set_u32(GEO + GEOMETRY_STATE, 0x0010_0000).unwrap();
    assert_eq!(cause(&mut m, false), "unread");
    let mut m = mem(0);
    m.set_u32(TAGS, 0x0010_0000).unwrap();
    assert_eq!(shader(&m, SHADER_TAG), Err("unread"));
}
