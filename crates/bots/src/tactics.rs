//! Combat decisions (docs/game/bots.md, section 3.4, "Built: combat
//! boost"): whom to shoot, where on them, how to work the trigger of the
//! weapon in hand, when a grenade is safe, where to pre-aim, and the
//! signals the movement reads (the range to keep, wanting cover).
//!
//! Pure and deterministic: [`Tactics`] is the state the mind keeps, and
//! every function takes the tick. Time is the game's tick (30 a second).

use crate::arms::{Arms, Gun, Kit, Ranged, Role};
use crate::brain::{Seen, Vec3};
use crate::mind::Skill;

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
/// In a fight already (another enemy seen within [`GRACE`]), a new
/// target takes this fraction of the reaction time.
pub const ALERT_REACTION: f32 = 0.5;
/// Enemies remembered at most this long (ticks) for [`GRACE`].
const FORGET: u32 = 4 * GRACE;

/// A pull of a once-per-pull trigger lasts this many ticks...
pub const PULL_TICKS: u32 = 2;
/// ...and pulls come at least this many ticks apart (so it lets go).
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
    /// Picks the weapon by range ([`crate::arms::Situation::by_range`]).
    pub by_range: bool,
    /// Pre-aims at where an enemy went out of view.
    pub pre_aim: bool,
    /// Holds a grenade when a teammate is near the target.
    pub careful_grenades: bool,
}

impl Style {
    pub fn of(skill: Skill) -> Style {
        match skill {
            Skill::Easy => Style {
                head: false,
                feet: false,
                pace: 12,
                by_range: false,
                pre_aim: false,
                careful_grenades: false,
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
    /// Pulled for [`PULL_TICKS`] every this many ticks: a once-per-pull
    /// trigger fires once per pull, so holding it fires one shot.
    Tap(u32),
    /// Not at all: a blade swings with melee.
    Never,
}

/// What the held weapon means for shooting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub role: Role,
    pub pull: Pull,
    /// The projectile's speed for lead (world units a second).
    pub speed: f32,
    /// Shoots at targets this far, no nearer or further.
    pub range: (f32, f32),
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
                    (30.0 / p.rate).round() as u32
                } else {
                    PULL_DEFAULT
                };
                Pull::Tap(every.max(PULL_MIN) + style.pace)
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
            range: role.fire_range(),
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

/// The fight's state, kept by the mind.
#[derive(Clone, Debug, Default)]
pub struct Tactics {
    /// Enemies noticed: id, the tick its reaction counts from, the tick
    /// last seen.
    known: Vec<(u32, u32, u32)>,
    /// The next tick a pull may start, and the tick the current one ends.
    next_pull: u32,
    pull_until: u32,
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
    seen.iter()
        .map(|o| {
            let to = sub([o.position[0], o.position[1], o.position[2] + CHEST], eye);
            let mut cost = len(to) + TURN_COST * angle_between(to, aim);
            if current == Some(o.id) {
                cost *= STICK;
            }
            (cost, *o)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, o)| o)
}

impl Tactics {
    /// Notes who is in view this tick. A newcomer's reaction counts from
    /// now, or from half the reaction ago when the bot is already in a
    /// fight; one back in view within [`GRACE`] keeps its old count.
    pub fn see(&mut self, seen: &[Seen], tick: u32, reaction: u32) {
        let alert = self
            .known
            .iter()
            .any(|&(_, _, last)| tick.wrapping_sub(last) <= GRACE);
        self.known
            .retain(|&(_, _, last)| tick.wrapping_sub(last) <= FORGET);
        let t0 = if alert {
            tick.saturating_sub((reaction as f32 * ALERT_REACTION) as u32)
        } else {
            tick
        };
        for o in seen {
            match self.known.iter().position(|k| k.0 == o.id) {
                Some(i) if tick.wrapping_sub(self.known[i].2) <= GRACE => self.known[i].2 = tick,
                Some(i) => self.known[i] = (o.id, t0, tick),
                None => self.known.push((o.id, t0, tick)),
            }
        }
    }

    /// The tick `id`'s reaction counts from (`tick` if not noticed).
    pub fn since(&self, id: u32, tick: u32) -> u32 {
        self.known.iter().find(|k| k.0 == id).map_or(tick, |k| k.1)
    }

    /// Forgets everyone (the bot died).
    pub fn reset(&mut self) {
        self.known.clear();
        self.next_pull = 0;
        self.pull_until = 0;
        self.wants_cover = false;
    }

    /// Whether the trigger is down this tick, wanting to fire (`want`):
    /// held in bursts counted from `t0`, or pulled and let go.
    pub fn trigger(&mut self, tick: u32, t0: u32, pull: Pull, want: bool) -> bool {
        match pull {
            Pull::Never => false,
            Pull::Hold(burst) => {
                want && burst.is_none_or(|(on, off)| tick.wrapping_sub(t0) % (on + off) < on)
            }
            Pull::Tap(every) => {
                if tick < self.pull_until {
                    // A pull under way finishes: letting go early would
                    // waste it.
                    return true;
                }
                if want && tick >= self.next_pull {
                    self.pull_until = tick + PULL_TICKS;
                    self.next_pull = tick + every;
                    return true;
                }
                false
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
