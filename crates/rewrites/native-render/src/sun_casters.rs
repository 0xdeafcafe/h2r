//! E3 sun tiles' casters from a sealed scene
//! (docs/graphics/e3-lighting.md, "Sun shadows from objects").
//!
//! Each object shadow the guest drew in a view (`FrameScene::object_shadows`,
//! the calls to `0x00032350`) is a candidate. A candidate becomes a
//! [`SunCaster`] and a world-space mesh when its caster is an instance of
//! the view with a loaded model and node matrices, it takes the mask
//! branch (not a creature's ground shadow), and its layers left a light
//! direction and a weight above retail's draw threshold. Every other
//! candidate is refused by name and keeps retail's mask.
//!
//! The mesh is the instance's model skinned on the CPU with its node
//! matrices: every opaque part of the model as built (one permutation per
//! region, the loader's LOD), positions and normals in world space.

use std::sync::{Arc, Mutex};

use frame_scene::{FrameScene, NodeMatrix, View};

use crate::model::Model;
use crate::sun_tiles::{SunCaster, floats_hash};

/// Retail draws an object shadow only when its weight exceeds this
/// (`0x00032438`).
pub const DRAW_THRESHOLD: f32 = 0.09;

/// The loaded models, by object definition tag datum
/// ([`frame_scene::Instance::definition`]): each definition's render
/// model with its default variant's sections, as `crate::model_load`
/// picks them. A render model is shared by definitions whose variants
/// pick different sections, so the definition is the key.
pub trait CasterModels {
    fn model(&self, definition: u32) -> Option<&Model>;
}

/// The caster models one map's loader shares with the render thread.
pub type SharedModels = Arc<dyn CasterModels + Send + Sync>;

/// The current map's caster models with the map epoch they load for
/// (`frame_scene::SceneId::map_epoch`).
static MODELS: Mutex<Option<(u64, SharedModels)>> = Mutex::new(None);

/// The game's object loader offers map `map_epoch`'s caster models to the
/// render thread, or withdraws them (`None`). Only the frame graph's E3
/// sun tiles read them ([`models_for`]); retail never does.
pub fn set_models(map_epoch: u64, models: Option<SharedModels>) {
    let mut slot = MODELS.lock().unwrap_or_else(|e| e.into_inner());
    match models {
        Some(m) => *slot = Some((map_epoch, m)),
        // An older map's loader does not withdraw a newer map's models.
        None => {
            if slot.as_ref().is_some_and(|(epoch, _)| *epoch == map_epoch) {
                *slot = None;
            }
        }
    }
}

/// The caster models for a scene of map `map_epoch`; `None` until that
/// map's objects have loaded, and for any other map's scene.
pub fn models_for(map_epoch: u64) -> Option<SharedModels> {
    let slot = MODELS.lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref()
        .filter(|(epoch, _)| *epoch == map_epoch)
        .map(|(_, m)| Arc::clone(m))
}

/// Why an object shadow has no sun tile. It keeps retail's mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CasterRefusal {
    /// The ground-shadow branch `0x00032ce0`: the design keeps it.
    Creature,
    /// The caster is not an instance of the view.
    NoInstance,
    /// The instance has no node matrices, or one is not finite.
    NoNodes,
    /// No model is loaded for the instance.
    NoModel,
    /// Its layers read no projection.
    NoDirection,
    /// Its layer 11 read no weight.
    NoWeight,
    /// The weight is at or under [`DRAW_THRESHOLD`]: retail draws nothing.
    Faint,
    /// The skinned mesh has no triangle.
    Empty,
}

/// A caster's world-space triangles, for `sun_tiles_gpu::SunMesh::new`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CasterMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// One admitted object shadow.
#[derive(Clone, Debug, PartialEq)]
pub struct Admitted {
    /// Into `FrameScene::object_shadows`.
    pub shadow: usize,
    pub caster: SunCaster,
    pub mesh: CasterMesh,
}

