//! Clankermatch (docs/game/bots.md §8): matchmaking where every other
//! player is a Clanker this console runs.
//!
//! **The row.** The Xbox Live menu's Clankermatch row, after OptiMatch
//! (`crate::custom_games`), runs the OptiMatch row's press with
//! [`active`] set; every other row's press clears it.
//!
//! **The playlists.** The OptiMatch playlist screen (loader `0x00230c8d`)
//! builds its rows in `0x002b1ff4`, which asks
//! [`HOPPER_LISTED`](engine::points::HOPPER_LISTED) (`0x00192db0`, ECX the
//! descriptor index, AL the answer) once per hopper and stores the index in
//! each row. A `before` on it answers "not listed" for a hopper of the
//! other kind: with [`active`] only the server's clanker hoppers
//! (`live_hle::Hle::clanker_hoppers`), without it only the others. A
//! hopper it lets through still goes to the game's own check.
//!
//! **The search** is the HLE's (`live_hle::clankermatch`): a clanker
//! hopper's search finds no session after 4 to 20 s, and the game hosts.
//!
//! **The fill.** While this console hosts a clanker hopper's lobby, the
//! Clankers driver (`crate::clankers`) asks Live for the playlist's roster
//! and seats its bots as phantoms one at a time ([`schedule`]), on teams
//! [`balance`] picks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use feature::{BuildError, Builder, Flow, Manifest};
use live_proto::hopper::clankers::ClankerHoppers;

/// The feature (`features.clankermatch`).
pub const FEATURE: &str = "clankermatch";

/// The Clankermatch row's press set it; any other Live menu row's press
/// clears it.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the playlist screen is Clankermatch's.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// From the Live menu's press: `on` for the Clankermatch row.
pub fn set_active(on: bool) {
    if ACTIVE.swap(on, Ordering::Relaxed) != on {
        eprintln!(
            "clankermatch: {}",
            if on {
                "the playlists are the clanker hoppers"
            } else {
                "off: the playlists are the human hoppers"
            }
        );
    }
}

/// Whether the playlist list shows hopper `index`: a clanker hopper only
/// for Clankermatch, any other only for OptiMatch.
pub fn listed(index: u32, marks: &ClankerHoppers, clankermatch: bool) -> bool {
    marks.contains(index) == clankermatch
}

// ---- the fill ----

/// The shortest and longest wait before each bot joins (§8.4 item 2).
pub const MIN_GAP: Duration = Duration::from_millis(500);
pub const MAX_GAP: Duration = Duration::from_millis(3000);

/// SplitMix64: a small, seeded generator for the schedule.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// When each of `bots` bots joins, from the lobby's start: each one
/// [`MIN_GAP`] to [`MAX_GAP`] after the one before.
pub fn schedule(bots: usize, seed: u64) -> Vec<Duration> {
    let span = (MAX_GAP - MIN_GAP).as_millis() as u64 + 1;
    let mut state = seed;
    let mut at = Duration::ZERO;
    (0..bots)
        .map(|_| {
            at += MIN_GAP + Duration::from_millis(splitmix(&mut state) % span);
            at
        })
        .collect()
}

/// How many of a schedule's bots are in by `elapsed`.
pub fn seated(schedule: &[Duration], elapsed: Duration) -> usize {
    schedule.iter().take_while(|t| **t <= elapsed).count()
}

/// The teams of `skills` bots (in roster order) in a game of `teams`
/// teams with the humans' `humans` (each a team and a skill), by §9.1's
/// rule "the host balances by total skill": each bot goes to the team
/// with the fewest players, of those the lowest total skill, of those the
/// lowest index. With no teams (free for all), none.
pub fn balance(teams: u32, humans: &[(u8, f32)], skills: &[f32]) -> Vec<Option<u8>> {
    if teams < 2 {
        return vec![None; skills.len()];
    }
    let n = teams.min(16) as usize;
    let mut count = vec![0usize; n];
    let mut total = vec![0f32; n];
    for (t, k) in humans {
        let i = usize::from(*t);
        if i < n {
            count[i] += 1;
            total[i] += k;
        }
    }
    skills
        .iter()
        .map(|k| {
            let best = (0..n)
                .min_by(|a, b| {
                    count[*a]
                        .cmp(&count[*b])
                        .then(total[*a].total_cmp(&total[*b]))
                        .then(a.cmp(b))
                })
                .expect("two teams or more");
            count[best] += 1;
            total[best] += k;
            Some(best as u8)
        })
        .collect()
}

// ---- the hook ----

/// The playlist filter on [`HOPPER_LISTED`](engine::points::HOPPER_LISTED).
pub struct PlaylistFilter {
    /// `None` with no Live: every hopper is listed, as retail. Placed
    /// anyway, so the boot plan (and its tests) always holds the hook.
    pub hle: Option<Arc<live_hle::Hle>>,
}

