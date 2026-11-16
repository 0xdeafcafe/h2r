//! The vertex streams a backend receives are exactly the guest bytes each
//! draw reaches, whatever buffers the device refills: stream buffers come
//! back from the backend holding earlier draws' bytes, larger or smaller
//! than the next draw needs, and a failed read hands its buffer back too.
//! Every draw's streams are checked against bytes read straight from guest
//! memory, through the synchronous backend and through the render thread
//! (whose spent buffers return to the device later and in batches).

mod common;

use common::*;
use d3d::backend::{Recorded, ResolvedDraw};
use d3d::{
    Backend, Device, GuestMemory, RecordingBackend, SparseMemory, ThreadedBackend, UploadPolicy,
};
use gfx::vertex::fvf;
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg};

const VAF: u32 = MISC + 0x3000;
const STREAM_INPUTS: u32 = MISC + 0x3200;
const FUNCTION: u32 = MISC + 0x3400;
const PSDEF: u32 = MISC + 0x3800;
const PS_OBJECT: u32 = MISC + 0x3900;
const LISTS: u32 = MISC + 0x8000;
const UP_DATA: u32 = MISC + 0xC000;
const VERTS: u32 = 2048;
/// Position (float4), colour (D3DCOLOR), texcoord (float2).
const STRIDES: [u32; 3] = [16, 4, 8];
/// A vertex buffer whose data is not mapped.
const BAD_HEADER: u32 = MISC + 0x6100;

fn vb_header(set: u32, stream: u32) -> u32 {
    MISC + 0x6000 + 0x40 * set + 0x10 * stream
}
fn vb_data(set: u32, stream: u32) -> u32 {
    0x0050_0000 + (set * 3 + stream) * VERTS * 16
}
/// Index lists: (first vertex, window width), 30 indices each.
const WINDOWS: [(u32, u32); 4] = [(10, 5), (0, 2000), (1500, 40), (300, 1000)];
fn list(i: usize) -> u32 {
    LISTS + 0x100 * i as u32
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn bytes(&mut self, n: u32) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

fn setup<B: Backend>(backend: B) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    m.map(0x8000_0000 | 0x0050_0000, 2 * 3 * VERTS * 16);
    set_states(&mut d, &mut m);
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None).to_out(false, OutReg::T(0), 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    m.write(FUNCTION, &code).unwrap();
    d.load_vertex_shader_program(&mut m, FUNCTION, 0).unwrap();
    for reg in 0..16u32 {
        let (stream, format) = match reg {
            0 => (0, 0x42),
            3 => (1, 0x40),
            9 => (2, 0x22),
            _ => (0, 0x02),
        };
        for (j, w) in [stream, 0, format, 0].iter().enumerate() {
            m.write_u32(VAF + 16 * reg + 4 * j as u32, *w).unwrap();
        }
    }
    for set in 0..2 {
        for s in 0..3 {
            let h = vb_header(set, s);
            m.write_u32(h, 1 | 0x0100_0000).unwrap();
            m.write_u32(h + 4, vb_data(set, s)).unwrap();
            m.write_u32(h + 8, 0).unwrap();
        }
    }
    m.write_u32(BAD_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(BAD_HEADER + 4, 0x0070_0000).unwrap();
    m.write_u32(BAD_HEADER + 8, 0).unwrap();
    let mut rng = Rng(5);
    for (i, (first, width)) in WINDOWS.iter().enumerate() {
        // Both ends of the window, then anything inside it.
        let mut idx = vec![*first, first + width - 1];
        idx.extend((0..28).map(|_| first + rng.next() % width));
        let bytes: Vec<u8> = idx.iter().flat_map(|i| (*i as u16).to_le_bytes()).collect();
        m.write(list(i), &bytes).unwrap();
    }
    let def = shaders::psh::PixelShaderDef {
        combiner_count: 1,
        ..Default::default()
    };
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    (d, m)
}

/// Three streams from `headers` (stream 1 at byte offset `offset`).
fn bind<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, headers: [u32; 3], offset: u32) {
    d.select_vertex_shader_direct(m, VAF, 0).unwrap();
    for s in 0..3 {
        let o = if s == 1 { offset } else { 0 };
        for (i, w) in [headers[s], STRIDES[s], o].iter().enumerate() {
            m.write_u32(STREAM_INPUTS + 12 * s as u32 + 4 * i as u32, *w)
                .unwrap();
        }
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 3, STREAM_INPUTS)
        .unwrap();
}

