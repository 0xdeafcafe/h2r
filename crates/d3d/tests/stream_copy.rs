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
                // Reported, as the runtime's write barrier does: the
                // vertex cache keeps what was not written.
                d.note_write(0x8000_0000 | vb_data(set, s), bytes.len() as u32);
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
            .map(|s| (s.data.to_vec(), s.stride))
            .collect();
        let want = if want.is_empty() {
            // Immediate quads: the same bytes as the first one.
            quad.iter().map(|s| (s.data.to_vec(), s.stride)).collect()
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

/// Draws of a program left as it is in program memory share one copy of
/// its microcode (the shader key compares it by pointer); another program
/// loaded over it gets its own, and loading the first again gives its
/// words back.
#[test]
fn draws_of_an_unchanged_program_share_its_microcode() {
    use d3d::shader::VertexProcessing;
    let (mut d, mut m) = setup(RecordingBackend::new());
    bind(&mut d, &mut m, set_headers(0), 0);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 4, 4).unwrap();
    let other = program(&[Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None)
        .to_out(false, OutReg::Pos, 0xF)
        .last()]);
    m.write(FUNCTION + 0x200, &other).unwrap();
    d.load_vertex_shader_program(&mut m, FUNCTION + 0x200, 0)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.load_vertex_shader_program(&mut m, FUNCTION, 0).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let code: Vec<_> = draws(std::mem::take(&mut d.backend.calls))
        .into_iter()
        .map(|draw| match draw.shader.vertex {
            VertexProcessing::Program { microcode, .. } => microcode,
            v => panic!("{v:?}"),
        })
        .collect();
    assert_eq!(code.len(), 4);
    assert!(code[0].same(&code[1]), "one copy for an unchanged program");
    assert_ne!(code[1], code[2], "another program");
    assert_eq!(code[2].len(), 4, "one instruction");
    assert_eq!(code[3], code[0], "the first program again");
    assert_eq!(code[0].len(), 12, "three instructions");
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

/// The stream layout a program draw gets follows the attribute format words
/// and the strides bound at that draw, whatever the device derived for the
/// draws before it: formats rewritten in place under the same handle, a
/// stream rebound with another stride, a register moved to another stream,
/// the original formats again, and a fixed-function draw in between. Each
/// draw matches the one a fresh device makes after the same changes.
#[test]
fn layout_follows_formats_and_strides_whatever_came_before() {
    // The program's formats as the device reads them (the selected
    // shader object's copy), and the source the next selection copies.
    fn word(m: &mut SparseMemory, reg: u32, j: u32, w: u32) {
        let obj = d3d::GuestLayout::HALO2.static_vertex_shader;
        m.write_u32(obj + 0x14 + 16 * reg + 4 * j, w).unwrap();
        m.write_u32(VAF + 16 * reg + 4 * j, w).unwrap();
    }
    fn rebind(d: &mut Device<RecordingBackend>, m: &mut SparseMemory, stride0: u32) {
        bind(d, m, set_headers(0), 0);
        m.write_u32(STREAM_INPUTS + 4, stride0).unwrap();
        let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
        d.set_vertex_shader_input(m, handle, 3, STREAM_INPUTS)
            .unwrap();
    }
    // Each step changes something and leaves it changed for the next.
    let steps: [fn(&mut Device<RecordingBackend>, &mut SparseMemory); 8] = [
        |d, m| rebind(d, m, 16),
        |d, m| rebind(d, m, 24), // same formats, another stride
        |_, m| {
            word(m, 9, 2, 0x02); // texcoord off
            word(m, 3, 1, 4); // colour at offset 4
        },
        |d, m| rebind(d, m, 32),
        |_, m| word(m, 3, 0, 2), // colour from stream 2
        |d, m| rebind(d, m, 16),
        |d, m| {
            d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE).unwrap();
            d.set_stream_source(m, 0, vb_header(0, 0), 20).unwrap();
        },
        |d, m| {
            word(m, 9, 2, 0x22);
            word(m, 3, 0, 1);
            word(m, 3, 1, 0);
            rebind(d, m, 16);
        },
    ];
    let draw = |d: &mut Device<RecordingBackend>, m: &mut SparseMemory| {
        d.draw_vertices(m, TRIANGLESTRIP, 0, 3).unwrap();
    };
    let (mut warm, mut wm) = setup(RecordingBackend::new());
    for (k, step) in steps.iter().enumerate() {
        step(&mut warm, &mut wm);
        draw(&mut warm, &mut wm);
        let (mut cold, mut cm) = setup(RecordingBackend::new());
        for s in &steps[..=k] {
            s(&mut cold, &mut cm);
        }
        draw(&mut cold, &mut cm);
        let w = draws(std::mem::take(&mut warm.backend.calls))
            .pop()
            .unwrap();
        let c = draws(std::mem::take(&mut cold.backend.calls))
            .pop()
            .unwrap();
        assert_eq!(w.layout, c.layout, "step {k}");
        let strides = |d: &ResolvedDraw| {
            d.streams
                .iter()
                .map(|s| (s.stride, s.data.len()))
                .collect::<Vec<_>>()
        };
        assert_eq!(strides(&w), strides(&c), "step {k}");
        assert_eq!(w.shader.vertex, c.shader.vertex, "step {k}");
    }
}

