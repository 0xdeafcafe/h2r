//! The base HUD's feed (docs/game/game-types-without-carriers.md, "The
//! display"): on a record on the script index no retail engine draws the
//! score or the clock, so the HUD kit's `base` HUD
//! (`hud_kit_core::views::objective::base`) shows them for each local
//! player: the score under the script's `points` label, with the score
//! limit when the record has one, and the time left when it is timed.
//!
//! [`state`] is a pure function of what the feed reads, so a test shows it
//! without a game. The feed reads the record's limits, the viewer's player
//! score (a team's, in a team game) and the round clock from guest memory
//! each tick; the label is the running script's header (`rules::feature::points_label`).
//! A machine that runs no script (a guest in a Live game) has no header to
//! read and says [`DEFAULT_LABEL`]. Every other game, and a viewer with no
//! player, hides both lines. The objective HUDs' feed is the script HUD
//! feed's (`rules::world::hud`), not this one.

use std::sync::Arc;

use guest_layout::GuestMemory;
use hud_kit_core::model::{Value, Values};
use hud_kit_core::views::objective::{Base, base};

use super::guest::{self, NO_TIME_LIMIT};
use crate::hud_kit::{self, Handle};

/// The feed's feature id in the plan.
pub const FEED: &str = "rules-base-hud";

/// What a point is called when the script's header is not on this machine.
pub const DEFAULT_LABEL: &str = "Score";

/// The base HUD's state for one viewer: `limit` the record's score limit
/// (0 for none), `ticks_left` the round clock's ticks ([`NO_TIME_LIMIT`]
/// for none) at `rate` ticks a second. A part-second left counts as the
/// whole second, as a countdown does.
pub fn state(points: &str, score: i32, limit: i32, ticks_left: i32, rate: i32) -> base::State {
    let rate = rate.max(1);
    base::State {
        points: points.to_owned(),
        score,
        limit: (limit > 0).then_some(limit),
        time_left: (ticks_left != NO_TIME_LIMIT)
            .then(|| (ticks_left.max(0) as u32).div_ceil(rate as u32)),
    }
}

/// Both lines hidden: no script game, or a viewer with no player.
pub fn hidden() -> Values {
    let mut v = Values::default();
    v.set(base::SCORE, Value::hidden());
    v.set(base::TIME, Value::hidden());
    v
}

/// The viewer's score: the team's in a team game, else the player's.
fn score_of(m: &dyn GuestMemory, slot: u8) -> i32 {
    let team_play = guest::variant(m, "team_play").unwrap_or(0) != 0;
    if team_play {
        let team = crate::firefight::runtime::read_players(m)
            .unwrap_or_default()
            .into_iter()
            .find(|(p, _)| p.slot == slot)
            .map(|(p, _)| p.team);
        if let Some(team) = team {
            return guest::team_score(m, team).unwrap_or(0);
        }
    }
    guest::player_score(m, slot).unwrap_or(0)
}

/// Whether the active record is on the script index.
pub fn on_script_index(m: &dyn GuestMemory) -> bool {
    guest::engine_index(m).ok().flatten() == Some(i32::from(::rules::SCRIPT_INDEX))
}

/// The values for the viewer at roster slot `slot` (None: no player), as
/// the game now stands.
pub fn values(m: &dyn GuestMemory, label: Option<&str>, slot: Option<u8>) -> Values {
    let Some(slot) = slot.filter(|_| on_script_index(m)) else {
        return hidden();
    };
    let s = state(
        label.unwrap_or(DEFAULT_LABEL),
        score_of(m, slot),
        guest::variant(m, "score_limit").unwrap_or(0),
        guest::round_time_left(m).unwrap_or(NO_TIME_LIMIT),
        ::rules::caps::TICKS_PER_SECOND,
    );
    base::values(&s)
}

struct FeedFeature {
    handle: Handle,
}

impl hooks::Feature for FeedFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEED, "Score and clock of a script game on the HUD kit")
            .doc("docs/game/game-types-without-carriers.md")
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Rebuilt from guest memory each tick.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let handle = self.handle;
        b.frame().on_tick("rules base hud: values", move |o| {
            let mem = o.guest();
            let m: &dyn GuestMemory = &*mem;
            let slots = crate::firefight::hud::local_slots(m).unwrap_or_default();
            let label = super::feature::points_label();
            for (user, slot) in slots.iter().enumerate() {
                let values = values(m, label.as_deref(), *slot);
                if user == 0 {
                    handle.publish(values);
                } else {
                    handle.publish_for(user as u32, slot.map(|_| values));
                }
            }
        });
        Ok(())
    }
}

/// Registers the base HUD with the HUD kit, hidden, and adds its feed.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    let handle = hud_kit::register(Arc::new(Base));
    handle.publish(hidden());
    registry
        .add(&FeedFeature { handle })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{FEED}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &Values, id: &str) -> Option<String> {
        v.get(id).filter(|v| v.shown).map(|v| v.text.clone())
    }

    #[test]
    fn a_limit_and_a_clock_make_both_lines() {
        let s = state("Kills", 12, 25, 754 * 30, 30);
        assert_eq!(s.limit, Some(25));
        assert_eq!(s.time_left, Some(754));
        let v = base::values(&s);
        assert_eq!(text(&v, base::SCORE).as_deref(), Some("Kills 12 of 25"));
        assert_eq!(text(&v, base::TIME).as_deref(), Some("Time left 12:34"));
    }

    #[test]
    fn no_limit_and_no_time_limit_leave_them_off() {
        let s = state("Points", 340, 0, NO_TIME_LIMIT, 30);
        assert_eq!((s.limit, s.time_left), (None, None));
        let v = base::values(&s);
        assert_eq!(text(&v, base::SCORE).as_deref(), Some("Points 340"));
        assert_eq!(text(&v, base::TIME), None);
    }

    #[test]
    fn a_part_second_counts_whole_and_a_zero_rate_does_not_divide() {
        assert_eq!(state("a", 0, 0, 31, 30).time_left, Some(2));
        assert_eq!(state("a", 0, 0, 0, 30).time_left, Some(0));
        assert_eq!(state("a", 0, 0, 5, 0).time_left, Some(5));
    }

    #[test]
    fn hidden_hides_both_lines() {
        let v = hidden();
        assert_eq!(text(&v, base::SCORE), None);
        assert_eq!(text(&v, base::TIME), None);
    }
}
