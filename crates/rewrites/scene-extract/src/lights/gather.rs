//! The main view's visible-light gather in Rust, equal to the list the
//! guest leaves at `[0x00547f88 + 0x10]` (docs/graphics/native-lights.md,
//! "The visible-light gather"). Nothing here reads a front-end product:
//! the inputs are the light datums, the light reference partition per
//! cluster, the tag table and the visibility sets of
//! [`crate::visibility`].
//!
//! The guest builds the list inside the view's visibility gather
//! (`0x001320f0`, mode 0 of `0x00132220`, kind 0 and flag `0x10` clear):
//!
//! - **Per cluster** (`0x00132419`..`0x001324ea`): for each visible
//!   cluster in the order reached, the light references at
//!   `[0x004e0310][cluster]`, nodes in the pool `[0x004e0314]` (`+4` the
//!   light handle, `+8` the next), each handed to `0x00132a30` with the
//!   cluster's ordinal. A node whose handle is `-1` ends the walk.
//! - **The light test** `0x00132a30`: `0x000c3140` (the record is live,
//!   `+0x24` and `+0x100` above the epsilon `0x0045dbdc`, a colour's
//!   squared length above `0x0045dc1c`) must hold; a light whose stamp
//!   (`+0xc`) is this pass's epoch (`[0x004e0308]`) is skipped; then the
//!   clip by the view's mode (`+0x2a7c`): 0 the cluster's frusta
//!   `0x00165010`, 1 the sphere `0x000461c0` (view radius plus `+0x24`),
//!   2 none, over the light's sphere `+0x18` and `+0x24`; a light the
//!   clip rejects is still taken under view flag `0x80`. It is appended
//!   by `0x00163c80`, which refuses at `count >= capacity - 1`, and
//!   stamped (`0x000c3110`) only when appended, so a refused or clipped
//!   light can be listed through a later cluster.
//! - **The player-only tail** (`0x00132589`..`0x00132611`), when the
//!   view's start cluster is not `0xffff`: `0x000c3950` takes at most
//!   four lights, in slot order, whose owner test `0x00031520` holds and
//!   whose definition flags have bits `0x20` and `0x40000`. Each that
//!   has no cluster (`+0x48` is `0xffff`) and is live and unstamped is
//!   appended and stamped. A view index of `-1` (`[0x004b9ed4]`) takes
//!   none.
//!
//! [`gather`] is that walk over a [`LightWorld`]; [`read_world`] fills the
//! world from the sim's datums. The list's order is the guest's append
//! order, which the ranking `0x00031910` reads.

use std::collections::{HashMap, HashSet};

use guest_layout::memory::GuestMemory;
use engine::render::light::{self, layout};
use engine::render::light_gather::visible;

use crate::visibility::flood::Reached;
use crate::visibility::objects::{Clip, view_flags};

/// The light reference partition: the heads array (a node datum per
/// cluster) and the node pool.
pub const CLUSTER_LIGHTS: (u32, u32) = (0x004e_0310, 0x004e_0314);
/// The main view's visibility struct and its light list at `+0x10`.
pub const MAIN_VIS: u32 = 0x0054_7f88;
pub const LIST: u32 = 0x10;
/// The view index `0x000c3950` is given (`-1` takes no player-only light).
pub const VIEW_INDEX: u32 = 0x004b_9ed4;
/// Visibility flag `0x10`: the view lists no lights.
pub const NO_LIGHTS: u32 = 0x10;
/// Lights `0x000c3950` takes at most (`push 4` at `0x00132594`).
pub const PLAYER_ONLY_LIMIT: usize = 4;
/// Definition flags a player-only light has (`0x000c39fd`, `0x000c3a05`).
const PLAYER_ONLY_BITS: [u32; 2] = [0x20, 0x4_0000];
const NONE: u32 = u32::MAX;
const CLUSTER: u32 = 0x48;
const OWNER: u32 = 0x4c;
const NO_CLUSTER: u16 = u16::MAX;

