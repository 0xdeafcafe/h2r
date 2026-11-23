//! The superbounce practice's tally and its part of the report
//! (docs/runtime/sim.md, "Bounce practice").
//!
//! Pure, as the rest of the tally: the game's glue feeds plain events. Two
//! sources, per game tick:
//!
//! - **the bots' practice** (`bots::mind::PracticeEvent`, drained by the
//!   sim's `Metered` brain wrapper): a route to a site started, its word
//!   reached -3, the try ended (bounced as the route saw it, or why not);
//! - **the game's medal chain** (`superbounce_chain::Award`, the same
//!   awards the HUD and Live get): a bounce, with its count in the chain,
//!   and a Bounced death;
//!
//! and each bot's height every tick ([`BounceTally::height`]), for the
//! launch: from the touchdown before a chain bounce to the top of its
//! flight.
//!
//! Counted per bot and per site (the site the bot's try was at). A
//! bounced **try** is one in which the chain filed at least one bounce;
//! the **bounce rate** is bounced tries over tries. The practice's own
//! word (`practice_bounces`: the route rose 0.4 over its lowest point) is
//! kept beside it, as are the chain's links (`chain_bounces`). How the bots
//! practised ([`Habits`]: variety, fumbles, chains planned and flown,
//! Monsters, letting players go first) comes from the practice's own
//! events.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

/// The bounce block's schema tag.
pub const FORMAT: &str = "h2-sim-bounce/1";
/// Two stand points this close (XY, world units) and [`SAME_Z`] in height
/// are one site (`bots::mind::practice`'s tally does the same).
pub const SAME_XY: f32 = 0.05;
pub const SAME_Z: f32 = 0.5;
/// A launch is over this far under its top (world units) ...
pub const LAUNCH_FALL: f32 = 0.1;
/// ... or this long (ticks) after the bounce.
pub const LAUNCH_TICKS: u32 = 150;
/// The heights kept before a bounce, to find its touchdown: the award
/// comes when the landing's launch window closes,
/// [`crate::drops::LAUNCH_TICKS`] after the touchdown, so the touchdown
/// and the tick before it are kept with the window.
pub const RECENT: usize = crate::drops::LAUNCH_TICKS as usize + 2;

/// One event, as the glue reads it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The bot's practice started a route to the site at `site`.
    Started { site: [f32; 3] },
    /// The route's word reached -3: `read` from the game, `own` at a
    /// crouch spot (else the route started primed).
    Primed {
        site: [f32; 3],
        read: bool,
        own: bool,
    },
    /// The try is over: `bounced` as the route saw it, else `why`.
    Tried {
        site: [f32; 3],
        bounced: bool,
        why: Option<String>,
    },
    /// The medal chain filed a bounce, `chain` into its chain.
    Chain { chain: u32 },
    /// The medal chain filed a Bounced: a fall out of the map during a
    /// bounce, ending a chain of `chain` (0 if none).
    Bounced { chain: u32 },
    /// What the try just started means to do: a Monster site, the chain's
    /// links planned, the error it fumbles with, whether it goes again at
    /// a site it just missed.
    Intent {
        site: [f32; 3],
        monster: bool,
        links: u32,
        fumble: Option<String>,
        again: bool,
    },
    /// The try's chain after its first bounce: links tried and made.
    Chained { site: [f32; 3], tried: u32, made: u32 },
    /// The bot let a player go first at the site for `waited` ticks, then
    /// went on or gave it up.
    Yielded {
        site: [f32; 3],
        waited: u32,
        gave_up: bool,
    },
    /// Standing between tries, the bot stepped out of a player's way.
    SteppedAside,
    /// The medal chain's last bounce was a Monster Bounce.
    Monster,
}

