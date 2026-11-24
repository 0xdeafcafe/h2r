//! Object groups in a session (docs/game/forge-editing.md, "Selection,
//! groups and prefabs").
//!
//! A group is a Forge-family op ([`forge_doc::Group`]); the session reads
//! the active ones off the document and keeps nothing of its own. Moving a
//! member of a group moves the whole group: [`Session::move_thing`] writes
//! a position and a rotation for each member in one undo step, as a delete
//! of a linked machine is one step. The group turns about its centre, or
//! about its first member when its pivot says so.
use forge_doc::{Edit, Endpoint, Forge, Group, OpId};

use super::{NO_THING, Session};
use crate::{Frame, Thing, World};

/// An active group, its members named by identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupInfo {
    /// The op that made it.
    pub id: OpId,
    pub name: String,
    pub locked: bool,
    pub pivot_first: bool,
    /// In the order the group was made with; the first is the pivot when
    /// `pivot_first`.
    pub members: Vec<Endpoint>,
}

impl GroupInfo {
    fn of((id, group): (OpId, Group)) -> Self {
        Self {
            id,
            name: group.name,
            locked: group.locked,
            pivot_first: group.pivot_first,
            members: group.members,
        }
    }

    /// Whether the group has `end`.
    pub fn has(&self, end: Endpoint) -> bool {
        let end = end.identity();
        self.members.iter().any(|m| m.identity() == end)
    }
}

/// The frames the members of a group take when the member `held` is
/// carried from its frame in `now` to `to`. The group turns rigidly about
/// its pivot ([`Group::pivot_first`]): by the yaw the held member turns,
/// and the held member lands exactly on `to`. Pitch and roll are the
/// held member's alone; the others keep theirs.
pub fn carried(now: &[Frame], pivot_first: bool, held: usize, to: Frame) -> Vec<Frame> {
    let pivot = if pivot_first {
        now[0].position
    } else {
        let n = now.len() as f32;
        std::array::from_fn(|i| now.iter().map(|f| f.position[i]).sum::<f32>() / n)
    };
    let yaw = to.rotation[0] - now[held].rotation[0];
    let (s, c) = yaw.sin_cos();
    let turn = |p: [f32; 3]| {
        let d: [f32; 3] = std::array::from_fn(|i| p[i] - pivot[i]);
        [c * d[0] - s * d[1], s * d[0] + c * d[1], d[2]]
    };
    // Where the pivot lands, so that the held member lands on `to`.
    let turned = turn(now[held].position);
    let moved: [f32; 3] = std::array::from_fn(|i| to.position[i] - turned[i]);
    now.iter()
        .enumerate()
        .map(|(i, f)| {
            if i == held {
                return to;
            }
            let at = turn(f.position);
            Frame {
                position: std::array::from_fn(|k| moved[k] + at[k]),
                rotation: [
                    (f.rotation[0] + yaw).rem_euclid(std::f32::consts::TAU),
                    f.rotation[1],
                    f.rotation[2],
                ],
            }
        })
        .collect()
}

impl Session {
    /// The active groups, by the op that made each.
    pub fn groups(&self) -> Vec<GroupInfo> {
        self.doc
            .group_targets(&self.limits.base_counts)
            .unwrap_or_default()
            .into_iter()
            .map(GroupInfo::of)
            .collect()
    }

    /// The group that has the thing `id` names.
    pub fn group_of(&self, id: Endpoint) -> Option<GroupInfo> {
        self.groups().into_iter().find(|g| g.has(id))
    }

    /// The members of the group of `id`, as things, in the group's order;
    /// just the thing when it is in no group.
    pub fn group_things(&self, id: Endpoint) -> Result<Vec<Thing>, String> {
        let thing = self.thing(id).ok_or(NO_THING)?;
        match self.group_of(id) {
            None => Ok(vec![thing]),
            Some(group) => group
                .members
                .iter()
                .map(|m| self.thing(*m).ok_or_else(|| NO_THING.to_string()))
                .collect(),
        }
    }

    /// The frames every thing of the group of `id` takes when `id` is
    /// carried to `frame`, in the group's order, the held thing first
    /// being itself where it stands in the order. A thing in no group is
    /// just itself.
    pub fn group_frames(
        &self,
        id: Endpoint,
        frame: Frame,
    ) -> Result<Vec<(Endpoint, Frame)>, String> {
        let things = self.group_things(id)?;
        let Some(group) = self.group_of(id) else {
            return Ok(vec![(things[0].id, frame)]);
        };
        let held = things
            .iter()
            .position(|t| t.id.identity() == id.identity())
            .ok_or(NO_THING)?;
        let now: Vec<Frame> = things.iter().map(|t| t.frame).collect();
        let to = carried(&now, group.pivot_first, held, frame);
        Ok(things.iter().map(|t| t.id).zip(to).collect())
    }

    /// Group things under a name in one op. A thing is in one group; the
    /// group has two or more.
    pub fn group(
        &mut self,
        world: &mut impl World,
        tick: u32,
        name: &str,
        members: &[Endpoint],
        pivot_first: bool,
    ) -> Result<OpId, String> {
        self.restore(world)?;
        let members = members
            .iter()
            .map(|m| Ok(self.thing(*m).ok_or(NO_THING)?.id.identity()))
            .collect::<Result<Vec<_>, String>>()?;
        let ops = self.ops(vec![Edit::Forge(Forge::Group(Group {
            name: name.into(),
            members,
            pivot_first,
            locked: false,
        }))])?;
        let id = ops[0].id;
        self.edit(world, tick, ops, None)?;
        Ok(id)
    }

    /// End the group the op `group` made.
    pub fn ungroup(
        &mut self,
        world: &mut impl World,
        tick: u32,
        group: OpId,
    ) -> Result<(), String> {
        self.restore(world)?;
        let ops = self.ops(vec![Edit::Forge(Forge::Ungroup { group })])?;
        self.edit(world, tick, ops, None)
    }

    /// Lock or unlock a group. The tool does not aim at a locked group's
    /// things.
    pub fn set_group_locked(
        &mut self,
        world: &mut impl World,
        tick: u32,
        group: OpId,
        locked: bool,
    ) -> Result<(), String> {
        self.restore(world)?;
        let info = self
            .groups()
            .into_iter()
            .find(|g| g.id == group)
            .ok_or("No such group.")?;
        if info.locked == locked {
            return Ok(());
        }
        let ops = self.ops(vec![Edit::Forge(Forge::GroupLock { group, locked })])?;
        self.edit(world, tick, ops, None)
    }

    /// Whether the thing `id` names is in a locked group.
    pub fn locked(&self, id: Endpoint) -> bool {
        self.group_of(id).is_some_and(|g| g.locked)
    }

    /// The edits that take `id` out of its group before it is removed: the
    /// group ends and, when two or more remain, is made again without it,
    /// with its name, pivot and lock. Nothing when it is in no group.
    pub(super) fn leave_group(&self, id: Endpoint) -> Vec<Edit> {
        let Some(group) = self.group_of(id) else {
            return vec![];
        };
        let rest: Vec<Endpoint> = group
            .members
            .iter()
            .copied()
            .filter(|m| m.identity() != id.identity())
            .collect();
        let mut edits = vec![Edit::Forge(Forge::Ungroup { group: group.id })];
        if rest.len() >= forge_doc::MIN_GROUP_MEMBERS {
            edits.push(Edit::Forge(Forge::Group(Group {
                name: group.name,
                members: rest,
                pivot_first: group.pivot_first,
                locked: group.locked,
            })));
        }
        edits
    }
}
