//! The decorators a view draws, typed (docs/graphics/native-decorators.md):
//! each placement's draw as the model decorator draw `0x0004e260` makes it,
//! one item of the layer's list, with its instances in the guest's order
//! and, per instance, the model it draws and the five vertex registers
//! (`v11..v15`) the draw sets with `SET_VERTEX_DATA4F` before its indexed
//! draw. `scene_extract::layout::decorators` reads them; `native_render`'s
//! decorator stage draws each run of instances of one model as one
//! instanced draw.

use std::ops::Range;

use serde::Serialize;

use crate::builder::{BuildError, SceneBuilder};
use crate::scene::{FrameScene, Row, View};

/// The first vertex register the draw sets per instance.
pub const FIRST_REGISTER: u8 = 11;
/// Registers it sets per instance: `v11` (normal, fade), `v12..v14` (the
/// instance's rows: scaled basis and position), `v15` (colour, 1).
pub const REGISTERS: usize = 5;
/// The vertex constant the draw sets once per placement (hardware
/// numbering): the detail level's two factors, 0, 1.
pub const LOD_CONSTANT: u8 = 53;

/// One instance of a placement, as the draw hands it to its indexed draw.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DecoratorInstance {
    /// Its index in the placement's instances (`placement +8` onwards),
    /// in the guest's order.
    pub index: u16,
    /// The set's model it draws (the permutation's `+0xa`).
    pub model: u16,
    /// The model's indices in the set's index buffer: the first (u16
    /// indices) and how many, as `DrawIndexedVertices(TRIANGLELIST, ..)`
    /// takes them.
    pub first_index: u32,
    pub index_count: u32,
    /// `v11..v15`.
    pub registers: [Row; REGISTERS],
}

impl DecoratorInstance {
    /// The world position (`v12..v14`'s `w`).
    pub fn position(&self) -> [f32; 3] {
        [self.registers[1][3], self.registers[2][3], self.registers[3][3]]
    }

    /// The fade the draw puts in `v11.w`, clamped to `[0, 1]`.
    pub fn fade(&self) -> f32 {
        self.registers[0][3]
    }
}

/// One placement's draw: one item of a layer's list whose build is the
/// model decorator build `0x0004de20`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecoratorDraw {
    /// The item's index in the list.
    pub slot: u32,
    pub layer: u8,
    /// The decorator set's tag datum (the scenario's palette entry `+4`).
    pub set: u32,
    /// The shader the item names (`+8`): the permutation's.
    pub shader: u32,
    /// The item's bytes `+0x15` and `+0x14`, the build's third and fifth
    /// arguments: which of the shader template's entries the build picks.
    pub variant: u8,
    pub pass: u8,
    /// The item's argument (`+0xc`): the placement index `<< 8`, the
    /// visible-cluster slot in the low byte, the detail level in bits 30
    /// and 31.
    pub argument: u32,
    /// `c[53]`: the detail level's two factors, 0, 1.
    pub lod_constant: Row,
    /// Into [`FrameScene::decorator_instances`], in the guest's order.
    pub instances: Range<u32>,
}

impl DecoratorDraw {
    /// The placement it draws.
    pub fn placement(&self) -> u32 {
        (self.argument >> 8) & 0x3f_ffff
    }

    /// The detail level (argument bits 30 and 31).
    pub fn lod(&self) -> u8 {
        (self.argument >> 30) as u8
    }
}

impl FrameScene {
    /// The view's decorator draws, in the list's order.
    pub fn decorators_of(&self, view: &View) -> &[DecoratorDraw] {
        self.decorators.get(view.decorators.start as usize..view.decorators.end as usize).unwrap_or(&[])
    }

    /// A draw's instances, in the guest's order.
    pub fn decorator_instances_of(&self, draw: &DecoratorDraw) -> &[DecoratorInstance] {
        self.decorator_instances.get(draw.instances.start as usize..draw.instances.end as usize).unwrap_or(&[])
    }
}

/// Every view's decorator draws inside the array and after the view
/// before's, every draw's instances inside theirs.
pub(crate) fn check(scene: &FrameScene) -> Result<(), String> {
    let mut last = 0;
    for (n, v) in scene.views.iter().enumerate() {
        let r = &v.decorators;
        if r.start > r.end || r.end as usize > scene.decorators.len() {
            return Err(format!("view {n} decorators {r:?} outside 0..{}", scene.decorators.len()));
        }
        if r.start < last {
            return Err(format!("view {n} decorators {r:?} overlap the view before"));
        }
        last = r.end;
    }
    for (n, d) in scene.decorators.iter().enumerate() {
        let r = &d.instances;
        if r.start > r.end || r.end as usize > scene.decorator_instances.len() {
            return Err(format!("decorator draw {n} instances {r:?} outside 0..{}", scene.decorator_instances.len()));
        }
    }
    Ok(())
}

/// The view's decorators as summary-hash words, in order: each draw's
/// slot, layer, set, shader, variant, pass, argument, constant, and its
/// instances (index, model, indices, registers by their bits); then the
/// view's decorator remainder.
pub(crate) fn summary_words(scene: &FrameScene, view: &View, word: &mut dyn FnMut(u32)) {
    let draws = scene.decorators_of(view);
    word(draws.len() as u32);
    for d in draws {
        word(d.slot);
        word(u32::from(d.layer));
        word(d.set);
        word(d.shader);
        word(u32::from(d.variant) | u32::from(d.pass) << 8);
        word(d.argument);
        d.lod_constant.iter().for_each(|x| word(x.to_bits()));
        let instances = scene.decorator_instances_of(d);
        word(instances.len() as u32);
        for i in instances {
            word(u32::from(i.index) | u32::from(i.model) << 16);
            word(i.first_index);
            word(i.index_count);
            i.registers.iter().flatten().for_each(|x| word(x.to_bits()));
        }
    }
    word(view.decorator_remainder);
}

