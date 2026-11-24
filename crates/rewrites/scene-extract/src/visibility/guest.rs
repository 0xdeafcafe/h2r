//! What the Rust visibility reads of the running game, and nothing it
//! writes (docs/graphics/native-visibility.md, "Interfaces"): the main view's
//! mode, start cluster and flags, the open portals, three tuning values
//! from the image's data, and the sim's object references and spheres.
//! Every address and offset is a research point from the read of the 1.0
//! XBE; no value read here is committed.

use engine::render_loop::occlusion::guest::{
    BODY_SPHERE_CENTER, BODY_SPHERE_RADIUS, HEADER_BODY, HEADER_SIZE, MAIN_VIS, OBJECTS,
    VIEW_PLAYER, VIS_KIND, VIS_OBJECTS,
};

use super::Decline;
use super::flood::{OpenPortals, Tuning};
use super::objects::{ObjectInfo, ObjectWorld};
use super::read::{at, f32_at, f64_at, ptr_at, u8_at, u16_at, u32_at, vec3_at};
use crate::GuestRead;

/// The visibility struct the last gather used (`0x001320f0` stores it).
pub const ACTIVE_VIS: u32 = 0x004c_0b80;
/// The active structure BSP tag.
pub const ACTIVE_STRUCTURE: u32 = 0x004e_0348;
/// The open portals' bit array (one bit per portal).
pub const OPEN_PORTALS: u32 = 0x004f_93a4;
/// The tuning values of [`Tuning`], read from the image's data.
pub const STRADDLE: u32 = 0x0045_e060;
pub const WIDEN: u32 = 0x0045_dfa0;
pub const MERGE: u32 = 0x0043_ff08;
/// The cluster object references: per partition the heads array (a
/// reference datum per cluster) and the reference pool. A reference is
/// `+4` the object handle, `+8` the next reference.
pub const CLUSTER_OBJECTS: [(u32, u32); 2] =
    [(0x004d_e2e0, 0x004d_e2e4), (0x004d_e2d4, 0x004d_e2d8)];
/// The sky cluster the main view adds unclipped (`0x00132220`).
pub const SKY_CLUSTER_ENABLED: u32 = 0x004b_9ee9;
pub const SKY_CLUSTER_PREDICATE: u32 = 0x0046_86c4;
pub const SKY_CLUSTER_INDEX: u32 = 0x004b_9eec;
/// An object body's type byte.
pub const BODY_TYPE: u32 = 0xaa;

/// Offsets in the visibility struct (`0x00132b80` fills them).
pub mod vis {
    /// The view location's cluster (`u16`), where the flood starts.
    pub const START_CLUSTER: u32 = 0x8;
    pub const FLAGS: u32 = 0x2a60;
    /// The number of view frusta (`u16`) the flood starts from.
    pub const FRUSTA: u32 = 0x2a64;
    /// 0 portal flood, 1 sphere, 2 the potentially visible set.
    pub const MODE: u32 = 0x2a7c;
}

const NONE: u32 = u32::MAX;
const MAX_OBJECT_INDEX: u32 = 2048;

/// The main view's visibility inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewInputs {
    pub structure: u32,
    pub mode: u32,
    /// `u16::MAX` when the camera is outside the structure.
    pub start: u16,
    pub frusta: u16,
    pub flags: u32,
    /// The object list's capacity.
    pub capacity: usize,
}

pub fn view(mem: &dyn GuestRead) -> Result<ViewInputs, Decline> {
    let u = Decline::Unread;
    if u32_at(mem, ACTIVE_VIS) != Some(MAIN_VIS) || u32_at(mem, MAIN_VIS + VIS_KIND) != Some(0) {
        return Err(Decline::NotMain);
    }
    let list = ptr_at(mem, MAIN_VIS + VIS_OBJECTS).ok_or(u)?;
    Ok(ViewInputs {
        structure: ptr_at(mem, ACTIVE_STRUCTURE).ok_or(u)?,
        mode: u32_at(mem, MAIN_VIS + vis::MODE).ok_or(u)?,
        start: u16_at(mem, MAIN_VIS + vis::START_CLUSTER).ok_or(u)?,
        frusta: u16_at(mem, MAIN_VIS + vis::FRUSTA).ok_or(u)?,
        flags: u32_at(mem, MAIN_VIS + vis::FLAGS).ok_or(u)?,
        capacity: u32_at(mem, list).ok_or(u)? as usize,
    })
}