fn set_headers(set: u32) -> [u32; 3] {
    [vb_header(set, 0), vb_header(set, 1), vb_header(set, 2)]
}

/// The bytes `count` vertices from `first` occupy in each stream.
fn expect(m: &SparseMemory, set: u32, offset: u32, first: u32, count: u32) -> Vec<(Vec<u8>, u32)> {
    (0..3)
        .map(|s| {
            let stride = STRIDES[s as usize];
            let o = if s == 1 { offset } else { 0 };
            let va = 0x8000_0000 | (vb_data(set, s) + o + first * stride);
            (m.read_vec(va, (count * stride) as usize).unwrap(), stride)
        })
        .collect()
}

fn immediate_quad<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, z: f32) {
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    d.begin(m, QUADLIST).unwrap();
    for (x, y) in [(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0)] {
        d.set_vertex_data(m, 3, [1.0, 0.5, 0.25, 1.0]).unwrap();
        d.set_vertex_data(m, -1, [x, y, z, 1.0]).unwrap();
    }
    d.end(m).unwrap();
}

/// Runs rounds of draws of every kind and size; returns the streams each
/// draw should carry, in order.
fn run<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) -> Vec<Vec<(Vec<u8>, u32)>> {
    let mut want = Vec::new();
    let mut rng = Rng(9);
    // The immediate quad's bytes, from a device whose pool is still empty.
    immediate_quad(d, m, 0.5);
    want.push(Vec::new());
    for round in 0..4 {
        // The guest rewrites its vertices between rounds.
        for set in 0..2 {
            for s in 0..3 {
                let bytes = rng.bytes(VERTS * STRIDES[s as usize]);
                m.write(0x8000_0000 | vb_data(set, s), &bytes).unwrap();
            }
        }
        let set = round % 2;
        let offset = 4 * (round % 3);
        bind(d, m, set_headers(set), offset);
        for (i, (first, width)) in WINDOWS.iter().enumerate() {
            d.draw_indexed_vertices(m, 5, 30, list(i)).unwrap();
            want.push(expect(m, set, offset, *first, *width));
            // A small draw straight after: a big buffer comes back.
            d.draw_vertices(m, TRIANGLESTRIP, 7 + round, 3 + i as u32)
                .unwrap();
            want.push(expect(m, set, offset, 7 + round, 3 + i as u32));
        }
        // A read that faults hands its buffer back; nothing is drawn.
        bind(d, m, [vb_header(set, 0), BAD_HEADER, vb_header(set, 2)], 0);
        assert!(d.draw_indexed_vertices(m, 5, 30, list(1)).is_err());
        bind(d, m, set_headers(set), offset);
        d.draw_indexed_vertices(m, 5, 30, list(3)).unwrap();
        want.push(expect(m, set, offset, 300, 1000));

        immediate_quad(d, m, 0.5);
        want.push(Vec::new());
        let up = rng.bytes(4 * 20);
        m.write(UP_DATA, &up).unwrap();
        d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE).unwrap();
        d.draw_vertices_up(m, TRIANGLESTRIP, 4, UP_DATA, 20)
            .unwrap();
        want.push(vec![(up, 20)]);
    }
    want
}