/// How the bots practised (docs/runtime/sim.md, "Bounce practice"): the
/// practice's own account of its variety, fumbles, chains, Monsters and
/// yielding.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Habits {
    /// Tries at the same site as the bot's try before.
    pub repeats: u64,
    /// The sites a bot tried, each once (a bot's line; summed over the
    /// bots in the totals and in pools).
    pub distinct_sites: u64,
    /// Tries set up with a fumble, and of those, tries the chain still
    /// filed a bounce in.
    pub fumbles: u64,
    pub fumble_bounces: u64,
    /// Tries going again at a site whose landing just missed.
    pub retries: u64,
    /// Tries at a Monster site, and Monster Bounces the chain filed.
    pub monster_tries: u64,
    pub monster_bounces: u64,
    /// Chain links planned after the first bounce, tried (in the air off a
    /// launch) and made (launched again).
    pub chain_links_planned: u64,
    pub chain_links_tried: u64,
    pub chain_links_made: u64,
    /// Times a bot let a player go first, of those given up, and the ticks
    /// waited.
    pub yields: u64,
    pub yields_given_up: u64,
    pub yield_ticks: u64,
    /// Steps out of a player's way between tries.
    pub stepped_aside: u64,
}

impl Habits {
    fn add(&mut self, o: &Habits) {
        self.repeats += o.repeats;
        self.distinct_sites += o.distinct_sites;
        self.fumbles += o.fumbles;
        self.fumble_bounces += o.fumble_bounces;
        self.retries += o.retries;
        self.monster_tries += o.monster_tries;
        self.monster_bounces += o.monster_bounces;
        self.chain_links_planned += o.chain_links_planned;
        self.chain_links_tried += o.chain_links_tried;
        self.chain_links_made += o.chain_links_made;
        self.yields += o.yields;
        self.yields_given_up += o.yields_given_up;
        self.yield_ticks += o.yield_ticks;
        self.stepped_aside += o.stepped_aside;
    }
}

/// Launch heights: how many, the best and the mean, world units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Launch {
    pub n: u64,
    pub best: f64,
    pub mean: f64,
}

/// The counts of a bot, a site, or the whole match.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Tries the practice tallied.
    pub attempts: u64,
    /// Primes at a crouch spot: the word reached -3.
    pub primes: u64,
    /// Of those, read from the game (else assumed after the stand-up).
    pub primes_read: u64,
    /// Tries that began primed with the word read at -3 rather than
    /// priming at a crouch spot: a drill's word, set by the host.
    #[serde(default)]
    pub armed: u64,
    /// Tries the route saw launch.
    pub practice_bounces: u64,
    /// Tries with at least one chain bounce.
    pub bounced_tries: u64,
    /// `bounced_tries / attempts`; 0 with no tries.
    pub bounce_rate: f64,
    /// Every bounce the medal chain filed.
    pub chain_bounces: u64,
    /// Chains by length (`"1"`, `"2"`, ...): each chain once, at its end.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub chains: BTreeMap<String, u64>,
    /// The longest chain.
    pub best_chain: u32,
    /// Bounced deaths.
    pub bounced_deaths: u64,
    /// Launch heights, touchdown to top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<Launch>,
    /// Why the other tries did not bounce, by the practice's names
    /// (`prime_failed`, `pair_lost`, `not_primed`, `missed`, `too_slow`,
    /// `no_bounce`, `no_drop`, `no_route`, `timeout`, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub why: BTreeMap<String, u64>,
    /// How the bots practised.
    #[serde(default)]
    pub habits: Habits,
}

/// One bot's line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerBounce {
    pub index: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<u8>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    /// Ticks from the match's start to its first chain bounce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_bounce_ticks: Option<u64>,
    pub stats: Stats,
}

/// One site's line, over every bot.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteBounce {
    /// The site's stand point.
    pub site: [f32; 3],
    pub stats: Stats,
}

/// The report's `bounce` block.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BounceReport {
    pub format: String,
    pub totals: Stats,
    pub players: Vec<PlayerBounce>,
    pub sites: Vec<SiteBounce>,
}

/// Who a player is, for the report's lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Who {
    pub index: u8,
    pub seat: Option<u8>,
    pub name: String,
    pub arm: Option<String>,
}

