//! The layer draw's return (`render_bridge::frame_step::layer`) against a
//! model of the guest body `0x00044550` read from the image
//! (docs/graphics/layer-draw.md, "The read"): from the same seeded state,
//! the words after a return equal the words the body leaves, and where the
//! body would call a function (an item's build, the cache's restore
//! `0x00018900`) the frame step runs the guest instead and writes nothing.

use guest_layout::memory::{FlatMemory, GuestMemory};
use render_bridge::frame_step::layer::{self, Keep};

const BASE: u32 = 0x0045_0000;
const LEN: usize = 0x7_0000;
const LIST: u32 = 0x004a_0000;
const ITEMS: u32 = 0x004a_0100;
/// The layers `0x00044550` draws (`0x00bffdee`).
const DRAWN_MASK: u32 = 0x00bf_fdee;

/// The guest body, as read: `Err` names the call it would make, after
/// which the model cannot say what it writes.
fn guest(m: &mut FlatMemory) -> Result<(), &'static str> {
    let current = m.u32(layer::CURRENT_LAYER).unwrap();
    m.set_u32(layer::DRAWN_LAYER, current).unwrap();
    m.set_u8(layer::DRAW_FLAG, 1).unwrap();
    // 0x00016b10, ESI = 0x00485b48.
    let s = layer::SETUP;
    if m.u32(s + 0x24).unwrap() != 0 {
        return Err("0x00018900");
    }
    let f = m.u32(layer::SETUP_FLOAT).unwrap();
    for at in [0x0c, 0x10, 0x14, 0x18, 0x1c, 0x20, 0x24] {
        m.set_u32(s + at, 0).unwrap();
    }
    m.set_u32(s, u32::MAX).unwrap();
    m.set_u32(s + 0x40, u32::MAX).unwrap();
    for at in [0x1320, 0x1360, 0x13a0, 0x13e0] {
        m.set_u32(s + at, u32::MAX).unwrap();
        m.set_u32(s + at + 0x10, f).unwrap();
    }
    let bit = 1u32 << (current & 31);
    // Layer 16 draws only when the fog mode is 3 or byte 1 or 2 of
    // `0x00485a74` is set (`0x00044582`).
    let fog = m.u32(0x0048_574c).unwrap() == 3 || m.u32(0x0048_5a74).unwrap() & 0x00ff_ff00 != 0;
    if bit & DRAWN_MASK != 0 && (current != 16 || fog) {
        // 0x0003bc30 → 0x0003bab0: every item of `first..count` whose mask
        // has the bit is built; none, no entry, no handler.
        let list = m.u32(layer::CURRENT_LIST).unwrap();
        let count = m.u32(list + 4).unwrap() as i32;
        let items = m.u32(list + 0x14).unwrap();
        let mut at = m.u32(list + 0xc).unwrap() as i32;
        while at < count {
            if m.u32(items + at as u32 * 0x18 + 0x10).unwrap() & bit != 0 {
                return Err("the item's build");
            }
            at += 1;
        }
    }
    m.set_u32(layer::DRAWN_LAYER, u32::MAX).unwrap();
    Ok(())
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n)) as u32
    }
}

/// A seeded state: noise in every word the draw touches, the current layer
/// one of the opaque block's, a list of up to 40 items. `keep_bit`: how
/// often an item keeps the layer's bit (0 never).
fn state(seed: u64, keep_bit: u32) -> (FlatMemory, u32) {
    let mut r = Rng(seed);
    let mut m = FlatMemory::new(BASE, LEN);
    for b in m.bytes.iter_mut() {
        *b = r.next() as u8;
    }
    // The opaque block's layers, the light passes' and shadow layers' and
    // R5's late, fog and water layers share the body.
    let layers: Vec<u32> = layer::LAYERS
        .iter()
        .chain(&layer::LIGHT_LAYERS)
        .chain(&layer::LATE_LAYERS)
        .chain(&layer::FOG_WATER_LAYERS)
        .copied()
        .collect();
    let layer = layers[r.below(layers.len() as u32) as usize];
    m.set_u32(layer::CURRENT_LAYER, layer).unwrap();
    m.set_u32(layer::CURRENT_LIST, LIST).unwrap();
    let count = r.below(41);
    let first = r.below(count + 1);
    m.set_u32(LIST + 4, count).unwrap();
    m.set_u32(LIST + 0xc, first).unwrap();
    m.set_u32(LIST + 0x14, ITEMS).unwrap();
    for i in 0..count {
        let item = ITEMS + i * 0x18;
        let mut mask = m.u32(item + 0x10).unwrap() & !(1 << layer);
        if keep_bit != 0 && r.below(keep_bit) == 0 {
            mask |= 1 << layer;
        }
        m.set_u32(item + 0x10, mask).unwrap();
        let build = if r.below(3) == 0 { 0x0004_d000 } else { layer::GEOMETRY_BUILD };
        m.set_u32(item, build).unwrap();
        let index = r.below(64);
        m.set_u32(item + 0xc, index << 8 | r.below(256)).unwrap();
        let owner = if r.below(2) == 0 { u32::MAX } else { 0xe000_0000 | index };
        m.set_u32(layer::RENDER_RECORDS + index * 0x20 + 4, owner).unwrap();
    }
    // Most states have the cache closed, as after a layer that returned.
    if r.below(4) != 0 {
        m.set_u32(layer::SETUP + layer::SETUP_OPEN, 0).unwrap();
    }
    // Half the fog states have the gate off: no fog drawn.
    if r.below(2) == 0 {
        m.set_u32(layer::FOG_MODE, 0).unwrap();
        m.set_u32(layer::FOG_BYTES, 0).unwrap();
    }
    (m, layer)
}

