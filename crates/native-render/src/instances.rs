//! One view's objects as the GPU draws them: every drawn object's node
//! matrices in one array, one instance record per object, and one draw per
//! (object, part), grouped by model and material. Pure: built on the render
//! thread from what the hook copied out of guest memory.

use bytemuck::{Pod, Zeroable};

use crate::cull::Frustum;
use crate::model::{Model, NodeMatrix};
use crate::world::Aabb;
use frame_scene::{FrameScene, Instance, View};

/// What the vertex shader reads per object.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct InstanceRecord {
    /// First node matrix of the object in the frame's node array.
    pub node_base: u32,
    /// The object's node count (indices past it read node 0).
    pub node_count: u32,
    pub _pad: [u32; 2],
}

/// One draw of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameDraw {
    /// Index of the model in the caller's model list.
    pub model: u32,
    pub material: u16,
    pub first_index: u32,
    pub index_count: u32,
    /// The instance record (`instance_index` in the shader).
    pub instance: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameObjects {
    pub nodes: Vec<NodeMatrix>,
    pub instances: Vec<InstanceRecord>,
    /// Sorted by (model, material).
    pub draws: Vec<FrameDraw>,
    pub culled: u32,
    /// Objects whose node count does not match their model.
    pub mismatched: u32,
}

/// One object to draw: its model (an index the caller resolves through
/// `models`), its node matrices and its world sphere.
pub struct ObjectInput<'a> {
    pub model: u32,
    pub nodes: &'a [NodeMatrix],
    pub sphere: ([f32; 3], f32),
}

/// Packs `objects` against `models`, dropping the ones `frustum` cannot see
/// (sphere test, conservative) and those whose node count disagrees with
/// their model's inverse rest frames.
pub fn pack<'a>(
    models: &dyn Fn(u32) -> Option<&'a Model>,
    objects: &[ObjectInput<'_>],
    frustum: Option<&Frustum>,
) -> FrameObjects {
    let mut out = FrameObjects::default();
    for o in objects {
        let Some(model) = models(o.model) else { continue };
        if o.nodes.len() != model.inverse_binds.len() {
            out.mismatched += 1;
            continue;
        }
        let (c, r) = o.sphere;
        // A sphere that is not finite proves nothing: drawn.
        let usable = r.is_finite() && r > 0.0 && c.iter().all(|x| x.is_finite());
        let visible = !usable
            || frustum.is_none_or(|f| {
                f.may_see(&Aabb {
                    min: [c[0] - r, c[1] - r, c[2] - r],
                    max: [c[0] + r, c[1] + r, c[2] + r],
                })
            });
        if !visible {
            out.culled += 1;
            continue;
        }
        let instance = out.instances.len() as u32;
        out.instances.push(InstanceRecord {
            node_base: out.nodes.len() as u32,
            node_count: o.nodes.len() as u32,
            _pad: [0; 2],
        });
        out.nodes.extend_from_slice(o.nodes);
        out.draws.extend(model.draws.iter().map(|d| FrameDraw {
            model: o.model,
            material: d.material,
            first_index: d.first_index,
            index_count: d.index_count,
            instance,
        }));
    }
    out.draws.sort_by_key(|d| (d.model, d.material, d.instance, d.first_index));
    out
}