/// What the gather reads of one light record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightInfo {
    /// `0x000c3140`.
    pub active: bool,
    /// The sphere the clip tests: `+0x18` and `+0x24`.
    pub centre: [f32; 3],
    pub radius: f32,
    /// `+0x48`; `0xffff` for none.
    pub cluster: u16,
}

/// The sim's light references for the clusters a view reaches, and the
/// lights they name. Rust-owned: read once per view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightWorld {
    /// Per cluster, its light handles in list order.
    pub refs: HashMap<u16, Vec<u32>>,
    pub lights: HashMap<u32, LightInfo>,
    /// The lights `0x000c3950` takes, in order.
    pub player_only: Vec<u32>,
}

/// The view's inputs to the gather.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatherView {
    /// Visibility struct `+0x2a60`.
    pub flags: u32,
    /// Visibility struct `+8`; `0xffff` outside the structure.
    pub start: u16,
    /// The list's capacity (`[list + 0]`): it holds one fewer.
    pub capacity: usize,
}

/// The light handles the main view lists, in the guest's append order.
pub fn gather(world: &LightWorld, view: &GatherView, reached: &Reached, clip: &Clip) -> Vec<u32> {
    let mut list: Vec<u32> = Vec::new();
    if view.flags & NO_LIGHTS != 0 {
        return list;
    }
    let mut stamped: HashSet<u32> = HashSet::new();
    // 0x00163c80 then 0x000c3110: the stamp only when it fits.
    fn append(list: &mut Vec<u32>, stamped: &mut HashSet<u32>, capacity: usize, handle: u32) {
        if list.len() + 1 < capacity {
            list.push(handle);
            stamped.insert(handle & 0xffff);
        }
    }
    for &cluster in &reached.clusters {
        let Some(refs) = world.refs.get(&cluster) else { continue };
        for &handle in refs {
            let Some(info) = world.lights.get(&handle) else { continue };
            if !info.active || stamped.contains(&(handle & 0xffff)) {
                continue;
            }
            if clip.admits_sphere(cluster, info.centre, info.radius) || view.flags & view_flags::ADMIT_ALL != 0 {
                append(&mut list, &mut stamped, view.capacity, handle);
            }
        }
    }
    if view.start != NO_CLUSTER {
        for &handle in &world.player_only {
            let Some(info) = world.lights.get(&handle) else { continue };
            if info.cluster == NO_CLUSTER && info.active && !stamped.contains(&(handle & 0xffff)) {
                append(&mut list, &mut stamped, view.capacity, handle);
            }
        }
    }
    list
}

/// The main view's list for the clusters its visibility reached, in order,
/// under the clip of its mode: [`read_world`] and [`gather`] in one call.
/// `None` when the light data or the list's allocation does not read.
pub fn main_view(m: &dyn GuestMemory, flags: u32, start: u16, clusters: &[u16], clip: &Clip) -> Option<Vec<u32>> {
    let view = GatherView { flags, start, capacity: capacity(m)? };
    let world = read_world(m, clusters)?;
    let reached = Reached { clusters: clusters.to_vec(), ..Reached::default() };
    Some(gather(&world, &view, &reached, clip))
}

/// The list's capacity, from the allocation the guest keeps at
/// `[0x00547f88 + 0x10]`.
pub fn capacity(m: &dyn GuestMemory) -> Option<usize> {
    let list = m.u32(MAIN_VIS + LIST).ok().filter(|&p| p != 0)?;
    Some(m.u32(list).ok()? as usize)
}