impl feature::Feature for PlaylistFilter {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "Clankermatch: the OptiMatch playlists split into human and clanker hoppers",
        )
        .doc("docs/game/bots.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use engine::points::HOPPER_LISTED;
        let hle = self.hle.clone();
        let told = Arc::new(AtomicBool::new(false));
        b.raw()
            .before(&HOPPER_LISTED, "clanker or human hoppers", move |c| {
                let Some(hle) = &hle else {
                    return Ok(Flow::Continue);
                };
                let index = c.cpu.ecx;
                let marks = hle.clanker_hoppers();
                let on = active();
                if on && marks.hoppers.is_empty() && !told.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "clankermatch: the server named no clanker hoppers: the list is empty"
                    );
                }
                if listed(index, &marks, on) {
                    return Ok(Flow::Continue);
                }
                c.cpu.eax &= !0xff;
                c.ret(0).map(Flow::Return)
            });
        // Its state is the menu's choice and the server's marks, both read
        // afresh at each list build; nothing a snapshot would miss.
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the playlist filter; with no Live it lists every hopper.
pub fn install(
    registry: &mut feature::Registry,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<()> {
    registry
        .add(&PlaylistFilter { hle: live })
        .map_err(|e| anyhow::anyhow!("clankermatch playlists: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::hopper::clankers::ClankerHopper;

    fn marks(ids: &[u32]) -> ClankerHoppers {
        ClankerHoppers {
            hoppers: ids
                .iter()
                .map(|&hopper_id| ClankerHopper {
                    hopper_id,
                    playlist_id: hopper_id + 1,
                    players: 8,
                    teams: 0,
                    ranks_as: None,
                })
                .collect(),
        }
    }

    #[test]
    fn each_row_lists_its_own_hoppers() {
        let m = marks(&[8, 9, 10]);
        let human: Vec<u32> = (0..16).filter(|i| listed(*i, &m, false)).collect();
        let clanker: Vec<u32> = (0..16).filter(|i| listed(*i, &m, true)).collect();
        assert_eq!(clanker, vec![8, 9, 10]);
        assert_eq!(human, vec![0, 1, 2, 3, 4, 5, 6, 7, 11, 12, 13, 14, 15]);
        // No marks (an older server): OptiMatch as retail, Clankermatch
        // nothing.
        let none = ClankerHoppers::default();
        assert!((0..16).all(|i| listed(i, &none, false)));
        assert!((0..16).all(|i| !listed(i, &none, true)));
    }

    #[test]
    fn bots_join_half_a_second_to_three_seconds_apart() {
        for seed in [0, 1, 0xc1a4, u64::MAX] {
            let s = schedule(7, seed);
            assert_eq!(s.len(), 7);
            let mut last = Duration::ZERO;
            for t in &s {
                let gap = *t - last;
                assert!((MIN_GAP..=MAX_GAP).contains(&gap), "{seed}: {gap:?}");
                last = *t;
            }
        }
        // Seeded: the same lobby, the same schedule.
        assert_eq!(schedule(5, 42), schedule(5, 42));
        assert_ne!(schedule(5, 42), schedule(5, 43));
        assert!(schedule(0, 1).is_empty());
    }

    #[test]
    fn the_lobby_fills_one_at_a_time() {
        let s = schedule(3, 7);
        assert_eq!(seated(&s, Duration::ZERO), 0);
        assert_eq!(seated(&s, s[0] - Duration::from_millis(1)), 0);
        assert_eq!(seated(&s, s[0]), 1);
        assert_eq!(seated(&s, s[1]), 2);
        assert_eq!(seated(&s, s[2] + Duration::from_secs(60)), 3);
    }

    #[test]
    fn teams_balance_by_count_then_skill() {
        // Free for all: no teams.
        assert_eq!(balance(0, &[(0, 0.5)], &[0.5, 0.5]), vec![None, None]);
        // Two teams of four, the human on red: three bots join red, four
        // blue.
        let t = balance(2, &[(0, 0.5)], &[0.5; 7]);
        assert_eq!(t.iter().filter(|t| **t == Some(0)).count(), 3);
        assert_eq!(t.iter().filter(|t| **t == Some(1)).count(), 4);
        // The human on blue: the other way round.
        let t = balance(2, &[(1, 0.5)], &[0.5; 7]);
        assert_eq!(t.iter().filter(|t| **t == Some(0)).count(), 4);
        // Equal counts: the weaker side gets the next bot. A human of 1.0
        // on red, then bots of 0.9 and 0.1: blue takes the 0.9 (fewest
        // players); then red (1.0) and blue (0.9) have one each, and the
        // 0.1 goes to blue, the lower total.
        assert_eq!(balance(2, &[(0, 1.0)], &[0.9, 0.1]), vec![Some(1), Some(1)]);
        // A human on a team the game does not have counts for none.
        assert_eq!(balance(2, &[(5, 1.0)], &[0.5, 0.5]), vec![Some(0), Some(1)]);
    }
}
