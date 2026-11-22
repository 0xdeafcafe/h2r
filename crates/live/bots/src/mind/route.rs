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
        self.pad(facing, look, tick, motion)
    }

    fn pad(
        &mut self,
        facing: (f32, f32),
        look: Option<[f32; 2]>,
        tick: u32,
        motion: RouteMotion,
    ) -> HostPadState {
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

    /// [`RoutePad::report`], looking from `eye` at `point` (yaw and
    /// pitch, over the motion's yaw) and pulling the trigger once the aim
    /// is within [`SHOOT_CONE`] of it: a route breaking glass
    /// (`bounce_route`'s break step).
    pub fn report_shooting(
        &mut self,
        aim: Vec3,
        eye: Vec3,
        point: Vec3,
        tick: u32,
        motion: RouteMotion,
    ) -> HostPadState {
        let facing = angles(aim);
        let to = angles([point[0] - eye[0], point[1] - eye[1], point[2] - eye[2]]);
        let error = angle_error(facing, to);
        let mut pad = self.pad(facing, Some(error), tick, motion);
        let on = error[0].abs() <= SHOOT_CONE && error[1].abs() <= SHOOT_CONE;
        pad.right_trigger = if on { 1.0 } else { 0.0 };
        pad
    }
}

/// A route shoots at its point only while the aim is this near it, in yaw
/// and in pitch (radians: 4 degrees).
pub const SHOOT_CONE: f32 = 4.0 * std::f32::consts::PI / 180.0;
