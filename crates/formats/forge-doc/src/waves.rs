//! The wave set a document holds (docs/game/forge-firefight.md, "The wave
//! set").
//!
//! `ForgeDoc::waves` is one `firefight::Def`, the format the built-in sets
//! are written in, with no Forge wrapper. A document with one has format
//! [`FORMAT_WAVES`]; one without keeps the number its ops give it, so its
//! identity does not change. The set is checked against a fixed class list
//! ([`limits`]), so a document validates the same on a machine with no disc
//! and on Live.
//!
//! Zones are the kit's: zone `NN` is the `NN`th `ff_drop` by position, then
//! facing ([`zone_order`], the order `firefight_kit::ground::Labels::canonical`
//! gives). When a drop is added, moved or removed, [`remap`] keeps each
//! squad on the marker it prefers.
use serde::{Deserialize, Serialize};

use crate::{ForgeDoc, Refusal, Result, wire};

/// The definition and a squad's drop, as `firefight` has them.
pub use firefight::{Def, Drop};

/// The wire version of a document with a wave set.
pub const FORMAT_WAVES: u32 = content::forge::DOCUMENT_FORMAT_WAVES;

/// The most bytes a document's wave set has: the definition's own limit.
pub const MAX_WAVES_BYTES: usize = firefight::MAX_DOC_BYTES;

/// The most `ff_drop` markers a map has: the zones a definition can name
/// (`Drop::PreferZone` takes 0 to 11).
pub const MAX_DROPS: usize = 12;

/// The sentence a thirteenth drop is refused with.
pub const DROPS_FULL: &str = "A map has 12 drop zones at most.";

/// The classes a Forge map's wave set may name: the Elite the kit authors,
/// then `firefight_kit::retail::CLASSES` in keep order (forge-build's
/// `the_documents_classes_are_the_kits` holds the two together).
pub const CLASSES: [&str; 6] = ["elite", "grunt", "jackal", "drone", "brute", "hunter"];

/// The classes the stage-one ground builds: the Elite alone
/// (docs/game/firefight.md, "The kit").
pub const GROUND_CLASSES: [&str; 1] = ["elite"];

/// The set a map with none of its own plays: Holdout, the first built-in
/// (docs/game/firefight.md, "The definition").
pub fn default_set() -> Def {
    let [holdout, ..] = firefight::builtin::definitions();
    holdout
}

impl ForgeDoc {
    /// The document with `waves` as its wave set (`None` takes it out) and
    /// the format that needs. A document that already has it is returned
    /// as it is; any other change drops `expect`, as the identity changes.
    pub fn with_waves(&self, waves: Option<Def>) -> ForgeDoc {
        if self.waves == waves {
            return self.clone();
        }
        let mut doc = self.clone();
        doc.waves = waves;
        doc.format = doc.required_format();
        doc.expect = None;
        doc
    }
}

/// The kit limits a document's wave set is admitted against.
pub fn limits() -> firefight::Limits {
    firefight::Limits::new(CLASSES)
}

/// Admit a wave set for a document: at most [`MAX_WAVES_BYTES`], measured
/// before the validator reads a round, then `firefight::validate` against
/// [`limits`]. A refusal carries the validator's sentence.
pub fn check(def: &Def) -> Result<()> {
    wire::measure(def, "waves bytes", MAX_WAVES_BYTES)?;
    firefight::validate(def, &limits()).map_err(refused)
}

/// The validator's refusal as the document's: its sentence under `waves`.
pub fn refused(error: firefight::Refusal) -> Refusal {
    Refusal::invalid("waves", error.to_string())
}

/// The drops in zone order: by position, then facing, each coordinate and
/// the facing compared as `f32::total_cmp` does. `drops` are
/// `(identity, position, facing)` in any order; the answer is the
/// identities, zone 0 first.
pub fn zone_order<T: Clone>(drops: &[(T, [f32; 3], f32)]) -> Vec<T> {
    let mut sorted: Vec<&(T, [f32; 3], f32)> = drops.iter().collect();
    sorted.sort_by(|a, b| {
        a.1.iter()
            .zip(&b.1)
            .map(|(x, y)| x.total_cmp(y))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.total_cmp(&b.2))
    });
    sorted.into_iter().map(|d| d.0.clone()).collect()
}

