//! Writing a scene: the one writer is the sim thread, during the frame
//! step (docs/graphics/framescene.md, section 4.5). Views are written one
//! at a time, so each view's ranges are contiguous; a view is sealed when
//! the next opens or the scene seals; a sealed scene is immutable
//! (`Arc<FrameScene>`), and its arrays come back through a [`FreeList`]
//! when its last reader drops it.

use std::sync::{Arc, Mutex};

use crate::camera::Camera;
use crate::layer::{LayerItem, LayerPass};
use crate::light::{LightStep, SceneLight};
use crate::light_group::{LightEntry, LightGroup, LightPose, ObjectShadow};
use crate::node::NodeMatrix;
use crate::object::{Defaults, ItemKey, Lighting};
use crate::scene::{
    AuxDepth, Cut, FrameScene, Instance, InstanceKey, PartDraw, PartFlags, Row, SceneId, Stamp,
    Subparts, View, ViewKey, WorldState,
};

/// Scenes whose readers are done, emptied, for the builder to fill again.
/// Clones share the list.
#[derive(Clone, Default)]
pub struct FreeList(Arc<Mutex<Vec<FrameScene>>>);

/// Scenes kept at most: two in flight and one being built.
const KEEP: usize = 3;

impl FreeList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives `scene` back when this was its last reference: emptied, its
    /// allocations kept. Otherwise another reader still holds it and its
    /// drop frees it.
    pub fn recycle(&self, scene: Arc<FrameScene>) {
        if let Ok(mut scene) = Arc::try_unwrap(scene) {
            scene.clear();
            let mut free = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if free.len() < KEEP {
                free.push(scene);
            }
        }
    }

    /// An empty scene, recycled when one is free.
    pub fn take(&self) -> FrameScene {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop()
            .unwrap_or_default()
    }

    /// Scenes waiting to be reused.
    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One object as extraction hands it to the builder.
#[derive(Clone, Copy, Debug)]
pub struct InstanceInput<'a> {
    pub key: InstanceKey,
    pub definition: u32,
    pub model: u32,
    pub nodes: &'a [NodeMatrix],
    pub sphere: ([f32; 3], f32),
    pub lighting: Option<Lighting>,
    pub defaults: Option<Defaults>,
}

/// Why the builder refused a write. The scene is left as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// No scene begun.
    NoScene,
    /// No view open.
    NoView,
    /// A part for an instance that is not the open view's last.
    NotLastInstance,
    /// A part's slot not after the instance's previous part's.
    SlotOrder,
    /// A layer item with no layer pass open in the view.
    NoLayer,
    /// A light group with no light in the open view.
    NoLight,
    /// An object-shadow group with no object shadow in the open view.
    NoShadow,
    /// A late layer other than 16 or 6, or one the open view does not have.
    NotLate,
    /// A post callee's call with no post passes open in the view.
    NoPost,
}

/// What the builder remembers of the scene before, for cuts.
#[derive(Clone, Debug, Default)]
struct Previous {
    map_epoch: Option<u64>,
    views: Vec<ViewKey>,
}

/// Writes scenes, one at a time.
#[derive(Default)]
pub struct SceneBuilder {
    free: FreeList,
    scene: Option<FrameScene>,
    /// The open view's index in the scene's views.
    open: Option<usize>,
    /// Cut reasons the next scene carries, beyond what the builder sees.
    pending_cut: Cut,
    previous: Previous,
}

impl SceneBuilder {
    pub fn new(free: FreeList) -> Self {
        Self {
            free,
            ..Default::default()
        }
    }

    pub fn free_list(&self) -> &FreeList {
        &self.free
    }

    /// Starts scene `id`. A scene begun and never sealed is dropped (its
    /// arrays kept): the frame never reached its present.
    pub fn begin(&mut self, id: SceneId) {
        let mut scene = match self.scene.take() {
            Some(mut s) => {
                s.clear();
                s
            }
            None => self.free.take(),
        };
        scene.id = id;
        self.scene = Some(scene);
        self.open = None;
    }

    pub fn is_building(&self) -> bool {
        self.scene.is_some()
    }

    /// Marks the next sealed scene a cut (a load, a revert, a BSP switch).
    pub fn cut(&mut self, why: Cut) {
        self.pending_cut |= why;
    }

    /// Retires the in-progress frame and carries the cut to the next seal.
    pub fn discard(&mut self, why: Cut) {
        self.open = None;
        self.scene = None;
        self.cut(why);
    }

    /// The view keys of the scene sealed before, sorted: with the open
    /// scene's own, the views a frame step expects this frame (the E3
    /// light choice's share, `native_render::light_choice::share_of`).
    pub fn previous_views(&self) -> &[ViewKey] {
        &self.previous.views
    }

    /// The scene being written.
    pub fn scene(&self) -> Option<&FrameScene> {
        self.scene.as_ref()
    }

    /// The open view.
    pub fn view(&self) -> Option<&View> {
        Some(&self.scene.as_ref()?.views[self.open?])
    }

