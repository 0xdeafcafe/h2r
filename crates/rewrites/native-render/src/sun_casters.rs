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
//!
//! A shadow's receivers are what its layer 11 draws on: objects, skinned
//! the same way, and the structure's cluster and placement parts, from
//! the world the loader offers for the scene's BSP ([`SunWorld`]).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use frame_scene::{FrameScene, InstanceKey, LayerOwner, NodeMatrix, View};

use crate::model::Model;
use crate::sun_tiles::{SunCaster, floats_hash};
use crate::world::{Aabb, Owner, World};

/// Retail draws an object shadow only when its weight exceeds this
/// (`0x00032438`).
pub const DRAW_THRESHOLD: f32 = frame_scene::ObjectShadow::DRAW_THRESHOLD;

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

/// The structure's parts as sun tile receivers: a structure cluster's or
/// an instanced placement's part, by the owner and part index a layer
/// item names ([`frame_scene::LightStream::part`]).
pub trait WorldReceivers {
    /// Whether this is still the world of the BSP the guest has active.
    /// A world of another BSP names other clusters by the same indices.
    fn current(&self) -> bool {
        true
    }

    /// Opaque parts of visible clusters whose bounds meet the shadow projection.
    fn shadow_parts(&self, _clusters: &[u16], _caster: &SunCaster) -> Vec<ReceiverMesh> {
        Vec::new()
    }

    /// Part `part` of `owner`'s section in world space, when held.
    fn part(&self, owner: Owner, part: u8) -> Option<WorldPart>;
}

/// One structure part a tile may fall on.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldPart {
    pub mesh: Arc<CasterMesh>,
    /// The sphere around the part's bounds.
    pub centre: [f32; 3],
    pub radius: f32,
}

/// The world one BSP's loader shares with the render thread.
pub type SharedWorld = Arc<dyn WorldReceivers + Send + Sync>;

/// The current map's world receivers with the map epoch they load for.
static WORLD: Mutex<Option<(u64, SharedWorld)>> = Mutex::new(None);

/// The world loader offers map `map_epoch`'s structure to the render
/// thread, or withdraws it (`None`). Only the frame graph's E3 sun tiles
/// read it ([`world_for`]); retail never does.
pub fn set_world(map_epoch: u64, world: Option<SharedWorld>) {
    let mut slot = WORLD.lock().unwrap_or_else(|e| e.into_inner());
    match world {
        Some(w) => *slot = Some((map_epoch, w)),
        // An older map's loader does not withdraw a newer map's world.
        None => {
            if slot.as_ref().is_some_and(|(epoch, _)| *epoch == map_epoch) {
                *slot = None;
            }
        }
    }
}

/// The world receivers for a scene of map `map_epoch`; `None` until that
/// map's BSP has loaded, for any other map's scene, and once the guest
/// has left the BSP it was loaded for ([`WorldReceivers::current`]).
pub fn world_for(map_epoch: u64) -> Option<SharedWorld> {
    let slot = WORLD.lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref()
        .filter(|(epoch, w)| *epoch == map_epoch && w.current())
        .map(|(_, w)| Arc::clone(w))
}

/// One structure part's triangles in [`SunWorld`]'s lists.
#[derive(Clone, Debug)]
struct HeldPart {
    owner: Owner,
    part: u8,
    indices: Range<u32>,
    bounds: Aabb,
}

/// A loaded BSP's opaque parts, placed, as [`WorldReceivers`]: every
/// opaque part the world was built with, whether or not the native
/// lightmap pass draws it. A part's mesh is built at its first ask and
/// kept.
pub struct SunWorld {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
    /// Sorted by (owner, part).
    parts: Vec<HeldPart>,
    meshes: Mutex<HashMap<(Owner, u8), Arc<CasterMesh>>>,
    current: Option<Box<dyn Fn() -> bool + Send + Sync>>,
}

