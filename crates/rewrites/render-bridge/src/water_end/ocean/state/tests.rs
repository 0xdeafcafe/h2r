use super::*;
use crate::water_end::ocean::fake::{Event, Fake, PUSH};
use engine::memory::FlatMemory;

const TABLE: u32 = 0x004f_0000;
const SCRATCH: u32 = 0x0050_9000;
const DEVICE_AT: u32 = 0x004f_8000;

fn mem() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0017_0000);
    m.set_u32(TAGS, TABLE).unwrap();
    m
}

fn tag(m: &mut FlatMemory, n: u32, data: u32) {
    m.set_u32(TABLE + 16 * n + 8, data).unwrap();
}

/// Only the stages whose requested texture differs from the committed one
/// are set, in stage order, and committed.
#[test]
fn the_commit_sets_the_stages_that_changed() {
    let mut m = mem();
    for (s, (req, com)) in [(1, 1), (0, 2), (3, 0), (0, 0)].into_iter().enumerate() {
        m.set_u32(STAGE_REQUESTED + 4 * s as u32, req).unwrap();
        m.set_u32(STAGE_COMMITTED + 4 * s as u32, com).unwrap();
    }
    let host = Fake::default();
    commit_stages(&mut m, &host).unwrap();
    assert_eq!(host.take(), vec![Event::Texture { stage: 1, texture: 0 }, Event::Texture { stage: 2, texture: 3 }]);
    assert_eq!((0..4).map(|s| m.u32(STAGE_COMMITTED + 4 * s).unwrap()).collect::<Vec<_>>(), vec![1, 0, 3, 0]);
}

/// The vertex type's program is loaded and the stream record reset once;
/// the same type and kind again only clears `+0x80`.
#[test]
fn a_vertex_type_loads_its_program_once() {
    let mut m = mem();
    tag(&mut m, 5, 0x0050_6000);
    m.set_u32(0x0050_6000 + 8, 0x0050_6100).unwrap();
    m.set_u32(0x0050_6100 + 0x1c + 0x10, 0x77aa).unwrap();
    m.set_u32(STREAMS + 0xcc, 9).unwrap();
    m.set_u32(STREAMS + 0x2d0, 4).unwrap();
    let host = Fake::default();
    vertex_type(&mut m, &host, 5, 1, 1).unwrap();
    assert_eq!(host.take(), vec![Event::VertexProgram { function: 0x77aa, address: 0 }]);
    assert_eq!((m.u32(STREAMS + 4).unwrap(), m.u32(STREAMS + 8).unwrap()), (5, 1));
    assert_eq!((m.u32(STREAMS + 0xcc).unwrap(), m.u32(STREAMS + 0x2d0).unwrap()), (0, 0));
    assert_eq!((m.u8(STREAMS + 0x88).unwrap(), m.u8(STREAMS + 0x20c).unwrap(), m.u8(STREAMS + 0x2d4).unwrap()), (1, 1, 1));
    m.set_u32(STREAMS + 0x80, 7).unwrap();
    vertex_type(&mut m, &host, 5, 1, 1).unwrap();
    assert!(host.take().is_empty());
    assert_eq!(m.u32(STREAMS + 0x80).unwrap(), 0);
}

/// The kinds of `0x0001cb20`'s table, a built-in type's handle, and a
/// select past the table named.
#[test]
fn the_vertex_kind_follows_the_jump_table() {
    let mut m = mem();
    tag(&mut m, 5, 0x0050_6000);
    m.set_u32(0x0050_6000 + 4, 4).unwrap();
    assert_eq!([0, 1, 2].map(|s| kind(&m, s, 5, 9).unwrap()), [0, 1, 2]);
    // Kind 3: the caller's EAX plus 2 when below the type's count, else 0.
    assert_eq!((kind(&m, 3, 5, 1), kind(&m, 3, 5, 2)), (Ok(3), Ok(0)));
    assert_eq!(kind(&m, 4, 5, 0), Err("vertex_kind"));
    m.set_u32(VERTEX_BUILTIN + 2 * 20, 0x55).unwrap();
    assert_eq!(vertex_handle(&m, 2, u32::MAX), Ok(0x55));
}

