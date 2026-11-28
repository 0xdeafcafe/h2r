//! A dynamic light's visibility in Rust beside the guest's
//! (docs/graphics/native-visibility.md): the clusters and objects the
//! light's gather lists in its visibility struct `0x0054aa68`, computed
//! from the struct's inputs and compared with its lists as sets. A
//! differential only: nothing here is carried into the scene.
//!
//! The light frame (`0x00031c80`, step 11 of native-light-gather.md)
//! calls the gather `0x001320f0` with EAX 1, so `0x00132b80` fills the
//! light's struct: kind 1 (`+0x2a78`), the flags `(definition flags >>
//! 12) & 0x400` (`+0x2a60`, bit 0 added when the main view's struct is the
//! source), the frusta count `0x000c1d90` wrote (six for a light of volume
//! kind 0, else one; `+0x2a64`), the light's cluster (`L+0x48`) or the one
//! its centre is in (`+8`), the centre `L+0x18` (`+0x2a68`) and the radius
//! `L+0x24` (`+0x2a74`). A kind other than 0 never takes mode 2: no frusta
//! or a radius below the word at `0x0045dbd4` without flag `0x100` is the
//! sphere mode 1, else the frusta flood 0.
//!
//! Mode 1 (`0x00132d60`): the sphere flood `0x0014a5b0` from the start
//! cluster (the start alone for a radius not above the word at
//! `0x0045dbd8`, none for `0xffff`), then `0x00132220` per cluster with the
//! object test `0x00132e60`, which admits an object whose second sphere
//! (body `+0x50`, `+0x5c`) reaches the light's: `(r + R)² >= d²`. No sky
//! cluster and no player unit in a light's view. Under flag `0x400`
//! (definition bit 22) the first partition keeps type 7 only, and step 13
//! (`0x00031c20`) appends the owner's first ancestor of type 0 or with no
//! parent to the list when it has room, which [`compute_with`] adds the
//! same way.
//!
//! Mode 0 floods from each supported light frustum and tests the second
//! object sphere against the resulting windows.

use engine::render::light::{ARRAY_POINTER as LIGHTS, RECORD_SIZE};
use engine::render_loop::occlusion::guest::{
    LIST_COUNT, LIST_HANDLES, VIS_KIND, VIS_OBJECTS,
};

pub use engine::render::light_gather::LIGHT_VIS;

use super::flood::{self, Reached};
use super::frusta::WindowFrusta;
use super::graph::{MAX_CLUSTERS, PortalGraph};
use super::guest::{self, ACTIVE_VIS, ViewInputs};
use super::objects::{self, Clip, view_flags};
use super::read::{at, ptr_at, u16_at, u32_at};
use super::sphere::{self, Rows};
use super::{Decline, GraphCache, Parity, VisibilitySets, compare};
use crate::GuestRead;

/// Offsets of the region list.
pub mod vis {
    /// In the region block `[vis]`: the count (`i16`) and the cluster
    /// index (`u16`) of each region, [`REGION_STRIDE`] apart.
    pub const REGION_COUNT: u32 = 0xa6c;
    pub const REGION_CLUSTERS: u32 = 0xa6e;
    pub const REGION_STRIDE: u32 = 0x1a;
}

/// A light record's owner attachment (`u16`, `0xffff` none) and owner.
const RECORD_ATTACH: u32 = 0x54;
const RECORD_OWNER: u32 = 0x4c;
const NONE: u32 = u32::MAX;

/// A light view's inputs, as `0x00132b80` left them in its struct.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightView {
    pub view: ViewInputs,
    pub centre: [f32; 3],
    pub radius: f32,
}

/// The light's struct, when the last gather filled it in a light's kind.
pub fn light_view(mem: &dyn GuestRead) -> Result<LightView, Decline> {
    let u = Decline::Unread;
    if u32_at(mem, ACTIVE_VIS) != Some(LIGHT_VIS) {
        return Err(Decline::NotMain);
    }
    match u32_at(mem, LIGHT_VIS + VIS_KIND) {
        None => return Err(u),
        Some(0) => return Err(Decline::NotMain),
        Some(_) => {}
    }
    let view = guest::view_at(mem, LIGHT_VIS)?;
    let (centre, radius) = guest::view_sphere(mem, LIGHT_VIS)?;
    Ok(LightView {
        view,
        centre,
        radius,
    })
}