impl SunWorld {
    /// `world`'s opaque draws over its placed vertices.
    pub fn new(world: &World) -> Self {
        let mut indices = Vec::new();
        let mut parts = Vec::with_capacity(world.draws.len());
        for d in &world.draws {
            let start = d.first_index as usize;
            let end = start
                .saturating_add(d.index_count as usize)
                .min(world.indices.len());
            if start >= end {
                continue;
            }
            let first = indices.len() as u32;
            indices.extend_from_slice(&world.indices[start..end]);
            parts.push(HeldPart {
                owner: d.owner,
                part: d.part,
                indices: first..indices.len() as u32,
                bounds: d.bounds,
            });
        }
        parts.sort_by_key(|p| (p.owner, p.part));
        parts.dedup_by_key(|p| (p.owner, p.part));
        Self {
            positions: world.vertices.iter().map(|v| v.position).collect(),
            normals: world.vertices.iter().map(|v| v.normal).collect(),
            indices,
            parts,
            meshes: Mutex::new(HashMap::new()),
            current: None,
        }
    }

    /// The world, offered only while `current` holds: the loader's test
    /// that its BSP is still the guest's.
    pub fn with_current(mut self, current: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.current = Some(Box::new(current));
        self
    }

    fn mesh_of(&self, held: &HeldPart) -> CasterMesh {
        let mut mesh = CasterMesh::default();
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let range = held.indices.start as usize..held.indices.end as usize;
        for tri in self.indices[range].chunks_exact(3) {
            if tri.iter().any(|&i| i as usize >= self.positions.len()) {
                continue;
            }
            for &i in tri {
                let slot = *remap.entry(i).or_insert_with(|| {
                    mesh.positions.push(self.positions[i as usize]);
                    mesh.normals.push(self.normals[i as usize]);
                    mesh.positions.len() as u32 - 1
                });
                mesh.indices.push(slot);
            }
        }
        mesh
    }
}

impl WorldReceivers for SunWorld {
    fn current(&self) -> bool {
        self.current.as_ref().is_none_or(|f| f())
    }

    fn shadow_parts(&self, clusters: &[u16], caster: &SunCaster) -> Vec<ReceiverMesh> {
        let rows = crate::sun_tiles::tile_rows(caster.centre, caster.radius, caster.towards_light);
        let mut receivers = Vec::new();
        for &cluster in clusters {
            let owner = Owner::Cluster(u32::from(cluster));
            let first = self.parts.partition_point(|p| p.owner < owner);
            for p in self.parts[first..].iter().take_while(|p| p.owner == owner) {
                let (centre, radius) = box_sphere(&p.bounds);
                if crate::sun_tiles::meets(&rows, caster.radius, centre, radius)
                    && let Some(part) = self.part(owner, p.part)
                {
                    receivers.push(ReceiverMesh {
                        key: ReceiverKey::World(owner, p.part),
                        mesh: part.mesh,
                        centre,
                        radius,
                    });
                }
            }
        }
        receivers
    }

    fn part(&self, owner: Owner, part: u8) -> Option<WorldPart> {
        let at = self
            .parts
            .binary_search_by_key(&(owner, part), |p| (p.owner, p.part))
            .ok()?;
        let held = &self.parts[at];
        let mesh = {
            let mut meshes = self.meshes.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(
                meshes
                    .entry((owner, part))
                    .or_insert_with(|| Arc::new(self.mesh_of(held))),
            )
        };
        if mesh.indices.is_empty() {
            return None;
        }
        let (centre, radius) = box_sphere(&held.bounds);
        Some(WorldPart {
            mesh,
            centre,
            radius,
        })
    }
}

/// The sphere through a box's corners.
fn box_sphere(b: &Aabb) -> ([f32; 3], f32) {
    let centre = [0, 1, 2].map(|k| 0.5 * (b.min[k] + b.max[k]));
    let radius = (0..3)
        .map(|k| (b.max[k] - centre[k]).powi(2))
        .sum::<f32>()
        .sqrt();
    (centre, radius)
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
    /// Its layer 11 is not in the scene whole, or has no entry: what
    /// retail's receivers draw is not known.
    NoReceivers,
    /// A layer-11 receiver is a structure cluster's or an instanced
    /// placement's part the world receivers do not hold: no world is
    /// offered for the scene's BSP, or it has no such opaque part.
    WorldReceiver,
}

/// What a receiver is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReceiverKey {
    /// An object, by its handle.
    Object(InstanceKey),
    /// A structure cluster's or placement's part.
    World(Owner, u8),
}

/// One receiver of an object shadow's tile, in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceiverMesh {
    pub key: ReceiverKey,
    /// An object's is skinned for this scene. A structure part's is the
    /// world's own, the same allocation every frame while that world is
    /// offered.
    pub mesh: Arc<CasterMesh>,
    pub centre: [f32; 3],
    pub radius: f32,
}

