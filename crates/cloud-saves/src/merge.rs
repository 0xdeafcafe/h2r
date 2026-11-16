//! The merge: what to do with each item, from the last agreed state
//! (the base), what this machine has and what the server has. Pure: no
//! I/O, so the rules are tested over invented trees.
//!
//! The rules (docs/live/live-cloud-saves.md "Conflicts"):
//!
//! 1. Both sides the same: nothing moves; that is the new base.
//! 2. Only one side changed since the base: its change wins (a pull, a
//!    push, or the deletion carried over).
//! 3. One side deleted it and the other changed it: the change wins, so
//!    nothing is ever lost to a deletion.
//! 4. Both changed it, to different bytes: the side whose change is newer
//!    (the writer's own clock, [`crate::LocalItem::modified`] and
//!    [`StorageItem::modified`]) wins; a tie goes to the server. The losing
//!    bytes are kept as a conflict copy, never dropped.
//! 5. A save container is decided as a whole: when any file of
//!    `<directory>/` conflicts (rule 4), the winner of that container takes
//!    every file of it that differs, and each losing changed file is kept
//!    as a conflict copy. A profile's or variant's files stay a set that
//!    the game wrote together.

use std::collections::{BTreeMap, BTreeSet};

use crate::state::Base;
use crate::{Key, LocalItem, Sha256, StorageItem};

/// What to do with one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Both sides have the same thing: remember it as the base.
    Agree,
    /// Download the server's bytes over the local item. `keep_local`:
    /// first copy the local bytes away as a conflict copy.
    Pull { keep_local: bool },
    /// The server deleted it: delete the local item (kept as a copy).
    RemoveLocal,
    /// Upload the local item in place of the server's `if_match` version.
    /// `keep_remote`: first download the server's bytes as a conflict copy.
    Push { if_match: u64, keep_remote: bool },
    /// Delete the server's item, at version `if_match`.
    DeleteRemote { if_match: u64 },
}

/// Why, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Same,
    /// Changed on the server only (rule 2).
    RemoteChanged,
    /// Changed here only (rule 2).
    LocalChanged,
    /// Deleted on one side and changed on the other: the change won (rule 3).
    ChangeBeatDeletion,
    /// Both changed; the server's change is newer or ties (rule 4).
    RemoteNewer,
    /// Both changed; this machine's change is newer (rule 4).
    LocalNewer,
    /// Its container's conflict was decided for the other side (rule 5).
    ContainerFollows,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub key: Key,
    pub action: Action,
    pub why: Why,
}

impl Step {
    /// Whether this step keeps a conflict copy.
    pub fn conflict(&self) -> bool {
        matches!(
            self.action,
            Action::Pull { keep_local: true } | Action::Push { keep_remote: true, .. }
        )
    }
}

struct Facts<'a> {
    key: &'a Key,
    local: Option<&'a LocalItem>,
    remote: Option<Sha256>,
    remote_modified: i64,
    remote_version: u64,
    local_changed: bool,
    remote_changed: bool,
}