/// Sums behind [`Stats`]: the launch's sum, the rate worked out at the end.
#[derive(Clone, Debug, Default, PartialEq)]
struct Acc {
    stats: Stats,
    launch_sum: f64,
}

impl Acc {
    fn launch(&mut self, h: f64) {
        let l = self.stats.launch.get_or_insert_with(Launch::default);
        l.n += 1;
        l.best = l.best.max(h);
        self.launch_sum += h;
        l.mean = self.launch_sum / l.n as f64;
    }

    fn chain_ended(&mut self, len: u32) {
        if len == 0 {
            return;
        }
        *self.stats.chains.entry(len.to_string()).or_default() += 1;
        self.stats.best_chain = self.stats.best_chain.max(len);
    }

    fn done(&self) -> Stats {
        let mut s = self.stats.clone();
        s.bounce_rate = if s.attempts > 0 {
            s.bounced_tries as f64 / s.attempts as f64
        } else {
            0.0
        };
        s
    }
}

/// A launch being watched: the touchdown height, the top so far, since.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Flight {
    from: f32,
    top: f32,
    since: u32,
    site: Option<usize>,
}

/// One player's running state.
#[derive(Clone, Debug, Default, PartialEq)]
struct Player {
    acc: Acc,
    /// The try under way: its site, and whether the chain bounced in it.
    try_site: Option<usize>,
    try_chained: bool,
    /// The chain under way's count.
    chain: u32,
    recent: VecDeque<f32>,
    flight: Option<Flight>,
    first_bounce: Option<u32>,
    /// The try under way is fumbled; the site of the try before; every
    /// site tried.
    try_fumble: bool,
    last_site: Option<usize>,
    tried: BTreeSet<usize>,
}

/// The practice's tally over a match.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BounceTally {
    players: BTreeMap<u8, Player>,
    sites: Vec<([f32; 3], Acc)>,
    any: bool,
}

fn same(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < SAME_XY && (a[2] - b[2]).abs() < SAME_Z
}

impl BounceTally {
    pub fn new() -> BounceTally {
        BounceTally::default()
    }

    /// Whether anything has been fed.
    pub fn is_empty(&self) -> bool {
        !self.any
    }

    fn site(&mut self, at: [f32; 3]) -> usize {
        match self.sites.iter().position(|(s, _)| same(*s, at)) {
            Some(i) => i,
            None => {
                self.sites.push((at, Acc::default()));
                self.sites.len() - 1
            }
        }
    }

    /// Applies `f` to the player's and, when there is one, the site's sums.
    fn both(&mut self, player: u8, site: Option<usize>, f: impl Fn(&mut Acc)) {
        f(&mut self.players.entry(player).or_default().acc);
        if let Some(i) = site
            && let Some((_, a)) = self.sites.get_mut(i)
        {
            f(a);
        }
    }

