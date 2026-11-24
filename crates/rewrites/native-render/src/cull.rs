//! Frustum culling of the world's draws into indirect slots.
//!
//! Every draw owns one 20-byte slot of the indirect buffer for the whole
//! map ([`indirect_slots`]); a frame only flips each slot's
//! `instance_count` between 0 and 1 ([`cull`]). The slots of a batch are
//! contiguous, so the pass makes one `multi_draw_indexed_indirect` per
//! batch. A compute pass can take the same slots over later.

use crate::camera::Mat4;
use crate::world::{Aabb, World};
use bytemuck::{Pod, Zeroable};

/// `wgpu`'s indexed indirect layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct IndirectSlot {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

/// Size of a slot in bytes.
pub const SLOT_SIZE: u64 = std::mem::size_of::<IndirectSlot>() as u64;

/// One slot per draw, in draw order, all culled.
pub fn indirect_slots(world: &World) -> Vec<IndirectSlot> {
    world
        .draws
        .iter()
        .map(|d| IndirectSlot {
            index_count: d.index_count,
            instance_count: 0,
            first_index: d.first_index,
            base_vertex: 0,
            first_instance: 0,
        })
        .collect()
}

/// Six planes `(n, d)` with `n · p + d >= 0` inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    pub planes: [[f32; 4]; 6],
}

impl Frustum {
    /// From a view-projection with wgpu's clip volume (`-w <= x, y <= w`,
    /// `0 <= z <= w`).
    pub fn from_view_projection(m: &Mat4) -> Self {
        let row = |r: usize| [m[0][r], m[1][r], m[2][r], m[3][r]];
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
        let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
        let subp = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
        Self {
            planes: [
                add(r3, r0),
                subp(r3, r0),
                add(r3, r1),
                subp(r3, r1),
                r2,
                subp(r3, r2),
            ],
        }
    }

    /// [`Frustum::from_view_projection`] without its near and far planes:
    /// the four sides, and in front of the eye (`w >= 0`) twice. For a
    /// pass whose clipping is the game's own (its programs' depth, the
    /// D3D path's W clip range): the render camera's `near` and `far` are
    /// not the range the game's projection clips to (a cinematic's far
    /// geometry lies past the camera's `far`), so culling by them leaves
    /// out parts the D3D path draws.
    pub fn sides(m: &Mat4) -> Self {
        let mut f = Self::from_view_projection(m);
        let eye = [m[0][3], m[1][3], m[2][3], m[3][3]];
        f.planes[4] = eye;
        f.planes[5] = eye;
        f
    }

