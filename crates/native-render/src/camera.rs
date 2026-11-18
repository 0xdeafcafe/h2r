//! The game's render camera, read from guest memory at the layer-3 hook,
//! and the view and projection the native pass draws with.
//!
//! The render camera record (`0x0002f800` fills it per view;
//! docs/graphics/occlusion-culling.md, "Camera and view"): position `+0`,
//! forward `+0xc`, up `+0x18`, vertical FOV `+0x28` (radians), viewport
//! `+0x30` (i16 top, left, bottom, right), near `+0x40`, far `+0x44`.
//! World space is z up. The projection follows the game's builder
//! `0x0002fb70`: `t = tan(fov / 2)`, y scale `1 / t`, x scale
//! `1 / (t · aspect)`, aspect `(right - left) / (bottom - top)`.

/// Matrices are column-major (`m[column][row]`), as WGSL's `mat4x4`.
pub type Mat4 = [[f32; 4]; 4];

/// The render camera record's address (research point).
pub const RENDER_CAMERA: u32 = 0x004b_9da0;

/// Reads guest memory. Implemented by the feature over the runtime's
/// memory; by a byte array in tests.
pub trait GuestRead {
    fn read_u32(&self, addr: u32) -> Option<u32>;

    fn read_f32(&self, addr: u32) -> Option<f32> {
        self.read_u32(addr).map(f32::from_bits)
    }
}

/// Writes guest memory (the skip's mask bits only).
pub trait GuestWrite: GuestRead {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderCamera {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    /// Vertical field of view, radians.
    pub fov: f32,
    /// Top, left, bottom, right in pixels.
    pub viewport: [i16; 4],
    pub near: f32,
    pub far: f32,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(a, a).sqrt();
    (l.is_finite() && l > 1e-6).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

/// `a · b` for column-major matrices.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            out[c][r] = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

/// `m · v`.
pub fn transform(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for r in 0..4 {
        out[r] = (0..4).map(|c| m[c][r] * v[c]).sum();
    }
    out
}

impl RenderCamera {
    /// Reads the record at `addr`; `None` when unreadable or not a usable
    /// camera (non-finite, zero axes, an empty viewport, `far <= near`).
    pub fn read(mem: &dyn GuestRead, addr: u32) -> Option<Self> {
        let v3 = |o: u32| -> Option<[f32; 3]> {
            Some([mem.read_f32(addr + o)?, mem.read_f32(addr + o + 4)?, mem.read_f32(addr + o + 8)?])
        };
        let half = |o: u32| -> Option<i16> {
            let w = mem.read_u32((addr + o) & !3)?;
            Some(if (addr + o) & 2 == 0 { w as u16 as i16 } else { (w >> 16) as u16 as i16 })
        };
        let cam = Self {
            position: v3(0)?,
            forward: v3(0xc)?,
            up: v3(0x18)?,
            fov: mem.read_f32(addr + 0x28)?,
            viewport: [half(0x30)?, half(0x32)?, half(0x34)?, half(0x36)?],
            near: mem.read_f32(addr + 0x40)?,
            far: mem.read_f32(addr + 0x44)?,
        };
        cam.is_usable().then_some(cam)
    }

    fn is_usable(&self) -> bool {
        let finite = self
            .position
            .iter()
            .chain(&self.forward)
            .chain(&self.up)
            .chain([&self.fov, &self.near, &self.far])
            .all(|f| f.is_finite());
        let [top, left, bottom, right] = self.viewport;
        finite
            && self.fov > 0.0
            && self.fov < std::f32::consts::PI
            && self.near > 0.0
            && self.far > self.near
            && right > left
            && bottom > top
            && normalize(cross(self.forward, self.up)).is_some()
    }

    /// The viewport's width over its height.
    pub fn aspect(&self) -> f32 {
        let [top, left, bottom, right] = self.viewport;
        f32::from(right - left) / f32::from(bottom - top)
    }

    /// World to view: x right, y up, z backwards (right-handed, looking
    /// down -z), so the view depth along `forward` is `-z`, the `w` the
    /// projection makes.
    pub fn view(&self) -> Mat4 {
        let f = normalize(self.forward).unwrap_or([1.0, 0.0, 0.0]);
        let r = normalize(cross(f, self.up)).unwrap_or([0.0, -1.0, 0.0]);
        let u = cross(r, f);
        let p = self.position;
        [
            [r[0], u[0], -f[0], 0.0],
            [r[1], u[1], -f[1], 0.0],
            [r[2], u[2], -f[2], 0.0],
            [-dot(r, p), -dot(u, p), dot(f, p), 1.0],
        ]
    }

    /// View to clip, wgpu depth range 0..1 between near and far. Clip `w`
    /// is the view depth; the fragment shader writes the W-buffer depth
    /// from it ([`w_depth`]), so clip z only clips.
    pub fn projection(&self) -> Mat4 {
        let t = (self.fov * 0.5).tan();
        let ys = 1.0 / t;
        let xs = 1.0 / (t * self.aspect());
        let (n, f) = (self.near, self.far);
        [
            [xs, 0.0, 0.0, 0.0],
            [0.0, ys, 0.0, 0.0],
            [0.0, 0.0, f / (n - f), -1.0],
            [0.0, 0.0, n * f / (n - f), 0.0],
        ]
    }

    pub fn view_projection(&self) -> Mat4 {
        mul(&self.projection(), &self.view())
    }