/// Reads the world for `clusters`: the references of each, the lights
/// they name, and the player-only lights.
pub fn read_world(m: &dyn GuestMemory, clusters: &[u16]) -> Option<LightWorld> {
    let array = m.u32(light::ARRAY_POINTER).ok().filter(|&p| p != 0)?;
    if m.u32(array.checked_add(0x24)?).ok()? != light::RECORD_SIZE as u32 {
        return None;
    }
    let end = m.u32(array.checked_add(0x38)?).ok()?;
    let data = m.u32(array.checked_add(0x44)?).ok().filter(|&p| p != 0)?;
    if end > light::MAXIMUM_COUNT as u32 {
        return None;
    }
    let slot = |handle: u32| -> Option<u32> {
        let index = handle & 0xffff;
        if handle == NONE || index >= end {
            return None;
        }
        data.checked_add(index * light::RECORD_SIZE as u32)
    };
    let info = |handle: u32| -> Option<LightInfo> {
        let at = slot(handle)?;
        Some(LightInfo {
            active: visible::active(m, at).ok()?,
            centre: m.vec3(at.checked_add(layout::BOUNDS_CENTRE as u32)?).ok()?,
            radius: m.f32(at.checked_add(layout::BOUNDS_RADIUS as u32)?).ok()?,
            cluster: m.u16(at.checked_add(CLUSTER)?).ok()?,
        })
    };
    let mut world = LightWorld::default();
    let heads = m.u32(CLUSTER_LIGHTS.0).ok().filter(|&p| p != 0)?;
    let pool = m.u32(CLUSTER_LIGHTS.1).ok().filter(|&p| p != 0)?;
    let (stride, nodes_end, nodes) = (
        m.u32(pool.checked_add(0x24)?).ok()?,
        m.u32(pool.checked_add(0x38)?).ok()?,
        m.u32(pool.checked_add(0x44)?).ok().filter(|&p| p != 0)?,
    );
    if !(12..=0x1_0000).contains(&stride) || nodes_end > 0x1_0000 {
        return None;
    }
    for &cluster in clusters {
        if world.refs.contains_key(&cluster) {
            continue;
        }
        let mut handles = Vec::new();
        let mut node = m.u32(heads.checked_add(4 * u32::from(cluster))?).ok()?;
        for _ in 0..=nodes_end {
            if node == NONE {
                break;
            }
            if (node & 0xffff) >= nodes_end {
                return None;
            }
            let at = nodes.checked_add((node & 0xffff) * stride)?;
            let handle = m.u32(at.checked_add(4)?).ok()?;
            if handle == NONE {
                break;
            }
            handles.push(handle);
            if let std::collections::hash_map::Entry::Vacant(e) = world.lights.entry(handle) {
                e.insert(info(handle)?);
            }
            node = m.u32(at.checked_add(8)?).ok()?;
        }
        world.refs.insert(cluster, handles);
    }
    world.player_only = player_only(m, array, end, data)?;
    for &handle in &world.player_only {
        if let std::collections::hash_map::Entry::Vacant(e) = world.lights.entry(handle) {
            e.insert(info(handle)?);
        }
    }
    Some(world)
}

/// `0x000c3950`: from the light array's slots in use, in slot order, the
/// first [`PLAYER_ONLY_LIMIT`] whose owner test `0x00031520` holds and
/// whose definition has both [`PLAYER_ONLY_BITS`].
fn player_only(m: &dyn GuestMemory, array: u32, end: u32, data: u32) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    if m.i32(VIEW_INDEX).ok()? == -1 {
        return Some(out);
    }
    let used = m.u32(array.checked_add(0x48)?).ok().filter(|&p| p != 0)?;
    let tags = m.u32(guest_layout::tags::TAG_ENTRIES_GLOBAL).ok().filter(|&p| p != 0)?;
    for index in 0..end {
        if out.len() == PLAYER_ONLY_LIMIT {
            break;
        }
        let word = m.u32(used.checked_add(4 * (index >> 5))?).ok()?;
        if word & (1 << (index & 31)) == 0 {
            continue;
        }
        let at = data.checked_add(index * light::RECORD_SIZE as u32)?;
        let handle = (u32::from(m.u16(at).ok()?) << 16) | index;
        let definition = m.u32(at.checked_add(layout::DEFINITION as u32)?).ok()?;
        if definition == NONE {
            continue;
        }
        // The guest takes the tag's data pointer unchecked.
        let tag = m.u32(tags.checked_add((definition & 0xffff) * 16 + 8)?).ok()?;
        let flags = m.u32(tag).ok()?;
        let owner = m.u32(at.checked_add(OWNER)?).ok()?;
        if visible::player_light_visible(m, at, flags, owner).ok()? && PLAYER_ONLY_BITS.iter().all(|b| flags & b != 0) {
            out.push(handle);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
