//! The superbounce HUD in the game (docs/game/hud-kit.md, "The runtime,
//! for this HUD"): `hud_kit_core`'s superbounce HUD, registered with the
//! HUD kit ([`crate::hud_kit`]) and fed each tick from
//! [`superbounce_mode`]'s state through one adapter, [`view`], with the
//! tutorial's step machine and its `superbounce.tutorial_seen` setting.
//!
//! The HUD shows for the first local player while the mode is on; with
//! the mode off every element is hidden. Its notifications ("Primed!", a
//! loss's reason, and every local player's medals unless the variant
//! turned them off; the arming bar and the prompt say "charging") are sent
//! through the kit's `Handle::notify` once each ([`Sent::notes`]). Why a
//! landing did not bounce is the readout's line
//! ([`crate::superbounce_readout`]), not a notification. The arming bar
//! follows the charge's phase tick to tick ([`ArmTrack`]); the kit plays
//! its motion on wall time, and the primed ending once per priming
//! ([`EndingOnce`]). After a loss the prompt waits for the reason's line
//! ([`after_reason`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::memory::{GuestMemory, MemFault};

use hud_kit_core::views::superbounce::{
    self as sb, Arm, ArmTrack, Change, Charge, Lost, Medal, Note, Phase, Scan, Superbounce, Target,
    Tutorial, Verb, View, Watch, arm_for,
};
use hud_kit_core::model::Values;
use hud_kit_core::motion::Motion;

use crate::hud_kit::{self, Handle};
use crate::setting_defs::{
    SUPERBOUNCE_TUTORIAL_ENV, SUPERBOUNCE_TUTORIAL_SEEN, superbounce_tutorial_shows,
};
use crate::superbounce_mode::{self as mode, METRES_PER_UNIT, ScanState, State};

/// The feature's id.
pub const FEATURE: &str = "superbounce-hud";

/// Game ticks a second.
const TICKS_PER_SECOND: f64 = 30.0;

/// The tutorial's clock after `ticks` of the HUD's: game seconds, so a
/// step's time limit is the same at every frame rate and runs slow with
/// the game below 30 fps.
pub fn tutorial_seconds(ticks: u64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND
}

/// The HUD's [`View`] of the mode's state `s`, with the tutorial at step
/// `tutorial`.
///
/// - The scan: ready with spots, ready with none, failed, else scanning.
/// - The charge: the mode's phase and fill as they are; the arming bar,
///   the phase's on its own ([`arm_for`]: the tick loop's [`ArmTrack`]
///   adds the run out).
/// - The verb: the mode's (a crouch step, the warning to keep moving, the
///   new charge after a loss); in the near phase, not primed and with no
///   verb of the mode's, "hold (crouch) and walk under".
/// - The navpoint: the mode's target and its number in metres (world
///   units to metres; S: the contract's distances are in world units, as
///   the state's other distances are): over a crouch spot the distance to
///   it, over a strip the drop to come from (its `fall`; 0 for a standing
///   jump).
/// - The note: the loss ([`mode::lost`]: why, at its tick; [`note`]). A
///   landing has none: a bounce's line is its medal, and why another did
///   not bounce is the readout's.
/// - The medal: the last award of a local player, its highest medal at
///   its tick ([`medal`]).
pub fn view(s: &State, tutorial: Option<u8>) -> View {
    let scan = match &s.scan {
        ScanState::Ready if s.bounce + s.crouch == 0 => Scan::NoStrips,
        ScanState::Ready => Scan::Ready,
        ScanState::Failed(_) => Scan::Failed,
        ScanState::Idle | ScanState::Scanning => Scan::Scanning,
    };
    let charge = charge(s);
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
    let nav = s.nav_target.as_ref().map(|n| match &n.kind {
        mode::NavKind::Crouch => (Target::Crouch, n.distance * METRES_PER_UNIT),
        mode::NavKind::Strip => (Target::Strip, n.fall.max(0.0) * METRES_PER_UNIT),
    });
    View {
        primed: s.primed(),
        scan,
        charge,
        arm: arm_for(charge.map(|c| c.phase)),
        verb,
        nav,
        note: note(mode::lost(s)),
        medal: medal(s),
        tutorial,
    }
}

/// The mode's charge in `s` as the HUDs read it: the phase of the recipe
/// and the fill (`superbounce_charge::ChargePhase`, one for one).
pub fn charge(s: &State) -> Option<Charge> {
    s.charge.as_ref().map(|c| Charge {
        phase: match &c.phase {
            mode::ChargePhase::Near => Phase::Near,
            mode::ChargePhase::Under => Phase::Under,
            mode::ChargePhase::Held => Phase::Held,
            mode::ChargePhase::Primed => Phase::Primed,
            mode::ChargePhase::Lost => Phase::Lost,
        },
        fill: c.fill,
    })
}

/// `v` as it is shown in the air: the notification and the medal stay;
/// the arming bar, the prompt line, the navpoint and the tutorial step
/// go. In the air the screen is the readout's and
/// the tracker's (docs/game/superbounce.md, "In the air").
pub fn in_air(v: View) -> View {
    View {
        charge: None,
        arm: None,
        verb: None,
        nav: None,
        tutorial: None,
        ..v
    }
}

