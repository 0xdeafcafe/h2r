//! The decorator stage's plan (docs/graphics/native-decorators.md): a
//! view's typed decorator draws ([`frame_scene::DecoratorDraw`]) as
//! instanced draws. The guest draws each instance alone, one indexed draw
//! after it sets `v11..v15`; here a run of consecutive instances, in the
//! guest's order, that draw the same model of the same set with the same
//! shader and `c[53]` is one draw, `v11..v15` coming from an
//! instance-rate buffer (locations 11 to 15). Joining only neighbours
//! keeps the guest's draw order, so equal depths resolve as they do.
//!
//! [`decorators_gpu`](crate::decorators_gpu) records a plan; the program
//! for a decorator pass is the material's, translated with
//! [`PROVIDED_INPUTS`] supplied.

use std::ops::Range;

use frame_scene::decorator::{FIRST_REGISTER, REGISTERS};
use frame_scene::{DecoratorDraw, FrameScene, Row, View};

/// The skip: layer 3's bit cleared on the planned items, all or nothing.
pub use scene_extract::decorators::clear_slots;

/// Bytes of one instance in the instance buffer: `v11..v15`, four floats
/// each.
pub const INSTANCE_STRIDE: u64 = (REGISTERS * 16) as u64;

/// The vertex registers the instance buffer supplies (bit `n` for `v<n>`),
/// for the translator's `provided_inputs`.
pub const PROVIDED_INPUTS: u16 = ((1u32 << (FIRST_REGISTER as usize + REGISTERS)) - (1u32 << FIRST_REGISTER)) as u16;

/// The instance buffer's attributes: shader location and byte offset.
pub fn instance_attributes() -> [(u32, u64); REGISTERS] {
    std::array::from_fn(|n| (u32::from(FIRST_REGISTER) + n as u32, 16 * n as u64))
}

/// What a batch's pipeline and bindings are chosen by: the set (its
/// vertex stream and index buffer) and the shader's pass as the build
/// picks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Material {
    pub set: u32,
    pub shader: u32,
    pub variant: u8,
    pub pass: u8,
    pub layer: u8,
}

impl Material {
    pub fn of(draw: &DecoratorDraw) -> Self {
        Self { set: draw.set, shader: draw.shader, variant: draw.variant, pass: draw.pass, layer: draw.layer }
    }
}

/// One instanced draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub material: Material,
    /// `c[53]` for the batch's draws.
    pub lod_constant: Row,
    /// The model's indices in the set's index buffer (u16, a triangle
    /// list).
    pub first_index: u32,
    pub index_count: u32,
    /// Into [`Plan::instances`]: the instance range of the draw.
    pub instances: Range<u32>,
}

/// A view's decorators as instanced draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub batches: Vec<Batch>,
    /// `v11..v15` of every instance drawn, in batch order.
    pub instances: Vec<[Row; REGISTERS]>,
    /// The list slots of the draws the plan replaces, in the list's order:
    /// what the guest skips.
    pub slots: Vec<u32>,
    /// Draws the plan leaves to the guest: their material is not one the
    /// stage has.
    pub refused: u32,
}

impl Plan {
    /// The instance buffer's bytes.
    pub fn instance_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.instances)
    }

    /// Instances drawn.
    pub fn instance_count(&self) -> u32 {
        self.instances.len() as u32
    }
}

/// The view's decorators whose material `has` accepts, as instanced
/// draws; the others are the guest's (counted in [`Plan::refused`]).
pub fn plan(scene: &FrameScene, view: &View, has: impl Fn(&Material) -> bool) -> Plan {
    let mut out = Plan::default();
    for draw in scene.decorators_of(view) {
        let material = Material::of(draw);
        if !has(&material) {
            out.refused += 1;
            continue;
        }
        out.slots.push(draw.slot);
        for i in scene.decorator_instances_of(draw) {
            let n = out.instances.len() as u32;
            out.instances.push(i.registers);
            let joins = out.batches.last().is_some_and(|b| {
                b.material == material
                    && b.lod_constant.map(f32::to_bits) == draw.lod_constant.map(f32::to_bits)
                    && b.first_index == i.first_index
                    && b.index_count == i.index_count
                    && b.instances.end == n
            });
            match out.batches.last_mut() {
                Some(b) if joins => b.instances.end = n + 1,
                _ => out.batches.push(Batch {
                    material,
                    lod_constant: draw.lod_constant,
                    first_index: i.first_index,
                    index_count: i.index_count,
                    instances: n..n + 1,
                }),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use frame_scene::{DecoratorInstance, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};

    use super::*;

    fn camera() -> frame_scene::RenderCamera {
        frame_scene::RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        }
    }

    fn draw(slot: u32, set: u32, lod: f32) -> DecoratorDraw {
        DecoratorDraw {
            slot,
            layer: 3,
            set,
            shader: 0xe500_0000 | set,
            variant: 0,
            pass: 0,
            argument: slot << 8,
            lod_constant: [lod, 1.0, 0.0, 1.0],
            instances: 0..0,
        }
    }

    fn instance(index: u16, model: u16, x: f32) -> DecoratorInstance {
        DecoratorInstance {
            index,
            model,
            first_index: 30 * u32::from(model),
            index_count: 30,
            registers: [[x; 4]; REGISTERS],
        }
    }

    #[test]
    fn neighbours_of_one_model_join_and_the_guests_order_is_kept() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera()).unwrap();
        // Draw 1: models 0, 0, 1, 0. Draw 2 (same set, same constant):
        // model 0 continues the last run. Draw 3: another detail level.
        // Draw 4: a set the stage does not have.
        b.push_decorator(draw(1, 1, 0.5), &[instance(0, 0, 1.0), instance(1, 0, 2.0), instance(2, 1, 3.0), instance(3, 0, 4.0)])
            .unwrap();
        b.push_decorator(draw(2, 1, 0.5), &[instance(0, 0, 5.0)]).unwrap();
        b.push_decorator(draw(3, 1, 0.25), &[instance(0, 0, 6.0)]).unwrap();
        b.push_decorator(draw(4, 2, 0.5), &[instance(0, 0, 7.0)]).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        let p = plan(&s, &s.views[0], |m| m.set == 1);
        let runs: Vec<(u32, Range<u32>)> = p.batches.iter().map(|b| (b.first_index, b.instances.clone())).collect();
        assert_eq!(runs, [(0, 0..2), (30, 2..3), (0, 3..5), (0, 5..6)]);
        assert_eq!(p.batches[3].lod_constant[0], 0.25);
        assert_eq!(p.instances.iter().map(|r| r[0][0]).collect::<Vec<_>>(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!((p.slots.clone(), p.refused), (vec![1, 2, 3], 1));
        assert_eq!(p.instance_bytes().len() as u64, 6 * INSTANCE_STRIDE);
        assert_eq!(p.instance_count(), 6);
    }

    #[test]
    fn the_instance_buffer_supplies_v11_to_v15() {
        assert_eq!(PROVIDED_INPUTS, 0xf800);
        assert_eq!(instance_attributes(), [(11, 0), (12, 16), (13, 32), (14, 48), (15, 64)]);
        assert_eq!(INSTANCE_STRIDE, 80);
    }
}