/// Each stream fills the next slot; a slot that already holds it and a
/// format already set leave the dirty bytes clear.
#[test]
fn a_stream_fills_the_next_slot() {
    let mut m = mem();
    let at = 0x0050_7000;
    m.write(at, &[2, 3]).unwrap();
    m.set_u32(at + 8, 0x0050_8000).unwrap();
    m.set_u32(at + 0x10, 0x40).unwrap();
    m.set_u32(STREAMS + 0x2d0, 1).unwrap();
    stream(&mut m, at).unwrap();
    let slot = STREAMS + 12;
    assert_eq!([0x210, 0x214, 0x218].map(|o| m.u32(slot + o).unwrap()), [0x40, 3, 0x0050_8000]);
    assert_eq!(m.u32(STREAMS + 4 + 0x8c).unwrap(), 2 * 0x15 + 0x0043_f408);
    assert_eq!((m.u8(STREAMS + 0x2d4).unwrap(), m.u8(STREAMS + 0x20c).unwrap(), m.u32(STREAMS + 0x2d0).unwrap()), (1, 1, 2));
    let mut m2 = mem();
    m2.write(at, &[2, 3]).unwrap();
    m2.set_u32(at + 8, 0x0050_8000).unwrap();
    m2.set_u32(at + 0x10, 0x40).unwrap();
    for (o, v) in [(0x210, 0x40), (0x214, 3), (0x218, 0x0050_8000), (0xcc, 2 * 0x15 + 0x0043_f408)] {
        m2.set_u32(STREAMS + o, v).unwrap();
    }
    stream(&mut m2, at).unwrap();
    assert_eq!((m2.u8(STREAMS + 0x2d4).unwrap(), m2.u8(STREAMS + 0x20c).unwrap()), (0, 0));
}

/// The block is cached and its five dirty words or-ed; the cached block
/// again changes nothing.
#[test]
fn the_state_block_ors_its_masks_once() {
    let mut m = mem();
    for (i, (at, a, b)) in STATE_DIRTY.into_iter().enumerate() {
        m.set_u32(at, 1 << i).unwrap();
        m.set_u32(a, 0x100 << i).unwrap();
        m.set_u32(b, 0x10000 << i).unwrap();
    }
    state_block(&mut m, 0x004c_1a90).unwrap();
    assert_eq!(m.u32(STATE_CACHED).unwrap(), 0x004c_1a90);
    for (i, (at, _, _)) in STATE_DIRTY.into_iter().enumerate() {
        assert_eq!(m.u32(at).unwrap(), 0x10101 << i);
        m.set_u32(at, 0).unwrap();
    }
    state_block(&mut m, 0x004c_1a90).unwrap();
    assert!(STATE_DIRTY.iter().all(|&(at, _, _)| m.u32(at).unwrap() == 0));
}

/// A pending decompression uploads the identity block and clears the
/// flag; none pending uploads nothing; a compressed type is named.
#[test]
fn the_identity_decompression_uploads_when_pending() {
    let mut m = mem();
    let one = 1f32.to_bits();
    m.set_u32(ONE, one).unwrap();
    let host = Fake::default();
    position_decompression(&mut m, &host, SCRATCH, 0, 0).unwrap();
    assert!(host.take().is_empty());
    m.write(DECOMPRESSION_PENDING, &[1]).unwrap();
    position_decompression(&mut m, &host, SCRATCH, 0, 0).unwrap();
    assert_eq!(host.take(), vec![Event::Constants { register: 0xaa, words: vec![one, one, one, one, 0, 0, 0, 0, one, one, 0, 0] }]);
    assert_eq!(m.u8(DECOMPRESSION_PENDING).unwrap(), 0);
    assert_eq!(position_decompression(&mut m, &host, SCRATCH, 0x0050_0000, 1), Err("position_compression"));
}

