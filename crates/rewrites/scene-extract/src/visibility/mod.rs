//! Visibility in Rust (docs/graphics/native-visibility.md; slice R4 of
//! docs/graphics/retire-d3d.md): the main view's and the lights' visible
//! clusters and object lists, and the main view's records, computed from
//! the structure's portal graph, the observer camera and the sim's object
//! references.
//!
//! - [`graph`] decodes the structure's clusters, portals and potentially
//!   visible set once into Rust-owned arrays ([`GraphCache`]).
//! - [`view`] builds the main view's render camera, projection, start
//!   cluster and mode: the [`Origin`] the main view's sets are walked from.
//! - [`flood`] is the portal flood (mode 0) and the visible-set row
//!   (mode 2), [`pvs`] a row computed at run time, [`sphere`] the sphere
//!   mode (1).
//! - [`frusta`] builds the windows' frusta mode 0 tests objects against.
//! - [`objects`] is the object set over [`objects::ObjectWorld`], the
//!   references and spheres [`guest`] reads.
//! - [`light`] is a light view's sets against its struct's lists.
//! - [`parity`] compares sets with the guest's lists, and
//!   [`frusta::compare`] the window frusta with the guest's
//!   ([`guest::window_frusta`]).
//! - [`shadow`] is the three audit arms (clusters, frusta, objects): a
//!   sampled frame compares, an arm answers after its clean matches and
//!   falls back to the guest on any difference.
//!
//! The scene step runs [`compute`] for a game view from the Rust view
//! record, and [`answer`] puts the answering arms' sets in the view's
//! `ClusterGather`. A view whose record or computation refused keeps the
//! guest's lists for that frame; nothing here writes guest memory, so the
//! sim's hashes cannot move.

pub mod flood;
pub mod frusta;
pub mod graph;
pub mod guest;
pub mod light;
mod light_frusta;
pub mod objects;
pub mod parity;
mod placements;
pub mod portal_walk;
pub mod portal_projection;
pub mod portal_clip;
pub mod pvs;
mod read;
pub mod shadow;
pub mod sphere;
pub mod view;
pub mod window_record;

pub use parity::{Mismatch, Parity, VisibilitySets, compare};

use frame_scene::RenderCamera;

use crate::GuestRead;
use engine::render_loop::occlusion::guest::MAIN_VIS;
use graph::PortalGraph;

/// Why the Rust visibility made no sets for a view. The view keeps the
/// guest's, and `render.scene` counts the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// Guest memory it needs did not read.
    Unread,
    /// A count, index or link out of the bounds the guest's arrays have.
    Layout,
    /// A visibility mode it does not port.
    Mode,
    /// An unsupported frustum shape or count.
    Frusta,
    /// A node, window, region or list limit reached: the guest drops by its
    /// own order there.
    Overflow,
    /// The camera does not make a lens.
    Camera,
    /// The active visibility struct is not the main view's.
    NotMain,
}

impl Decline {
    pub const ALL: [Decline; 7] = [
        Self::Unread,
        Self::Layout,
        Self::Mode,
        Self::Frusta,
        Self::Overflow,
        Self::Camera,
        Self::NotMain,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Layout => "layout",
            Self::Mode => "mode",
            Self::Frusta => "frusta",
            Self::Overflow => "overflow",
            Self::Camera => "camera",
            Self::NotMain => "not_main",
        }
    }
}

/// Where the main view's sets are walked from: the start cluster and mode
/// of the Rust view record ([`view::build`]), not the ones the guest's
/// `0x00132b80` stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    /// `u16::MAX` outside the structure.
    pub start: u16,
    /// 0 portal flood, 1 sphere, 2 the potentially visible set.
    pub mode: u32,
}

impl Origin {
    pub fn of(record: &view::ViewRecord) -> Self {
        Self { start: record.start, mode: record.mode }
    }
}

/// The graph of the structure last decoded. A new map (`epoch`), another
/// structure, or a structure whose cluster or portal count changed decodes
/// again; a structure that declined for its layout is not decoded again.
#[derive(Debug, Default)]
pub struct GraphCache {
    key: Option<(u64, u32, [u32; 2])>,
    graph: Option<Result<PortalGraph, Decline>>,
    placements: Option<placements::Index>,
}

impl GraphCache {
    pub fn graph(
        &mut self,
        mem: &dyn GuestRead,
        epoch: u64,
        structure: u32,
    ) -> Result<&PortalGraph, Decline> {
        let shape = [graph::layout::CLUSTER_COUNT, graph::layout::PORTAL_COUNT]
            .map(|o| structure.checked_add(o).and_then(|a| mem.read_u32(a)));
        let [Some(clusters), Some(portals)] = shape else {
            return Err(Decline::Unread);
        };
        let key = (epoch, structure, [clusters, portals]);
        if self.key != Some(key) || self.graph.is_none() {
            let read = graph::read(mem, structure);
            if matches!(read, Err(Decline::Unread)) {
                self.forget();
                return Err(Decline::Unread);
            }
            self.placements = None;
            self.key = Some(key);
            self.graph = Some(read);
        }
        match self.graph.as_ref() {
            Some(Ok(graph)) => Ok(graph),
            Some(Err(why)) => Err(*why),
            None => Err(Decline::Unread),
        }
    }

