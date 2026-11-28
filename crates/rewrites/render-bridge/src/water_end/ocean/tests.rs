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
const STREAM_AT: u32 = 0x0050_7100;
const BUFFER: u32 = 0x0050_7200;
const VERTEX_TYPE: u32 = 5;

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
    set(IMPL + 0x118, flags);
    set(tag(4) + 8, BITMAP);
    set(BITMAP + 0x44, 2);
    set(BITMAP + 0x48, IMAGES);
    set(DEF + SHADER, SHADER_TAG);
    set(DEF + GEOMETRY, GEO);
    set(BSP, BSP_AT);
    set(BSP_AT + WATER_COUNT, 1);
    set(GEO + GEOMETRY_STATE, STREAM_AT);
    set(STREAM_AT, 0x0300_0002);
    set(STREAM_AT + 8, BUFFER);
    set(STREAM_AT + 0x10, 0x40);
    set(BUFFER + 4, 0x0012_3000);
    set(0x0045_dbc0, 1f32.to_bits());
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
    for n in 0..12 {
        set(ROWS + 4 * n, 0x3f80_0000 + n);
    }
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
/// Where the grid leaves its rows (`scene_extract::late::OCEAN_ROWS`).
const ROWS: u32 = scene_extract::late::OCEAN_ROWS;

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
    fn draw(&mut self, _mem: &mut dyn GuestMemory, draw: &Draw) -> Result<(), Cause> {
        if self.refuse_draw {
            return Err("draw");
        }
        self.host.push(Event::Draw(Box::new(draw.clone())));
        Ok(())
    }
    fn visible(&mut self, _mem: &mut dyn GuestMemory, definition: u32) -> Result<bool, Cause> {
        assert_eq!(definition, DEF);
        self.host.push(Event::Visible);
        Ok(self.visible)
    }
    fn sky(&mut self, _mem: &mut dyn GuestMemory) -> Result<(), Cause> {
        // The constants the device holds: the last upload's.
        let constants = self.host.events.lock().unwrap().iter().rev().find_map(|e| match e {
            Event::Constants { register: 0, words } => Some(words.clone()),
            _ => None,
        });
        self.host.push(Event::Sky { constants: constants.unwrap_or_default() });
        Ok(())
    }
    fn fp(&mut self) -> (&mut u32, u16) {
        (&mut self.mxcsr, 0x027f)
    }
}

/// An event's kind, for the order.
fn kind(e: &Event) -> String {
    format!("{e:?}").split([' ', '{', '(']).next().unwrap().to_owned()
}

/// Runs the ocean to its return: the device's events and what it returns.
fn run(m: &mut FlatMemory, clear: bool) -> (Vec<Event>, Ended) {
    run_with(m, clear, Recorder::new())
}

/// [`run`] with the occlusion gate's answer `visible`.
fn run_seen(m: &mut FlatMemory, clear: bool, visible: bool) -> (Vec<Event>, Ended) {
    run_with(m, clear, Recorder { visible, ..Recorder::new() })
}

fn run_with(m: &mut FlatMemory, clear: bool, mut device: Recorder) -> (Vec<Event>, Ended) {
    let end = super::run(m, &mut device, DEF, clear, 0xecc).unwrap();
    (device.host.take(), end)
}

/// The cause of the first step the ocean cannot make.
fn cause(m: &mut FlatMemory, device: &mut Recorder) -> Cause {
    super::run(m, device, DEF, false, 0).unwrap_err()
}

/// The draw the device was handed.
fn drawn(events: &[Event]) -> &Draw {
    events.iter().find_map(|e| if let Event::Draw(d) = e { Some(&**d) } else { None }).expect("a draw")
}

