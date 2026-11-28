use std::sync::Mutex;

use guest_layout::memory::FlatMemory;
use scene_extract::layout::decorators::{
    CACHE_DATA, CACHE_ENTRIES, CACHE_ENTRY_SIZE, CACHE_ENTRY_STAMP, CACHE_STAMP, GEOMETRY_CACHE,
    TAG_TABLE,
};

use super::*;

const ESP: u32 = 0x0058_0000;
const ENTRY: u32 = 0x0058_0100;
const IMPLEMENTATION: u32 = 0x0059_0000;
const BSP: u32 = 0x0057_0000;
const BLOCK: u32 = 0x0057_0100;
const PLACEMENTS: u32 = 0x0057_0200;
const RECORDS: u32 = 0x0057_0400;
const SCEN: u32 = 0x0057_0800;
const PALETTE: u32 = 0x0057_0900;
const TAGS: u32 = 0x0057_0a00;
const SET: u32 = 0x0057_0c00;
const CLASSES: u32 = 0x0057_0d00;
const GEOMETRY: u32 = 0x0057_0e00;
const BUFFER: u32 = 0x0057_0f00;
const CACHE: u32 = 0x0057_1000;
const CACHE_DATA_AT: u32 = 0x0057_1100;
const CACHE_ENTRIES_AT: u32 = 0x0057_1200;
const SHADER: u32 = 0xe3c0_0011;
/// Placement 2 at detail level 1.
const ARGUMENT: u32 = 1 << 30 | 2 << 8 | 7;
const STAMP_AT: u32 = CACHE_ENTRIES_AT + 5 * CACHE_ENTRY_SIZE + CACHE_ENTRY_STAMP;

/// One quad-list placement (index 2: palette 1, class 1, geometry record
/// 3, vertices 40 to 47), its record resident in cache entry 5, its
/// block's stream type 7, stride 24, offset 0x100 and data `0x80123000`.
fn mem() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
    m.set_u32(INIT_FLAGS, 3).unwrap();
    m.set_u32(ACTIVE_BSP, BSP).unwrap();
    m.set_u32(BSP + BSP_DECORATORS, BLOCK).unwrap();
    m.set_u32(BLOCK + BLOCK_PLACEMENTS, PLACEMENTS).unwrap();
    m.set_u32(BLOCK + BLOCK_GEOMETRY, RECORDS).unwrap();
    let placement = PLACEMENTS + 2 * PLACEMENT_SIZE;
    m.set_u8(placement, 1).unwrap();
    m.set_u8(placement + 1, 1).unwrap();
    m.set_u16(placement + 6, 3).unwrap();
    m.set_u16(placement + PLACEMENT_START, 40).unwrap();
    m.set_u16(placement + PLACEMENT_COUNT, 8).unwrap();
    let record = RECORDS + 3 * GEOMETRY_SIZE;
    m.set_u8(record + 0x1e, 1).unwrap();
    m.set_u16(record + 0x20, 5).unwrap();
    m.set_u32(record + RECORD_BLOCK, GEOMETRY).unwrap();
    m.set_u8(GEOMETRY + BLOCK_STREAM, 7).unwrap();
    m.set_u8(GEOMETRY + BLOCK_STREAM + 1, 24).unwrap();
    m.set_u32(GEOMETRY + BLOCK_STREAM + 8, 0x100).unwrap();
    m.set_u32(GEOMETRY + BLOCK_STREAM + 0x10, BUFFER).unwrap();
    m.set_u32(BUFFER + 4, 0xf012_3000).unwrap();
    m.set_u32(SCENARIO, SCEN).unwrap();
    m.set_u32(SCEN + SCENARIO_PALETTE, PALETTE).unwrap();
    m.set_u32(PALETTE + 8 + 4, 0xe1f0_0004).unwrap();
    m.set_u32(TAG_TABLE, TAGS).unwrap();
    m.set_u32(TAGS + 4 * 0x10 + 8, SET).unwrap();
    m.set_u32(SET + SET_CLASSES, CLASSES).unwrap();
    m.set_u8(CLASSES + CLASS_SIZE + CLASS_TYPE, 1).unwrap();
    m.set_u32(GEOMETRY_CACHE, CACHE).unwrap();
    m.set_u32(CACHE + CACHE_STAMP, 0x77).unwrap();
    m.set_u32(CACHE + CACHE_DATA, CACHE_DATA_AT).unwrap();
    m.set_u32(CACHE_DATA_AT + CACHE_ENTRIES, CACHE_ENTRIES_AT)
        .unwrap();
    m.set_f32(LOD_FACTORS[0] + 4, 1.5).unwrap();
    m.set_f32(LOD_FACTORS[1] + 4, 2.5).unwrap();
    m.set_f32(C53_W, 1.0).unwrap();
    for (n, w) in [SHADER, 0, 0, 15, 1, ARGUMENT, ENTRY]
        .into_iter()
        .enumerate()
    {
        m.set_u32(ESP + 4 + 4 * n as u32, w).unwrap();
    }
    m.set_u8(ENTRY + ENTRY_IMPLEMENTATION_INDEX, 0).unwrap();
    m.set_u32(ENTRY + ENTRY_IMPLEMENTATION, IMPLEMENTATION)
        .unwrap();
    m
}

