//! `Device::light_passes_tail` (docs/graphics/native-lights.md, stage 4):
//! a light the native renderer draws in place of Halo's
//! `render_light_passes` leaves the device as that function's tail does.
//! Two devices from the same state, stencil on and a scissor set: one
//! runs the tail's calls as the game does (`SetRenderState_StencilEnable`
//! and `SetScissors` through their entries, then `0x00015720`'s clip-range
//! stores and the viewport fragment `0x3f6190`), the other the device's
//! tail. The guest's D3D globals and device object, the push buffer and
//! the next draw are the same.

use crate::common::*;
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

    // The device's hardware view and bound target are the same: no scissor,
    // no stencil for the next draw.
    assert_eq!(drawn.hw(), skipped.hw());
    assert_eq!(drawn.bound_target(), skipped.bound_target());
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

#[test]
fn a_view_light_matches_the_layer_target_setters_constants_and_final_viewport() {
    for layers in 0..=2 {
        let (mut guest, mut gm) = mid_frame();
        let (mut typed, mut tm) = mid_frame();
        let frame = std::array::from_fn(|i| 0x3f00_0000 + i as u32);
        let data = RECTS + 64;
        let header = synthetic_encode()[82];
        for (i, word) in frame.iter().enumerate() {
            gm.write_u32(data + 4 * i as u32, *word).unwrap();
            tm.write_u32(data + 4 * i as u32, *word).unwrap();
        }
        for (device, memory) in [(&mut guest, &mut gm), (&mut typed, &mut tm)] {
            assert!(device.begin_external_boundary(memory).unwrap());
        }
        let dev = gm.read_u32(GuestLayout::HALO2.p_device).unwrap();
        guest.set_vertex_shader_constant_dwords(&mut gm, 31, data, 36).unwrap();
        let vp = dev + f::VIEWPORT;
        for _ in 0..layers {
            guest.set_viewport(&mut gm, vp).unwrap();
            guest.set_render_state_simple(&mut gm, header, 0).unwrap();
            guest.set_render_state(&mut gm, 158, 0).unwrap();
            guest.set_render_state(&mut gm, d3d::state::slot::STENCILENABLE, 0).unwrap();
        }
        game_tail(&mut guest, &mut gm);
        guest.set_viewport(&mut gm, vp).unwrap();

        let held = typed.prepare_external_sequence(&mut tm, Vec::new()).unwrap().unwrap();
        assert!(held.commit_light(&mut tm, CLIP, &frame, None, layers, Some(header), []).unwrap());
        assert_eq!(region(&gm, 0x0040_4000, 0x5000), region(&tm, 0x0040_4000, 0x5000));
        assert_eq!(region(&gm, PB.addr, PB.len as usize), region(&tm, PB.addr, PB.len as usize));
        assert_eq!(typed.hw(), guest.hw());
        assert_eq!(typed.bound_target(), guest.bound_target());
        for reg in 0..192 {
            assert_eq!(typed.constant(reg).map(f32::to_bits), guest.constant(reg).map(f32::to_bits));
        }
    }
}

#[test]
fn a_late_tail_memory_refusal_restores_the_guest_and_device() {
    struct ReadOnlyWord<'a> {
        memory: &'a mut SparseMemory,
        word: u32,
    }
    impl GuestMemory for ReadOnlyWord<'_> {
        fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), d3d::GuestError> {
            self.memory.read(addr, out)
        }
        fn write(&mut self, addr: u32, bytes: &[u8]) -> Result<(), d3d::GuestError> {
            if addr == self.word { return Err(d3d::GuestError { addr, len: bytes.len() as u32 }); }
            self.memory.write(addr, bytes)
        }
    }
    let (mut d, mut m) = mid_frame();
    assert!(d.begin_external_boundary(&mut m).unwrap());
    let before = region(&m, 0x0040_4000, 0x5000);
    let push_before = region(&m, PB.addr, PB.len as usize);
    let dev = m.read_u32(GuestLayout::HALO2.p_device).unwrap();
    let put = m.read_u32(dev + f::PUT).unwrap();
    let regs = (d.bound_target(), *d.hw(), d.constant(31), d.stats().external_passes);
    let untouched = region(&m, MISC + 0x100, 8);
    let mut locked = ReadOnlyWord { memory: &mut m, word: MISC + 0x10a };
    let writes: [(u32, &[u8]); 3] = [
        (MISC + 0x101, &[2, 3, 4, 5]),
        (MISC + 0x102, &[6, 7]),
        (MISC + 0x10a, &[8]),
    ];
    let held = d.prepare_external_sequence(&mut locked, Vec::new()).unwrap().unwrap();
    assert!(!held.commit_light(&mut locked, CLIP, &[u32::MAX; 36], None, 1,
        Some(synthetic_encode()[82]), writes).unwrap());
    assert_eq!(before, region(&m, 0x0040_4000, 0x5000));
    assert_eq!(untouched, region(&m, MISC + 0x100, 8));
    assert_eq!(push_before, region(&m, PB.addr, PB.len as usize));
    assert_eq!(regs, (d.bound_target(), *d.hw(), d.constant(31), d.stats().external_passes));
    assert_eq!(m.read_u32(dev + f::PUT).unwrap(), put);
    game_tail(&mut d, &mut m);
}