/// A view's object shadows, admitted and refused, in the guest's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gathered {
    pub admitted: Vec<Admitted>,
    /// Into `FrameScene::object_shadows`, with the cause.
    pub refused: Vec<(usize, CasterRefusal)>,
}

impl Gathered {
    pub fn casters(&self) -> Vec<SunCaster> {
        self.admitted.iter().map(|a| a.caster).collect()
    }

    /// Whether object shadow `shadow` has a tile candidate: the frame
    /// draws its tile in place of retail's mask.
    pub fn admits(&self, shadow: usize) -> bool {
        self.admitted.iter().any(|a| a.shadow == shadow)
    }
}

/// A sphere's diameter in pixels of a view `height` pixels tall, under
/// `view`'s perspective camera.
pub fn view_diameter(view: &View, height: u32, centre: [f32; 3], radius: f32) -> f32 {
    let c = &view.camera;
    let f = c.forward;
    let n = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt().max(1e-6);
    let d = [
        centre[0] - c.position[0],
        centre[1] - c.position[1],
        centre[2] - c.position[2],
    ];
    let depth = ((d[0] * f[0] + d[1] * f[1] + d[2] * f[2]) / n).max(c.near.max(1e-3));
    let half = (0.5 * c.fov).tan().max(1e-6);
    radius * height as f32 / (depth * half)
}

/// `model` skinned by `nodes`: its opaque parts in world space.
pub fn skin(source: &Model, nodes: &[NodeMatrix]) -> CasterMesh {
    let mut mesh = CasterMesh::default();
    let mut remap = vec![u32::MAX; source.vertices.len()];
    for draw in &source.draws {
        let start = draw.first_index as usize;
        let end = start
            .saturating_add(draw.index_count as usize)
            .min(source.indices.len());
        for tri in source.indices[start.min(end)..end].chunks_exact(3) {
            if tri.iter().any(|&i| i as usize >= source.vertices.len()) {
                continue;
            }
            for &i in tri {
                let slot = &mut remap[i as usize];
                if *slot == u32::MAX {
                    let v = &source.vertices[i as usize];
                    *slot = mesh.positions.len() as u32;
                    mesh.positions.push(source.skin(v, nodes));
                    mesh.normals.push(skin_normal(source, v, nodes));
                }
                mesh.indices.push(*slot);
            }
        }
    }
    mesh
}

/// A vertex normal through its nodes' rotations, as [`Model::skin`] does
/// a position.
fn skin_normal(source: &Model, v: &crate::model::ModelVertex, nodes: &[NodeMatrix]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for k in 0..4 {
        let w = v.weights[k];
        if w <= 0.0 {
            continue;
        }
        let i = usize::from(v.nodes[k]);
        let (Some(n), Some(inv)) = (nodes.get(i), source.inverse_binds.get(i)) else {
            continue;
        };
        let r = n.vector(inv.vector(v.normal));
        for c in 0..3 {
            out[c] += w * r[c];
        }
    }
    out
}

