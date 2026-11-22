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