pub fn tuning(mem: &dyn GuestRead) -> Result<Tuning, Decline> {
    let t = Tuning {
        straddle: f64_at(mem, STRADDLE).ok_or(Decline::Unread)?,
        widen: f32_at(mem, WIDEN).ok_or(Decline::Unread)?,
        merge: f32_at(mem, MERGE).ok_or(Decline::Unread)?,
    };
    if t.straddle.is_finite() && t.widen.is_finite() && t.merge.is_finite() {
        Ok(t)
    } else {
        Err(Decline::Layout)
    }
}

pub fn open_portals(mem: &dyn GuestRead, portals: usize) -> Result<OpenPortals, Decline> {
    if portals == 0 {
        return Ok(OpenPortals::default());
    }
    let bits = ptr_at(mem, OPEN_PORTALS).ok_or(Decline::Unread)?;
    (0..portals.div_ceil(32) as u32)
        .map(|w| u32_at(mem, at(bits, 4 * w)?))
        .collect::<Option<Vec<u32>>>()
        .map(OpenPortals)
        .ok_or(Decline::Unread)
}

/// A datum array: `+0x24` the slot size, `+0x38` the slots in use, `+0x44`
/// the slots; a slot's `+0` is its salt (the handle's high half).
struct Pool {
    data: u32,
    end: u32,
    stride: u32,
}

impl Pool {
    fn read(mem: &dyn GuestRead, header: u32) -> Option<Self> {
        let stride = u32_at(mem, at(header, 0x24)?)?;
        let end = u32_at(mem, at(header, 0x38)?)?;
        let data = ptr_at(mem, at(header, 0x44)?)?;
        // A reference is at least its handle and next (`+4`, `+8`).
        ((12..=0x1_0000).contains(&stride) && end <= 0x1_0000).then_some(Self { data, end, stride })
    }

    fn slot(&self, mem: &dyn GuestRead, datum: u32) -> Result<u32, Decline> {
        let index = datum & 0xffff;
        if datum == NONE || index >= self.end || datum >> 16 == 0 {
            return Err(Decline::Layout);
        }
        let slot = index
            .checked_mul(self.stride)
            .and_then(|o| at(self.data, o))
            .ok_or(Decline::Layout)?;
        if u16_at(mem, slot).ok_or(Decline::Unread)? != (datum >> 16) as u16 {
            return Err(Decline::Layout);
        }
        Ok(slot)
    }
}

/// The handles one cluster's references in one partition name, in order.
fn references(
    mem: &dyn GuestRead,
    (heads, pool): (u32, u32),
    cluster: u16,
) -> Result<Vec<u32>, Decline> {
    let u = Decline::Unread;
    let heads = ptr_at(mem, heads).ok_or(u)?;
    let pool = Pool::read(mem, ptr_at(mem, pool).ok_or(u)?).ok_or(Decline::Layout)?;
    let mut reference = u32_at(mem, at(heads, 4 * u32::from(cluster)).ok_or(u)?).ok_or(u)?;
    let mut handles = Vec::new();
    for _ in 0..=pool.end {
        if reference == NONE {
            return Ok(handles);
        }
        let node = pool.slot(mem, reference)?;
        let handle = u32_at(mem, at(node, 4).ok_or(u)?).ok_or(u)?;
        if handle == NONE {
            return Ok(handles);
        }
        handles.push(handle);
        reference = u32_at(mem, at(node, 8).ok_or(u)?).ok_or(u)?;
    }
    Err(Decline::Layout)
}

