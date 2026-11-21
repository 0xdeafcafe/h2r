//! Wire parameters and results for controller routes.

use serde::{Deserialize, Serialize};

pub const DEFAULT_TIMEOUT_TICKS: u32 = 1800;
pub const DEFAULT_TOLERANCE: f32 = 0.1;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteParams {
    #[serde(default)]
    pub player: u32,
    pub legs: Vec<Leg>,
    #[serde(default = "default_timeout")]
    pub timeout_ticks: u32,
}

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT_TICKS
}

fn default_tolerance() -> f32 {
    DEFAULT_TOLERANCE
}

fn full_speed() -> f32 {
    1.0
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortParams {
    #[serde(default)]
    pub player: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Leg {
    Move {
        move_to: [f32; 3],
        crouch: bool,
        #[serde(default = "default_tolerance")]
        tolerance: f32,
        #[serde(default = "full_speed")]
        speed: f32,
    },
    Face {
        face: Facing,
    },
    Jump {
        jump: Jump,
    },
    Crouch {
        crouch_into: [f32; 3],
        hold_ticks: u32,
        exit_face: f32,
    },
    Wait {
        wait: u32,
    },
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(untagged)]
pub enum Facing {
    Yaw(f32),
    Point([f32; 2]),
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jump {
    pub from: [f32; 3],
    pub land: [f32; 3],
}

impl Leg {
    pub fn check(&self) -> Result<(), &'static str> {
        let finite = |point: &[f32]| point.iter().all(|x| x.is_finite());
        match *self {
            Self::Move {
                move_to,
                tolerance,
                speed,
                ..
            } => {
                if !finite(&move_to) {
                    return Err("move_to must contain finite coordinates");
                }
                if !tolerance.is_finite() || tolerance <= 0.0 {
                    return Err("tolerance must be finite and positive");
                }
                if !(0.0..=1.0).contains(&speed) {
                    return Err("speed must be between 0 and 1");
                }
            }
            Self::Face {
                face: Facing::Yaw(yaw),
            } if !yaw.is_finite() => {
                return Err("face yaw must be finite");
            }
            Self::Face {
                face: Facing::Point(point),
            } if !finite(&point) => {
                return Err("face must contain finite coordinates");
            }
            Self::Jump { jump } if !finite(&jump.from) || !finite(&jump.land) => {
                return Err("jump must contain finite coordinates");
            }
            Self::Crouch {
                crouch_into,
                exit_face,
                ..
            } if !finite(&crouch_into) || !exit_face.is_finite() => {
                return Err("crouch_into and exit_face must contain finite coordinates and yaw");
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Running,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub player: u32,
    pub state: State,
    pub ticks: u32,
    pub leg: Option<usize>,
    pub legs: Vec<LegStatus>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LegStatus {
    pub ok: Option<bool>,
    pub ticks: u32,
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump: Option<JumpTrace>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct JumpTrace {
    pub takeoff: Option<Sample>,
    pub apex: Option<Sample>,
    pub landing: Option<Sample>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Sample {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub speed: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json};

    #[test]
    fn routes_decode_defaults_and_reject_ambiguous_legs() {
        let route: RouteParams = from_value(json!({"legs": [
            {"move_to":[0,1,2], "crouch":false},
            {"face":[1,2]},
            {"face":-66},
            {"jump":{"from":[0,0,0], "land":[1,1,0]}},
            {"crouch_into":[1,0,0], "hold_ticks":3, "exit_face":-66},
            {"wait":4}
        ]}))
        .unwrap();
        assert_eq!(route.timeout_ticks, DEFAULT_TIMEOUT_TICKS);
        assert!(route.legs.iter().all(|leg| leg.check().is_ok()));
        let Leg::Move {
            tolerance,
            speed,
            crouch,
            ..
        } = route.legs[0]
        else {
            panic!()
        };
        assert_eq!((tolerance, speed, crouch), (DEFAULT_TOLERANCE, 1.0, false));
        assert!(from_value::<Leg>(json!({"face":0, "wait":4})).is_err());
        assert!(from_value::<Leg>(json!({"move_to":[0,0,0]})).is_err());
        let fast: Leg =
            from_value(json!({"move_to":[0,0,0], "crouch":false, "speed":1.5})).unwrap();
        assert!(fast.check().is_err());
    }
}
