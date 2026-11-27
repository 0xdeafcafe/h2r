//! **Rebuild map** and the reload after **Build AI ground**
//! (docs/game/forge-design.md, "Rebuild map"; docs/game/forge-map-variants.md,
//! "AI ground").
//!
//! The game builds a play copy of the map from the document and reloads
//! the running map onto it; the session then goes on over the copy with the
//! same document, log, undo and redo ([`Session::reopen`]).
//!
//! **The live form.** A loaded block is never reordered under a running
//! game: an own entry the session removes is made inert where it is, and
//! the n-th entry the document adds to a block is row `base + n`. The
//! materialiser swap-removes a removed own entry, which would move rows.
//! So a play copy is built from the document's live form ([`live_form`]):
//! the removal of an own entry of an object block becomes a write of an
//! empty palette index, which makes no object; the removal of an own entry
//! of any other block is left out. Every row of the copy is then at the
//! index the session has it at, and the adds are rows already present.
//!
//! **Reopening.** The session takes the copy's rows as what the world holds:
//! the added objects the game made from their rows (by the handles the game
//! found for them), the own entries as the live form leaves them, and the
//! kit's rows. It then brings the world to the document as after any edit,
//! so a removal the live form left out is made inert again and an added
//! object the game did not make is placed. Nothing already present is
//! placed twice.
use std::collections::{BTreeMap, BTreeSet};

use forge_doc::{BlockKind, Edit, Endpoint, EntryOp, FieldWrite, ForgeDoc, Link, OpId, Value};
use map::scenario_layout::{self as layout, Role};

use super::{Session, entries, reason, scene, things};
use crate::{PlacedObject, World};

/// Which play copy the running map was reloaded onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyKind {
    /// **Rebuild map**: the document with the kit, without the AI ground.
    Rebuild,
    /// **Build AI ground** or **Test waves**: the play form with the
    /// ground, which has no spare rows for the kit.
    Ground,
}

/// What the running map holds of the document: the form it was built
/// from, by digest, and the device links baked into it.
#[derive(Clone, Debug, Default)]
pub(super) struct Baked {
    /// The digest of the form the running map was built from; none when
    /// it could not be taken.
    form: Option<forge_doc::Sha256>,
    links: BTreeSet<OpId>,
    /// The ground stamp the running map was built with.
    ground: Option<forge_doc::Sha256>,
}

impl Baked {
    /// The running map holds `form`, and nothing else of a document.
    pub(super) fn of(form: &ForgeDoc) -> Self {
        Self {
            form: digest(form),
            ..Self::default()
        }
    }
}

fn digest(doc: &ForgeDoc) -> Option<forge_doc::Sha256> {
    let json = doc.canonical_json().ok()?;
    Some(forge_doc::Sha256::of(json.as_bytes()))
}

/// The document a play copy is built from: `doc` with each own entry's
/// removal in its live form (module doc), and no expectation, since the
/// identity is not the document's.
pub fn live_form(
    doc: &ForgeDoc,
    base_counts: &BTreeMap<BlockKind, u32>,
) -> Result<ForgeDoc, String> {
    let resolved = forge_doc::resolve(&doc.ops, base_counts).map_err(reason)?;
    let mut out = doc.clone();
    out.expect = None;
    out.ops = Vec::with_capacity(doc.ops.len());
    for op in &doc.ops {
        let Edit::Entry(EntryOp::Remove { target }) = &op.edit else {
            out.ops.push(op.clone());
            continue;
        };
        let Endpoint::Base { block, .. } = *target else {
            out.ops.push(op.clone());
            continue;
        };
        // A removal a later reset covers has no effect on the rows.
        if !resolved.lowered.contains_key(&op.id) {
            out.ops.push(op.clone());
            continue;
        }
        if let Some(field) = layout::block(block).role(Role::PaletteIndex) {
            let mut op = op.clone();
            op.edit = Edit::Entry(EntryOp::Set {
                target: *target,
                write: FieldWrite {
                    field: field.name.into(),
                    value: Value::Tag(None),
                },
            });
            out.ops.push(op);
        }
    }
    Ok(out)
}

/// What the game found on the reloaded map for [`Session::reopen`].
#[derive(Clone, Debug, Default)]
pub struct Reopened {
    /// The live object of each added object, by its add's op, as the game
    /// found it at the object's row ([`Session::baked_objects`]). An add
    /// with none is placed again.
    pub handles: BTreeMap<OpId, u32>,
    /// The spare rows each kit block has past its count on the reloaded
    /// map; none when it has no kit.
    pub room: Option<BTreeMap<BlockKind, u32>>,
}

impl Session {
    /// The form **Rebuild map** builds: the live form of the document, with
    /// the kit when the session has it, without the AI ground.
    pub fn rebuild_form(&self) -> Result<ForgeDoc, String> {
        live_form(&self.doc, &self.limits.base_counts)
    }

