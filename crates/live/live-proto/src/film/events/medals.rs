//! The medals of a game, projected from its ordered game events: the
//! film's Tracker ([`super::Tracker`]) and Live's read model and stats
//! (`crates/live/live-server`, both homes) run the same [`Projection`].
//!
//! Two kinds of medal, and nothing else:
//!
//! - **Halo 2's own** ([`OFFICIAL`]): the game awards them and the console
//!   that records the game reports each award as a `GameMedal` game event
//!   (`live-telemetry`'s medal observer, docs/live/live-telemetry.md). The
//!   projection passes them through; it never works them out.
//! - **Our modes' own**: the superbounce family ([`BOUNCE_MEDALS`]) from
//!   `Superbounce` and `Bounced`, which only the superbounce mode sends,
//!   and [`INCEL`] from the teabag watch's `Teabag`s at the game's end
//!   (docs/game/teabag.md, "The medal").
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::game;

/// A Halo 2 medal as the game counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Official {
    /// Its place in the game's medal statistics: the index the game's
    /// award function takes (EAX at `0x001970a0`; the medal names at
    /// `0x0046e120`, 16 bytes each, docs/live/live-gamestate.md, "Medals").
    pub index: u32,
    /// Our id for it, on the wire and in the stats.
    pub id: &'static str,
    /// The name the game gives it (the shared map's `medal_*` strings).
    pub name: &'static str,
}

/// Every medal Halo 2 awards, in the game's order. Indices 24 to 31 of
/// the game's table are `unused1` to `unused8` and award nothing. The
/// names are the shared map's `medal_*` strings but three: Rampage,
/// Berserker and Overkill have only their HUD lines (`flavor_15_kills`,
/// `_20_`, `_25_`), Flag Returned has only its post-game glyph
/// (`flag_return`), and the disc's `medal_collision_kill` reads
/// "Koadkill" and `medal_triple_kill` "Triple kill", written here as the
/// game's other names are.
pub static OFFICIAL: [Official; 24] = [
    Official { index: 0, id: "double-kill", name: "Double Kill" },
    Official { index: 1, id: "triple-kill", name: "Triple Kill" },
    Official { index: 2, id: "killtacular", name: "Killtacular" },
    Official { index: 3, id: "kill-frenzy", name: "Kill Frenzy" },
    Official { index: 4, id: "killtrocity", name: "Killtrocity" },
    Official { index: 5, id: "killimanjaro", name: "Killimanjaro" },
    Official { index: 6, id: "sniper-kill", name: "Sniper Kill" },
    Official { index: 7, id: "roadkill", name: "Roadkill" },
    Official { index: 8, id: "bonecracker", name: "Bonecracker" },
    Official { index: 9, id: "stealth-kill", name: "Stealth Kill" },
    Official { index: 10, id: "vehicle-kill", name: "Vehicle Kill" },
    Official { index: 11, id: "carjack", name: "Carjack" },
    Official { index: 12, id: "stick-it", name: "Stick It" },
    Official { index: 13, id: "killing-spree", name: "Killing Spree" },
    Official { index: 14, id: "running-riot", name: "Running Riot" },
    Official { index: 15, id: "rampage", name: "Rampage" },
    Official { index: 16, id: "berserker", name: "Berserker" },
    Official { index: 17, id: "overkill", name: "Overkill" },
    Official { index: 18, id: "flag-taken", name: "Flag Taken" },
    Official { index: 19, id: "flag-save", name: "Flag Save" },
    Official { index: 20, id: "flag-returned", name: "Flag Returned" },
    Official { index: 21, id: "bomb-planted", name: "Bomb Planted" },
    Official { index: 22, id: "bomb-save", name: "Bomb Save" },
    Official { index: 23, id: "bomb-diffused", name: "Bomb Diffused" },
];

/// The Halo 2 medal with id `id`.
pub fn official(id: &str) -> Option<&'static Official> {
    OFFICIAL.iter().find(|m| m.id == id)
}