    /// The view depth of a world point (distance along `forward`).
    pub fn depth_of(&self, p: [f32; 3]) -> f32 {
        let f = normalize(self.forward).unwrap_or([1.0, 0.0, 0.0]);
        dot(sub(p, self.position), f)
    }
}

/// The depth the game's W-buffer stores for view depth `w`: `floor(w ·
/// zs / far) / zs`, clamped to 0..1, `zs` the depth format's full scale
/// (docs/graphics/occlusion-culling.md, "The W scale"; the translated
/// pixel programs' `xps_w_depth`).
pub fn w_depth(w: f32, far: f32, zs: f32) -> f32 {
    ((w * (zs / far)).floor() / zs).clamp(0.0, 1.0)
}

/// D24's full scale.
pub const D24_SCALE: f32 = 16_777_215.0;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Sparse guest memory of words.
    #[derive(Default)]
    pub struct Mem(pub HashMap<u32, u32>);

    impl Mem {
        pub fn f(&mut self, addr: u32, v: f32) {
            self.0.insert(addr, v.to_bits());
        }
        pub fn w(&mut self, addr: u32, v: u32) {
            self.0.insert(addr, v);
        }
    }
    impl GuestRead for Mem {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }
    impl GuestWrite for Mem {
        fn write_u32(&mut self, addr: u32, value: u32) -> Option<()> {
            self.0.insert(addr, value).map(|_| ())
        }
    }

    /// A camera at (1, 2, 3) looking down +x, z up, 640x480, 70 degrees.
    pub fn record(mem: &mut Mem, at: u32) {
        for (i, v) in [1.0, 2.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
            mem.f(at + 4 * i as u32, v);
        }
        mem.f(at + 0x28, 70f32.to_radians());
        mem.w(at + 0x30, 0); // top 0, left 0
        mem.w(at + 0x34, 480 | (640 << 16)); // bottom 480, right 640
        mem.f(at + 0x40, 0.0625);
        mem.f(at + 0x44, 1024.0);
    }

    #[test]
    fn the_record_reads_and_bad_records_do_not() {
        let mut m = Mem::default();
        record(&mut m, RENDER_CAMERA);
        let c = RenderCamera::read(&m, RENDER_CAMERA).unwrap();
        assert_eq!(c.position, [1.0, 2.0, 3.0]);
        assert_eq!(c.viewport, [0, 0, 480, 640]);
        assert_eq!((c.near, c.far), (0.0625, 1024.0));
        assert!((c.aspect() - 4.0 / 3.0).abs() < 1e-6);
        m.f(RENDER_CAMERA + 0x44, 0.01);
        assert_eq!(RenderCamera::read(&m, RENDER_CAMERA), None, "far below near");
        record(&mut m, RENDER_CAMERA);
        m.f(RENDER_CAMERA + 0xc, f32::NAN);
        assert_eq!(RenderCamera::read(&m, RENDER_CAMERA), None, "NaN forward");
        record(&mut m, RENDER_CAMERA);
        m.0.remove(&(RENDER_CAMERA + 0x44));
        assert_eq!(RenderCamera::read(&m, RENDER_CAMERA), None, "unreadable");
    }

    #[test]
    fn the_projection_puts_forward_at_the_centre_with_w_the_view_depth() {
        let mut m = Mem::default();
        record(&mut m, RENDER_CAMERA);
        let c = RenderCamera::read(&m, RENDER_CAMERA).unwrap();
        let vp = c.view_projection();
        // 10 units ahead: the centre of the screen, w = 10.
        let p = transform(&vp, [11.0, 2.0, 3.0, 1.0]);
        assert!((p[3] - 10.0).abs() < 1e-5, "{p:?}");
        assert!(p[0].abs() < 1e-5 && p[1].abs() < 1e-5);
        assert!(p[2] > 0.0 && p[2] < p[3]);
        assert!((c.depth_of([11.0, 2.0, 3.0]) - 10.0).abs() < 1e-6);
        // Up is +y on screen, the camera's left (+y in the world) is -x.
        let up = transform(&vp, [11.0, 2.0, 4.0, 1.0]);
        let left = transform(&vp, [11.0, 3.0, 3.0, 1.0]);
        assert!(up[1] > 0.0 && left[0] < 0.0);
        // The top edge of the frustum: tan(35 degrees) · 10 up is y = w.
        let edge = transform(&vp, [11.0, 2.0, 3.0 + 10.0 * 35f32.to_radians().tan(), 1.0]);
        assert!((edge[1] / edge[3] - 1.0).abs() < 1e-5);
        // The side edge is the aspect wider.
        let side = 10.0 * 35f32.to_radians().tan() * 4.0 / 3.0;
        let edge = transform(&vp, [11.0, 2.0 - side, 3.0, 1.0]);
        assert!((edge[0] / edge[3] - 1.0).abs() < 1e-5);
        // Near and far map to 0 and 1.
        let near = transform(&vp, [1.0625, 2.0, 3.0, 1.0]);
        let far = transform(&vp, [1025.0, 2.0, 3.0, 1.0]);
        assert!((near[2] / near[3]).abs() < 1e-4);
        assert!((far[2] / far[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn w_depth_floors_to_the_format_step() {
        let step = 1024.0 / D24_SCALE;
        assert_eq!(w_depth(0.0, 1024.0, D24_SCALE), 0.0);
        assert_eq!(w_depth(1024.0, 1024.0, D24_SCALE), 1.0);
        assert_eq!(w_depth(5000.0, 1024.0, D24_SCALE), 1.0);
        let d = w_depth(10.0 + step * 0.5, 1024.0, D24_SCALE);
        assert_eq!(d, w_depth(10.0, 1024.0, D24_SCALE).max(d));
        assert!(((d * D24_SCALE).fract()).abs() < 1e-3);
    }
}
