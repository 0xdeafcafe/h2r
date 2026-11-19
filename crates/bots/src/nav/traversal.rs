use super::{Kind, Steer, V3, Waypoint, dist_xy};

#[derive(Clone, Copy, Debug)]
pub(super) struct Transit {
    start: u64,
    armed: Option<u64>,
    approach: [f32; 2],
    airborne: Option<u64>,
    last_distance: f32,
    best_distance: f32,
    progress: u64,
}

impl Transit {
    pub(super) fn armed(self) -> bool {
        self.armed.is_some()
    }
}

fn toward(at: V3, to: V3) -> [f32; 2] {
    let d = dist_xy(at, to).max(1e-6);
    [(to[0] - at[0]) / d, (to[1] - at[1]) / d]
}

/// Finish only after arriving at the landing height and position. Until
/// then the follower keeps control even if a lift carries it off the mesh.
pub(super) fn follow(
    entry: Waypoint,
    exit: V3,
    at: V3,
    frame: u64,
    state: &mut Option<Transit>,
) -> Option<Steer> {
    let d = dist_xy(at, entry.at);
    let near = if entry.kind == Kind::CrouchJump {
        0.23
    } else {
        0.15
    };
    let transit = state.get_or_insert(Transit {
        start: frame,
        armed: None,
        approach: [0.0; 2],
        airborne: None,
        last_distance: d,
        best_distance: d,
        progress: frame,
    });
    let mut steer = Steer {
        traversing: true,
        ..Steer::default()
    };
    let landed = if entry.kind == Kind::CrouchJump {
        transit.airborne.is_some() && dist_xy(at, exit) < 0.3 && (at[2] - exit[2]).abs() < 0.18
    } else {
        dist_xy(at, exit) < 0.55 && (at[2] - exit[2]).abs() < 0.45
    };
    if landed
        && (transit.armed.is_some_and(|t| t != frame)
            || entry.kind == Kind::Teleporter && frame != transit.start)
    {
        return None;
    }
    let just_teleported = entry.kind == Kind::Teleporter && transit.last_distance < 0.6 && d > 2.0;
    transit.last_distance = d;
    if just_teleported {
        return Some(Steer {
            replan: true,
            ..steer
        });
    }
    if transit.armed.is_none() {
        if d > near {
            if d < transit.best_distance - 0.035 {
                transit.best_distance = d;
                transit.progress = frame;
            }
            if frame.saturating_sub(transit.progress) > 45 {
                steer.replan = true;
                return Some(steer);
            }
            transit.approach = toward(at, entry.at);
            steer.dir = transit.approach;
            return Some(steer);
        }
        transit.armed = Some(frame);
    }
    let elapsed = frame.saturating_sub(transit.armed.unwrap_or(frame));
    match entry.kind {
        Kind::Teleporter => {
            // Walk into the trigger. Stop before running out of its far
            // side. A different receiver (or a blocked one) needs a replan.
            if elapsed < 8 {
                steer.dir = transit.approach;
            }
            steer.replan = dist_xy(at, entry.at) > 2.0 || elapsed >= 30;
        }
        Kind::Lift => {
            steer.replan = elapsed >= 100;
            // Stay on the pad during pickup. Once airborne (or already
            // carried horizontally) steer for the landing, including down.
            if (at[2] - entry.at[2]).abs() > 0.35 || d > 0.5 {
                steer.dir = toward(at, exit);
            }
        }
        Kind::Elevator => {
            steer.replan = elapsed >= 360;
            // The platform carries the rider; defer lateral input until
            // the elevator is level with the landing.
            if (at[2] - exit[2]).abs() < 0.55 {
                steer.dir = toward(at, exit);
            }
        }
        Kind::CrouchJump => {
            steer.dir = toward(at, exit);
            if at[2] > entry.at[2] + 0.07 {
                transit.airborne.get_or_insert(frame);
            }
            steer.jump = elapsed < 6 && transit.airborne.is_none();
            steer.crouch = transit
                .airborne
                .is_some_and(|t| frame.saturating_sub(t) >= 2);
            steer.replan = elapsed >= 45;
        }
        _ => unreachable!("only paired waypoints traverse"),
    }
    Some(steer)
}
