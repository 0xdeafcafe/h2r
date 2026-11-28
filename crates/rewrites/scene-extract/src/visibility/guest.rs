//! What the Rust visibility reads of the running game, and nothing it
//! writes (docs/graphics/native-visibility.md, "Interfaces"): the main view's
//! mode, start cluster and flags, the open portals, three tuning values
//! from the image's data, and the sim's object references and spheres.
//! Every address and offset is a research point from the read of the 1.0
//! XBE; no value read here is committed.

use engine::render_loop::occlusion::guest::{
    MAIN_VIS, VIEW_PLAYER, VIS_KIND, VIS_OBJECTS,
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

impl ViewInputs {
    /// These inputs walked from `origin` (the Rust view record's start
    /// cluster and mode) in place of the struct's.
    pub fn from_origin(self, origin: super::Origin) -> Self {
        Self { start: origin.start, mode: origin.mode, ..self }
    }
}

pub fn view(mem: &dyn GuestRead) -> Result<ViewInputs, Decline> {
    if u32_at(mem, ACTIVE_VIS) != Some(MAIN_VIS) || u32_at(mem, MAIN_VIS + VIS_KIND) != Some(0) {
        return Err(Decline::NotMain);
    }
    view_at(mem, MAIN_VIS)
}

/// The visibility inputs of the struct at `struct_at`, whichever view it
/// is: the caller checks that it is the one it means.
pub fn view_at(mem: &dyn GuestRead, struct_at: u32) -> Result<ViewInputs, Decline> {
    let u = Decline::Unread;
    let field = |offset: u32| at(struct_at, offset).ok_or(Decline::Layout);
    let list = ptr_at(mem, field(VIS_OBJECTS)?).ok_or(u)?;
    Ok(ViewInputs {
        structure: ptr_at(mem, ACTIVE_STRUCTURE).ok_or(u)?,
        mode: u32_at(mem, field(vis::MODE)?).ok_or(u)?,
        start: u16_at(mem, field(vis::START_CLUSTER)?).ok_or(u)?,
        frusta: u16_at(mem, field(vis::FRUSTA)?).ok_or(u)?,
        flags: u32_at(mem, field(vis::FLAGS)?).ok_or(u)?,
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

/// One object's sphere and type from the sim's views (`sim-state`).
fn object(
    objects: &dyn guest_layout::Mem,
    handle: u32,
    light: bool,
) -> Result<ObjectInfo, Decline> {
    let u = Decline::Unread;
    if handle & 0xffff >= MAX_OBJECT_INDEX {
        return Err(Decline::Layout);
    }
    let view = sim_state::object(objects, sim_state::ObjectId(handle))
        .map_err(|_| u)?
        .ok_or(Decline::Layout)?;
    let sphere = if light { view.dynamic_sphere() } else { view.bounding_sphere() }.map_err(|_| u)?;
    Ok(ObjectInfo {
        centre: sphere.center,
        radius: sphere.radius,
        kind: view.body_kind().map_err(|_| u)?,
    })
}

/// The local players' globals (`[0x004e8c20]`): from `+0xc` the four
/// local users' player datums, none for an unmapped user.
pub const PLAYER_GLOBALS: u32 = 0x004e_8c20;
pub const LOCAL_PLAYERS: u32 = 0xc;
/// The players' datum array header (`[0x004e8c24]`): a player is `0x21c`
/// bytes, its unit at `+0x2c`.
pub const PLAYERS: u32 = 0x004e_8c24;
pub const PLAYER_SIZE: u32 = 0x21c;
pub const PLAYER_UNIT: u32 = 0x2c;
/// The local users' cameras, `0x140` bytes each: `+0xc` the camera's kind
/// (its update routine) and `+0x4` the float `0x00155760` tests.
pub const USER_CAMERAS: u32 = 0x004e_8c44;
pub const USER_CAMERA_SIZE: u32 = 0x140;
pub const USER_CAMERA_KIND: u32 = 0xc;
pub const USER_CAMERA_TEST: u32 = 0x4;
/// The kind `0x00155760` answers 0 for: the first-person camera.
pub const FIRST_PERSON_CAMERA: u32 = 0x0023_c110;
/// The image's `0.0` that `0x00155760` compares `+0x4` with.
pub const CAMERA_TEST_ZERO: u32 = 0x0045_dbd8;

/// The unit `0x0002b990` passes in EDI for `0x0003f33c` to append: the
/// view's local user (`VIEW_PLAYER` holds the user index, not a unit;
/// `0x0002ba10` stores its argument there and passes the same argument in
/// EAX) when mapped to a player whose camera `0x00155760` answers 0 for
/// (kind [`FIRST_PERSON_CAMERA`] with `+0x4` equal to `0.0`, unordered
/// not equal), that player's unit (`[[0x004e8c24]+0x44] + 0x21c * index
/// + 0x2c`). Otherwise none, as for a unit of `-1`.
pub fn view_player_unit(mem: &dyn GuestRead) -> Result<Option<u32>, Decline> {
    let (u, l) = (Decline::Unread, Decline::Layout);
    let user = u32_at(mem, VIEW_PLAYER).ok_or(u)?;
    if user == NONE {
        return Ok(None);
    }
    if user >= 4 {
        return Err(l);
    }
    let player = crate::guest::with_players(mem, |players| sim_state::local_users(players))
        .map_err(|_| u)?[user as usize];
    let Some(player) = player else {
        return Ok(None);
    };
    let camera = USER_CAMERAS + USER_CAMERA_SIZE * user;
    let kind = u32_at(mem, camera + USER_CAMERA_KIND).ok_or(u)?;
    let test = f32_at(mem, camera + USER_CAMERA_TEST).ok_or(u)?;
    let zero = f32_at(mem, CAMERA_TEST_ZERO).ok_or(u)?;
    if kind != FIRST_PERSON_CAMERA || test != zero {
        return Ok(None);
    }
    let unit = crate::guest::with_players(mem, |players| {
        sim_state::players(players)
            .map_err(|_| u)
            .map(|all| all.into_iter().find(|p| p.id.index() == player as u16).and_then(|p| p.unit))
    })?;
    Ok(unit.map(|id| id.0))
}

/// The references of `clusters` (and of the sky cluster), and the objects
/// they name.
pub fn object_world(
    mem: &dyn GuestRead,
    view: &ViewInputs,
    clusters: &[u16],
) -> Result<ObjectWorld, Decline> {
    let mut world = ObjectWorld {
        view_flags: view.flags,
        player: view_player_unit(mem)?,
        capacity: view.capacity,
        ..ObjectWorld::default()
    };
    if view.flags & super::objects::view_flags::NO_OBJECTS != 0 || clusters.is_empty() {
        return Ok(world);
    }
    world.sky = sky_cluster(mem, view.structure)?;
    references_into(mem, &mut world, clusters, false)?;
    Ok(world)
}

/// [`object_world`] for a light's view (kind not 0): each object's second
/// sphere, no sky cluster (`0x00132220` adds it only in kind 0) and no
/// player unit (`0x0003f33c` appends it to the main view's list only).
pub fn light_object_world(
    mem: &dyn GuestRead,
    view: &ViewInputs,
    clusters: &[u16],
) -> Result<ObjectWorld, Decline> {
    let mut world = ObjectWorld {
        view_flags: view.flags,
        capacity: view.capacity,
        ..ObjectWorld::default()
    };
    if view.flags & super::objects::view_flags::NO_OBJECTS != 0 || clusters.is_empty() {
        return Ok(world);
    }
    references_into(mem, &mut world, clusters, true)?;
    Ok(world)
}

/// The references of `clusters` and of `world.sky` into `world`, with the
/// objects they name.
fn references_into(
    mem: &dyn GuestRead,
    world: &mut ObjectWorld,
    clusters: &[u16],
    light: bool,
) -> Result<(), Decline> {
    crate::guest::with_objects(mem, |pool| {
        for &cluster in clusters.iter().chain(world.sky.as_ref()) {
            if usize::from(cluster) >= super::graph::MAX_CLUSTERS || world.refs.contains_key(&cluster) {
                continue;
            }
            let lists = [
                references(mem, CLUSTER_OBJECTS[0], cluster)?,
                references(mem, CLUSTER_OBJECTS[1], cluster)?,
            ];
            for &handle in lists.iter().flatten() {
                if let std::collections::hash_map::Entry::Vacant(e) = world.objects.entry(handle) {
                    e.insert(object(pool, handle, light)?);
                }
            }
            world.refs.insert(cluster, lists);
        }
        Ok(())
    })
}

/// The guest's own sets for the main view, from its lists.
pub fn guest_sets(gather: &frame_scene::scene::ClusterGather) -> super::VisibilitySets {
    super::VisibilitySets::new(gather.visible.clone(), gather.objects.clone())
}

/// The image's projection rows (`0x00440b94`: six rows of three `i16`),
/// of which the sphere flood takes each row's first two axes.
pub const PROJECTION_ROWS: u32 = 0x0044_0b94;
/// The radius at or under which the sphere mode takes the start cluster
/// alone (`0x00132d60`'s `comiss`).
pub const SPHERE_FLOOD_MIN: u32 = 0x0045_dbd8;
/// The view sphere in the visibility struct: centre and radius.
pub const VIS_SPHERE_CENTRE: u32 = 0x2a68;
pub const VIS_SPHERE_RADIUS: u32 = 0x2a74;

pub fn projection_rows(mem: &dyn GuestRead) -> Result<super::sphere::Rows, Decline> {
    let mut rows = [[0u8; 2]; 6];
    for (r, row) in rows.iter_mut().enumerate() {
        for (k, axis) in row.iter_mut().enumerate() {
            let a = u16_at(mem, PROJECTION_ROWS + 6 * r as u32 + 2 * k as u32)
                .ok_or(Decline::Unread)? as i16;
            *axis = u8::try_from(a)
                .ok()
                .filter(|&a| a < 3)
                .ok_or(Decline::Layout)?;
        }
    }
    Ok(rows)
}

pub fn sphere_flood_min(mem: &dyn GuestRead) -> Result<f32, Decline> {
    f32_at(mem, SPHERE_FLOOD_MIN).ok_or(Decline::Unread)
}

/// The view sphere of the struct at `vis` (`0x00132b80` stores the
/// position and radius it was given).
pub fn view_sphere(mem: &dyn GuestRead, vis: u32) -> Result<([f32; 3], f32), Decline> {
    let u = Decline::Unread;
    let field = |offset: u32| at(vis, offset).ok_or(Decline::Layout);
    Ok((
        vec3_at(mem, field(VIS_SPHERE_CENTRE)?).ok_or(u)?,
        f32_at(mem, field(VIS_SPHERE_RADIUS)?).ok_or(u)?,
    ))
}

/// The values the window frusta read from the image (`0x00163db0` and its
/// normalise `0x00030bf0`; see [`super::frusta::Inputs`]).
pub const FRUSTUM_UNIT: u32 = 0x0045_dbc0;
pub const FRUSTUM_NEGATIVE_UNIT: u32 = 0x0045_dbcc;
pub const NORMALIZE_EPSILON: u32 = 0x0045_dbdc;
/// Points at the six floats a window frustum's box starts from.
pub const FRUSTUM_BOUNDS: u32 = 0x0046_87e0;

/// The view frustum the flood starts from: `[vis]` points at the frusta
/// struct (`0x00173130` fills it), its first view frustum at `+4`
/// (`0x1bc` bytes).
pub mod view_frustum {
    pub const FIRST: u32 = 0x4;
    pub const TEST_PORTAL_FAR: u32 = 0x84;
    pub const FAR: u32 = 0x88;
    /// A byte: the eye plane is replaced by [`CLIP_PLANE`], negated.
    pub const HAS_CLIP_PLANE: u32 = 0x6c;
    pub const CLIP_PLANE: u32 = 0x74;
}

fn floats<const N: usize>(mem: &dyn GuestRead, base: u32) -> Result<[f32; N], Decline> {
    let mut out = [0.0; N];
    for (i, v) in out.iter_mut().enumerate() {
        let addr = at(base, 4 * i as u32).ok_or(Decline::Layout)?;
        *v = f32_at(mem, addr).ok_or(Decline::Unread)?;
    }
    Ok(out)
}

/// Main-view window inputs and the flood's optional portal distance limit.
pub fn frustum_inputs(mem: &dyn GuestRead) -> Result<(super::frusta::Inputs, Option<f32>), Decline> {
    let u = Decline::Unread;
    let view = ptr_at(mem, MAIN_VIS)
        .and_then(|f| at(f, view_frustum::FIRST))
        .ok_or(u)?;
    let clip_plane = if u8_at(mem, at(view, view_frustum::HAS_CLIP_PLANE).ok_or(u)?).ok_or(u)? != 0
    {
        Some(floats(mem, at(view, view_frustum::CLIP_PLANE).ok_or(u)?)?)
    } else {
        None
    };
    let far = f32_at(mem, at(view, view_frustum::FAR).ok_or(u)?).ok_or(u)?;
    let test_far = u8_at(mem, at(view, view_frustum::TEST_PORTAL_FAR).ok_or(u)?).ok_or(u)?;
    Ok((window_inputs(mem, far, clip_plane)?, (test_far != 0).then_some(far)))
}

pub(super) fn window_inputs(
    mem: &dyn GuestRead,
    far: f32,
    clip_plane: Option<[f32; 4]>,
) -> Result<super::frusta::Inputs, Decline> {
    let u = Decline::Unread;
    let bounds = floats(mem, ptr_at(mem, FRUSTUM_BOUNDS).ok_or(u)?)?;
    let inputs = super::frusta::Inputs {
        far,
        unit: f32_at(mem, FRUSTUM_UNIT).ok_or(u)?,
        negative_unit: f32_at(mem, FRUSTUM_NEGATIVE_UNIT).ok_or(u)?,
        epsilon: f32_at(mem, NORMALIZE_EPSILON).ok_or(u)?,
        bounds,
        clip_plane,
    };
    if far.is_finite() && inputs.unit.is_finite() && inputs.negative_unit.is_finite() && inputs.epsilon.is_finite() {
        Ok(inputs)
    } else {
        Err(Decline::Layout)
    }
}

/// The region records the gather leaves behind `[vis]`: the frustum count
/// (`u16` at `+0`), the region count and rows (`0x1a` bytes: the cluster,
/// six window counts, six first-window indices), the window count and the
/// `0x108`-byte windows (box at `+0x60`, planes at `+0x78`).
pub mod regions {
    pub const FRUSTA: u32 = 0x0;
    pub const COUNT: u32 = 0xa6c;
    pub const ROWS: u32 = 0xa6e;
    pub const ROW_STRIDE: u32 = 0x1a;
    pub const ROW_WINDOWS: u32 = 0x2;
    pub const ROW_FIRST: u32 = 0xe;
    pub const WINDOW_COUNT: u32 = 0x1970;
    pub const WINDOWS: u32 = 0x1974;
    pub const WINDOW_STRIDE: u32 = 0x108;
    pub const WINDOW_BOUNDS: u32 = 0x60;
    pub const WINDOW_PLANES: u32 = 0x78;
}

/// The guest's window frusta of the main view in mode 0, each with its
/// region's cluster: per region, the windows `first..first + count`, as
/// the object test `0x00165010` reads them. One view frustum only, as the
/// Rust flood ([`Decline::Frusta`] otherwise).
pub fn window_frusta(mem: &dyn GuestRead) -> Result<super::frusta::WindowFrusta, Decline> {
    use super::flood::{MAX_REGIONS, MAX_WINDOWS};
    use super::frusta::{Frustum, WindowFrusta};
    let view = view(mem)?;
    if view.mode != 0 {
        return Err(Decline::Mode);
    }
    let u = Decline::Unread;
    let l = Decline::Layout;
    let base = ptr_at(mem, MAIN_VIS).ok_or(u)?;
    let field = |offset: u32| at(base, offset).ok_or(l);
    if u16_at(mem, field(regions::FRUSTA)?).ok_or(u)? != 1 {
        return Err(Decline::Frusta);
    }
    let count = u32::from(u16_at(mem, field(regions::COUNT)?).ok_or(u)?);
    let windows = u32::from(u16_at(mem, field(regions::WINDOW_COUNT)?).ok_or(u)?);
    if count as usize > MAX_REGIONS || windows as usize > MAX_WINDOWS {
        return Err(l);
    }
    let mut out = WindowFrusta::default();
    for i in 0..count {
        let row = field(regions::ROWS + regions::ROW_STRIDE * i)?;
        let cluster = u16_at(mem, row).ok_or(u)?;
        let n = u32::from(u16_at(mem, at(row, regions::ROW_WINDOWS).ok_or(l)?).ok_or(u)?);
        if n == 0 {
            continue;
        }
        let first = u32::from(u16_at(mem, at(row, regions::ROW_FIRST).ok_or(l)?).ok_or(u)?);
        if first + n > windows {
            return Err(l);
        }
        for w in first..first + n {
            let slot = field(regions::WINDOWS + regions::WINDOW_STRIDE * w)?;
            let bounds = floats(mem, at(slot, regions::WINDOW_BOUNDS).ok_or(l)?)?;
            let flat: [f32; 24] = floats(mem, at(slot, regions::WINDOW_PLANES).ok_or(l)?)?;
            let planes = std::array::from_fn(|p| std::array::from_fn(|k| flat[4 * p + k]));
            out.0.push((cluster, Frustum { bounds, planes }));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::object::header;
    use guest_layout::{Mem as _, Object, ObjectType};
    use sim_objects::synth::Pool;
    use sim_objects::{ObjectTable, PoolMem};

    /// A synthetic arena read by words, as the guest's memory is.
    struct Words<'a>(&'a guest_layout::FlatRam);

    impl GuestRead for Words<'_> {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            let mut b = [0; 4];
            self.0.read_bytes(addr, &mut b).ok()?;
            Some(u32::from_le_bytes(b))
        }
    }

    /// The sim's view of an object is the object the guest-memory read finds,
    /// for either sphere, a stale handle and a handle past the pool.
    #[test]
    fn the_sim_views_answer_as_the_guest_reads_do() {
        let mut p = Pool::new();
        let a = p.add(3, 0x8003, ObjectType::Biped, header::ACTIVE);
        let b = p.add(9, 0x8009, ObjectType::Crate, header::ACTIVE);
        for (slot, kind, c, r, dc, dr) in [(3, 1u8, [1.0, 2.0, 3.0], 0.5, [4.0, 5.0, 6.0], 0.75), (9, 0, [-1.0, 0.0, 8.0], 2.0, [0.0, 1.0, 2.0], 3.0)] {
            let body = p.body(slot);
            body.set(&mut p.ram, Object::BOUND_CENTER, c).unwrap();
            body.set(&mut p.ram, Object::BOUND_RADIUS, r).unwrap();
            body.set(&mut p.ram, Object::DYNAMIC_CENTER, dc).unwrap();
            body.set(&mut p.ram, Object::DYNAMIC_RADIUS, dr).unwrap();
            body.set(&mut p.ram, Object::BODY_KIND, kind).unwrap();
        }
        let table = ObjectTable::import(&p.ram).unwrap().unwrap();
        let pool = PoolMem::new(&table);
        let words = Words(&p.ram);
        let guest = crate::guest::WordMem(&words);
        for handle in [a.raw(), b.raw(), 0x8004_0003, 0x8003_0009, 0x8003_0800, u32::MAX] {
            for light in [false, true] {
                let sim = object(&pool, handle, light);
                let read = object(&guest, handle, light);
                assert_eq!(sim.is_ok(), read.is_ok(), "{handle:#x} light {light}");
                if let (Ok(sim), Ok(read)) = (sim, read) {
                    assert_eq!((sim.centre, sim.radius, sim.kind), (read.centre, read.radius, read.kind));
                }
            }
        }
        let on = object(&pool, a.raw(), true).unwrap();
        assert_eq!((on.centre, on.radius, on.kind), ([4.0, 5.0, 6.0], 0.75, 1));
    }
}
