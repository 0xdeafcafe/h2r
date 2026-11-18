//! Occlusion culling of objects against the previous frame's depth
//! (docs/graphics/occlusion-culling.md).
//!
//! The pure half of the design, no GPU and no guest calls:
//!
//! - [`DepthGrid`]: a small grid of the farthest view depth under each
//!   cell, made from a full depth buffer by [`DepthGrid::reduce`] (the CPU
//!   reference for the GPU reduction) or handed over by the GPU readback.
//! - [`HiZ`]: the max-depth pyramid built over the grid.
//! - [`Camera`] and [`Camera::project`]: a bounding sphere to a screen
//!   rectangle and its nearest view depth, conservatively.
//! - [`HiZ::test`]: occluded or not, against the farthest depth the
//!   rectangle covers.
//! - [`Tracker`]: the rules that turn tests into skips (2+ consecutive
//!   occluded frames, camera cuts reset everything, exempt objects).
//!
//! Depths here are linear view depths (distance along the camera's
//! forward axis), which is what the game's W-buffer stores
//! ([`DepthMapping::WBuffer`]). Far (cleared) texels are `f32::INFINITY`
//! and occlude nothing.

use std::collections::HashMap;

pub type Vec3 = [f32; 3];

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: Vec3) -> Vec3 {
    let l = len(a);
    if l > 0.0 && l.is_finite() {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        a
    }
}

/// How a raw depth-buffer value (normalised to `[0, 1]`) maps to linear
/// view depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DepthMapping {
    /// The NV2A W-buffer: the buffer holds `floor(w * scale) / zmax`, `w`
    /// the view depth (docs/graphics/shaders.md, "W-buffering"). The upper
    /// bound of the `w` that stored value is taken (`(d·zmax + 1) / scale`),
    /// so the floor never makes an occluder look nearer than it is. `d >= 1`
    /// is the cleared value: infinitely far.
    WBuffer { zmax: f32, scale: f32 },
    /// A projective z buffer with `near` at 0 and `far` at 1. `d >= 1` is
    /// infinitely far.
    Projective { near: f32, far: f32 },
    /// The values are already linear view depths.
    Linear,
}

impl DepthMapping {
    pub fn view_depth(self, d: f32) -> f32 {
        if d.is_nan() {
            return f32::INFINITY;
        }
        match self {
            DepthMapping::WBuffer { zmax, scale } => {
                if d >= 1.0 || scale <= 0.0 {
                    f32::INFINITY
                } else {
                    (d.max(0.0) * zmax + 1.0) / scale
                }
            }
            DepthMapping::Projective { near, far } => {
                if d >= 1.0 {
                    f32::INFINITY
                } else {
                    let d = d.max(0.0);
                    near * far / (far - d * (far - near))
                }
            }
            DepthMapping::Linear => d,
        }
    }
}

/// The farthest view depth under each cell of a `width x height` grid over
/// the view. Row 0 is the top of the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthGrid {
    pub width: u32,
    pub height: u32,
    pub depth: Vec<f32>,
}

impl DepthGrid {
    /// A grid with every cell far (occludes nothing).
    pub fn far(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            depth: vec![f32::INFINITY; (width * height) as usize],
        }
    }

    /// Reduces a `src_w x src_h` depth buffer (raw values, row 0 the top)
    /// to the grid: each cell keeps the farthest view depth of the pixels
    /// whose centre falls in it. Pixel `p` belongs to cell
    /// `floor((p + 0.5) · width / src_w)`, the same rule [`HiZ::test`] uses
    /// for a rectangle's edges, so a rectangle covering a pixel's centre
    /// always reads that pixel's cell.
    pub fn reduce(
        src: &[f32],
        src_w: u32,
        src_h: u32,
        width: u32,
        height: u32,
        map: DepthMapping,
    ) -> Self {
        let mut g = Self {
            width,
            height,
            depth: vec![f32::NEG_INFINITY; (width * height) as usize],
        };
        let col: Vec<u32> = (0..src_w).map(|x| cell_of(x, src_w, width)).collect();
        for y in 0..src_h {
            let cy = cell_of(y, src_h, height);
            let row = &src[(y * src_w) as usize..((y + 1) * src_w) as usize];
            for (x, &d) in row.iter().enumerate() {
                let i = (cy * width + col[x]) as usize;
                let v = map.view_depth(d);
                if v > g.depth[i] {
                    g.depth[i] = v;
                }
            }
        }
        // A cell no pixel centre fell in (grid finer than the buffer) knows
        // nothing: far.
        for d in &mut g.depth {
            if *d == f32::NEG_INFINITY {
                *d = f32::INFINITY;
            }
        }
        g
    }

    pub fn at(&self, x: u32, y: u32) -> f32 {
        self.depth[(y * self.width + x) as usize]
    }
}

