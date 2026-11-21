//! Whole attachment values that can be reconstructed without a GPU read.
//! A draw invalidates the value of its destination. A frame that samples
//! the previous render target still needs that frame's actual pixels.

use std::sync::Arc;

use frame_scene::TargetRef;
use gfx::texture::TextureDesc;

use super::{ReplayFrame, ReplayOp};

#[derive(Clone, Debug)]
pub enum TargetSeed {
    Colour([f32; 4]),
    Upload { desc: TextureDesc, bytes: Arc<[u8]> },
}

/// Before sealing translates names, each attachment indexes the physical
/// target list. Only this present's live handles are kept for the next one.
pub(super) fn final_contents(frame: &ReplayFrame) -> Vec<Option<TargetSeed>> {
    let mut seeds = vec![None; frame.aux.len()];
    let mut bound = None;
    let index = |target| match target {
        TargetRef::Aux(i) => Some(usize::from(i)),
        _ => None,
    };
    for (target, seed) in &frame.initial {
        if let Some(i) = index(*target) {
            seeds[i] = Some(seed.clone());
        }
    }
    for (_, op) in &frame.ops {
        match op {
            ReplayOp::Bind { colour, depth } => bound = Some((index(*colour), index(*depth))),
            ReplayOp::Clear { colour, depth, clear } => {
                if let (Some(i), Some(c)) = (index(*colour), clear.color) { seeds[i] = Some(TargetSeed::Colour(c)); }
                if (clear.depth.is_some() || clear.stencil.is_some()) && let Some(i) = index(*depth) { seeds[i] = None; }
            }
            ReplayOp::ClearRects { colour, depth, clear } => {
                if clear.color.is_some() && let Some(i) = index(*colour) { seeds[i] = None; }
                if (clear.depth.is_some() || clear.stencil.is_some()) && let Some(i) = index(*depth) { seeds[i] = None; }
            }
            ReplayOp::Copy { from, to } => {
                if let Some(dst) = index(*to) {
                    seeds[dst] = index(*from).filter(|&src| frame.aux[src] == frame.aux[dst])
                        .and_then(|src| seeds[src].clone());
                }
            }
            ReplayOp::Upload { to, desc, bytes } => {
                if let Some(i) = index(*to) {
                    let t = frame.aux[i];
                    seeds[i] = ((t.width, t.height) == (desc.width, desc.height)).then(|| {
                        TargetSeed::Upload { desc: desc.clone(), bytes: Arc::clone(bytes) }
                    });
                }
            }
            ReplayOp::Draw { draw, .. } => {
                if let Some((colour, depth)) = bound {
                    let mask = draw.state.render.color_write;
                    if (mask.red || mask.green || mask.blue || mask.alpha) && let Some(i) = colour { seeds[i] = None; }
                    if draw.writes_depth_stencil() && let Some(i) = depth { seeds[i] = None; }
                }
            }
            ReplayOp::Stage(stage) => {
                for target in [stage.surfaces.color, stage.surfaces.depth].into_iter().chain(
                    stage.plan.slots.iter().zip(&stage.surfaces.slots).filter_map(|(slot, target)| {
                        slot.filter(|s| s.written).and(*target)
                    })
                ) {
                    if let Some(i) = index(target) { seeds[i] = None; }
                }
            }
            ReplayOp::Sample { .. } | ReplayOp::BeginFlareQuery | ReplayOp::EndFlareQuery(_) => {}
        }
    }
    seeds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Backend, RecordingBackend, SceneSink};
    use frame_scene::{Slot, SlotAt};
    use gfx::{Clear, D3DFormat};
    use gfx::texture::TextureKind;
    use std::sync::Mutex;

    #[test]
    fn earlier_upload_and_clear_seed_a_private_frame_until_another_write() {
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&frames);
        let mut sink = SceneSink::new(RecordingBackend::new(), || Some(SlotAt { slot: Slot::Opaque, view: 0 }))
            .with_replay(move |f| output.lock().unwrap().push(f));
        let fmt = D3DFormat::LinA8R8G8B8;
        let side = sink.create_texture_target(fmt, 4, 4).unwrap();
        let main = sink.create_surface(fmt, 4, 4, false).unwrap();
        let desc = TextureDesc::new(fmt, TextureKind::D2, 4, 4, 1, 1);
        let mut texels = [3, 73, 149, 255].repeat(16);
        sink.write_surface(side, &desc, &texels).unwrap();
        texels.fill(0);
        sink.clear(main, None, Clear::all([0., 0., 1., 1.])).unwrap();
        sink.present(main, 0).unwrap();

        sink.copy_surface(side, main).unwrap();
        sink.present(main, 0).unwrap();
        let frame = frames.lock().unwrap().pop().unwrap();
        assert_eq!(frame.private_target_refusal(), None);
        assert!(frame.initial.iter().any(|(t, seed)| matches!((t, seed),
            (TargetRef::Aux(0), TargetSeed::Upload { bytes, .. }) if bytes[..4] == [3, 73, 149, 255])));
        assert!(frame.initial.iter().any(|(t, seed)| matches!((t, seed),
            (TargetRef::SceneColour, TargetSeed::Colour(v)) if *v == [0., 0., 1., 1.])));
        // Unobserved device writes must invalidate even a formerly exact value.
        sink.inner_mut().clear(side, None, Clear::all([1.; 4])).unwrap();
        sink.copy_surface(side, main).unwrap();
        sink.present(main, 0).unwrap();
        let lost = frames.lock().unwrap().pop().unwrap();
        assert!(lost.initial.is_empty());
        assert!(lost.private_target_refusal().is_some());

        #[cfg(feature = "gpu")]
        if let Ok(ctx) = gfx::GfxContext::headless() {
            let mut gpu = crate::GfxBackend::new(ctx);
            for scale in [1, 4] {
                let mut pass = crate::gfx_backend::GuestDrawPass::new(&mut gpu, &frame, scale).unwrap();
                for (_, op) in &frame.ops { pass.apply(op).unwrap(); }
                let ([w, h], pixels) = pass.read_rgba().unwrap();
                assert_eq!([w, h], [4 * scale; 2]);
                assert!(pixels.chunks_exact(4).all(|pixel| pixel == [149, 73, 3, 255]));
            }
        }
    }
}