/// The grid's twelve words go to `c[0x32]` and the word after is cleared.
#[test]
fn the_grid_constants_upload_twelve_words() {
    let mut m = mem();
    m.set_u32(GRID_CONSTANTS, 7).unwrap();
    m.set_u32(GRID_CLEARED, 9).unwrap();
    let host = Fake::default();
    grid_constants(&mut m, &host).unwrap();
    let mut words = vec![0; 12];
    words[0] = 7;
    assert_eq!(host.take(), vec![Event::Constants { register: 0x32, words }]);
    assert_eq!(m.u32(GRID_CLEARED).unwrap(), 0);
}

/// Row 0 of the projection for small integers (the scale 2 / 1):
/// `(7 * 2 + 3 * 11 + 13 * 5) * 2` and the translation `4 * 2`.
#[test]
fn the_projection_is_the_sse_sums() {
    let mut m = mem();
    let cam = 0x0050_a000;
    let f = |m: &mut FlatMemory, at: u32, v: f32| m.set_u32(at, v.to_bits()).unwrap();
    f(&mut m, PROJECTION_NUMERATOR, 2.0);
    f(&mut m, PROJECTION_DIVISOR, 1.0);
    for (o, v) in [(0xc, 2.0), (4, 3.0), (8, 5.0), (0x98, 7.0), (0x78, 11.0), (0x88, 13.0), (0xa8, 4.0)] {
        f(&mut m, cam + o, v);
    }
    let rows = projection(&m, cam).unwrap();
    assert_eq!(rows[..4], [224.0, 0.0, 0.0, 8.0]);
    assert!(rows[4..].iter().all(|&v| v == 0.0));
}

/// The viewport's device words and push; the projection uploaded to
/// `c[0..4]` only while the cached byte is set.
#[test]
fn the_viewport_pushes_and_uploads_the_projection_once() {
    let mut m = mem();
    m.set_u32(DEVICE, DEVICE_AT).unwrap();
    m.set_u32(DEVICE_AT, 0x100).unwrap();
    m.set_u32(DEVICE_AT + 4, 0x100).unwrap();
    m.set_u32(DEVICE_AT + DEVICE_VIEWPORT_FLAGS, 0x10).unwrap();
    m.set_u32(ONE, 1f32.to_bits()).unwrap();
    m.set_u32(Z_SCALE, 3f32.to_bits()).unwrap();
    m.set_u32(Z_OFFSET, 0x1234).unwrap();
    m.set_u32(PROJECTION_DIVISOR, 1f32.to_bits()).unwrap();
    m.write(PROJECTION_CACHED, &[1]).unwrap();
    let host = Fake::default();
    viewport(&mut m, &host, SCRATCH).unwrap();
    let events = host.take();
    // The put at the limit makes room first (the fake's put 0).
    assert_eq!(events[..2], [Event::MakeSpace, Event::PushViewport { put: 0 }]);
    assert!(matches!(&events[2], Event::Registers { register: 0, words } if words.len() == 16));
    assert_eq!(m.u32(DEVICE_AT).unwrap(), PUSH);
    assert_eq!(m.u32(DEVICE_AT + DEVICE_Z_SCALE).unwrap(), 3f32.to_bits());
    assert_eq!(m.u32(DEVICE_AT + DEVICE_Z_OFFSET).unwrap(), 0x1234);
    assert_eq!(m.u32(DEVICE_AT + DEVICE_VIEWPORT_FLAGS).unwrap(), 0x12);
    assert_eq!(m.u8(PROJECTION_CACHED).unwrap(), 0);
    m.set_u32(DEVICE_AT + 4, 0x1000).unwrap();
    viewport(&mut m, &host, SCRATCH).unwrap();
    assert_eq!(host.take(), vec![Event::PushViewport { put: PUSH }]);
}

