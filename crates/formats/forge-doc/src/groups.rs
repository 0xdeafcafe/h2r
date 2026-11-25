//! Object groups: a name, the objects in it and two flags, made by one op.
//!
//! A group is a Forge-family op in the edit log. It names its members the
//! way a link or a label does ([`Endpoint`]), so a member survives later
//! appends and swap-removes, and [`ForgeDoc::groups`](crate::ForgeDoc::groups)
//! resolves the active groups to final indices. A group has no scenario
//! bytes: it is document data a session keeps in its own table, so a game
//! that reads none pays nothing for it (docs/game/forge-editing.md,
//! "Selection, groups and prefabs").
//!
//! An object is in at most one group. [`Forge::Ungroup`] ends the group the
//! op `group` made, and [`Forge::GroupLock`] sets or clears its lock. A
//! locked group is a fact the tool honours; the document only records it.
//! An entry in a group cannot be removed until its group ends, as a linked
//! or labelled entry cannot.
//!
//! A document with a group op is format 4 ([`FORMAT_GROUPS`](crate::FORMAT_GROUPS)).
//! One with none keeps its older number, so every older document keeps its
//! bytes and its identity.
use serde::{Deserialize, Serialize};

use crate::{BlockKind, Endpoint, OpId, Refusal, Result};

/// The most characters a group's name takes.
pub const MAX_GROUP_NAME: usize = 32;
/// The most objects one group holds.
pub const MAX_GROUP_MEMBERS: usize = 256;
/// The fewest objects a group holds: one object is not a group.
pub const MIN_GROUP_MEMBERS: usize = 2;

/// A group as its op carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// The group's name, which is its folder in the placed objects browser.
    pub name: String,
    /// The members in the order the author gave them; the first is the
    /// pivot when `pivot_first` says so.
    pub members: Vec<Endpoint>,
    /// The group turns about its first member, not its centre.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pivot_first: bool,
    /// The group is locked from the start.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A group an active op made, with its members' final indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveGroup {
    /// The op that made it.
    pub id: OpId,
    pub name: String,
    pub pivot_first: bool,
    pub locked: bool,
    /// Block and final index of each member, in the group's order.
    pub members: Vec<(BlockKind, u32)>,
}

/// The document's active groups, resolved against the base map's counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSet {
    /// In op order.
    pub active: Vec<ActiveGroup>,
}

/// A group's name: 1 to [`MAX_GROUP_NAME`] characters, not blank, with no
/// control character.
pub fn group_name(name: &str) -> Result<()> {
    let count = name.chars().count();
    if count == 0 || name.trim().is_empty() {
        return Err(Refusal::invalid("group", "must have a name"));
    }
    if count > MAX_GROUP_NAME {
        return Err(Refusal::invalid(
            "group",
            format!("a group's name is at most {MAX_GROUP_NAME} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(Refusal::invalid(
            "group",
            "name contains a control character",
        ));
    }
    Ok(())
}

/// The check that needs only the op.
pub(crate) fn check_op(group: &Group) -> Result<()> {
    group_name(&group.name)?;
    if group.members.len() < MIN_GROUP_MEMBERS || group.members.len() > MAX_GROUP_MEMBERS {
        return Err(Refusal::invalid(
            "group",
            format!("want {MIN_GROUP_MEMBERS} to {MAX_GROUP_MEMBERS} members"),
        ));
    }
    for (i, end) in group.members.iter().enumerate() {
        match *end {
            Endpoint::Base { index, .. } if index > i16::MAX as u32 => {
                return Err(Refusal::invalid("group", "entry index exceeds block limit"));
            }
            Endpoint::Added(OpId(0)) => {
                return Err(Refusal::invalid("group", "want the ID of an add op"));
            }
            _ => {}
        }
        if group.members[..i]
            .iter()
            .any(|m| m.identity() == end.identity())
        {
            return Err(Refusal::invalid("group", "an object is named twice"));
        }
    }
    Ok(())
}
