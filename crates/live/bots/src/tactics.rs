//! Combat decisions (docs/game/bots.md, section 3.4, "Built: combat
//! boost"): whom to shoot, where on them, how to work the trigger of the
//! weapon in hand, when a grenade is safe, where to pre-aim, and the
//! signals the movement reads (the range to keep, wanting cover).
//!
//! Pure and deterministic: [`Tactics`] is the state the mind keeps, and
//! every function takes the tick. Time is the game's tick (30 a second).

use crate::arms::{Arms, Gun, Kit, Ranged, Role};
use crate::brain::{Seen, Vec3};
use crate::mind::{
    Skill,
    feel::{Rng, reached},
};

/// The target point's heights over a unit's origin: the chest (as the mind
/// has it), the head, and the feet (splash).
pub const CHEST: f32 = 0.4;
pub const HEAD: f32 = 0.6;
pub const FEET: f32 = 0.05;

/// A target's cost is its distance plus this per radian off the aim:
/// one in front beats one a little nearer behind.
pub const TURN_COST: f32 = 4.0;
/// The current target's cost is scaled by this: another must be clearly
/// better to take its place (no flip-flop).
pub const STICK: f32 = 0.6;
/// An enemy back in view within this many ticks of last being seen needs
/// no new reaction: strafing round a pillar does not freeze the bot.
pub const GRACE: u32 = 45;
/// One visual reaction, even for an alert Legendary bot, takes at least
/// five ticks (167 ms). A reappearance during grace keeps the old clock.
pub const MIN_REACTION: u32 = 5;
/// Enemies remembered at most this long (ticks) for [`GRACE`].
const FORGET: u32 = 4 * GRACE;

/// A scripted pull (the noob combo's release) lasts this many ticks.
pub const PULL_TICKS: u32 = 2;
/// Pulls come at least this many ticks apart (so it lets go).
pub const PULL_MIN: u32 = 4;
/// Pulls this far apart when the weapon's rate is not known.
pub const PULL_DEFAULT: u32 = 8;
/// A held trigger, once down, stays down while the aim is within this
/// many fire cones of the aim point...
pub const HOLD_CONE: f32 = 2.2;
/// ...and through this many ticks outside that.
pub const HOLD_GRACE: u32 = 4;

/// No grenade at a target with a teammate this close to it.
pub const FRIENDLY_SPLASH: f32 = 3.0;
/// Pre-aims at where an enemy went out of view within this (world units)...
pub const PRE_AIM_RANGE: f32 = 15.0;
/// ...while walking no more than this far (radians) away from facing it.
pub const PRE_AIM_CONE: f32 = 100.0 * std::f32::consts::PI / 180.0;

/// What skill changes in the fight, beyond the aim model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Aims at the head with a precise weapon or a sniper rifle.
    pub head: bool,
    /// Aims at the feet with a splash weapon.
    pub feet: bool,
    /// Extra ticks between pulls of a once-per-pull trigger.
    pub pace: u32,
    /// Refines range fit and enables advanced inventory decisions.
    pub by_range: bool,
    /// Pre-aims at where an enemy went out of view.
    pub pre_aim: bool,
    /// Holds a grenade when a teammate is near the target.
    pub careful_grenades: bool,
}

impl Style {
    pub(crate) fn with_behavior(skill: Skill, behavior: crate::decisions::Behavior) -> Style {
        let mut style = Self::of(skill);
        // Patient bots keep their eyes on a corner, even on Easy. A panicky
        // bot lets the direction of travel take over when leaving a fight.
        style.pre_aim =
            (style.pre_aim || behavior.personality.wait_weight() > 0.43) && behavior.fear() < 0.30;
        style
    }