/// The structure's calls are the upload's words in the guest's order.
#[test]
fn the_structure_upload_is_made_in_the_guests_order() {
    let mut m = mem();
    let block = 0x0050_b000;
    let up = 0x0050_c000;
    for (at, v) in [
        (ONE, 1.0f32),
        (0x0045_dbdc, 1e-4),
        (0x0045_dbbc, 0.5),
        (0x0045_dbcc, 0.25),
        (0x0045_e458, 2.0),
        (0x0045_dc38, 255.0),
        (up + 8, 1.0),
        (0x004b_9db0, 1.0),
        (block + 0x68, 2.0),
        (block + 0x6c, 3.0),
        (block + 0x70, 4.0),
        (block + 0x90, 7.0),
    ] {
        m.set_u32(at, v.to_bits()).unwrap();
    }
    m.set_u32(0x0046_87b0, up).unwrap();
    m.write(0x0045_e450, &0.5f64.to_le_bytes()).unwrap();
    let u = structure_upload::upload(&m, block, FCW, MXCSR).unwrap();
    let host = Fake::default();
    structure(&mut m, &host, SCRATCH, block).unwrap();
    let mut want = Vec::new();
    for (s, words) in (1..).zip(u.bump_env) {
        for (state, value) in BUMP_STATES.into_iter().zip(words) {
            want.push(Event::StageState { stage: s, state, value });
        }
    }
    let reg = |i: usize| Event::Registers { register: 21 + i as u32, words: u.constants[i].to_vec() };
    want.extend([reg(0), reg(2), reg(3), Event::RenderState { slot: crate::view_setup::SLOT_FOG_COLOUR, value: u.fog_colour }, reg(1), reg(4)]);
    assert_eq!(host.take(), want);
    let scratch: Vec<u32> = (0..13).map(|i| m.u32(structure_upload::SCRATCH + 4 * i).unwrap()).collect();
    assert_eq!(scratch, u.scratch);
}

const RECORD: u32 = 0x0050_d000;
const FORMAT: u32 = 0x0050_e000;

/// A record whose slot 0 is the built-in vertex type, one stream pending
/// `FORMAT`: semantic 0 as three floats, a skip of 4, semantic 3 as a
/// colour, semantic 5 (no register) as two shorts, then the end.
fn record(m: &mut FlatMemory) {
    m.set_u32(RECORD + 4, u32::MAX).unwrap();
    m.set_u32(RECORD + RECORD_FORMATS, FORMAT).unwrap();
    m.set_u32(RECORD + RECORD_COUNT, 1).unwrap();
    m.set_u32(RECORD + RECORD_PROGRAM, 0x31).unwrap();
    m.write(FORMAT, &[0, 0, 2, 0xfe, 4, 3, 17, 5, 9, 14, 3, 0xff, 0]).unwrap();
    m.write(RECORD + RECORD_VAF_DIRTY, &[1]).unwrap();
}