    /// The form the reload after an AI ground build is built from: the live
    /// form of the play form with the ground recipe. It is what
    /// [`Session::build_ground`] hands the builder.
    pub fn ground_copy_form(&self) -> Result<ForgeDoc, String> {
        let play = forge_doc::play_form(&self.ground_form());
        live_form(&play, &self.limits.base_counts)
    }

    /// Whether **Rebuild map** has anything to do: the running map was not
    /// built from what the rebuild would build.
    pub fn rebuild_needed(&self) -> bool {
        match self.rebuild_form().ok().as_ref().and_then(digest) {
            Some(now) => Some(now) != self.baked.form,
            None => true,
        }
    }

    /// Whether the running map holds the AI ground the document asks for
    /// now: it was reloaded onto a ground build whose stamp is current.
    pub fn ground_loaded(&self) -> bool {
        self.baked.ground.is_some() && self.baked.ground == self.ground_stamp().ok()
    }

    /// The device links the running map does not have yet: those a rebuild
    /// applies.
    pub(super) fn unbaked_device_links(&self) -> usize {
        self.resolved(&self.doc).map_or(0, |l| {
            l.active
                .iter()
                .filter(|a| matches!(a.link, Link::Device { .. }))
                .filter(|a| !self.baked.links.contains(&a.id))
                .count()
        })
    }

    /// The rows of `built` that are added objects: each add's op, its block
    /// and its row index on a map built from `built`. The game finds each
    /// one's live object by that row for [`Session::reopen`].
    pub fn baked_objects(&self, built: &ForgeDoc) -> Result<Vec<(OpId, BlockKind, u32)>, String> {
        let scene = scene::project(built, &self.limits, &self.palette, self.budget)?;
        Ok(scene
            .objects
            .iter()
            .filter_map(|o| Some((o.id, o.item.block, *scene.index.get(&o.id)?)))
            .collect())
    }

    /// Go on over the map reloaded onto a copy built from `built` (a
    /// [`Session::rebuild_form`] or [`Session::ground_copy_form`]): take
    /// the copy's rows as what the world holds, then bring the world to
    /// the document. The document, the log, undo and redo are kept; a held
    /// thing is let go. A refusal leaves the session as it was.
    pub fn reopen(
        &mut self,
        world: &mut impl World,
        built: &ForgeDoc,
        kind: CopyKind,
        found: Reopened,
    ) -> Result<(), String> {
        let before = self.clone();
        let result = self.adopt_copy(built, kind, found).and_then(|()| self.settle(world));
        if result.is_err() {
            *self = before;
        }
        result
    }

    /// The first half of [`Session::reopen`], which touches no world: take
    /// the copy's rows as what the world holds. A game whose world stages
    /// its calls from the session's objects ([`Session::objects`]) adopts
    /// first and then brings the world to the document with
    /// [`Session::release`].
    pub fn adopt_copy(
        &mut self,
        built: &ForgeDoc,
        kind: CopyKind,
        found: Reopened,
    ) -> Result<(), String> {
        let scene = scene::project(built, &self.limits, &self.palette, self.budget)?;
        let own = things::desired(built, &self.limits.base_counts, &self.base)?;
        let rows = entries::rows(&scene)?;
        self.objects = scene
            .objects
            .iter()
            .filter_map(|o| {
                Some(PlacedObject {
                    id: o.id,
                    handle: *found.handles.get(&o.id)?,
                    item: o.item.clone(),
                    frame: o.frame,
                })
            })
            .collect();
        self.own_live = own;
        // The copy's game starts with no spare clips (`forge_spare_clips`
        // empties its table at activation): the next settle writes them.
        self.clips_live.clear();
        if let Some(kit) = &mut self.kit {
            // The kit's room is what the copy has past its rows, and the
            // rows the document adds are in it already.
            let spare = found.room.unwrap_or_default();
            kit.room = BTreeMap::from_iter(crate::kit::BLOCKS.iter().map(|&block| {
                let added = rows.get(&block).map_or(0, |r| r.len() as u32);
                (block, spare.get(&block).copied().unwrap_or(0) + added)
            }));
            kit.live = rows;
            kit.repair = None;
        }
        self.repair = None;
        self.own_repair = None;
        self.held = None;
        self.baked = Baked {
            form: digest(built),
            links: self
                .resolved(built)?
                .active
                .iter()
                .filter(|a| matches!(a.link, Link::Device { .. }))
                .map(|a| a.id)
                .collect(),
            ground: match kind {
                CopyKind::Ground => self.ground_stamp().ok(),
                CopyKind::Rebuild => None,
            },
        };
        Ok(())
    }
}
