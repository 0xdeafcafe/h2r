//! A game view as one pass per slot (native-only plan, `nor-8b`;
//! docs/graphics/framescene.md, "Slice 6"). Each game view of the frame is
//! the thirteen passes of [`ViewSlot::PASSES`] in traced order, then the
//! frame's overlay is its three. A pass names the declared targets
//! ([`targets::declare`]) its ops touch, and how each loads there: the
//! declared load in the first pass that touches the target, `Keep` in
//! every later one. This is what a graph that owns its targets begins its
//! passes with in place of the guest's binds and clears.
//!
//! The replay carries five slots per view (frame-types `Slot`), each
//! spanning a run of traced slots ([`ViewSlot::carried`]). An op is put in
//! the first pass its carried slot spans: the earliest it can draw, so a
//! target's load is never late. The replay's binds and clears still run;
//! the plan is counted (`FrameReport::passes`), not yet drawn by.

use frame_scene::{Slot, TargetRef, ViewSlot};

use d3d::scene_sink::ReplayFrame;

use super::targets::{self, Declared, Load};

/// One pass of the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewPass {
    /// The guest view index; `None` for the overlay's passes.
    pub view: Option<u8>,
    pub slot: ViewSlot,
    /// The targets the pass's ops touch, in the order first touched, with
    /// their load in this pass.
    pub targets: Vec<(TargetRef, Load)>,
}

/// The frame's passes: thirteen per game view, in the order the views
/// first draw, then the overlay's three when the frame has a game view.
/// A frame with no game view (a menu, a loading screen) has none.
pub fn plan(frame: &ReplayFrame, declared: &[Declared]) -> Vec<ViewPass> {
    let mut views: Vec<u8> = Vec::new();
    for (at, _) in &frame.ops {
        if at.slot.in_game_view() && !views.contains(&at.view) {
            views.push(at.view);
        }
    }
    if views.is_empty() {
        return Vec::new();
    }
    let per_view = &ViewSlot::PASSES[..13];
    let mut passes: Vec<ViewPass> = views
        .iter()
        .flat_map(|v| per_view.iter().map(|s| ViewPass { view: Some(*v), slot: *s, targets: Vec::new() }))
        .chain(ViewSlot::carried(Slot::Overlay).iter().map(|s| ViewPass { view: None, slot: *s, targets: Vec::new() }))
        .collect();
    let mut loaded: Vec<TargetRef> = Vec::new();
    let mut bound = None;
    for (at, op) in &frame.ops {
        let touched = targets::op_targets(op, &mut bound);
        let Some(first) = ViewSlot::carried(at.slot).first() else {
            continue;
        };
        let view = at.slot.in_game_view().then_some(at.view);
        let Some(pass) = passes.iter_mut().find(|p| p.view == view && p.slot == *first) else {
            continue;
        };
        for target in touched {
            if target == TargetRef::None || pass.targets.iter().any(|(t, _)| *t == target) {
                continue;
            }
            let load = if loaded.contains(&target) {
                Load::Keep
            } else {
                loaded.push(target);
                declared.iter().find(|d| d.target == target).map_or(Load::Keep, |d| d.load)
            };
            pass.targets.push((target, load));
        }
    }
    passes
}

#[cfg(test)]
mod tests {
    use super::*;
    use d3d::scene_sink::ReplayOp;
    use frame_scene::{AuxTarget, SlotAt};
    use gfx::{Clear, D3DFormat};

    fn at(slot: Slot, view: u8) -> SlotAt {
        SlotAt { slot, view }
    }