/// The format is built by the vertex type's registers in stream order,
/// the pending format moved to the last. Rebuilding it alone makes no
/// call (`0x0001c330` clears its dirty byte); dirty inputs are set
/// through the static shader with the format copied in, then a dirty
/// program selected.
#[test]
fn the_stream_commit_builds_the_format_then_sets_the_inputs_and_program() {
    let mut m = mem();
    record(&mut m);
    let host = Fake::default();
    stream_commit(&mut m, &host, RECORD).unwrap();
    assert!(host.take().is_empty());
    let vaf = RECORD + RECORD_VAF;
    let reg = |m: &FlatMemory, r: u32| [0, 4, 8].map(|o| m.u32(vaf + 16 * r + o).unwrap());
    // Semantic 0 to register 0 (float3, 0x32); 3 to 3 at 16 (colour,
    // 0x40); 5 (no register) skips two shorts; 14 to 9 at 24 (float4,
    // 0x42).
    assert_eq!(reg(&m, 0), [0, 0, 0x32]);
    assert_eq!(reg(&m, 3), [0, 16, 0x40]);
    assert_eq!(reg(&m, 9), [0, 24, 0x42]);
    assert_eq!(reg(&m, 1), [0, 0, 2], "an unused register is D3DVSDT_NONE");
    assert_eq!(m.u32(RECORD + RECORD_LAST_FORMATS).unwrap(), FORMAT);
    assert_eq!(m.u32(RECORD + RECORD_FORMATS).unwrap(), 0);
    assert_eq!(m.u32(RECORD + RECORD_COUNT).unwrap(), 0);
    m.set_u32(RECORD + RECORD_COUNT, 1).unwrap();
    m.write(RECORD + RECORD_INPUTS_DIRTY, &[1]).unwrap();
    m.write(RECORD + RECORD_PROGRAM_DIRTY, &[1]).unwrap();
    stream_commit(&mut m, &host, RECORD).unwrap();
    assert_eq!(host.take(), vec![
        Event::VertexInput { handle: STATIC_SHADER | 1, count: 1, inputs: RECORD + RECORD_INPUTS },
        Event::SelectProgram { vaf, address: 0x31 },
    ]);
    assert_eq!(m.u32(STATIC_SHADER + 0x14 + 16 * 3 + 4).unwrap(), 16);
    assert_eq!(m.u32(RECORD + RECORD_COUNT).unwrap(), 0);
    assert_eq!((m.u8(RECORD + RECORD_INPUTS_DIRTY).unwrap(), m.u8(RECORD + RECORD_PROGRAM_DIRTY).unwrap()), (0, 0));
    // Nothing dirty: no calls, the count still cleared.
    m.set_u32(RECORD + RECORD_COUNT, 1).unwrap();
    stream_commit(&mut m, &host, RECORD).unwrap();
    assert!(host.take().is_empty());
    assert_eq!(m.u32(RECORD + RECORD_COUNT).unwrap(), 0);
}

/// Inputs dirty with no streams set none; a dirty program alone selects.
#[test]
fn the_stream_commit_without_streams_sets_no_inputs() {
    let mut m = mem();
    m.write(RECORD + RECORD_INPUTS_DIRTY, &[1]).unwrap();
    m.write(RECORD + RECORD_PROGRAM_DIRTY, &[1]).unwrap();
    let host = Fake::default();
    stream_commit(&mut m, &host, RECORD).unwrap();
    assert_eq!(host.take(), vec![
        Event::VertexInput { handle: 0, count: 0, inputs: 0 },
        Event::SelectProgram { vaf: RECORD + RECORD_VAF, address: 0 },
    ]);
    assert_eq!((m.u8(RECORD + RECORD_INPUTS_DIRTY).unwrap(), m.u8(RECORD + RECORD_PROGRAM_DIRTY).unwrap()), (0, 0));
}

/// A tag's vertex type maps its kind's semantics in list order; a
/// semantic past the table is named.
#[test]
fn a_tags_vertex_type_maps_its_semantics() {
    let mut m = mem();
    record(&mut m);
    let (data, kinds, list) = (0x0050_f000, 0x0050_f100, 0x0050_f200);
    tag(&mut m, 6, data);
    m.set_u32(RECORD + 4, 6).unwrap();
    m.set_u32(RECORD + 8, 1).unwrap();
    m.set_u32(data + 8, kinds).unwrap();
    m.set_u32(kinds + 0x1c + 4, 2).unwrap();
    m.set_u32(kinds + 0x1c + 8, list).unwrap();
    // Semantic 3 to register 0, semantic 0 to register 1.
    m.set_u32(list, 3).unwrap();
    declaration(&mut m, RECORD, RECORD + RECORD_VAF).unwrap();
    let vaf = RECORD + RECORD_VAF;
    assert_eq!((m.u32(vaf + 4).unwrap(), m.u32(vaf + 8).unwrap()), (16, 0x40));
    assert_eq!((m.u32(vaf + 16 + 4).unwrap(), m.u32(vaf + 16 + 8).unwrap()), (0, 0x32));
    m.set_u32(RECORD + RECORD_FORMATS, FORMAT).unwrap();
    m.set_u32(list, 21).unwrap();
    assert_eq!(declaration(&mut m, RECORD, RECORD + RECORD_VAF), Err("vertex_declaration"));
}
