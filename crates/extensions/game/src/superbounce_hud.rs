//! The superbounce HUD in the game (docs/game/hud-kit.md, "The runtime,
//! for this HUD"): `hud_kit_core`'s superbounce HUD, registered with the
//! HUD kit ([`crate::hud_kit`]) and fed each tick from
//! [`superbounce_mode`]'s state through one adapter, [`view`], with the
//! tutorial's step machine and its `superbounce.tutorial_seen` setting.
//!
//! The HUD shows for the first local player while the mode is on; with
//! the mode off every element is hidden. Its notifications (primed, a
//! loss's reason, a landing that did not bounce, and every local player's
//! medals) are sent through the kit's `Handle::notify` once each
//! ([`Sent::notes`]).

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use hud_kit_core::huds::superbounce::{
    self as sb, Change, Charge, Landed, Lost, Medal, Note, Phase, Scan, Superbounce, Target,
    Tutorial, Verb, View, Watch,
};
use hud_kit_core::model::Values;

use crate::hud_kit::{self, Handle};
use crate::setting_defs::{
    SUPERBOUNCE_TUTORIAL_ENV, SUPERBOUNCE_TUTORIAL_SEEN, superbounce_tutorial_shows,
};
use crate::superbounce_mode::{self as mode, METRES_PER_UNIT, ScanState, State};

/// The feature's id.
pub const FEATURE: &str = "superbounce-hud";

/// The HUD's [`View`] of the mode's state `s`, with the tutorial at step
/// `tutorial`.
///
/// - The scan: ready with spots, ready with none, failed, else scanning.
/// - The charge: the mode's phase and fill as they are.
/// - The verb: the mode's (a crouch step, the warning to keep moving, the
///   new charge after a loss); in the near phase, not primed and with no
///   verb of the mode's, "hold (crouch) and walk under".
/// - The navpoint: the mode's target and its distance, world units to
///   metres (S: the contract's distance is in world units, as the state's
///   other distances are).
/// - The note: the newer of the loss ([`mode::lost`]: why, at its tick)
///   and the last landing's result at its tick ([`note`]); a bounce's
///   line is its medal.
/// - The medal: the last award of a local player, its highest medal at
///   its tick ([`medal`]).
pub fn view(s: &State, tutorial: Option<u8>) -> View {
    let scan = match &s.scan {
        ScanState::Ready if s.bounce + s.crouch == 0 => Scan::NoStrips,
        ScanState::Ready => Scan::Ready,
        ScanState::Failed(_) => Scan::Failed,
        ScanState::Idle | ScanState::Scanning => Scan::Scanning,
    };
    let charge = s.charge.as_ref().map(|c| Charge {
        phase: match &c.phase {
            mode::ChargePhase::Near => Phase::Near,
            mode::ChargePhase::Under => Phase::Under,
            mode::ChargePhase::Held => Phase::Held,
            mode::ChargePhase::Primed => Phase::Primed,
            mode::ChargePhase::Lost => Phase::Lost,
        },
        fill: c.fill,
    });
    let verb = match s.verb.as_ref() {
        Some(mode::Verb::LetGoOfCrouch) => Some(Verb::LetGoOfCrouch),
        Some(mode::Verb::WalkOutToStand) => Some(Verb::WalkOutToStand),
        Some(mode::Verb::KeepMoving) => Some(Verb::KeepMoving),
        Some(mode::Verb::ChargeAgain(l)) => Some(Verb::ChargeAgain(lost(l.reason))),
        None if !s.primed() && charge.is_some_and(|c| c.phase == Phase::Near) => {
            Some(Verb::HoldAndWalkUnder)
        }
        None => None,
    };
    // A ghost run's prompt (superbounce_ghost.rs).
    let verb = crate::superbounce_ghost::hud_verb(verb);
    let nav = s.nav_target.as_ref().map(|n| {
        let target = match &n.kind {
            mode::NavKind::Crouch => Target::Crouch,
            mode::NavKind::Strip => Target::Strip,
        };
        (target, n.distance * METRES_PER_UNIT)
    });
    View {
        primed: s.primed(),
        scan,
        charge,
        verb,
        nav,
        note: note(s, mode::lost(s)),
        medal: medal(s),
        tutorial,
    }
}

