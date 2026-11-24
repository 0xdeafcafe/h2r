//! The view's object set (`0x00132220` with its per-object test
//! `0x00133050` and its clip `0x00132e60`, and the player append
//! `0x0003f33c`;
//! docs/graphics/native-visibility.md, "Objects"), over the object
//! references and spheres read into [`ObjectWorld`].

use std::collections::{HashMap, HashSet};

use super::Decline;
use super::flood::Reached;
use super::frusta::WindowFrusta;

/// The object type the first reference partition keeps when the view's
/// flags hold [`view_flags::FIRST_PARTITION_TYPE`].
pub const KEPT_TYPE: u8 = 7;

/// The view flags (visibility struct `+0x2a60`) the set reads.
pub mod view_flags {
    /// Every referenced object is admitted, seen or not.
    pub const ADMIT_ALL: u32 = 0x80;
    /// No objects.
    pub const NO_OBJECTS: u32 = 0x40;
    /// The first partition keeps only objects of [`super::KEPT_TYPE`].
    pub const FIRST_PARTITION_TYPE: u32 = 0x400;
}

/// What the object test reads of one object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectInfo {
    pub centre: [f32; 3],
    pub radius: f32,
    /// Its type byte (body `+0xaa`).
    pub kind: u8,
}

/// The sim's object references for the clusters a view needs, and the
/// objects they name. Rust-owned: read once per view (`super::guest`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectWorld {
    pub view_flags: u32,
    /// Per cluster, the handles of its two reference partitions, in list
    /// order.
    pub refs: HashMap<u16, [Vec<u32>; 2]>,
    pub objects: HashMap<u32, ObjectInfo>,
    /// The sky cluster whose objects are added unclipped, when the view
    /// has one.
    pub sky: Option<u16>,
    /// The view's player unit, appended when not listed.
    pub player: Option<u32>,
    /// The object list's capacity: it holds one fewer.
    pub capacity: usize,
}

/// How objects are clipped (`0x00132e60` by the view's mode).
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    /// Mode 0 (`0x00165010`): against the frusta of the windows of the
    /// cluster they are referenced from.
    Frusta(WindowFrusta),
    /// Mode 1: against the view's sphere (visibility struct `+0x2a68`, its
    /// radius `+0x2a74`).
    Sphere { centre: [f32; 3], radius: f32 },
    /// Mode 2: not clipped.
    All,
}

impl Clip {
    fn admits(&self, cluster: u16, info: &ObjectInfo) -> bool {
        match self {
            Self::Frusta(frusta) => frusta.admits(cluster, info.centre, info.radius),
            Self::Sphere { centre, radius } => {
                // In the guest's order; NaN rejects.
                let [dx, dy, dz] = [0, 1, 2].map(|i| info.centre[i] - centre[i]);
                let reach = info.radius + *radius;
                reach * reach >= (dz * dz + dx * dx) + dy * dy
            }
            Self::All => true,
        }
    }
}

struct Set<'a> {
    world: &'a ObjectWorld,
    listed: Vec<u32>,
    seen: HashSet<u32>,
}

impl Set<'_> {
    fn add(
        &mut self,
        handle: u32,
        admitted: impl FnOnce(&ObjectInfo) -> bool,
    ) -> Result<(), Decline> {
        let index = handle & 0xffff;
        if self.seen.contains(&index) {
            return if self.listed.contains(&handle) {
                Ok(())
            } else {
                Err(Decline::Layout)
            };
        }
        let info = self.world.objects.get(&handle).ok_or(Decline::Unread)?;
        if !(self.world.view_flags & view_flags::ADMIT_ALL != 0 || admitted(info)) {
            return Ok(());
        }
        // The guest drops what does not fit in its own walk order, which
        // differs from this one: a full list declines.
        if self.listed.len() + 1 >= self.world.capacity {
            return Err(Decline::Overflow);
        }
        self.seen.insert(index);
        self.listed.push(handle);
        Ok(())
    }
}

