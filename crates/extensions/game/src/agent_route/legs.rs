use agent_proto::route::{Facing, Leg, LegStatus, Sample};
use bots::cache::MapData;
use bots::mind::RouteMotion;
use bots::nav::{RouteJump, RouteWalk, V3, plan_route_jump};

use super::Biped;

const FACE_TOLERANCE: f32 = 2.0_f32.to_radians();
const LAND_TOLERANCE: f32 = 0.15;
const STILL_SPEED: f32 = 0.06;
const STANDING: f32 = 0.05;
const CROUCH_PACE: f32 = 0.42;
const STALL_TICKS: u8 = 5;

pub(super) enum Step {
    Pad(RouteMotion),
    Done,
}

impl Step {
    fn neutral() -> Self {
        Self::Pad(RouteMotion::default())
    }
}

pub(super) struct LegRun {
    leg: Leg,
    walk: Option<RouteWalk>,
    face_settled: bool,
    crouch: Crouch,
    jump: Option<(RouteJump, Flight)>,
}

impl LegRun {
    pub fn new(leg: Leg) -> Self {
        Self {
            leg,
            walk: None,
            face_settled: false,
            crouch: Crouch::Enter {
                heading: None,
                stalled: 0,
            },
            jump: None,
        }
    }

    pub fn step(
        &mut self,
        nav: &MapData,
        b: Biped,
        tick: u32,
        status: &mut LegStatus,
    ) -> Result<Step, &'static str> {
        match self.leg {
            Leg::Wait { wait } => Ok(if status.ticks >= wait {
                Step::Done
            } else {
                Step::neutral()
            }),
            Leg::Face { face } => {
                let yaw = match face {
                    Facing::Yaw(yaw) => radians(yaw),
                    Facing::Point(point) => {
                        let dir = direction(b.position, point);
                        if dir == [0.0; 2] {
                            b.yaw()
                        } else {
                            dir[1].atan2(dir[0])
                        }
                    }
                };
                if facing(b, yaw) {
                    if self.face_settled {
                        return Ok(Step::Done);
                    }
                    self.face_settled = true;
                    return Ok(Step::neutral());
                }
                self.face_settled = false;
                Ok(Step::Pad(turn(yaw)))
            }
            Leg::Move {
                move_to,
                crouch,
                tolerance,
                speed,
            } => {
                if !crouch && (b.crouch_request || b.crouch > STANDING) {
                    return Ok(Step::neutral());
                }
                walk(
                    &mut self.walk,
                    nav,
                    b,
                    tick,
                    move_to,
                    crouch,
                    tolerance,
                    speed,
                )
            }
            Leg::Crouch {
                crouch_into,
                hold_ticks,
                exit_face,
            } => self
                .crouch
                .step(b, status.ticks, crouch_into, hold_ticks, radians(exit_face)),
            Leg::Jump { jump } => {
                if self.jump.is_none() {
                    let plan = plan_route_jump(nav, jump.from, jump.land).ok_or("no_jump_arc")?;
                    self.jump = Some((plan, Flight::Approach));
                }
                let (plan, flight) = self.jump.as_mut().unwrap();
                flight.step(*plan, &mut self.walk, nav, b, tick, status, jump.land)
            }
        }
    }
}

fn radians(degrees: f32) -> f32 {
    degrees.rem_euclid(360.0).to_radians()
}

fn facing(b: Biped, yaw: f32) -> bool {
    let error = yaw - b.yaw();
    error.sin().atan2(error.cos()).abs() <= FACE_TOLERANCE
}

fn turn(yaw: f32) -> RouteMotion {
    RouteMotion {
        yaw: Some(yaw),
        ..RouteMotion::default()
    }
}

fn forward(yaw: f32, speed: f32, crouch: bool) -> RouteMotion {
    RouteMotion {
        dir: [yaw.cos(), yaw.sin()],
        speed,
        crouch,
        ..RouteMotion::default()
    }
}

fn direction(at: V3, to: [f32; 2]) -> [f32; 2] {
    let dx = to[0] - at[0];
    let dy = to[1] - at[1];
    let len = dx.hypot(dy).max(1e-6);
    [dx / len, dy / len]
}

fn distance(a: V3, b: V3) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

fn walk(
    follower: &mut Option<RouteWalk>,
    nav: &MapData,
    b: Biped,
    tick: u32,
    goal: V3,
    crouch: bool,
    tolerance: f32,
    speed: f32,
) -> Result<Step, &'static str> {
    if distance(b.position, goal) <= tolerance {
        return Ok(if !b.airborne && b.horizontal_speed() <= STILL_SPEED {
            Step::Done
        } else {
            Step::neutral()
        });
    }
    if follower.is_none() {
        *follower = Some(RouteWalk::new(nav, b.position, goal, crouch, tick)?);
    }
    Ok(follower
        .as_mut()
        .unwrap()
        .step(nav, b.position, tick, tolerance, speed)?
        .map_or_else(Step::neutral, Step::Pad))
}

enum Crouch {
    Enter { heading: Option<f32>, stalled: u8 },
    Release { heading: f32, at_tick: u32 },
    Exit,
}

