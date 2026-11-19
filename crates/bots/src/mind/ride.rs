//! The mind with vehicles (docs/game/bots.md, section 3.9): the glue
//! between [`Mind::step`] and the pure planners in [`crate::vehicle`].
//!
//! - In a vehicle, [`Mind::ride`] makes the whole pad: the driver follows
//!   a route on the nav mesh to where it is going, steering by the camera,
//!   running infantry over and shooting with the vehicle's guns; a gunner
//!   or passenger shoots; and either gets out when the vehicle is about to
//!   blow up, flipped, stuck, stranded, or where it was going needs feet.
//! - On foot, [`Mind::mount_plan`] picks a vehicle to take or board and
//!   [`Mind::mount_step`] walks there and holds X by the seat.
//! - In a fight, [`Mind::lock_on`] holds a rocket launcher on a vehicle
//!   until the lock.
//!
//! A child of `mind`, so it uses the mind's own walking (`Mind::go`), the
//! hand that turns the right stick (`mind::look`), the thumb that eases
//! the left (`mind::pad::Feet`), and the trigger (`tactics`). Getting in
//! and out are long holds of X, not taps, so they are pressed here rather
//! than through `mind::pad::Buttons` (whose taps last 2 to 4 ticks).

use super::*;
use crate::arms::Gun;
use crate::brain::Me;
use crate::tactics::Pull;
use crate::vehicle::{
    Class, Going, SeatRole, Sight, Wheels, drive, flight,
    lock::{self, Locking},
    plan::{self, Why},
};

/// The X hold for getting out: held this many ticks of every
/// [`EXIT_EVERY`], so a press and a hold both work.
pub const EXIT_HOLD: u32 = 15;
pub const EXIT_EVERY: u32 = 20;
/// A boarder walks straight at the vehicle (no path) within this.
pub const BOARD_STRAIGHT: f32 = 4.0;
/// Takes a vehicle on the way to an objective only when that is further
/// than this (XY).
pub const OBJECTIVE_FAR: f32 = 20.0;
/// A route to a goal that moved this much is planned again.
pub const ROUTE_STALE: f32 = 3.0;

impl Mind {
    /// What the bot knows about vehicles this frame
    /// ([`crate::vehicle::read::sight`]). Without it the bot plays on foot.
    pub fn see_vehicles(&mut self, sight: Sight) {
        self.vehicles.sight = sight;
    }

