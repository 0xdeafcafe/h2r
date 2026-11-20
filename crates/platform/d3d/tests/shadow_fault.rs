//! A constant upload whose guest shadow runs into a protected page, through
//! the real `GuestAdapter`: the words before the page are stored and the
//! fault the runtime keeps is the per-word one, not the bulk write's.

use d3d::runtime_glue::GuestAdapter;
use d3d::{D3dError, Device, DeviceConfig, GuestMemory, PresentParameters, RecordingBackend};
use runtime::Guest;
use runtime::guest::{Access, FaultReason, Permissions};

const PB: d3d::GuestRange = d3d::GuestRange {
    addr: 0x0100_0000,
    len: 0x4_0000,
};
const SCRATCH: d3d::GuestRange = d3d::GuestRange {
    addr: 0x0200_0000,
    len: 0x1000,
};
const MISC: u32 = 0x0300_0000;
const SRC: u32 = MISC + 0x4000;

fn encode_bytes() -> Vec<u8> {
    let mut t = [(1u32 << 18) | 0x1D90; d3d::pushbuf::SIMPLE_ENCODE_SLOTS];
    t[57] = (1 << 18) | 0x0354;
    t[59] = (1 << 18) | 0x0304;
    t.iter().flat_map(|w| w.to_le_bytes()).collect()
}

#[test]
fn shadow_fault_reaches_the_runtime_as_the_per_word_fault() {
    let cfg = DeviceConfig::halo2(PB, SCRATCH);
    let mut g = Guest::new();
    // The D3D globals: 0x404000..0x408000 and a separate page at 0x408000
    // (a mapping is protected as a whole).
    g.map_zeroed(0x0040_4000, 0x4000, Permissions::RW).unwrap();
    g.map_zeroed(0x0040_8000, 0x1000, Permissions::RW).unwrap();
    let encode = d3d::GuestLayout::HALO2.simple_encode;
    g.map_zeroed(encode & !0xFFF, 0x1000, Permissions::RW)
        .unwrap();
    g.write_bytes(encode, &encode_bytes(), 1).unwrap();
    g.map_zeroed(MISC, 0x1_0000, Permissions::RW).unwrap();
    d3d::runtime_glue::map_device_memory(&mut g, &cfg).unwrap();

    let mut d = Device::new(cfg, RecordingBackend::new());
    {
        let mut m = GuestAdapter::new(&mut g);
        PresentParameters {
            width: 64,
            height: 64,
            back_buffer_format: 0x06,
            back_buffer_count: 1,
            multisample_type: 0,
            swap_effect: 0,
            enable_auto_depth_stencil: true,
            auto_depth_stencil_format: 0x2A,
            flags: 0,
            refresh_rate: 0,
            presentation_interval: 0x8000_0000,
            buffer_surface: 0,
            depth_stencil_surface: 0,
        }
        .write(&mut m, MISC)
        .unwrap();
        d.create_device(&mut m, 0x40, MISC, MISC + 0x100).unwrap();
        for (i, w) in [0x1111_1111u32, 0x1111_1112, 0x1111_1113, 0x1111_1114]
            .iter()
            .enumerate()
        {
            m.write_u32(SRC + 4 * i as u32, *w).unwrap();
        }
        assert!(m.take_fault().is_none());
    }
    // Register 140's shadow is 16 bytes at 0x407ff8; 0x408000 is read-only.
    assert_eq!(d3d::GuestLayout::HALO2.vs_constants + 16 * 140, 0x0040_7FF8);
    g.protect(0x0040_8000, Permissions::READ).unwrap();

    let mut m = GuestAdapter::new(&mut g);
    let err = d
        .set_vertex_shader_constant(&mut m, 140, SRC, 1, true)
        .unwrap_err();
    assert!(
        matches!(
            err,
            D3dError::Guest(d3d::GuestError {
                addr: 0x0040_8000,
                len: 4
            })
        ),
        "{err:?}"
    );
    // The words before the page persist.
    assert_eq!(m.read_u32(0x0040_7FF8).unwrap(), 0x1111_1111);
    assert_eq!(m.read_u32(0x0040_7FFC).unwrap(), 0x1111_1112);
    // And the fault the runtime keeps is the faulting word's.
    let fault = m.take_fault().expect("a fault was recorded");
    assert_eq!(
        (fault.addr, fault.len, fault.access, fault.reason),
        (0x0040_8000, 4, Access::Write, FaultReason::Protection)
    );
}