    /// Whether any of the box may be inside: false only when the box is
    /// wholly outside one plane (conservative near the corners).
    pub fn may_see(&self, b: &Aabb) -> bool {
        self.planes.iter().all(|p| {
            // The box corner furthest along the plane's normal.
            let x = if p[0] >= 0.0 { b.max[0] } else { b.min[0] };
            let y = if p[1] >= 0.0 { b.max[1] } else { b.min[1] };
            let z = if p[2] >= 0.0 { b.max[2] } else { b.min[2] };
            p[0] * x + p[1] * y + p[2] * z + p[3] >= 0.0
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CullStats {
    pub visible: u32,
    pub total: u32,
}

/// Sets each slot's `instance_count` from the frustum test of its draw.
pub fn cull(world: &World, frustum: &Frustum, slots: &mut [IndirectSlot]) -> CullStats {
    let mut visible = 0;
    for (slot, d) in slots.iter_mut().zip(&world.draws) {
        let on = frustum.may_see(&d.bounds);
        slot.instance_count = u32::from(on);
        visible += u32::from(on);
    }
    CullStats {
        visible,
        total: world.draws.len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::RenderCamera;
    use crate::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};

    fn camera() -> RenderCamera {
        RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 90f32.to_radians(),
            projection_xy: None,
            viewport: [0, 0, 100, 100],
            near: 0.1,
            far: 100.0,
        }
    }

    fn cube(at: [f32; 3], size: f32) -> Aabb {
        Aabb {
            min: at,
            max: [at[0] + size, at[1] + size, at[2] + size],
        }
    }

    #[test]
    fn boxes_inside_straddling_and_outside() {
        let f = Frustum::from_view_projection(&camera().view_projection());
        assert!(f.may_see(&cube([10.0, -0.5, -0.5], 1.0)), "ahead");
        assert!(f.may_see(&cube([-0.5, -0.5, -0.5], 1.0)), "around the eye");
        assert!(f.may_see(&cube([10.0, 9.0, 0.0], 3.0)), "across the left edge");
        assert!(!f.may_see(&cube([-10.0, 0.0, 0.0], 1.0)), "behind");
        assert!(!f.may_see(&cube([10.0, 12.0, 0.0], 1.0)), "left of the 90 degree view");
        assert!(!f.may_see(&cube([10.0, -13.0, 0.0], 1.0)), "right of it");
        assert!(!f.may_see(&cube([10.0, 0.0, 12.0], 1.0)), "above");
        assert!(!f.may_see(&cube([200.0, 0.0, 0.0], 1.0)), "past far");
        let s = Frustum::sides(&camera().view_projection());
        assert!(s.may_see(&cube([200.0, 0.0, 0.0], 1.0)), "past far: the sides alone keep it");
        assert!(s.may_see(&cube([0.01, -0.001, -0.001], 0.002)), "nearer than near");
        assert!(!s.may_see(&cube([-10.0, 0.0, 0.0], 1.0)), "behind");
        assert!(!s.may_see(&cube([10.0, 12.0, 0.0], 1.0)), "left of the view");
        assert!(!s.may_see(&cube([10.0, 0.0, 12.0], 1.0)), "above");
    }

    #[test]
    fn wider_player_and_display_lenses_admit_bsp_clusters_at_their_edges() {
        use crate::camera::display_camera;

        // Ten units forward, with clusters in the centre, at the left edge,
        // below the retail view, and beyond even the wider view.
        let mut builder = WorldBuilder::new();
        for (id, y, z) in [(0, 0., 0.), (1, 8.5, 0.), (2, 0., -8.), (3, -25., 0.)] {
            let section = SectionInput {
                positions: vec![[10., y, z], [10., y + 0.2, z], [10., y, z + 0.2]],
                parts: vec![PartInput {
                    material: 0,
                    transparent: false,
                    triangles: vec![[0, 1, 2]],
                }],
                ..Default::default()
            };
            builder.add_section(Owner::Cluster(id), &section, &Placement::IDENTITY, None);
        }
        let world = builder.finish();
        let mut slots = indirect_slots(&world);
        let lens = |horizontal: f32| RenderCamera {
            // Player degrees describe a horizontal 4:3 lens; the render camera stores vertical FOV.
            fov: 2.0 * ((horizontal.to_radians() * 0.5).tan() * 0.75).atan(),
            viewport: [0, 0, 480, 640],
            ..camera()
        };
        let retail = lens(70.);
        for (view, visible) in [
            (retail, vec![Owner::Cluster(0)]),
            (lens(110.), vec![Owner::Cluster(0), Owner::Cluster(1), Owner::Cluster(2)]),
            // The 16:9 display retains the 70-degree camera's vertical lens.
            (display_camera(retail, 0.75), vec![Owner::Cluster(0), Owner::Cluster(1)]),
        ] {
            cull(&world, &Frustum::sides(&view.view_projection()), &mut slots);
            let drawn: Vec<_> = world.draws.iter().zip(&slots)
                .filter(|(_, slot)| slot.instance_count != 0)
                .map(|(draw, _)| draw.owner)
                .collect();
            assert_eq!(drawn, visible);
        }
    }

    #[test]
    fn slots_follow_the_draws_and_culling_flips_instance_counts() {
        let quad = |x: f32| SectionInput {
            positions: vec![[x, -1.0, -1.0], [x, 1.0, -1.0], [x, -1.0, 1.0]],
            parts: vec![PartInput {
                material: 0,
                transparent: false,
                triangles: vec![[0, 1, 2]],
            }],
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &quad(10.0), &Placement::IDENTITY, None);
        b.add_section(Owner::Cluster(1), &quad(-10.0), &Placement::IDENTITY, None);
        let w = b.finish();
        let mut slots = indirect_slots(&w);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[1].first_index, 3);
        assert!(slots.iter().all(|s| s.index_count == 3 && s.instance_count == 0));
        let f = Frustum::from_view_projection(&camera().view_projection());
        let stats = cull(&w, &f, &mut slots);
        assert_eq!(stats, CullStats { visible: 1, total: 2 });
        let on: Vec<_> = w.draws.iter().zip(&slots).map(|(d, s)| (d.owner, s.instance_count)).collect();
        assert!(on.contains(&(Owner::Cluster(0), 1)) && on.contains(&(Owner::Cluster(1), 0)));
        assert_eq!(SLOT_SIZE, 20);
    }
}