    /// The scene and its open view, for a stage that draws during the
    /// frame step (slice 1: on the game thread, before the seal).
    pub fn open(&self) -> Option<(&FrameScene, &View)> {
        let scene = self.scene.as_ref()?;
        Some((scene, scene.views.get(self.open?)?))
    }

    /// Opens a view (sealing the open one). A view of a key already in
    /// the scene is a second record of it: the guest drew that view twice
    /// or moved its camera between two stages; each stage reads the view
    /// it was extracted with.
    pub fn open_view(&mut self, key: ViewKey, camera: Camera) -> Result<&View, BuildError> {
        self.seal_view();
        let scene = self.scene.as_mut().ok_or(BuildError::NoScene)?;
        let cut = !self.previous.views.contains(&key);
        let (c, i, o) = (
            scene.clusters.len() as u32,
            scene.instances.len() as u32,
            scene.order.len() as u32,
        );
        let l = scene.lights.len() as u32;
        let p = scene.layer_passes.len() as u32;
        let d = scene.decorators.len() as u32;
        let dc = scene.decals.len() as u32;
        scene.views.push(View {
            key,
            cut,
            camera,
            clusters: c..c,
            instances: i..i,
            order: o..o,
            world: WorldState::default(),
            remainder: 0,
            display_visibility: None,
            display_transparents: None,
            aux_depth: None,
            structure_parameters: None,
            lights: l..l,
            layers: p..p,
            decorators: d..d,
            decorator_remainder: 0,
            decals: dc..dc,
            decal_remainder: 0,
            effects: Default::default(),
            fog: None,
            water: None,
            water_composite: None,
            ocean: None,
            post: None,
        });
        scene.cluster_gathers.push(None);
        self.open = Some(scene.views.len() - 1);
        Ok(scene.views.last().expect("just pushed"))
    }

    /// Attach shadow-only poses to the open view without adding visible instances.
    pub fn add_nearby_casters(
        &mut self,
        casters: Vec<crate::NearbyCaster>,
    ) -> Result<(), BuildError> {
        let (scene, view) = self.open_mut()?;
        scene
            .nearby_casters
            .extend(casters.into_iter().map(|c| (view as u32, c)));
        Ok(())
    }

    /// The scene being written, for a write that belongs to no view (the
    /// overlay, `crate::overlay`).
    pub(crate) fn scene_mut(&mut self) -> Result<&mut FrameScene, BuildError> {
        self.scene.as_mut().ok_or(BuildError::NoScene)
    }

    pub(crate) fn open_mut(&mut self) -> Result<(&mut FrameScene, usize), BuildError> {
        let scene = self.scene.as_mut().ok_or(BuildError::NoScene)?;
        let open = self.open.ok_or(BuildError::NoView)?;
        Ok((scene, open))
    }

    /// The scene and the index of its last view of `key`, open or sealed:
    /// a write that does not extend a view's ranges (the late passes,
    /// `crate::late`) may land in a view another has followed.
    pub(crate) fn keyed_mut(
        &mut self,
        key: ViewKey,
    ) -> Result<(&mut FrameScene, usize), BuildError> {
        let scene = self.scene.as_mut().ok_or(BuildError::NoScene)?;
        let index = scene
            .views
            .iter()
            .rposition(|v| v.key == key)
            .ok_or(BuildError::NoView)?;
        Ok((scene, index))
    }

