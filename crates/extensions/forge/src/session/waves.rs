//! The wave set in a session (docs/game/forge-firefight.md, "The wave set").
//!
//! The session keeps the set beside its document, as it keeps the ground
//! recipe: a save writes it into the document ([`Session::variant_form`])
//! and a load takes it back out. It is not an op, so it is not in the log,
//! the undo stack or a film. Each edit is one [`Session::set_waves`] with
//! the whole definition, and a set `forge_doc::waves::check` refuses
//! changes nothing.
//!
//! Zones follow their markers: when an edit adds, moves or removes an
//! `ff_drop`, the session reads the zone order before and after and remaps
//! the set ([`forge_doc::waves::remap`]), so a squad keeps the marker it
//! prefers under its new number. A session holds [`MAX_DROPS`] drops at most.
use forge_doc::waves::{self, DROPS_FULL, Def, MAX_DROPS, Problem};
use forge_doc::{ForgeDoc, GROUND_BLOCK, GroundLabel, OpId, Refusal};

use super::{Session, scene};
use crate::net::HOST_SEAT;

/// The set and the one it was loaded with, beside the document.
#[derive(Clone, Debug, Default)]
pub(super) struct Track {
    set: Option<Def>,
    loaded: Option<Def>,
}

/// What a seat other than the host's is told when it edits the waves.
pub const WAVES_HOST_ONLY: &str = "In a shared session only the host edits the waves.";

impl Session {
    /// The session's own wave set; `None` plays the default
    /// ([`Session::waves_or_default`]).
    pub fn waves(&self) -> Option<&Def> {
        self.waves.set.as_ref()
    }

    /// Replace the set, or clear it with `None`. A set the validator
    /// refuses changes nothing and the refusal carries its sentence. In a
    /// shared session only the host's seat edits it.
    pub fn set_waves(&mut self, def: Option<Def>) -> Result<(), Refusal> {
        if self.seat().is_some_and(|seat| seat != HOST_SEAT) {
            return Err(Refusal::Invalid {
                what: "waves".into(),
                reason: WAVES_HOST_ONLY.into(),
            });
        }
        if let Some(def) = &def {
            waves::check(def)?;
        }
        self.waves.set = def;
        Ok(())
    }

    /// The set as the session loaded it: **Reset waves**.
    pub fn reset_waves(&mut self) {
        self.waves.set = self.waves.loaded.clone();
    }

    /// The set the session plays: its own, else Holdout.
    pub fn waves_or_default(&self) -> Def {
        self.waves.set.clone().unwrap_or_else(waves::default_set)
    }

    /// Whether the session plays the default set, having none of its own.
    pub fn waves_are_default(&self) -> bool {
        self.waves.set.is_none()
    }

    /// What the set the session plays asks for that the ground lacks: a
    /// class the ground does not build and a preferred zone no drop gives.
    pub fn wave_problems(&self) -> Vec<Problem> {
        let classes: Vec<String> = waves::GROUND_CLASSES.map(String::from).to_vec();
        waves::problems(
            &self.waves_or_default(),
            &classes,
            self.drop_count(&self.doc),
        )
    }

    /// The session's `ff_drop` markers in zone order: zone `NN` is the
    /// `NN`th, as the kit numbers them.
    pub fn drops(&self) -> Vec<OpId> {
        self.drop_order(&self.doc)
    }

    /// The drops `doc` places, as the session's markers: the netgame flags
    /// it adds that carry `ff_drop`, in zone order.
    fn drop_order(&self, doc: &ForgeDoc) -> Vec<OpId> {
        let Ok(scene) = scene::project(doc, &self.limits, &self.palette, self.budget) else {
            return vec![];
        };
        let Ok(labels) = doc.labels(&self.limits.base_counts) else {
            return vec![];
        };
        let drops: Vec<(OpId, [f32; 3], f32)> = scene
            .markers
            .iter()
            .filter(|m| {
                labels.active.iter().any(|a| {
                    a.block == GROUND_BLOCK
                        && a.index == m.index
                        && GroundLabel::of(&a.label.text) == Some(GroundLabel::Drop)
                })
            })
            .map(|m| (m.id, m.frame.position, m.frame.rotation[0]))
            .collect();
        waves::zone_order(&drops)
    }

    /// How many `ff_drop` labels `doc` has, on any flag.
    fn drop_count(&self, doc: &ForgeDoc) -> usize {
        doc.ground_marks(&self.limits.base_counts)
            .map(|marks| marks.iter().filter(|m| m.kind == GroundLabel::Drop).count())
            .unwrap_or(0)
    }

    /// Refuse a document that would hold more drops than [`MAX_DROPS`]
    /// and more than the session's has now.
    pub(super) fn check_drops(&self, next: &ForgeDoc) -> Result<(), String> {
        let count = self.drop_count(next);
        if count > MAX_DROPS && count > self.drop_count(&self.doc) {
            return Err(DROPS_FULL.into());
        }
        Ok(())
    }

    /// The zone order before an edit, when there is a set to keep on its
    /// markers.
    pub(super) fn zones_before(&self) -> Option<Vec<OpId>> {
        self.waves.set.as_ref().map(|_| self.drops())
    }

    /// After an edit: renumber the set's preferred zones from `before`'s
    /// order to the document's.
    pub(super) fn follow_drops(&mut self, before: Option<Vec<OpId>>) {
        let (Some(before), Some(set)) = (before, &self.waves.set) else {
            return;
        };
        let after = self.drops();
        if after != before {
            self.waves.set = Some(waves::remap(set, &before, &after));
        }
    }

    /// Take the wave set out of a loaded document: the session keeps it
    /// beside the document.
    pub(super) fn take_waves(doc: ForgeDoc) -> (ForgeDoc, Option<Def>) {
        let set = doc.waves.clone();
        (doc.with_waves(None), set)
    }

    pub(super) fn restart_waves(&mut self, set: Option<Def>) {
        self.waves = Track {
            loaded: set.clone(),
            set,
        };
    }
}