#[test]
fn a_returned_layer_draw_leaves_the_words_the_guest_body_leaves() {
    let (mut returned, mut lights, mut late) = (0, 0, 0);
    for seed in 0..2_000 {
        let (start, layer) = state(seed, 0);
        let mut model = FlatMemory { base: start.base, bytes: start.bytes.clone() };
        let mut ours = FlatMemory { base: start.base, bytes: start.bytes.clone() };
        let want = guest(&mut model);
        let got = layer::step(&mut ours, layer);
        match want {
            Ok(()) => {
                assert_eq!(got, Ok(()), "seed {seed}: the body builds nothing, so the draw returns");
                assert!(model.bytes == ours.bytes, "seed {seed}: the words differ from the body's");
                returned += 1;
                lights += u32::from(layer::LIGHT_LAYERS.contains(&layer));
                late += u32::from(layer::LATE_LAYERS.contains(&layer));
            }
            Err(call) => {
                assert_eq!(got, Err(Keep::OpenShader), "seed {seed}: the body calls {call}");
                assert!(ours.bytes == start.bytes, "seed {seed}: a kept draw writes nothing");
            }
        }
    }
    assert!(returned > 1_000, "most seeds return: {returned}");
    assert!(lights > 200, "the light passes' and shadow layers' too: {lights}");
    assert!(late > 200, "R5's late layers too: {late}");
}

#[test]
fn an_item_still_holding_the_bit_runs_the_guest_body_by_its_kind() {
    let mut kinds = [0u32; 3];
    for seed in 0..2_000 {
        let (start, layer) = state(seed, 4);
        let mut model = FlatMemory { base: start.base, bytes: start.bytes.clone() };
        let mut ours = FlatMemory { base: start.base, bytes: start.bytes.clone() };
        let want = guest(&mut model);
        let got = layer::step(&mut ours, layer);
        match (want, got) {
            (Ok(()), Ok(())) => assert!(model.bytes == ours.bytes, "seed {seed}"),
            (Err(_), Err(keep)) => {
                assert!(ours.bytes == start.bytes, "seed {seed}: a kept draw writes nothing");
                match keep {
                    Keep::Structure => kinds[0] += 1,
                    Keep::Object => kinds[1] += 1,
                    Keep::Other => kinds[2] += 1,
                    Keep::OpenShader => {}
                    k => panic!("seed {seed}: {k:?}"),
                }
            }
            (w, g) => panic!("seed {seed}: body {w:?}, frame step {g:?}"),
        }
    }
    assert!(kinds.iter().all(|&n| n > 0), "every kind kept: {kinds:?}");
}

#[test]
fn a_list_that_does_not_read_runs_the_guest_body() {
    let (mut m, layer) = state(7, 0);
    m.set_u32(layer::SETUP + layer::SETUP_OPEN, 0).unwrap();
    m.set_u32(layer::CURRENT_LIST, 0x0010_0000).unwrap();
    let before = m.bytes.clone();
    assert_eq!(layer::step(&mut m, layer), Err(Keep::Unread));
    assert!(m.bytes == before);
    m.set_u32(layer::CURRENT_LIST, 0).unwrap();
    assert_eq!(layer::keep(&m, layer), Err(Keep::Unread), "no list");
}

/// Layer 16 with its gate off builds no entry whatever its items' bits,
/// so its draw returns with them held; with the gate on an item held runs
/// the body.
#[test]
fn a_fog_layer_with_its_gate_off_returns_with_its_items_held() {
    let mut off = 0;
    for seed in 0..2_000 {
        let (start, current) = state(seed, 2);
        if current != layer::FOG_LAYER {
            continue;
        }
        let mut model = FlatMemory { base: start.base, bytes: start.bytes.clone() };
        let mut ours = FlatMemory { base: start.base, bytes: start.bytes.clone() };
        let want = guest(&mut model);
        let got = layer::step(&mut ours, current);
        let gate = layer::builds(&start, current).unwrap();
        match want {
            Ok(()) => {
                assert_eq!(got, Ok(()), "seed {seed}");
                assert!(model.bytes == ours.bytes, "seed {seed}");
                if !gate {
                    assert!(layer::held(&start, current).unwrap().is_empty(), "seed {seed}: the gate off holds nothing");
                }
                off += u32::from(!gate);
            }
            Err(_) => assert!(got.is_err() && ours.bytes == start.bytes, "seed {seed}"),
        }
    }
    assert!(off > 20, "gated fog returns: {off}");
}
