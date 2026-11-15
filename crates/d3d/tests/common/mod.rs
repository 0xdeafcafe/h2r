//! Shared helpers: synthetic guest memory laid out like Halo's, adapter
//! calls through the LTCG entry table, and guest resources.
#![allow(dead_code)]

use d3d::abi::{self, Entry, Outcome, Regs};
use d3d::device::{DeviceConfig, GuestRange, PresentParameters};
use d3d::pushbuf::method;
use d3d::{Backend, D3dError, Device, GuestMemory, SparseMemory, UploadPolicy};
use gfx::vertex::fvf;

pub const PB: GuestRange = GuestRange {
    addr: 0x0100_0000,
    len: 0x4_0000,
};
pub const SCRATCH: GuestRange = GuestRange {
    addr: 0x0200_0000,
    len: 0x1000,
};
pub const MISC: u32 = 0x0300_0000;
pub const PARAMS: u32 = MISC;
pub const PP_DEVICE: u32 = MISC + 0x100;
pub const VB_HEADER: u32 = MISC + 0x1000;
pub const TEX_HEADER: u32 = MISC + 0x2000;
pub const STACK_TOP: u32 = MISC + 0xF000;
/// Physical addresses of resource data (`Data` fields); seen at `0x8000_0000 | pa`.
pub const VB_DATA: u32 = 0x0010_0000;
pub const TEX_DATA: u32 = 0x0020_0000;

pub const W: u32 = 64;
pub const H: u32 = 64;

// Guest render-state slots used below.
pub const RS_ZFUNC: u32 = 57;
pub const RS_ALPHABLENDENABLE: u32 = 59;

// Xbox primitive types.
pub const TRIANGLESTRIP: u32 = 6;
pub const QUADLIST: u32 = 8;

/// A hand-built stand-in for `D3DSIMPLERENDERSTATEENCODE` (the real one is
/// in the XBE and never in the repository): only the slots the tests use
/// carry their method; every other slot shares the non-invertible `0x1D90`.
pub const ENCODE_SLOTS: &[(usize, u32)] = &[
    (RS_ZFUNC as usize, 0x0354),
    (RS_ALPHABLENDENABLE as usize, 0x0304),
    (64, 0x035C), // ZWRITEENABLE
    (82, 0x1D78),
];

/// The synthetic table as push headers.
pub fn synthetic_encode() -> [u32; d3d::pushbuf::SIMPLE_ENCODE_SLOTS] {
    let mut t = [(1 << 18) | 0x1D90; d3d::pushbuf::SIMPLE_ENCODE_SLOTS];
    for &(slot, m) in ENCODE_SLOTS {
        t[slot] = (1 << 18) | m;
    }
    t
}

/// The synthetic table as the bytes the image would hold at
/// `GuestLayout::simple_encode`.
pub fn synthetic_encode_bytes() -> Vec<u8> {
    synthetic_encode()
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect()
}

pub fn memory() -> SparseMemory {
    let mut m = SparseMemory::new();
    m.map(0x0040_4000, 0x5000); // D3D globals and the device object
    let encode = d3d::GuestLayout::HALO2.simple_encode;
    m.map(encode & !0xFFF, 0x1000);
    m.write(encode, &synthetic_encode_bytes()).unwrap();
    m.map(PB.addr, PB.len);
    m.map(SCRATCH.addr, SCRATCH.len);
    m.map(MISC, 0x1_0000);
    m.map(0x8000_0000 | VB_DATA, 0x1000);
    m.map(0x8000_0000 | TEX_DATA, 0x1000);
    m
}

/// Runs entry `addr` through its LTCG adapter, as the runtime would.
pub fn call<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    addr: u32,
    regs: Regs,
    stack: &[u32],
) -> Result<Outcome, D3dError> {
    let entries = abi::entries::<B>();
    let e: &Entry<B> = entries.iter().find(|e| e.addr == addr).expect("entry");
    let esp = STACK_TOP - 4 - 4 * stack.len() as u32;
    m.write_u32(esp, 0xDEAD_BEEF).unwrap(); // return address
    for (i, v) in stack.iter().enumerate() {
        m.write_u32(esp + 4 + 4 * i as u32, *v).unwrap();
    }
    let out = abi::call(e, d, m, &Regs { esp, ..regs })?;
    assert_eq!(
        out.cleanup,
        4 * stack.len() as u32,
        "{} pops its arguments",
        e.name
    );
    Ok(out)
}