/// The Halo 2 medal at the game's medal index `index`; `None` past the
/// last one (the game's unused rows).
pub fn official_at(index: u32) -> Option<&'static Official> {
    OFFICIAL.get(usize::try_from(index).ok()?)
}

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
/// A bounce that launches well above a normal one (a `Superbounce` event
/// with `monster`), beside its chain's medals.
pub const MONSTER_BOUNCE: &str = "monster-bounce";
/// The bounce medal ids: the chain's, lowest first, then `bounced`, then
/// `monster-bounce`.
pub const BOUNCE_MEDALS: [&str; 8] = [
    SUPERBOUNCE,
    DOUBLE_BOUNCE,
    TRIPLE_BOUNCE,
    QUAD_BOUNCE,
    CHAIN_BOUNCE,
    BOUNCETACULAR,
    BOUNCED,
    MONSTER_BOUNCE,
];

/// Completed a game without teabagging or getting teabagged
/// (docs/game/teabag.md, "The medal"): [`Incel`] says who.
pub const INCEL: &str = "incel";

/// Who earns [`INCEL`] at a game's end: every participant who completed
/// the game and was in no `Teabag`, as the crouching player or as the
/// one whose body it was. Completed is all three of:
///
/// - in the game before its first `PlayerKilled` (of any kind): on
///   `GameStarted`'s roster, or joined before anyone died ([`Incel::joined`]
///   after [`Incel::killed`] is late);
/// - still in it at `GameEnded`: a participant who left is gone, and one
///   who takes the seat after is a new participant (the caller's
///   generations);
/// - the game ended with reason `finished` (not `quit` or `lost`).
///
/// Bots count as players. Participants are the caller's ids
/// ([`Projection`]'s generations).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Incel {
    killed: bool,
    late: BTreeSet<u32>,
    teabagged: BTreeSet<u32>,
}

impl Incel {
    /// Participant `who` joined: late once anyone has died.
    pub fn joined(&mut self, who: u32) {
        if self.killed {
            self.late.insert(who);
        } else {
            self.late.remove(&who);
        }
    }

    /// A `PlayerKilled`: from now on a join is late.
    pub fn killed(&mut self) {
        self.killed = true;
    }

    /// A teabag: neither side earns the medal.
    pub fn teabag(&mut self, giver: u32, victim: u32) {
        self.teabagged.insert(giver);
        self.teabagged.insert(victim);
    }

    /// Whether participant `who`, in the game at an end with `reason`,
    /// earns the medal.
    pub fn earns(&self, who: u32, reason: &str) -> bool {
        reason == "finished" && !self.late.contains(&who) && !self.teabagged.contains(&who)
    }
}

/// The medal a bounce earns at its place in a chain, as a list of one
/// (docs/game/superbounce.md, "Medals"): Superbounce for the first, then
/// the tier of its count, like Halo 2's multi-kills, and Bouncetacular
/// again from the sixth on as Killimanjaro repeats from the seventh kill.
/// A count of 0 is no bounce.
pub fn bounce_medals(chain: u32) -> &'static [&'static str] {
    match chain {
        0 => &[],
        1 => &[SUPERBOUNCE],
        2 => &[DOUBLE_BOUNCE],
        3 => &[TRIPLE_BOUNCE],
        4 => &[QUAD_BOUNCE],
        5 => &[CHAIN_BOUNCE],
        _ => &[BOUNCETACULAR],
    }
}

/// The medals of a bounce at `chain` that is a Monster when `monster`:
/// [`bounce_medals`], then [`MONSTER_BOUNCE`] last for a Monster. A count
/// of 0 is no bounce, Monster or not.
pub fn bounce_medals_with(chain: u32, monster: bool) -> impl Iterator<Item = &'static str> {
    bounce_medals(chain)
        .iter()
        .copied()
        .chain((monster && chain > 0).then_some(MONSTER_BOUNCE))
}

/// One medal awarded in a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Award {
    /// The earner's slot when it was awarded.
    pub player: u32,
    /// The earner as a participant of the game: a seat taken again after
    /// a leave is a new participant ([`Projection::participant`]).
    pub participant: u32,
    /// The match tick it was awarded at.
    pub tick: u64,
    /// The medal's id: an [`OFFICIAL`] one's, a bounce medal or [`INCEL`].
    pub medal: &'static str,
}

/// The game event kinds a [`Projection`] reads; it passes over the rest.
pub const KINDS: [&str; 9] = [
    game::GAME_STARTED,
    game::PLAYER_JOINED,
    game::PLAYER_LEFT,
    game::PLAYER_KILLED,
    game::GAME_MEDAL,
    game::SUPERBOUNCE,
    game::BOUNCED,
    game::TEABAG,
    game::GAME_ENDED,
];