fn cell_of(p: u32, src: u32, cells: u32) -> u32 {
    // floor((p + 0.5) * cells / src) in integers.
    (((2 * p as u64 + 1) * cells as u64) / (2 * src as u64)).min(cells as u64 - 1) as u32
}

/// The max-depth pyramid over a [`DepthGrid`]: level 0 is the grid, each
/// level above holds the farthest of the (up to) 2x2 texels under it.
#[derive(Clone, Debug)]
pub struct HiZ {
    levels: Vec<DepthGrid>,
}

/// A view-space rectangle in grid cells (inclusive) and the nearest view
/// depth of what it bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub near: f32,
}

/// What a test says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Behind the depth everywhere it covers.
    Occluded,
    /// Something of it may be in front.
    Visible,
    /// The test cannot say (crosses the near plane, off screen, degenerate
    /// input): treated as visible.
    Unknown,
}

impl HiZ {
    pub fn build(grid: DepthGrid) -> Self {
        let mut levels = vec![grid];
        loop {
            let prev = levels.last().unwrap();
            if prev.width <= 1 && prev.height <= 1 {
                break;
            }
            let w = prev.width.div_ceil(2);
            let h = prev.height.div_ceil(2);
            let mut next = DepthGrid {
                width: w,
                height: h,
                depth: vec![f32::NEG_INFINITY; (w * h) as usize],
            };
            for y in 0..prev.height {
                for x in 0..prev.width {
                    let i = ((y / 2) * w + x / 2) as usize;
                    let v = prev.at(x, y);
                    // NaN never lands here (reduce maps it far), but keep
                    // the max NaN-safe: a NaN cell is far.
                    let v = if v.is_nan() { f32::INFINITY } else { v };
                    if v > next.depth[i] {
                        next.depth[i] = v;
                    }
                }
            }
            levels.push(next);
        }
        Self { levels }
    }

    pub fn levels(&self) -> &[DepthGrid] {
        &self.levels
    }

    pub fn width(&self) -> u32 {
        self.levels[0].width
    }

    pub fn height(&self) -> u32 {
        self.levels[0].height
    }

    /// The farthest depth over cells `x0..=x1`, `y0..=y1` of level 0, read
    /// from the lowest level where the rectangle spans at most 2x2 texels
    /// (a superset of the rectangle, so never nearer than the true max).
    pub fn max_depth(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
        let mut l = 0usize;
        while l + 1 < self.levels.len() && ((x1 >> l) - (x0 >> l) > 1 || (y1 >> l) - (y0 >> l) > 1)
        {
            l += 1;
        }
        let g = &self.levels[l];
        let mut m = f32::NEG_INFINITY;
        for y in (y0 >> l)..=(y1 >> l).min(g.height - 1) {
            for x in (x0 >> l)..=(x1 >> l).min(g.width - 1) {
                m = m.max(g.at(x, y));
            }
        }
        m
    }

    /// Tests a footprint: occluded when its nearest depth is beyond the
    /// farthest depth it covers (plus `bias`, in view-depth units).
    pub fn test(&self, f: &Footprint, bias: f32) -> Verdict {
        let m = self.max_depth(f.x0, f.y0, f.x1, f.y1);
        if f.near.is_finite() && m.is_finite() && f.near > m + bias {
            Verdict::Occluded
        } else {
            Verdict::Visible
        }
    }
}

