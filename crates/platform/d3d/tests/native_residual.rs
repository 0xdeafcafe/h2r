//! Whole clears, device-owned target switches, and flare result parity.

use crate::common::*;
use d3d::abi::Regs;
use d3d::hw::method as hw;
use d3d::layout::device_field as field;
use d3d::{GuestLayout, GuestMemory, Recorded, RecordingBackend, UploadPolicy};

#[derive(Debug, PartialEq)]
struct Trace {
    calls: Vec<Recorded>,
    target: [u32; 8],
    result: [u32; 3],
}

fn run(native: bool) -> Trace {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    d.set_native_residual(native);
    set_states(&mut d, &mut m);
    d.set_vertex_shader(&mut m, gfx::vertex::fvf::XYZRHW | gfx::vertex::fvf::DIFFUSE)
        .unwrap();
    let dev = GuestLayout::HALO2.device;
    let depth = m.read_u32(dev + field::DEPTH_STENCIL).unwrap();
    let color = m.read_u32(dev + field::RENDER_TARGET).unwrap();
    d.set_render_target_same_size(&mut m, color, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xf3, 0xff80_4020, 0.25, 7).unwrap();
    d.set_render_target(&mut m, color, depth).unwrap();
    d.clear(&mut m, 0, 0, 0xf3, 0xff01_2345, 0.5, 23).unwrap();
    d.clear(&mut m, 0, 0, 0x01, 0, 1.0, 0).unwrap();
    // One masked channel still takes the rectangular clear path.
    d.clear(&mut m, 0, 0, 0x10, 0, 1.0, 0).unwrap();

    let mut put = m.read_u32(dev + field::PUT).unwrap();
    for word in [(2 << 18) | hw::CLEAR_REPORT_VALUE, 1, 1] {
        m.write_u32(put, word).unwrap();
        put += 4;
    }
    m.write_u32(dev + field::PUT, put).unwrap();
    inline_quad(&mut m, 0.0, 0.0, 8.0, 8.0, [0, 255, 0, 255]);
    d.end_visibility_test(&mut m, 6).unwrap();

    const RESULT: u32 = MISC + 0x800;
    let regs = Regs {
        eax: 6,
        edx: RESULT + 4,
        ..Regs::default()
    };
    for offset in [0, 4, 8] {
        m.write_u32(RESULT + offset, 0xdead_beef).unwrap();
    }
    d.backend.pixel_counts_pending = true;
    let incomplete = call(&mut d, &mut m, 0x003f_8630, regs, &[RESULT]).unwrap();
    assert_eq!(incomplete.eax, Some(d3d::device::D3DERR_TESTINCOMPLETE));
    for offset in [0, 4, 8] {
        assert_eq!(m.read_u32(RESULT + offset).unwrap(), 0xdead_beef);
    }
    d.backend.pixel_counts_pending = false;
    let ready = call(&mut d, &mut m, 0x003f_8630, regs, &[RESULT]).unwrap();
    assert_eq!(ready.eax, Some(0));
    assert_eq!(m.read_u32(RESULT).unwrap(), W * H);

    Trace {
        target: d.bound_target(),
        result: std::array::from_fn(|i| m.read_u32(RESULT + i as u32 * 4).unwrap()),
        calls: d.backend.calls,
    }
}

#[test]
fn attachment_paths_and_query_results_match_the_d3d_reference() {
    let baseline = run(false);
    assert!(
        baseline
            .calls
            .iter()
            .any(|call| matches!(call, Recorded::Clear { .. }))
    );
    assert!(
        baseline
            .calls
            .iter()
            .any(|call| matches!(call, Recorded::ClearRects { .. }))
    );
    assert_eq!(run(true), baseline);
}