/// The steps that bring both sides to the same state, one per item that
/// any of the three knows, in key order.
pub fn plan(
    base: &BTreeMap<Key, Base>,
    local: &BTreeMap<Key, LocalItem>,
    remote: &BTreeMap<Key, StorageItem>,
) -> Vec<Step> {
    let keys: BTreeSet<&Key> = base.keys().chain(local.keys()).chain(remote.keys()).collect();
    let facts: Vec<Facts> = keys
        .into_iter()
        .map(|key| {
            let b = base.get(key);
            let l = local.get(key);
            let r = remote.get(key);
            let rh = r.filter(|r| !r.deleted).and_then(|r| r.sha256);
            let rv = r.map_or(0, |r| r.version);
            Facts {
                key,
                local: l,
                remote: rh,
                remote_modified: r.map_or(0, |r| r.modified),
                remote_version: rv,
                local_changed: match b {
                    Some(b) => l.map(|l| l.sha256) != b.sha256,
                    None => l.is_some(),
                },
                remote_changed: rv != b.map_or(0, |b| b.version),
            }
        })
        .collect();
    let differ = |f: &Facts| f.local.map(|l| l.sha256) != f.remote;
    let both_have = |f: &Facts| f.local.is_some() && f.remote.is_some();
    // Rule 4's conflicts, and rule 5's decision per container: newest
    // change on each side over the container's conflicting files.
    let mut groups: BTreeMap<(&str, &str), (i64, i64)> = BTreeMap::new();
    for f in &facts {
        let conflict = differ(f) && both_have(f) && f.local_changed == f.remote_changed;
        if conflict {
            let newest = groups.entry(f.key.group()).or_insert((i64::MIN, i64::MIN));
            newest.0 = newest.0.max(f.local.map_or(i64::MIN, |l| l.modified));
            newest.1 = newest.1.max(f.remote_modified);
        }
    }
    let local_wins = |f: &Facts| groups.get(&f.key.group()).map(|(l, r)| l > r);
    facts
        .iter()
        .map(|f| {
            let (action, why) = if !differ(f) {
                (Action::Agree, Why::Same)
            } else if both_have(f)
                && let Some(local_won) = local_wins(f)
            {
                let in_conflict = f.local_changed == f.remote_changed;
                if local_won {
                    let why = if in_conflict { Why::LocalNewer } else { Why::ContainerFollows };
                    let push = Action::Push {
                        if_match: f.remote_version,
                        keep_remote: f.remote_changed,
                    };
                    (push, why)
                } else {
                    let why = if in_conflict { Why::RemoteNewer } else { Why::ContainerFollows };
                    (Action::Pull { keep_local: f.local_changed }, why)
                }
            } else {
                one_sided(f)
            };
            Step {
                key: f.key.clone(),
                action,
                why,
            }
        })
        .collect()
}