#[test]
fn prepared_bsp_vertices_follow_the_live_binding_and_writes_between_subdraws() {
    use d3d::device::{StreamSource, declaration_streams};
    use gfx::vertex::VertexLayout;

    let (mut d, mut m) = setup(RecordingBackend::new());
    d.set_vertex_cache(Some(1 << 20));
    let headers = set_headers(0);
    bind(&mut d, &mut m, headers, 0);
    let mut rng = Rng(73);
    for set in 0..2 {
        for s in 0..3 {
            m.write(0x8000_0000 | vb_data(set, s), &rng.bytes(VERTS * STRIDES[s as usize])).unwrap();
        }
    }

    let vaf = d3d::guest::read_array::<64>(&m, VAF).unwrap();
    let layout = VertexLayout {
        streams: declaration_streams(&vaf, |s| STRIDES[s], "synthetic declaration").unwrap(),
        pretransformed: false,
    };
    let sources = (0..3).map(|s| StreamSource { buffer: headers[s], stride: STRIDES[s], offset: 0 }).collect();
    let range = d.prepare_index_range(&m, list(0), 30).unwrap();
    let key = [41, 59, 3];
    let prepared = d.prepare_section_vertices(&m, key, std::slice::from_ref(&range),
        || Some((vaf, layout, sources))).unwrap();
    let (_, _, first) = d.world_vertex_part(&m, &prepared, 0, &range, None).unwrap();
    let prepared = d.prepare_section_vertices(&m, key, std::slice::from_ref(&range),
        || panic!("unchanged section unnecessarily resolved its inputs again")).unwrap();
    assert!(first.iter().all(|s| s.data.cache_id().is_some()));
    let copy_bytes = d.stats().vertex_bytes_copied;
    let (_, _, clean) = d.world_vertex_part(&m, &prepared, 0, &range, None).unwrap();
    assert_eq!(clean[0].data.cache_id(), first[0].data.cache_id());
    assert_eq!(d.stats().vertex_bytes_copied, copy_bytes);

    // Keep the first packet alive, as a threaded backend can. These writes
    // include a page invalidation, a header retarget with unchanged guest
    // object identity, a stream-offset change, and a restore of host caches.
    let (lo, _) = range.window().unwrap();
    let va = (0x8000_0000 | vb_data(0, 0)) + u32::from(lo) * STRIDES[0];
    for step in 0..5 {
        match step {
            0 => {
                m.write_u32(va, 37).unwrap();
                d.note_write(va, 4);
            }
            1 => m.write_u32(headers[0] + 4, vb_data(1, 0)).unwrap(),
            2 => bind(&mut d, &mut m, headers, 8),
            3 => {
                bind(&mut d, &mut m, headers, 0);
                m.write_u32(headers[0] + 4, vb_data(0, 0)).unwrap();
                d.drop_host_caches().unwrap();
                // A restored image replaces the old bytes without reporting
                // each individual store to the old tracker.
                let stream2 = (0x8000_0000 | vb_data(0, 2)) + u32::from(lo) * STRIDES[2];
                m.write_u32(stream2, 78).unwrap();
            }
            _ => {
                let dev = d3d::GuestLayout::HALO2.device;
                m.write_u32(dev + d3d::layout::device_field::BASE_VERTEX, 1).unwrap();
            }
        }
        let (layout, vertex, bytes) = d.world_vertex_part(&m, &prepared, 0, &range, None).unwrap();
        if step == 1 {
            let mut called = false;
            d.prepare_section_vertices(&m, key, std::slice::from_ref(&range), || {
                called = true;
                None
            }).unwrap();
            assert!(called, "a retargeted stream must refresh the section projection");
        }
        if step == 3 { assert_eq!(&bytes[2].data[..4], &78_u32.to_le_bytes()); }
        d.draw_indexed_vertices(&mut m, 5, 30, list(0)).unwrap();
        let serial = d.backend.calls.iter().rev().find_map(|c| match c {
            Recorded::Draw { draw, .. } => Some(draw),
            _ => None,
        }).unwrap();
        assert_eq!(serial.layout, layout);
        assert_eq!(serial.shader.vertex, vertex);
        assert_eq!(serial.streams, bytes, "step {step}");
    }
}