/// A view's camera as the occlusion test needs it: where it is, where it
/// looks, and the half-angle tangents of the frustum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    /// `tan(horizontal FOV / 2)`.
    pub tan_half_x: f32,
    /// `tan(vertical FOV / 2)`.
    pub tan_half_y: f32,
    /// The near plane distance; anything reaching it is never culled.
    pub near: f32,
}

impl Camera {
    /// A camera from a vertical FOV (radians) and an aspect (width over
    /// height).
    pub fn from_fov(
        position: Vec3,
        forward: Vec3,
        up: Vec3,
        fov_y: f32,
        aspect: f32,
        near: f32,
    ) -> Self {
        let tan_half_y = (fov_y * 0.5).tan();
        Self {
            position,
            forward,
            up,
            tan_half_x: tan_half_y * aspect,
            tan_half_y,
            near,
        }
    }

    fn valid(&self) -> bool {
        let finite = |v: Vec3| v.iter().all(|c| c.is_finite());
        finite(self.position)
            && finite(self.forward)
            && finite(self.up)
            && self.tan_half_x.is_finite()
            && self.tan_half_y.is_finite()
            && self.tan_half_x > 0.0
            && self.tan_half_y > 0.0
            && self.near.is_finite()
            && self.near >= 0.0
            && len(self.forward) > 1e-6
            && len(cross(self.forward, self.up)) > 1e-6
    }

    /// Projects a sphere onto a `width x height` grid over the view.
    ///
    /// Conservative: the rectangle bounds the projection of the sphere's
    /// view-space bounding box (which holds the sphere), and the depth is
    /// the box's nearest. `None` when the sphere reaches the near plane,
    /// lies wholly outside the view (the game's own frustum test decides
    /// those), or the input is not finite.
    pub fn project(&self, center: Vec3, radius: f32, width: u32, height: u32) -> Option<Footprint> {
        if !self.valid()
            || !radius.is_finite()
            || radius < 0.0
            || !center.iter().all(|c| c.is_finite())
        {
            return None;
        }
        let f = normalize(self.forward);
        let r = normalize(cross(f, self.up));
        let u = cross(r, f);
        let d = sub(center, self.position);
        let (cx, cy, cz) = (dot(d, r), dot(d, u), dot(d, f));
        let near = cz - radius;
        if near <= self.near.max(1e-4) {
            return None;
        }
        // x/z over the box [cx±r]x[cz±r] (z > 0 throughout): its extremes
        // are at corners.
        let ext = |c: f32, t: f32| {
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for a in [c - radius, c + radius] {
                for z in [cz - radius, cz + radius] {
                    let v = a / z / t;
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            (lo, hi)
        };
        let (xl, xh) = ext(cx, self.tan_half_x);
        let (yl, yh) = ext(cy, self.tan_half_y);
        if xh < -1.0 || xl > 1.0 || yh < -1.0 || yl > 1.0 {
            return None;
        }
        // NDC to cells: u = (x + 1) / 2 across, v = (1 - y) / 2 down.
        let to_cell = |n: f32, cells: u32| -> u32 {
            let c = ((n.clamp(-1.0, 1.0) + 1.0) * 0.5 * cells as f32).floor();
            (c.max(0.0) as u32).min(cells - 1)
        };
        Some(Footprint {
            x0: to_cell(xl, width),
            x1: to_cell(xh, width),
            y0: to_cell(-yh, height),
            y1: to_cell(-yl, height),
            near,
        })
    }
}

/// One frame's occlusion data for one view: the pyramid and the camera
/// the depth was drawn with.
#[derive(Clone, Debug)]
pub struct Frame {
    pub hiz: HiZ,
    pub camera: Camera,
    /// The frame the depth was drawn in.
    pub frame: u64,
}

/// The conservative rules (docs/graphics/occlusion-culling.md, "Rules").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// Consecutive occluded tests before an object is skipped.
    pub frames: u8,
    /// Radius multiplier.
    pub inflate: f32,
    /// Added to every radius, world units.
    pub margin: f32,
    /// Depth slack, world units: occluded only past `max + bias`.
    pub bias: f32,
    /// A camera that moved further than this between two frames is a cut.
    pub cut_distance: f32,
    /// A camera whose forward turned by more than this (cosine) is a cut.
    pub cut_cos: f32,
    /// Spheres at least this large are never culled (whole-level objects).
    pub max_radius: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            frames: 2,
            inflate: 1.1,
            margin: 0.1,
            bias: 0.25,
            cut_distance: 4.0,
            cut_cos: 0.7,
            max_radius: 100.0,
        }
    }
}

