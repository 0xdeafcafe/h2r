//! The local director camera. Its pose and its aim ray are presentation state.
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl Camera {
    pub fn forward(self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [cy * cp, sy * cp, sp]
    }

    /// Forward, right and up, a right-handed orthonormal basis.
    pub fn basis(self) -> [[f32; 3]; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [self.forward(), [sy, -cy, 0.0], [-cy * sp, -sy * sp, cp]]
    }

    /// `local` (along forward, right and up) in world axes.
    pub fn to_world(self, local: [f32; 3]) -> [f32; 3] {
        let b = self.basis();
        std::array::from_fn(|i| local[0] * b[0][i] + local[1] * b[1][i] + local[2] * b[2][i])
    }

    /// The world vector `v` along forward, right and up.
    pub fn to_local(self, v: [f32; 3]) -> [f32; 3] {
        self.basis()
            .map(|axis| axis[0] * v[0] + axis[1] * v[1] + axis[2] * v[2])
    }

    pub fn point(self, distance: f32) -> [f32; 3] {
        let forward = self.forward();
        std::array::from_fn(|i| self.position[i] + distance * forward[i])
    }

    /// The camera standing `distance` from `target`, on the side of it
    /// this camera is on, looking at it (the browser's fly-to). A camera
    /// on the target looks along its own heading.
    pub fn toward(self, target: [f32; 3], distance: f32) -> Camera {
        let offset: [f32; 3] = std::array::from_fn(|i| self.position[i] - target[i]);
        let length = offset[0].hypot(offset[1]).hypot(offset[2]);
        let away = if length > 1e-4 {
            offset.map(|v| v / length)
        } else {
            self.forward().map(|v| -v)
        };
        Camera {
            position: std::array::from_fn(|i| target[i] + away[i] * distance),
            yaw: (-away[1]).atan2(-away[0]).rem_euclid(TAU),
            pitch: (-away[2]).asin().clamp(-1.55, 1.55),
        }
    }

    pub fn fly(&mut self, movement: [f32; 3], look: [f32; 2], seconds: f32) {
        self.yaw = (self.yaw + look[0]).rem_euclid(TAU);
        self.pitch = (self.pitch + look[1]).clamp(-1.55, 1.55);
        let (sy, cy) = self.yaw.sin_cos();
        let forward = self.forward();
        let right = [sy, -cy, 0.0];
        let len = movement[0].hypot(movement[1]).hypot(movement[2]).max(1.0);
        let speed = 6.0 * seconds / len;
        for (i, p) in self.position.iter_mut().enumerate() {
            *p += speed * (movement[0] * right[i] + movement[1] * forward[i]);
        }
        self.position[2] += speed * movement[2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn toward_stands_off_on_the_same_side_and_looks_at_the_target() {
        let from = Camera {
            position: [10., 0., 0.],
            yaw: 1.,
            pitch: 0.,
        };
        let to = from.toward([0., 0., 0.], 4.);
        assert!(near(to.position[0], 4.) && near(to.position[1], 0.) && near(to.position[2], 0.));
        // Looking back along -x: yaw half a turn, level.
        assert!(near(to.yaw, std::f32::consts::PI) && near(to.pitch, 0.));
        let at = to.point(4.);
        assert!(at.iter().all(|v| near(*v, 0.)), "{at:?}");
    }

    #[test]
    fn toward_looks_down_at_a_target_below_and_keeps_its_distance() {
        let from = Camera {
            position: [7., 4., 5.],
            yaw: 0.,
            pitch: 0.,
        };
        let to = from.toward([3., 4., 2.], 5.);
        assert!(to.pitch < 0., "looks down");
        let at = to.point(5.);
        assert!(near(at[0], 3.) && near(at[1], 4.) && near(at[2], 2.), "{at:?}");
    }

    #[test]
    fn toward_from_on_the_target_uses_the_heading() {
        let from = Camera {
            position: [1., 1., 1.],
            yaw: 0.,
            pitch: 0.,
        };
        let to = from.toward([1., 1., 1.], 2.);
        let at = to.point(2.);
        assert!(near(at[0], 1.) && near(at[1], 1.) && near(at[2], 1.), "{at:?}");
    }
}
