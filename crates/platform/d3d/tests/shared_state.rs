//! A draw's raw state words (`ResolvedDraw::raw`) are shared with the
//! device's pushed snapshot instead of copied. A draw keeps the words it was
//! made with whatever the device sets afterwards (render state, stage state,
//! inline pushes that change only the snapshot), and the snapshot follows
//! those setters, through the synchronous backend and the render thread.

use crate::common::*;
use d3d::abi::Regs;
use d3d::backend::{Recorded, ResolvedDraw};
use d3d::{Backend, Device, RecordingBackend, SparseMemory, ThreadedBackend, UploadPolicy};

const STAGE: u32 = 1;
const STAGE_STATE: u32 = 10;

fn run<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory) {
    set_states(d, m);
    quad_vb(m, -1.0, -1.0, 1.0, 1.0, 0xFF00_FF00);
    bind_quad(d, m);
    // 0: as set up.
    d.draw_vertices(m, QUADLIST, 0, 4).unwrap();
    // 1: an API render-state and stage-state change.
    d.set_render_state(m, RS_ZFUNC, 3).unwrap();
    d.set_texture_stage_state(m, STAGE, STAGE_STATE, 7).unwrap();
    d.draw_vertices(m, QUADLIST, 0, 4).unwrap();
    // 2: an inline push (SetRenderState_Simple: ALPHABLENDENABLE = 1) that
    // changes the pushed snapshot only, then an inline draw against it.
    call(
        d,
        m,
        0x3f5cf0,
        Regs {
            ecx: 0x0004_0304,
            edx: 1,
            ..Regs::default()
        },
        &[],
    )
    .unwrap();
    inline_quad(m, -1.0, -1.0, 1.0, 1.0, [1, 2, 3, 4]);
    d.kick_off(m).unwrap();
    // 3: the next API draw reads the guest arrays again.
    d.set_render_state(m, RS_ZFUNC, 5).unwrap();
    d.draw_vertices(m, QUADLIST, 0, 4).unwrap();
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

fn check(d: &[ResolvedDraw]) {
    let rs = |i: usize, slot: u32| d[i].raw.render[slot as usize];
    let ts = |i: usize| d[i].raw.stages[STAGE as usize][STAGE_STATE as usize];
    assert_eq!(d.len(), 4);
    assert_ne!(rs(0, RS_ZFUNC), 3);
    assert_ne!(ts(0), 7);
    assert_eq!((rs(1, RS_ZFUNC), ts(1)), (3, 7));
    assert_eq!(rs(1, RS_ALPHABLENDENABLE), 0);
    assert_eq!(
        (rs(2, RS_ZFUNC), ts(2), rs(2, RS_ALPHABLENDENABLE)),
        (3, 7, 1)
    );
    // The guest array still holds 0 for ALPHABLENDENABLE: the API draw
    // reads it, not the snapshot.
    assert_eq!(
        (rs(3, RS_ZFUNC), ts(3), rs(3, RS_ALPHABLENDENABLE)),
        (5, 7, 0)
    );
}

#[test]
fn draws_keep_their_words_through_later_setters() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    run(&mut d, &mut m);
    let sync = draws(std::mem::take(&mut d.backend.calls));
    check(&sync);

    let (mut t, mut tm) = create(
        ThreadedBackend::new(RecordingBackend::new()).unwrap(),
        UploadPolicy::Tracked,
    );
    run(&mut t, &mut tm);
    let threaded = draws(
        t.backend
            .with_inner(|b| std::mem::take(&mut b.calls))
            .unwrap(),
    );
    check(&threaded);
    for (a, b) in sync.iter().zip(&threaded) {
        assert_eq!(a.raw, b.raw);
        assert_eq!(a.state, b.state);
    }
}