/// Rules 2 and 3, for an item that differs but is not in a conflicting
/// container with content on both sides.
fn one_sided(f: &Facts) -> (Action, Why) {
    let pull = || match f.remote {
        Some(_) => Action::Pull { keep_local: false },
        None => Action::RemoveLocal,
    };
    let push = || match f.local {
        Some(_) => Action::Push {
            if_match: f.remote_version,
            keep_remote: false,
        },
        None => Action::DeleteRemote {
            if_match: f.remote_version,
        },
    };
    match (f.local_changed, f.remote_changed) {
        (false, true) => (pull(), Why::RemoteChanged),
        (true, false) => (push(), Why::LocalChanged),
        // Changed on both sides and one of them is a deletion (both with
        // content is a container conflict, above): the change wins.
        _ if f.local.is_some() => (push(), Why::ChangeBeatDeletion),
        _ => (pull(), Why::ChangeBeatDeletion),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KIND_SAVE, KIND_SETTINGS};

    fn h(s: &str) -> Sha256 {
        Sha256::of(s.as_bytes())
    }

    /// A tree: name -> (content, modified).
    fn local(items: &[(&str, &str, i64)]) -> BTreeMap<Key, LocalItem> {
        items
            .iter()
            .map(|(n, c, m)| {
                (
                    Key::save(n),
                    LocalItem {
                        sha256: h(c),
                        size: c.len() as u64,
                        modified: *m,
                    },
                )
            })
            .collect()
    }

    /// The server: name -> (content or None for a tombstone, version, modified).
    fn remote(items: &[(&str, Option<&str>, u64, i64)]) -> BTreeMap<Key, StorageItem> {
        items
            .iter()
            .map(|(n, c, v, m)| {
                (
                    Key::save(n),
                    StorageItem {
                        kind: KIND_SAVE.into(),
                        name: n.to_string(),
                        version: *v,
                        sha256: c.map(h),
                        size: c.map_or(0, |c| c.len() as u64),
                        modified: *m,
                        stored_at: String::new(),
                        deleted: c.is_none(),
                    },
                )
            })
            .collect()
    }

    fn base(items: &[(&str, Option<&str>, u64)]) -> BTreeMap<Key, Base> {
        items
            .iter()
            .map(|(n, c, v)| {
                (
                    Key::save(n),
                    Base {
                        version: *v,
                        sha256: c.map(h),
                    },
                )
            })
            .collect()
    }

    fn actions(steps: &[Step]) -> Vec<(&str, Action, Why)> {
        steps
            .iter()
            .map(|s| (s.key.name.as_str(), s.action, s.why))
            .collect()
    }

    const PROFILE: &str = "A00000000001/profile";
    const META: &str = "A00000000001/SaveMeta.xbx";
    const VARIANT: &str = "B00000000002/slayer";

    #[test]
    fn a_new_machine_pulls_everything_and_an_old_one_pushes_its_own() {
        let steps = plan(
            &base(&[]),
            &local(&[]),
            &remote(&[(PROFILE, Some("p"), 3, 10), (VARIANT, Some("v"), 1, 10)]),
        );
        assert_eq!(
            actions(&steps),
            [
                (PROFILE, Action::Pull { keep_local: false }, Why::RemoteChanged),
                (VARIANT, Action::Pull { keep_local: false }, Why::RemoteChanged),
            ]
        );
        let steps = plan(&base(&[]), &local(&[(PROFILE, "p", 10)]), &remote(&[]));
        assert_eq!(
            actions(&steps),
            [(
                PROFILE,
                Action::Push {
                    if_match: 0,
                    keep_remote: false
                },
                Why::LocalChanged
            )]
        );
    }

    #[test]
    fn identical_bytes_agree_with_or_without_a_base() {
        let steps = plan(
            &base(&[]),
            &local(&[(PROFILE, "p", 99)]),
            &remote(&[(PROFILE, Some("p"), 4, 1)]),
        );
        assert_eq!(actions(&steps), [(PROFILE, Action::Agree, Why::Same)]);
        // Deleted on both sides: agree on the tombstone.
        let steps = plan(
            &base(&[(PROFILE, Some("p"), 4)]),
            &local(&[]),
            &remote(&[(PROFILE, None, 5, 1)]),
        );
        assert_eq!(actions(&steps), [(PROFILE, Action::Agree, Why::Same)]);
    }

    #[test]
    fn a_change_on_one_side_wins() {
        // The server moved on; this machine did not touch it.
        let steps = plan(
            &base(&[(PROFILE, Some("old"), 2)]),
            &local(&[(PROFILE, "old", 50)]),
            &remote(&[(PROFILE, Some("new"), 3, 10)]),
        );
        assert_eq!(
            actions(&steps),
            [(PROFILE, Action::Pull { keep_local: false }, Why::RemoteChanged)]
        );
        // This machine changed it, even with an older clock.
        let steps = plan(
            &base(&[(PROFILE, Some("old"), 2)]),
            &local(&[(PROFILE, "new", 1)]),
            &remote(&[(PROFILE, Some("old"), 2, 99)]),
        );
        assert_eq!(
            actions(&steps),
            [(
                PROFILE,
                Action::Push {
                    if_match: 2,
                    keep_remote: false
                },
                Why::LocalChanged
            )]
        );
    }

    #[test]
    fn deletions_carry_over_when_the_other_side_is_untouched() {
        let steps = plan(
            &base(&[(VARIANT, Some("v"), 2)]),
            &local(&[]),
            &remote(&[(VARIANT, Some("v"), 2, 10)]),
        );
        assert_eq!(
            actions(&steps),
            [(VARIANT, Action::DeleteRemote { if_match: 2 }, Why::LocalChanged)]
        );
        let steps = plan(
            &base(&[(VARIANT, Some("v"), 2)]),
            &local(&[(VARIANT, "v", 10)]),
            &remote(&[(VARIANT, None, 3, 10)]),
        );
        assert_eq!(
            actions(&steps),
            [(VARIANT, Action::RemoveLocal, Why::RemoteChanged)]
        );
    }

    #[test]
    fn a_change_beats_a_deletion_either_way() {
        // Deleted here, changed on the server: the change comes back.
        let steps = plan(
            &base(&[(VARIANT, Some("v"), 2)]),
            &local(&[]),
            &remote(&[(VARIANT, Some("v2"), 3, 10)]),
        );
        assert_eq!(
            actions(&steps),
            [(VARIANT, Action::Pull { keep_local: false }, Why::ChangeBeatDeletion)]
        );
        // Deleted on the server, changed here: it goes back up over the
        // tombstone.
        let steps = plan(
            &base(&[(VARIANT, Some("v"), 2)]),
            &local(&[(VARIANT, "v2", 1)]),
            &remote(&[(VARIANT, None, 3, 99)]),
        );
        assert_eq!(
            actions(&steps),
            [(
                VARIANT,
                Action::Push {
                    if_match: 3,
                    keep_remote: false
                },
                Why::ChangeBeatDeletion
            )]
        );
        // A machine that never saw the item, and a tombstone: same rule.
        let steps = plan(
            &base(&[]),
            &local(&[(VARIANT, "mine", 1)]),
            &remote(&[(VARIANT, None, 7, 99)]),
        );
        assert_eq!(
            steps[0].action,
            Action::Push {
                if_match: 7,
                keep_remote: false
            }
        );
    }

    #[test]
    fn both_changed_the_newer_wins_and_the_loser_is_kept() {
        let b = base(&[(VARIANT, Some("v"), 2)]);
        let steps = plan(
            &b,
            &local(&[(VARIANT, "here", 200)]),
            &remote(&[(VARIANT, Some("there"), 3, 100)]),
        );
        assert_eq!(
            actions(&steps),
            [(
                VARIANT,
                Action::Push {
                    if_match: 3,
                    keep_remote: true
                },
                Why::LocalNewer
            )]
        );
        assert!(steps[0].conflict());
        let steps = plan(
            &b,
            &local(&[(VARIANT, "here", 100)]),
            &remote(&[(VARIANT, Some("there"), 3, 200)]),
        );
        assert_eq!(
            actions(&steps),
            [(VARIANT, Action::Pull { keep_local: true }, Why::RemoteNewer)]
        );
        // A tie goes to the server.
        let steps = plan(
            &b,
            &local(&[(VARIANT, "here", 150)]),
            &remote(&[(VARIANT, Some("there"), 3, 150)]),
        );
        assert_eq!(steps[0].action, Action::Pull { keep_local: true });
        // Two machines that never synced, with different bytes: a conflict.
        let steps = plan(
            &base(&[]),
            &local(&[(VARIANT, "here", 1)]),
            &remote(&[(VARIANT, Some("there"), 1, 2)]),
        );
        assert_eq!(
            actions(&steps),
            [(VARIANT, Action::Pull { keep_local: true }, Why::RemoteNewer)]
        );
    }

    #[test]
    fn a_container_is_decided_as_a_whole() {
        // The profile conflicts and the server's is newer; SaveMeta.xbx
        // changed only here, so alone it would be pushed. The container
        // follows the server, and the local SaveMeta.xbx is kept.
        // The variant in another container is untouched by that decision.
        let steps = plan(
            &base(&[
                (PROFILE, Some("p"), 1),
                (META, Some("m"), 1),
                (VARIANT, Some("v"), 1),
            ]),
            &local(&[(PROFILE, "p-here", 100), (META, "m-here", 100), (VARIANT, "v-here", 100)]),
            &remote(&[
                (PROFILE, Some("p-there"), 2, 300),
                (META, Some("m"), 1, 0),
                (VARIANT, Some("v"), 1, 0),
            ]),
        );
        assert_eq!(
            actions(&steps),
            [
                (META, Action::Pull { keep_local: true }, Why::ContainerFollows),
                (PROFILE, Action::Pull { keep_local: true }, Why::RemoteNewer),
                (
                    VARIANT,
                    Action::Push {
                        if_match: 1,
                        keep_remote: false
                    },
                    Why::LocalChanged
                ),
            ]
        );
        // The other way round: this machine's is newer, so its files go up,
        // and the server's changed one is kept.
        let steps = plan(
            &base(&[(PROFILE, Some("p"), 1), (META, Some("m"), 1)]),
            &local(&[(PROFILE, "p-here", 300), (META, "m", 0)]),
            &remote(&[(PROFILE, Some("p-there"), 2, 100), (META, Some("m-there"), 2, 100)]),
        );
        assert_eq!(
            actions(&steps),
            [
                (
                    META,
                    Action::Push {
                        if_match: 2,
                        keep_remote: true
                    },
                    Why::ContainerFollows
                ),
                (
                    PROFILE,
                    Action::Push {
                        if_match: 2,
                        keep_remote: true
                    },
                    Why::LocalNewer
                ),
            ]
        );
    }

    #[test]
    fn settings_profiles_are_their_own_groups() {
        let a = Key::new(KIND_SETTINGS, "6368696566");
        let b = Key::new(KIND_SETTINGS, "6172626974");
        assert_ne!(a.group(), b.group());
        assert_eq!(Key::save(META).group(), Key::save(PROFILE).group());
        assert_eq!(Key::save("TitleMeta.xbx").group(), (KIND_SAVE, "TitleMeta.xbx"));
        assert!(Key::save(META).within(KIND_SAVE, "A00000000001"));
        assert!(!Key::save("A000000000012/x").within(KIND_SAVE, "A00000000001"));
    }
}
