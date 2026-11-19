//! `Device::light_passes_tail` (docs/graphics/native-renderer.md, stage 4):
//! a light the native renderer draws in place of Halo's
//! `render_light_passes` leaves the device as that function's tail does.
//! Two devices from the same state, stencil on and a scissor set: one
//! runs the tail's calls as the game does (`SetRenderState_StencilEnable`
//! and `SetScissors` through their entries, then `0x00015720`'s clip-range
//! stores and the viewport fragment `0x3f6190`), the other the device's
//! tail. The guest's D3D globals and device object, the push buffer and
//! the next draw are the same.

mod common;

use common::*;
use d3d::abi::Regs;
use d3d::layout::device_field as f;
use d3d::{Device, GuestLayout, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};

const RECTS: u32 = MISC + 0x3000;
/// The game's clip range words (`[0x00485adc]`, `[0x00485ae0]`).
const CLIP: [u32; 2] = [0x3f80_0000, 0x4b7f_ffff];

/// A device mid-frame: a quad's states, stencil on, one scissor, and a
/// clip range the game's tail will replace.
fn mid_frame() -> (Device<RecordingBackend>, SparseMemory) {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    call(&mut d, &mut m, 0x3f5590, Regs::default(), &[1]).unwrap();
    for (i, w) in [8u32, 8, 40, 40].into_iter().enumerate() {
        m.write_u32(RECTS + 4 * i as u32, w).unwrap();
    }
    call(&mut d, &mut m, 0x3f8760, Regs::default(), &[1, 0, RECTS]).unwrap();
    let dev = m.read_u32(GuestLayout::HALO2.p_device).unwrap();
    m.write_u32(dev + f::CLIP_PROGRAM, 0).unwrap();
    m.write_u32(dev + f::CLIP_PROGRAM + 4, 0x3f80_0000).unwrap();
    (d, m)
}

/// The tail as the game runs it.
fn game_tail(d: &mut Device<RecordingBackend>, m: &mut SparseMemory) {
    call(d, m, 0x3f5590, Regs::default(), &[0]).unwrap();
    call(d, m, 0x3f8760, Regs::default(), &[0, 0, 0]).unwrap();
    // 0x00015720.
    let dev = m.read_u32(GuestLayout::HALO2.p_device).unwrap();
    m.write_u32(dev + f::CLIP_PROGRAM, CLIP[0]).unwrap();
    m.write_u32(dev + f::CLIP_PROGRAM + 4, CLIP[1]).unwrap();
    let flags = m.read_u32(dev + f::CLIP_FLAGS).unwrap();
    m.write_u32(dev + f::CLIP_FLAGS, flags | 2).unwrap();
    let put = m.read_u32(dev + f::PUT).unwrap();
    let out = call(
        d,
        m,
        0x3f6190,
        Regs {
            eax: put,
            esi: dev,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    m.write_u32(dev + f::PUT, out.eax.unwrap()).unwrap();
}

fn region(m: &SparseMemory, addr: u32, len: usize) -> Vec<u8> {
    m.read_vec(addr, len).unwrap()
}

#[test]
fn a_skipped_light_leaves_the_device_as_the_games_tail_does() {
    let (mut drawn, mut dm) = mid_frame();
    let (mut skipped, mut sm) = mid_frame();
    game_tail(&mut drawn, &mut dm);
    assert!(skipped.light_passes_tail(&mut sm, CLIP).unwrap());

    // D3D's globals and the device object, and the push buffer.
    assert_eq!(region(&dm, 0x0040_4000, 0x5000), region(&sm, 0x0040_4000, 0x5000));
    assert_eq!(region(&dm, PB.addr, PB.len as usize), region(&sm, PB.addr, PB.len as usize));
    let slot = GuestLayout::HALO2.render_state + 4 * d3d::state::slot::STENCILENABLE;
    assert_eq!(sm.read_u32(slot).unwrap(), 0, "stencil off");

    // The next draw is the same: no scissor, no stencil.
    for (d, m) in [(&mut drawn, &mut dm), (&mut skipped, &mut sm)] {
        d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
    }
    let a = drawn.backend.draws().last().copied().cloned().unwrap();
    let b = skipped.backend.draws().last().copied().cloned().unwrap();
    assert_eq!(a, b);
}

#[test]
fn without_room_for_the_fragment_nothing_is_set() {
    let (mut d, mut m) = mid_frame();
    let dev = m.read_u32(GuestLayout::HALO2.p_device).unwrap();
    let put = m.read_u32(dev + f::PUT).unwrap();
    let threshold = m.read_u32(dev + f::THRESHOLD).unwrap();
    m.write_u32(dev + f::THRESHOLD, put).unwrap();
    let before = region(&m, 0x0040_4000, 0x5000);
    assert!(!d.light_passes_tail(&mut m, CLIP).unwrap());
    assert_eq!(region(&m, 0x0040_4000, 0x5000), before);
    m.write_u32(dev + f::THRESHOLD, threshold).unwrap();
}