/// Game ticks the prompt waits after a loss for the reason's line to be
/// read: two seconds.
pub const REASON_TICKS: u32 = 60;

/// `v` with the "charge again" prompt held back while the loss's reason
/// (`lost`, at its tick) is new: the reason shows first, the prompt after
/// it, not the two over one another.
pub fn after_reason(v: View, s: &State, lost: Option<mode::Lost>) -> View {
    let new = s
        .tick
        .zip(lost)
        .is_some_and(|(now, l)| now.checked_sub(l.tick).is_none_or(|n| n < REASON_TICKS));
    if new && matches!(v.verb, Some(Verb::ChargeAgain(_))) {
        View { verb: None, ..v }
    } else {
        v
    }
}

/// Ticks the primed ending takes on the bar: 0.6 s of game ticks.
pub const ENDING_TICKS: u32 = 18;

/// The arming bar's primed ending, once per priming. The bar stays the
/// ending while primed, and the kit restamps the motion whenever the bar
/// shows again (it is hidden in the air, [`in_air`]), so a landing would
/// play it again. After [`ENDING_TICKS`] of it the bar is `None` until the
/// charge leaves the primed phase.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EndingOnce {
    ticks: u32,
}

impl EndingOnce {
    /// The bar to show for this tick's `arm`.
    pub fn update(&mut self, arm: Option<Arm>) -> Option<Arm> {
        match arm {
            Some(a) if a.motion == Motion::Ending => {
                self.ticks = self.ticks.saturating_add(1);
                (self.ticks <= ENDING_TICKS).then_some(a)
            }
            other => {
                self.ticks = 0;
                other
            }
        }
    }

