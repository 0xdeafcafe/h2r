//! The medals Live awards from the ordered kills (web/src/state/derive.ts).
//! Keep the counters and last scoring kill; no lifetime list of kills is needed.
use std::collections::BTreeMap;

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
