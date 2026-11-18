//! Who may cheat, and which cheats a session allows.
//!
//! 1. Not on Xbox Live (offline, split screen, System Link): allowed,
//!    subject to 3.
//! 2. On Live (a local player signed in to our Live service, which every
//!    Live session needs): only when every signed-in Live gamertag is on
//!    [`LIVE_ALLOWED`] (case-insensitive, trimmed).
//! 3. Whoever you are, refused where a cheat would break the run:
//!    - a lockstep run (`H2_LOCKSTEP`, input record or replay) compares
//!      deterministic runs: every cheat;
//!    - a synchronous co-op session (simulation modes 2 and 3): every
//!      machine runs the same simulation, so only this one would change
//!      and the players would desync: every cheat that changes the
//!      simulation ([`Reach::Simulation`], [`Reach::Clock`]);
//!    - a networked (distributed) game's guest (mode 4): the host's
//!      simulation decides; ours would be overruled: the same;
//!    - a networked game's host (mode 5): the host's simulation is the
//!      game, so [`Reach::Simulation`] cheats work there (on the host's
//!      own players); [`Reach::Clock`] (game speed) does not, since the
//!      guests' clocks would not follow.
//!
//!    [`Reach::Local`] cheats (what this machine draws) are refused only
//!    by 1, 2 and lockstep.

/// The Live gamertags allowed to cheat on Live.
pub const LIVE_ALLOWED: &[&str] = &["afr"];

/// What a cheat changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The simulation: units, weapons, damage.
    Simulation,
    /// Game time itself.
    Clock,
    /// Only what this machine draws.
    Local,
}

/// Live as the cheats see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Live {
    /// Not signed in to Live (or no Live at all).
    #[default]
    Off,
    /// Signed in: each local Live user's gamertag, empty when unknown.
    SignedIn(Vec<String>),
}

/// Whether `tag` is on [`LIVE_ALLOWED`].
pub fn allowed_on_live(tag: &str) -> bool {
    let t = tag.trim();
    !t.is_empty() && LIVE_ALLOWED.iter().any(|a| a.eq_ignore_ascii_case(t))
}

/// What the session is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The simulation's mode (`coop_probe::read_sim`), when readable.
    pub sim_mode: Option<u32>,
    pub lockstep: bool,
    pub live: Live,
}

/// Why cheats are refused, by reach.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refusals {
    /// Every cheat.
    pub all: Option<String>,
    pub simulation: Option<String>,
    pub clock: Option<String>,
}

impl Refusals {
    /// Every cheat refused for `why`.
    pub fn every(why: impl Into<String>) -> Self {
        let why = why.into();
        Refusals {
            all: Some(why.clone()),
            simulation: Some(why.clone()),
            clock: Some(why),
        }
    }

    /// Why a cheat of `reach` is refused, if it is.
    pub fn of(&self, reach: Reach) -> Option<&str> {
        match reach {
            Reach::Local => self.all.as_deref(),
            Reach::Simulation => self.simulation.as_deref(),
            Reach::Clock => self.clock.as_deref(),
        }
    }

    /// None refused.
    pub fn none(&self) -> bool {
        self.all.is_none() && self.simulation.is_none() && self.clock.is_none()
    }
}

pub const LOCKSTEP: &str =
    "a lockstep run (H2_LOCKSTEP or input record/replay) compares deterministic runs";
pub const CO_OP: &str =
    "a co-op session: only this machine would cheat, and the players would desync";
pub const GUEST: &str = "a guest in a networked game: the host's simulation decides";
pub const HOST_CLOCK: &str =
    "a networked game: the guests' clocks would not follow the host's speed";

/// What `s` refuses.
pub fn refusals(s: &Session) -> Refusals {
    if s.lockstep {
        return Refusals::every(LOCKSTEP);
    }
    if let Live::SignedIn(tags) = &s.live
        && (tags.is_empty() || !tags.iter().all(|t| allowed_on_live(t)))
    {
        return Refusals::every(format!(
            "cheats are off on Xbox Live (only for {})",
            LIVE_ALLOWED.join(", ")
        ));
    }
    match s.sim_mode {
        Some(2 | 3) => Refusals {
            all: None,
            simulation: Some(CO_OP.into()),
            clock: Some(CO_OP.into()),
        },
        Some(4) => Refusals {
            all: None,
            simulation: Some(GUEST.into()),
            clock: Some(GUEST.into()),
        },
        Some(5) => Refusals {
            all: None,
            simulation: None,
            clock: Some(HOST_CLOCK.into()),
        },
        _ => Refusals::default(),
    }
}

