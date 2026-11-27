//! Labels and prefabs in a session (docs/game/forge-editing.md,
//! "Selection, groups and prefabs"; docs/game/rules-engine.md, "Forge
//! labels").
//!
//! A label is a Forge-family op ([`forge_doc::Forge::Label`]) and the
//! session reads the active ones off the document. A prefab is a file the
//! session reads and writes beside its documents ([`forge_doc::Prefab`]);
//! placing one writes the ordinary add, label and group ops of its pieces
//! in one undo step, so the document keeps no trace of the file.
use std::path::Path;

use forge_doc::{
    Captured, Edit, Endpoint, FieldWrite, Forge, GroundLabel, Group, Label, MIN_GROUP_MEMBERS,
    OpId, Placement, Prefab, Value,
};

use super::{NO_THING, Session, reason, vector};
use crate::{Frame, files};

const ONLY_ADDED: &str = "Only objects placed in this session can be saved as a prefab.";

impl Session {
    /// The label the thing `id` names has, by identity.
    pub fn label_of(&self, id: Endpoint) -> Option<Label> {
        self.doc
            .label_targets(&self.limits.base_counts)
            .ok()?
            .remove(&id.identity())
    }

    /// The active labels, by the thing each names, in the document's order
    /// of identity.
    pub fn active_labels(&self) -> Vec<(Endpoint, Label)> {
        self.doc
            .label_targets(&self.limits.base_counts)
            .map(|m| m.into_iter().collect())
            .unwrap_or_default()
    }

    /// Give a thing a label, replace it, or clear it (`None`), in one undo
    /// step. A label is a name for a script to find the thing by
    /// ([`Label`]); the AI ground's labels are set on the AI and
    /// objectives page and are refused here, both to set and to replace.
    pub fn set_label(
        &mut self,
        world: &mut impl crate::World,
        tick: u32,
        id: Endpoint,
        label: Option<Label>,
    ) -> Result<(), String> {
        self.restore(world)?;
        let target = self.thing(id).ok_or(NO_THING)?.id.identity();
        let have = self.label_of(target);
        // A barrier is a label on an object: it is set here as any
        // other, and by `label_barrier` from the Siege page.
        let ground = |l: &Label| GroundLabel::of(&l.text).is_some_and(GroundLabel::on_flag);
        if label.as_ref().is_some_and(ground) || have.as_ref().is_some_and(ground) {
            return Err("A ground marker's label is set on the AI and objectives page or the Siege page.".into());
        }
        if let Some(label) = &label {
            label.check().map_err(reason)?;
        }
        if label.is_none() && have.is_none() {
            return Err("This has no label.".into());
        }
        if have == label {
            return Ok(());
        }
        let ops = self.ops(vec![Edit::Forge(Forge::Label { target, label })])?;
        self.edit(world, tick, ops, None)
    }

    /// The edit that clears the label of `id` before it is removed: a
    /// labelled entry cannot be removed. Nothing when it has none.
    pub(super) fn leave_label(&self, id: Endpoint) -> Vec<Edit> {
        match self.label_of(id) {
            Some(_) => vec![Edit::Forge(Forge::Label {
                target: id.identity(),
                label: None,
            })],
            None => vec![],
        }
    }

    /// The prefab `name` of the things `members` name, the first being the
    /// pivot. Only objects the session placed are saved: an own entry of
    /// the map has no tag of its own to name. Each piece's label goes with
    /// it, and two or more pieces place as a group.
    pub fn capture_prefab(&self, name: &str, members: &[Endpoint]) -> Result<Prefab, String> {
        let labels = self
            .doc
            .label_targets(&self.limits.base_counts)
            .map_err(reason)?;
        let mut seen = vec![];
        let mut pieces = vec![];
        for member in members {
            let end = member.identity();
            if seen.contains(&end) {
                continue;
            }
            seen.push(end);
            let Endpoint::Added(id) = end else {
                return Err(ONLY_ADDED.into());
            };
            let object = self.objects.iter().find(|o| o.id == id).ok_or(ONLY_ADDED)?;
            let frame = self.thing(end).ok_or(NO_THING)?.frame;
            pieces.push(Captured {
                block: object.item.block,
                tag: object.item.tag.clone(),
                pose: (frame.position, frame.rotation),
                label: labels.get(&end).cloned(),
            });
        }
        Prefab::capture(name, pieces.len() >= MIN_GROUP_MEMBERS, &pieces).map_err(reason)
    }

