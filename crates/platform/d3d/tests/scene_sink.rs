//! The scene sink (docs/graphics/framescene.md, slice 6) on the device's
//! draw rig: it carries the device's draws in the order the backend gets
//! them, each at the slot that was current, names the presented surface
//! the scene's colour, and passes every draw on to the backend under it.
//! Synthetic device over the recording backend; no GPU.

mod common;

use std::sync::atomic::{AtomicU16, Ordering};

use common::*;
use d3d::{Device, RecordingBackend, ResolvedDraw, SceneSink, SparseMemory, UploadPolicy};
use frame_scene::{GuestOp, Slot, SlotAt, TargetRef};

/// This file's slot: `u16::MAX` off, else `slot << 8 | view`.
static AT: AtomicU16 = AtomicU16::new(u16::MAX);

fn source() -> Option<SlotAt> {
    let v = AT.load(Ordering::Relaxed);
    (v != u16::MAX).then(|| SlotAt { slot: Slot::ALL[usize::from(v >> 8)], view: v as u8 })
}

fn at(slot: Slot, view: u8) {
    AT.store(((slot.index() as u16) << 8) | u16::from(view), Ordering::Relaxed);
}

fn quad<B: d3d::Backend>(d: &mut Device<B>, m: &mut SparseMemory, color: u32) {
    quad_vb(m, 0.0, 0.0, 32.0, 32.0, color);
    bind_quad(d, m);
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

#[test]
fn the_sink_carries_the_devices_draws_in_order_by_slot_and_the_backend_still_draws_them() {
    let sink = SceneSink::new(RecordingBackend::new(), source).keeping_draws();
    let (mut d, mut m) = create(sink, UploadPolicy::Always);
    set_states(&mut d, &mut m);
    // Frames until every buffer of the swap chain has been presented once.
    at(Slot::Outside, 0);
    for _ in 0..3 {
        quad(&mut d, &mut m, 0xFF00_0000);
        d.swap(&mut m, 0).unwrap();
    }
    at(Slot::Opaque, 1);
    quad(&mut d, &mut m, 0xFFFF_0000);
    quad(&mut d, &mut m, 0xFF00_FF00);
    at(Slot::Transparents, 1);
    quad(&mut d, &mut m, 0xFF00_00FF);
    at(Slot::Overlay, 1);
    quad(&mut d, &mut m, 0xFFFF_FFFF);
    d.swap(&mut m, 0).unwrap();

    let frame = d.backend.last_frame();
    let carried: Vec<(Slot, &ResolvedDraw)> = frame
        .ops
        .iter()
        .filter_map(|o| match &o.op {
            GuestOp::Draw(Some(draw)) => Some((o.at.slot, &**draw)),
            _ => None,
        })
        .collect();
    assert_eq!(carried.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [
        Slot::Opaque,
        Slot::Opaque,
        Slot::Transparents,
        Slot::Overlay
    ]);
    let recorded = d.backend.inner().draws();
    assert!(recorded.len() >= carried.len());
    let tail = &recorded[recorded.len() - carried.len()..];
    assert!(
        carried.iter().map(|(_, draw)| *draw).eq(tail.iter().copied()),
        "the carried draws are the backend's, in its order"
    );
    assert!(frame.ops.iter().all(|o| o.at.view == 1));
    // One bind per slot: the attachments do not change, the slot does.
    let binds: Vec<(Slot, TargetRef)> = frame
        .ops
        .iter()
        .filter_map(|o| match o.op {
            GuestOp::Bind { colour, .. } => Some((o.at.slot, colour)),
            _ => None,
        })
        .collect();
    assert_eq!(binds, [
        (Slot::Opaque, TargetRef::SceneColour),
        (Slot::Transparents, TargetRef::SceneColour),
        (Slot::Overlay, TargetRef::SceneColour)
    ]);
    assert!(frame.aux.is_empty(), "nothing but the back buffer is drawn into: {:?}", frame.aux);
    let summary = frame.summary();
    assert_eq!(summary.slots[Slot::Opaque.index()].draws, 2);
    assert_eq!(summary.views.len(), 1);
    assert!(summary.side_targets.is_empty());
    assert_eq!(summary.readback_total(), 0);
}

#[test]
fn a_sink_with_no_slot_records_nothing_and_changes_nothing() {
    fn off() -> Option<SlotAt> {
        None
    }
    let (mut d, mut m) = create(SceneSink::new(RecordingBackend::new(), off).keeping_draws(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    let (mut plain, mut pm) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut plain, &mut pm);
    for _ in 0..2 {
        quad(&mut d, &mut m, 0xFF12_3456);
        d.swap(&mut m, 0).unwrap();
        quad(&mut plain, &mut pm, 0xFF12_3456);
        plain.swap(&mut pm, 0).unwrap();
    }
    assert!(d.backend.last_frame().ops.is_empty());
    assert_eq!(d.backend.inner().calls, plain.backend.calls, "the sink passes the same calls on");
}
