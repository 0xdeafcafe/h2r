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

    pub(crate) fn rotate(&mut self, look: [f32; 2]) {
        self.yaw = (self.yaw + look[0]).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + look[1]).clamp(-PITCH_LIMIT, PITCH_LIMIT);
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
    let fov = vertical_fov(70.0);
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

pub(crate) fn vertical_fov(horizontal_degrees: f32) -> f32 {
    2.0 * ((horizontal_degrees.to_radians() * 0.5).tan() * 0.75).atan()
}

pub(crate) fn horizontal_fov(vertical_radians: f32) -> f32 {
    (2.0 * ((vertical_radians * 0.5).tan() / 0.75).atan()).to_degrees()
}

pub(crate) fn put_fov(record: &mut [u8; VIEW_STRIDE as usize], vertical: f32) {
    for at in [0x0c, 0x80] {
        record[at + 0x28..at + 0x2c].copy_from_slice(&vertical.to_le_bytes());
    }
}

/// Retail cameras carry a vertical lens and the complete view basis.
pub(crate) fn retail_view(guest: &dyn GuestMemory, view: u32) -> Option<(Pose, f32)> {
    view.checked_add(VIEW_STRIDE)?;
    let camera = view + 0x0c;
    let pose = Pose::looking(guest.vec3(camera).ok()?, guest.vec3(camera + 0x0c).ok()?)?;
    let fov = guest.f32(camera + 0x28).ok()?;
    (0.0 < fov && fov < std::f32::consts::PI).then_some((pose, horizontal_fov(fov)))
}

/// Replaces the world renderer's argument with a private guest allocation.
pub(crate) fn draw_world_view(
    call: &mut hooks::Call<'_>,
    scratch: u32,
    resolve: impl FnOnce(&runtime::Guest, &[u32]) -> Option<(Pose, Option<f32>)>,
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
    resolve: impl FnOnce(&M, &[u32]) -> Option<(Pose, Option<f32>)>,
) -> bool {
    if guest.u32(esp).ok() != Some(WORLD_VIEW_RETURN) {
        return false;
    }
    let arg = esp.wrapping_add(12);
    let mut views = [0; 4];
    let mut len = 0;
    if let Ok(base) = guest.u32(arg) {
        for i in 0..count.min(views.len() as u32) {
            let Some(view) = base.checked_add(i * VIEW_STRIDE) else { break };
            if base != 0 && guest.u32(view).ok() == Some(0) {
                views[len] = view;
                len += 1;
            }
        }
    }
    let Some((pose, fov)) = resolve(guest, &views[..len]) else {
        return false;
    };
    let mut record = [0; VIEW_STRIDE as usize];
    if views[..len].first().is_none_or(|&addr| guest.read(addr, &mut record).is_err()) {
        record = blank_view();
    }
    record[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    // A replacement view occupies the full presentation, even in split screen.
    if count > 1 {
        let blank = blank_view();
        for at in [0x0c, 0x80] {
            record[at + 0x30..at + 0x40].copy_from_slice(&blank[at + 0x30..at + 0x40]);
        }
    }
    put_view(&mut record, pose, clip_distances(guest));
    if let Some(fov) = fov {
        put_fov(&mut record, vertical_fov(fov));
    }
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
            Some((pose, None))
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
            |_, _| Some((pose, None))
        ));
        assert_eq!(guest.u32(STACK + 12).unwrap(), ORIGINAL);
    }
}
