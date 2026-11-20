//! Writing a scene: the one writer is the sim thread, during the frame
//! step (docs/graphics/framescene.md, section 4.5). Views are written one
//! at a time, so each view's ranges are contiguous; a view is sealed when
//! the next opens or the scene seals; a sealed scene is immutable
//! (`Arc<FrameScene>`), and its arrays come back through a [`FreeList`]
//! when its last reader drops it.

use std::sync::{Arc, Mutex};

use crate::camera::Camera;
use crate::node::NodeMatrix;
use crate::object::{Defaults, ItemKey, Lighting};
use crate::scene::{Cut, FrameScene, Instance, InstanceKey, PartDraw, PartFlags, Row, SceneId, Stamp, View, ViewKey, WorldState};

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
        self.0.lock().unwrap_or_else(|e| e.into_inner()).pop().unwrap_or_default()
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
        Self { free, ..Default::default() }
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
        let (c, i, o) = (scene.clusters.len() as u32, scene.instances.len() as u32, scene.order.len() as u32);
        scene.views.push(View {
            key,
            cut,
            camera,
            clusters: c..c,
            instances: i..i,
            order: o..o,
            world: WorldState::default(),
            remainder: 0,
        });
        self.open = Some(scene.views.len() - 1);
        Ok(scene.views.last().expect("just pushed"))
    }

    fn open_mut(&mut self) -> Result<(&mut FrameScene, usize), BuildError> {
        let scene = self.scene.as_mut().ok_or(BuildError::NoScene)?;
        let open = self.open.ok_or(BuildError::NoView)?;
        Ok((scene, open))
    }

    /// The open view's visible clusters, replacing any set before.
    pub fn set_clusters(&mut self, clusters: impl IntoIterator<Item = u16>) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        let start = scene.views[open].clusters.start;
        scene.clusters.truncate(start as usize);
        scene.clusters.extend(clusters);
        scene.views[open].clusters = start..scene.clusters.len() as u32;
        Ok(())
    }

    pub fn set_world(&mut self, world: WorldState) -> Result<(), BuildError> {
        let (scene, open) = self.open_mut()?;
        scene.views[open].world = world;
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
    pub fn push_part(&mut self, instance: u32, slot: u32, key: ItemKey, layer: u8, rows: &[Row]) -> Result<u32, BuildError> {
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
            flags: PartFlags::default(),
        });
        scene.instances[instance as usize].parts.end = index + 1;
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

    /// Seals the scene: the open view, the cut reasons (a new map epoch, a
    /// changed set of views, and any [`Self::cut`] asked), and the stamp.
    /// `None` when no scene was begun.
    pub fn seal(&mut self, stamp: Stamp) -> Option<Arc<FrameScene>> {
        self.seal_view();
        let mut scene = self.scene.take()?;
        let mut cut = std::mem::take(&mut self.pending_cut);
        if self.previous.map_epoch.is_some_and(|e| e != scene.id.map_epoch) {
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
        self.previous = Previous { map_epoch: Some(scene.id.map_epoch), views: keys };
        Some(Arc::new(scene))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::camera;
    use crate::scene::ViewKind;

    fn key(view: u8) -> ViewKey {
        ViewKey { kind: ViewKind::Game, player: 0, view }
    }

    fn item(record: u16) -> ItemKey {
        ItemKey { record, shader: 0xe000_0010, section: 0, part: 0, implementation: 0 }
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
        b.begin(SceneId { map_epoch: 1, frame: 10, tick: 20 });
        b.open_view(key(0), camera()).unwrap();
        b.set_clusters([3, 1, 4]).unwrap();
        let a = b.push_instance(object(1, &[NodeMatrix::IDENTITY; 2])).unwrap();
        b.push_part(a, 2, item(5), 3, &rows(3, 1.0)).unwrap();
        b.push_part(a, 7, item(6), 3, &rows(6, 2.0)).unwrap();
        let c = b.push_instance(object(2, &[NodeMatrix::IDENTITY])).unwrap();
        b.push_part(c, 4, item(8), 3, &rows(3, 3.0)).unwrap();
        b.add_remainder(2).unwrap();
        b.open_view(key(1), camera()).unwrap();
        let d = b.push_instance(object(3, &[NodeMatrix::IDENTITY; 3])).unwrap();
        b.push_part(d, 0, item(9), 3, &rows(9, 4.0)).unwrap();
        b.seal(Stamp { published: None, ticks_run: 1 }).unwrap()
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
        let slots: Vec<u32> = s.order[v.order.start as usize..v.order.end as usize].iter().map(|&p| s.parts[p as usize].slot).collect();
        assert_eq!(slots, [2, 4, 7]);
        let (instance, part) = s.part_at(v, 4).unwrap();
        assert_eq!(instance.key, InstanceKey(2));
        assert_eq!(s.rows_of(part), rows(3, 3.0).as_slice());
        assert!(s.part_at(v, 3).is_none());
        let counts = s.counts(v);
        assert_eq!((counts.clusters, counts.instances, counts.parts, counts.rows), (3, 2, 3, 12));
        let w = &s.views[1];
        assert!(s.clusters_of(w).is_empty());
        assert_eq!(s.counts(w).rows, 9);
        assert_eq!(s.nodes_of(&s.instances_of(w)[0]).len(), 3);
        assert!(s.part_at(w, 2).is_none(), "another view's slot");
    }

    #[test]
    fn writes_need_a_scene_and_a_view_and_parts_go_to_the_last_instance_in_slot_order() {
        let mut b = SceneBuilder::default();
        assert_eq!(b.open_view(key(0), camera()).unwrap_err(), BuildError::NoScene);
        b.begin(SceneId::default());
        assert_eq!(b.push_instance(object(1, &[])).unwrap_err(), BuildError::NoView);
        b.open_view(key(0), camera()).unwrap();
        let a = b.push_instance(object(1, &[])).unwrap();
        let c = b.push_instance(object(2, &[])).unwrap();
        assert_eq!(b.push_part(a, 1, item(1), 3, &[]).unwrap_err(), BuildError::NotLastInstance);
        b.push_part(c, 5, item(1), 3, &[]).unwrap();
        assert_eq!(b.push_part(c, 5, item(2), 3, &[]).unwrap_err(), BuildError::SlotOrder);
        b.open_view(key(1), camera()).unwrap();
        assert_eq!(b.push_part(c, 9, item(3), 3, &[]).unwrap_err(), BuildError::NotLastInstance, "a sealed view's");
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
        assert!(s.cut.is_empty(), "the first scene has nothing before it to differ from");
        assert!(s.views.iter().all(|v| v.cut), "no view before");
        let s = two_views(&mut b);
        assert!(s.cut.is_empty());
        assert!(s.views.iter().all(|v| !v.cut));
        b.cut(Cut::LOAD);
        b.begin(SceneId { map_epoch: 2, frame: 11, tick: 21 });
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
        b.begin(SceneId { map_epoch: 1, frame: 1, tick: 1 });
        b.open_view(key(0), camera()).unwrap();
        b.push_instance(object(1, &[NodeMatrix::IDENTITY])).unwrap();
        b.begin(SceneId { map_epoch: 1, frame: 2, tick: 2 });
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