/// Live's state from the Live client: the signed-in users' xuids (0 for
/// none) and the gamertag of a xuid, when this machine's accounts know it.
pub fn live_from(users: &[u64], gamertag: impl Fn(u64) -> Option<String>) -> Live {
    let signed: Vec<u64> = users.iter().copied().filter(|x| *x != 0).collect();
    if signed.is_empty() {
        return Live::Off;
    }
    Live::SignedIn(
        signed
            .into_iter()
            .map(|x| gamertag(x).unwrap_or_default())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(tags: &[&str]) -> Session {
        Session {
            sim_mode: Some(1),
            lockstep: false,
            live: Live::SignedIn(tags.iter().map(|t| t.to_string()).collect()),
        }
    }

    #[test]
    fn offline_allows_everything() {
        for mode in [None, Some(0), Some(1)] {
            let s = Session {
                sim_mode: mode,
                ..Session::default()
            };
            assert!(refusals(&s).none(), "{mode:?}");
        }
    }

    #[test]
    fn live_allows_only_the_listed_gamertags() {
        let oks: [&[&str]; 4] = [&["afr"], &["AFR"], &[" Afr "], &["afr", "aFr"]];
        for ok in oks {
            assert!(refusals(&live(ok)).none(), "{ok:?}");
        }
        let nos: [&[&str]; 6] = [
            &["Chief"][..],
            &["afr", "Chief"],
            &[""],
            &[],
            &["afr2"],
            &["a fr"],
        ];
        for no in nos {
            let r = refusals(&live(no));
            assert!(
                r.all
                    .as_deref()
                    .is_some_and(|w| w.starts_with("cheats are off on Xbox Live")),
                "{no:?}: {r:?}"
            );
            for reach in [Reach::Local, Reach::Simulation, Reach::Clock] {
                assert!(r.of(reach).is_some());
            }
        }
    }

    #[test]
    fn live_state_comes_from_the_signed_in_users() {
        let tag = |x: u64| (x == 7).then(|| "afr".to_string());
        assert_eq!(live_from(&[0, 0, 0, 0], tag), Live::Off);
        assert_eq!(
            live_from(&[7, 0, 0, 0], tag),
            Live::SignedIn(vec!["afr".into()])
        );
        // A user this machine has no account for: unknown, so refused.
        let s = Session {
            live: live_from(&[7, 9, 0, 0], tag),
            ..Session::default()
        };
        assert_eq!(s.live, Live::SignedIn(vec!["afr".into(), String::new()]));
        assert!(refusals(&s).all.is_some());
    }

    #[test]
    fn lockstep_refuses_even_afr() {
        let mut s = live(&["afr"]);
        s.lockstep = true;
        assert_eq!(refusals(&s), Refusals::every(LOCKSTEP));
        s.live = Live::Off;
        assert_eq!(refusals(&s), Refusals::every(LOCKSTEP));
    }

    #[test]
    fn co_op_and_guests_refuse_the_simulation_but_not_what_is_drawn() {
        for (mode, why) in [(2, CO_OP), (3, CO_OP), (4, GUEST)] {
            for l in [Live::Off, Live::SignedIn(vec!["afr".into()])] {
                let r = refusals(&Session {
                    sim_mode: Some(mode),
                    lockstep: false,
                    live: l,
                });
                assert_eq!(r.of(Reach::Simulation), Some(why), "{mode}");
                assert_eq!(r.of(Reach::Clock), Some(why), "{mode}");
                assert_eq!(r.of(Reach::Local), None, "{mode}");
            }
        }
    }

    #[test]
    fn a_networked_host_keeps_the_simulation_cheats_not_the_clock() {
        for l in [Live::Off, Live::SignedIn(vec!["afr".into()])] {
            let r = refusals(&Session {
                sim_mode: Some(5),
                lockstep: false,
                live: l,
            });
            assert_eq!(r.of(Reach::Simulation), None);
            assert_eq!(r.of(Reach::Local), None);
            assert_eq!(r.of(Reach::Clock), Some(HOST_CLOCK));
        }
        // Not afr on Live: refused as a host too.
        let mut s = live(&["Chief"]);
        s.sim_mode = Some(5);
        assert!(refusals(&s).of(Reach::Simulation).is_some());
    }
}
