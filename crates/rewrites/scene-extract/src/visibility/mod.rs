//! Visibility in Rust (docs/graphics/native-visibility.md; slice R4 of
//! docs/graphics/retire-d3d.md): the main view's and the lights' visible
//! clusters and object lists, and the main view's records, computed from
//! the structure's portal graph, the observer camera and the sim's object
//! references, beside the guest's.
//!
//! - [`graph`] decodes the structure's clusters, portals and potentially
//!   visible set once into Rust-owned arrays ([`GraphCache`]).
//! - [`flood`] is the portal flood (mode 0) and the visible-set row
//!   (mode 2), [`pvs`] a row computed at run time, [`sphere`] the sphere
//!   mode (1).
//! - [`frusta`] builds the windows' frusta mode 0 tests objects against.
//! - [`objects`] is the object set over [`objects::ObjectWorld`], the
//!   references and spheres [`guest`] reads.
//! - [`view`] builds the main view's render camera, projection, start
//!   cluster and mode, and compares them with the guest's.
//! - [`light`] is a light view's sets against its struct's lists.
//! - [`parity`] compares sets with the guest's lists.
//!
//! The scene step runs [`compute`] for every game view after the guest's
//! gather, compares, counts, and puts the Rust sets in the view's
//! `ClusterGather` only when they equal the guest's. Nothing here writes
//! guest memory, so the sim's hashes cannot move.

pub mod flood;
pub mod frusta;
pub mod graph;
pub mod guest;
pub mod light;
pub mod objects;
pub mod parity;
pub mod pvs;
mod read;
pub mod sphere;
pub mod view;

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
    /// A mode it does not port: an unknown one, or a light view's
    /// frusta (mode 0).
    Mode,
    /// More than one view frustum.
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

/// The graph of the structure last decoded. A new map (`epoch`), another
/// structure, or a structure whose cluster or portal count changed decodes
/// again; a structure that declined for its layout is not decoded again.
#[derive(Debug, Default)]
pub struct GraphCache {
    key: Option<(u64, u32, [u32; 2])>,
    graph: Option<Result<PortalGraph, Decline>>,
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
            self.key = Some(key);
            self.graph = Some(read);
        }
        match self.graph.as_ref() {
            Some(Ok(graph)) => Ok(graph),
            Some(Err(why)) => Err(*why),
            None => Err(Decline::Unread),
        }
    }

    pub fn forget(&mut self) {
        self.key = None;
        self.graph = None;
    }
}

/// The main view's Rust visibility. `epoch` names the loaded map, for the
/// cache.
pub fn compute(
    mem: &dyn GuestRead,
    camera: &RenderCamera,
    cache: &mut GraphCache,
    epoch: u64,
) -> Result<VisibilitySets, Decline> {
    let view = guest::view(mem)?;
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
                let reached = flood::flood(graph, &lens, &tuning, &open, view.start)?;
                let inputs = guest::frustum_inputs(mem)?;
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
    let objects = objects::object_set(&world, &reached, clip)?;
    Ok(VisibilitySets::new(reached.clusters, objects))
}

/// The view's gather with the Rust sets in place of the guest's lists when
/// they are equal as sets, and the comparison. A difference keeps the
/// guest's lists: they stay the fallback.
pub fn arm(
    mut gather: frame_scene::scene::ClusterGather,
    rust: VisibilitySets,
) -> (frame_scene::scene::ClusterGather, Parity) {
    let parity = compare(&rust, &guest::guest_sets(&gather));
    if parity.holds() {
        gather.visible = rust.clusters;
        gather.objects = rust.objects;
        gather.source = frame_scene::VisibilitySource::Rust;
    }
    (gather, parity)
}

#[cfg(test)]
mod tests;