/// A view's instances as [`pack`] takes them, in the view's order:
/// `model` resolves an instance to the caller's model index (`None`
/// leaves it out, as [`pack`] does a model it cannot find).
pub fn view_objects<'a>(
    scene: &'a FrameScene,
    view: &View,
    model: &dyn Fn(&Instance) -> Option<u32>,
) -> Vec<ObjectInput<'a>> {
    scene
        .instances_of(view)
        .iter()
        .filter_map(|i| Some(ObjectInput { model: model(i)?, nodes: scene.nodes_of(i), sphere: i.sphere }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::RenderCamera;
    use crate::model::{ModelDraw, ModelInput};

    fn model(nodes: usize, draws: &[(u16, u32)]) -> Model {
        Model {
            inverse_binds: vec![NodeMatrix::IDENTITY; nodes],
            draws: draws
                .iter()
                .map(|&(material, first_index)| ModelDraw {
                    material,
                    first_index,
                    index_count: 3,
                })
                .collect(),
            ..Model::build(&ModelInput::default())
        }
    }

    fn frustum() -> Frustum {
        let c = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 90f32.to_radians(),
            viewport: [0, 0, 100, 100],
            near: 0.1,
            far: 100.0,
        };
        Frustum::from_view_projection(&c.view_projection())
    }

    #[test]
    fn objects_pack_their_nodes_and_draws_and_culled_or_mismatched_ones_are_left_out() {
        let models = [model(2, &[(1, 0), (0, 3)]), model(1, &[(0, 0)])];
        let get = |i: u32| models.get(i as usize);
        let two = [NodeMatrix::IDENTITY; 2];
        let one = [NodeMatrix::IDENTITY; 1];
        let objects = [
            ObjectInput { model: 1, nodes: &one, sphere: ([10.0, 0.0, 0.0], 1.0) },
            ObjectInput { model: 0, nodes: &two, sphere: ([10.0, 1.0, 0.0], 1.0) },
            ObjectInput { model: 0, nodes: &two, sphere: ([-10.0, 0.0, 0.0], 1.0) },
            ObjectInput { model: 0, nodes: &one, sphere: ([10.0, 0.0, 0.0], 1.0) },
            ObjectInput { model: 7, nodes: &one, sphere: ([10.0, 0.0, 0.0], 1.0) },
        ];
        let f = pack(&get, &objects, Some(&frustum()));
        assert_eq!((f.culled, f.mismatched), (1, 1));
        assert_eq!(f.nodes.len(), 3);
        assert_eq!(
            f.instances.iter().map(|i| (i.node_base, i.node_count)).collect::<Vec<_>>(),
            vec![(0, 1), (1, 2)]
        );
        let d: Vec<_> = f.draws.iter().map(|d| (d.model, d.material, d.instance, d.first_index)).collect();
        assert_eq!(d, vec![(0, 0, 1, 3), (0, 1, 1, 0), (1, 0, 0, 0)]);
    }

    #[test]
    fn without_a_frustum_or_with_a_bad_radius_nothing_is_culled() {
        let models = [model(1, &[(0, 0)])];
        let get = |i: u32| models.get(i as usize);
        let one = [NodeMatrix::IDENTITY; 1];
        let behind = [ObjectInput { model: 0, nodes: &one, sphere: ([-10.0, 0.0, 0.0], 1.0) }];
        assert_eq!(pack(&get, &behind, None).instances.len(), 1);
        let nan = [ObjectInput { model: 0, nodes: &one, sphere: ([-10.0, 0.0, 0.0], f32::NAN) }];
        assert_eq!(pack(&get, &nan, Some(&frustum())).instances.len(), 1);
    }

    #[test]
    fn a_scene_view_packs_as_its_objects_do() {
        use frame_scene::{InstanceInput, InstanceKey, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};
        let models = [model(2, &[(1, 0), (0, 3)]), model(1, &[(0, 0)])];
        let get = |i: u32| models.get(i as usize);
        let two = [NodeMatrix::IDENTITY; 2];
        let one = [NodeMatrix::IDENTITY; 1];
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 90f32.to_radians(),
            viewport: [0, 0, 100, 100],
            near: 0.1,
            far: 100.0,
        };
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).unwrap();
        // Definitions 11 and 10 draw models 1 and 0; 12 has none.
        for (handle, definition, nodes, centre) in
            [(1, 11, &one[..], [10.0, 0.0, 0.0]), (2, 10, &two[..], [10.0, 1.0, 0.0]), (3, 10, &two[..], [-10.0, 0.0, 0.0]), (4, 12, &one[..], [10.0, 0.0, 0.0])]
        {
            b.push_instance(InstanceInput {
                key: InstanceKey(handle),
                definition,
                model: 0,
                nodes,
                sphere: (centre, 1.0),
                lighting: None,
                defaults: None,
            })
            .unwrap();
        }
        let scene = b.seal(Stamp::default()).unwrap();
        let view = &scene.views[0];
        let objects = view_objects(&scene, view, &|i| match i.definition {
            10 => Some(0),
            11 => Some(1),
            _ => None,
        });
        assert_eq!(objects.len(), 3);
        let frustum = Frustum::from_view_projection(&view.camera.view_projection());
        let f = pack(&get, &objects, Some(&frustum));
        assert_eq!((f.culled, f.mismatched), (1, 0));
        let d: Vec<_> = f.draws.iter().map(|d| (d.model, d.material, d.instance, d.first_index)).collect();
        assert_eq!(d, vec![(0, 0, 1, 3), (0, 1, 1, 0), (1, 0, 0, 0)]);
    }
}