/// An object as the test sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// The object's datum handle (index and salt): the tracker's key.
    pub key: u32,
    pub center: Vec3,
    pub radius: f32,
    /// Never culled (the player's unit, what it holds, first person,
    /// lights).
    pub exempt: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    streak: u8,
    seen: u64,
}

/// Running counts, for the `occlusion:` line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub tested: u64,
    pub culled: u64,
    pub unknown: u64,
    pub exempt: u64,
    pub resets: u64,
}

/// Per view: decides, object by object, whether to skip it this frame.
///
/// Each frame [`Tracker::begin`] takes the newest [`Frame`] (the previous
/// frame's depth and camera) and the current camera; [`Tracker::decide`]
/// then tests each object against it and answers whether to skip it. An
/// object is skipped only once its tests said occluded `rules.frames`
/// times in a row; one visible (or unknown) answer resets its streak. The
/// test of frame N uses frame N-1's depth, so the earliest skip is the
/// second frame an object is hidden for.
#[derive(Debug)]
pub struct Tracker {
    pub rules: Rules,
    entries: HashMap<u32, Entry>,
    frame: u64,
    depth: Option<Frame>,
    last_camera: Option<Camera>,
    /// The current camera's displacement from the depth's: added to radii.
    parallax: f32,
    pub counts: Counts,
}

impl Tracker {
    pub fn new(rules: Rules) -> Self {
        Self {
            rules,
            entries: HashMap::new(),
            frame: 0,
            depth: None,
            last_camera: None,
            parallax: 0.0,
            counts: Counts::default(),
        }
    }

    /// Forgets every streak (camera cut, map change, view change).
    pub fn reset(&mut self) {
        self.entries.clear();
        self.counts.resets += 1;
    }

    /// Starts frame `frame` with the current `camera` and the newest depth
    /// the GPU has handed over (`None`: nothing is culled this frame).
    pub fn begin(&mut self, frame: u64, camera: Camera, depth: Option<Frame>) {
        if let Some(last) = self.last_camera
            && self.is_cut(&last, &camera)
        {
            self.reset();
        }
        self.last_camera = Some(camera);
        self.frame = frame;
        // Depth older than the previous frame or from a cut is no use.
        self.depth = depth.filter(|d| {
            d.frame < frame && frame - d.frame <= 2 && !self.is_cut(&d.camera, &camera)
        });
        self.parallax = self
            .depth
            .as_ref()
            .map_or(0.0, |d| len(sub(camera.position, d.camera.position)));
        // Drop objects not seen for a while.
        if frame.is_multiple_of(64) {
            self.entries.retain(|_, e| frame - e.seen <= 64);
        }
    }

    fn is_cut(&self, a: &Camera, b: &Camera) -> bool {
        let moved = len(sub(a.position, b.position));
        let turned = dot(normalize(a.forward), normalize(b.forward));
        !(moved <= self.rules.cut_distance && turned >= self.rules.cut_cos)
    }