impl SceneBuilder {
    /// Adds a placement's draw with its instances to the open view, after
    /// its decorator draws so far; its index in the scene. The builder
    /// sets `draw.instances`.
    pub fn push_decorator(&mut self, draw: DecoratorDraw, instances: &[DecoratorInstance]) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let i = scene.decorator_instances.len() as u32;
        scene.decorator_instances.extend_from_slice(instances);
        let index = scene.decorators.len() as u32;
        scene.decorators.push(DecoratorDraw { instances: i..scene.decorator_instances.len() as u32, ..draw });
        scene.views[open].decorators.end = index + 1;
        Ok(index)
    }

    /// Counts decorator draws the guest keeps (extraction could not read
    /// them).
    pub fn add_decorator_remainder(&mut self, draws: u32) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].decorator_remainder += draws;
        Ok(())
    }
}

/// One view's decorator sizes ([`FrameScene::decorator_counts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecoratorCounts {
    pub draws: u32,
    pub instances: u32,
    /// Draws the guest still draws.
    pub remainder: u32,
}

impl FrameScene {
    pub fn decorator_counts(&self, view: &View) -> DecoratorCounts {
        let draws = self.decorators_of(view);
        DecoratorCounts {
            draws: draws.len() as u32,
            instances: draws.iter().map(|d| d.instances.len() as u32).sum(),
            remainder: view.decorator_remainder,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::camera;
    use crate::scene::{SceneId, Stamp, ViewKey, ViewKind};
    use crate::summary_hash;

    fn key(view: u8) -> ViewKey {
        ViewKey { kind: ViewKind::Game, player: 0, view }
    }

    fn draw(slot: u32) -> DecoratorDraw {
        DecoratorDraw {
            slot,
            layer: 3,
            set: 0xe400_0001,
            shader: 0xe500_0002,
            variant: 0,
            pass: 0,
            argument: 7 << 8 | 2 | 1 << 30,
            lod_constant: [0.5, 2.0, 0.0, 1.0],
            instances: 0..0,
        }
    }

    fn instance(index: u16, model: u16, x: f32) -> DecoratorInstance {
        DecoratorInstance {
            index,
            model,
            first_index: 12 * u32::from(model),
            index_count: 12,
            registers: [[0.0, 0.0, 1.0, 0.75], [1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 2.0], [0.0, 0.0, 1.0, 3.0], [1.0; 4]],
        }
    }

    #[test]
    fn a_views_decorator_draws_keep_their_instances_in_order() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(b.push_decorator(draw(0), &[]).unwrap_err(), BuildError::NoView);
        b.open_view(key(0), camera()).unwrap();
        b.push_decorator(draw(4), &[instance(0, 1, 10.0), instance(1, 1, 11.0), instance(2, 0, 12.0)]).unwrap();
        b.push_decorator(draw(9), &[instance(0, 2, 20.0)]).unwrap();
        b.add_decorator_remainder(1).unwrap();
        b.open_view(key(1), camera()).unwrap();
        b.push_decorator(draw(1), &[]).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        let draws = s.decorators_of(v);
        assert_eq!(draws.iter().map(|d| d.slot).collect::<Vec<_>>(), [4, 9]);
        assert_eq!((draws[0].placement(), draws[0].lod()), (7, 1));
        let xs: Vec<f32> = s.decorator_instances_of(&draws[0]).iter().map(|i| i.position()[0]).collect();
        assert_eq!(xs, [10.0, 11.0, 12.0]);
        assert_eq!(s.decorator_instances_of(&draws[0])[0].fade(), 0.75);
        assert_eq!(s.decorator_counts(v), DecoratorCounts { draws: 2, instances: 4, remainder: 1 });
        assert_eq!(s.decorator_counts(&s.views[1]), DecoratorCounts { draws: 1, instances: 0, remainder: 0 });
        let mut bad = (*s).clone();
        bad.decorators[1].instances = 3..90;
        assert!(bad.check().unwrap_err().contains("instances"));
        let mut bad = (*s).clone();
        bad.views[1].decorators = 1..3;
        assert!(bad.check().is_err());
    }

    #[test]
    fn decorators_count_in_the_hash_and_a_recycled_scene_has_none() {
        let build = |x: f32| {
            let mut b = SceneBuilder::default();
            b.begin(SceneId::default());
            b.open_view(key(0), camera()).unwrap();
            b.push_decorator(draw(4), &[instance(0, 1, x)]).unwrap();
            b.seal(Stamp::default()).unwrap()
        };
        assert_eq!(summary_hash(&build(1.0)), summary_hash(&build(1.0)));
        assert_ne!(summary_hash(&build(1.0)), summary_hash(&build(1.5)), "an instance moved");
        let mut s = (*build(1.0)).clone();
        s.clear();
        assert!(s.decorators.is_empty() && s.decorator_instances.is_empty());
    }
}