    /// The open view's visible clusters, replacing any set before.
    pub fn set_clusters(
        &mut self,
        clusters: impl IntoIterator<Item = u16>,
    ) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        let start = scene.views[open].clusters.start;
        scene.clusters.truncate(start as usize);
        scene.clusters.extend(clusters);
        scene.views[open].clusters = start..scene.clusters.len() as u32;
        Ok(())
    }

    /// Record the main gather before the layer filters its cluster list.
    pub fn set_cluster_gather(&mut self, gather: crate::scene::ClusterGather) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.cluster_gathers[open] = Some(gather);
        Ok(())
    }

    pub fn set_display_transparents(&mut self, queue: impl Into<std::sync::Arc<crate::transparent::DisplayTransparentQueue>>) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].display_transparents = Some(queue.into());
        Ok(())
    }

    pub fn set_display_visibility(&mut self, visibility: crate::DisplayVisibility) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].display_visibility = Some(visibility.into());
        Ok(())
    }

    pub fn set_world(&mut self, world: WorldState) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].world = world;
        Ok(())
    }

    /// What the open view's auxiliary-depth parts draw with.
    pub fn set_aux_depth(&mut self, aux: AuxDepth) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].aux_depth = Some(aux);
        Ok(())
    }

    /// What the open view's [`crate::PartFlags::STRUCTURE_PARAMETERS`]
    /// parts' prologue uploads.
    pub fn set_structure_parameters(&mut self, parameters: crate::StructureParameters) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].structure_parameters = Some(parameters);
        Ok(())
    }

    /// Counts object items the guest keeps (refused by extraction).
    pub fn add_remainder(&mut self, items: u32) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].remainder += items;
        Ok(())
    }

    /// Adds an instance to the open view; its index in the scene.
    pub fn push_instance(&mut self, input: InstanceInput<'_>) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let n = scene.nodes.len() as u32;
        scene.nodes.extend_from_slice(input.nodes);
        let p = scene.parts.len() as u32;
        let index = scene.instances.len() as u32;
        scene.instances.push(Instance {
            key: input.key,
            definition: input.definition,
            model: input.model,
            nodes: n..scene.nodes.len() as u32,
            sphere: input.sphere,
            lighting: input.lighting,
            defaults: input.defaults,
            parts: p..p,
            teleported: false,
        });
        scene.views[open].instances.end = index + 1;
        Ok(index)
    }

    /// Adds a part to `instance`, which must be the open view's last;
    /// parts of one instance in increasing `slot`.
    pub fn push_part(
        &mut self,
        instance: u32,
        slot: u32,
        key: ItemKey,
        layer: u8,
        rows: &[Row],
    ) -> Result<u32, BuildError> {
        self.push_part_flagged(instance, slot, key, layer, PartFlags::NONE, None, rows)
    }

    /// [`Self::push_part`] with how the part draws beyond its material:
    /// its flags and, with [`PartFlags::SUBPARTS`], the subparts it draws.
    pub fn push_part_flagged(
        &mut self,
        instance: u32,
        slot: u32,
        key: ItemKey,
        layer: u8,
        flags: PartFlags,
        subparts: Option<Subparts>,
        rows: &[Row],
    ) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let view = &scene.views[open];
        if instance + 1 != view.instances.end || view.instances.is_empty() {
            return Err(BuildError::NotLastInstance);
        }
        let parts = scene.instances[instance as usize].parts.clone();
        if parts.end > parts.start && scene.parts[parts.end as usize - 1].slot >= slot {
            return Err(BuildError::SlotOrder);
        }
        let r = scene.rows.len() as u32;
        scene.rows.extend_from_slice(rows);
        let index = scene.parts.len() as u32;
        scene.parts.push(PartDraw {
            slot,
            key,
            layer,
            instance,
            rows: r..scene.rows.len() as u32,
            flags,
            subparts,
        });
        scene.instances[instance as usize].parts.end = index + 1;
        Ok(index)
    }

    /// Adds a light to the open view, after its lights so far, with its
    /// passes' steps in order; its index in the scene. The builder sets
    /// `light.steps`.
    pub fn push_light(
        &mut self,
        light: SceneLight,
        steps: &[LightStep],
    ) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let s = scene.light_steps.len() as u32;
        scene.light_steps.extend_from_slice(steps);
        let index = scene.lights.len() as u32;
        scene.lights.push(SceneLight {
            steps: s..scene.light_steps.len() as u32,
            guest_selected: true,
            ..light
        });
        scene.views[open].lights.end = index + 1;
        Ok(index)
    }

    /// Merges the visible candidates after the guest's last light pass.
    /// The guest's groups keep their indices and draw order. A host light
    /// ([`crate::Light::host_light`]) has no datum to match: it is always
    /// added, after the lights before it.
    pub fn add_light_candidates(
        &mut self,
        candidates: Vec<crate::Light>,
    ) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        let range = &mut scene.views[open].lights;
        let existing = range.start as usize..range.end as usize;
        let indices: std::collections::HashMap<_, _> = existing
            .filter(|&i| scene.lights[i].host.is_none())
            .map(|i| (scene.lights[i].handle, i))
            .collect();
        for candidate in candidates {
            let merged = candidate
                .host
                .is_none()
                .then(|| indices.get(&candidate.handle).copied())
                .flatten();
            if let Some(i) = merged {
                scene.lights[i].candidate = candidate.candidate;
            } else {
                let end = scene.light_steps.len() as u32;
                scene.lights.push(crate::Light {
                    steps: end..end,
                    ..candidate
                });
            }
        }
        range.end = scene.lights.len() as u32;
        Ok(())
    }

    /// Adds a layer draw to the open view, after its others; its items
    /// follow ([`Self::push_layer_item`]). The builder sets `pass.items`.
    pub fn push_layer(&mut self, pass: LayerPass) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let i = scene.layer_items.len() as u32;
        let index = scene.layer_passes.len() as u32;
        scene.layer_passes.push(LayerPass {
            items: i..i,
            ..pass
        });
        scene.views[open].layers.end = index + 1;
        Ok(index)
    }

    /// Adds an item, with its palette rows, to the open view's last layer
    /// draw. The builder sets `item.rows`.
    pub fn push_layer_item(&mut self, item: LayerItem, rows: &[Row]) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        if scene.views[open].layers.is_empty() {
            return Err(BuildError::NoLayer);
        }
        let r = scene.rows.len() as u32;
        scene.rows.extend_from_slice(rows);
        scene.layer_items.push(LayerItem {
            rows: r..scene.rows.len() as u32,
            ..item
        });
        let end = scene.layer_items.len() as u32;
        scene
            .layer_passes
            .last_mut()
            .expect("the view's last pass")
            .items
            .end = end;
        Ok(())
    }

    /// Items extraction left to the guest in the open view's last layer
    /// draw.
    pub fn add_layer_remainder(&mut self, items: u32) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        if scene.views[open].layers.is_empty() {
            return Err(BuildError::NoLayer);
        }
        scene
            .layer_passes
            .last_mut()
            .expect("the view's last pass")
            .remainder += items;
        Ok(())
    }

    /// Adds a gathered group to the open view's last light, after its
    /// groups so far, with its entries in the sorted order; its index in
    /// the scene. Each entry comes with the node rows of a
    /// [`LightPose::Rows`] pose (the builder sets the range; other poses
    /// take none). The builder sets `group.light` and `group.entries`.
    pub fn push_light_group(
        &mut self,
        group: LightGroup,
        entries: &[(LightEntry, Vec<Row>)],
    ) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let lights = &scene.views[open].lights;
        if lights.is_empty() || !scene.lights[lights.end as usize - 1].guest_selected {
            return Err(BuildError::NoLight);
        }
        let light = lights.end - 1;
        let entries = push_entries(scene, entries);
        let index = scene.light_groups.len() as u32;
        scene.light_groups.push(LightGroup {
            light,
            entries,
            ..group
        });
        Ok(index)
    }

    /// Adds an object shadow to the open view, after its others; its
    /// index in the scene. Its groups follow ([`Self::push_shadow_group`]).
    pub fn push_object_shadow(
        &mut self,
        identity: [u32; 2],
        creature: bool,
    ) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let g = scene.shadow_groups.len() as u32;
        let index = scene.object_shadows.len() as u32;
        scene.object_shadows.push(ObjectShadow {
            view: open as u32,
            identity,
            groups: g..g,
            creature,
            weight_bits: None,
            filter: None,
        });
        Ok(index)
    }

    /// Records the light weight of the open view's last object shadow
    /// ([`ObjectShadow::weight_bits`]).
    pub fn set_shadow_weight(&mut self, weight: f32) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        match scene.object_shadows.last_mut() {
            Some(o) if o.view as usize == open => {
                o.weight_bits = Some(weight.to_bits());
                Ok(())
            }
            _ => Err(BuildError::NoShadow),
        }
    }

    /// Records the filter of the open view's last object shadow.
    pub fn set_shadow_filter(&mut self, filter: crate::ShadowFilter) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        match scene.object_shadows.last_mut() {
            Some(o) if o.view as usize == open => {
                o.filter = Some(filter);
                Ok(())
            }
            _ => Err(BuildError::NoShadow),
        }
    }

    /// Adds a gathered group to the open view's last object shadow, as
    /// [`Self::push_light_group`] to a light.
    pub fn push_shadow_group(
        &mut self,
        group: LightGroup,
        entries: &[(LightEntry, Vec<Row>)],
    ) -> Result<u32, BuildError> {
        let (scene, open) = self.open_mut()?;
        let shadow = match scene.object_shadows.last() {
            Some(o) if o.view as usize == open => scene.object_shadows.len() as u32 - 1,
            _ => return Err(BuildError::NoShadow),
        };
        let entries = push_entries(scene, entries);
        let index = scene.shadow_groups.len() as u32;
        scene.shadow_groups.push(LightGroup {
            light: shadow,
            entries,
            ..group
        });
        scene.object_shadows[shadow as usize].groups.end = index + 1;
        Ok(index)
    }

    /// Seals the open view: its parts ordered by the guest's list.
    pub fn seal_view(&mut self) {
        let (Some(scene), Some(open)) = (self.scene.as_mut(), self.open.take()) else {
            return;
        };
        let instances = scene.views[open].instances.clone();
        let start = scene.order.len();
        for i in instances {
            let parts = scene.instances[i as usize].parts.clone();
            scene.order.extend(parts);
        }
        let parts = &scene.parts;
        scene.order[start..].sort_by_key(|&p| parts[p as usize].slot);
        scene.views[open].order = start as u32..scene.order.len() as u32;
    }

    /// Adds the visual effect parts created since the last scene.
    pub fn push_part_events(&mut self, events: impl IntoIterator<Item = crate::effect::PartEvent>) -> Result<(), BuildError> {
        let scene = self.scene.as_mut().ok_or(BuildError::NoScene)?;
        scene.part_events.extend(events);
        Ok(())
    }

    /// Seals the scene: the open view, the cut reasons (a new map epoch, a
    /// changed set of views, and any [`Self::cut`] asked), and the stamp.
    /// `None` when no scene was begun.
    pub fn seal(&mut self, stamp: Stamp) -> Option<Arc<FrameScene>> {
        self.seal_view();
        let mut scene = self.scene.take()?;
        let mut cut = std::mem::take(&mut self.pending_cut);
        if self
            .previous
            .map_epoch
            .is_some_and(|e| e != scene.id.map_epoch)
        {
            cut |= Cut::MAP;
        }
        let mut keys: Vec<ViewKey> = scene.views.iter().map(|v| v.key).collect();
        keys.sort();
        keys.dedup();
        if self.previous.map_epoch.is_some() && keys != self.previous.views {
            cut |= Cut::VIEWS;
        }
        scene.cut = cut;
        scene.stamp = stamp;
        self.previous = Previous {
            map_epoch: Some(scene.id.map_epoch),
            views: keys,
        };
        Some(Arc::new(scene))
    }
}

