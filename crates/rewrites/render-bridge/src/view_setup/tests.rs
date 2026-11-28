use std::collections::BTreeMap;
use std::sync::Mutex;

use engine::memory::FlatMemory;

use super::*;
use crate::native_targets::Bind;

const VIEW: u32 = 0x0057_0000;
const DEVICE_AT: u32 = 0x0058_0000;
const PUSH: u32 = 0x0059_0000;
const STACK: u32 = 0x005f_0000;
const COLOUR: u32 = 0x005a_0000;
const DEPTH: u32 = 0x005a_1000;

/// What the device was asked, in order.
#[derive(Clone, Debug, PartialEq)]
enum Op {
    State(u32, u32),
    PushViewport(u32),
    Space(u32, u32),
    Constants(u32, Vec<u32>),
    Bind(Bind, u32, u32),
    Viewport([u32; 6]),
    Clear(u32, u32),
    Simple(u32, u32),
}

#[derive(Default)]
struct Device {
    ops: Mutex<Vec<Op>>,
    constants: Mutex<BTreeMap<usize, [u32; 4]>>,
    states: Mutex<BTreeMap<u32, u32>>,
    refuse_states: bool,
}

impl Device {
    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }
}

impl Host for Device {
    fn bind(&self, _: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool {
        self.ops.lock().unwrap().push(Op::Bind(bind, colour, depth));
        true
    }
    fn clear(&self, _: &mut dyn GuestMemory, flags: u32, colour: u32, _z: u32, _stencil: u32) -> bool {
        self.ops.lock().unwrap().push(Op::Clear(flags, colour));
        true
    }
    fn bound(&self) -> Option<[u32; 8]> {
        None
    }
    fn render_state(&self, _: &mut dyn GuestMemory, slot: u32, value: u32) -> bool {
        if self.refuse_states {
            return false;
        }
        self.ops.lock().unwrap().push(Op::State(slot, value));
        self.states.lock().unwrap().insert(slot, value);
        true
    }
    fn simple_state(&self, _: &mut dyn GuestMemory, header: u32, value: u32) -> bool {
        self.ops.lock().unwrap().push(Op::Simple(header, value));
        true
    }
    fn constants(&self, mem: &mut dyn GuestMemory, register: u32, data: u32, dwords: u32) -> bool {
        let words: Vec<u32> = (0..dwords).map(|i| mem.u32(data + 4 * i).unwrap()).collect();
        let mut c = self.constants.lock().unwrap();
        for (r, row) in words.chunks(4).enumerate() {
            c.insert(register as usize + r, [row[0], row[1], row[2], row[3]]);
        }
        self.ops.lock().unwrap().push(Op::Constants(register, words));
        true
    }
    fn make_space(&self, _: &mut dyn GuestMemory, minimum: u32, requested: u32) -> Option<u32> {
        self.ops.lock().unwrap().push(Op::Space(minimum, requested));
        Some(PUSH)
    }
    fn push_viewport(&self, _: &mut dyn GuestMemory, put: u32, _device: u32) -> Option<u32> {
        self.ops.lock().unwrap().push(Op::PushViewport(put));
        Some(put + 13 * 4)
    }
    fn viewport(&self, mem: &mut dyn GuestMemory, at: u32) -> bool {
        let v: [u32; 6] = std::array::from_fn(|i| mem.u32(at + 4 * i as u32).unwrap());
        self.ops.lock().unwrap().push(Op::Viewport(v));
        true
    }
}

fn set_f(m: &mut FlatMemory, at: u32, v: f32) {
    m.set_f32(at, v).unwrap();
}

/// A game view of kind 0 over 640 x 480 that clears, with the game's
/// constants, its target 0's surfaces and a device with room in its push
/// buffer.
fn mem() -> FlatMemory {
    let mut m = FlatMemory::new(0x0001_0000, 0x0060_0000);
    for (at, v) in [(ONE, 1.0), (HALF, 0.5), (MINUS_HALF, -0.5), (SCALE, 1.0), (SCALE_MIN, 0.5), (Z_OFFSET, 0.25)] {
        set_f(&mut m, at, v);
    }
    for (at, v) in [(KIND2_EXTENT, 2.0), (ROW_SCALE, 1.0), (TRANSFORM_27, 3.0), (TRANSFORM_31, 4.0), (PUSH_W, 5.0)] {
        set_f(&mut m, at, v);
    }
    for (at, v) in [(CLIP_NEAR, 0.0625), (CLIP_FAR, 1024.0), (COLOUR_SCALE, 255.0), (RESET_SCALAR, 7.0), (FOG_FLOOR, 0.001)] {
        set_f(&mut m, at, v);
    }
    m.set_u32(DEVICE, DEVICE_AT).unwrap();
    m.set_u32(DEVICE_AT, PUSH + 0x100).unwrap();
    m.set_u32(DEVICE_AT + 4, PUSH + 0x8000).unwrap();
    m.set_u32(DEVICE_AT + DEVICE_CLIP + 8, 1).unwrap();
    m.set_u32(PUSH_THRESHOLD, 0x400).unwrap();
    m.set_u32(VIEW_COLOUR, COLOUR).unwrap();
    m.set_u32(VIEW_DEPTH, DEPTH).unwrap();
    // The view: kind 0, a clear, the window, the matrices, the clear colour.
    m.set_u16(VIEW + 2, 0).unwrap();
    m.set_u16(VIEW + 8, 1).unwrap();
    for (off, v) in [(0x48, 0u16), (0x4a, 0), (0x4c, 480), (0x4e, 640)] {
        m.set_u16(VIEW + off, v).unwrap();
    }
    set_f(&mut m, VIEW + 0x5c, 2.0);
    for k in 1..=12 {
        set_f(&mut m, VIEW + 0x8c + 4 * k, k as f32);
    }
    for i in 0..16 {
        set_f(&mut m, VIEW + 0x8c + 0x78 + 4 * i, 0.5 * i as f32);
    }
    for (off, v) in [(0xc, 1.0), (0x10, 0.5), (0x14, 0.0)] {
        set_f(&mut m, VIEW + off, v);
    }
    m
}

fn plan_of(m: &FlatMemory) -> Setup {
    plan(m, VIEW).expect("covered")
}

/// The setup's device calls in the body's order, through the host, and its
/// game-side words: the context, the target, the stencil and the draw
/// state's mirrors, the transform's shadow and the raw push. No guest D3D
/// entry is made.
#[test]
fn the_view_setup_makes_no_d3d_entry() {
    let mut m = mem();
    let dev = Device::default();
    let s = plan_of(&m);
    apply(&mut m, &dev, &s, STACK).unwrap();
    let ops = dev.ops();
    assert_eq!(ops[0], Op::State(SLOT_STENCIL_ENABLE, 0), "the stencil off first");
    assert_eq!(ops[1], Op::PushViewport(PUSH + 0x100), "the clip range pushed at put");
    assert!(matches!(&ops[2], Op::Constants(0, w) if w.len() == 48), "the transform at c[-96]");
    assert_eq!(ops[3], Op::Bind(Bind::Full, COLOUR, DEPTH), "target 0's pair");
    assert_eq!(ops[4], Op::Viewport([0, 0, 640, 480, 0, 1f32.to_bits()]));
    assert_eq!(ops[5], Op::Clear(0xf0 | 3, 0x00ff_8000), "colour, depth and stencil in the view's colour");
    assert_eq!(ops[6], Op::State(SLOT_FILL_MODE, SOLID));
    assert_eq!(ops.len(), 7, "no fog");
    assert_eq!(m.bytes(CONTEXT, VIEW_SIZE as usize).unwrap(), m.bytes(VIEW, VIEW_SIZE as usize).unwrap());
    assert_eq!((m.u32(TARGET).unwrap(), m.u32(FILL_MODE).unwrap(), m.u8(DRAW_BEGIN).unwrap()), (0, SOLID, 1));
    assert_eq!(m.u32(TRANSFORM_SHADOW + 4 * 31).unwrap(), 4f32.to_bits(), "the shadow holds the transform");
    assert_eq!(m.u32(DRAW_STATE + 0x1330).unwrap(), 7f32.to_bits());
    assert_eq!(m.u32(DRAW_STATE + 0x40).unwrap(), u32::MAX);
    let put = PUSH + 0x100 + 13 * 4;
    assert_eq!(m.u32(put).unwrap(), 0x0010_1af0, "the raw push after the viewport's");
    assert_eq!(m.u32(put + 16).unwrap(), 5f32.to_bits());
    assert_eq!(m.u32(DEVICE_AT).unwrap(), put + 0x14);
    assert_eq!(m.u32(DEVICE_AT + DEVICE_CLIP).unwrap(), 0.0625f32.to_bits());
    assert_eq!(m.u32(DEVICE_AT + DEVICE_CLIP + 8).unwrap(), 3, "the game owns the clip range");
    assert_eq!(m.u32(native_targets::CACHE_COLOUR).unwrap(), COLOUR, "the select's cache");
}

/// The transform is `0x00012fa0`'s arithmetic: the rows, the view's
/// words, and the screen mapping of the window.
#[test]
fn the_transform_is_the_bodys_arithmetic() {
    let mut m = mem();
    set_f(&mut m, VIEW + 0x8c + 0x38, 11.0);
    let t = transform(&m, VIEW).unwrap();
    let f = |i: usize| f32::from_bits(t[i]);
    // Row 0: (a2 v3 + v1 a0 + a1 v2) s with a = 0, 0.5 * 4, 0.5 * 8, 0.5 * 12 and s = 1 / 2.
    assert_eq!(f(0), (4.0 * 3.0 + 1.0 * 0.0 + 2.0 * 2.0) * 0.5);
    assert_eq!(f(3), 6.0 * 0.5 + (4.0 * 12.0 + 11.0 * 2.0 + 0.0 * 10.0) * 0.5);
    assert_eq!((f(19), f(23), f(27), f(31)), (1.0, 0.5, 3.0, 4.0));
    assert_eq!(t[16], 11f32.to_bits(), "the matrix word at +0xc4");
    assert_eq!(f(32), 320.0, "width / 2");
    assert_eq!(f(35), 641.0 * 0.5 + 0.25);
    assert_eq!(f(37), -240.0, "height / -2");
    assert_eq!(f(39), 481.0 * 0.5 + 0.25);
    assert_eq!((f(40), f(41), f(42)), (320.0, -240.0, 1.0));
    assert_eq!((f(44), f(45)), (320.5, 240.5));
    assert_eq!((t[33], t[46], t[47]), (0, 0, 0));
}

/// View kind 2 maps a 2 x 2 square at the origin, with no scale or offset,
/// onto a 128 square viewport.
#[test]
fn view_kind_two_is_the_square() {
    let mut m = mem();
    m.set_u16(VIEW + 2, 2).unwrap();
    let t = transform(&m, VIEW).unwrap();
    assert_eq!(f32::from_bits(t[32]), 1.0);
    assert_eq!(f32::from_bits(t[35]), 1.5, "(2 + 1) / 2, no offset");
    assert_eq!(viewport(&m, VIEW, 0).unwrap(), [0, 0, 0x80, 0x80, 0, 1f32.to_bits()]);
}

/// `0x00131ed0` packs red, green and blue as bytes, each rounded to
/// nearest even.
#[test]
fn the_clear_colour_packs_as_the_guest_rounds() {
    let mut m = mem();
    for (off, v) in [(0, 1.0), (4, 0.5), (8, 2.0 / 255.0)] {
        set_f(&mut m, 0x0050_0000 + off, v);
    }
    assert_eq!(pack(&m, 0x0050_0000).unwrap(), 0x00ff_8002, "127.5 rounds to 128");
    set_f(&mut m, 0x0050_0000, 1.0e30);
    assert_eq!(pack(&m, 0x0050_0000).unwrap() >> 16, 0, "out of range: the indefinite's low byte");
}

/// View kind 1 draws to target `0x12`: the cache's surface on the view's
/// depth, the select's flag 0, and no prelude.
#[test]
fn view_kind_one_draws_to_the_cached_target() {
    let mut m = mem();
    m.set_u16(VIEW + 2, 1).unwrap();
    m.set_u8(VIEW + 7, 1).unwrap();
    let record = TARGET_CACHE + CACHED_TARGET * TARGET_RECORD;
    m.set_u32(record + 0x88, 1).unwrap();
    m.set_i32(record + 0x78, 64).unwrap();
    let dev = Device::default();
    let s = plan_of(&m);
    assert_eq!((s.colour, s.depth, s.flag), (record + 0x18, DEPTH, 0));
    apply(&mut m, &dev, &s, STACK).unwrap();
    assert_eq!(m.u32(TARGET).unwrap(), CACHED_TARGET);
    assert_eq!(m.u8(native_targets::CACHE_FLAG).unwrap(), 0);
    let ops = dev.ops();
    assert_eq!(ops[3], Op::Bind(Bind::Full, record + 0x18, DEPTH));
    assert_eq!(ops[5], Op::Clear(0xf3, 0x00ff_8000), "no prelude off target 0");
    m.set_u8(record + 0x95, 1).unwrap();
    assert_eq!(plan_of(&m).colour, 0, "an off record: no surface");
}

/// The viewport takes the clamped scale off the screen target, and is the
/// 640 x 480 screen on it with the view's `+7`.
#[test]
fn the_viewport_scales_the_window() {
    let mut m = mem();
    m.set_u8(VIEW + 7, 1).unwrap();
    set_f(&mut m, SCALE, 0.75);
    assert_eq!(viewport(&m, VIEW, 0).unwrap(), [0, 0, 480, 360, 0, 1f32.to_bits()]);
    set_f(&mut m, SCALE, 0.1);
    assert_eq!(viewport(&m, VIEW, 0).unwrap()[2..4], [320, 240], "clamped at the floor");
    m.set_u16(VIEW + 0x4a, 3).unwrap();
    set_f(&mut m, SCALE, 0.75);
    assert_eq!(viewport(&m, VIEW, 0).unwrap()[..3], [2, 0, 477], "truncated: 2.25, 637 * 0.75");
    m.set_u32(SCREEN_B.0, 1).unwrap();
    assert_eq!(screen_target(&m).unwrap(), 0x12);
    assert_eq!(viewport(&m, VIEW, CACHED_TARGET).unwrap(), [0, 0, 640, 480, 0, 1f32.to_bits()], "the screen target");
    m.set_u8(VIEW + 7, 0).unwrap();
    assert_eq!(viewport(&m, VIEW, CACHED_TARGET).unwrap()[..4], [3, 0, 637, 480], "scale 1 on the screen target");
    m.set_u32(SCREEN_A.0, 1).unwrap();
    assert_eq!(screen_target(&m).unwrap(), 0x14);
}

/// A clear at target 0 with the view's `+7` makes the prelude first: the
/// screen's colour cleared black between two viewports.
#[test]
fn a_wide_clear_makes_the_prelude() {
    let mut m = mem();
    m.set_u8(VIEW + 7, 1).unwrap();
    let dev = Device::default();
    apply(&mut m, &dev, &plan_of(&m), STACK).unwrap();
    let ops = dev.ops();
    let screen = [0, 0, 640, 480, 0, 1f32.to_bits()];
    assert_eq!(ops[4..9], [Op::Viewport(screen), Op::Viewport(screen), Op::Clear(0xf0, 0), Op::Viewport([0; 6]), Op::Clear(0xf3, 0x00ff_8000)]);
}

/// The draw state's restore list, after the fill mode: a simple state
/// through its push header, a deferred one's dirty bits, a setter.
#[test]
fn the_restore_list_is_made_after_the_fill_mode() {
    let mut m = mem();
    let (implementation, states, list) = (0x0050_0000, 0x0050_1000, 0x0050_2000);
    m.set_u32(DRAW_STATE + 0x24, implementation).unwrap();
    m.set_u16(implementation + 0x11c, 3 << 9 | 2).unwrap();
    m.set_u32(DRAW_STATE + 0x20, states).unwrap();
    m.set_u32(states + 0x14, list).unwrap();
    let entries: [(u8, u32); 3] = [(3, 0x11), (0x60, 0x22), (0x8b, 0x33)];
    for (i, (slot, value)) in entries.iter().enumerate() {
        let e = list + 10 + 5 * i as u32;
        m.set_u8(e, *slot).unwrap();
        m.write(e + 1, &value.to_le_bytes()).unwrap();
    }
    m.set_u32(SIMPLE_HEADERS + 4 * 3, 0x0004_0300).unwrap();
    m.set_u32(DEFERRED_BITS + 4 * 0x60, 0x80).unwrap();
    m.set_u32(DEFERRED_DIRTY, 1).unwrap();
    let s = plan_of(&m);
    assert_eq!(
        s.restore,
        [
            Restore::Simple { slot: 3, header: 0x0004_0300, value: 0x11 },
            Restore::Deferred { slot: 0x60, bits: 0x80, value: 0x22 },
            Restore::Setter { slot: 0x8b, value: 0x33 },
        ]
    );
    let dev = Device::default();
    apply(&mut m, &dev, &s, STACK).unwrap();
    let ops = dev.ops();
    assert_eq!(ops[6..9], [Op::State(SLOT_FILL_MODE, SOLID), Op::Simple(0x0004_0300, 0x11), Op::State(0x8b, 0x33)]);
    assert_eq!((m.u32(RENDER_STATES + 4 * 3).unwrap(), m.u32(RENDER_STATES + 4 * 0x60).unwrap()), (0x11, 0x22));
    assert_eq!(m.u32(DEFERRED_DIRTY).unwrap(), 0x81);
    assert_eq!(m.u32(DRAW_STATE + 0x24).unwrap(), 0, "the reset after the list");
}

/// A fog effect is the caller's last call; the setup plans the rest.
#[test]
fn a_fog_effect_is_called_last() {
    let mut m = mem();
    m.set_u8(VIEW + 0x26c, 1).unwrap();
    m.set_u32(VIEW + 0x1b8, 3).unwrap();
    assert!(plan_of(&m).fog_effect);
    m.set_u32(VIEW + 0x1b8, u32::MAX).unwrap();
    assert!(!plan_of(&m).fog_effect);
}

/// With the view's fog: its block copied into the context, its constants
/// at c[-84] and its colour, after the fill mode.
#[test]
fn a_fogged_view_sets_its_fog_after_the_fill_mode() {
    let mut m = mem();
    m.set_u8(VIEW + 0x26c, 1).unwrap();
    m.set_u32(VIEW + 0x1b8, u32::MAX).unwrap();
    for (off, v) in [(0x2c, 1.0), (0x30, 5.0), (0x44, 2.0), (0x48, 10.0), (0xb0, 4.0), (0xb4, 8.0), (0x108, 4.0)] {
        set_f(&mut m, VIEW + 0x14c + off, v);
    }
    set_f(&mut m, VIEW + 0x14c + 0x1c, 1.0);
    set_f(&mut m, CAMERA + 0xc, 2.0);
    let dev = Device::default();
    let s = plan_of(&m);
    let fog = s.fog.clone().unwrap();
    let f = |i: usize| f32::from_bits(fog.block[i]);
    assert_eq!((f(0), f(8), f(16)), (0.5, 0.5, 0.25), "the camera axis over each range");
    assert_eq!(f(3), 0.0 - 1.0 * 0.25, "0 - (near + d) / range");
    assert_eq!(f(12), 0.0, "0 - 4 / 8 clamps at 0");
    assert_eq!(fog.colour, 0x00ff_0000);
    apply(&mut m, &dev, &s, STACK).unwrap();
    let ops = dev.ops();
    assert_eq!(ops[6], Op::State(SLOT_FILL_MODE, SOLID));
    assert!(matches!(&ops[7], Op::Constants(0xc, w) if w.len() == 0x18));
    assert_eq!(ops[8], Op::State(SLOT_FOG_COLOUR, 0x00ff_0000));
    assert_eq!(m.u32(FOG_COLOUR).unwrap(), 0x00ff_0000);
    assert_eq!(m.u32(FOG_SHADOW).unwrap(), fog.block[0]);
}

/// A full push buffer asks for room as the body's inline writers do.
#[test]
fn a_full_push_buffer_asks_for_room() {
    let mut m = mem();
    m.set_u32(DEVICE_AT, PUSH + 0x8000).unwrap();
    let dev = Device::default();
    let plan = plan_of(&m);
    apply(&mut m, &dev, &plan, STACK).unwrap();
    assert_eq!(dev.ops()[1], Op::Space(0x200, 0x400));
    assert_eq!(dev.ops()[2], Op::PushViewport(PUSH));
}

/// A refused device call is named.
#[test]
fn a_refused_call_is_named() {
    let mut m = mem();
    let dev = Device { refuse_states: true, ..Default::default() };
    let plan = plan_of(&m);
    assert_eq!(apply(&mut m, &dev, &plan, STACK), Err("device"));
    assert!(dev.ops().is_empty());
}

/// The view end clears alpha with the gate above 1 and pops the stack; the
/// view left is the record below.
#[test]
fn the_view_end_clears_alpha_and_pops() {
    let mut m = mem();
    let dev = Device::default();
    m.set_i32(ALPHA_GATE, 2).unwrap();
    m.set_i32(VIEW_STACK, 2).unwrap();
    assert_eq!(end_begin(&mut m, &dev), Ok(Some(VIEW_RECORDS + VIEW_SIZE)));
    assert_eq!(dev.ops(), [Op::Clear(0x80, 0)]);
    assert_eq!(m.i32(VIEW_STACK).unwrap(), 1);
    m.set_i32(ALPHA_GATE, 1).unwrap();
    m.set_i32(VIEW_STACK, 0).unwrap();
    assert_eq!(end_begin(&mut m, &dev), Ok(None), "the last view: nothing left to set up");
    assert_eq!(dev.ops().len(), 1, "no clear at 1");
}

/// After the setup, the end copies the view's kind, fog block and camera.
#[test]
fn the_view_end_copies_the_camera_of_the_view_left() {
    let mut m = mem();
    m.set_i32(VIEW_STACK, 1).unwrap();
    let view = VIEW_RECORDS + VIEW_SIZE;
    m.set_u16(view, 0xfffe).unwrap();
    m.set_u32(view + 0x18, 0x1234).unwrap();
    m.set_u32(view + 0x14c + 0x11c, 0x5678).unwrap();
    end_copies(&mut m).unwrap();
    assert_eq!(m.i32(CAMERA_KIND).unwrap(), -2);
    assert_eq!(m.u32(CAMERA).unwrap(), 0x1234);
    assert_eq!(m.u32(CAMERA_FOG + 0x11c).unwrap(), 0x5678);
}