impl Crouch {
    fn step(
        &mut self,
        b: Biped,
        ticks: u32,
        target: V3,
        hold_ticks: u32,
        exit: f32,
    ) -> Result<Step, &'static str> {
        match self {
            Self::Enter { heading, stalled } => {
                let yaw = *heading.get_or_insert_with(|| {
                    let dir = direction(b.position, [target[0], target[1]]);
                    if dir == [0.0; 2] {
                        b.yaw()
                    } else {
                        dir[1].atan2(dir[0])
                    }
                });
                let at_target = distance(b.position, target) <= 0.22;
                if at_target
                    && !b.airborne
                    && b.crouch_request
                    && b.crouch >= 0.9
                    && b.horizontal_speed() <= STILL_SPEED
                {
                    *stalled += 1;
                    if *stalled >= STALL_TICKS {
                        *self = Self::Release {
                            heading: yaw,
                            at_tick: ticks,
                        };
                        return Ok(Step::Pad(forward(yaw, CROUCH_PACE, false)));
                    }
                } else {
                    *stalled = 0;
                }
                Ok(Step::Pad(forward(yaw, CROUCH_PACE, true)))
            }
            Self::Release { heading, at_tick } => {
                // Verify the request remains set with the button up. Use at
                // least one released sample even when hold_ticks is zero.
                if !b.crouch_request {
                    return Err("crouch_not_forced");
                }
                if ticks - *at_tick >= hold_ticks.max(1) {
                    *self = Self::Exit;
                    return self.step(b, ticks, target, hold_ticks, exit);
                }
                Ok(Step::Pad(forward(*heading, CROUCH_PACE, false)))
            }
            Self::Exit => {
                if !b.crouch_request && b.crouch <= STANDING && facing(b, exit) {
                    return Ok(Step::Done);
                }
                let mut motion = forward(b.yaw(), CROUCH_PACE, false);
                motion.yaw = Some(exit);
                Ok(Step::Pad(motion))
            }
        }
    }
}

enum Flight {
    Approach,
    Align,
    Run,
    Launch { pressed: u32, previous: Sample },
    Air { previous: Sample },
}

impl Flight {
    fn step(
        &mut self,
        plan: RouteJump,
        follower: &mut Option<RouteWalk>,
        nav: &MapData,
        b: Biped,
        tick: u32,
        status: &mut LegStatus,
        requested_land: V3,
    ) -> Result<Step, &'static str> {
        let trace = status.jump.get_or_insert_with(Default::default);
        let yaw = if plan.dir == [0.0; 2] {
            b.yaw()
        } else {
            plan.dir[1].atan2(plan.dir[0])
        };
        match self {
            Self::Approach => {
                if b.crouch_request || b.crouch > STANDING {
                    return Ok(Step::neutral());
                }
                if let Step::Pad(motion) =
                    walk(follower, nav, b, tick, plan.run_up, false, 0.08, 1.0)?
                {
                    return Ok(Step::Pad(motion));
                }
                *self = Self::Align;
                Ok(Step::Pad(turn(yaw)))
            }
            Self::Align => {
                if facing(b, yaw) {
                    *self = Self::Run;
                    Ok(Step::neutral())
                } else {
                    Ok(Step::Pad(turn(yaw)))
                }
            }
            Self::Run => {
                let mut motion = plan.run_motion(b.position, b.velocity);
                if b.airborne || plan.missed_takeoff(b.position) {
                    return Err("missed_takeoff");
                }
                if plan.launch_ready(b.position, b.velocity, b.ticks_per_second) {
                    motion.jump = true;
                    *self = Self::Launch {
                        pressed: tick,
                        previous: b.sample(),
                    };
                }
                Ok(Step::Pad(motion))
            }
            Self::Launch { pressed, previous } => {
                let now = b.sample();
                if b.airborne && b.velocity[2] > 0.0 {
                    trace.takeoff = Some(Sample {
                        position: previous.position,
                        ..now
                    });
                    trace.apex = Some(now);
                    *self = Self::Air { previous: now };
                    return Ok(Step::Pad(plan.air_motion(b.position, b.velocity)));
                }
                let elapsed = tick.saturating_sub(*pressed);
                if elapsed >= 8 {
                    return Err("jump_did_not_take_off");
                }
                *previous = now;
                let mut motion = plan.run_motion(b.position, b.velocity);
                motion.jump = elapsed < bots::moves::JUMP_TICKS;
                Ok(Step::Pad(motion))
            }
            Self::Air { previous } => {
                let now = b.sample();
                // Ground contact can remove the in-speed or launch the biped
                // again on the same tick. Keep the last incoming velocity.
                let rebound = previous.velocity[2] < -0.25 && b.velocity[2] > 0.25;
                let caught =
                    b.supported && previous.velocity[2] < -0.25 && b.velocity[2].abs() < 0.05;
                if !b.airborne || rebound || caught {
                    let position = if rebound {
                        previous.position
                    } else {
                        now.position
                    };
                    trace.landing = Some(Sample {
                        position,
                        ..*previous
                    });
                    return if distance(position, requested_land) <= LAND_TOLERANCE {
                        Ok(Step::Done)
                    } else {
                        Err("missed_landing")
                    };
                }
                if trace
                    .apex
                    .is_none_or(|apex| now.position[2] > apex.position[2])
                {
                    trace.apex = Some(now);
                }
                *previous = now;
                Ok(Step::Pad(plan.air_motion(b.position, b.velocity)))
            }
        }
    }
}