/// A group's entries into the scene, each [`LightPose::Rows`] pose's rows
/// with it; their range.
fn push_entries(
    scene: &mut FrameScene,
    entries: &[(LightEntry, Vec<Row>)],
) -> std::ops::Range<u32> {
    let e = scene.light_entries.len() as u32;
    for (entry, rows) in entries {
        let pose = match entry.pose {
            LightPose::Rows(_) => {
                let r = scene.rows.len() as u32;
                scene.rows.extend_from_slice(rows);
                LightPose::Rows(r..scene.rows.len() as u32)
            }
            ref other => other.clone(),
        };
        scene.light_entries.push(LightEntry {
            pose,
            ..entry.clone()
        });
    }
    e..scene.light_entries.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ViewKind;
    use crate::test_camera::camera;

    fn key(view: u8) -> ViewKey {
        ViewKey {
            kind: ViewKind::Game,
            player: 0,
            view,
        }
    }

    fn item(record: u16) -> ItemKey {
        ItemKey {
            record,
            shader: 0xe000_0010,
            section: 0,
            part: 0,
            implementation: 0,
        }
    }

    fn object<'a>(handle: u32, nodes: &'a [NodeMatrix]) -> InstanceInput<'a> {
        InstanceInput {
            key: InstanceKey(handle),
            definition: 0xe100_0000 | handle,
            model: 0xe200_0000 | handle,
            nodes,
            sphere: ([0.0; 3], 1.0),
            lighting: None,
            defaults: Some(Defaults::default()),
        }
    }

    fn rows(n: usize, x: f32) -> Vec<Row> {
        (0..n).map(|i| [x, i as f32, 0.0, 1.0]).collect()
    }

    /// Two views: the first with two objects (parts out of slot order
    /// across them), the second with one.
    fn two_views(b: &mut SceneBuilder) -> Arc<FrameScene> {
        b.begin(SceneId {
            map_epoch: 1,
            frame: 10,
            tick: 20,
        });
        b.open_view(key(0), camera()).unwrap();
        b.set_clusters([3, 1, 4]).unwrap();
        let a = b
            .push_instance(object(1, &[NodeMatrix::IDENTITY; 2]))
            .unwrap();
        b.push_part(a, 2, item(5), 3, &rows(3, 1.0)).unwrap();
        b.push_part(a, 7, item(6), 3, &rows(6, 2.0)).unwrap();
        let c = b.push_instance(object(2, &[NodeMatrix::IDENTITY])).unwrap();
        b.push_part(c, 4, item(8), 3, &rows(3, 3.0)).unwrap();
        b.add_remainder(2).unwrap();
        b.open_view(key(1), camera()).unwrap();
        let d = b
            .push_instance(object(3, &[NodeMatrix::IDENTITY; 3]))
            .unwrap();
        b.push_part(d, 0, item(9), 3, &rows(9, 4.0)).unwrap();
        b.seal(Stamp {
            published: None,
            ticks_run: 1,
        })
        .unwrap()
    }

    #[test]
    fn a_sealed_scene_has_whole_ranges_and_its_parts_in_list_order() {
        let mut b = SceneBuilder::default();
        let s = two_views(&mut b);
        s.check().unwrap();
        assert!(!b.is_building());
        assert_eq!(s.views.len(), 2);
        let v = &s.views[0];
        assert_eq!(s.clusters_of(v), &[3, 1, 4]);
        assert_eq!(s.instances_of(v).len(), 2);
        assert_eq!(v.remainder, 2);
        let slots: Vec<u32> = s.order[v.order.start as usize..v.order.end as usize]
            .iter()
            .map(|&p| s.parts[p as usize].slot)
            .collect();
        assert_eq!(slots, [2, 4, 7]);
        let (instance, part) = s.part_at(v, 4).unwrap();
        assert_eq!(instance.key, InstanceKey(2));
        assert_eq!(s.rows_of(part), rows(3, 3.0).as_slice());
        assert!(s.part_at(v, 3).is_none());
        let counts = s.counts(v);
        assert_eq!(
            (counts.clusters, counts.instances, counts.parts, counts.rows),
            (3, 2, 3, 12)
        );
        let w = &s.views[1];
        assert!(s.clusters_of(w).is_empty());
        assert_eq!(s.counts(w).rows, 9);
        assert_eq!(s.nodes_of(&s.instances_of(w)[0]).len(), 3);
        assert!(s.part_at(w, 2).is_none(), "another view's slot");
    }

    #[test]
    fn a_part_keeps_its_flags_and_the_view_counts_them_by_name() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(key(0), camera()).unwrap();
        let a = b.push_instance(object(1, &[NodeMatrix::IDENTITY])).unwrap();
        b.push_part(a, 1, item(1), 3, &rows(3, 1.0)).unwrap();
        b.push_part_flagged(
            a,
            2,
            item(2),
            3,
            PartFlags::STENCIL_MARK,
            None,
            &rows(3, 1.0),
        )
        .unwrap();
        b.push_part_flagged(
            a,
            3,
            item(3),
            3,
            PartFlags::STENCIL_MARK | PartFlags::STRIP,
            None,
            &rows(3, 1.0),
        )
        .unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        let flags: Vec<PartFlags> = s
            .parts_of(&s.instances_of(v)[0])
            .iter()
            .map(|p| p.flags)
            .collect();
        assert_eq!(
            flags,
            [PartFlags::NONE, PartFlags::STENCIL_MARK, PartFlags(3)]
        );
        assert_eq!(s.counts(v).flagged, [("stencil_mark", 2), ("strip", 1)]);
        assert_eq!(PartFlags(3).names(), ["stencil_mark", "strip"]);
        assert!(PartFlags::NONE.names().is_empty());
    }

    #[test]
    fn a_subpart_part_keeps_its_visible_subparts() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(key(0), camera()).unwrap();
        let a = b.push_instance(object(1, &[NodeMatrix::IDENTITY])).unwrap();
        let visible = Subparts::of(6, 5, [1, 2, 4]);
        assert_eq!(visible.visible, [0b1_0110]);
        b.push_part_flagged(
            a,
            1,
            item(1),
            3,
            PartFlags::SUBPARTS,
            Some(visible.clone()),
            &rows(3, 1.0),
        )
        .unwrap();
        b.push_part(a, 2, item(2), 3, &rows(3, 1.0)).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        let (_, p) = s.part_at(v, 1).unwrap();
        assert_eq!(p.subparts, Some(visible));
        assert_eq!(p.subparts.as_ref().unwrap().drawn().collect::<Vec<_>>(), [1, 2, 4]);
        assert_eq!(s.part_at(v, 2).unwrap().1.subparts, None, "the whole part");
        assert_eq!(s.counts(v).flagged, [("subparts", 1)]);
        // Bits past the part's own subparts draw nothing.
        let all = |count| Subparts { first: 0, count, visible: vec![u64::MAX; 2] };
        assert_eq!(all(2).drawn().count(), 2);
        assert_eq!(all(0).drawn().count(), 0);
    }

    /// A part of more than 64 subparts keeps every one it chose.
    #[test]
    fn a_part_of_a_hundred_subparts_keeps_every_one_it_chose() {
        let chosen: Vec<u16> = (0..100).filter(|n| n % 3 == 0 || *n >= 90).collect();
        let s = Subparts::of(10, 100, chosen.iter().copied());
        assert_eq!(s.visible.len(), 2);
        assert_eq!(s.drawn().collect::<Vec<_>>(), chosen);
        assert!(s.drawn().any(|n| n == 99), "past the first word");
    }

    #[test]
    fn a_views_lights_keep_their_steps_in_order_and_count_their_shadows() {
        use crate::light::{LightStep, LightTarget, SceneLight};
        let step = |layer, stencil, target| LightStep {
            layer,
            stencil,
            target,
            draws: true,
        };
        let light = |handle, flags| SceneLight {
            handle,
            definition: 0xe300_0000,
            flags,
            definition_flags: 0,
            additional_casters: false,
            steps: 0..0,
            ..Default::default()
        };
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(
            b.push_light(light(1, 0), &[]).unwrap_err(),
            BuildError::NoView
        );
        b.open_view(key(0), camera()).unwrap();
        let shadowed = [
            step(10, None, LightTarget::ShadowMap),
            step(11, Some(0), LightTarget::View),
            step(14, Some(0), LightTarget::Target(1)),
            step(12, Some(0), LightTarget::View),
        ];
        b.push_light(light(7, 4), &shadowed).unwrap();
        b.push_light(
            light(8, 3),
            &[
                step(14, Some(0), LightTarget::Target(1)),
                step(12, Some(0), LightTarget::View),
            ],
        )
        .unwrap();
        b.open_view(key(1), camera()).unwrap();
        b.push_light(light(9, 0), &[step(12, Some(0), LightTarget::View)])
            .unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        let lights = s.lights_of(v);
        assert_eq!(lights.iter().map(|l| l.handle).collect::<Vec<_>>(), [7, 8]);
        assert_eq!(s.steps_of(&lights[0]), &shadowed);
        assert_eq!(
            s.steps_of(&lights[1])
                .iter()
                .map(|s| s.layer)
                .collect::<Vec<_>>(),
            [14, 12]
        );
        assert_eq!((s.counts(v).lights, s.counts(v).shadows), (2, 1));
        let w = &s.views[1];
        assert_eq!((s.counts(w).lights, s.counts(w).shadows), (1, 0));
        assert_eq!(s.steps_of(&s.lights_of(w)[0]).len(), 1);
        let mut bad = (*s).clone();
        bad.lights[0].steps = 0..100;
        assert!(bad.check().unwrap_err().contains("steps"));
        let mut bad = (*s).clone();
        bad.views[1].lights = 1..3;
        assert!(bad.check().is_err());
    }

    /// Host lights (docs/game/forge-monitor-appearance.md, ML5) join the
    /// open view after its candidates, one a seat even with no datum to
    /// tell them apart; they have no steps and no shadow, and a game
    /// light's candidate still merges by its datum.
    #[test]
    fn host_lights_join_the_view_last_with_no_steps_and_no_shadow() {
        use crate::light::{LightStep, LightTarget, NO_DATUM, SceneLight};
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(key(0), camera()).unwrap();
        let guest = SceneLight {
            handle: 7,
            definition: 0xe300_0000,
            flags: 4,
            steps: 0..0,
            ..Default::default()
        };
        let shadow = LightStep {
            layer: 10,
            stencil: None,
            target: LightTarget::ShadowMap,
            draws: true,
        };
        b.push_light(guest.clone(), &[shadow]).unwrap();
        let mut merged = SceneLight::host_light(9, [0.0; 3], [1.0; 3], 1.0);
        merged.handle = 7;
        merged.host = None;
        let green = [0.0, 1.0, 0.0];
        b.add_light_candidates(vec![
            merged,
            SceneLight::host_light(1, [1.0, 2.0, 3.0], green, 2.0),
            SceneLight::host_light(3, [4.0, 5.0, 6.0], green, 4.0),
        ])
        .unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let lights = s.lights_of(&s.views[0]);
        assert_eq!(lights.len(), 3, "the merged candidate adds no light");
        assert_eq!(lights[0].handle, 7);
        assert!(lights[0].candidate.is_some());
        assert_eq!(
            lights[1..].iter().map(|l| (l.handle, l.definition, l.host)).collect::<Vec<_>>(),
            [(NO_DATUM, NO_DATUM, Some(1)), (NO_DATUM, NO_DATUM, Some(3))]
        );
        for l in &lights[1..] {
            assert!(!l.shadowed() && !l.guest_selected);
            assert!(s.steps_of(l).is_empty(), "no shadow map, no guest step");
            let c = l.candidate.as_ref().unwrap();
            assert_eq!((c.colour_a, c.colour_b), (green, green));
            assert!(c.constants.is_none());
        }
        assert_eq!(s.counts(&s.views[0]).shadows, 1, "only the game's light");
    }

    #[test]
    fn a_views_layer_draws_keep_their_items_and_rows_and_a_first_person_view_is_its_own() {
        use crate::layer::{LayerItem, LayerOwner, LayerPass};
        let pass = |layer| LayerPass {
            layer,
            stencil_off: false,
            texaccum: layer == 1,
            items: 0..0,
            remainder: 0,
            inputs: Default::default(),
        };
        let entry = |slot, owner| LayerItem {
            slot,
            key: item(1),
            level: 0, pass: 0, owner,
            model: u32::MAX,
            rows: 0..0,
            lighting: None,
            flags: crate::PartFlags::NONE,
            subparts: None,
            refusal: None,
            prt: None,
        };
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(key(0), camera()).unwrap();
        assert_eq!(
            b.push_layer_item(entry(0, LayerOwner::Cluster(1)), &[])
                .unwrap_err(),
            BuildError::NoLayer
        );
        assert_eq!(b.add_layer_remainder(1).unwrap_err(), BuildError::NoLayer);
        b.push_layer(pass(1)).unwrap();
        b.push_layer_item(entry(0, LayerOwner::Cluster(1)), &[])
            .unwrap();
        b.push_layer_item(entry(4, LayerOwner::Object(InstanceKey(7))), &rows(3, 5.0))
            .unwrap();
        b.add_layer_remainder(2).unwrap();
        b.push_layer(pass(5)).unwrap();
        let first_person = ViewKey {
            kind: ViewKind::FirstPerson,
            ..key(0)
        };
        b.open_view(first_person, camera()).unwrap();
        assert_eq!(
            b.push_layer_item(entry(0, LayerOwner::Cluster(1)), &[])
                .unwrap_err(),
            BuildError::NoLayer,
            "a sealed view's"
        );
        b.push_layer(pass(3)).unwrap();
        b.push_layer_item(entry(2, LayerOwner::Object(InstanceKey(9))), &rows(6, 1.0))
            .unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let v = &s.views[0];
        let passes = s.layers_of(v);
        assert_eq!(passes.iter().map(|p| p.layer).collect::<Vec<_>>(), [1, 5]);
        assert!(passes[0].texaccum && passes[0].remainder == 2);
        let items = s.items_of(&passes[0]);
        assert_eq!(items.iter().map(|i| i.slot).collect::<Vec<_>>(), [0, 4]);
        assert!(items[0].rows.is_empty());
        assert_eq!(
            &s.rows[items[1].rows.start as usize..items[1].rows.end as usize],
            rows(3, 5.0).as_slice()
        );
        assert!(s.items_of(&passes[1]).is_empty());
        assert_eq!(s.counts(v).layers, [(1, 2, 2), (5, 0, 0)]);
        let w = &s.views[1];
        assert_eq!(w.key.to_string(), "first_person:0:0");
        assert_eq!(s.counts(w).layers, [(3, 1, 0)]);
        // Each pass finds the view it was drawn in; a copy is no pass of
        // the scene's.
        assert_eq!(s.view_of_pass(&passes[1]).map(|v| v.key), Some(v.key));
        assert_eq!(s.view_of_pass(&s.layers_of(w)[0]).map(|v| v.key), Some(w.key));
        assert!(s.view_of_pass(&passes[0].clone()).is_none());
        let mut bad = (*s).clone();
        bad.layer_items[1].rows = 0..100;
        assert!(bad.check().unwrap_err().contains("rows"));
    }

    #[test]
    fn writes_need_a_scene_and_a_view_and_parts_go_to_the_last_instance_in_slot_order() {
        let mut b = SceneBuilder::default();
        assert_eq!(
            b.open_view(key(0), camera()).unwrap_err(),
            BuildError::NoScene
        );
        b.begin(SceneId::default());
        assert_eq!(
            b.push_instance(object(1, &[])).unwrap_err(),
            BuildError::NoView
        );
        b.open_view(key(0), camera()).unwrap();
        let a = b.push_instance(object(1, &[])).unwrap();
        let c = b.push_instance(object(2, &[])).unwrap();
        assert_eq!(
            b.push_part(a, 1, item(1), 3, &[]).unwrap_err(),
            BuildError::NotLastInstance
        );
        b.push_part(c, 5, item(1), 3, &[]).unwrap();
        assert_eq!(
            b.push_part(c, 5, item(2), 3, &[]).unwrap_err(),
            BuildError::SlotOrder
        );
        b.open_view(key(1), camera()).unwrap();
        assert_eq!(
            b.push_part(c, 9, item(3), 3, &[]).unwrap_err(),
            BuildError::NotLastInstance,
            "a sealed view's"
        );
        b.seal(Stamp::default()).unwrap().check().unwrap();
    }

    #[test]
    fn check_finds_a_broken_range() {
        let mut b = SceneBuilder::default();
        let s = two_views(&mut b);
        let mut bad = (*s).clone();
        bad.parts[0].rows = 0..1000;
        assert!(bad.check().unwrap_err().contains("rows"));
        let mut bad = (*s).clone();
        bad.views[1].instances = 0..3;
        assert!(bad.check().unwrap_err().contains("overlaps"));
        let mut bad = (*s).clone();
        bad.parts[1].instance = 1;
        assert!(bad.check().is_err());
    }

    #[test]
    fn cuts_follow_the_map_epoch_the_set_of_views_and_requests() {
        let mut b = SceneBuilder::default();
        let s = two_views(&mut b);
        assert!(
            s.cut.is_empty(),
            "the first scene has nothing before it to differ from"
        );
        assert!(s.views.iter().all(|v| v.cut), "no view before");
        let s = two_views(&mut b);
        assert!(s.cut.is_empty());
        assert!(s.views.iter().all(|v| !v.cut));
        b.cut(Cut::LOAD);
        b.begin(SceneId {
            map_epoch: 2,
            frame: 11,
            tick: 21,
        });
        b.open_view(key(0), camera()).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        assert_eq!(s.cut.names(), ["map", "load", "views"]);
        let s = two_views(&mut b);
        assert_eq!(s.cut.names(), ["map", "views"]);
        assert!(!s.views[0].cut && s.views[1].cut);
    }

    #[test]
    fn a_scene_never_sealed_is_dropped_at_the_next_begin() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId {
            map_epoch: 1,
            frame: 1,
            tick: 1,
        });
        b.open_view(key(0), camera()).unwrap();
        b.push_instance(object(1, &[NodeMatrix::IDENTITY])).unwrap();
        b.begin(SceneId {
            map_epoch: 1,
            frame: 2,
            tick: 2,
        });
        let s = b.seal(Stamp::default()).unwrap();
        assert!(s.views.is_empty() && s.instances.is_empty() && s.nodes.is_empty());
        assert_eq!(s.id.frame, 2);
        assert!(b.seal(Stamp::default()).is_none());
    }

    #[test]
    fn recycled_scenes_come_back_empty_with_their_allocations() {
        let free = FreeList::new();
        let mut b = SceneBuilder::new(free.clone());
        let s = two_views(&mut b);
        let (parts, rows) = (s.parts.len(), s.rows.len());
        let held = Arc::clone(&s);
        free.recycle(s);
        assert!(free.is_empty(), "a reader still holds it");
        free.recycle(held);
        assert_eq!(free.len(), 1);
        b.begin(SceneId::default());
        assert!(free.is_empty(), "the builder took it");
        let scene = b.scene().unwrap();
        assert!(scene.parts.is_empty() && scene.rows.is_empty() && scene.views.is_empty());
        assert!(scene.parts.capacity() >= parts && scene.rows.capacity() >= rows);
        for _ in 0..5 {
            free.recycle(Arc::new(FrameScene::default()));
        }
        assert_eq!(free.len(), KEEP);
    }
}