    /// Placement spheres reached through the wide portals only. Also
    /// exclude every placement the guest submitted, including shared ones.
    pub fn display_placements(&mut self, mem: &dyn GuestRead, retail: &frusta::WindowFrusta, wide: &frusta::WindowFrusta) -> Option<Vec<u16>> {
        let (_, bsp, _) = self.key?;
        if self.placements.is_none() {
            self.placements = Some(placements::Index::read(mem, bsp)?);
        }
        Some(self.placements.as_ref()?.extra(retail, wide, placements::submitted(mem)?))
    }

    pub fn forget(&mut self) {
        self.placements = None;
        self.key = None;
        self.graph = None;
    }
}

/// The main view's Rust visibility, walked from `origin` with `camera`'s
/// lens. `epoch` names the loaded map, for the cache.
pub fn compute(
    mem: &dyn GuestRead,
    camera: &RenderCamera,
    origin: Origin,
    cache: &mut GraphCache,
    epoch: u64,
) -> Result<VisibilitySets, Decline> {
    compute_with_windows(mem, camera, origin, cache, epoch).map(|(sets, _)| sets)
}

pub fn compute_with_windows(
    mem: &dyn GuestRead, camera: &RenderCamera, origin: Origin, cache: &mut GraphCache, epoch: u64,
) -> Result<(VisibilitySets, Option<frusta::WindowFrusta>), Decline> {
    let view = guest::view(mem)?.from_origin(origin);
    let graph = cache.graph(mem, epoch, view.structure)?;
    let lens = flood::Lens::new(camera).ok_or(Decline::Camera)?;
    let none = view.start == u16::MAX;
    let (reached, clip) = match view.mode {
        0 => {
            if view.frusta != 1 {
                return Err(Decline::Frusta);
            }
            let (reached, frusta) = if none {
                (flood::Reached::default(), frusta::WindowFrusta::default())
            } else {
                let tuning = guest::tuning(mem)?;
                let open = guest::open_portals(mem, graph.portals.len())?;
                let (inputs, portal_far) = guest::frustum_inputs(mem)?;
                let mut lens = lens;
                lens.far = portal_far.unwrap_or(f32::INFINITY);
                let reached = flood::flood(graph, &lens, &tuning, &open, view.start)?;
                let frusta = frusta::WindowFrusta::build(&reached.windows, &lens, &inputs);
                (reached, frusta)
            };
            (reached, objects::Clip::Frusta(frusta))
        }
        1 => {
            let (centre, radius) = guest::view_sphere(mem, MAIN_VIS)?;
            let clusters = if none {
                Vec::new()
            } else {
                let rows = guest::projection_rows(mem)?;
                let flood_min = guest::sphere_flood_min(mem)?;
                sphere::view_clusters(graph, &rows, view.start, centre, radius, flood_min)?
            };
            let reached = flood::Reached {
                clusters,
                ..flood::Reached::default()
            };
            (reached, objects::Clip::Sphere { centre, radius })
        }
        2 => {
            let reached = if none {
                flood::Reached::default()
            } else {
                let open = guest::open_portals(mem, graph.portals.len())?;
                flood::potentially_visible(graph, view.start, &open)?
            };
            (reached, objects::Clip::All)
        }
        _ => return Err(Decline::Mode),
    };
    let world = guest::object_world(mem, &view, &reached.clusters)?;
    let objects = objects::object_set_with_clip(&world, &reached, &clip)?;
    let windows = match clip { objects::Clip::Frusta(w) => Some(w), _ => None };
    Ok((VisibilitySets::new(reached.clusters, objects), windows))
}

/// The view's gather with the shadow arms' answers ([`shadow`]): the Rust
/// clusters, and the mode they were walked in, in place of the guest's
/// when `clusters`, the Rust objects when `objects`. The source is Rust
/// only when both answer, so a gather marked Rust holds ascending sets.
/// The structure list stays the guest's, and the guest's lists remain
/// what the render records were built from.
pub fn answer(
    mut gather: frame_scene::scene::ClusterGather,
    rust: VisibilitySets,
    mode: u32,
    clusters: bool,
    objects: bool,
) -> frame_scene::scene::ClusterGather {
    if clusters {
        gather.visible = rust.clusters;
        gather.mode = mode;
    }
    if objects {
        gather.objects = rust.objects;
    }
    if clusters && objects {
        gather.source = frame_scene::VisibilitySource::Rust;
    }
    gather
}

#[cfg(test)]
mod tests;
