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
const BSP_AT: u32 = 0x0050_5000;
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
    set(BSP, BSP_AT);
    set(BSP_AT + WATER_COUNT, 1);
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
    mxcsr: u32,
}

impl Recorder {
    fn new() -> Self {
        Self { host: Fake::default(), refuse_draw: false, visible: true, mxcsr: 0x1f80 }
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
    fn fp(&mut self) -> (&mut u32, u16) {
        (&mut self.mxcsr, 0x027f)
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
/// target, 1 otherwise). The calls and the return.
fn run(m: &mut FlatMemory, clear: bool, al: &dyn Fn(u32) -> u32) -> (Vec<Call>, Vec<Event>, Next) {
    run_with(m, clear, al, Recorder::new())
}

/// [`run`] with the occlusion gate's answer `visible`.
fn run_seen(m: &mut FlatMemory, clear: bool, visible: bool, al: &dyn Fn(u32) -> u32) -> (Vec<Call>, Vec<Event>, Next) {
    run_with(m, clear, al, Recorder { visible, ..Recorder::new() })
}

fn run_with(m: &mut FlatMemory, clear: bool, al: &dyn Fn(u32) -> u32, mut device: Recorder) -> (Vec<Call>, Vec<Event>, Next) {
    let mut ocean = Ocean::new(DEF, clear, Regs { ebx: 0xb, esi: 0x5, edi: 0xd, ..Regs::default() });
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

/// With the copy bit and the clear byte, the ocean makes the body's
/// steps in its order: the grid, the gates, the copy, the clear, the
/// draw state and stream commit, the quads, then the stamps.
#[test]
fn the_ocean_makes_the_bodys_steps_in_order() {
    let mut m = mem(1 << COPY_BIT);
    m.write(state::DECOMPRESSION_PENDING, &[1]).unwrap();
    let (calls, events, end) = run(&mut m, true, &|_| 1);
    let mut order: Vec<String> = events.iter().map(kind).collect();
    order.dedup();
    let g = |t: u32| format!("guest {t:#x}");
    let want: Vec<String> = [
        "Visible".into(), "CopyView".into(), "Clear".into(),
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
    // The copy into target 0x16 and the frame's flags (0x0003bea0).
    assert!(events.contains(&Event::CopyView { target: COPY_INTO }));
    assert_eq!((m.u8(TARGET_COPIED).unwrap(), m.u8(engine::render_loop::layer::DRAW_FLAG).unwrap()), (1, 1));
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

/// A definition with an angle: a camera 3 above a 640x480 raster (its
/// top 7) and a grid of angle 1.
fn grid_mem(raster: [u16; 4]) -> FlatMemory {
    let mut m = mem(0);
    for (at, v) in [
        (ocean_grid::constant::ONE, 1.0f32),
        (ocean_grid::constant::CAP, 0.78),
        (ocean_grid::constant::MARGIN, 1.13),
        (ocean_grid::FOV, 1.2),
        (ocean_grid::CAMERA + 8, 3.0),
        (DEF + ocean_grid::ANGLE, 1.0),
        (DEF + ocean_grid::WIDTH, 1.0),
    ] {
        m.set_f32(at, v).unwrap();
    }
    for (o, v) in raster.iter().enumerate() {
        m.set_u16(ocean_grid::RASTER + 2 * o as u32, *v).unwrap();
    }
    m
}

/// The grid is built in Rust before the gates: the camera's basis and the
/// model rows the draw uploads.
#[test]
fn the_grid_is_built_before_the_gates() {
    let mut m = grid_mem([7, 0, 487, 640]);
    let (_, events, _) = run(&mut m, false, &|_| 1);
    assert_eq!(events[0], Event::Visible);
    assert_eq!(m.f32(ocean_grid::MATRIX).unwrap(), 3.0);
    // Forward along y and up along z: left along -x, the sideways span.
    let side = m.f32(ocean_grid::ROWS + 4).unwrap();
    assert!(side < -3.0 && side.is_finite(), "{side}");
}

/// An unmasked exception in the grid is a named gap.
#[test]
#[should_panic(expected = "water ocean grid: simd")]
fn an_unmasked_grid_exception_is_unimplemented() {
    // A zero raster: 0 / 0, with the invalid operation unmasked.
    let mut m = grid_mem([0; 4]);
    run_with(&mut m, false, &|_| 1, Recorder { mxcsr: 0x1f80 & !0x80, ..Recorder::new() });
}

/// The implementation's copy bit picks the copy, once a frame; the clear
/// byte the clear.
#[test]
fn the_shader_flags_pick_the_copy() {
    let head = |m: &mut FlatMemory, clear: bool| {
        let (_, events, _) = run(m, clear, &|_| 1);
        events.into_iter().take_while(|e| !matches!(e, Event::VertexProgram { .. })).collect::<Vec<_>>()
    };
    use Event::*;
    let gates = [Visible];
    assert_eq!(head(&mut mem(0), false), gates);
    let mut m = mem(1 << COPY_BIT);
    assert_eq!(head(&mut m, false), [&gates[..], &[CopyView { target: COPY_INTO }]].concat());
    assert_eq!(m.u8(TARGET_COPIED).unwrap(), 1);
    // The frame's flag is set: the next call makes no copy.
    let mut copied = mem(1 << COPY_BIT);
    copied.set_u8(TARGET_COPIED, 1).unwrap();
    assert_eq!(head(&mut copied, true), [&gates[..], &[Clear { flags: 0x80, colour: 0x00ff_ffff }]].concat());
}

/// The cause of the first step the ocean cannot make.
fn cause(m: &mut FlatMemory, device: &mut Recorder) -> Cause {
    let mut ocean = Ocean::new(DEF, false, Regs::default());
    loop {
        match ocean.next(m, device, Regs { eax: 1, ..Regs::default() }) {
            Ok(Next::Call(_)) => continue,
            Ok(end) => panic!("{end:?}"),
            Err(cause) => break cause,
        }
    }
}

/// A copy the device refuses, a copy with no water in the BSP's way, and
/// the sky pass into target 0x15 are named; a BSP that does not read too.
#[test]
fn the_copies_name_what_they_cannot_make() {
    let refuse = Recorder::new();
    refuse.host.refuse_copy.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(cause(&mut mem(1 << COPY_BIT), &mut { refuse }), "copy_target");
    assert_eq!(cause(&mut mem(1 << PASS_BIT), &mut Recorder::new()), "copy_pass");
    let mut m = mem(1 << COPY_BIT);
    m.set_u32(BSP, 0x0010_0000).unwrap();
    assert_eq!(cause(&mut m, &mut Recorder::new()), "unread");
    // With no water the copy is skipped and the flags still set.
    let mut m = mem(1 << COPY_BIT);
    m.set_u32(BSP_AT + WATER_COUNT, 0).unwrap();
    let (_, events, _) = run(&mut m, false, &|_| 1);
    assert!(!events.iter().any(|e| matches!(e, Event::CopyView { .. })));
    assert_eq!(m.u8(TARGET_COPIED).unwrap(), 1);
}

/// A closed gate goes to the stamps: the occlusion query's, or a texture
/// block not resident; a definition with no shader returns after the
/// gates with EAX -1 under AL 1.
#[test]
fn a_closed_gate_goes_to_the_stamps() {
    use Event::*;
    let (_, events, end) = run_seen(&mut mem(0), true, false, &|_| 1);
    assert_eq!(events, vec![Visible]);
    assert_eq!(end, Next::Return { eax: 1, ecx: 1 });
    let mut m = mem(0);
    m.write(DEF + TEXTURE_BLOCK + 0x1e, &[0]).unwrap();
    let (_, events, end) = run(&mut m, true, &|_| 1);
    assert_eq!(events, vec![Visible]);
    assert_eq!(end, Next::Return { eax: 1, ecx: 1 });
    // The stamps run past a closed gate: an image the cache must load is
    // named.
    let mut m = mem(0);
    m.set_u32(IMAGES + 0x74 + 0x70, 0).unwrap();
    m.set_u32(IMAGES + 0x74 + 0x50, 0).unwrap();
    m.write(IMAGES + 0x74 + 0xf, &[2]).unwrap();
    let mut ocean = Ocean::new(DEF, true, Regs::default());
    let mut device = Recorder { visible: false, ..Recorder::new() };
    assert_eq!(ocean.next(&mut m, &mut device, Regs::default()), Err("texture_request"));
    let mut m = mem(0);
    m.set_u32(DEF + SHADER, u32::MAX).unwrap();
    let (_, events, end) = run(&mut m, true, &|_| 1);
    assert_eq!(events, vec![Visible]);
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
