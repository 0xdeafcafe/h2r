//! Drawing one private, playerless world view for Theater or an agent camera.
use engine::{GuestMemory, render_loop::views::VIEW_STRIDE};
use serde::{Deserialize, Serialize};

pub(crate) const PITCH_LIMIT: f32 = 1.55;
const RENDER_CAMERA_BYTES: usize = 0x74;
const WORLD_VIEW_RETURN: u32 = 0x0022_3719;
const NEAR_CLIP: u32 = 0x0048_5ad4;
const FAR_CLIP: u32 = 0x0048_5ad8;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub(crate) struct Pose {
    pub(crate) position: [f32; 3],
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

impl Pose {
    pub(crate) fn looking(position: [f32; 3], direction: [f32; 3]) -> Option<Self> {
        let [x, y, z] = direction;
        let planar = x.hypot(y);
        (position.into_iter().chain(direction).all(f32::is_finite) && planar.hypot(z) > 0.0001)
            .then(|| Self {
                position,
                yaw: y.atan2(x),
                pitch: z.atan2(planar).clamp(-PITCH_LIMIT, PITCH_LIMIT),
            })
    }

    pub(crate) fn axes(self) -> ([f32; 3], [f32; 3]) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        ([cy * cp, sy * cp, sp], [-cy * sp, -sy * sp, cp])
    }
}

pub(crate) fn put_view(record: &mut [u8; VIEW_STRIDE as usize], pose: Pose, clip: [f32; 2]) {
    let (forward, up) = pose.axes();
    let put = |bytes: &mut [u8], at: usize, values: &[f32]| {
        for (slot, value) in bytes[at..].chunks_exact_mut(4).zip(values) {
            slot.copy_from_slice(&value.to_le_bytes());
        }
    };
    for at in [0x0c, 0x80] {
        let camera = &mut record[at..at + RENDER_CAMERA_BYTES];
        put(camera, 0, &pose.position);
        put(camera, 0x0c, &forward);
        put(camera, 0x18, &up);
        put(camera, 0x40, &clip);
    }
}

pub(crate) fn blank_view() -> [u8; VIEW_STRIDE as usize] {
    // View kind and slot are zero. The player index must be -1: the
    // retail view reader otherwise indexes the local player's object.
    let mut record = [0u8; VIEW_STRIDE as usize];
    record[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    let screen = [0i16, 0, 480, 640];
    let fov = 2.0 * ((35.0f32.to_radians()).tan() / (640.0 / 480.0)).atan();
    for at in [0x0c, 0x80] {
        record[at + 0x28..at + 0x2c].copy_from_slice(&fov.to_le_bytes());
        record[at + 0x2c..at + 0x30].copy_from_slice(&1.0f32.to_le_bytes());
        for viewport in [0x30, 0x38] {
            for (i, edge) in screen.into_iter().enumerate() {
                let offset = at + viewport + i * 2;
                record[offset..offset + 2].copy_from_slice(&edge.to_le_bytes());
            }
        }
    }
    record
}

fn clip_distances(guest: &dyn GuestMemory) -> [f32; 2] {
    let near = guest.f32(NEAR_CLIP).unwrap_or_default();
    let far = guest.f32(FAR_CLIP).unwrap_or_default();
    if near.is_finite() && far.is_finite() && near > 0.0 && far > near {
        [near, far]
    } else {
        [0.06, 1024.0]
    }
}

/// Replaces the world renderer's argument with a private guest allocation.
pub(crate) fn draw_world_view(
    call: &mut hooks::Call<'_>,
    scratch: u32,
    resolve: impl FnOnce(&runtime::Guest, Option<Pose>) -> Option<Pose>,
) -> bool {
    let drawn = replace_world_view(
        &mut *call.ctx.memory(),
        call.cpu.esp,
        call.cpu.eax,
        scratch,
        resolve,
    );
    if drawn {
        call.cpu.eax = 1;
    }
    drawn
}

fn replace_world_view<M: GuestMemory>(
    guest: &mut M,
    esp: u32,
    count: u32,
    scratch: u32,
    resolve: impl FnOnce(&M, Option<Pose>) -> Option<Pose>,
) -> bool {
    if guest.u32(esp).ok() != Some(WORLD_VIEW_RETURN) {
        return false;
    }
    let arg = esp.wrapping_add(12);
    let template = guest.u32(arg).ok().filter(|&addr| {
        addr != 0
            && count > 0
            && guest.u32(addr).ok() == Some(0)
            && guest.u32(addr.wrapping_add(4)).ok() == Some(0)
    });
    let retail = template.and_then(|addr| {
        Pose::looking(
            guest.vec3(addr.wrapping_add(0x0c)).ok()?,
            guest.vec3(addr.wrapping_add(0x18)).ok()?,
        )
    });
    let Some(pose) = resolve(guest, retail) else {
        return false;
    };
    let mut record = [0; VIEW_STRIDE as usize];
    if template.is_none_or(|addr| guest.read(addr, &mut record).is_err()) {
        record = blank_view();
    }
    record[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    put_view(&mut record, pose, clip_distances(guest));
    guest.write(scratch, &record).is_ok() && guest.set_u32(arg, scratch).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn replaces_both_world_cameras_and_preserves_the_source_view() {
        const STACK: u32 = 0x1000;
        const ORIGINAL: u32 = 0x2000;
        const SCRATCH: u32 = 0x3000;
        let mut guest = FakeGuest::new();
        let mut original = blank_view();
        original[8..12].copy_from_slice(&0u32.to_le_bytes());
        original[0x0c + 0x28..0x0c + 0x2c].copy_from_slice(&0.92f32.to_le_bytes());
        guest
            .put_u32(STACK, WORLD_VIEW_RETURN)
            .put_u32(STACK + 12, ORIGINAL)
            .put(ORIGINAL, &original)
            .map(SCRATCH, VIEW_STRIDE);
        let pose = Pose {
            position: [6.0, -2.0, 4.0],
            yaw: 0.0,
            pitch: 0.0,
        };
        assert!(replace_world_view(&mut guest, STACK, 1, SCRATCH, |_, _| {
            Some(pose)
        }));
        assert_eq!(guest.u32(STACK + 12).unwrap(), SCRATCH);
        assert_eq!(guest.u32(SCRATCH + 8).unwrap(), u32::MAX);
        assert_eq!(guest.f32(SCRATCH + 0x0c + 0x28).unwrap(), 0.92);
        for camera in [0x0c, 0x80] {
            assert_eq!(guest.vec3(SCRATCH + camera).unwrap(), pose.position);
            assert_eq!(guest.vec3(SCRATCH + camera + 12).unwrap(), [1.0, 0.0, 0.0]);
        }
        let mut unchanged = [0; VIEW_STRIDE as usize];
        guest.read(ORIGINAL, &mut unchanged).unwrap();
        assert_eq!(original, unchanged);

        guest.put_u32(STACK, 0).put_u32(STACK + 12, ORIGINAL);
        assert!(!replace_world_view(
            &mut guest,
            STACK,
            1,
            SCRATCH,
            |_, _| Some(pose)
        ));
        assert_eq!(guest.u32(STACK + 12).unwrap(), ORIGINAL);
    }
}