/// The last medal award of a local player in `s`: its highest medal and
/// its tick.
pub fn medal(s: &State) -> Option<(Medal, u32)> {
    s.medals
        .recent
        .iter()
        .rev()
        .filter(|a| a.user.is_some())
        .find_map(|a| Some((crate::superbounce_chain::highest(&a.medals)?, a.tick)))
}

/// The HUD's name for a loss's reason.
fn lost(reason: mode::LostReason) -> Lost {
    match reason {
        mode::LostReason::StoodStill => Lost::StoodStill,
        mode::LostReason::Crouched => Lost::Crouched,
        mode::LostReason::Unknown => Lost::Unknown,
    }
}

/// The notification for `s` with the last loss `lost`: the loss's
/// reason when it is newer than the last landing (or there is none),
/// else the landing's result, each at its tick. A bounce has none: its
/// line is its medal.
pub fn note(s: &State, lost: Option<mode::Lost>) -> Option<(Note, u32)> {
    let landing = s.last_landing.as_ref();
    if let Some(l) = lost.filter(|l| landing.is_none_or(|landing| l.tick > landing.tick)) {
        return Some((Note::ChargeLost(self::lost(l.reason)), l.tick));
    }
    landing.and_then(|l| Some((Note::Landed(landed(l.result)?), l.tick)))
}

/// A landing's notification; `None` for a bounce.
fn landed(result: mode::LandingResult) -> Option<Landed> {
    match result {
        mode::LandingResult::Bounce => None,
        mode::LandingResult::NotPrimed => Some(Landed::NotPrimed),
        mode::LandingResult::Missed => Some(Landed::Missed),
        mode::LandingResult::TooSlow => Some(Landed::TooSlow),
        mode::LandingResult::NoBounce => Some(Landed::NoBounce),
    }
}

/// What the HUD has sent, so each notification goes once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sent {
    /// The state has been seen once: what it held then is not news.
    seen: bool,
    primed: bool,
    lost: Option<u32>,
    landing: Option<u32>,
    /// The last medal award sent (`superbounce_chain::Award::seq`).
    medal: u64,
}

impl Sent {
    /// The notifications `s` has that were not sent yet, each a local
    /// user and its line, in order: primed, a loss's reason, a landing
    /// that did not bounce (each the first local player's, user 0), then
    /// each new medal award of a local player, its highest medal's line,
    /// to that player's user. `s` of `None` (the mode off) forgets all.
    pub fn notes(&mut self, s: Option<&State>, lost: Option<mode::Lost>) -> Vec<(u32, String)> {
        let Some(s) = s else {
            *self = Sent::default();
            return Vec::new();
        };
        let landing = s.last_landing.as_ref();
        let last_seq = s.medals.recent.last().map_or(0, |a| a.seq);
        // The medals start again at a new game.
        if last_seq < self.medal {
            self.medal = 0;
        }
        let mut out = Vec::new();
        if self.seen {
            if s.primed() && !self.primed {
                out.push((0, Note::Primed.text()));
            }
            if let Some(l) = lost.filter(|l| Some(l.tick) != self.lost) {
                out.push((0, Note::ChargeLost(self::lost(l.reason)).text()));
            }
            if let Some(l) = landing.filter(|l| Some(l.tick) != self.landing)
                && let Some(n) = landed(l.result)
            {
                out.push((0, Note::Landed(n).text()));
            }
            for a in s.medals.recent.iter().filter(|a| a.seq > self.medal) {
                if let (Some(user), Some(m)) =
                    (a.user, crate::superbounce_chain::highest(&a.medals))
                {
                    out.push((u32::from(user), m.line()));
                }
            }
        }
        self.seen = true;
        self.primed = s.primed();
        self.lost = lost.map(|l| l.tick).or(self.lost);
        self.landing = landing.map(|l| l.tick).or(self.landing);
        self.medal = self.medal.max(last_seq);
        out
    }
}

/// What the tutorial watches in `s`: primed, metres to the nearest bounce
/// spot, and a bounce landed since `seen_tick` (the last landing's tick
/// the tutorial saw).
pub fn watch(s: &State, seen_tick: Option<u32>) -> Watch {
    let bounced = s.last_landing.as_ref().is_some_and(|l| {
        matches!(l.result, mode::LandingResult::Bounce) && Some(l.tick) != seen_tick
    });
    Watch {
        primed: s.primed(),
        strip_m: s.nearest.map(|n| n.distance * METRES_PER_UNIT),
        bounced,
    }
}