/// The sky cluster, as the guest's gather finds it.
fn sky_cluster(mem: &dyn GuestRead, structure: u32) -> Result<Option<u16>, Decline> {
    let u = Decline::Unread;
    if u8_at(mem, SKY_CLUSTER_ENABLED).ok_or(u)? == 0
        || u16_at(mem, SKY_CLUSTER_PREDICATE).ok_or(u)? == u16::MAX
    {
        return Ok(None);
    }
    let index = u32_at(mem, SKY_CLUSTER_INDEX).ok_or(u)?;
    if index == NONE {
        return Ok(None);
    }
    let length = u32_at(mem, at(structure, 0xac).ok_or(u)?).ok_or(u)?;
    if index >= length {
        return if length == 0 {
            Ok(None)
        } else {
            Err(Decline::Layout)
        };
    }
    let table = ptr_at(mem, at(structure, 0xb0).ok_or(u)?).ok_or(u)?;
    let cluster = u16_at(mem, at(table, 2 * index).ok_or(u)?).ok_or(u)?;
    Ok((cluster != u16::MAX).then_some(cluster))
}

fn object(mem: &dyn GuestRead, objects: &Pool, handle: u32) -> Result<ObjectInfo, Decline> {
    let u = Decline::Unread;
    if handle & 0xffff >= MAX_OBJECT_INDEX {
        return Err(Decline::Layout);
    }
    let slot = objects.slot(mem, handle)?;
    let body = ptr_at(mem, at(slot, HEADER_BODY).ok_or(u)?).ok_or(u)?;
    Ok(ObjectInfo {
        centre: vec3_at(mem, at(body, BODY_SPHERE_CENTER).ok_or(u)?).ok_or(u)?,
        radius: f32_at(mem, at(body, BODY_SPHERE_RADIUS).ok_or(u)?).ok_or(u)?,
        kind: u8_at(mem, at(body, BODY_TYPE).ok_or(u)?).ok_or(u)?,
    })
}

/// The references of `clusters` (and of the sky cluster), and the objects
/// they name.
pub fn object_world(
    mem: &dyn GuestRead,
    view: &ViewInputs,
    clusters: &[u16],
) -> Result<ObjectWorld, Decline> {
    let u = Decline::Unread;
    let player = u32_at(mem, VIEW_PLAYER).ok_or(u)?;
    let mut world = ObjectWorld {
        view_flags: view.flags,
        player: (player != NONE).then_some(player),
        capacity: view.capacity,
        ..ObjectWorld::default()
    };
    if view.flags & super::objects::view_flags::NO_OBJECTS != 0 || clusters.is_empty() {
        return Ok(world);
    }
    world.sky = sky_cluster(mem, view.structure)?;
    let objects = Pool::read(mem, ptr_at(mem, OBJECTS).ok_or(u)?).ok_or(Decline::Layout)?;
    if objects.stride != HEADER_SIZE {
        return Err(Decline::Layout);
    }
    for &cluster in clusters.iter().chain(world.sky.as_ref()) {
        if usize::from(cluster) >= super::graph::MAX_CLUSTERS || world.refs.contains_key(&cluster) {
            continue;
        }
        let lists = [
            references(mem, CLUSTER_OBJECTS[0], cluster)?,
            references(mem, CLUSTER_OBJECTS[1], cluster)?,
        ];
        for &handle in lists.iter().flatten() {
            if !world.objects.contains_key(&handle) {
                world.objects.insert(handle, object(mem, &objects, handle)?);
            }
        }
        world.refs.insert(cluster, lists);
    }
    Ok(world)
}

/// The guest's own sets for the main view, from its lists.
pub fn guest_sets(gather: &frame_scene::scene::ClusterGather) -> super::VisibilitySets {
    super::VisibilitySets::new(gather.visible.clone(), gather.objects.clone())
}
