//! The medals Live awards from the ordered kills and bounces
//! (web/src/state/derive.ts). Keep the counters and last scoring kill; no
//! lifetime list of kills is needed.
use std::collections::BTreeMap;

/// A bounce off a superbounce crease: every bounce earns it.
pub const SUPERBOUNCE: &str = "superbounce";
/// The second bounce of a chain.
pub const DOUBLE_BOUNCE: &str = "double-bounce";
/// The third bounce of a chain.
pub const TRIPLE_BOUNCE: &str = "triple-bounce";
/// The fourth bounce of a chain.
pub const QUAD_BOUNCE: &str = "quad-bounce";
/// The fifth bounce of a chain.
pub const CHAIN_BOUNCE: &str = "chain-bounce";
/// The sixth bounce of a chain.
pub const BOUNCETACULAR: &str = "bouncetacular";
/// A death falling out of or through the map while trying a bounce (a
/// `Bounced` event).
pub const BOUNCED: &str = "bounced";
/// The bounce medal ids: the chain's, lowest first, then `bounced`.
pub const BOUNCE_MEDALS: [&str; 7] = [
    SUPERBOUNCE,
    DOUBLE_BOUNCE,
    TRIPLE_BOUNCE,
    QUAD_BOUNCE,
    CHAIN_BOUNCE,
    BOUNCETACULAR,
    BOUNCED,
];

/// The medals a bounce earns at its place in a chain, lowest first, so the
/// last is the one a HUD line shows (docs/game/superbounce.md, "The rule").
/// Superbounce is every bounce; a chain gives each of the others once, at
/// 2 to 6 bounces, and from the seventh on only Superbounce. A count of 0
/// is no bounce. The site keeps a copy (`bounceMedals` in
/// web/src/state/derive.ts).
pub fn bounce_medals(chain: u32) -> &'static [&'static str] {
    match chain {
        0 => &[],
        2 => &[SUPERBOUNCE, DOUBLE_BOUNCE],
        3 => &[SUPERBOUNCE, TRIPLE_BOUNCE],
        4 => &[SUPERBOUNCE, QUAD_BOUNCE],
        5 => &[SUPERBOUNCE, CHAIN_BOUNCE],
        6 => &[SUPERBOUNCE, BOUNCETACULAR],
        _ => &[SUPERBOUNCE],
    }
}

#[derive(Default)]
struct Player {
    deaths_to: Option<(u32, u64)>,
    last_kill: Option<u64>,
    chain: u32,
    spree: u32,
    awards: BTreeMap<String, u32>,
}

impl Player {
    fn award(&mut self, name: &'static str, report: &mut impl FnMut(&'static str)) {
        report(name);
        *self.awards.entry(name.into()).or_default() += 1;
    }
}

#[derive(Default)]
pub struct Tally {
    players: BTreeMap<u32, Player>,
    deaths: u64,
    last: Option<u32>,
}

impl Tally {
    #[cfg(test)]
    fn kill(&mut self, tick: u64, scoring_killer: Option<u32>, victim: u32) {
        self.kill_with(tick, scoring_killer, victim, |_| {});
    }

