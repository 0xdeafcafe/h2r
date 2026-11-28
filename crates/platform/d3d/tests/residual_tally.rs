//! The device's tally for `render.residual` (docs/graphics/retire-d3d.md,
//! slice R0) on the draw rig, split the way the adapter's watch splits an
//! entry call: what its drain decoded, and what the entry did.

use crate::common::*;
use d3d::residual::Watch;
use d3d::{RecordingBackend, UploadPolicy};

#[test]
fn inline_draws_are_the_drains_and_the_entrys_own_are_its_body() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xFFFF_0000);
    bind_quad(&mut d, &mut m);
    let words_before = d.stats().push_words;
    // Game code writes two inline quads, then calls DrawVertices: the
    // adapter drains before the entry runs.
    inline_quad(&mut m, 0.0, 0.0, 8.0, 8.0, [255, 0, 0, 255]);
    inline_quad(&mut m, 8.0, 8.0, 16.0, 16.0, [0, 255, 0, 255]);
    let mut w = Watch::start(0x0004_c976, d.residual_tally());
    assert_eq!(d.drain_until_callback(&mut m).unwrap(), None);
    w.drained(d.residual_tally());
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let call = w.finish(0, "D3DDevice_DrawVertices", d.residual_tally(), true);
    assert_eq!(call.ret, 0x0004_c976);
    assert_eq!((call.inline.draws, call.inline.residual), (2, 0));
    assert_eq!(
        (call.own.draws, call.own.residual, call.own.claimed),
        (1, 0, 0)
    );
    assert!(call.inline.push_words > 0, "{call:?}");
    // The words are what the device decoded; no draw reaches the backend.
    assert_eq!(
        call.inline.push_words + call.own.push_words,
        d.stats().push_words - words_before
    );
    assert_eq!(d.stats().backend_draws, 0);
}

#[test]
fn the_deferred_commit_counts_the_state_words_it_applies() {
    let (mut d, mut m) = create(RecordingBackend::default(), UploadPolicy::Always);
    let before = d.residual_tally();
    d.commit(&mut m).unwrap();
    let words = d3d::layout::RENDER_STATE_SLOTS + d3d::layout::STAGES * d3d::layout::STAGE_SLOTS;
    assert_eq!(d.residual_tally().since(before).state_words, words as u64);
}