    /// The mode went off.
    pub fn clear(&mut self) {
        self.ticks = 0;
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

/// The notification for the last loss `lost`: its reason at its tick.
pub fn note(lost: Option<mode::Lost>) -> Option<(Note, u32)> {
    lost.map(|l| (Note::ChargeLost(self::lost(l.reason)), l.tick))
}

/// What the HUD has sent, so each notification goes once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sent {
    /// The state has been seen once: what it held then is not news.
    seen: bool,
    primed: bool,
    lost: Option<u32>,
    /// The last medal award sent (`superbounce_chain::Award::seq`).
    medal: u64,
}

impl Sent {
    /// The notifications `s` has that were not sent yet, each a local
    /// user and its line, in order: primed, a loss's reason (each the first local player's,
    /// user 0), then, with `medals` (the variant's option), each new
    /// medal award of a local player, its highest medal's line, to that
    /// player's user. An award made while `medals` is off is not sent
    /// later. `s` of `None` (the mode off) forgets all.
    pub fn notes(
        &mut self,
        s: Option<&State>,
        lost: Option<mode::Lost>,
        medals: bool,
    ) -> Vec<(u32, String)> {
        let Some(s) = s else {
            *self = Sent::default();
            return Vec::new();
        };
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
            for a in s
                .medals
                .recent
                .iter()
                .filter(|a| medals && a.seq > self.medal)
            {
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

// ---- the lock and the reticle ----

/// The HUD widget input Halo 2's unit lists as "autoaimed": the third
/// unit input, after shield (16) and body (17), before the grenade
/// counts (20, 21) and shield low (22) that are decoded
/// (docs/halo1/health.md). From Assembly's `nhdt` plugin's list and that
/// order; S: that the weapon reticles read it and their shader turns red
/// on it (docs/game/superbounce.md, "Lock-on").
pub const AUTOAIMED: u8 = 18;
/// The widget loop's return from a bitmap widget's inputs (E
/// `0x0013a532`; `hud_kit::INPUT_RETURNS`): the reticles are bitmaps.
pub const BITMAP_INPUTS_RETURN: u32 = 0x0013_a61c;

/// The lock holds this tick: the reticle reads autoaimed.
static LOCKED: AtomicBool = AtomicBool::new(false);

/// Whether the lock holds as of the last tick.
pub fn reticle_lock_now() -> bool {
    LOCKED.load(Ordering::Relaxed)
}

/// Whether the lock holds in `s` (primed, in the air, falling onto the
/// slot fast enough), the rule the world's red brackets follow.
pub fn reticle_locked(s: &State) -> bool {
    s.primed() && s.airborne && s.landing.is_some_and(|l| l.locked())
}

/// A retail bitmap widget's inputs call the lock answers on its way out:
/// where the call returns to (the stack after its `ret 12`), its four
/// outputs, and which of them read [`AUTOAIMED`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Autoaim {
    pub sp_after: u32,
    pub out: u32,
    pub slots: u8,
}

/// The slots of `inputs` that read [`AUTOAIMED`], one bit each.
pub fn autoaim_slots(inputs: [u8; 4]) -> u8 {
    inputs
        .iter()
        .enumerate()
        .filter(|(_, i)| **i == AUTOAIMED)
        .fold(0, |m, (k, _)| m | 1 << k)
}

/// At the inputs call's entry (`sp` at its return address): while
/// `locked`, the first local player's bitmap widget that reads
/// [`AUTOAIMED`] is answered on the way out. The game computes the call;
/// only those slots are changed after it.
pub fn autoaim_entry(
    mem: &dyn GuestMemory,
    sp: u32,
    locked: bool,
) -> Result<Option<Autoaim>, MemFault> {
    if !locked || mem.u32(sp)? != BITMAP_INPUTS_RETURN || mem.u32(sp + 4)? != 0 {
        return Ok(None);
    }
    let mut inputs = [0u8; 4];
    mem.read(mem.u32(sp + 8)?, &mut inputs)?;
    let slots = autoaim_slots(inputs);
    Ok((slots != 0).then_some(Autoaim {
        sp_after: sp + 16,
        out: mem.u32(sp + 12)?,
        slots,
    }))
}

/// On the way out of the call `a` was taken at (`sp` its stack after the
/// return): the autoaimed slots read 1, as over an enemy in range. A
/// return to another stack is not that call: nothing is written.
pub fn autoaim_exit(mem: &mut dyn GuestMemory, sp: u32, a: Autoaim) -> Result<bool, MemFault> {
    if sp != a.sp_after {
        return Ok(false);
    }
    for k in 0..4 {
        if a.slots & 1 << k != 0 {
            mem.set_f32(a.out + 4 * k, 1.0)?;
        }
    }
    Ok(true)
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
    /// Ticks the HUD has seen: the tutorial's clock ([`tutorial_seconds`]).
    ticks: u64,
    settings: Option<Arc<settings::Store>>,
    sent: Sent,
    /// The arming bar, tick to tick.
    arm: ArmTrack,
    /// Its primed ending, once per priming.
    ending: EndingOnce,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Hud {
    fn tick(&mut self) {
        self.ticks += 1;
        let Some(s) = mode::hud_state() else {
            LOCKED.store(false, Ordering::Relaxed);
            self.lost = None;
            self.arm.clear();
            self.ending.clear();
            self.sent.notes(None, None, true);
            self.handle.publish(hidden());
            self.handle.navpoint(None);
            return;
        };
        LOCKED.store(reticle_locked(&s), Ordering::Relaxed);
        let medals = crate::superbounce_options::current().medals;
        for (user, text) in self.sent.notes(Some(&s), mode::lost(&s), medals) {
            self.handle.notify(user, &text);
            // A medal line is spoken too (medal_announcer.rs).
            crate::medal_announcer::announce(&text);
        }
        self.lost = mode::lost(&s).or(self.lost);
        let now = tutorial_seconds(self.ticks);
        let mut w = watch(&s, self.seen_tick);
        if !self.started {
            self.started = true;
            w.bounced = false;
        }
        self.seen_tick = s.last_landing.as_ref().map(|l| l.tick).or(self.seen_tick);
        if self.tutorial.update(Some(w), now) == Change::Done {
            self.mark_seen();
        }
        let base = view(&s, self.tutorial.step());
        let v = View {
            note: note(self.lost),
            arm: self
                .ending
                .update(self.arm.update(base.charge.map(|c| c.phase))),
            ..base
        };
        let v = after_reason(v, &s, self.lost);
        let v = if s.airborne { in_air(v) } else { v };
        self.handle.publish(sb::values(&v));
        // The takeoff while the floor is too low for the strip, else the
        // target's spot (`State::navpoint`).
        self.handle
            .navpoint(s.navpoint.filter(|_| v.nav.is_some() && s.nav_target.is_some()));
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
        // Nothing is drawn over the reticle while the lock holds, and the
        // game's own is not turned red: an after hook on
        // HUD_WIDGET_INPUTS would put a guest hook on the HUD.
        // [`reticle_lock_now`] says whether it holds.
        Ok(())
    }
}

/// Registers the superbounce HUD and the readout with the HUD kit and
/// adds the features that feed them. The tutorial's steps are in the
/// training box, in the middle of the screen, so they show only when
/// `H2_SUPERBOUNCE_TUTORIAL=1` asks for them; the last step sets
/// `superbounce.tutorial_seen` in `settings`.
pub fn install(
    registry: &mut hooks::Registry,
    settings: Option<Arc<settings::Store>>,
) -> anyhow::Result<()> {
    let env = std::env::var(SUPERBOUNCE_TUTORIAL_ENV).ok();
    // Asked for only: as a tutorial that has been seen.
    let show = superbounce_tutorial_shows(true, env.as_deref());
    let handle = hud_kit::register(Arc::new(Superbounce));
    handle.publish(hidden());
    // The readout after it, so this HUD's navpoint is the kit's first
    // (superbounce_readout.rs).
    crate::superbounce_readout::install(registry)?;
    // Ghost runs are shown only when asked for (superbounce_ghost.rs).
    crate::superbounce_ghost::install_feature(registry)?;
    let hud = Hud {
        handle,
        tutorial: Tutorial::new(show),
        lost: None,
        seen_tick: None,
        started: false,
        ticks: 0,
        settings,
        sent: Sent::default(),
        arm: ArmTrack::default(),
        ending: EndingOnce::default(),
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
