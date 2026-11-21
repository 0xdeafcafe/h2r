//! Kill points: class points times the difficulty multiplier and the
//! product of the active skulls' multipliers, all in integer thousandths.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Difficulty, MULTIPLIER_SCALE, Scoring, Skull};

/// Why a `Score` command credits points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreReason {
    /// The killing player.
    Kill,
    /// A second player who damaged the actor in its last five seconds.
    Assist,
}

/// Points for one kill of `class`, or `None` for a class without a score.
///
/// The product is formed exactly and rounded down once, so the result does
/// not depend on the order of the skulls.
pub fn kill_points(
    scoring: &Scoring,
    class: &str,
    difficulty: Difficulty,
    skulls: &BTreeSet<Skull>,
) -> Option<u32> {
    let base = *scoring.by_class.get(class)?;
    let scale = u128::from(MULTIPLIER_SCALE);
    let mut numerator =
        u128::from(base) * u128::from(scoring.difficulty_multiplier.get(difficulty));
    let mut denominator = scale;
    for skull in skulls {
        let multiplier = scoring
            .skull_multipliers
            .get(skull)
            .copied()
            .unwrap_or(MULTIPLIER_SCALE);
        // At most nine skulls of at most 2,000 thousandths each: the
        // numerator stays under 2^128.
        numerator *= u128::from(multiplier);
        denominator *= scale;
    }
    Some(u32::try_from(numerator / denominator).unwrap_or(u32::MAX))
}

/// The assist's share of a kill's points, rounded down.
pub fn assist_points(scoring: &Scoring, kill_points: u32) -> u32 {
    let share = u64::from(kill_points) * u64::from(scoring.assist) / u64::from(MULTIPLIER_SCALE);
    share as u32
}
