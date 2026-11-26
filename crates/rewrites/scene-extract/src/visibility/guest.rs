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
/// The second sphere a view of kind not 0 tests (`0x001330bc`): centre and
/// radius.
pub const BODY_LIGHT_SPHERE_CENTER: u32 = 0x50;
pub const BODY_LIGHT_SPHERE_RADIUS: u32 = 0x5c;

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

/// Which of a body's two spheres a view tests (`0x00133050`): the main
/// view's `+0x30`/`+0x3c`, every other kind's `+0x50`/`+0x5c`.
#[derive(Clone, Copy)]
struct SphereAt {
    centre: u32,
    radius: u32,
}

const MAIN_SPHERE: SphereAt = SphereAt {
    centre: BODY_SPHERE_CENTER,
    radius: BODY_SPHERE_RADIUS,
};
const LIGHT_SPHERE: SphereAt = SphereAt {
    centre: BODY_LIGHT_SPHERE_CENTER,
    radius: BODY_LIGHT_SPHERE_RADIUS,
};

fn object(
    mem: &dyn GuestRead,
    objects: &Pool,
    handle: u32,
    sphere: SphereAt,
) -> Result<ObjectInfo, Decline> {
    let u = Decline::Unread;
    if handle & 0xffff >= MAX_OBJECT_INDEX {
        return Err(Decline::Layout);
    }
    let slot = objects.slot(mem, handle)?;
    let body = ptr_at(mem, at(slot, HEADER_BODY).ok_or(u)?).ok_or(u)?;
    Ok(ObjectInfo {
        centre: vec3_at(mem, at(body, sphere.centre).ok_or(u)?).ok_or(u)?,
        radius: f32_at(mem, at(body, sphere.radius).ok_or(u)?).ok_or(u)?,
        kind: u8_at(mem, at(body, BODY_TYPE).ok_or(u)?).ok_or(u)?,
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
    let globals = ptr_at(mem, PLAYER_GLOBALS).ok_or(u)?;
    let player = u32_at(mem, at(globals, LOCAL_PLAYERS + 4 * user).ok_or(l)?).ok_or(u)?;
    if player == NONE {
        return Ok(None);
    }
    let camera = USER_CAMERAS + USER_CAMERA_SIZE * user;
    let kind = u32_at(mem, camera + USER_CAMERA_KIND).ok_or(u)?;
    let test = f32_at(mem, camera + USER_CAMERA_TEST).ok_or(u)?;
    let zero = f32_at(mem, CAMERA_TEST_ZERO).ok_or(u)?;
    if kind != FIRST_PERSON_CAMERA || test != zero {
        return Ok(None);
    }
    let players = ptr_at(mem, PLAYERS).ok_or(u)?;
    let data = ptr_at(mem, at(players, 0x44).ok_or(l)?).ok_or(u)?;
    let slot = (player & 0xffff)
        .checked_mul(PLAYER_SIZE)
        .and_then(|o| at(data, o))
        .and_then(|s| at(s, PLAYER_UNIT))
        .ok_or(l)?;
    let unit = u32_at(mem, slot).ok_or(u)?;
    Ok((unit != NONE).then_some(unit))
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
    references_into(mem, &mut world, clusters, MAIN_SPHERE)?;
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
    references_into(mem, &mut world, clusters, LIGHT_SPHERE)?;
    Ok(world)
}

/// The references of `clusters` and of `world.sky` into `world`, with the
/// objects they name.
fn references_into(
    mem: &dyn GuestRead,
    world: &mut ObjectWorld,
    clusters: &[u16],
    sphere: SphereAt,
) -> Result<(), Decline> {
    let u = Decline::Unread;
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
            if let std::collections::hash_map::Entry::Vacant(e) = world.objects.entry(handle) {
                e.insert(object(mem, &objects, handle, sphere)?);
            }
        }
        world.refs.insert(cluster, lists);
    }
    Ok(())
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