    /// Write the prefab of `members` to `<data>/forge/prefabs`.
    pub fn save_prefab(
        &self,
        data: &Path,
        name: &str,
        members: &[Endpoint],
    ) -> Result<Prefab, String> {
        let prefab = self.capture_prefab(name, members)?;
        files::save_prefab(data, &prefab)?;
        Ok(prefab)
    }

    /// Whether the prefab can be placed here: every tag it names is in the
    /// map's palette, and its pieces fit the budget. A tag the map lacks is
    /// named, and nothing is substituted.
    pub fn prefab_fits(&self, prefab: &Prefab) -> Result<(), String> {
        prefab.check().map_err(reason)?;
        if let Some(tag) = prefab.missing(|block, tag| {
            self.palette
                .iter()
                .any(|item| item.block == block && item.tag == *tag)
        }) {
            return Err(format!(
                "The map has no {} tag named {}.",
                tag.group, tag.path
            ));
        }
        let left = self.budget.saturating_sub(self.objects.len());
        if prefab.pieces.len() > left {
            return Err(format!(
                "{} has {} objects and only {left} more fit.",
                prefab.name,
                prefab.pieces.len()
            ));
        }
        Ok(())
    }

    /// Place a prefab with its pivot at `frame` (the position, and the yaw
    /// it faces): an object for each piece, its label, and the group when
    /// the prefab is one, all in one undo step. Returns the objects, in the
    /// prefab's order.
    pub fn place_prefab(
        &mut self,
        world: &mut impl crate::World,
        tick: u32,
        prefab: &Prefab,
        frame: Frame,
    ) -> Result<Vec<OpId>, String> {
        self.restore(world)?;
        self.prefab_fits(prefab)?;
        let poses = prefab.poses((frame.position, frame.rotation));
        let mut edits = vec![];
        for (piece, (position, rotation)) in prefab.pieces.iter().zip(poses) {
            let item = self
                .palette
                .iter()
                .find(|item| item.block == piece.block && item.tag == piece.tag)
                .ok_or("The prefab names a tag the map lacks.")?;
            edits.push(Edit::Placement(Placement::Add {
                block: item.block,
                like: None,
                fields: vec![
                    FieldWrite {
                        field: "palette index".into(),
                        value: Value::Tag(Some(item.tag.clone())),
                    },
                    vector("position", position)?,
                    vector("rotation", rotation)?,
                ],
            }));
        }
        let n = edits.len();
        // Labels and the group name the adds by their IDs, which the ops fix.
        let unset = Edit::Forge(Forge::Label {
            target: Endpoint::Added(OpId(0)),
            label: None,
        });
        let labelled: Vec<usize> = (0..n)
            .filter(|i| prefab.pieces[*i].label.is_some())
            .collect();
        edits.extend(labelled.iter().map(|_| unset.clone()));
        if prefab.grouped {
            edits.push(unset);
        }
        let mut ops = self.ops(edits)?;
        let ids: Vec<OpId> = ops[..n].iter().map(|op| op.id).collect();
        for (k, piece) in labelled.iter().enumerate() {
            ops[n + k].edit = Edit::Forge(Forge::Label {
                target: Endpoint::Added(ids[*piece]),
                label: prefab.pieces[*piece].label.clone(),
            });
        }
        if prefab.grouped {
            ops[n + labelled.len()].edit = Edit::Forge(Forge::Group(Group {
                name: prefab.name.clone(),
                members: ids.iter().map(|id| Endpoint::Added(*id)).collect(),
                pivot_first: true,
                locked: false,
            }));
        }
        self.edit(world, tick, ops, None)?;
        Ok(ids)
    }
}
