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

    pub fn point(self, distance: f32) -> [f32; 3] {
        let forward = self.forward();
        std::array::from_fn(|i| self.position[i] + distance * forward[i])
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