/// The values with the mode off: every element hidden.
pub fn hidden() -> Values {
    sb::values(&View {
        scan: Scan::NoStrips,
        ..View::idle()
    })
}

struct Hud {
    handle: Handle,
    tutorial: Tutorial,
    /// The last loss, kept past the mode's state (which has it for
    /// [`crate::superbounce_charge::AGAIN_TICKS`]) so its note does not
    /// give way to an older landing's.
    lost: Option<mode::Lost>,
    seen_tick: Option<u32>,
    /// A landing older than the HUD's first tick is not a bounce of the
    /// tutorial's.
    started: bool,
    start: Instant,
    settings: Option<Arc<settings::Store>>,
    sent: Sent,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Hud {
    fn tick(&mut self) {
        let Some(s) = mode::hud_state() else {
            self.lost = None;
            self.sent.notes(None, None);
            self.handle.publish(hidden());
            self.handle.navpoint(None);
            return;
        };
        for (user, text) in self.sent.notes(Some(&s), mode::lost(&s)) {
            self.handle.notify(user, &text);
        }
        self.lost = mode::lost(&s).or(self.lost);
        let now = self.start.elapsed().as_secs_f64();
        let mut w = watch(&s, self.seen_tick);
        if !self.started {
            self.started = true;
            w.bounced = false;
        }
        self.seen_tick = s.last_landing.as_ref().map(|l| l.tick).or(self.seen_tick);
        if self.tutorial.update(Some(w), now) == Change::Done {
            self.mark_seen();
        }
        let v = View {
            note: note(&s, self.lost),
            ..view(&s, self.tutorial.step())
        };
        self.handle.publish(sb::values(&v));
        self.handle.navpoint(
            s.nav_target
                .as_ref()
                .filter(|_| v.nav.is_some())
                .map(|n| n.at),
        );
    }

    fn mark_seen(&self) {
        let Some(store) = &self.settings else {
            return;
        };
        match store.set(SUPERBOUNCE_TUTORIAL_SEEN.key, settings::Value::Bool(true)) {
            Ok(o) => {
                if let Some(e) = o.not_saved {
                    eprintln!("superbounce: the tutorial's seen flag was not saved: {e}");
                }
            }
            Err(e) => eprintln!("superbounce: the tutorial's seen flag: {e}"),
        }
    }
}

struct SuperbounceHud {
    hud: Arc<Mutex<Hud>>,
}

impl hooks::Feature for SuperbounceHud {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce HUD on the HUD kit")
            .doc("docs/game/hud-kit.md")
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Rebuilt from the mode's state each tick.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let hud = Arc::clone(&self.hud);
        b.frame()
            .on_tick("superbounce hud: values", move |_| lock(&hud).tick());
        Ok(())
    }
}

/// Registers the superbounce HUD with the HUD kit and adds the feature
/// that feeds it. The tutorial shows unless `superbounce.tutorial_seen` is
/// set in `settings`, or `H2_SUPERBOUNCE_TUTORIAL=1` asks for it anyway;
/// its last step sets the flag.
pub fn install(
    registry: &mut hooks::Registry,
    settings: Option<Arc<settings::Store>>,
) -> anyhow::Result<()> {
    let seen = settings
        .as_ref()
        .is_some_and(|s| s.handle::<bool>(SUPERBOUNCE_TUTORIAL_SEEN.key).get());
    let env = std::env::var(SUPERBOUNCE_TUTORIAL_ENV).ok();
    let show = superbounce_tutorial_shows(seen, env.as_deref());
    let handle = hud_kit::register(Arc::new(Superbounce));
    handle.publish(hidden());
    let hud = Hud {
        handle,
        tutorial: Tutorial::new(show),
        lost: None,
        seen_tick: None,
        started: false,
        start: Instant::now(),
        settings,
        sent: Sent::default(),
    };
    registry
        .add(&SuperbounceHud {
            hud: Arc::new(Mutex::new(hud)),
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("superbounce hud: {e}"))
}

#[cfg(test)]
#[path = "superbounce_hud_tests.rs"]
mod tests;