    pub fn of(skill: Skill) -> Style {
        match skill {
            Skill::Easy => Style {
                head: false,
                feet: false,
                pace: 12,
                by_range: false,
                pre_aim: false,
                careful_grenades: true,
            },
            Skill::Normal => Style {
                head: false,
                feet: false,
                pace: 6,
                by_range: true,
                pre_aim: true,
                careful_grenades: true,
            },
            Skill::Heroic => Style {
                head: true,
                feet: true,
                pace: 3,
                by_range: true,
                pre_aim: true,
                careful_grenades: true,
            },
            Skill::Legendary => Style {
                head: true,
                feet: true,
                pace: 1,
                by_range: true,
                pre_aim: true,
                careful_grenades: true,
            },
        }
    }
}

/// How the trigger is worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pull {
    /// Held, in bursts of on and off ticks ([`hold_pattern`]; `None`:
    /// held throughout).
    Hold(Option<(u32, u32)>),
    /// Pulled for 2–3 ticks. The number is the minimum time between
    /// starts: holding a once-per-pull trigger fires only one shot.
    Tap(u32),
    /// No gunfire. The mind chooses B or the lunge trigger for a sword.
    Never,
}

/// What the held weapon means for shooting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub role: Role,
    pub pull: Pull,
    /// The projectile's speed for lead (world units a second).
    pub speed: f32,
    /// Shoots at targets this far, no nearer or further: the gun's reach
    /// ([`crate::weapons::reach`]), using its aim assist and projectile.
    pub range: (f32, f32),
    /// A blade's lunge (world units): it swings this close. 0 for a gun.
    pub lunge: f32,
    /// The target point's height over the target's origin.
    pub height: f32,
    /// Something to fire now: never dry-fires.
    pub ready: bool,
}

impl Shot {
    /// For the gun in hand (`None`: arms not read, the weapon-blind
    /// behaviour), with `hold` for a held trigger ([`hold_pattern`]) and
    /// the lead speed `default_speed` when the projectile is not known.
    pub fn of(
        held: Option<Gun>,
        read: bool,
        style: &Style,
        hold: Option<(u32, u32)>,
        default_speed: f32,
    ) -> Shot {
        let role = held.map_or(Role::Unknown, |g| g.role());
        let p = held.map(|g| g.profile).unwrap_or_default();
        let pull = match p.trigger {
            crate::arms::Trigger::Single | crate::arms::Trigger::Charge => {
                let every = if p.rate > 0.0 {
                    (30.0 / p.rate).ceil() as u32
                } else {
                    PULL_DEFAULT
                };
                Pull::Tap(every.max(PULL_MIN).saturating_add(style.pace))
            }
            crate::arms::Trigger::Absent if role == Role::Blade => Pull::Never,
            _ => Pull::Hold(hold),
        };
        let height = match role {
            Role::Precise | Role::Sniper if style.head => HEAD,
            Role::Blast if style.feet => FEET,
            _ => CHEST,
        };
        Shot {
            role,
            pull,
            speed: if p.shot_speed > 0.0 {
                p.shot_speed
            } else {
                default_speed
            },
            range: held.map_or(role.fire_range(), |g| g.reach().fire),
            lunge: held.map_or(0.0, |g| g.reach().lunge),
            height,
            // Arms not read: as before, it always may.
            ready: !read || held.is_some_and(|g| g.ammo.ready()),
        }
    }
    /// Whether a target `d` away is one to shoot at.
    pub fn in_range(&self, d: f32) -> bool {
        (self.range.0..self.range.1).contains(&d)
    }
}

/// How a held trigger is worked on a target `dist` away with a gun whose
/// band ends at `band_far`: inside the band it is held until the target
/// or the magazine is gone, at every skill; beyond it, the skill's `burst`
/// (`None`, Easy's: held anyway), its on time stretched by a spraying
/// name's `spray` (0 to 1, [`spray`]) from 0.7 to 1.5 times.
pub fn hold_pattern(
    dist: f32,
    band_far: f32,
    burst: Option<(u32, u32)>,
    spray: f32,
) -> Option<(u32, u32)> {
    let (on, off) = burst?;
    if dist <= band_far {
        return None;
    }
    let stretch = 0.7 + 0.8 * spray.clamp(0.0, 1.0);
    Some((((on as f32) * stretch).round().max(1.0) as u32, off))
}

