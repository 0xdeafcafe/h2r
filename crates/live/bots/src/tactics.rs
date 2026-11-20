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
    /// Held, in the skill's bursts (`None`: held throughout).
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
    /// ([`crate::weapons::reach`]), its role's refined by its aim assist.
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
    /// behaviour), with the skill's `burst` for a held trigger and the
    /// lead speed `default_speed` when the projectile is not known.
    pub fn of(
        held: Option<Gun>,
        read: bool,
        style: &Style,
        burst: Option<(u32, u32)>,
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
            _ => Pull::Hold(burst),
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
    holding: bool,
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
            holding: false,
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
        self.holding = false;
        self.wants_cover = false;
    }

    /// Break the pull when the target disappears or firing is impossible.
    pub(crate) fn stop_trigger(&mut self, tick: u32) {
        if self.pull_until.take().is_some() || self.holding {
            let release = tick.wrapping_add(2);
            if self.next_pull.is_none_or(|next| reached(release, next)) {
                self.next_pull = Some(release);
            }
        }
        self.holding = false;
    }

    /// Whether the trigger is down this tick. Tap lengths and recovery
    /// gaps vary. Burst starts are relative to when the bot lines up a shot.
    pub fn trigger(&mut self, tick: u32, pull: Pull, want: bool) -> bool {
        if self.pull != Some(pull) {
            let was_down = self.pull_until.is_some() || self.holding;
            self.pull = Some(pull);
            self.pull_until = None;
            self.holding = false;
            self.next_pull = was_down.then_some(tick.wrapping_add(2));
        }
        let ready = self.next_pull.is_none_or(|t| reached(tick, t));
        match pull {
            Pull::Never => false,
            Pull::Hold(None) => {
                self.holding = want && ready;
                self.holding
            }
            Pull::Tap(every) => {
                if self.pull_until.is_some_and(|t| !reached(tick, t)) {
                    return true;
                }
                self.pull_until = None;
                if !want || !ready {
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
            Pull::Hold(Some((on, off))) => {
                if self.pull_until.is_some_and(|t| !reached(tick, t)) && want {
                    return true;
                }
                self.pull_until = None;
                if !want || !ready {
                    return false;
                }
                let hold = self
                    .trigger_rng
                    .ticks(on.saturating_sub(2).max(1), on.saturating_add(2).max(1));
                let gap = self
                    .trigger_rng
                    .ticks(off.saturating_sub(2).max(1), off.saturating_add(2).max(1));
                self.pull_until = Some(tick.wrapping_add(hold));
                self.next_pull = Some(tick.wrapping_add(hold).wrapping_add(gap));
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
