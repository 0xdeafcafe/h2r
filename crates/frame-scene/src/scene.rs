//! The scene: flat arrays and index ranges (docs/graphics/framescene.md,
//! section 4.2), so a scene is a handful of allocations that are recycled
//! ([`crate::FreeList`]) and can be walked without chasing pointers.
//!
//! Slice 1 fills the views, their cameras, clusters, object instances,
//! parts and palette rows. Lights, shadow maps, the carried part and
//! texture uploads join in the slices that type or carry them (3 and 6).

use std::fmt;
use std::ops::Range;
use std::time::Instant;

use serde::Serialize;

use crate::camera::Camera;
use crate::node::NodeMatrix;
use crate::object::{Defaults, ItemKey, Lighting};

/// One vertex constant register: a palette row is three of them, world
/// space.
pub type Row = [f32; 4];

/// Which frame a scene is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct SceneId {
    /// The native map work's generation: a new map, BSP or load starts
    /// another.
    pub map_epoch: u64,
    /// The presents before this frame's.
    pub frame: u64,
    /// The game tick after this frame's ticks.
    pub tick: u32,
}

/// When a scene was published, and how many ticks ran before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Stamp {
    /// On the sim thread, at the publish.
    #[serde(skip)]
    pub published: Option<Instant>,
    /// 0 (paused, loading), 1 or 2.
    pub ticks_run: u8,
}

/// Why a scene does not follow on from the one before (a bitset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Cut(pub u8);

impl Cut {
    pub const NONE: Cut = Cut(0);
    pub const MAP: Cut = Cut(1);
    pub const BSP: Cut = Cut(1 << 1);
    pub const REVERT: Cut = Cut(1 << 2);
    pub const LOAD: Cut = Cut(1 << 3);
    pub const VIEWS: Cut = Cut(1 << 4);
    const NAMES: [(Cut, &'static str); 5] =
        [(Cut::MAP, "map"), (Cut::BSP, "bsp"), (Cut::REVERT, "revert"), (Cut::LOAD, "load"), (Cut::VIEWS, "views")];

    pub fn contains(self, other: Cut) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The reasons set, by name.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES.iter().filter(|(c, _)| !c.is_empty() && self.contains(*c)).map(|(_, n)| *n).collect()
    }
}

impl std::ops::BitOr for Cut {
    type Output = Cut;
    fn bitor(self, rhs: Cut) -> Cut {
        Cut(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Cut {
    fn bitor_assign(&mut self, rhs: Cut) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ViewKind {
    Game,
    Cinematic,
    Alternate,
}

/// A view's identity across scenes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ViewKey {
    pub kind: ViewKind,
    pub player: u8,
    /// The guest's view index.
    pub view: u8,
}

impl fmt::Display for ViewKey {
    /// `game:0:0` (kind:player:view), as `render.scene` names it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ViewKind::Game => "game",
            ViewKind::Cinematic => "cinematic",
            ViewKind::Alternate => "alternate",
        };
        write!(f, "{kind}:{}:{}", self.player, self.view)
    }
}

/// A view's world state the typed world stages read, by value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct WorldState {
    /// Whether the texture accumulation target was there to bind when the
    /// view was extracted (`scene_extract::layout::object_nodes::texaccum_bound`).
    pub texaccum: bool,
}

/// One game view.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    pub key: ViewKey,
    /// Nothing of this view's camera can be blended from the scene
    /// before: no view of the same key there (slice 8 adds scripted cuts,
    /// respawns and the distance backstop).
    pub cut: bool,
    pub camera: Camera,
    /// Into [`FrameScene::clusters`].
    pub clusters: Range<u32>,
    /// Into [`FrameScene::instances`], in the guest's order.
    pub instances: Range<u32>,
    /// Into [`FrameScene::order`]: this view's parts in the guest's list
    /// order (by [`PartDraw::slot`]).
    pub order: Range<u32>,
    pub world: WorldState,
    /// Object items extraction refused: the guest still draws them.
    pub remainder: u32,
}

/// An object's identity across scenes: its datum handle with its salt.
/// Opaque to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct InstanceKey(pub u32);

/// One object drawn in a view.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Instance {
    pub key: InstanceKey,
    /// The object definition's tag datum.
    pub definition: u32,
    /// The render model's tag datum (`ModelId`).
    pub model: u32,
    /// Into [`FrameScene::nodes`]: world-space node matrices, model node
    /// order.
    pub nodes: Range<u32>,
    /// World bounding sphere: centre, radius.
    pub sphere: ([f32; 3], f32),
    pub lighting: Option<Lighting>,
    pub defaults: Option<Defaults>,
    /// Into [`FrameScene::parts`].
    pub parts: Range<u32>,
    /// Set by extraction when the engine says so (slice 8).
    pub teleported: bool,
}

/// How a part draws beyond its material (slice 3: stencil marking,
/// conditional stream ranges, the auxiliary state toggle). Empty today:
/// a part with any of them stays the guest's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct PartFlags(pub u8);

/// What one instance draws for one layer item.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PartDraw {
    /// The item's place in the guest's sorted layer list.
    pub slot: u32,
    /// The record, the material (shader tag datum), section, part and
    /// implementation the guest picked.
    pub key: ItemKey,
    pub layer: u8,
    /// Into [`FrameScene::instances`]: the instance it belongs to.
    pub instance: u32,
    /// Into [`FrameScene::rows`]: the palette rows its record uploads,
    /// already remapped for its section.
    pub rows: Range<u32>,
    pub flags: PartFlags,
}

