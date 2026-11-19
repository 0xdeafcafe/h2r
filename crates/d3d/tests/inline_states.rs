//! `Device::set_render_states_inline` (docs/graphics/native-renderer.md,
//! stage 5): the render states the shader draw-state reset (`0x00016b10`,
//! through `0x00018900`) restores at the start of `render_screen_effects`,
//! set for a call the native renderer draws in its place. Two devices
//! from the same state: one runs `0x00018900`'s loop as the game does
//! (`SetRenderState_Simple` through its entry with the image's header and
//! the slot stored; a deferred state's dirty bits and slot; a setter
//! through its entry from the function table), the other the device's
//! method. The guest's D3D globals and device object, the push buffer and
//! the next draw are the same. A list the device cannot set as the game
//! would sets nothing.

mod common;

use common::*;
use d3d::abi::Regs;
use d3d::device::{DEFERRED_DIRTY_BITS, DEFERRED_STATES_END, RENDER_STATE_FUNCTIONS};
use d3d::{Device, GuestLayout, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};

/// Deferred states the synthetic tables name, with their dirty bits.
const DEFERRED: [(u32, u32); 2] = [(96, 0x0000_0040), (112, 0x0000_2000)];
/// `SetRenderState_StencilEnable`, `SetRenderState_CullMode`.
const SETTERS: [(u32, u32); 2] = [(144, 0x3f5590), (147, 0x3f53d0)];

/// The restore list: every path of the loop, in an order that mixes them.
const STATES: [(u32, u32); 7] = [
    (RS_ZFUNC, 3),
    (96, 0),
    (144, 1),
    (RS_ALPHABLENDENABLE, 1),
    (147, 0),
    (112, 0),
    (64, 0),
];

/// A device mid-frame, with the image's two state tables (synthetic: only
/// the states the list uses) and the deferred slots holding other values.
fn mid_frame() -> (Device<RecordingBackend>, SparseMemory) {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    m.map(RENDER_STATE_FUNCTIONS & !0xFFF, 0x1000);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    let l = GuestLayout::HALO2;
    for (state, bits) in DEFERRED {
        m.write_u32(DEFERRED_DIRTY_BITS + 4 * state, bits).unwrap();
        m.write_u32(l.render_state_slot(state), 5).unwrap();
    }
    for (state, f) in SETTERS {
        m.write_u32(RENDER_STATE_FUNCTIONS + 4 * state, f).unwrap();
    }
    (d, m)
}

/// `0x00018900`'s loop as the game runs it.
fn game_loop(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, states: &[(u32, u32)]) {
    let l = GuestLayout::HALO2;
    for &(state, value) in states {
        if state < 0x5c {
            // 0x0001893b: ECX = the header, EDX = the value, then the slot.
            let header = m.read_u32(l.simple_encode + 4 * state).unwrap();
            call(
                d,
                m,
                0x3f5cf0,
                Regs {
                    ecx: header,
                    edx: value,
                    ..Regs::default()
                },
                &[],
            )
            .unwrap();
            m.write_u32(l.render_state_slot(state), value).unwrap();
        } else if state < 0x88 {
            // 0x0001895a: the dirty bits, then the slot.
            let bits = m.read_u32(DEFERRED_DIRTY_BITS + 4 * state).unwrap();
            let dirty = m.read_u32(l.dirty_flags).unwrap();
            m.write_u32(l.dirty_flags, dirty | bits).unwrap();
            m.write_u32(l.render_state_slot(state), value).unwrap();
        } else {
            // 0x00018978: push the value, call the table's function.
            let f = m.read_u32(RENDER_STATE_FUNCTIONS + 4 * state).unwrap();
            call(d, m, f, Regs::default(), &[value]).unwrap();
        }
    }
}

fn region(m: &SparseMemory, addr: u32, len: usize) -> Vec<u8> {
    m.read_vec(addr, len).unwrap()
}

#[test]
fn the_inlined_setter_leaves_the_device_as_the_games_loop_does() {
    assert_eq!(DEFERRED_STATES_END, 0x88);
    let (mut game, mut gm) = mid_frame();
    let (mut port, mut pm) = mid_frame();
    game_loop(&mut game, &mut gm, &STATES);
    assert!(port.set_render_states_inline(&mut pm, &STATES).unwrap());

    // D3D's globals and the device object, and the push buffer.
    assert_eq!(region(&gm, 0x0040_4000, 0x5000), region(&pm, 0x0040_4000, 0x5000));
    assert_eq!(region(&gm, PB.addr, PB.len as usize), region(&pm, PB.addr, PB.len as usize));
    let l = GuestLayout::HALO2;
    assert_eq!(pm.read_u32(l.render_state_slot(96)).unwrap(), 0, "a deferred slot is stored");
    assert_eq!(pm.read_u32(l.dirty_flags).unwrap() & 0x2040, 0x2040, "and its bits set");
    assert_eq!(pm.read_u32(l.render_state_slot(144)).unwrap(), 1, "a setter's slot");

    // The next draw is the same.
    for (d, m) in [(&mut game, &mut gm), (&mut port, &mut pm)] {
        d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
    }
    let a = game.backend.draws().last().copied().cloned().unwrap();
    let b = port.backend.draws().last().copied().cloned().unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_list_it_cannot_set_as_the_game_would_sets_nothing() {
    let (mut d, mut m) = mid_frame();
    let before = |m: &SparseMemory| (region(m, 0x0040_4000, 0x5000), region(m, PB.addr, PB.len as usize));
    let start = before(&m);
    // A simple state whose header the device does not know (the synthetic
    // table's non-invertible 0x1D90), after states it could set.
    assert!(!d.set_render_states_inline(&mut m, &[(RS_ZFUNC, 3), (96, 0), (20, 1)]).unwrap());
    assert_eq!(before(&m), start);
    // A function that is not the state's setter.
    m.write_u32(RENDER_STATE_FUNCTIONS + 4 * 147, 0x3f5590).unwrap();
    assert!(!d.set_render_states_inline(&mut m, &[(144, 1), (147, 0)]).unwrap());
    assert_eq!(before(&m), start);
    // A state past the slots.
    assert!(!d.set_render_states_inline(&mut m, &[(200, 0)]).unwrap());
    assert_eq!(before(&m), start);
    // Nothing to set: done.
    assert!(d.set_render_states_inline(&mut m, &[]).unwrap());
}