fn check(draws: Vec<ResolvedDraw>, want: Vec<Vec<(Vec<u8>, u32)>>) {
    assert_eq!(draws.len(), want.len());
    let quad = &draws[0].streams;
    assert_eq!(quad.len(), 1);
    assert_eq!(quad[0].data.len(), 4 * 20);
    for (n, (draw, want)) in draws.iter().zip(&want).enumerate() {
        let got: Vec<(Vec<u8>, u32)> = draw
            .streams
            .iter()
            .map(|s| (s.data.clone(), s.stride))
            .collect();
        let want = if want.is_empty() {
            // Immediate quads: the same bytes as the first one.
            quad.iter().map(|s| (s.data.clone(), s.stride)).collect()
        } else {
            want.clone()
        };
        assert!(got == want, "draw {n}: streams differ from guest memory");
        let len = got[0].0.len() as u32 / got[0].1;
        assert_eq!(draw.vertex_count, len, "draw {n}");
    }
}

fn draws(calls: Vec<Recorded>) -> Vec<ResolvedDraw> {
    calls
        .into_iter()
        .filter_map(|c| match c {
            Recorded::Draw { draw, .. } => Some(*draw),
            _ => None,
        })
        .collect()
}

#[test]
fn backend_streams_match_guest_bytes_through_reused_buffers() {
    let (mut d, mut m) = setup(RecordingBackend::new());
    let want = run(&mut d, &mut m);
    check(draws(std::mem::take(&mut d.backend.calls)), want);
}

#[test]
fn render_thread_streams_match_guest_bytes_through_returned_buffers() {
    let (mut d, mut m) = setup(ThreadedBackend::new(RecordingBackend::new()).unwrap());
    let mut want = Vec::new();
    // Enough draws for spent buffers to come back in several batches.
    for _ in 0..4 {
        want.extend(run(&mut d, &mut m));
    }
    let calls = d
        .backend
        .with_inner(|b| std::mem::take(&mut b.calls))
        .unwrap();
    check(draws(calls), want);
}

#[test]
fn read_append_matches_read_and_leaves_out_alone_on_a_fault() {
    let mut m = SparseMemory::new();
    m.map(0x1000, 0x3000);
    let bytes = Rng(1).bytes(0x3000);
    m.write(0x1000, &bytes).unwrap();
    // Across page boundaries, after bytes already there.
    let mut out = vec![7u8; 3];
    m.read_append(0x1ff0, 0x1020, &mut out).unwrap();
    assert_eq!(out[..3], [7, 7, 7]);
    assert_eq!(out[3..], bytes[0xff0..0x2010]);
    // Runs off the mapped pages: an error and `out` as it was.
    let before = out.clone();
    assert!(m.read_append(0x3ff0, 0x20, &mut out).is_err());
    assert_eq!(out, before);
    assert!(m.read_append(0xffff_fff0, 0x20, &mut out).is_err());
    assert_eq!(out, before);
    m.read_append(0x2000, 0, &mut out).unwrap();
    assert_eq!(out, before);
}

#[test]
fn guest_adapter_read_append_matches_read_and_keeps_the_fault() {
    use runtime::guest::{FaultReason, Permissions};
    let mut g = runtime::Guest::new();
    g.map_zeroed(0x10000, 0x2000, Permissions::RW).unwrap();
    let bytes = Rng(2).bytes(0x2000);
    g.write_bytes(0x10000, &bytes, 1).unwrap();
    let m = d3d::runtime_glue::GuestAdapter::new(&mut g);
    let mut want = vec![0; 0x30];
    m.read(0x10ff0, &mut want).unwrap();
    let mut out = vec![1u8];
    m.read_append(0x10ff0, 0x30, &mut out).unwrap();
    assert_eq!(out[1..], want);
    assert!(m.take_fault().is_none());
    let before = out.clone();
    assert!(m.read_append(0x11ff0, 0x20, &mut out).is_err());
    assert_eq!(out, before);
    assert_eq!(m.take_fault().unwrap().reason, FaultReason::Unmapped);
}