/// How much a name lets the trigger run (0 to 1): its aggression and its
/// reaction jitter, half each; 0.5 with no name.
pub(crate) fn spray(weights: Option<crate::name_profile::Weights>) -> f32 {
    weights.map_or(0.5, |w| 0.5 * w.aggression + 0.5 * w.reaction_jitter)
}

/// A held trigger: down on target, kept down through a near miss
/// ([`HOLD_CONE`], [`HOLD_GRACE`]), in bursts when it has a pattern. A
/// burst let go before its end is suspended: the next press finishes it
/// without waiting out the gap.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Hold {
    down: bool,
    /// Where the burst under way ends (`None`: held until let go).
    until: Option<u32>,
    /// The gap that follows the burst under way.
    gap: u32,
    /// The rest of a burst let go early.
    rest: Option<u32>,
    /// The first tick of this run outside the near cone.
    strayed: Option<u32>,
    /// No press before this tick.
    next: Option<u32>,
}

impl Hold {
    pub(crate) fn down(&self) -> bool {
        self.down
    }

    /// Let go, if down, and press again no sooner than two ticks on.
    pub(crate) fn stop(&mut self, tick: u32) {
        if self.down {
            let release = tick.wrapping_add(2);
            if self.next.is_none_or(|next| reached(release, next)) {
                self.next = Some(release);
            }
        }
        let next = self.next;
        *self = Hold {
            next,
            ..Hold::default()
        };
    }

    /// Whether the trigger is down this tick: pressed when `on` (the aim
    /// in the fire cone), kept down while `near` (within [`HOLD_CONE`]
    /// cones) or for [`HOLD_GRACE`] ticks without.
    pub(crate) fn step(
        &mut self,
        tick: u32,
        pattern: Option<(u32, u32)>,
        on: bool,
        near: bool,
        rng: &mut Rng,
    ) -> bool {
        if self.down {
            if self.until.is_some_and(|until| reached(tick, until)) {
                *self = Hold {
                    next: Some(tick.wrapping_add(self.gap)),
                    ..Hold::default()
                };
                return false;
            }
            // Out past the band mid-hold: this hold becomes a burst.
            if self.until.is_none()
                && let Some(burst) = pattern
            {
                let (hold, gap) = draw(rng, burst);
                self.until = Some(tick.wrapping_add(hold));
                self.gap = gap;
            }
            if near {
                self.strayed = None;
                return true;
            }
            let strayed = *self.strayed.get_or_insert(tick);
            if tick.wrapping_sub(strayed) < HOLD_GRACE {
                return true;
            }
            let rest = self
                .until
                .map(|until| until.wrapping_sub(tick))
                .filter(|&rest| rest > 0 && rest < 1 << 16);
            *self = Hold {
                rest,
                gap: self.gap,
                next: Some(tick.wrapping_add(2)),
                ..Hold::default()
            };
            return false;
        }
        if !on || self.next.is_some_and(|next| !reached(tick, next)) {
            return false;
        }
        self.down = true;
        self.strayed = None;
        match (pattern, self.rest.take()) {
            (None, _) => self.until = None,
            (Some(_), Some(rest)) => self.until = Some(tick.wrapping_add(rest)),
            (Some(burst), None) => {
                let (hold, gap) = draw(rng, burst);
                self.until = Some(tick.wrapping_add(hold));
                self.gap = gap;
            }
        }
        true
    }
}

/// One burst's on and gap ticks: the pattern's, give or take two.
fn draw(rng: &mut Rng, (on, off): (u32, u32)) -> (u32, u32) {
    let hold = rng.ticks(on.saturating_sub(2).max(1), on.saturating_add(2).max(1));
    let gap = rng.ticks(off.saturating_sub(2).max(1), off.saturating_add(2).max(1));
    (hold, gap)
}