/// The handles the view lists, ascending.
pub fn object_set(
    world: &ObjectWorld,
    reached: &Reached,
    clip: Clip,
) -> Result<Vec<u32>, Decline> {
    let mut set = Set {
        world,
        listed: Vec::new(),
        seen: HashSet::new(),
    };
    if world.view_flags & view_flags::NO_OBJECTS == 0 && !reached.clusters.is_empty() {
        for &cluster in &reached.clusters {
            let [first, second] = world.refs.get(&cluster).ok_or(Decline::Unread)?;
            let seen = |info: &ObjectInfo| clip.admits(cluster, info);
            for &handle in first {
                let info = world.objects.get(&handle).ok_or(Decline::Unread)?;
                if world.view_flags & view_flags::FIRST_PARTITION_TYPE == 0
                    || info.kind == KEPT_TYPE
                {
                    set.add(handle, &seen)?;
                }
            }
            for &handle in second {
                set.add(handle, &seen)?;
            }
        }
        if let Some(sky) = world.sky {
            let [first, second] = world.refs.get(&sky).ok_or(Decline::Unread)?;
            for &handle in first.iter().chain(second) {
                set.add(handle, |_| true)?;
            }
        }
    }
    let mut listed = set.listed;
    if let Some(player) = world.player
        && !listed.contains(&player)
        && listed.len() + 1 < world.capacity
    {
        listed.push(player);
    }
    listed.sort_unstable();
    Ok(listed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::flood::tests::camera_at;
    use crate::visibility::flood::{Lens, Rect};
    use crate::visibility::frusta::tests::INPUTS;

    /// Mode 0's clip from the origin looking down +x.
    fn frusta(r: &Reached) -> Clip {
        let lens = Lens::new(&camera_at([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])).unwrap();
        Clip::Frusta(WindowFrusta::build(&r.windows, &lens, &INPUTS))
    }

    fn info(centre: [f32; 3], kind: u8) -> ObjectInfo {
        ObjectInfo {
            centre,
            radius: 0.5,
            kind,
        }
    }

    /// Clusters 1 and 2 seen, 2 through the right half of the screen only.
    /// Object 0x10001 in 1 ahead, 0x10002 in 2 ahead and right (seen),
    /// 0x10003 in 2 ahead and left (outside 2's window), 0x10004 in both 1
    /// and 2 (listed once), 0x10005 in cluster 7, not seen.
    fn world() -> (ObjectWorld, Reached) {
        let objects = HashMap::from([
            (0x10001, info([5.0, 0.0, 0.0], 0)),
            (0x10002, info([10.0, -3.0, 0.0], 0)),
            (0x10003, info([10.0, 3.0, 0.0], 0)),
            (0x10004, info([6.0, -1.0, 0.0], 7)),
            (0x10005, info([5.0, 0.0, 0.0], 0)),
        ]);
        let refs = HashMap::from([
            (1, [vec![0x10001, 0x10004], vec![]]),
            (2, [vec![0x10002], vec![0x10003, 0x10004]]),
            (7, [vec![0x10005], vec![]]),
        ]);
        let world = ObjectWorld {
            view_flags: 0,
            refs,
            objects,
            sky: None,
            player: None,
            capacity: 64,
        };
        let reached = Reached {
            clusters: vec![1, 2],
            windows: vec![(1, Rect::SCREEN), (2, Rect([0.0, 1.0, -1.0, 1.0]))],
        };
        (world, reached)
    }

    #[test]
    fn objects_are_listed_once_when_seen_through_their_clusters_windows() {
        let (w, r) = world();
        assert_eq!(
            object_set(&w, &r, frusta(&r)).unwrap(),
            [0x10001, 0x10002, 0x10004]
        );
        assert_eq!(
            object_set(&w, &r, Clip::All).unwrap(),
            [0x10001, 0x10002, 0x10003, 0x10004]
        );
    }

    #[test]
    fn the_sphere_mode_lists_objects_whose_spheres_touch_the_views() {
        let (w, r) = world();
        // Radius 5 plus each object's 0.5 reaches 5.5 from (5, 0, 0): 0x10002
        // at (10, -3, 0) is about 5.83 away, 0x10004 at (6, -1, 0) about 1.41.
        let sphere = Clip::Sphere {
            centre: [5.0, 0.0, 0.0],
            radius: 5.0,
        };
        assert_eq!(object_set(&w, &r, sphere).unwrap(), [0x10001, 0x10004]);
        let touching = Clip::Sphere {
            centre: [5.0, 0.0, 0.0],
            radius: 4.5,
        };
        assert!(touching.admits(1, &info([10.0, 0.0, 0.0], 0)), "touching counts");
        let nan = Clip::Sphere {
            centre: [f32::NAN, 0.0, 0.0],
            radius: 1.0,
        };
        assert!(!nan.admits(1, &info([0.0, 0.0, 0.0], 0)));
    }

    #[test]
    fn the_view_flags_the_sky_cluster_and_the_player_change_the_set() {
        let (mut w, r) = world();
        w.view_flags = view_flags::ADMIT_ALL;
        assert_eq!(
            object_set(&w, &r, frusta(&r)).unwrap(),
            [0x10001, 0x10002, 0x10003, 0x10004]
        );
        w.view_flags = view_flags::FIRST_PARTITION_TYPE;
        assert_eq!(
            object_set(&w, &r, frusta(&r)).unwrap(),
            [0x10004],
            "only type 7 from the first"
        );
        w.view_flags = view_flags::NO_OBJECTS;
        w.player = Some(0x20009);
        assert_eq!(
            object_set(&w, &r, frusta(&r)).unwrap(),
            [0x20009],
            "the player is appended alone"
        );
        w.view_flags = 0;
        w.sky = Some(7);
        assert_eq!(
            object_set(&w, &r, frusta(&r)).unwrap(),
            [0x10001, 0x10002, 0x10004, 0x10005, 0x20009]
        );
    }

    #[test]
    fn a_full_list_a_missing_object_and_a_reused_slot_decline() {
        let (mut w, r) = world();
        w.capacity = 3;
        assert_eq!(
            object_set(&w, &r, frusta(&r)),
            Err(Decline::Overflow)
        );
        let (mut w, r) = world();
        w.objects.remove(&0x10002);
        assert_eq!(
            object_set(&w, &r, frusta(&r)),
            Err(Decline::Unread)
        );
        let (mut w, r) = world();
        w.refs.get_mut(&2).unwrap()[0].push(0x20001);
        w.objects.insert(0x20001, info([5.0, -0.5, 0.0], 0));
        assert_eq!(
            object_set(&w, &r, frusta(&r)),
            Err(Decline::Layout),
            "slot 1 under two salts"
        );
    }
}