    /// An event of player `player` at game tick `tick`.
    pub fn event(&mut self, player: u8, tick: u32, e: Event) {
        self.any = true;
        match e {
            Event::Started { site } => {
                let i = self.site(site);
                let p = self.players.entry(player).or_default();
                p.try_site = Some(i);
                p.try_chained = false;
            }
            Event::Primed { site, read, own } => {
                if !own {
                    // A word carried in: counted when it was read.
                    if read {
                        let i = self.site(site);
                        self.both(player, Some(i), |a| a.stats.armed += 1);
                    }
                    return;
                }
                let i = self.site(site);
                self.both(player, Some(i), |a| {
                    a.stats.primes += 1;
                    if read {
                        a.stats.primes_read += 1;
                    }
                });
            }
            Event::Tried { site, bounced, why } => {
                let i = self.site(site);
                let (chained, repeat, fumbled) = {
                    let p = self.players.entry(player).or_default();
                    let c = p.try_chained && p.try_site == Some(i);
                    p.try_site = None;
                    p.try_chained = false;
                    let repeat = p.last_site == Some(i);
                    p.last_site = Some(i);
                    p.tried.insert(i);
                    (c, repeat, std::mem::take(&mut p.try_fumble))
                };
                self.both(player, Some(i), |a| {
                    let h = &mut a.stats.habits;
                    if repeat {
                        h.repeats += 1;
                    }
                    if fumbled && chained {
                        h.fumble_bounces += 1;
                    }
                    a.stats.attempts += 1;
                    if bounced {
                        a.stats.practice_bounces += 1;
                    }
                    if chained {
                        a.stats.bounced_tries += 1;
                    }
                    if !bounced && let Some(w) = &why {
                        *a.stats.why.entry(w.clone()).or_default() += 1;
                    }
                });
            }
            Event::Chain { chain } => {
                let (site, ended) = {
                    let p = self.players.entry(player).or_default();
                    // A count of 1 starts a chain: the last one is over.
                    let ended = if chain <= 1 {
                        std::mem::take(&mut p.chain)
                    } else {
                        0
                    };
                    p.chain = chain.max(p.chain);
                    p.try_chained = true;
                    p.first_bounce.get_or_insert(tick);
                    let from = p.recent.iter().copied().fold(f32::INFINITY, f32::min);
                    if from.is_finite() {
                        p.flight = Some(Flight {
                            from,
                            top: from,
                            since: tick,
                            site: p.try_site,
                        });
                    }
                    (p.try_site, ended)
                };
                self.both(player, site, |a| {
                    a.stats.chain_bounces += 1;
                    a.chain_ended(ended);
                });
            }
            Event::Bounced { chain } => {
                let site = {
                    let p = self.players.entry(player).or_default();
                    p.chain = 0;
                    p.flight = None;
                    p.try_site
                };
                self.both(player, site, |a| {
                    a.stats.bounced_deaths += 1;
                    a.chain_ended(chain);
                });
            }
            Event::Intent {
                site,
                monster,
                links,
                fumble,
                again,
            } => {
                let i = self.site(site);
                self.players.entry(player).or_default().try_fumble = fumble.is_some();
                self.both(player, Some(i), |a| {
                    let h = &mut a.stats.habits;
                    h.fumbles += u64::from(fumble.is_some());
                    h.retries += u64::from(again);
                    h.monster_tries += u64::from(monster);
                    h.chain_links_planned += u64::from(links);
                });
            }
            Event::Chained { site, tried, made } => {
                let i = self.site(site);
                self.both(player, Some(i), |a| {
                    a.stats.habits.chain_links_tried += u64::from(tried);
                    a.stats.habits.chain_links_made += u64::from(made);
                });
            }
            Event::Yielded {
                site,
                waited,
                gave_up,
            } => {
                let i = self.site(site);
                self.both(player, Some(i), |a| {
                    let h = &mut a.stats.habits;
                    h.yields += 1;
                    h.yields_given_up += u64::from(gave_up);
                    h.yield_ticks += u64::from(waited);
                });
            }
            Event::SteppedAside => {
                self.both(player, None, |a| a.stats.habits.stepped_aside += 1);
            }
            Event::Monster => {
                let site = {
                    let p = self.players.entry(player).or_default();
                    p.try_site.or(p.flight.and_then(|f| f.site))
                };
                self.both(player, site, |a| a.stats.habits.monster_bounces += 1);
            }
        }
    }

    /// Player `player`'s height (its origin's z) after game tick `tick`.
    pub fn height(&mut self, player: u8, tick: u32, z: f32) {
        if !z.is_finite() {
            return;
        }
        let done = {
            let p = self.players.entry(player).or_default();
            if p.recent.len() >= RECENT {
                p.recent.pop_front();
            }
            p.recent.push_back(z);
            match &mut p.flight {
                Some(f) => {
                    f.top = f.top.max(z);
                    let over = z < f.top - LAUNCH_FALL || tick.wrapping_sub(f.since) > LAUNCH_TICKS;
                    if over {
                        let f = *f;
                        p.flight = None;
                        Some(f)
                    } else {
                        None
                    }
                }
                None => None,
            }
        };
        if let Some(f) = done {
            let h = f64::from(f.top - f.from);
            self.both(player, f.site, |a| a.launch(h));
        }
    }

