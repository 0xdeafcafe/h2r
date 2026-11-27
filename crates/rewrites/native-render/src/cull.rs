//! The world's view volume: the frustum planes and the conservative box
//! test. The world pass's cull runs this test on the GPU
//! ([`crate::cull_gpu`], `cull.wgsl`'s `may_see`); the guest's admission
//! of selected parts and the models' instances run it here.

use crate::camera::Mat4;
use crate::world::Aabb;

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

/// One view's world cull, as the GPU counted it
/// (`GpuWorld::read_cull_stats`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CullStats {
    /// Draws the frustum keeps.
    pub visible: u32,
    /// Draws drawn (the early run's and the late run's).
    pub drawn: u32,
    /// Draws the frustum keeps that the depth pyramid hides.
    pub occluded: u32,
    pub total: u32,
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
            let frustum = Frustum::sides(&view.view_projection());
            let drawn: Vec<_> = world.draws.iter()
                .filter(|draw| frustum.may_see(&draw.bounds))
                .map(|draw| draw.owner)
                .collect();
            assert_eq!(drawn, visible);
        }
    }
}