    /// Report each medal at its award tick as well as updating the menu totals.
    pub fn kill_with(
        &mut self,
        tick: u64,
        scoring_killer: Option<u32>,
        victim: u32,
        mut report: impl FnMut(&'static str),
    ) {
        self.deaths += 1;
        let dead = self.players.entry(victim).or_default();
        dead.last_kill = None;
        dead.spree = 0;
        let Some(killer) = scoring_killer else { return };
        dead.deaths_to = Some((killer, tick));
        let p = self.players.entry(killer).or_default();
        if self.last.replace(killer).is_none() {
            p.award("opening-act", &mut report);
        }
        p.chain = if p
            .last_kill
            .is_some_and(|last| tick.saturating_sub(last) <= 120)
        {
            p.chain + 1
        } else {
            1
        };
        p.last_kill = Some(tick);
        match p.chain {
            2 => p.award("twofer", &mut report),
            3 => p.award("hat-trick", &mut report),
            4 => p.award("full-house", &mut report),
            5.. => p.award("encore", &mut report),
            _ => {}
        }
        p.spree += 1;
        match p.spree {
            5 => p.award("hot-streak", &mut report),
            10 => p.award("wildfire", &mut report),
            15 => p.award("untouchable", &mut report),
            _ => {}
        }
        if p.deaths_to
            .is_some_and(|(by, at)| by == victim && tick.saturating_sub(at) <= 300)
        {
            p.award("payback", &mut report);
            p.deaths_to = None;
        }
    }

    /// A bounce at `chain`: each medal of [`bounce_medals`], reported at
    /// the bounce's tick as well as added to the menu totals.
    pub fn bounce_with(&mut self, player: u32, chain: u32, mut report: impl FnMut(&'static str)) {
        let p = self.players.entry(player).or_default();
        for &medal in bounce_medals(chain) {
            p.award(medal, &mut report);
        }
    }

    /// A death falling out of the map while trying a bounce: `bounced`.
    pub fn bounced_with(&mut self, player: u32, mut report: impl FnMut(&'static str)) {
        self.players
            .entry(player)
            .or_default()
            .award(BOUNCED, &mut report);
    }

    /// The final medal is resolved only when the match ends.
    pub fn last_word(&self) -> Option<u32> {
        self.last.filter(|_| self.deaths > 1)
    }

    pub fn counts(&self, datum: u32, ended: bool) -> BTreeMap<String, u32> {
        let mut counts = self
            .players
            .get(&datum)
            .map(|p| p.awards.clone())
            .unwrap_or_default();
        if ended && self.last_word() == Some(datum) {
            counts.insert("last-word".into(), 1);
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounce_medals_follow_the_chain_table() {
        assert_eq!(bounce_medals(0), &[] as &[&str]);
        assert_eq!(bounce_medals(1), [SUPERBOUNCE]);
        assert_eq!(bounce_medals(2), [SUPERBOUNCE, DOUBLE_BOUNCE]);
        assert_eq!(bounce_medals(3), [SUPERBOUNCE, TRIPLE_BOUNCE]);
        assert_eq!(bounce_medals(4), [SUPERBOUNCE, QUAD_BOUNCE]);
        assert_eq!(bounce_medals(5), [SUPERBOUNCE, CHAIN_BOUNCE]);
        assert_eq!(bounce_medals(6), [SUPERBOUNCE, BOUNCETACULAR]);
        for chain in [7, 8, 12, u32::MAX] {
            assert_eq!(bounce_medals(chain), [SUPERBOUNCE], "{chain}");
        }
        assert_eq!(
            BOUNCE_MEDALS,
            [
                "superbounce",
                "double-bounce",
                "triple-bounce",
                "quad-bounce",
                "chain-bounce",
                "bouncetacular",
                "bounced",
            ]
        );
    }

    /// A chain of eight is eight Superbounces and one of each of the
    /// others; a bounce death is `bounced`.
    #[test]
    fn a_chain_of_eight_counts_each_chain_medal_once() {
        let mut tally = Tally::default();
        let mut reported = Vec::new();
        for chain in 1..=8 {
            tally.bounce_with(4, chain, |m| reported.push(m));
        }
        tally.bounce_with(4, 1, |m| reported.push(m));
        tally.bounced_with(4, |m| reported.push(m));
        assert_eq!(reported.iter().filter(|&&m| m == SUPERBOUNCE).count(), 9);
        assert_eq!(reported.last(), Some(&BOUNCED));
        assert_eq!(
            tally.counts(4, true),
            BTreeMap::from([
                (SUPERBOUNCE.into(), 9),
                (DOUBLE_BOUNCE.into(), 1),
                (TRIPLE_BOUNCE.into(), 1),
                (QUAD_BOUNCE.into(), 1),
                (CHAIN_BOUNCE.into(), 1),
                (BOUNCETACULAR.into(), 1),
                (BOUNCED.into(), 1),
            ])
        );
        // Bounces are no kills: no last-word, no opening act.
        assert_eq!(tally.last_word(), None);
    }

    #[test]
    fn death_resets_chain_and_revenge_follows_salted_participant() {
        let mut tally = Tally::default();
        tally.kill(1, Some(7), 2);
        tally.kill(121, Some(7), 3);
        tally.kill(122, Some(2), 7);
        tally.kill(130, None, 2);
        tally.kill(131, Some(2), 5);
        assert_eq!(
            tally.counts(7, true),
            BTreeMap::from([("opening-act".into(), 1), ("twofer".into(), 1)])
        );
        assert_eq!(
            tally.counts(2, true),
            BTreeMap::from([("payback".into(), 1), ("last-word".into(), 1)])
        );
    }
}