    /// The block for the report, the match having started at `started`;
    /// `who` names the bots (players not in it are left out of the lines
    /// but counted in the totals).
    pub fn report(&self, started: Option<u32>, who: &[Who]) -> BounceReport {
        // Chains and flights still open count as they stand.
        let mut totals = Acc::default();
        let mut players = Vec::new();
        for (index, p) in &self.players {
            let mut acc = p.acc.clone();
            acc.stats.habits.distinct_sites = p.tried.len() as u64;
            acc.chain_ended(p.chain);
            if let Some(f) = p.flight {
                acc.launch(f64::from(f.top - f.from));
            }
            add(&mut totals, &acc);
            if let Some(w) = who.iter().find(|w| w.index == *index) {
                players.push(PlayerBounce {
                    index: *index,
                    seat: w.seat,
                    name: w.name.clone(),
                    arm: w.arm.clone(),
                    first_bounce_ticks: p
                        .first_bounce
                        .map(|t| u64::from(t.wrapping_sub(started.unwrap_or(t)))),
                    stats: acc.done(),
                });
            }
        }
        let mut sites: Vec<(usize, Acc)> = self
            .sites
            .iter()
            .enumerate()
            .map(|(i, (_, a))| (i, a.clone()))
            .collect();
        for p in self.players.values() {
            if let Some(i) = p.try_site.or(p.flight.and_then(|f| f.site))
                && let Some((_, a)) = sites.iter_mut().find(|(j, _)| *j == i)
            {
                a.chain_ended(p.chain);
                if let Some(f) = p.flight {
                    a.launch(f64::from(f.top - f.from));
                }
            }
        }
        BounceReport {
            format: FORMAT.into(),
            totals: totals.done(),
            players,
            sites: sites
                .into_iter()
                .map(|(i, a)| SiteBounce {
                    site: self.sites[i].0,
                    stats: a.done(),
                })
                .collect(),
        }
    }
}

/// Adds `b`'s sums into `a`.
fn add(a: &mut Acc, b: &Acc) {
    let (s, o) = (&mut a.stats, &b.stats);
    s.attempts += o.attempts;
    s.primes += o.primes;
    s.primes_read += o.primes_read;
    s.armed += o.armed;
    s.practice_bounces += o.practice_bounces;
    s.bounced_tries += o.bounced_tries;
    s.chain_bounces += o.chain_bounces;
    s.bounced_deaths += o.bounced_deaths;
    s.best_chain = s.best_chain.max(o.best_chain);
    for (k, v) in &o.chains {
        *s.chains.entry(k.clone()).or_default() += v;
    }
    for (k, v) in &o.why {
        *s.why.entry(k.clone()).or_default() += v;
    }
    s.habits.add(&o.habits);
    if let Some(l) = o.launch {
        let m = s.launch.get_or_insert_with(Launch::default);
        let n = m.n + l.n;
        let sum = a.launch_sum + l.mean * l.n as f64;
        m.best = m.best.max(l.best);
        m.n = n;
        a.launch_sum = sum;
        m.mean = if n > 0 { sum / n as f64 } else { 0.0 };
    }
}

/// Pools several lines' stats (bots of an arm, runs of a batch): counts
/// add, launches pool by count, the rate is worked out again.
pub fn pool<'a>(stats: impl IntoIterator<Item = &'a Stats>) -> Stats {
    let mut acc = Acc::default();
    for s in stats {
        add(
            &mut acc,
            &Acc {
                stats: s.clone(),
                launch_sum: s.launch.map_or(0.0, |l| l.mean * l.n as f64),
            },
        );
    }
    acc.done()
}

#[cfg(test)]
mod tests;
