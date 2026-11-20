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
    let mut pass = MixedPrivatePass::new(backend, replay, scale)?;
    let e3 = record(&mut pass, replay, true, lighting, observer)?;
    let (size, rgba) = pass.read_target(replay.presented)?;
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

    /// A screenshot of the back buffer alone cannot detect a larger auxiliary
    /// target when no edge lands between guest texels. Read the actual grid
    /// used by each pass and compare a pixel on it.
    #[test]
    fn private_scaled_frame_preserves_each_attachment_grid() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let mut backend = GfxBackend::new(ctx);
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
}