/// The guest's lists in the struct at `struct_at`: its regions' clusters and its
/// object list, as sets.
pub fn guest_lists(mem: &dyn GuestRead, struct_at: u32) -> Result<VisibilitySets, Decline> {
    let u = Decline::Unread;
    let field = |base: u32, offset: u32| at(base, offset).ok_or(Decline::Layout);
    let regions = ptr_at(mem, struct_at).ok_or(u)?;
    let count = u16_at(mem, field(regions, vis::REGION_COUNT)?).ok_or(u)? as i16;
    if count < 0 || count as usize > MAX_CLUSTERS {
        return Err(Decline::Layout);
    }
    let clusters = (0..count as u32)
        .map(|i| {
            let region = field(regions, vis::REGION_CLUSTERS + vis::REGION_STRIDE * i)?;
            u16_at(mem, region).ok_or(u)
        })
        .collect::<Result<Vec<u16>, Decline>>()?;
    let list = ptr_at(mem, field(struct_at, VIS_OBJECTS)?).ok_or(u)?;
    let capacity = u32_at(mem, list).ok_or(u)?;
    let count = u32::from(u16_at(mem, field(list, LIST_COUNT)?).ok_or(u)?);
    if count > capacity {
        return Err(Decline::Layout);
    }
    let objects = if count == 0 {
        Vec::new()
    } else {
        let handles = ptr_at(mem, field(list, LIST_HANDLES)?).ok_or(u)?;
        (0..count)
            .map(|i| u32_at(mem, field(handles, 4 * i)?).ok_or(u))
            .collect::<Result<Vec<u32>, Decline>>()?
    };
    Ok(VisibilitySets::new(clusters, objects))
}

/// Step 13's object for the light datum `light` (`0x00031c20`): its
/// owner's first ancestor whose type byte (body `+0xaa`) is 0 or that has
/// no parent; `None` for a light with no owner.
fn extra_caster(mem: &dyn GuestRead, light: u32) -> Result<Option<u32>, Decline> {
    let u = Decline::Unread;
    let field = |base: u32, offset: u32| at(base, offset).ok_or(Decline::Layout);
    if light == NONE {
        return Err(Decline::Layout);
    }
    let records = ptr_at(mem, field(ptr_at(mem, LIGHTS).ok_or(u)?, 0x44)?).ok_or(u)?;
    let record = field(records, (light & 0xffff) * RECORD_SIZE as u32)?;
    if u16_at(mem, field(record, RECORD_ATTACH)?).ok_or(u)? == 0xffff {
        return Ok(None);
    }
    let owner = u32_at(mem, field(record, RECORD_OWNER)?).ok_or(u)?;
    if owner == NONE {
        return Ok(None);
    }
    crate::guest::with_objects(mem, |pool| {
        let mut handle = owner;
        for _ in 0..0x1_0000 {
            let view = sim_state::object(pool, sim_state::ObjectId(handle)).map_err(|_| u)?.ok_or(Decline::Layout)?;
            let parent = view.parent().map_err(|_| u)?.map_or(NONE, |p| p.0);
            if view.body_kind().map_err(|_| u)? == 0 || parent == NONE {
                return Ok(Some(handle));
            }
            handle = parent;
        }
        Err(Decline::Layout)
    })
}

/// The light view's sets over `graph`, with the image's projection `rows`
/// and the radius the sphere flood must exceed (`flood_min`, the word at
/// `0x0045dbd8`), both read from the running image by [`compute`].
/// `light` is the light datum whose gather filled the struct, for step 13.
pub fn compute_with(
    mem: &dyn GuestRead,
    graph: &PortalGraph,
    light_view: &LightView,
    rows: &Rows,
    flood_min: f32,
    light: u32,
) -> Result<VisibilitySets, Decline> {
    let view = &light_view.view;
    if view.mode != 1 {
        return Err(Decline::Mode);
    }
    let clusters = sphere::view_clusters(
        graph,
        rows,
        view.start,
        light_view.centre,
        light_view.radius,
        flood_min,
    )?;
    let reached = Reached {
        clusters,
        windows: Vec::new(),
    };
    let clip = Clip::Sphere {
        centre: light_view.centre,
        radius: light_view.radius,
    };
    listed_objects(mem, view, reached, clip, light)
}