/// With the copy bit and the clear byte, the ocean makes the body's
/// steps in its order: the gate, the copy, the clear, the draw, then the
/// stamps; no draw state, shader begin or quads through the device.
#[test]
fn the_ocean_makes_no_shader_begin() {
    let mut m = mem(1 << COPY_BIT);
    let (events, end) = run(&mut m, true);
    let order: Vec<String> = events.iter().map(kind).collect();
    assert_eq!(order, ["Visible", "CopyView", "Clear", "Draw"]);
    assert!(events.contains(&Event::Clear { flags: 0x80, colour: 0x00ff_ffff }));
    // The copy into target 0x16 and the frame's flags (0x0003bea0).
    assert!(events.contains(&Event::CopyView { target: COPY_INTO }));
    assert_eq!((m.u8(TARGET_COPIED).unwrap(), m.u8(engine::render_loop::layer::DRAW_FLAG).unwrap()), (1, 1));
    // The texture block's cache entry stamped with the cache's frame.
    assert_eq!(m.u32(CACHE_AT + 0x200 + 0x14).unwrap(), 7);
    // AL 1 over the bitmap count the loop ends on, ECX the count.
    assert_eq!(end, Ended { eax: 1, ecx: 1 });
}

/// The draw carries the frame's record: the grid's rows, the definition's
/// parameter block upload, the flags and the clear byte; and the quads and
/// the grid's stream.
#[test]
fn the_ocean_draws_from_its_record() {
    let mut m = mem(1 << COPY_BIT);
    let (events, _) = run(&mut m, true);
    let d = drawn(&events);
    assert_eq!((d.ocean.definition, d.ocean.shader), (DEF, SHADER_TAG));
    assert_eq!(d.ocean.rows[0], [0x3f80_0000, 0x3f80_0001, 0x3f80_0002, 0x3f80_0003]);
    assert_eq!(d.ocean.rows[2][3], 0x3f80_000b);
    assert_eq!(Some(d.ocean.structure), scene_extract::layout::structure_parameters::of(&TestRead(&m), DEF));
    assert!(d.ocean.clear_alpha && d.ocean.copies && !d.ocean.reflects);
    assert_eq!((d.indices, d.count), (INDICES, 8));
    assert_eq!(d.stream, Stream { kind: 2, buffer: BUFFER, offset: 0x40 });
    assert_eq!(d.stream.vertices(&m), Ok(0x8012_3040));
    let (events, _) = run(&mut mem(0), false);
    assert!(!drawn(&events).ocean.clear_alpha && !drawn(&events).ocean.copies);
}

/// Guest memory as extraction reads it.
struct TestRead<'a>(&'a FlatMemory);

impl scene_extract::GuestRead for TestRead<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
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
    let (events, end) = run(&mut m, false);
    assert_eq!(events[0], Event::Visible);
    assert_eq!(end, Ended { eax: 1, ecx: 1 });
    // The record takes the rows the grid wrote.
    assert_eq!(drawn(&events).ocean.rows[0][1], m.u32(ocean_grid::ROWS + 4).unwrap());
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
    run_with(&mut m, false, Recorder { mxcsr: 0x1f80 & !0x80, ..Recorder::new() });
}