pub fn create<B: Backend>(backend: B, upload: UploadPolicy) -> (Device<B>, SparseMemory) {
    let mut m = memory();
    write_params(&mut m);
    let mut d = Device::new(
        DeviceConfig {
            upload,
            ..DeviceConfig::halo2(PB, SCRATCH)
        },
        backend,
    );
    // Halo: EAX = BehaviorFlags 0x40, ECX = &g_device, [esp+4] = &params.
    let out = call(
        &mut d,
        &mut m,
        0x3f5240,
        Regs {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..Regs::default()
        },
        &[PARAMS],
    )
    .unwrap();
    assert_eq!(out.eax, Some(0));
    (d, m)
}

/// The test target's present parameters at `PARAMS`.
pub fn write_params(m: &mut SparseMemory) {
    PresentParameters {
        width: W,
        height: H,
        back_buffer_format: 0x06, // D3DFMT_A8R8G8B8
        back_buffer_count: 1,
        multisample_type: 0,
        swap_effect: 0,
        enable_auto_depth_stencil: true,
        auto_depth_stencil_format: 0x2A, // D3DFMT_D24S8
        flags: 0,
        refresh_rate: 0,
        presentation_interval: 0x8000_0000,
        buffer_surface: 0,
        depth_stencil_surface: 0,
    }
    .write(m, PARAMS)
    .unwrap();
}

/// `XYZRHW | DIFFUSE` vertex.
pub fn vertex(x: f32, y: f32, color: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for f in [x, y, 0.5, 1.0] {
        v.extend_from_slice(&f.to_le_bytes());
    }
    v.extend_from_slice(&color.to_le_bytes());
    v
}

/// A vertex buffer header at `VB_HEADER` holding a strip quad.
pub fn quad_vb(m: &mut SparseMemory, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
    let mut bytes = Vec::new();
    for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        bytes.extend(vertex(x, y, color));
    }
    m.write(0x8000_0000 | VB_DATA, &bytes).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap(); // type 0 (VB), refcount 1, D3DCREATED
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
}

/// What game code does for an inlined Begin/SetVertexData/End: reserve
/// space at put, store NV2A method words, advance put.
pub fn inline_quad(m: &mut SparseMemory, x0: f32, y0: f32, x1: f32, y1: f32, rgba: [u8; 4]) {
    let dev = d3d::GuestLayout::HALO2.device;
    let mut put = m.read_u32(dev).unwrap();
    let threshold = m.read_u32(dev + 4).unwrap();
    assert!(put < threshold);
    let mut push = |w: u32| {
        m.write_u32(put, w).unwrap();
        put += 4;
    };
    let header = |method: u32, count: u32| (count << 18) | method;
    push(header(method::BEGIN_END, 1));
    push(QUADLIST);
    // Diffuse once (registers persist), then four positions.
    push(header(method::VERTEX_DATA4UB + 4 * 3, 1));
    push(u32::from_le_bytes(rgba));
    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
        push(header(method::VERTEX4F, 4));
        for f in [x, y, 0.5, 1.0] {
            push(f.to_bits());
        }
    }
    push(header(method::BEGIN_END, 1));
    push(0);
    m.write_u32(dev, put).unwrap();
}

pub fn set_states<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    // SetRenderState_ZEnable(FALSE), SetRenderState_CullMode(NONE).
    call(d, m, 0x3f54f0, Regs::default(), &[0]).unwrap();
    call(d, m, 0x3f53d0, Regs::default(), &[0]).unwrap();
    // Inline state the way game code does it: store into D3D__RenderState
    // and call SetRenderState_Simple with the NV method (ECX) and value (EDX).
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.render_state_slot(RS_ALPHABLENDENABLE), 0)
        .unwrap();
    call(
        d,
        m,
        0x3f5cf0,
        Regs {
            ecx: 0x0004_0304,
            edx: 0,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
}

pub fn bind_quad<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 20).unwrap();
}

pub fn pixel(px: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

/// A 16x4 `LIN_A8R8G8B8` texture (pitch 64) at `TEX_HEADER`.
pub fn texture(m: &mut SparseMemory, fill: u8) {
    m.write_u32(TEX_HEADER, 1 | (4 << 16) | 0x0100_0000)
        .unwrap();
    m.write_u32(TEX_HEADER + 4, TEX_DATA).unwrap();
    m.write_u32(TEX_HEADER + 8, 0).unwrap();
    m.write_u32(TEX_HEADER + 12, (2 << 4) | (0x12 << 8) | (1 << 16))
        .unwrap();
    m.write_u32(TEX_HEADER + 16, 15 | (3 << 12)).unwrap(); // pitch 64: (64 / 64 - 1) << 24 = 0
    m.write(0x8000_0000 | TEX_DATA, &[fill; 256]).unwrap();
}
