//! The frame's GPU particle systems as the game publishes them
//! (docs/graphics/native-particles.md, "GPU particle systems", "The
//! frame graph's particle stage"). The births come from the game's model
//! of the guest walk (one [`Spawn`] a particle, `particle_mirror::spawn`);
//! each system names its `prt3` definition as the GPU holds it, its
//! sprite table and the bitmap tag's image it samples. The frame graph
//! draws the set in each game view's slot 9 (`frame_graph::particles`).
//!
//! A system is the frame's births alone (`GpuParticles::mirror`): the
//! game owns the particles, the GPU draws them as they are this frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use frame_scene::RenderCamera;

use crate::gpu::{DepthRule, Image};
use crate::particle_compute::Blend;
use crate::particle_system::{Definition, Spawn, Sprite, View, depth};

/// A bitmap tag's image, the texture its systems sample. Held by pointer
/// across frames: one upload per device while the game publishes it.
pub struct SpriteBitmap {
    /// The bitmap tag's datum.
    pub tag: u32,
    pub image: Image,
}

/// One system: a definition and bitmap, and its particles this frame.
#[derive(Clone)]
pub struct System {
    pub bitmap: Arc<SpriteBitmap>,
    /// `particle_definition::definition`, then `particle_mirror::owned`.
    pub definition: Definition,
    /// The bitmap's sprite table (`Sheet::sprites`).
    pub sprites: Vec<Sprite>,
    pub blend: Blend,
    /// In the guest's order; past `MAX_CAPACITY` they are dropped.
    pub births: Vec<Spawn>,
}

/// What the game published for the frame, in draw order.
#[derive(Clone, Default)]
pub struct Set {
    pub systems: Vec<System>,
}

impl Set {
    /// Whether any system has a particle to draw.
    pub fn draws(&self) -> bool {
        self.systems.iter().any(|s| !s.births.is_empty())
    }
}

static PUBLISHED: Mutex<Option<Arc<Set>>> = Mutex::new(None);
static HAS_PUBLISHED: AtomicBool = AtomicBool::new(false);

/// The frame's systems, or none. A set with no birth is none.
pub fn publish(set: Option<Set>) {
    let set = set.filter(Set::draws).map(Arc::new);
    if set.is_none() && !HAS_PUBLISHED.swap(false, Ordering::AcqRel) {
        return;
    }
    HAS_PUBLISHED.store(set.is_some(), Ordering::Release);
    *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = set;
}

pub fn published() -> Option<Arc<Set>> {
    if !HAS_PUBLISHED.load(Ordering::Acquire) {
        return None;
    }
    PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The draw's view uniform for a game view's camera over the guest's
/// D24 W-buffer: clip from the camera's view-projection, the billboard on
/// the camera's right and up, and the stored depth the world's fixed
/// stage writes (`cull_gpu::DepthModel::fixed`: the view depth times
/// `full_scale / far`, floored, culled behind the eye and past far).
pub fn view(camera: &RenderCamera) -> View {
    let clip = camera.view_projection();
    let v = camera.view();
    let rule = DepthRule::D24;
    let k = rule.full_scale / camera.far;
    View {
        clip_from_world: clip,
        right: [v[0][0], v[1][0], v[2][0], 0.0],
        up: [v[0][1], v[1][1], v[2][1], 0.0],
        w_row: [clip[0][3] * k, clip[1][3] * k, clip[2][3] * k, clip[3][3] * k],
        depth: [rule.full_scale, 0.0, rule.full_scale, depth::FLOOR as f32],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> RenderCamera {
        RenderCamera {
            position: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 70f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.0625,
            far: 1024.0,
        }
    }

    #[test]
    fn the_view_faces_the_camera_and_stores_the_w_buffers_depth() {
        let c = camera();
        let v = view(&c);
        // Looking down +x with z up: right is -y, up is +z.
        assert_eq!(v.right, [0.0, -1.0, 0.0, 0.0]);
        assert_eq!(v.up, [0.0, 0.0, 1.0, 0.0]);
        // A point 512 ahead stores half the full scale.
        let p = [513.0, 2.0, 3.0, 1.0];
        let w: f32 = v.w_row.iter().zip(p).map(|(a, b)| a * b).sum();
        let want = 512.0 * DepthRule::D24.full_scale / c.far;
        assert!((w - want).abs() <= want * 1e-5, "{w} {want}");
        assert_eq!(v.depth, [DepthRule::D24.full_scale, 0.0, DepthRule::D24.full_scale, 1.0]);
    }
}