/// Plan the weapon draw for where an approaching opponent will be when
/// the switch completes. Only consecutive visible samples supply velocity.
pub(crate) fn draw_distance(me: Vec3, foe: Vec3, velocity: Vec3) -> f32 {
    let range = crate::decisions::distance(me, foe);
    let next = std::array::from_fn(|i| foe[i] + 0.5 * velocity[i]);
    // A bad sample (respawn, teleport) cannot summon a sword at range.
    crate::decisions::distance(me, next).min(range).max((range - 2.0).max(0.0))
}

/// A beatdown uses the chest line regardless of where the gun aims.
/// Commit from the outer edge of reach for a shield break, a plasma
/// weapon, or a rush. A sword can always use its own lunge.
pub(crate) fn melee_window(
    dist: f32,
    angle: f32,
    held: Option<Gun>,
    shield_down: bool,
    rushing: bool,
) -> bool {
    let blade = held.is_some_and(|g| g.role() == Role::Blade && g.ready());
    let plasma = held.is_some_and(|g| matches!(
        g.profile.dual,
        Some(crate::arms::DualGun::PlasmaPistol | crate::arms::DualGun::PlasmaRifle)
    ));
    let reach = if blade {
        held.map_or(0.0, |g| g.reach().lunge)
    } else if shield_down || plasma || rushing {
        crate::mind::MELEE_RANGE * 1.3
    } else {
        crate::mind::MELEE_RANGE
    };
    dist < reach && angle < 0.85
}

/// A softened target this close is run down for the beatdown with an
/// automatic, a shotgun or nothing read (world units)...
pub const FINISH_PUSH: f32 = 6.0;
/// ...and with a rifle only this close: further out its headshot finishes.
pub const FINISH_PUSH_RIFLE: f32 = 2.5;

/// Whether to stop strafing and run straight in on a target whose shield
/// is down (or that has taken a long burst), as a player finishes with a
/// melee instead of letting the shield come back. Never with a sniper, a
/// splash weapon or a sword (the sword has its own lunge).
pub(crate) fn finish_push(held: Option<Gun>, dist: f32, softened: bool) -> bool {
    if !softened {
        return false;
    }
    let reach = match held.map(|g| (g.role(), g.ready())) {
        None | Some((_, false)) => FINISH_PUSH,
        Some((Role::Spray | Role::Scatter | Role::Unknown, true)) => FINISH_PUSH,
        Some((Role::Precise, true)) => FINISH_PUSH_RIFLE,
        Some(_) => 0.0,
    };
    dist < reach
}

#[derive(Clone, Copy, Debug)]
struct Notice {
    id: u32,
    ready: u32,
    last: u32,
}

/// The fight's state, kept by the mind.
#[derive(Clone, Debug)]
pub struct Tactics {
    reaction_rng: Rng,
    trigger_rng: Rng,
    known: Vec<Notice>,
    pull: Option<Pull>,
    next_pull: Option<u32>,
    pull_until: Option<u32>,
    hold: Hold,
    /// For the movement: the range (min, max, world units) to keep the
    /// target at, from the held weapon ([`crate::arms::preferred_range`]).
    pub band: (f32, f32),
    /// For the movement: shields down, or reloading or switching with the
    /// enemy within the band: break line of sight if it can.
    pub wants_cover: bool,
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len(a: Vec3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn angle_between(a: Vec3, b: Vec3) -> f32 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (len(a) * len(b)).max(1e-9);
    d.clamp(-1.0, 1.0).acos()
}

/// The target to fight among `seen`, from an eye at `eye` aiming along
/// `aim`: the cheapest by distance plus [`TURN_COST`] per radian off the
/// aim, the current one's cost scaled by [`STICK`].
pub fn choose(seen: &[Seen], eye: Vec3, aim: Vec3, current: Option<u32>) -> Option<Seen> {
    choose_weighted(seen, eye, aim, current, |_| 1.0)
}

pub(crate) fn choose_weighted(
    seen: &[Seen],
    eye: Vec3,
    aim: Vec3,
    current: Option<u32>,
    weight: impl Fn(Seen) -> f32,
) -> Option<Seen> {
    seen.iter()
        .map(|o| {
            let to = sub([o.position[0], o.position[1], o.position[2] + CHEST], eye);
            let mut cost = len(to) + TURN_COST * angle_between(to, aim);
            if current == Some(o.id) {
                cost *= STICK;
            }
            (cost * weight(*o), *o)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, o)| o)
}

impl Default for Tactics {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Tactics {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            reaction_rng: Rng::new(seed ^ 0x7265_6163_746e),
            trigger_rng: Rng::new(seed ^ 0x7472_6967_6765),
            known: Vec::new(),
            pull: None,
            next_pull: None,
            pull_until: None,
            hold: Hold::default(),
            band: (0.0, 0.0),
            wants_cover: false,
        }
    }

