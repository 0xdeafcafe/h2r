//! The scene: flat arrays and index ranges (docs/graphics/framescene.md,
//! section 4.2), so a scene is a handful of allocations that are recycled
//! ([`crate::FreeList`]) and can be walked without chasing pointers.
//!
//! Slice 1 fills the views, their cameras, clusters, object instances,
//! parts and palette rows; slice 3 the lights and their passes' steps,
//! the world layers' and the first person's items.
//! Shadow maps, the carried part and texture uploads join in the slices
//! that type or carry them (3 and 6).

use std::fmt;
use std::ops::Range;
use std::time::Instant;

use serde::Serialize;

use crate::camera::Camera;
use crate::layer::{LayerItem, LayerPass};
use crate::light::{LightStep, SceneLight};
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
    /// The first-person draw `0x0003fdf0` of a game view: its own
    /// projection and lists.
    FirstPerson,
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
            ViewKind::FirstPerson => "first_person",
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

/// What the auxiliary state toggle `0x00012d50` sets for the view
/// (docs/graphics/framescene.md, slice 3): the view-projection rows it
/// uploads to `c[0..3]` and the program clip range (W-buffer units) it
/// stores, on, as a part with [`PartFlags::AUX_DEPTH`] draws, and off, as
/// the part's epilogue leaves the device. On is off times [`Self::scale`],
/// the rows whole and the clip's near end.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AuxDepth {
    pub scale: f32,
    pub rows: [Row; 4],
    pub clip: [f32; 2],
    pub rest_rows: [Row; 4],
    pub rest_clip: [f32; 2],
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
    /// What its [`PartFlags::AUX_DEPTH`] parts draw with; read when the
    /// view has one.
    pub aux_depth: Option<AuxDepth>,
    /// Into [`FrameScene::lights`], in the guest's light order.
    pub lights: Range<u32>,
    /// Into [`FrameScene::layer_passes`], in the guest's draw order.
    pub layers: Range<u32>,
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

/// How a part draws beyond its material: what its render record's `+0`
/// flags make the guest's geometry draw (`0x0004d0b0`) do around and in
/// its draw call (docs/graphics/framescene.md, slice 3). A bitset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct PartFlags(pub u8);

impl PartFlags {
    pub const NONE: PartFlags = PartFlags(0);
    /// Record bit 14 in layers 1 and 3: the draw's prologue (`0x0004d640`)
    /// sets stencil mode 2 (the structure mode with reference `0xff`) and
    /// its epilogue (`0x0004d720`) leaves mode 1, unless the layer draws
    /// with the stencil off.
    pub const STENCIL_MARK: PartFlags = PartFlags(1);
    /// Record bit 9 on a part that is not a triangle list: the emitter
    /// (`0x0004daa0`) draws its indices as a strip. Without the bit it
    /// draws every part as a list.
    pub const STRIP: PartFlags = PartFlags(1 << 1);
    /// Record bit 6 with a visible-subpart mask, in a section with
    /// subparts: the emitter draws only the visible subparts
    /// ([`PartDraw::subparts`]), each its own strip with strip topology,
    /// consecutive ones as one list otherwise.
    pub const SUBPARTS: PartFlags = PartFlags(1 << 2);
    /// Record bit 11: the draw's prologue and epilogue call the auxiliary
    /// state toggle `0x00012d50`, and the part draws with its view's
    /// [`AuxDepth`] (its view-projection rows and clip range scaled).
    pub const AUX_DEPTH: PartFlags = PartFlags(1 << 3);
    const NAMES: [(PartFlags, &'static str); 4] = [
        (PartFlags::STENCIL_MARK, "stencil_mark"),
        (PartFlags::STRIP, "strip"),
        (PartFlags::SUBPARTS, "subparts"),
        (PartFlags::AUX_DEPTH, "aux_depth"),
    ];

    pub fn contains(self, other: PartFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags set, by name (`render.scene`'s `part_flags`).
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES.iter().filter(|(f, _)| self.contains(*f)).map(|(_, n)| *n).collect()
    }
}

impl std::ops::BitOr for PartFlags {
    type Output = PartFlags;
    fn bitor(self, rhs: PartFlags) -> PartFlags {
        PartFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for PartFlags {
    fn bitor_assign(&mut self, rhs: PartFlags) {
        self.0 |= rhs.0;
    }
}

/// Which of a part's subparts the guest's emitter (`0x0004daa0`) draws:
/// the record's visible-subpart mask (record `+0x14`, a bit per subpart of
/// the section) read over the part's own subparts. Bit `n` of `visible` is
/// the section's subpart `first + n`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Subparts {
    /// The part's first subpart (part `+0xa`).
    pub first: u16,
    /// Its subparts (part `+0xc`); at most [`Subparts::MAX`].
    pub count: u8,
    pub visible: u64,
}

impl Subparts {
    /// The most subparts a part can have here; a part with more keeps its
    /// D3D draw.
    pub const MAX: u8 = 64;

    /// The part's subparts that draw, by index among its own (0 the
    /// part's first), ascending.
    pub fn drawn(self) -> impl Iterator<Item = u8> {
        (0..self.count.min(Self::MAX)).filter(move |&n| self.visible >> n & 1 != 0)
    }
}

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
    /// With [`PartFlags::SUBPARTS`]: the subparts it draws; `None`, the
    /// whole part.
    pub subparts: Option<Subparts>,
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
    pub lights: Vec<SceneLight>,
    /// Each light's steps; lights index ranges of it.
    pub light_steps: Vec<LightStep>,
    pub layer_passes: Vec<LayerPass>,
    /// Each layer pass's items; passes index ranges of it.
    pub layer_items: Vec<LayerItem>,
}

fn slice<'a, T>(v: &'a [T], r: &Range<u32>) -> &'a [T] {
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
        self.lights.clear();
        self.light_steps.clear();
        self.layer_passes.clear();
        self.layer_items.clear();
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