fn listed_objects(
    mem: &dyn GuestRead,
    view: &ViewInputs,
    reached: Reached,
    clip: Clip,
    light: u32,
) -> Result<VisibilitySets, Decline> {
    let world = guest::light_object_world(mem, view, &reached.clusters)?;
    let mut listed = objects::object_set(&world, &reached, clip)?;
    if view.flags & view_flags::FIRST_PARTITION_TYPE != 0
        && let Some(extra) = extra_caster(mem, light)?
        && !listed.contains(&extra)
        && listed.len() + 1 < view.capacity
    {
        listed.push(extra);
    }
    Ok(VisibilitySets::new(reached.clusters, listed))
}

/// Gather once per light frustum, as in `0x00173130`. A cluster reached
/// through several faces has one region and can admit objects through
/// the windows of any of its faces (`0x00165010`).
fn frustum_sets(
    mem: &dyn GuestRead,
    graph: &PortalGraph,
    view: &ViewInputs,
    light: u32,
    pose: u32,
    volume: u32,
) -> Result<VisibilitySets, Decline> {
    let mut reached = Reached::default();
    let mut windows = WindowFrusta::default();
    if view.start != u16::MAX {
        let faces = super::light_frusta::frusta(mem, pose, volume, view.frusta)?;
        let tuning = guest::tuning(mem)?;
        let open = guest::open_portals(mem, graph.portals.len())?;
        for (lens, clip_plane) in faces {
            let face = flood::flood(graph, &lens, &tuning, &open, view.start)?;
            let inputs = guest::window_inputs(mem, lens.far, clip_plane)?;
            for cluster in face.clusters {
                if !reached.clusters.contains(&cluster) {
                    reached.clusters.push(cluster);
                }
            }
            windows.0.extend(WindowFrusta::build(&face.windows, &lens, &inputs).0);
            if reached.clusters.len() > flood::MAX_REGIONS || windows.0.len() > flood::MAX_WINDOWS {
                return Err(Decline::Overflow);
            }
        }
    }
    listed_objects(mem, view, reached, Clip::Frusta(windows), light)
}

/// Visibility from the pose and volume passed to this light's draw.
pub fn compute(
    mem: &dyn GuestRead,
    cache: &mut GraphCache,
    epoch: u64,
    light: u32,
    pose: u32,
    volume: u32,
) -> Result<VisibilitySets, Decline> {
    let light_view = light_view(mem)?;
    let view = &light_view.view;
    let graph = cache.graph(mem, epoch, view.structure)?;
    match view.mode {
        0 => frustum_sets(mem, graph, view, light, pose, volume),
        1 => {
            let rows = guest::projection_rows(mem)?;
            let flood_min = guest::sphere_flood_min(mem)?;
            compute_with(mem, graph, &light_view, &rows, flood_min, light)
        }
        _ => Err(Decline::Mode),
    }
}