/// The medals of one game, from its game events in order. A `GameStarted`
/// starts it over. It keeps who is in the game and what [`Incel`] needs,
/// never a list of kills.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Projection {
    /// Slot -> participant, while the seat's player is in the game.
    generations: BTreeMap<u32, u32>,
    next_generation: u32,
    /// The participants who joined (on the roster or by `PlayerJoined`),
    /// not a slot an event named that no one joined.
    joined: BTreeSet<u32>,
    incel: Incel,
    ended: bool,
}

impl Projection {
    /// The participant in slot `slot`, a new one if the seat is empty.
    pub fn generation(&mut self, slot: u32) -> u32 {
        if let Some(generation) = self.generations.get(&slot) {
            return *generation;
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        self.generations.insert(slot, generation);
        generation
    }

    /// The participant in slot `slot`, if the seat is taken.
    pub fn participant(&self, slot: u32) -> Option<u32> {
        self.generations.get(&slot).copied()
    }

    fn join(&mut self, slot: u32) {
        let generation = self.generation(slot);
        self.joined.insert(generation);
        self.incel.joined(generation);
    }

    fn award(&mut self, player: u32, tick: u64, medal: &'static str) -> Award {
        Award {
            player,
            participant: self.generation(player),
            tick,
            medal,
        }
    }

    /// The medals game event `kind` with `data` awards; none for a kind
    /// not in [`KINDS`] or data that does not parse (Live checked each
    /// event on the way in, so that is a log line's worth, not an error).
    pub fn game_event(&mut self, kind: &str, data: &Value) -> Vec<Award> {
        if !KINDS.contains(&kind) {
            return Vec::new();
        }
        match super::Event::from_game(kind, data) {
            Ok(event) => self.apply(&event),
            Err(_) => Vec::new(),
        }
    }

    /// The medals `event` awards, in the order they are given.
    pub fn apply(&mut self, event: &super::Event) -> Vec<Award> {
        use super::Event;
        match event {
            Event::GameStarted(start) => {
                *self = Self::default();
                for player in &start.game.players {
                    self.join(player.index);
                }
                Vec::new()
            }
            Event::PlayerJoined(joined) => {
                self.join(joined.player.index);
                Vec::new()
            }
            Event::PlayerLeft(left) => {
                self.generations.remove(&left.index);
                Vec::new()
            }
            Event::PlayerKilled(_) => {
                self.incel.killed();
                Vec::new()
            }
            // The game's own award, passed through as the game named it.
            Event::GameMedal(m) => official(&m.medal)
                .map(|o| vec![self.award(m.player, m.tick, o.id)])
                .unwrap_or_default(),
            Event::Superbounce(b) => bounce_medals_with(b.chain, b.monster)
                .collect::<Vec<_>>()
                .into_iter()
                .map(|medal| self.award(b.player, b.tick, medal))
                .collect(),
            Event::Bounced(d) => vec![self.award(d.player, d.tick, BOUNCED)],
            Event::Teabag(bag) => {
                let giver = self.generation(bag.player);
                let victim = self.generation(bag.victim);
                self.incel.teabag(giver, victim);
                Vec::new()
            }
            // Everyone still in the game who completed it without a teabag,
            // in slot order, once (docs/game/teabag.md, "The medal").
            Event::GameEnded(end) => {
                if std::mem::replace(&mut self.ended, true) {
                    return Vec::new();
                }
                self.generations
                    .iter()
                    .filter(|&(_, &g)| self.joined.contains(&g) && self.incel.earns(g, &end.reason))
                    .map(|(&player, &participant)| Award {
                        player,
                        participant,
                        tick: end.tick,
                        medal: INCEL,
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

/// Whether `id` is a medal Live keeps: one of [`OFFICIAL`], a bounce
/// medal or [`INCEL`].
pub fn is_medal(id: &str) -> bool {
    official(id).is_some() || BOUNCE_MEDALS.contains(&id) || id == INCEL
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bounce_medals_follow_the_chain_table() {
        assert_eq!(bounce_medals(0), &[] as &[&str]);
        assert_eq!(bounce_medals(1), [SUPERBOUNCE]);
        assert_eq!(bounce_medals(2), [DOUBLE_BOUNCE]);
        assert_eq!(bounce_medals(3), [TRIPLE_BOUNCE]);
        assert_eq!(bounce_medals(4), [QUAD_BOUNCE]);
        assert_eq!(bounce_medals(5), [CHAIN_BOUNCE]);
        assert_eq!(bounce_medals(6), [BOUNCETACULAR]);
        for chain in [7, 8, 12, u32::MAX] {
            assert_eq!(bounce_medals(chain), [BOUNCETACULAR], "{chain}");
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
                "monster-bounce",
            ]
        );
    }

    /// The table is the game's: 24 medals at indices 0 to 23, every id
    /// once, and none of the names Live once made up.
    #[test]
    fn the_official_table_is_the_games_medal_order() {
        for (i, m) in OFFICIAL.iter().enumerate() {
            assert_eq!(m.index as usize, i, "{}", m.id);
            assert_eq!(official(m.id), Some(m));
            assert_eq!(official_at(m.index), Some(m));
            assert!(is_medal(m.id));
        }
        assert_eq!(official_at(24), None, "unused1");
        assert_eq!(official_at(31), None, "unused8");
        let ids: BTreeSet<_> = OFFICIAL.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), OFFICIAL.len());
        assert_eq!(official_at(0).unwrap().name, "Double Kill");
        assert_eq!(official_at(5).unwrap().name, "Killimanjaro");
        assert_eq!(official_at(13).unwrap().name, "Killing Spree");
        assert_eq!(official_at(17).unwrap().name, "Overkill");
        assert_eq!(official_at(23).unwrap().name, "Bomb Diffused");
        for gone in [
            "twofer", "hat-trick", "full-house", "encore", "hot-streak", "wildfire",
            "untouchable", "opening-act", "last-word", "payback",
        ] {
            assert!(!is_medal(gone), "{gone}");
        }
        assert!(BOUNCE_MEDALS.iter().all(|m| is_medal(m) && official(m).is_none()));
        assert!(is_medal(INCEL) && official(INCEL).is_none());
    }

    /// A Monster gives its chain's medals, then `monster-bounce` last.
    #[test]
    fn a_monster_bounce_adds_its_medal_after_the_chain_medals() {
        let with = |chain, monster| bounce_medals_with(chain, monster).collect::<Vec<_>>();
        assert_eq!(with(2, true), [DOUBLE_BOUNCE, MONSTER_BOUNCE]);
        assert_eq!(with(1, true), [SUPERBOUNCE, MONSTER_BOUNCE]);
        assert_eq!(with(9, true), [BOUNCETACULAR, MONSTER_BOUNCE]);
        assert_eq!(with(0, true), [] as [&str; 0]);
        for chain in 0..=8 {
            assert_eq!(with(chain, false), bounce_medals(chain), "{chain}");
        }
    }

    /// Incel: a clean participant earns it; both sides of a teabag, a
    /// late joiner and every player of a game that did not finish do not.
    #[test]
    fn incel_is_for_who_completed_the_game_without_a_teabag() {
        let mut incel = Incel::default();
        for who in [0, 1, 2, 3] {
            incel.joined(who);
        }
        incel.teabag(1, 2);
        incel.killed();
        incel.joined(4);
        assert!(incel.earns(0, "finished"));
        assert!(incel.earns(3, "finished"));
        assert!(!incel.earns(1, "finished"), "the giver");
        assert!(!incel.earns(2, "finished"), "the receiver");
        assert!(!incel.earns(4, "finished"), "joined after a death");
        assert!(!incel.earns(0, "quit"));
        assert!(!incel.earns(0, "lost"));
    }

    fn start(variant: &str) -> (&'static str, Value) {
        (
            game::GAME_STARTED,
            json!({"game_id":"g","host":"h","tick":0,"map":"lockout","mode":"multiplayer",
                "variant":variant,"team_play":false,"players":[
                    {"index":0,"name":"Ace","team":0},{"index":1,"name":"Bee","team":1}]}),
        )
    }

    fn project(events: &[(&str, Value)]) -> Vec<(u32, u32, u64, &'static str)> {
        let mut p = Projection::default();
        events
            .iter()
            .flat_map(|(kind, data)| p.game_event(kind, data))
            .map(|a| (a.player, a.participant, a.tick, a.medal))
            .collect()
    }

    /// An official game: the game's own awards pass through as the game
    /// named them, at their ticks; kills alone award nothing (no made-up
    /// multi-kill, spree, first or last kill or payback), nor does a
    /// medal id the game does not have. Everyone who finished without a
    /// teabag gets Incel.
    #[test]
    fn official_awards_pass_through_and_kills_alone_award_nothing() {
        let kill = |tick: u64, killer: u32, victim: u32| {
            (
                game::PLAYER_KILLED,
                json!({"tick":tick,"kind":"kill","killer":killer,"victim":victim}),
            )
        };
        let medal = |tick: u64, player: u32, medal: &str| {
            (
                game::GAME_MEDAL,
                json!({"tick":tick,"player":player,"medal":medal}),
            )
        };
        let awards = project(&[
            start("Slayer"),
            kill(10, 0, 1),
            kill(20, 0, 1),
            medal(20, 0, "double-kill"),
            kill(25, 0, 1),
            medal(25, 0, "triple-kill"),
            kill(30, 1, 0),
            kill(40, 1, 0),
            medal(40, 1, "twofer"),
            medal(41, 1, "sniper-kill"),
            (
                game::GAME_ENDED,
                json!({"tick":900,"reason":"finished","players":[]}),
            ),
        ]);
        assert_eq!(
            awards,
            [
                (0, 0, 20, "double-kill"),
                (0, 0, 25, "triple-kill"),
                (1, 1, 41, "sniper-kill"),
                (0, 0, 900, INCEL),
                (1, 1, 900, INCEL),
            ]
        );
    }

    /// Mode medals come only where their mode runs: a Slayer game's
    /// events give no bounce medal, a Superbounce game's bounces give
    /// theirs, and a teabag takes Incel from both sides.
    #[test]
    fn bounce_medals_only_come_from_the_superbounce_modes_events() {
        let slayer = project(&[
            start("Slayer"),
            (
                game::PLAYER_KILLED,
                json!({"tick":5,"kind":"kill","killer":0,"victim":1}),
            ),
            (
                game::GAME_ENDED,
                json!({"tick":90,"reason":"finished","players":[]}),
            ),
        ]);
        assert!(
            slayer.iter().all(|a| !BOUNCE_MEDALS.contains(&a.3)),
            "{slayer:?}"
        );
        let bounces = project(&[
            start("Superbounce"),
            (
                game::SUPERBOUNCE,
                json!({"tick":40,"player":0,"chain":1}),
            ),
            (
                game::SUPERBOUNCE,
                json!({"tick":52,"player":0,"chain":2,"monster":true}),
            ),
            (game::BOUNCED, json!({"tick":55,"player":1})),
            (game::TEABAG, json!({"tick":60,"player":0,"victim":1})),
            (
                game::GAME_ENDED,
                json!({"tick":90,"reason":"finished","players":[]}),
            ),
        ]);
        assert_eq!(
            bounces,
            [
                (0, 0, 40, SUPERBOUNCE),
                (0, 0, 52, DOUBLE_BOUNCE),
                (0, 0, 52, MONSTER_BOUNCE),
                (1, 1, 55, BOUNCED),
            ]
        );
    }

    /// A seat taken again is a new participant; the one who left earns no
    /// Incel, the late joiner neither, and a second end awards nothing.
    #[test]
    fn a_seat_taken_again_is_a_new_participant() {
        let awards = project(&[
            start("Slayer"),
            (
                game::GAME_MEDAL,
                json!({"tick":8,"player":1,"medal":"killing-spree"}),
            ),
            (
                game::PLAYER_KILLED,
                json!({"tick":9,"kind":"kill","killer":0,"victim":1}),
            ),
            (game::PLAYER_LEFT, json!({"index":1})),
            (
                game::PLAYER_JOINED,
                json!({"player":{"index":1,"name":"Cee","team":1}}),
            ),
            (
                game::GAME_MEDAL,
                json!({"tick":12,"player":1,"medal":"stick-it"}),
            ),
            (
                game::GAME_ENDED,
                json!({"tick":90,"reason":"finished","players":[]}),
            ),
            (
                game::GAME_ENDED,
                json!({"tick":91,"reason":"finished","players":[]}),
            ),
        ]);
        assert_eq!(
            awards,
            [
                (1, 1, 8, "killing-spree"),
                (1, 2, 12, "stick-it"),
                (0, 0, 90, INCEL),
            ]
        );
    }
}