    pub fn lights_of(&self, view: &View) -> &[SceneLight] {
        slice(&self.lights, &view.lights)
    }

    pub fn steps_of(&self, light: &SceneLight) -> &[LightStep] {
        slice(&self.light_steps, &light.steps)
    }

    pub fn layers_of(&self, view: &View) -> &[LayerPass] {
        slice(&self.layer_passes, &view.layers)
    }

    pub fn items_of(&self, pass: &LayerPass) -> &[LayerItem] {
        slice(&self.layer_items, &pass.items)
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
        let mut flagged = Vec::<(&'static str, u32)>::new();
        for p in instances.iter().flat_map(|i| self.parts_of(i)) {
            for name in p.flags.names() {
                match flagged.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, c)) => *c += 1,
                    None => flagged.push((name, 1)),
                }
            }
        }
        let lights = self.lights_of(view);
        ViewCounts {
            clusters: view.clusters.len() as u32,
            instances: instances.len() as u32,
            parts,
            rows,
            remainder: view.remainder,
            flagged,
            lights: lights.len() as u32,
            shadows: lights.iter().filter(|l| l.shadowed()).count() as u32,
            layers: self
                .layers_of(view)
                .iter()
                .map(|p| (p.layer, p.items.len() as u32, p.remainder))
                .collect(),
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
        let mut last = [0u32; 5];
        for (n, v) in self.views.iter().enumerate() {
            within(&v.clusters, self.clusters.len(), &format!("view {n} clusters"))?;
            within(&v.instances, self.instances.len(), &format!("view {n} instances"))?;
            within(&v.order, self.order.len(), &format!("view {n} order"))?;
            within(&v.lights, self.lights.len(), &format!("view {n} lights"))?;
            within(&v.layers, self.layer_passes.len(), &format!("view {n} layers"))?;
            let ranges = [&v.clusters, &v.instances, &v.order, &v.lights, &v.layers];
            for (k, (r, l)) in ranges.into_iter().zip(&mut last).enumerate() {
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
        for (n, l) in self.lights.iter().enumerate() {
            within(&l.steps, self.light_steps.len(), &format!("light {n} steps"))?;
        }
        for (n, p) in self.layer_passes.iter().enumerate() {
            within(&p.items, self.layer_items.len(), &format!("layer pass {n} items"))?;
            for (k, i) in self.items_of(p).iter().enumerate() {
                within(&i.rows, self.rows.len(), &format!("layer pass {n} item {k} rows"))?;
            }
        }
        Ok(())
    }
}

/// One view's sizes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewCounts {
    pub clusters: u32,
    pub instances: u32,
    pub parts: u32,
    pub rows: u32,
    pub remainder: u32,
    /// Parts with each [`PartFlags`] flag, by name; flags no part has are
    /// left out.
    pub flagged: Vec<(&'static str, u32)>,
    pub lights: u32,
    /// Lights that cast their shadow: the shadow maps the view draws.
    pub shadows: u32,
    /// Each layer pass: its layer, items and remainder.
    pub layers: Vec<(u8, u32, u32)>,
}