/// The light whose gather last filled `0x0054aa68`, in Rust against its
/// lists: read after that gather and before the next light's (its passes'
/// entry). `light` is its datum.
pub fn light_arm(
    mem: &dyn GuestRead,
    cache: &mut GraphCache,
    epoch: u64,
    light: u32,
    pose: u32,
    volume: u32,
) -> Result<Parity, Decline> {
    let rust = compute(mem, cache, epoch, light, pose, volume)?;
    let guest = guest_lists(mem, LIGHT_VIS)?;
    Ok(compare(&rust, &guest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;
    use engine::render_loop::occlusion::guest::OBJECTS;
    use crate::visibility::Mismatch;
    use crate::visibility::graph::tests::two_rooms;
    use crate::visibility::guest::{
        ACTIVE_STRUCTURE, CLUSTER_OBJECTS, PROJECTION_ROWS, SPHERE_FLOOD_MIN, VIS_SPHERE_CENTRE,
        VIS_SPHERE_RADIUS,
    };
    use crate::visibility::sphere::tests::ROWS;

    const STRUCTURE: u32 = 0x8000;
    const REGIONS: u32 = 0x2_0000;
    const LIST: u32 = 0xe000;
    const HANDLES: u32 = 0xe100;
    const LIGHT_RECORDS: u32 = 0xf000;
    const POSE: u32 = 0x24000;
    const VOLUME: u32 = 0x24100;
    const LIGHT: u32 = 0x0001_0002;

    /// A datum array header at `header` with `end` slots of `stride` at
    /// `data`.
    fn pool(m: &mut Mem, header: u32, stride: u32, end: u32, data: u32) {
        m.w(header + 0x24, stride);
        m.w(header + 0x38, end);
        m.w(header + 0x44, data);
    }

    /// Object `index` (salt 1): its main sphere far away, its second
    /// sphere at `centre`, radius 0.5; unattached, no parent.
    fn object(m: &mut Mem, index: u32, centre: [f32; 3]) {
        let (slot, body) = (0xa100 + 12 * index, 0xb000 + 0x100 * index);
        m.w(slot, 1);
        m.w(slot + 8, body);
        for (i, v) in centre.into_iter().enumerate() {
            m.f(body + 0x30 + 4 * i as u32, 1000.0);
            m.f(body + 0x50 + 4 * i as u32, v);
        }
        m.f(body + 0x3c, 0.5);
        m.f(body + 0x5c, 0.5);
        m.w(body + 0x14, NONE);
        m.w(body + 0xa8, 0);
    }

    /// A light at (3.5, 0, 0) of radius 1 in room 0, half a unit from the
    /// portal (x = 4): its sphere crosses into room 1. Room 0 references
    /// object 1 at (3, 0, 0) (reached); room 1 objects 2 at (4.5, 0, 0)
    /// (reached) and 3 at (10, 0, 0) (not). The guest's lists hold room 1,
    /// room 0, objects 2 and 1.
    fn scene() -> Mem {
        let mut m = Mem::default();
        two_rooms(&mut m, STRUCTURE);
        m.w(ACTIVE_STRUCTURE, STRUCTURE);
        m.w(ACTIVE_VIS, LIGHT_VIS);
        m.w(LIGHT_VIS + VIS_KIND, 1);
        m.w(LIGHT_VIS + guest::vis::MODE, 1);
        m.w(LIGHT_VIS + guest::vis::START_CLUSTER, 0);
        m.w(LIGHT_VIS + guest::vis::FLAGS, 0);
        m.w(LIGHT_VIS + guest::vis::FRUSTA, 6);
        for (i, v) in [3.5, 0.0, 0.0].into_iter().enumerate() {
            m.f(LIGHT_VIS + VIS_SPHERE_CENTRE + 4 * i as u32, v);
        }
        m.f(LIGHT_VIS + VIS_SPHERE_RADIUS, 1.0);
        // The image's values, invented: the projection rows as i16
        // triples (the third unused) and the flood's minimum radius.
        for (r, [a, b]) in ROWS.into_iter().enumerate() {
            let at = PROJECTION_ROWS + 6 * r as u32;
            half(&mut m, at, u16::from(a));
            half(&mut m, at + 2, u16::from(b));
            half(&mut m, at + 4, 0);
        }
        m.f(SPHERE_FLOOD_MIN, 0.0);
        // Objects.
        m.w(OBJECTS, 0xa000);
        pool(&mut m, 0xa000, 12, 16, 0xa100);
        object(&mut m, 1, [3.0, 0.0, 0.0]);
        object(&mut m, 2, [4.5, 0.0, 0.0]);
        object(&mut m, 3, [10.0, 0.0, 0.0]);
        let [(heads, refs), (heads2, refs2)] = CLUSTER_OBJECTS;
        m.w(heads, 0xc000);
        m.w(refs, 0xc100);
        pool(&mut m, 0xc100, 12, 16, 0xc200);
        m.w(0xc000, 0x0002_0000);
        m.w(0xc004, 0x0002_0001);
        for (index, handle, next) in [
            (0, 0x0001_0001, NONE),
            (1, 0x0001_0002, 0x0002_0002),
            (2, 0x0001_0003, NONE),
        ] {
            let slot = 0xc200 + 12 * index;
            m.w(slot, 2);
            m.w(slot + 4, handle);
            m.w(slot + 8, next);
        }
        m.w(heads2, 0xc400);
        m.w(refs2, 0xc500);
        pool(&mut m, 0xc500, 12, 0, 0xc600);
        m.w(0xc400, NONE);
        m.w(0xc404, NONE);
        // The light record: no owner.
        m.w(LIGHTS, 0xef00);
        m.w(0xef00 + 0x44, LIGHT_RECORDS);
        let record = LIGHT_RECORDS + 2 * RECORD_SIZE as u32;
        m.w(record + 0x54, 0xffff);
        // The guest's lists.
        m.w(LIGHT_VIS, REGIONS);
        guest_regions(&mut m, &[1, 0]);
        m.w(LIGHT_VIS + VIS_OBJECTS, LIST);
        m.w(LIST, 64);
        m.w(LIST + LIST_HANDLES, HANDLES);
        guest_objects(&mut m, &[0x0001_0002, 0x0001_0001]);
        m
    }

    /// Writes the half at `addr` into its word, keeping the other half.
    fn half(m: &mut Mem, addr: u32, v: u16) {
        let shift = (addr & 2) * 8;
        let word = m.0.get(&(addr & !3)).copied().unwrap_or(0);
        m.w(addr & !3, (word & !(0xffff << shift)) | (u32::from(v) << shift));
    }

    fn guest_regions(m: &mut Mem, clusters: &[u16]) {
        half(m, REGIONS + vis::REGION_COUNT, clusters.len() as u16);
        for (i, &c) in clusters.iter().enumerate() {
            half(m, REGIONS + vis::REGION_CLUSTERS + vis::REGION_STRIDE * i as u32, c);
        }
    }

    fn guest_objects(m: &mut Mem, handles: &[u32]) {
        m.w(LIST + LIST_COUNT, handles.len() as u32);
        for (i, &h) in handles.iter().enumerate() {
            m.w(HANDLES + 4 * i as u32, h);
        }
    }

    /// The arm as the scene step runs it, with the flood's minimum radius
    /// `flood_min` in the image.
    fn run(m: &Mem, flood_min: f32) -> Result<Parity, Decline> {
        let mut m = Mem(m.0.clone());
        m.f(SPHERE_FLOOD_MIN, flood_min);
        light_arm(&m, &mut GraphCache::default(), 1, LIGHT, POSE, VOLUME)
    }

    fn sets(m: &Mem) -> VisibilitySets {
        let graph = super::super::graph::read(m, STRUCTURE).unwrap();
        compute_with(m, &graph, &light_view(m).unwrap(), &ROWS, 0.0, LIGHT).unwrap()
    }

    #[test]
    fn a_lights_sphere_reaches_through_the_portal_and_equals_the_guests_lists() {
        let m = scene();
        assert_eq!(
            sets(&m),
            VisibilitySets {
                clusters: vec![0, 1],
                objects: vec![0x0001_0001, 0x0001_0002],
            }
        );
        assert!(run(&m, 0.0).unwrap().holds());
    }

    #[test]
    fn a_deliberate_mismatch_names_what_only_the_guest_listed() {
        let mut m = scene();
        guest_objects(&mut m, &[0x0001_0002, 0x0001_0001, 0x0001_0003]);
        let p = run(&m, 0.0).unwrap();
        assert_eq!(p.first, Some(Mismatch::ObjectGuestOnly(0x0001_0003)));
        assert_eq!((p.objects_guest_only, p.objects_rust_only), (1, 0));
        // A radius not above the flood's minimum keeps the start cluster
        // alone: room 1 is the guest's only.
        let p = run(&m, 2.0).unwrap();
        assert_eq!(p.first, Some(Mismatch::ClusterGuestOnly(1)));
    }

    #[test]
    fn a_projected_light_sees_through_the_portal_up_to_its_own_far_plane() {
        use crate::visibility::{light_frusta::PORTAL_CLIP, tests::window_fields, view::lens::HALF};
        let mut m = scene();
        window_fields(&mut m);
        m.w(guest::OPEN_PORTALS, 0x25000);
        m.w(0x25000, 1);
        m.w(LIGHT_VIS + guest::vis::MODE, 0);
        m.w(LIGHT_VIS + guest::vis::FRUSTA, 1);
        m.w(VOLUME, 2);
        for (address, vector) in [
            (VOLUME + 0x1c, [0.0, -1.0, 0.0]),
            (POSE + 0x28, [1.0, 0.0, 0.0]),
            (POSE + 0x34, [0.0, 0.0, 1.0]),
            (VOLUME + 0x2c, [0.0, 0.0, 0.0]),
            (VOLUME + 0x3c, [1.0, 0.0, 0.0]),
        ] {
            for (axis, value) in vector.into_iter().enumerate() {
                m.f(address + 4 * axis as u32, value);
            }
        }
        let tolerance = 0.125_f32.to_bits();
        m.w(PORTAL_CLIP & !3, tolerance << 8);
        m.w((PORTAL_CLIP & !3) + 4, tolerance >> 24);
        m.f(HALF, 0.5);
        m.f(VOLUME + 0x28, 1.0);
        m.f(VOLUME + 0x58, 2.0);
        m.f(VOLUME + 0x5c, 2.0);
        m.f(VOLUME + 0x64, 6.0);
        assert!(run(&m, 0.0).unwrap().holds());
        guest_objects(&mut m, &[0x0001_0001, 0x0001_0002, 0x0001_0003]);
        let parity = run(&m, 0.0).unwrap();
        assert_eq!(parity.first, Some(Mismatch::ObjectGuestOnly(0x0001_0003)));
    }

    #[test]
    fn the_extra_caster_is_listed_under_definition_bit_22() {
        let mut m = scene();
        m.w(LIGHT_VIS + guest::vis::FLAGS, view_flags::FIRST_PARTITION_TYPE);
        // Owner object 3 (out of reach, type 1) has the parent object 4
        // (type 0): object 4 is appended.
        let record = LIGHT_RECORDS + 2 * RECORD_SIZE as u32;
        m.w(record + 0x54, 0);
        m.w(record + 0x4c, 0x0001_0003);
        object(&mut m, 4, [50.0, 0.0, 0.0]);
        m.w(0xb000 + 0x300 + 0x14, 0x0001_0004);
        m.w(0xb000 + 0x300 + 0xa8, 0x0001_0000); // type 1 at +0xaa
        // Objects 1 and 2 are type 0: under the flag the first partition
        // keeps type 7 only.
        assert_eq!(sets(&m).objects, [0x0001_0004]);
        // A full list leaves it out, as the guest's append does.
        m.w(LIST, 1);
        assert_eq!(sets(&m).objects, Vec::<u32>::new());
    }

    #[test]
    fn views_it_does_not_cover_decline_by_name() {
        let mut m = scene();
        m.w(LIGHT_VIS + guest::vis::MODE, 3);
        assert_eq!(run(&m, 0.0), Err(Decline::Mode), "unknown mode");
        let mut m = scene();
        m.w(ACTIVE_VIS, guest::vis::START_CLUSTER);
        assert_eq!(run(&m, 0.0), Err(Decline::NotMain), "another struct");
        let mut m = scene();
        m.w(LIGHT_VIS + VIS_KIND, 0);
        assert_eq!(run(&m, 0.0), Err(Decline::NotMain), "the main view's kind");
        let mut m = scene();
        m.0.remove(&(LIGHT_VIS + VIS_SPHERE_RADIUS));
        assert_eq!(run(&m, 0.0), Err(Decline::Unread));
        let mut m = scene();
        m.0.remove(&(PROJECTION_ROWS & !3));
        assert_eq!(run(&m, 0.0), Err(Decline::Unread), "the image's rows");
        let mut m = scene();
        half(&mut m, REGIONS + vis::REGION_COUNT, 0xffff);
        assert_eq!(guest_lists(&m, LIGHT_VIS), Err(Decline::Layout));
        let mut m = scene();
        m.w(LIST + LIST_COUNT, 65);
        assert_eq!(guest_lists(&m, LIGHT_VIS), Err(Decline::Layout), "past capacity");
    }

    #[test]
    fn a_light_outside_the_structure_lists_nothing() {
        let mut m = scene();
        m.w(LIGHT_VIS + guest::vis::START_CLUSTER, 0xffff);
        guest_regions(&mut m, &[]);
        guest_objects(&mut m, &[]);
        assert!(run(&m, 0.0).unwrap().holds());
    }
}