    /// In a vehicle: the whole of this frame's pad. On foot: `None`, and
    /// the mind goes on as usual.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ride(
        &mut self,
        p: &Perception,
        map: Option<&MapData>,
        me: &Me,
        chosen: Option<(Seen, u32)>,
        seen: &[Seen],
        goal: Option<Goal>,
        dt: u32,
    ) -> Option<HostPadState> {
        let tick = p.tick;
        // `step` returns the last pad for a repeated tick: this is a new one.
        self.vehicles.update(tick);
        if chosen.is_none() {
            self.vehicles.lock.reset();
        }
        let riding = self.vehicles.sight.riding?;
        // This pad is the whole pad and returns before the glitch gate.
        self.drop_glitches();
        let sight = &self.vehicles.sight;
        let mut pad = HostPadState::default();
        let Some(part) = sight.get(riding.vehicle) else {
            // Its seat is in a vehicle that did not read: sit still.
            return Some(pad);
        };
        let root = sight.root(part).clone();
        let part = part.clone();
        let speed = self.vehicles.speed(root.id);
        let driver = riding.role == SeatRole::Driver;
        let wheels = Wheels::of(self.skill);
        let aim = self.skill.aim();
        let style = Style::of(self.skill);
        let mut spec = root.spec();
        for mounted in sight.parts(root.id).filter(|v| v.id != root.id) {
            if let Some([_, height]) = mounted.hull {
                spec.height = spec
                    .height
                    .max(mounted.position[2] - root.position[2] + height);
            }
        }
        self.doing = if driver {
            Doing::Drive(root.id)
        } else {
            Doing::Ride(root.id)
        };
        // Nothing of walking carries on in a vehicle.
        self.follower = None;
        self.take = None;
        self.stuck.reset();
        self.moves.calm();
        self.combo.reset();
        self.grenade.reset();

        let reacted = chosen.filter(|&(_, ready)| reached(p.tick, ready));
        if driver && root.class == Class::Fighter {
            return Some(self.fly(p, map, me, &root, reacted, goal, dt));
        }
        let gunner = riding.role == SeatRole::Gunner;
        let stationary = root.class == Class::Turret;
        let eye = up(if gunner { part.position } else { me.position }, EYE);
        let aim_at = angles(me.aim);
        let facing = crate::vehicle::flat(root.forward);
        let mut look: Option<Vec3> = None;
        let mut aim_error: Option<[f32; 2]> = None;
        let mut go: Option<[f32; 2]> = None;
        // The fight from the seat.
        if reacted.is_none() {
            self.hand.lose_target();
            self.fight.stop_trigger(p.tick);
        }
        if let Some((o, _)) = reacted {
            let target = up(o.position, CHEST);
            let dist = len(sub(target, eye));
            let vel = self.tracks.get(&o.id).map_or([0.0; 3], |t| t.vel);
            let held = me.arms.and_then(|a| a.held);
            // A passenger shoots its own gun; the driver and a gunner the
            // vehicle's.
            let (pull, in_range, speed_of_shot) = if riding.role == SeatRole::Passenger {
                let shot = Shot::of(held, me.arms.is_some(), &style, aim.burst, SHOT_SPEED);
                (shot.pull, shot.in_range(dist) && shot.ready, shot.speed)
            } else {
                (Pull::Hold(aim.burst), dist < FIRE_RANGE, SHOT_SPEED)
            };
            let led = add(target, scale(vel, aim.lead * dist / speed_of_shot));
            // The hand (mind::look): flick, lag and drift as on foot.
            aim_error = Some(self.hand.track(
                self.skill,
                o.id,
                tick,
                aim_at,
                [sub(target, eye), sub(led, eye)],
                self.decisions.behavior().aim_noise_extra(),
            ));
            let off = angle_between(sub(led, eye), me.aim);
            let cone = aim.fire_cone + (0.4 / dist.max(0.5)).min(0.3);
            let guns = !driver || spec.driver_guns;
            if guns && in_range {
                if self.fight.trigger(p.tick, pull, off < cone) {
                    pad.right_trigger = 1.0;
                }
            } else {
                self.fight.stop_trigger(p.tick);
            }
            // On foot and in front: run them over.
            if driver
                && sight.ridden_by(o.id).is_none()
                && drive::splatter(
                    wheels.splatter && spec.splatter,
                    root.position,
                    facing,
                    o.position,
                    drive::SPLATTER_RANGE,
                )
                && map.is_none_or(|m| drive::clear(m, spec, root.position, o.position))
            {
                let to = sub(o.position, root.position);
                let f = (to[0] * to[0] + to[1] * to[1]).sqrt().max(1e-6);
                go = Some([to[0] / f, to[1] / f]);
            }
        }

        // Where the driver goes: the objective, the fight, where the enemy
        // was, else somewhere to roam.
        let mut there = matches!(goal, Some(Goal::Carry(_) | Goal::Keep));
        let mut stuck = false;
        let mut throttle = 0.0;
        if driver {
            let dest = match goal {
                Some(Goal::Go(at) | Goal::Hunt(_, at) | Goal::Escort(at) | Goal::Hold(at, _)) => {
                    Some(at)
                }
                _ => None,
            }
            .or(chosen.map(|(o, _)| o.position))
            .or(self.last_seen.map(|(_, at, _)| at));
            if let Some(Goal::Go(at)) = goal {
                there |= dist_xy(root.position, at) < drive::ON_FOOT;
            }
            let dest = match dest {
                Some(d) => Some(d),
                None => {
                    if self.goal.is_none()
                        || self.vehicles.route.as_ref().is_none_or(drive::Route::done)
                    {
                        self.goal = self.roam_goal(root.position, tick, map, &p.pickups);
                        self.vehicles.route = None;
                    }
                    self.goal
                }
            };
            if go.is_none()
                && let Some(dest) = dest
            {
                let stale = self
                    .vehicles
                    .route
                    .as_ref()
                    .is_none_or(|r| len(sub(r.goal, dest)) > ROUTE_STALE);
                if stale {
                    self.vehicles.route = match map {
                        Some(m) => m
                            .nav
                            .vehicle_path(
                                root.position,
                                dest,
                                spec.half_width,
                                spec.height,
                                &m.walls,
                            )
                            .map(|path| drive::Route::new(path, dest)),
                        None => Some(drive::Route::straight(dest)),
                    };
                    if self.vehicles.route.is_none() {
                        there |= matches!(goal, Some(Goal::Go(_)));
                        if chosen.is_none() && goal.is_none() {
                            self.goal = None;
                        }
                    }
                }
                let carrot = self.vehicles.route.as_mut().and_then(|r| match map {
                    Some(m) => {
                        r.carrot_clear(root.position, |to| drive::clear(m, spec, root.position, to))
                    }
                    None => r.carrot(root.position),
                });
                match carrot {
                    Some(c) => {
                        let to = sub(c, root.position);
                        let f = (to[0] * to[0] + to[1] * to[1]).sqrt().max(1e-6);
                        go = Some([to[0] / f, to[1] / f]);
                        if look.is_none() {
                            look = Some([c[0], c[1], eye[2]]);
                        }
                    }
                    None => {
                        // There: a roam goal is done; anything else, park.
                        if self.last_seen.is_none() && goal.is_none() {
                            self.goal = None;
                        }
                    }
                }
            }
            if let Some(d) = go {
                let off = wrap(d[1].atan2(d[0]) - aim_at.0);
                throttle = drive::throttle(off);
            }
            let trying = throttle > 0.3 || self.vehicles.route.as_ref().is_some_and(|r| !r.done());
            match self.vehicles.unstick.step(tick, speed, trying) {
                drive::Unstuck::Drive => {}
                drive::Unstuck::Reverse => {
                    // Back up the way it came: the stick straight back.
                    let back = [
                        root.position[0] - facing[0],
                        root.position[1] - facing[1],
                        root.position[2],
                    ];
                    if map.is_none_or(|m| drive::clear(m, spec, root.position, back)) {
                        go = Some([-facing[0], -facing[1]]);
                        throttle = 1.0;
                    } else {
                        go = None;
                    }
                }
                drive::Unstuck::GiveUp => stuck = true,
            }
        } else if look.is_none() {
            // A gun with nothing to shoot: where the enemy was, else ahead.
            look = Some(match self.last_seen {
                Some((_, at, _)) => up(at, CHEST),
                None => [eye[0] + facing[0] * 10.0, eye[1] + facing[1] * 10.0, eye[2]],
            });
        }

        // Getting out.
        if self.vehicles.leaving.is_none() {
            self.vehicles.leaving = drive::leave(&drive::Ride {
                health: root.health,
                flipped: self.vehicles.flipped,
                driverless: if stationary {
                    0
                } else {
                    self.vehicles.driverless
                },
                driver,
                stuck,
                there,
            });
            if self.vehicles.leaving.is_none()
                && gunner
                && plan::flanked(part.position, me.aim, seen.iter().map(|o| o.position))
            {
                self.vehicles.leaving = Some(drive::Leave::Flanked);
            }
        }
        if self.vehicles.leaving.is_some() {
            if tick % EXIT_EVERY < EXIT_HOLD {
                pad.buttons.set(RELOAD, true);
            }
            go = None;
            pad.right_trigger = 0.0;
            self.feet.stop();
        }

        // The right stick through the hand (mind::look): onto the target it
        // tracks, else toward what it looks at. The left stick through the
        // thumb (mind::pad::Feet), eased, where to go relative to the aim:
        // the vehicle turns to go there.
        if aim_error.is_none() {
            aim_error = look.map(|l| angle_error(aim_at, angles(sub(l, eye))));
        }
        pad.right_stick = self.hand.steer(self.skill, aim_error, dt);
        let [x, y] = self.feet.step(
            go.map(|d| [d[0] * throttle, d[1] * throttle]),
            dt,
            1.0,
            false,
            |_| true,
        );
        let (s, c) = aim_at.0.sin_cos();
        pad.left_stick = (x * s - y * c, x * c + y * s);
        Some(pad)
    }

    #[allow(clippy::too_many_arguments)]
    fn fly(
        &mut self,
        p: &Perception,
        map: Option<&MapData>,
        me: &Me,
        root: &crate::vehicle::Vehicle,
        target: Option<(Seen, u32)>,
        goal: Option<Goal>,
        dt: u32,
    ) -> HostPadState {
        let at = root.position;
        let nearby_floor = map.and_then(|m| flight::floor(m, at));
        let objective = match goal {
            Some(Goal::Go(pos) | Goal::Carry(pos)) => Some(pos),
            _ => None,
        };
        let ground_goal = objective.filter(|pos| dist_xy(at, *pos) < 8.0);
        let emergency =
            root.health < drive::LEAVE_HEALTH || self.vehicles.flipped >= drive::FLIPPED_TICKS;
        let target = target.map(|(o, _)| up(o.position, CHEST));
        let cruise = target.or(self.last_seen.map(|(_, pos, _)| pos));
        if cruise.is_none() && self.goal.is_none_or(|g| dist_xy(g, at) < 4.0) {
            self.goal = self.roam_goal(at, p.tick, map, &p.pickups);
        }
        let default_goal = [
            at[0] + root.forward[0] * 12.0,
            at[1] + root.forward[1] * 12.0,
            nearby_floor.unwrap_or(at[2] - 4.5),
        ];
        let destination = ground_goal
            .or_else(|| emergency.then(|| [at[0], at[1], nearby_floor.unwrap_or(at[2])]))
            .or(objective)
            .or(cruise)
            .or(self.goal)
            .unwrap_or(default_goal);
        let pilot = self.vehicles.flight.step(
            &flight::FlightIn {
                tick: p.tick,
                at,
                aim: me.aim,
                forward: root.forward,
                destination,
                target,
                health: root.health,
                has_bomb: root.alternate_fire,
                land: emergency || ground_goal.is_some(),
            },
            map,
        );
        let mut pad = HostPadState::default();
        // Damage may force a bailout even over the void. Routine
        // dismounts wait until close to the floor.
        if pilot.landed || emergency && (root.health < 0.08 || nearby_floor.is_none()) {
            self.vehicles.leaving.get_or_insert(if emergency {
                drive::Leave::Wrecked
            } else {
                drive::Leave::There
            });
        }
        if self.vehicles.leaving.is_some() {
            pad.buttons.set(RELOAD, p.tick % EXIT_EVERY < EXIT_HOLD);
        } else {
            let eye = up(me.position, EYE);
            pad.right_stick = self.hand.steer(
                self.skill,
                Some(angle_error(angles(me.aim), angles(sub(pilot.look, eye)))),
                dt,
            );
            pad.left_stick = (pilot.stick[0], pilot.stick[1]);
            pad.left_trigger = if pilot.boost { 1.0 } else { 0.0 };
            pad.right_trigger = if pilot.cannon { 1.0 } else { 0.0 };
            // Default Xbox pad: LT boost, A + lateral stick trick, B
            // alternate fire in a fighter. The engine gates availability.
            pad.buttons.set(JUMP, pilot.flip);
            pad.buttons.set(MELEE, pilot.bomb);
        }
        pad
    }

    /// On foot: the vehicle (object id) to go for now, if any. One already
    /// set out for is kept while it is still to be had; an enemy vehicle
    /// close and slow is boarded, in a fight or not; a free one is taken
    /// only when calm, not carrying, not going for a weapon, and not near
    /// the objective it is playing.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mount_plan(
        &mut self,
        p: &Perception,
        map: Option<&MapData>,
        me: &Me,
        known: Option<Vec3>,
        calm: bool,
        goal: Option<Goal>,
    ) -> Option<u32> {
        let tick = p.tick;
        let carrying = matches!(goal, Some(Goal::Carry(_) | Goal::Keep));
        if carrying || self.vehicles.sight.riding.is_some() {
            self.vehicles.going = None;
            return None;
        }
        let wheels = Wheels::of(self.skill);
        let foot = plan::Foot {
            at: me.position,
            team: me.team,
            teams: p.teams,
        };
        let mem = &self.vehicles;
        let speed = |id| mem.speed(id);
        // Still to be had?
        if let Some(g) = mem.going {
            let timeout = match g.pick.why {
                Why::Take => plan::MOUNT_TIMEOUT,
                Why::Board => plan::BOARD_TIMEOUT,
            };
            let keep = tick.wrapping_sub(g.since) < timeout
                && match g.pick.why {
                    Why::Take => mem.sight.get(g.pick.vehicle).is_some_and(|v| {
                        v.enterable(g.pick.role)
                            && mem
                                .sight
                                .crew(g.pick.root)
                                .all(|(r, _)| p.teams && r.team == me.team)
                    }),
                    Why::Board => plan::board(&foot, &mem.sight, &wheels, speed, |_| false)
                        .is_some_and(|b| b.root == g.pick.root),
                };
            if keep {
                return Some(g.pick.vehicle);
            }
            self.vehicles.shun(g.pick.root, tick);
            self.vehicles.going = None;
            return None;
        }
        let mem = &self.vehicles;
        let speed = |id| mem.speed(id);
        let shunned = |id| mem.shunned(id, tick);
        let contested = known.or_else(|| match goal {
            Some(Goal::Hold(at, _) | Goal::Hunt(_, at)) => Some(at),
            _ => None,
        });
        let pick = plan::board(&foot, &mem.sight, &wheels, speed, shunned)
            .or_else(|| {
                (wheels.gunner || !calm)
                    .then_some(contested)
                    .flatten()
                    .and_then(|at| {
                        plan::take_turret(&foot, &mem.sight, at, map, |id| {
                            shunned(id) || speed(id) > 1.5
                        })
                    })
            })
            .or_else(|| {
                let far_goal = match goal {
                    None => true,
                    Some(Goal::Go(at) | Goal::Hunt(_, at)) => {
                        dist_xy(me.position, at) > OBJECTIVE_FAR
                    }
                    _ => false,
                };
                if !calm || matches!(self.kit, Kit::Seek(_)) || !far_goal {
                    return None;
                }
                let friends: Vec<Vec3> = p
                    .others
                    .iter()
                    .filter(|o| p.teams && o.team == me.team)
                    .filter(|o| mem.sight.ridden_by(o.id).is_none())
                    .map(|o| o.position)
                    .collect();
                // A mounted gun is taken when it covers somewhere contested.
                let mut wheels = wheels;
                wheels.gunner = false;
                plan::take(&foot, &mem.sight, &friends, &wheels, speed, shunned)
            })?;
        self.vehicles.going = Some(Going {
            pick,
            since: tick,
            side: 0,
            holding: None,
        });
        Some(pick.vehicle)
    }

    /// On foot, going for vehicle `id` ([`Mind::mount_plan`]): walks to the
    /// point beside the seat and holds X there; a boarder runs straight at
    /// a vehicle close by. The walk, and where to look.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mount_step(
        &mut self,
        at: Vec3,
        aim: Vec3,
        tick: u32,
        map: Option<&MapData>,
        pad: &mut HostPadState,
        bodies: &[Vec3],
    ) -> (Option<Walk>, Option<Vec3>) {
        let Some(g) = self.vehicles.going else {
            return (None, None);
        };
        let Some(part) = self.vehicles.sight.get(g.pick.vehicle).cloned() else {
            self.vehicles.going = None;
            return (None, None);
        };
        let selected = plan::approach(&part, g.pick, at, g.side, map)
            .or_else(|| plan::approach(&part, g.pick, at, 1, map));
        let Some((point, entry)) = selected else {
            self.vehicles.going = None;
            self.vehicles.shun(g.pick.root, tick);
            return (None, None);
        };
        if dist_xy(at, point) < plan::ENTER_RANGE && plan::in_entry(&part, entry, at) {
            self.follower = None;
            let Some(going) = self.vehicles.going.as_mut() else {
                return (None, None);
            };
            let since = *going.holding.get_or_insert(tick);
            if tick.wrapping_sub(since) < plan::ENTER_TICKS {
                if plan::looking_at_entry(entry, at, aim) {
                    pad.buttons.set(RELOAD, true);
                }
            } else if g.side == 0 {
                going.side = 1;
                going.holding = None;
            } else {
                self.vehicles.going = None;
                self.vehicles.shun(g.pick.root, tick);
            }
            return (None, Some(entry.at));
        }
        if let Some(g) = self.vehicles.going.as_mut() {
            g.holding = None;
        }
        let d = dist_xy(at, point);
        let walk = if (g.pick.why == Why::Board && d < BOARD_STRAIGHT) || d <= crate::nav::REACHED {
            let dx = (point[0] - at[0]) / d.max(1e-6);
            let dy = (point[1] - at[1]) / d.max(1e-6);
            self.follower = None;
            Some(Walk::straight([dx, dy]))
        } else {
            self.go(at, point, tick, map, pad, bodies)
        };
        if self.gave_up || walk.is_none() && d > 0.5 {
            self.vehicles.going = None;
            self.vehicles.shun(g.pick.root, tick);
        }
        (walk, (g.pick.why == Why::Board).then_some(entry.at))
    }

    /// In a fight with `held` in hand: when it is a rocket launcher and the
    /// target `o` rides a vehicle (and the skill waits for the lock), the
    /// point to aim at (the vehicle's middle, no lead: a locked rocket
    /// follows it) and whether it is locked yet. `None` otherwise: shoot
    /// as at anyone.
    pub(super) fn lock_on(
        &mut self,
        o: &Seen,
        held: Option<Gun>,
        eye: Vec3,
        aim: Vec3,
        tick: u32,
        cone: f32,
    ) -> Option<(Vec3, bool)> {
        let k = Wheels::of(self.skill).lock;
        let vehicle = self
            .vehicles
            .sight
            .ridden_by(o.id)
            .map(|(v, _)| (v.id, v.position));
        let Some((id, at)) =
            vehicle.filter(|_| k.waits && held.is_some_and(|g| lock::locks_on(&g)))
        else {
            self.vehicles.lock.reset();
            return None;
        };
        let middle = up(at, lock::MIDDLE);
        let on = lock::on_target(eye, aim, middle, cone);
        let locked = self.vehicles.lock.step(id, on, tick, &k) == Locking::Locked;
        Some((middle, locked))
    }
}

#[cfg(test)]
mod tests;