/// What object shadow `shadow` (into `scene.object_shadows`) draws its
/// receivers on, as layer 11 lists them: each object once, skinned like
/// a caster, and each structure part once, from `world`. Refused when
/// the layer is not in the scene whole, when an object entry is not an
/// instance of the view with nodes and a loaded model, or when `world`
/// does not hold a structure entry's part.
pub fn receivers(
    scene: &FrameScene,
    shadow: usize,
    models: &dyn CasterModels,
    world: Option<&dyn WorldReceivers>,
) -> Result<Vec<ReceiverMesh>, CasterRefusal> {
    let o = scene
        .object_shadows
        .get(shadow)
        .ok_or(CasterRefusal::NoReceivers)?;
    let v = scene
        .views
        .get(o.view as usize)
        .ok_or(CasterRefusal::NoInstance)?;
    let group = scene
        .shadow_groups_of(o)
        .iter()
        .find(|g| g.layer == 11 && g.whole())
        .ok_or(CasterRefusal::NoReceivers)?;
    let entries = scene.entries_of(group);
    if entries.is_empty() {
        return Err(CasterRefusal::NoReceivers);
    }
    let instances = scene.instances_of(v);
    let mut out: Vec<ReceiverMesh> = Vec::new();
    for entry in entries {
        let part = entry.stream.part;
        let receiver = match entry.owner {
            LayerOwner::Object(key) => ReceiverKey::Object(key),
            LayerOwner::Cluster(c) => ReceiverKey::World(Owner::Cluster(u32::from(c)), part),
            LayerOwner::Placement(p) => ReceiverKey::World(Owner::Instance(u32::from(p)), part),
        };
        if out.iter().any(|r| r.key == receiver) {
            continue;
        }
        let key = match receiver {
            ReceiverKey::Object(key) => key,
            ReceiverKey::World(owner, part) => {
                let held = world
                    .and_then(|w| w.part(owner, part))
                    .ok_or(CasterRefusal::WorldReceiver)?;
                out.push(ReceiverMesh {
                    key: receiver,
                    mesh: held.mesh,
                    centre: held.centre,
                    radius: held.radius,
                });
                continue;
            }
        };
        let instance = instances
            .iter()
            .find(|i| i.key == key)
            .ok_or(CasterRefusal::NoInstance)?;
        let nodes = scene.nodes_of(instance);
        if nodes.is_empty() || !nodes.iter().all(NodeMatrix::is_finite) {
            return Err(CasterRefusal::NoNodes);
        }
        let loaded = models
            .model(instance.definition)
            .ok_or(CasterRefusal::NoModel)?;
        let mesh = skin(loaded, nodes);
        if mesh.indices.is_empty() {
            return Err(CasterRefusal::Empty);
        }
        let (centre, radius) = instance.sphere;
        out.push(ReceiverMesh {
            key: receiver,
            mesh: Arc::new(mesh),
            centre,
            radius,
        });
    }
    Ok(out)
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
    // The ranked shadow rows use absolute depth. A caster behind the eye
    // still has a finite apparent size for choosing its tile.
    let depth = c.depth_of(centre).abs().max(c.near.max(1e-3));
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
            let key = shadow.caster();
            let (definition, sphere, nodes) =
                if let Some(i) = instances.iter().find(|i| i.key == key) {
                    (i.definition, i.sphere, scene.nodes_of(i))
                } else {
                    let (_, c) = scene
                        .nearby_casters
                        .iter()
                        .find(|(v, c)| *v == view && c.handle == key.0)
                        .ok_or(CasterRefusal::NoInstance)?;
                    (c.definition, c.sphere, c.nodes.as_slice())
                };
            if nodes.is_empty() || !nodes.iter().all(NodeMatrix::is_finite) {
                return Err(CasterRefusal::NoNodes);
            }
            let loaded = models.model(definition).ok_or(CasterRefusal::NoModel)?;
            let towards_light = shadow
                .towards_light(scene)
                .ok_or(CasterRefusal::NoDirection)?;
            let weight = shadow.weight().ok_or(CasterRefusal::NoWeight)?;
            // Anything not above the threshold is refused, a NaN weight too.
            if weight.partial_cmp(&DRAW_THRESHOLD) != Some(std::cmp::Ordering::Greater) {
                return Err(CasterRefusal::Faint);
            }
            let mesh = skin(loaded, nodes);
            if mesh.indices.is_empty() {
                return Err(CasterRefusal::Empty);
            }
            let (centre, radius) = sphere;
            let words: &[f32] = bytemuck::cast_slice(nodes);
            Ok(Admitted {
                shadow: index,
                caster: SunCaster {
                    id: u64::from(key.0),
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
        GroupSite, InstanceInput, InstanceKey, LightBinding, LightEntry, LightGroup, LightPose,
        LightStream, PROJECTION_ROWS, RenderCamera, Row, SceneBuilder, SceneId, Stamp, Topology,
        ViewKey, ViewKind,
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
        shadow_scene_on(keys, shadows, &[])
    }

    /// An entry of a layer 11 drawn on `owner`.
    fn receiver_entry(owner: LayerOwner) -> (LightEntry, Vec<Row>) {
        let entry = LightEntry {
            slot: 0,
            record: 0,
            owner,
            model: 0,
            stream: LightStream {
                section: 0,
                part: 0,
                vertex_class: 0,
                first_index: 0,
                index_count: 3,
                topology: Topology::List,
            },
            pose: LightPose::World,
            binding: LightBinding {
                shader: 0,
                selector: 0,
                variant: 0,
                implementation: 3,
            },
        };
        (entry, Vec::new())
    }

    /// [`shadow_scene`], each shadow's layer 11 drawn on `on`.
    fn shadow_scene_on(
        keys: &[u32],
        shadows: &[(u32, bool, Option<f32>)],
        on: &[LayerOwner],
    ) -> FrameScene {
        let entries: Vec<_> = on.iter().map(|&o| receiver_entry(o)).collect();
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
                pass: None,
                entries: 0..0,
                remainder: 0,
            };
            b.push_shadow_group(group, &entries).unwrap();
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

    /// A shadow's receivers are its layer 11's objects, each once, skinned.
    #[test]
    fn receivers_are_layer_11s_objects() {
        let models = Models(HashMap::from([(DEFINITION, triangle_model())]));
        let objects = [
            LayerOwner::Object(InstanceKey(1)),
            LayerOwner::Object(InstanceKey(2)),
            LayerOwner::Object(InstanceKey(1)),
        ];
        let s = shadow_scene_on(&[1, 2], &[(1, false, Some(0.5))], &objects);
        let got = receivers(&s, 0, &models, None).unwrap();
        assert_eq!(
            got.iter().map(|r| r.key).collect::<Vec<_>>(),
            [
                ReceiverKey::Object(InstanceKey(1)),
                ReceiverKey::Object(InstanceKey(2))
            ]
        );
        assert_eq!(got[0].mesh.positions[0], [10.0, 0.0, 1.0]);
        assert_eq!((got[0].centre, got[0].radius), ([10.5, 0.5, 1.0], 1.0));
        let s = shadow_scene(&[1], &[(1, false, Some(0.5))]);
        assert_eq!(
            receivers(&s, 0, &models, None),
            Err(CasterRefusal::NoReceivers)
        );
        let s = shadow_scene_on(&[1], &[(1, false, Some(0.5))], &objects[1..2]);
        assert_eq!(
            receivers(&s, 0, &models, None),
            Err(CasterRefusal::NoInstance)
        );
    }

    /// A world of two sections: cluster 4's part 1 a unit quad at z = 0
    /// (part 0 transparent), and placement 2's part 0 the same quad moved
    /// to x = 5 and turned a quarter about z.
    fn two_section_world() -> World {
        use crate::world::{PartInput, Placement, SectionInput, WorldBuilder};
        let quad = |transparent_first: bool| SectionInput {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
            uvs: vec![[0.0; 2]; 4],
            normals: vec![[1.0, 0.0, 0.0]; 4],
            parts: if transparent_first {
                vec![
                    PartInput {
                        material: 1,
                        transparent: true,
                        triangles: vec![[0, 1, 2]],
                    },
                    PartInput {
                        material: 0,
                        transparent: false,
                        triangles: vec![[0, 1, 2], [1, 3, 2]],
                    },
                ]
            } else {
                vec![PartInput {
                    material: 0,
                    transparent: false,
                    triangles: vec![[0, 1, 2]],
                }]
            },
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(4), &quad(true), &Placement::IDENTITY, None);
        let turned = Placement {
            scale: 1.0,
            basis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            position: [5.0, 0.0, 0.0],
        };
        b.add_section(Owner::Instance(2), &quad(false), &turned, None);
        b.finish()
    }

    /// Cluster and placement entries draw on the world's own parts, placed,
    /// each once and the same mesh every ask; a part the world does not
    /// hold, a transparent one, or no world at all refuses the shadow by
    /// name.
    #[test]
    fn world_entries_receive_from_the_loaded_structure() {
        let models = Models(HashMap::from([(DEFINITION, triangle_model())]));
        let world = SunWorld::new(&two_section_world());
        let entry = |owner, part| {
            let (mut e, rows) = receiver_entry(owner);
            e.stream.part = part;
            (e, rows)
        };
        let scene_with = |on: &[(LayerOwner, u8)]| {
            let mut s = shadow_scene_on(&[1], &[(1, false, Some(0.5))], &[]);
            let start = s.light_entries.len() as u32;
            for &(owner, part) in on {
                s.light_entries.push(entry(owner, part).0);
            }
            s.shadow_groups[0].entries = start..s.light_entries.len() as u32;
            s
        };
        let s = scene_with(&[
            (LayerOwner::Cluster(4), 1),
            (LayerOwner::Placement(2), 0),
            (LayerOwner::Cluster(4), 1),
            (LayerOwner::Object(InstanceKey(1)), 0),
        ]);
        let got = receivers(&s, 0, &models, Some(&world)).unwrap();
        assert_eq!(
            got.iter().map(|r| r.key).collect::<Vec<_>>(),
            [
                ReceiverKey::World(Owner::Cluster(4), 1),
                ReceiverKey::World(Owner::Instance(2), 0),
                ReceiverKey::Object(InstanceKey(1)),
            ]
        );
        let cluster = &got[0];
        assert_eq!(
            cluster.mesh.indices.len(),
            6,
            "the opaque part's two triangles"
        );
        assert_eq!(cluster.mesh.positions.len(), 4);
        assert_eq!(cluster.mesh.normals[0], [1.0, 0.0, 0.0]);
        assert_eq!(cluster.centre, [0.5, 0.5, 0.0]);
        assert!((cluster.radius - 0.5f32.sqrt()).abs() < 1e-6);
        let placed = &got[1];
        assert_eq!(
            placed.mesh.positions,
            [[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [4.0, 0.0, 0.0]],
            "placed by the placement's transform"
        );
        assert_eq!(placed.mesh.normals[0], [0.0, 1.0, 0.0], "and turned");
        let again = receivers(&s, 0, &models, Some(&world)).unwrap();
        assert!(Arc::ptr_eq(&again[0].mesh, &cluster.mesh), "built once");

        for missing in [(LayerOwner::Cluster(4), 0), (LayerOwner::Cluster(5), 1)] {
            let s = scene_with(&[missing]);
            assert_eq!(
                receivers(&s, 0, &models, Some(&world)),
                Err(CasterRefusal::WorldReceiver),
                "{missing:?}"
            );
        }
        let s = scene_with(&[(LayerOwner::Cluster(4), 1)]);
        assert_eq!(
            receivers(&s, 0, &models, None),
            Err(CasterRefusal::WorldReceiver)
        );
    }

    /// The render thread reads a map's world only for that map's scenes,
    /// and only while its BSP is the guest's.
    #[test]
    fn world_receivers_serve_only_their_maps_current_bsp() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let current = Arc::new(AtomicBool::new(true));
        let held = Arc::clone(&current);
        let world: SharedWorld = Arc::new(
            SunWorld::new(&two_section_world()).with_current(move || held.load(Ordering::Relaxed)),
        );
        set_world(0x5ca2_0001, Some(world));
        assert!(world_for(0x5ca2_0001).is_some_and(|w| w.part(Owner::Cluster(4), 1).is_some()));
        assert!(world_for(0x5ca2_0002).is_none());
        current.store(false, Ordering::Relaxed);
        assert!(world_for(0x5ca2_0001).is_none(), "another BSP is active");
        current.store(true, Ordering::Relaxed);
        set_world(0x5ca2_0000, None);
        assert!(world_for(0x5ca2_0001).is_some());
        set_world(0x5ca2_0001, None);
        assert!(world_for(0x5ca2_0001).is_none());
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