fn key() -> Key {
    Key {
        args: [SHADER, 0, 0, 15, 1],
        implementation: (0, IMPLEMENTATION),
        block: GEOMETRY,
        kind: Kind::Quads,
        stream: Stream {
            kind: 7,
            stride: 24,
            offset: 0x100,
            buffer: BUFFER,
            data: 0x8012_3000,
        },
        c53: [1.5f32.to_bits(), 2.5f32.to_bits(), 0, 1.0f32.to_bits()],
    }
}

#[test]
fn a_call_reads_as_its_body_reads_it() {
    let m = mem();
    let call = read_call(&m, ESP).unwrap();
    assert_eq!(call.key, Some(key()));
    assert_eq!((call.start, call.count, call.class_type), (40, 8, 1));
    assert_eq!(call.stamp, (STAMP_AT, 0x77));
    // The placement's vertices: from the stream's data and offset.
    let mut v = FlatMemory::new(0x8012_3000, 0x2000);
    let at = 0x8012_3000 + 0x100 + 40 * 24;
    v.set_u32(at, 0xabcd).unwrap();
    let bytes = call.vertices(&v).unwrap();
    assert_eq!(bytes.len(), 8 * 24);
    assert_eq!(&bytes[..4], &0xabcd_u32.to_le_bytes());
    assert_eq!(call.vertices(&m), Err("vertices_unread"));
}

#[test]
fn a_triangle_class_draws_a_list_and_other_types_are_not_keyed() {
    let mut m = mem();
    m.set_u8(CLASSES + CLASS_SIZE + CLASS_TYPE, 2).unwrap();
    assert_eq!(
        read_call(&m, ESP).unwrap().key.map(|k| k.kind),
        Some(Kind::Triangles)
    );
    m.set_u8(CLASSES + CLASS_SIZE + CLASS_TYPE, 3).unwrap();
    let call = read_call(&m, ESP).unwrap();
    assert_eq!((call.key, call.class_type), (None, 3));
}

#[test]
fn the_body_runs_before_its_constants_or_for_a_record_not_resident() {
    let mut m = mem();
    m.set_u32(INIT_FLAGS, 1).unwrap();
    assert_eq!(read_call(&m, ESP), Err("init"));
    let mut m = mem();
    m.set_u8(RECORDS + 3 * GEOMETRY_SIZE + 0x1e, 0).unwrap();
    assert_eq!(read_call(&m, ESP), Err("nonresident"));
}

/// Draws every call the template holds for, or refuses.
#[derive(Default)]
struct Draws {
    frame: Mutex<u64>,
    holds: Mutex<Option<&'static str>>,
    drawn: Mutex<Vec<(u32, u32)>>,
}

impl Host for Draws {
    fn frame(&self) -> u64 {
        *self.frame.lock().unwrap()
    }
    fn begin_body(&self, _: &mut dyn GuestMemory, _: &Call, _: bool) {}
    fn end_body(&self, _: &mut dyn GuestMemory, _: &Call) -> Body {
        Body {
            native: Ok(()),
            differs: Vec::new(),
        }
    }
    fn holds(&self, _: &dyn GuestMemory, _: &Call) -> Result<(), &'static str> {
        self.holds.lock().unwrap().map_or(Ok(()), Err)
    }
    fn draw(&self, _: &mut dyn GuestMemory, call: &Call) -> Result<(), &'static str> {
        self.drawn.lock().unwrap().push((call.start, call.count));
        Ok(())
    }
}

fn row(state: &State, name: &str) -> u64 {
    state
        .counters()
        .into_iter()
        .find(|(n, _)| n == name)
        .map_or(0, |(_, v)| v)
}

