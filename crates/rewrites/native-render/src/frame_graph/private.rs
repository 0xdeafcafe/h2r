//! Private captures containing both display-grid and fixed-size attachments.

use d3d::scaled::private_scene::MixedPrivatePass;

use super::*;

impl FramePass for MixedPrivatePass<'_> {
    fn apply(&mut self, op: &ReplayOp) -> Result<(), String> {
        MixedPrivatePass::apply(self, op)
    }

    fn read_target(&mut self, target: TargetRef) -> ReadColour {
        MixedPrivatePass::read_target(self, target)
    }

    fn tile_work(&self) -> Option<TileWork> {
        MixedPrivatePass::tile_work(self)
    }
}

pub(super) fn draw_mixed(
    backend: &mut GfxBackend,
    scene: &FrameScene,
    replay: &ReplayFrame,
    scale: u32,
    lighting: Lighting,
    observer: Option<&mut dyn PassObserver>,
) -> Result<CapturedFrame, String> {
    let before = backend.batch_stats();
    let mut pass = MixedPrivatePass::new_lit(backend, replay, scale, lighting)?;
    // A mixed-size frame draws no sun tile: every object shadow keeps
    // retail's mask.
    let e3 = record(&mut pass, scene, replay, true, lighting, observer, None, None)?;
    let (size, rgba) = pass.read_rgba(replay.presented)?;
    drop(pass);
    Ok(CapturedFrame {
        id: scene.id,
        scene_hash: summary_hash(scene),
        size,
        rgba,
        report: FrameReport::new(scene.id, lighting, e3, before, backend.batch_stats()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{AuxTarget, SceneBuilder, Slot, SlotAt, Stamp};
    use gfx::{Clear, D3DFormat, GfxContext};

    /// An 8 by 4 main colour beside a 4 by 2 auxiliary one: the auxiliary
    /// cleared red, then the main blue.
    fn mixed_frame() -> ReplayFrame {
        let main = TargetRef::SceneColour;
        let aux = TargetRef::Aux(0);
        let colour = AuxTarget {
            width: 8,
            height: 4,
            depth: false,
            format: D3DFormat::LinA8R8G8B8 as u32,
        };
        let mut frame = ReplayFrame {
            presented: main,
            colour: Some(colour),
            depth: Some(AuxTarget {
                depth: true,
                format: D3DFormat::LinD24S8 as u32,
                ..colour
            }),
            aux: vec![AuxTarget {
                width: 4,
                height: 2,
                ..colour
            }],
            ..Default::default()
        };
        for (colour, rgba) in [(aux, [1., 0., 0., 1.]), (main, [0., 0., 1., 1.])] {
            frame.ops.push((
                SlotAt {
                    view: 0,
                    slot: Slot::Opaque,
                },
                ReplayOp::Clear {
                    colour,
                    depth: TargetRef::SceneDepth,
                    clear: Clear {
                        color: Some(rgba),
                        depth: Some(1.),
                        ..Clear::NONE
                    },
                },
            ));
        }
        frame
    }

    /// A screenshot of the back buffer alone cannot detect a larger auxiliary
    /// target when no edge lands between guest texels. Read the actual grid
    /// used by each pass and compare a pixel on it.
    #[test]
    fn private_scaled_frame_preserves_each_attachment_grid() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let mut backend = GfxBackend::new(ctx);
        let (main, aux) = (TargetRef::SceneColour, TargetRef::Aux(0));
        let frame = mixed_frame();
        let mut builder = SceneBuilder::default();
        builder.begin(SceneId::default());
        let scene = builder.seal(Stamp::default()).unwrap();

        #[derive(Default)]
        struct Samples(Vec<(TargetRef, [u32; 2], [u8; 4])>);
        impl PassObserver for Samples {
            fn pass_end(&mut self, _: usize, row: &PassRow, read: &mut dyn FnMut() -> ReadColour) {
                let (size, rgba) = read().unwrap();
                self.0
                    .push((row.colour, size, rgba[0..4].try_into().unwrap()));
            }
        }
        let mut samples = Samples::default();
        let retail = draw_private_with_observer(
            &mut backend,
            &scene,
            &frame,
            2,
            Lighting::Retail,
            Some(&mut samples),
        )
        .unwrap();
        assert_eq!(
            samples.0,
            [
                (aux, [4, 2], [255, 0, 0, 255]),
                (main, [16, 8], [0, 0, 255, 255]),
            ]
        );
        assert_eq!(retail.size, [16, 8]);
        let e3 = draw_private(&mut backend, &scene, &frame, 2, Lighting::E3).unwrap();
        assert_eq!(e3.rgba, retail.rgba);
        assert_eq!(e3.scene_hash, retail.scene_hash);
        assert!(backend.last_presented().is_none());
    }

    /// Above 1x a display frame with an auxiliary target beside the display
    /// grid draws it at its own size, as the private capture does, and
    /// hands the caller its display-grid image.
    #[test]
    fn a_scaled_display_frame_draws_mixed_size_attachments() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let mut backend = GfxBackend::new(ctx);
        let frame = mixed_frame();
        let mut builder = SceneBuilder::default();
        builder.begin(SceneId::default());
        let scene = builder.seal(Stamp::default()).unwrap();
        let captured = draw_private(&mut backend, &scene, &frame, 2, Lighting::Retail).unwrap();
        let drawn = crate::frame_graph::display_rate::draw_blend(
            &mut backend,
            &scene,
            &scene,
            &frame,
            2,
            Lighting::Retail,
            frame_scene::blend::Plan::Newest,
        )
        .unwrap();
        let (mut pixels, order) = backend.read_color(drawn.image).unwrap();
        if order == gfx::encode::HostPixelOrder::Bgra8 {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        assert_eq!(captured.size, [16, 8]);
        assert_eq!(pixels, captured.rgba);
        assert!(drawn.cameras.is_empty());
        // The caller keeps it until the next display frame.
        backend.retain_shown(drawn.image);
        assert!(backend.last_presented().is_none());
    }
}
