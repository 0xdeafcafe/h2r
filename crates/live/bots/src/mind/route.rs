//! The Clankers' controller mechanics for a caller that supplies the
//! desired motion. A route has no combat decisions or idle fidgets.

use input::host::HostPadState;

use super::look::{Hand, angle_error};
use super::pad::{Buttons, Feet};
use super::{CROUCH, JUMP, Skill, Vec3, angles, movement_stick};

/// One tick's requested travel and look. The yaw is in world radians.
#[derive(Clone, Copy, Debug, Default)]
pub struct RouteMotion {
    pub dir: [f32; 2],
    pub speed: f32,
    pub yaw: Option<f32>,
    pub jump: bool,
    pub crouch: bool,
}

/// Stateful bot thumb and jump timing for a route's controller.
#[derive(Clone, Debug)]
pub struct RoutePad {
    hand: Hand,
    feet: Feet,
    buttons: Buttons,
}

impl Default for RoutePad {
    fn default() -> Self {
        Self {
            hand: Hand::new(0),
            feet: Feet::new(0),
            buttons: Buttons::new(0),
        }
    }
}

impl RoutePad {
    /// Convert world motion to an ordinary pad in the current view frame.
    /// The caller samples this once per game tick.
    pub fn report(&mut self, aim: Vec3, tick: u32, motion: RouteMotion) -> HostPadState {
        let facing = angles(aim);
        let look = motion.yaw.map(|yaw| angle_error(facing, (yaw, facing.1)));
        let jump = self.buttons.jump(motion.jump, tick);
        let desired = (motion.speed > 0.0).then_some(motion.dir);
        // Follow the checked steering direction on turns, without
        // smoothing across the edge of the walkable corridor.
        let walk = self.feet.step(desired, 1, motion.speed, jump, |_| false);
        let mut pad = HostPadState {
            left_stick: movement_stick(walk, facing.0),
            right_stick: self.hand.steer(Skill::Legendary, look, 1),
            ..Default::default()
        };
        pad.buttons.set(JUMP, jump);
        pad.buttons.set(CROUCH, motion.crouch);
        pad
    }
}