/// The implementation's copy bit picks the copy, once a frame; the clear
/// byte the clear.
#[test]
fn the_shader_flags_pick_the_copy() {
    let head = |m: &mut FlatMemory, clear: bool| {
        let (events, _) = run(m, clear);
        events.into_iter().take_while(|e| !matches!(e, Event::Draw(_))).collect::<Vec<_>>()
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


/// A copy the device refuses, a copy with no water in the BSP's way, and
/// a reflection target that is not live are named; a BSP that does not
/// read too.
#[test]
fn the_copies_name_what_they_cannot_make() {
    let refuse = Recorder::new();
    refuse.host.refuse_copy.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(cause(&mut mem(1 << COPY_BIT), &mut { refuse }), "copy_target");
    // Target 0x15 not live: the reflection is named.
    assert_eq!(cause(&mut mem(1 << PASS_BIT), &mut Recorder::new()), "copy_reflection");
    let mut m = mem(1 << COPY_BIT);
    m.set_u32(BSP, 0x0010_0000).unwrap();
    assert_eq!(cause(&mut m, &mut Recorder::new()), "unread");
    // With no water the copy is skipped and the flags still set.
    let mut m = mem(1 << COPY_BIT);
    m.set_u32(BSP_AT + WATER_COUNT, 0).unwrap();
    let (events, _) = run(&mut m, false);
    assert!(!events.iter().any(|e| matches!(e, Event::CopyView { .. })));
    assert_eq!(m.u8(TARGET_COPIED).unwrap(), 1);
}

/// A closed gate goes to the stamps: the occlusion query's, or a texture
/// block not resident (empty, or still loading: the ocean waits a
/// frame); a definition with no shader returns after the gates with EAX
/// -1 under AL 1.
#[test]
fn a_closed_gate_goes_to_the_stamps() {
    use Event::*;
    let (events, end) = run_seen(&mut mem(0), true, false);
    assert_eq!(events, vec![Visible]);
    assert_eq!(end, Ended { eax: 1, ecx: 1 });
    let mut m = mem(0);
    m.write(DEF + TEXTURE_BLOCK + 0x1e, &[0]).unwrap();
    let (events, end) = run(&mut m, true);
    assert_eq!(events, vec![Visible]);
    assert_eq!(end, Ended { eax: 1, ecx: 1 });
    // A block with data still loading: not drawn this frame.
    m.set_u32(DEF + TEXTURE_BLOCK + 8, 0x100).unwrap();
    m.set_u32(DEF + TEXTURE_BLOCK + 0xc, 0).unwrap();
    let (events, end) = run(&mut m, true);
    assert_eq!(events, vec![Visible]);
    assert_eq!(end, Ended { eax: 1, ecx: 1 });
    // The stamps run past a closed gate: an image the cache does not hold
    // yet is left for a later frame.
    let mut m = mem(0);
    m.set_u32(IMAGES + 0x74 + 0x70, 0).unwrap();
    m.set_u32(IMAGES + 0x74 + 0x50, 0).unwrap();
    m.write(IMAGES + 0x74 + 0xf, &[2]).unwrap();
    let mut device = Recorder { visible: false, ..Recorder::new() };
    assert_eq!(super::run(&mut m, &mut device, DEF, true, 0), Ok(Ended { eax: 1, ecx: 1 }));
    // No shader: EAX -1 under AL 1, ECX as the grid left it (skipped: the
    // entry's).
    let mut m = mem(0);
    m.set_u32(DEF + SHADER, u32::MAX).unwrap();
    let (events, end) = run(&mut m, true);
    assert_eq!(events, vec![Visible]);
    assert_eq!(end, Ended { eax: 0xffff_ff01, ecx: 0xecc });
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

/// An `MTRP` or `3trp` shader's template comes through its group's
/// object, which keeps the index and its data: 3trp's at its data's
/// `+0x90`, MTRP's at the `+0x24` of the shader its data's `+0x1c` names.
#[test]
fn a_virtual_templates_group_reaches_the_same_implementation() {
    let shad = shader(&mut mem(1 << COPY_BIT), SHADER_TAG).unwrap();
    for (group, object) in [(GROUP_3TRP, OBJECT_3TRP), (GROUP_MTRP, OBJECT_MTRP)] {
        let mut m = mem(1 << COPY_BIT);
        m.set_u32(TABLE + 16, group).unwrap();
        m.set_u32(0x0050_6000 + 0x24, 0).unwrap();
        if group == GROUP_3TRP {
            m.set_u32(0x0050_6000 + 0x90, BLOCK).unwrap();
        } else {
            m.set_u32(0x0050_6000 + 0x1c, 0xabcd_0005).unwrap();
            m.set_u32(TABLE + 16 * 5 + 8, 0x0050_6d00).unwrap();
            m.set_u32(0x0050_6d00 + 0x24, BLOCK).unwrap();
        }
        assert_eq!(shader(&mut m, SHADER_TAG), Ok(shad), "{group:#x}");
        assert_eq!((m.u32(object + 4).unwrap(), m.u32(object + 8).unwrap()), (SHADER_TAG, 0x0050_6000));
    }
}

/// The steps the stage cannot make are named.
#[test]
fn unmade_steps_are_named() {
    assert_eq!(cause(&mut mem(0), &mut Recorder { refuse_draw: true, ..Recorder::new() }), "draw");
    // A stream record that does not read.
    let mut m = mem(0);
    m.set_u32(GEO + GEOMETRY_STATE, 0x0010_0000).unwrap();
    assert_eq!(cause(&mut m, &mut Recorder::new()), "unread");
    let mut m = mem(0);
    m.set_u32(TAGS, 0x0010_0000).unwrap();
    assert_eq!(shader(&mut m, SHADER_TAG), Err("unread"));
}

mod reflection;