/// One rendered frame, typed. Built on the sim thread by
/// [`crate::SceneBuilder`], sealed, then read only.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FrameScene {
    pub id: SceneId,
    pub stamp: Stamp,
    pub cut: Cut,
    /// At most four game views plus the alternate view.
    pub views: Vec<View>,
    /// Visible cluster ids; views index ranges of it.
    pub clusters: Vec<u16>,
    pub instances: Vec<Instance>,
    pub nodes: Vec<NodeMatrix>,
    pub parts: Vec<PartDraw>,
    /// Part indices, each view's in its guest list order.
    pub order: Vec<u32>,
    pub rows: Vec<Row>,
}

fn slice<T>(v: &[T], r: &Range<u32>) -> &[T] {
    v.get(r.start as usize..r.end as usize).unwrap_or(&[])
}

impl FrameScene {
    /// Empties every array, keeping its allocation.
    pub fn clear(&mut self) {
        self.id = SceneId::default();
        self.stamp = Stamp::default();
        self.cut = Cut::NONE;
        self.views.clear();
        self.clusters.clear();
        self.instances.clear();
        self.nodes.clear();
        self.parts.clear();
        self.order.clear();
        self.rows.clear();
    }

    pub fn view(&self, key: ViewKey) -> Option<&View> {
        self.views.iter().find(|v| v.key == key)
    }

    pub fn clusters_of(&self, view: &View) -> &[u16] {
        slice(&self.clusters, &view.clusters)
    }

    pub fn instances_of(&self, view: &View) -> &[Instance] {
        slice(&self.instances, &view.instances)
    }

    pub fn nodes_of(&self, instance: &Instance) -> &[NodeMatrix] {
        slice(&self.nodes, &instance.nodes)
    }

    pub fn parts_of(&self, instance: &Instance) -> &[PartDraw] {
        slice(&self.parts, &instance.parts)
    }

    pub fn rows_of(&self, part: &PartDraw) -> &[Row] {
        slice(&self.rows, &part.rows)
    }

    /// The part of `view` at `slot` of the guest's list, with its
    /// instance.
    pub fn part_at(&self, view: &View, slot: u32) -> Option<(&Instance, &PartDraw)> {
        let order = slice(&self.order, &view.order);
        let i = order.binary_search_by_key(&slot, |&p| self.parts[p as usize].slot).ok()?;
        let part = &self.parts[order[i] as usize];
        Some((self.instances.get(part.instance as usize)?, part))
    }

    /// The instance of `view` with `key`.
    pub fn instance(&self, view: &View, key: InstanceKey) -> Option<&Instance> {
        self.instances_of(view).iter().find(|i| i.key == key)
    }

    /// Counts per view, totals across views.
    pub fn counts(&self, view: &View) -> ViewCounts {
        let instances = self.instances_of(view);
        let parts: u32 = instances.iter().map(|i| i.parts.len() as u32).sum();
        let rows: u32 = instances
            .iter()
            .flat_map(|i| self.parts_of(i))
            .map(|p| p.rows.len() as u32)
            .sum();
        ViewCounts {
            clusters: view.clusters.len() as u32,
            instances: instances.len() as u32,
            parts,
            rows,
            remainder: view.remainder,
        }
    }

    /// Every range in bounds, each view's ranges disjoint from the other
    /// views', each instance's parts and nodes inside the arrays, each
    /// part pointing back at its instance, each view's order sorted by
    /// slot. An error names the first that is not.
    pub fn check(&self) -> Result<(), String> {
        fn within(r: &Range<u32>, len: usize, what: &str) -> Result<(), String> {
            if r.start > r.end || r.end as usize > len {
                return Err(format!("{what} {r:?} outside 0..{len}"));
            }
            Ok(())
        }
        let mut last = [0u32; 3];
        for (n, v) in self.views.iter().enumerate() {
            within(&v.clusters, self.clusters.len(), &format!("view {n} clusters"))?;
            within(&v.instances, self.instances.len(), &format!("view {n} instances"))?;
            within(&v.order, self.order.len(), &format!("view {n} order"))?;
            for (k, (r, l)) in [&v.clusters, &v.instances, &v.order].into_iter().zip(&mut last).enumerate() {
                if r.start < *l {
                    return Err(format!("view {n} range {k} {r:?} overlaps the view before"));
                }
                *l = r.end;
            }
            let mut previous = None;
            for &p in slice(&self.order, &v.order) {
                let part = self.parts.get(p as usize).ok_or_else(|| format!("view {n} orders part {p}, not in the scene"))?;
                if !v.instances.contains(&part.instance) {
                    return Err(format!("view {n} orders part {p} of another view's instance {}", part.instance));
                }
                if previous.is_some_and(|s| s >= part.slot) {
                    return Err(format!("view {n} order is not by slot at part {p}"));
                }
                previous = Some(part.slot);
            }
            let parts: usize = self.instances_of(v).iter().map(|i| i.parts.len()).sum();
            if parts != v.order.len() {
                return Err(format!("view {n} orders {} parts of {parts}", v.order.len()));
            }
        }
        for (n, i) in self.instances.iter().enumerate() {
            within(&i.nodes, self.nodes.len(), &format!("instance {n} nodes"))?;
            within(&i.parts, self.parts.len(), &format!("instance {n} parts"))?;
            for (k, p) in self.parts_of(i).iter().enumerate() {
                if p.instance as usize != n {
                    return Err(format!("instance {n} part {k} names instance {}", p.instance));
                }
                within(&p.rows, self.rows.len(), &format!("instance {n} part {k} rows"))?;
            }
        }
        Ok(())
    }
}

/// One view's sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewCounts {
    pub clusters: u32,
    pub instances: u32,
    pub parts: u32,
    pub rows: u32,
    pub remainder: u32,
}