/// `def` with each preferred zone renumbered from the zone order `old` to
/// `new`: a squad that prefers zone `z` keeps the marker `old[z]` under its
/// place in `new`, and one whose marker is gone returns to `Drop::Edge`. A
/// zone with no marker in `old` is left as it is.
pub fn remap<T: PartialEq>(def: &Def, old: &[T], new: &[T]) -> Def {
    let mut out = def.clone();
    for squad in out
        .rounds
        .iter_mut()
        .flat_map(|r| &mut r.waves)
        .flat_map(|w| &mut w.squads)
    {
        let firefight::Drop::PreferZone(zone) = squad.drop else {
            continue;
        };
        let Some(marker) = old.get(usize::from(zone)) else {
            continue;
        };
        squad.drop = new
            .iter()
            .position(|m| m == marker)
            .and_then(|at| u8::try_from(at).ok())
            .map_or(firefight::Drop::Edge, firefight::Drop::PreferZone);
    }
    out
}

/// What a set asks for that the built ground lacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// The squad's class is not one the ground built: never built, or cut
    /// for headroom.
    ClassNotBuilt,
    /// The squad prefers a zone no `ff_drop` marker gives.
    ZoneNotPlaced,
}

/// One squad's problem. `round`, `wave` and `squad` are one-based, as the
/// validator's sentences number them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub round: u8,
    pub wave: u8,
    pub squad: u8,
    pub what: ProblemKind,
}

impl Problem {
    /// The sentence the Waves page and a notice show.
    pub fn sentence(&self, def: &Def) -> String {
        let squad = def
            .rounds
            .get(usize::from(self.round) - 1)
            .and_then(|r| r.waves.get(usize::from(self.wave) - 1))
            .and_then(|w| w.squads.get(usize::from(self.squad) - 1));
        let at = format!(
            "Round {}, wave {}, squad {}",
            self.round, self.wave, self.squad
        );
        match (self.what, squad) {
            (ProblemKind::ClassNotBuilt, Some(s)) => {
                format!("{at}: the AI ground has no {}.", s.class)
            }
            (ProblemKind::ZoneNotPlaced, Some(s)) => match s.drop {
                firefight::Drop::PreferZone(z) => {
                    format!("{at}: no wave drop is zone {z}.")
                }
                firefight::Drop::Edge => format!("{at}: its wave drop is not placed."),
            },
            (ProblemKind::ClassNotBuilt, None) => format!("{at}: its class is not built."),
            (ProblemKind::ZoneNotPlaced, None) => format!("{at}: its wave drop is not placed."),
        }
    }
}

/// Each squad's problems against the ground: a class not in `classes`, and
/// a preferred zone at or past `drops` (the placed `ff_drop` count), in
/// round, wave and squad order, a class before a zone. Neither is a
/// refusal: the director takes any unseen zone when the preferred one is
/// absent, and the lobby's start names a class the map lacks.
pub fn problems(def: &Def, classes: &[String], drops: usize) -> Vec<Problem> {
    let mut out = Vec::new();
    for (r, round) in def.rounds.iter().enumerate() {
        for (w, wave) in round.waves.iter().enumerate() {
            for (s, squad) in wave.squads.iter().enumerate() {
                let at = |what| Problem {
                    round: (r + 1) as u8,
                    wave: (w + 1) as u8,
                    squad: (s + 1) as u8,
                    what,
                };
                if !classes.contains(&squad.class) {
                    out.push(at(ProblemKind::ClassNotBuilt));
                }
                if let firefight::Drop::PreferZone(zone) = squad.drop
                    && usize::from(zone) >= drops
                {
                    out.push(at(ProblemKind::ZoneNotPlaced));
                }
            }
        }
    }
    out
}