    fn frame() -> ReplayFrame {
        let colour = AuxTarget { width: 640, height: 480, depth: false, format: D3DFormat::LinA8R8G8B8 as u32 };
        let depth = AuxTarget { depth: true, format: D3DFormat::LinD24S8 as u32, ..colour };
        let (main, z, snap) = (TargetRef::SceneColour, TargetRef::SceneDepth, TargetRef::Aux(0));
        let clear = Clear { color: Some([0.; 4]), depth: Some(1.), stencil: Some(0) };
        ReplayFrame {
            presented: main,
            colour: Some(colour),
            depth: Some(depth),
            aux: vec![colour],
            ops: vec![
                (at(Slot::Opaque, 0), ReplayOp::Bind { colour: main, depth: z }),
                (at(Slot::Opaque, 0), ReplayOp::Clear { colour: main, depth: z, clear }),
                (at(Slot::AfterLights, 0), ReplayOp::Copy { from: main, to: snap }),
                (at(Slot::Opaque, 1), ReplayOp::Bind { colour: main, depth: z }),
                (at(Slot::Opaque, 1), ReplayOp::Clear { colour: main, depth: z, clear }),
                (at(Slot::Overlay, 1), ReplayOp::Bind { colour: main, depth: TargetRef::None }),
                (at(Slot::Overlay, 1), ReplayOp::Clear { colour: main, depth: TargetRef::None, clear: Clear { color: None, depth: None, stencil: None } }),
            ],
            ..Default::default()
        }
    }

    /// Two game views (split screen) and the overlay: thirteen passes per
    /// view in traced order, view after view, then the overlay's three
    /// once.
    #[test]
    fn a_game_view_is_one_pass_per_slot() {
        let frame = frame();
        let passes = plan(&frame, &targets::declare(&frame));
        assert_eq!(passes.len(), 13 * 2 + 3);
        for (v, view) in passes.chunks(13).take(2).enumerate() {
            assert!(view.iter().all(|p| p.view == Some(v as u8)), "view {v}");
            let slots: Vec<ViewSlot> = view.iter().map(|p| p.slot).collect();
            assert_eq!(slots, ViewSlot::PASSES[..13], "view {v}");
            assert_eq!(view.iter().map(|p| p.slot.number()).collect::<Vec<_>>(), (1..=13u8).map(Some).collect::<Vec<_>>());
        }
        let overlay: Vec<(Option<u8>, ViewSlot)> = passes[26..].iter().map(|p| (p.view, p.slot)).collect();
        assert_eq!(overlay, [(None, ViewSlot::Hud), (None, ViewSlot::Fade), (None, ViewSlot::Ui)]);
        let menu = ReplayFrame { ops: vec![(at(Slot::Outside, 0), ReplayOp::Bind { colour: TargetRef::SceneColour, depth: TargetRef::None })], ..Default::default() };
        assert!(plan(&menu, &targets::declare(&menu)).is_empty(), "a frame outside every game view has no view pass");
    }

    /// The scene targets load as declared in view 0's slot 1 and are kept
    /// after; a copy in the carried slot after the lights lands in slot 5,
    /// the first that slot spans; a pass no op reaches names nothing.
    #[test]
    fn a_target_loads_in_its_first_pass_only() {
        let frame = frame();
        let passes = plan(&frame, &targets::declare(&frame));
        let pass = |view: Option<u8>, slot: ViewSlot| passes.iter().find(|p| p.view == view && p.slot == slot).unwrap();
        let (main, z, snap) = (TargetRef::SceneColour, TargetRef::SceneDepth, TargetRef::Aux(0));
        assert_eq!(pass(Some(0), ViewSlot::OpaqueBase).targets, [(main, Load::Clear), (z, Load::Clear)]);
        assert_eq!(pass(Some(0), ViewSlot::DecalsMode1).targets, [(main, Load::Keep), (snap, Load::Keep)], "a copy is not a clear");
        assert_eq!(pass(Some(1), ViewSlot::OpaqueBase).targets, [(main, Load::Keep), (z, Load::Keep)], "loaded in view 0");
        assert_eq!(pass(None, ViewSlot::Hud).targets, [(main, Load::Keep)]);
        assert!(pass(Some(0), ViewSlot::Post).targets.is_empty());
    }
}