    /// Sample once when the bot takes notice. A return within the grace
    /// window keeps the original deadline, including an unfinished reaction.
    #[cfg(test)]
    fn focus(&mut self, id: u32, tick: u32, skill: Skill) -> u32 {
        self.focus_with_jitter(id, tick, skill, None)
    }

    pub(crate) fn focus_with_jitter(
        &mut self,
        id: u32,
        tick: u32,
        skill: Skill,
        jitter: Option<f32>,
    ) -> u32 {
        self.known.retain(|n| tick.wrapping_sub(n.last) <= FORGET);
        let prior = self.known.iter().position(|n| n.id == id);
        if let Some(i) = prior
            && tick.wrapping_sub(self.known[i].last) <= GRACE
        {
            self.known[i].last = tick;
            return self.known[i].ready;
        }
        let alert = self
            .known
            .iter()
            .any(|n| tick.wrapping_sub(n.last) <= GRACE);
        // Two uniform rolls put most reactions near the middle. Being
        // prepared shifts the distribution down without removing the floor.
        let nominal = skill.aim().reaction;
        let spread = match skill {
            Skill::Easy => 4,
            Skill::Normal => 3,
            Skill::Heroic | Skill::Legendary => 2,
        };
        let shift = if alert { 2 } else { 0 };
        let roll = match jitter {
            Some(jitter) => {
                // Interpolate between two early- and late-skewed draws.
                // The skill's full allowed window and reaction floor hold.
                let u = self.reaction_rng.unit();
                let early = u * u;
                let late = u * (2.0 - u);
                ((early + jitter * (late - early)) * (2 * spread) as f32).round() as u32
            }
            None => self.reaction_rng.ticks(0, spread) + self.reaction_rng.ticks(0, spread),
        };
        let delay = nominal.saturating_add(roll).saturating_sub(spread + shift).max(MIN_REACTION);
        let ready = tick.wrapping_add(delay);
        let notice = Notice {
            id,
            ready,
            last: tick,
        };
        if let Some(i) = prior {
            self.known[i] = notice;
        } else {
            self.known.push(notice);
        }
        ready
    }

    /// Forgets everyone (the bot died).
    pub(crate) fn forget(&mut self, id: u32) {
        self.known.retain(|notice| notice.id != id);
    }

    pub fn reset(&mut self) {
        self.known.clear();
        self.pull = None;
        self.next_pull = None;
        self.pull_until = None;
        self.hold = Hold::default();
        self.wants_cover = false;
    }

    /// Break the pull when the target disappears or firing is impossible.
    pub(crate) fn stop_trigger(&mut self, tick: u32) {
        if self.pull_until.take().is_some() {
            let release = tick.wrapping_add(2);
            if self.next_pull.is_none_or(|next| reached(release, next)) {
                self.next_pull = Some(release);
            }
        }
        self.hold.stop(tick);
    }