#[test]
fn a_call_returns_without_its_body_once_a_frame_has_proven_the_stage() {
    let state = State::new();
    let host = Draws::default();
    let mut m = mem();
    // Frame 1: unproven, the body runs and is drawn natively.
    *host.frame.lock().unwrap() = 1;
    let (turn, call) = state.turn(&mut m, ESP, Some(&host), 9);
    assert_eq!(turn, Turn::Body { sampled: false });
    state.record(&host.end_body(&mut m, call.as_ref().unwrap()), false);
    assert_eq!(row(&state, "scene_kept_decal_decorator_unproven"), 1);
    assert_eq!(
        m.u32(STAMP_AT).unwrap(),
        0,
        "the body writes the stamp, not the cut"
    );
    // Frame 2: proven; the call is drawn from the template and returns.
    *host.frame.lock().unwrap() = 2;
    let (turn, _) = state.turn(&mut m, ESP, Some(&host), 9);
    assert_eq!(turn, Turn::Return);
    assert_eq!(*host.drawn.lock().unwrap(), [(40, 8)]);
    assert_eq!(
        m.u32(STAMP_AT).unwrap(),
        0x77,
        "the cut writes the body's stamp"
    );
    assert_eq!(row(&state, "decal_decorator_returned"), 1);
    assert_eq!(row(&state, "decal_decorator_proven"), 1);
    // A template that does not hold runs the body, by its cause.
    *host.holds.lock().unwrap() = Some("state");
    assert_eq!(
        state.turn(&mut m, ESP, Some(&host), 9).0,
        Turn::Body { sampled: false }
    );
    assert_eq!(row(&state, "scene_kept_decal_decorator_state"), 1);
    // A new map forgets the proof.
    *host.holds.lock().unwrap() = None;
    assert_eq!(
        state.turn(&mut m, ESP, Some(&host), 10).0,
        Turn::Body { sampled: false }
    );
    assert_eq!(row(&state, "decal_decorator_proven"), 0);
}

#[test]
fn a_frame_with_a_body_drawn_by_d3d_proves_nothing() {
    let state = State::new();
    let host = Draws::default();
    let mut m = mem();
    *host.frame.lock().unwrap() = 1;
    let _ = state.turn(&mut m, ESP, Some(&host), 1);
    state.record(
        &Body {
            native: Err("admission_topology"),
            differs: Vec::new(),
        },
        false,
    );
    *host.frame.lock().unwrap() = 2;
    assert_eq!(
        state.turn(&mut m, ESP, Some(&host), 1).0,
        Turn::Body { sampled: false }
    );
    assert_eq!(
        row(&state, "decal_decorator_guest_d3d_admission_topology"),
        1
    );
    assert_eq!(row(&state, "scene_kept_decal_decorator_unproven"), 2);
    // No host: every keyed call runs its body.
    assert_eq!(
        state.turn(&mut m, ESP, None, 1).0,
        Turn::Body { sampled: false }
    );
    assert_eq!(row(&state, "scene_kept_decal_decorator_host"), 1);
}

#[test]
fn one_eligible_call_in_sample_runs_its_body_and_a_difference_ends_the_cut() {
    let state = State::new();
    let host = Draws::default();
    let mut m = mem();
    *host.frame.lock().unwrap() = 1;
    let _ = state.turn(&mut m, ESP, Some(&host), 1);
    state.record(
        &Body {
            native: Ok(()),
            differs: Vec::new(),
        },
        false,
    );
    *host.frame.lock().unwrap() = 2;
    let turns: Vec<_> = (0..SAMPLE)
        .map(|_| state.turn(&mut m, ESP, Some(&host), 1).0)
        .collect();
    assert!(
        turns[..SAMPLE as usize - 1]
            .iter()
            .all(|t| *t == Turn::Return)
    );
    assert_eq!(turns[SAMPLE as usize - 1], Turn::Body { sampled: true });
    state.record(
        &Body {
            native: Ok(()),
            differs: vec!["constants".into()],
        },
        true,
    );
    assert!(state.fallen_back());
    assert_eq!(row(&state, "decal_decorator_mismatch_constants"), 1);
    assert_eq!(row(&state, "decal_decorator_checked"), 1);
    assert_eq!(
        state.turn(&mut m, ESP, Some(&host), 1).0,
        Turn::Body { sampled: false }
    );
    assert_eq!(row(&state, "scene_kept_decal_decorator_fallback"), 1);
}

#[test]
fn a_block_with_no_buffer_returns_with_its_stamp_and_no_draw() {
    let state = State::new();
    let host = Draws::default();
    let mut m = mem();
    m.set_u32(GEOMETRY + BLOCK_STREAM + 0x10, 0).unwrap();
    assert_eq!(state.turn(&mut m, ESP, Some(&host), 1).0, Turn::Return);
    assert_eq!(m.u32(STAMP_AT).unwrap(), 0x77);
    assert!(host.drawn.lock().unwrap().is_empty());
    assert_eq!(row(&state, "decal_decorator_empty"), 1);
    // A class type with no draw keeps its body (it still begins the shader).
    m.set_u32(GEOMETRY + BLOCK_STREAM + 0x10, BUFFER).unwrap();
    m.set_u8(CLASSES + CLASS_SIZE + CLASS_TYPE, 0).unwrap();
    assert_eq!(
        state.turn(&mut m, ESP, Some(&host), 1).0,
        Turn::Body { sampled: false }
    );
    assert_eq!(row(&state, "scene_kept_decal_decorator_type"), 1);
}