    /// Tests one object and answers whether to skip it this frame.
    pub fn decide(&mut self, b: &Bounds) -> bool {
        self.counts.tested += 1;
        if b.exempt {
            self.counts.exempt += 1;
            return false;
        }
        let verdict = self.verdict(b);
        let e = self.entries.entry(b.key).or_default();
        if e.seen == self.frame && e.streak > 0 {
            // Tested twice in one frame (several layers or lists): the
            // first answer stands.
        } else if verdict == Verdict::Occluded {
            e.streak = e.streak.saturating_add(1);
        } else {
            e.streak = 0;
        }
        e.seen = self.frame;
        if verdict == Verdict::Unknown {
            self.counts.unknown += 1;
        }
        let skip = verdict == Verdict::Occluded && e.streak >= self.rules.frames;
        if skip {
            self.counts.culled += 1;
        }
        skip
    }

    fn verdict(&self, b: &Bounds) -> Verdict {
        let Some(d) = &self.depth else {
            return Verdict::Unknown;
        };
        if !(b.radius >= 0.0) || b.radius >= self.rules.max_radius {
            return Verdict::Unknown;
        }
        let radius = b.radius * self.rules.inflate + self.rules.margin + self.parallax;
        match d
            .camera
            .project(b.center, radius, d.hiz.width(), d.hiz.height())
        {
            Some(f) => match d.hiz.test(&f, self.rules.bias) {
                Verdict::Occluded => Verdict::Occluded,
                _ => Verdict::Visible,
            },
            None => Verdict::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        // At the origin looking down +x, z up, 90 degrees both ways.
        Camera {
            position: [0.0, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            tan_half_x: 1.0,
            tan_half_y: 1.0,
            near: 0.05,
        }
    }

    /// A depth buffer with a wall at `wall` filling pixel columns
    /// `x0..x1` (everything else far).
    fn wall_buffer(w: u32, h: u32, x0: u32, x1: u32, wall: f32) -> Vec<f32> {
        let mut v = vec![f32::INFINITY; (w * h) as usize];
        for y in 0..h {
            for x in x0..x1 {
                v[(y * w + x) as usize] = wall;
            }
        }
        v
    }

    #[test]
    fn mapping_wbuffer_is_conservative_and_far_is_infinite() {
        let m = DepthMapping::WBuffer {
            zmax: 16_777_215.0,
            scale: 16_777_215.0 / 1000.0,
        };
        // A stored value of 100 world units reads back at or beyond 100.
        let d = (100.0f32 * 16_777.215).floor() / 16_777_215.0;
        let v = m.view_depth(d);
        assert!(v >= 99.999 && v <= 100.01, "{v}");
        assert_eq!(m.view_depth(1.0), f32::INFINITY);
        assert_eq!(m.view_depth(f32::NAN), f32::INFINITY);
        let p = DepthMapping::Projective {
            near: 0.1,
            far: 100.0,
        };
        assert!((p.view_depth(0.0) - 0.1).abs() < 1e-6);
        assert_eq!(p.view_depth(1.0), f32::INFINITY);
    }

    #[test]
    fn reduce_keeps_the_farthest_pixel_of_each_cell() {
        // 8x4 buffer into 4x2: each cell 2x2 pixels.
        let mut src = vec![1.0; 32];
        src[0] = 5.0; // cell (0,0)
        src[8 * 3 + 7] = 9.0; // cell (3,1)
        let g = DepthGrid::reduce(&src, 8, 4, 4, 2, DepthMapping::Linear);
        assert_eq!(g.at(0, 0), 5.0);
        assert_eq!(g.at(3, 1), 9.0);
        assert_eq!(g.at(1, 0), 1.0);
    }

    #[test]
    fn reduce_with_uneven_sizes_assigns_every_pixel_once() {
        // 7 pixels into 3 cells by centre: 0,0,1,1,1,2,2 (centres at
        // 0.5/7.. of the width).
        let cells: Vec<u32> = (0..7).map(|p| cell_of(p, 7, 3)).collect();
        assert_eq!(cells, vec![0, 0, 1, 1, 1, 2, 2]);
        // A grid finer than the buffer: empty cells are far.
        let g = DepthGrid::reduce(&[3.0, 4.0], 2, 1, 4, 1, DepthMapping::Linear);
        assert_eq!(g.depth, vec![f32::INFINITY, 3.0, f32::INFINITY, 4.0]);
    }

    #[test]
    fn pyramid_levels_hold_the_max_and_handle_odd_sizes() {
        let g = DepthGrid {
            width: 5,
            height: 3,
            depth: (0..15).map(|i| i as f32).collect(),
        };
        let h = HiZ::build(g.clone());
        let sizes: Vec<(u32, u32)> = h.levels().iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(sizes, vec![(5, 3), (3, 2), (2, 1), (1, 1)]);
        assert_eq!(h.levels()[1].at(0, 0), 6.0);
        assert_eq!(h.levels()[1].at(2, 1), 14.0);
        assert_eq!(h.levels()[3].at(0, 0), 14.0);
        // Every rectangle's pyramid max equals the brute-force max or is
        // farther (never nearer).
        for x0 in 0..5 {
            for x1 in x0..5 {
                for y0 in 0..3 {
                    for y1 in y0..3 {
                        let mut brute = f32::NEG_INFINITY;
                        for y in y0..=y1 {
                            for x in x0..=x1 {
                                brute = brute.max(g.at(x, y));
                            }
                        }
                        assert!(h.max_depth(x0, y0, x1, y1) >= brute);
                    }
                }
            }
        }
    }

    #[test]
    fn projection_bounds_the_sphere() {
        let c = cam();
        // Straight ahead at 10: centred, small.
        let f = c.project([10.0, 0.0, 0.0], 1.0, 64, 32).unwrap();
        assert!((f.near - 9.0).abs() < 1e-5);
        assert!(
            f.x0 <= 32 && f.x1 >= 31 && f.y0 <= 16 && f.y1 >= 15,
            "{f:?}"
        );
        assert!(f.x1 - f.x0 <= 8, "{f:?}");
        // To the left (+y is left when looking down +x with z up): the
        // left half of the grid.
        let f = c.project([10.0, 5.0, 0.0], 1.0, 64, 32).unwrap();
        assert!(f.x1 < 32, "{f:?}");
        // Above: the top half.
        let f = c.project([10.0, 0.0, 5.0], 1.0, 64, 32).unwrap();
        assert!(f.y1 < 16, "{f:?}");
        // Through the near plane, behind, or off screen: no footprint.
        assert!(c.project([0.5, 0.0, 0.0], 1.0, 64, 32).is_none());
        assert!(c.project([-10.0, 0.0, 0.0], 1.0, 64, 32).is_none());
        assert!(c.project([10.0, 30.0, 0.0], 1.0, 64, 32).is_none());
        // Bad input.
        assert!(c.project([f32::NAN, 0.0, 0.0], 1.0, 64, 32).is_none());
        assert!(c.project([10.0, 0.0, 0.0], f32::NAN, 64, 32).is_none());
    }

    /// Every sample point of the sphere projects inside the footprint.
    #[test]
    fn projection_contains_sampled_sphere_points() {
        let c = Camera::from_fov(
            [1.0, 2.0, 3.0],
            [0.3, 1.0, -0.2],
            [0.0, 0.0, 1.0],
            1.1,
            16.0 / 9.0,
            0.1,
        );
        let f = normalize(c.forward);
        let r = normalize(cross(f, c.up));
        let u = cross(r, f);
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        for _ in 0..300 {
            let center = [
                1.0 + rnd() * 20.0,
                2.0 + 15.0 + rnd() * 14.0,
                3.0 + rnd() * 6.0,
            ];
            let radius = 0.2 + (rnd() + 1.0) * 2.0;
            let Some(fp) = c.project(center, radius, 64, 32) else {
                continue;
            };
            for _ in 0..64 {
                let dir = normalize([rnd(), rnd(), rnd()]);
                let p = [
                    center[0] + dir[0] * radius,
                    center[1] + dir[1] * radius,
                    center[2] + dir[2] * radius,
                ];
                let d = sub(p, c.position);
                let (x, y, z) = (dot(d, r), dot(d, u), dot(d, f));
                assert!(z >= fp.near - 1e-4);
                let nx = x / z / c.tan_half_x;
                let ny = y / z / c.tan_half_y;
                if nx.abs() > 1.0 || ny.abs() > 1.0 {
                    continue; // off screen: no cell to cover
                }
                let cx = (((nx + 1.0) * 0.5 * 64.0).floor() as u32).min(63);
                let cy = (((1.0 - ny) * 0.5 * 32.0).floor() as u32).min(31);
                assert!(
                    cx >= fp.x0 && cx <= fp.x1 && cy >= fp.y0 && cy <= fp.y1,
                    "{p:?} {fp:?}"
                );
            }
        }
    }

    fn frame_of(buffer: &[f32], w: u32, h: u32, frame: u64) -> Frame {
        Frame {
            hiz: HiZ::build(DepthGrid::reduce(
                buffer,
                w,
                h,
                64,
                32,
                DepthMapping::Linear,
            )),
            camera: cam(),
            frame,
        }
    }

    #[test]
    fn box_behind_a_wall_is_occluded_and_beside_it_is_not() {
        // 256x128 buffer, a wall at depth 5 over the left 3/4 of the screen.
        let buf = wall_buffer(256, 128, 0, 192, 5.0);
        let f = frame_of(&buf, 256, 128, 1);
        let c = cam();
        let behind = c.project([20.0, 6.0, 0.0], 1.0, 64, 32).unwrap();
        assert_eq!(f.hiz.test(&behind, 0.25), Verdict::Occluded);
        // In front of the wall.
        let front = c.project([4.0, 1.0, 0.0], 0.5, 64, 32).unwrap();
        assert_eq!(f.hiz.test(&front, 0.25), Verdict::Visible);
        // Behind the wall's edge, poking past it to the right.
        let edge = c.project([20.0, -10.0, 0.0], 1.0, 64, 32).unwrap();
        assert_eq!(f.hiz.test(&edge, 0.25), Verdict::Visible);
        // Wholly in the open right quarter.
        let open = c.project([20.0, -14.0, 0.0], 1.0, 64, 32).unwrap();
        assert_eq!(f.hiz.test(&open, 0.25), Verdict::Visible);
        // Just behind the wall but within the bias: kept.
        let close = c.project([6.1, 1.0, 0.0], 0.9, 64, 32).unwrap();
        assert!(close.near > 5.0);
        assert_eq!(f.hiz.test(&close, 0.25), Verdict::Visible);
    }

    #[test]
    fn a_one_pixel_hole_keeps_the_object() {
        let mut buf = wall_buffer(256, 128, 0, 256, 5.0);
        // A single far pixel right where the object is.
        buf[(64 * 256 + 128) as usize] = f32::INFINITY;
        let f = frame_of(&buf, 256, 128, 1);
        let fp = cam().project([20.0, 0.0, 0.0], 0.5, 64, 32).unwrap();
        assert_eq!(f.hiz.test(&fp, 0.25), Verdict::Visible);
    }

    #[test]
    fn tracker_skips_on_the_second_occluded_frame_and_resets_on_visible() {
        let buf = wall_buffer(256, 128, 0, 256, 5.0);
        let mut t = Tracker::new(Rules::default());
        let b = Bounds {
            key: 0x1234_0007,
            center: [20.0, 0.0, 0.0],
            radius: 1.0,
            exempt: false,
        };
        // Frame 1: no depth yet.
        t.begin(1, cam(), None);
        assert!(!t.decide(&b));
        // Frame 2: occluded once: not yet.
        t.begin(2, cam(), Some(frame_of(&buf, 256, 128, 1)));
        assert!(!t.decide(&b));
        // A second test in the same frame does not count twice.
        assert!(!t.decide(&b));
        // Frame 3: second occluded frame: skip.
        t.begin(3, cam(), Some(frame_of(&buf, 256, 128, 2)));
        assert!(t.decide(&b));
        // Frame 4: the wall is gone: drawn again at once.
        let open = vec![f32::INFINITY; 256 * 128];
        t.begin(4, cam(), Some(frame_of(&open, 256, 128, 3)));
        assert!(!t.decide(&b));
        t.begin(5, cam(), Some(frame_of(&buf, 256, 128, 4)));
        assert!(!t.decide(&b));
        assert_eq!(t.counts.culled, 1);
        assert_eq!(t.counts.tested, 6);
    }

    #[test]
    fn tracker_never_skips_exempt_large_near_or_stale() {
        let buf = wall_buffer(256, 128, 0, 256, 5.0);
        let mut t = Tracker::new(Rules::default());
        let mk = |key, center, radius, exempt| Bounds {
            key,
            center,
            radius,
            exempt,
        };
        let objs = [
            mk(1, [20.0, 0.0, 0.0], 1.0, true),     // the player
            mk(2, [200.0, 0.0, 0.0], 150.0, false), // too large
            mk(3, [0.5, 0.0, 0.0], 1.0, false),     // reaches the near plane
            mk(4, [20.0, 0.0, 0.0], f32::NAN, false),
        ];
        for frame in 1..6 {
            t.begin(frame, cam(), Some(frame_of(&buf, 256, 128, frame - 1)));
            for o in &objs {
                assert!(!t.decide(o), "{o:?}");
            }
        }
        // Stale depth (three frames old) culls nothing.
        let ok = mk(5, [20.0, 0.0, 0.0], 1.0, false);
        for frame in 10..14 {
            t.begin(frame, cam(), Some(frame_of(&buf, 256, 128, frame - 3)));
            assert!(!t.decide(&ok));
        }
    }

    #[test]
    fn camera_cut_resets_streaks_and_drops_the_old_depth() {
        let buf = wall_buffer(256, 128, 0, 256, 5.0);
        let mut t = Tracker::new(Rules::default());
        let b = Bounds {
            key: 9,
            center: [20.0, 0.0, 0.0],
            radius: 1.0,
            exempt: false,
        };
        t.begin(1, cam(), Some(frame_of(&buf, 256, 128, 0)));
        t.decide(&b);
        t.begin(2, cam(), Some(frame_of(&buf, 256, 128, 1)));
        assert!(t.decide(&b));
        // Teleport: the depth drawn from the old position is not used.
        let mut moved = cam();
        moved.position = [0.0, 50.0, 0.0];
        t.begin(3, moved, Some(frame_of(&buf, 256, 128, 2)));
        assert!(!t.decide(&b));
        assert_eq!(t.counts.resets, 1);
        // Turned around.
        let mut turned = cam();
        turned.forward = [-1.0, 0.0, 0.0];
        t.begin(4, turned, None);
        assert_eq!(t.counts.resets, 2);
    }

    #[test]
    fn camera_motion_inflates_the_bounds() {
        // An object just behind a wall's edge, occluded from the depth's
        // camera; with the current camera moved sideways the inflated
        // sphere crosses the edge and is kept.
        let buf = wall_buffer(256, 128, 0, 128, 5.0);
        let b = Bounds {
            key: 1,
            center: [20.0, 3.0, 0.0],
            radius: 0.5,
            exempt: false,
        };
        let mut still = Tracker::new(Rules::default());
        still.begin(1, cam(), Some(frame_of(&buf, 256, 128, 0)));
        still.decide(&b);
        still.begin(2, cam(), Some(frame_of(&buf, 256, 128, 1)));
        assert!(still.decide(&b));
        let mut moving = Tracker::new(Rules::default());
        let mut c = cam();
        moving.begin(1, c, Some(frame_of(&buf, 256, 128, 0)));
        moving.decide(&b);
        c.position = [0.0, -3.0, 0.0];
        moving.begin(2, c, Some(frame_of(&buf, 256, 128, 1)));
        assert!(!moving.decide(&b));
    }
}