    /// Whether a held trigger is down (it rides out a near miss).
    pub(crate) fn holding(&self) -> bool {
        self.hold.down()
    }

    /// Whether the trigger is down this tick, with the aim `on` the fire
    /// cone and `near` it ([`HOLD_CONE`] cones). Tap lengths and recovery
    /// gaps vary. Burst starts are relative to when the bot lines up a
    /// shot; a held trigger stays down through a near miss ([`Hold`]).
    pub fn trigger(&mut self, tick: u32, pull: Pull, on: bool, near: bool) -> bool {
        // A held trigger whose pattern changes (the target crossing the
        // band's end) is the same hold.
        let changed = match (self.pull, pull) {
            (Some(Pull::Hold(_)), Pull::Hold(_)) => false,
            (Some(was), now) => was != now,
            (None, _) => true,
        };
        if changed {
            let was_down = self.pull_until.is_some() || self.hold.down();
            self.pull = Some(pull);
            self.pull_until = None;
            self.hold = Hold::default();
            self.next_pull = was_down.then_some(tick.wrapping_add(2));
            self.hold.next = self.next_pull;
        }
        self.pull = Some(pull);
        let ready = self.next_pull.is_none_or(|t| reached(tick, t));
        match pull {
            Pull::Never => false,
            Pull::Hold(pattern) => self
                .hold
                .step(tick, pattern, on, near, &mut self.trigger_rng),
            Pull::Tap(every) => {
                if self.pull_until.is_some_and(|t| !reached(tick, t)) {
                    return true;
                }
                self.pull_until = None;
                if !on || !ready {
                    return false;
                }
                let hold = self.trigger_rng.ticks(2, 3);
                let gap = self.trigger_rng.ticks(1, 3);
                self.pull_until = Some(tick.wrapping_add(hold));
                self.next_pull = Some(
                    tick.wrapping_add(
                        every
                            .saturating_add(self.trigger_rng.ticks(0, 3))
                            .max(hold + gap),
                    ),
                );
                true
            }
        }
    }

    /// Sets the movement's signals for this tick.
    pub fn signal(&mut self, arms: Option<&Arms>, kit: Kit, recovering: bool, enemy: Option<f32>) {
        self.band = match (kit, arms) {
            (Kit::Melee, _) | (_, None) => kit.band(),
            (_, Some(a)) => a.band(),
        };
        let exposed = enemy.is_some_and(|d| d < self.band.1.max(crate::arms::MELEE_CHASE));
        self.wants_cover = recovering || (exposed && matches!(kit, Kit::Reload | Kit::Switch));
    }
}

/// Whether a grenade at `target` is safe for the team: no teammate (of
/// `team`, in a team game) within [`FRIENDLY_SPLASH`] of it.
pub fn grenade_safe(target: Vec3, others: &[Seen], teams: bool, team: i16) -> bool {
    !teams
        || !others
            .iter()
            .any(|o| o.team == team && len(sub(o.position, target)) < FRIENDLY_SPLASH)
}

/// Where to look while walking `walk` from `at` toward `last`, where an
/// enemy went out of view: at it, at chest height, when it is within
/// [`PRE_AIM_RANGE`] and not more than [`PRE_AIM_CONE`] off the walk (it
/// does not walk backwards to pre-aim).
pub fn pre_aim(at: Vec3, last: Vec3, walk: Option<[f32; 2]>) -> Option<Vec3> {
    let to = sub(last, at);
    let flat = (to[0] * to[0] + to[1] * to[1]).sqrt();
    if !(1.0..=PRE_AIM_RANGE).contains(&flat) {
        return None;
    }
    if let Some(w) = walk
        && w[0].abs() + w[1].abs() > 1e-3
        && angle_between([w[0], w[1], 0.0], [to[0], to[1], 0.0]) > PRE_AIM_CONE
    {
        return None;
    }
    Some([last[0], last[1], last[2] + CHEST])
}

#[cfg(test)]
mod tests;