/// The object shadows of view `view` (an index into `scene.views`) as
/// sun tile casters, for a view `height` pixels tall.
pub fn gather(scene: &FrameScene, view: u32, height: u32, models: &dyn CasterModels) -> Gathered {
    let mut out = Gathered::default();
    let Some(v) = scene.views.get(view as usize) else {
        return out;
    };
    let first = scene.object_shadows.partition_point(|o| o.view < view);
    let instances = scene.instances_of(v);
    for (k, shadow) in scene.object_shadows_of(view).iter().enumerate() {
        let index = first + k;
        let admitted = (|| {
            if shadow.creature {
                return Err(CasterRefusal::Creature);
            }
            let instance = instances
                .iter()
                .find(|i| i.key == shadow.caster())
                .ok_or(CasterRefusal::NoInstance)?;
            let nodes = scene.nodes_of(instance);
            if nodes.is_empty() || !nodes.iter().all(NodeMatrix::is_finite) {
                return Err(CasterRefusal::NoNodes);
            }
            let loaded = models
                .model(instance.definition)
                .ok_or(CasterRefusal::NoModel)?;
            let towards_light = shadow
                .towards_light(scene)
                .ok_or(CasterRefusal::NoDirection)?;
            let weight = shadow.weight().ok_or(CasterRefusal::NoWeight)?;
            if !(weight > DRAW_THRESHOLD) {
                return Err(CasterRefusal::Faint);
            }
            let mesh = skin(loaded, nodes);
            if mesh.indices.is_empty() {
                return Err(CasterRefusal::Empty);
            }
            let (centre, radius) = instance.sphere;
            let words: &[f32] = bytemuck::cast_slice(nodes);
            Ok(Admitted {
                shadow: index,
                caster: SunCaster {
                    id: u64::from(instance.key.0),
                    centre,
                    radius,
                    screen_diameter: view_diameter(v, height, centre, radius),
                    towards_light,
                    strength: weight.min(1.0),
                    rows: floats_hash(words),
                },
                mesh,
            })
        })();
        match admitted {
            Ok(a) => out.admitted.push(a),
            Err(cause) => out.refused.push((index, cause)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelDraw, ModelVertex};
    use frame_scene::{
        GroupSite, InstanceInput, InstanceKey, LightGroup, PROJECTION_ROWS, RenderCamera,
        SceneBuilder, SceneId, Stamp, ViewKey, ViewKind,
    };
    use std::collections::HashMap;

    struct Models(HashMap<u32, Model>);

    impl CasterModels for Models {
        fn model(&self, definition: u32) -> Option<&Model> {
            self.0.get(&definition)
        }
    }

    const DEFINITION: u32 = 0xe500_0001;

    /// One triangle on node 0, its normal up.
    fn triangle_model() -> Model {
        let v = |p: [f32; 3]| ModelVertex {
            position: p,
            normal: [0.0, 0.0, 1.0],
            nodes: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        Model {
            vertices: vec![v([0.0, 0.0, 0.0]), v([1.0, 0.0, 0.0]), v([0.0, 1.0, 0.0])],
            indices: vec![0, 1, 2],
            draws: vec![ModelDraw {
                material: 0,
                first_index: 0,
                index_count: 3,
            }],
            inverse_binds: vec![NodeMatrix::IDENTITY],
            ..Default::default()
        }
    }

    fn camera() -> RenderCamera {
        RenderCamera {
            position: [0.0, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: std::f32::consts::FRAC_PI_2,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        }
    }

    fn projection(c41: [f32; 3]) -> Option<[[u32; 4]; PROJECTION_ROWS]> {
        let mut p = [[0u32; 4]; PROJECTION_ROWS];
        p[1] = [c41[0].to_bits(), c41[1].to_bits(), c41[2].to_bits(), 0];
        Some(p)
    }

    /// A view with instances `keys` at x = 10, each moved up a unit, and
    /// object shadows for `shadows` (caster handle, creature, weight).
    fn shadow_scene(keys: &[u32], shadows: &[(u32, bool, Option<f32>)]) -> FrameScene {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey {
                kind: ViewKind::Game,
                player: 0,
                view: 0,
            },
            camera(),
        )
        .unwrap();
        let nodes = [NodeMatrix {
            position: [10.0, 0.0, 1.0],
            ..NodeMatrix::IDENTITY
        }];
        for &key in keys {
            b.push_instance(InstanceInput {
                key: InstanceKey(key),
                definition: DEFINITION,
                model: 0xe600_0002,
                nodes: &nodes,
                sphere: ([10.5, 0.5, 1.0], 1.0),
                lighting: None,
                defaults: None,
            })
            .unwrap();
        }
        for &(caster, creature, weight) in shadows {
            b.push_object_shadow([caster, 0x3f80_0000], creature)
                .unwrap();
            let group = LightGroup {
                light: 0,
                layer: 11,
                site: GroupSite::ObjectShadow,
                projection: projection([1.0, 0.0, 1.0]),
                entries: 0..0,
                remainder: 0,
            };
            b.push_shadow_group(group, &[]).unwrap();
            if let Some(w) = weight {
                b.set_shadow_weight(w).unwrap();
            }
        }
        (*b.seal(Stamp::default()).unwrap()).clone()
    }

    #[test]
    fn a_drawn_object_shadow_becomes_a_skinned_caster_with_its_light() {
        let models = Models(HashMap::from([(DEFINITION, triangle_model())]));
        let s = shadow_scene(&[0x8001_0001], &[(0x8001_0001, false, Some(0.75))]);
        let got = gather(&s, 0, 480, &models);
        assert!(got.refused.is_empty(), "{:?}", got.refused);
        assert!(got.admits(0));
        let a = &got.admitted[0];
        assert_eq!(a.caster.id, 0x8001_0001);
        assert_eq!(a.caster.towards_light, [1.0, 0.0, 1.0]);
        assert_eq!(a.caster.strength, 0.75);
        assert_eq!(
            a.mesh.positions,
            [[10.0, 0.0, 1.0], [11.0, 0.0, 1.0], [10.0, 1.0, 1.0]]
        );
        assert_eq!(a.mesh.normals, [[0.0, 0.0, 1.0]; 3]);
        assert_eq!(a.mesh.indices, [0, 1, 2]);
        // A unit sphere 10.5 units ahead under a 90-degree view, 480 tall.
        assert!(
            (a.caster.screen_diameter - 480.0 / 10.5).abs() < 0.01,
            "{}",
            a.caster.screen_diameter
        );
    }

    #[test]
    fn same_rows_hash_the_same_and_moved_rows_do_not() {
        let models = Models(HashMap::from([(DEFINITION, triangle_model())]));
        let s = shadow_scene(&[1], &[(1, false, Some(0.75))]);
        let a = gather(&s, 0, 480, &models).admitted[0].caster.rows;
        assert_eq!(gather(&s, 0, 480, &models).admitted[0].caster.rows, a);
        let mut moved = s.clone();
        moved.nodes[0].position[2] = 1.5;
        assert_ne!(gather(&moved, 0, 480, &models).admitted[0].caster.rows, a);
    }

    /// The render thread reads a map's caster models only for that map's
    /// scenes, and an older map's loader cannot withdraw a newer map's.
    #[test]
    fn caster_models_serve_only_their_maps_scenes() {
        let loaded: SharedModels =
            Arc::new(Models(HashMap::from([(DEFINITION, triangle_model())])));
        set_models(0x5ca1_0001, Some(loaded));
        assert!(models_for(0x5ca1_0001).is_some_and(|m| m.model(DEFINITION).is_some()));
        assert!(models_for(0x5ca1_0002).is_none());
        set_models(0x5ca1_0000, None);
        assert!(models_for(0x5ca1_0001).is_some());
        set_models(0x5ca1_0001, None);
        assert!(models_for(0x5ca1_0001).is_none());
    }

    /// Each refused shadow names its cause and keeps retail's mask.
    #[test]
    fn every_other_object_shadow_is_refused_by_name() {
        let models = Models(HashMap::from([(DEFINITION, triangle_model())]));
        let s = shadow_scene(
            &[1, 2],
            &[
                (1, true, Some(0.75)),
                (9, false, Some(0.75)),
                (1, false, None),
                (2, false, Some(0.05)),
                (2, false, Some(0.5)),
            ],
        );
        let got = gather(&s, 0, 480, &models);
        assert_eq!(
            got.refused,
            [
                (0, CasterRefusal::Creature),
                (1, CasterRefusal::NoInstance),
                (2, CasterRefusal::NoWeight),
                (3, CasterRefusal::Faint)
            ]
        );
        assert!(!got.admits(0) && got.admits(4));
        let none = gather(&s, 0, 480, &Models(HashMap::new()));
        assert_eq!(
            none.refused[1..]
                .iter()
                .filter(|r| r.1 == CasterRefusal::NoModel)
                .count(),
            3
        );
        let mut flat = s.clone();
        for g in &mut flat.shadow_groups {
            g.projection = None;
        }
        assert_eq!(
            gather(&flat, 0, 480, &models).refused[4],
            (4, CasterRefusal::NoDirection)
        );
    }
}
