//! What a recorded frame's draws do with matrices, how draws pair up across
//! two frames, and the constants an in-between frame substitutes.
//!
//! Nothing here knows Halo 2's register map. The vertex constants of each
//! draw are searched for matrices, and a matrix is only rewritten when it
//! can be rebuilt from its parts (docs/game/high-framerate.md, "Rules that
//! keep it safe"); everything else replays as the frame drew it:
//!
//! - **Perspective blocks**: four consecutive registers that split into a
//!   projection and a rigid camera ([`Persp::split`]), as rows (`dp4` per
//!   register) or as columns.
//! - **The view's camera**: per view (viewport and target size), the
//!   perspective block value most draws share, bit for bit, used by at
//!   least [`MIN_CAMERA_DRAWS`] draws. Every block equal to it is
//!   [`Kind::Camera`]: the world (level geometry, decals, particles)
//!   follows the in-between camera.
//! - **Objects drawn with the camera's projection**: a perspective block
//!   that is the camera's view-projection times a rotation, uniform scale
//!   and translation is [`Kind::World`] (the world matrix recovered with
//!   the camera's inverse, then checked by rebuilding).
//! - **Draws with a projection of their own** (the first-person weapon and
//!   hands): [`Kind::Local`], interpolated in camera space.
//! - **Node matrices**: in draws that use the camera, three registers that
//!   form a rotation, uniform scale and translation (rows) are
//!   [`Kind::Node`]: skinned and rigid object nodes in world space.
//! - **The eye**: in draws that use the camera, a register whose `xyz` is
//!   the camera's position (and `w` 0 or 1) is [`Kind::Eye`], so fog,
//!   specular and sky follow the in-between camera too.
//!
//! Draws pair up across the two frames by a signature (view, vertex
//! program, primitive, counts, strides, texture headers), and among draws
//! with the same signature, nearest anchor (world position) first.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::camera::Persp;
use super::epilogue::Position;
use super::math::{Mat4, Similarity, inverse, len3, max_abs, max_diff, mul, sub3};
use crate::backend::ResolvedDraw;
use crate::shader::{CONSTANTS, Constants, VertexProcessing};

/// A camera value must be shared by this many draws of its view.
pub const MIN_CAMERA_DRAWS: usize = 2;

/// How close an object's rebuilt world-view-projection must come to the
/// uploaded one, relative to its size.
pub const OBJECT_TOLERANCE: f64 = 1e-4;

/// Thresholds for snapping instead of blending (Halo world units, about
/// three metres each; radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// The camera's eye moved further than this in one frame: a cut.
    pub cut_distance: f64,
    /// The camera turned more than this in one frame: a cut.
    pub cut_angle: f64,
    /// The projection's terms changed by more than this, relatively: a cut
    /// (scope zoom, cinematic lens).
    pub cut_zoom: f64,
    /// An object moved further than this in one frame: a teleport.
    pub jump_distance: f64,
    /// An object turned more than this in one frame: a teleport.
    pub jump_angle: f64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            cut_distance: 3.0,
            cut_angle: 1.0,
            cut_zoom: 0.05,
            jump_distance: 2.0,
            jump_angle: 1.6,
        }
    }
}

/// How a block's registers hold the matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layout {
    /// Register `r + i` is row `i` (`dp4` against the vertex).
    Rows,
    /// Register `r + j` is column `j`.
    Columns,
}

/// Which view a draw belongs to: its viewport and target size. (A pass
/// and view tag from the Rust render loop can refine this later.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewKey {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub target: (u32, u32),
}

impl ViewKey {
    /// The view `draw` belongs to.
    pub fn of(draw: &ResolvedDraw) -> Self {
        Self {
            x: draw.viewport.x,
            y: draw.viewport.y,
            width: draw.viewport.width,
            height: draw.viewport.height,
            target: (draw.target.width, draw.target.height),
        }
    }
}

/// What a matrix block is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// The view's camera, exactly.
    Camera,
    /// The view's camera times this object (world) transform.
    World(Similarity),
    /// A perspective matrix with a projection of its own.
    Local(Persp),
    /// A world-space node matrix (three rows).
    Node(Similarity),
    /// The camera's eye position (one register, `xyz`; fog, specular and
    /// sky shaders read it).
    Eye,
}

/// A matrix found in a draw's constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    /// First register (constant index, hardware order: register + 96).
    pub reg: u8,
    pub layout: Layout,
    pub kind: Kind,
}

/// One recorded draw, as the replay sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawModel {
    pub view: ViewKey,
    /// What the draw's program computes its clip position from.
    pub position: Position,
    /// Identity across frames (see the module docs).
    pub sig: u64,
    pub blocks: Vec<Block>,
    /// World position, for pairing instances of the same mesh.
    pub anchor: Option<[f64; 3]>,
}

/// A view's camera in one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewCamera {
    pub view: ViewKey,
    pub camera: Persp,
    /// The view-projection as uploaded (bits), which identifies its blocks.
    pub raw: [u32; 16],
}

impl DrawModel {
    /// The block the draw's clip position is computed from, if it is one
    /// of its perspective blocks.
    pub fn position_block(&self) -> Option<&Block> {
        let Position::Rows(reg) = self.position else {
            return None;
        };
        self.blocks.iter().find(|b| {
            b.reg == reg && b.layout == Layout::Rows && !matches!(b.kind, Kind::Node(_) | Kind::Eye)
        })
    }

    /// Whether the camera places the draw's vertices: its position block is
    /// the camera or the camera times an object transform. A draw whose
    /// position is not known is taken to if any of its blocks is.
    pub fn placed_by_camera(&self) -> bool {
        let camera = |k: &Kind| matches!(k, Kind::Camera | Kind::World(_));
        match self.position {
            Position::Rows(_) => self.position_block().is_some_and(|b| camera(&b.kind)),
            Position::Fixed => false,
            Position::Unknown => self.blocks.iter().any(|b| camera(&b.kind)),
        }
    }
}

/// One frame's draws, classified.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameModel {
    pub cameras: Vec<ViewCamera>,
    pub draws: Vec<DrawModel>,
}

impl FrameModel {
    /// The camera of `view`, if it has one.
    pub fn camera(&self, view: ViewKey) -> Option<&ViewCamera> {
        self.cameras.iter().find(|c| c.view == view)
    }

    /// Whether any draw has a matrix the replay could move.
    pub fn has_matrices(&self) -> bool {
        self.draws.iter().any(|d| !d.blocks.is_empty())
    }

    /// Classifies `draws` (the frame's draws into the presented surface, in
    /// order). `positions[i]` is what draw `i`'s program computes its clip
    /// position from ([`position`](super::epilogue::position)).
    ///
    /// A view's camera is the value most draws compute their position from:
    /// a block a draw does not put its vertices through is never its camera,
    /// however many draws share it (constants that only look like a
    /// projection: a light's or a texture's matrix, packed parameters).
    /// Draws whose position does not come from one block's rows do not vote.
    pub fn build(draws: &[&ResolvedDraw], positions: &[Position]) -> FrameModel {
        // Perspective blocks per distinct constant block.
        let mut found: HashMap<*const [[f32; 4]; CONSTANTS], std::rc::Rc<Vec<Candidate>>> =
            HashMap::new();
        let per_draw: Vec<std::rc::Rc<Vec<Candidate>>> = draws
            .iter()
            .map(|d| {
                if d.layout.pretransformed {
                    return std::rc::Rc::new(Vec::new());
                }
                let c = &d.shader.constants;
                found
                    .entry(std::sync::Arc::as_ptr(c))
                    .or_insert_with(|| std::rc::Rc::new(perspective_blocks(c)))
                    .clone()
            })
            .collect();
        // Each view's camera: the perspective value most draws compute
        // their position from.
        let mut counts: HashMap<(ViewKey, [u32; 16]), (usize, usize)> = HashMap::new();
        for (i, (d, cands)) in draws.iter().zip(&per_draw).enumerate() {
            let Some(&Position::Rows(reg)) = positions.get(i) else {
                continue;
            };
            if let Some(c) = cands
                .iter()
                .find(|c| c.reg == reg && c.layout == Layout::Rows)
            {
                counts.entry((ViewKey::of(d), c.raw)).or_insert((0, i)).0 += 1;
            }
        }
        let mut best: HashMap<ViewKey, ([u32; 16], usize, usize)> = HashMap::new();
        for (&(view, raw), &(n, first)) in &counts {
            let e = best.entry(view).or_insert((raw, n, first));
            if n > e.1 || (n == e.1 && first < e.2) {
                *e = (raw, n, first);
            }
        }
        let mut cameras: Vec<ViewCamera> = Vec::new();
        for (view, (raw, n, first)) in best {
            if n < MIN_CAMERA_DRAWS {
                continue;
            }
            let camera = per_draw[first]
                .iter()
                .find(|c| c.raw == raw)
                .map(|c| c.persp)
                .expect("the camera's block");
            cameras.push(ViewCamera { view, camera, raw });
        }
        cameras.sort_by_key(|c| (c.view.y, c.view.x, c.view.width, c.view.height));
        let models = draws
            .iter()
            .zip(&per_draw)
            .enumerate()
            .map(|(i, (d, cands))| {
                let view = ViewKey::of(d);
                let cam = cameras.iter().find(|c| c.view == view);
                let position = positions.get(i).copied().unwrap_or(Position::Unknown);
                classify(d, cands, cam, view, position)
            })
            .collect();
        FrameModel {
            cameras,
            draws: models,
        }
    }
}

/// A perspective block found in a constant block.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    reg: u8,
    layout: Layout,
    matrix: Mat4,
    persp: Persp,
    raw: [u32; 16],
}

/// Reads the 4x4 matrix at `reg`.
pub fn read4(c: &[[f32; 4]; CONSTANTS], reg: usize, layout: Layout) -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = match layout {
                Layout::Rows => c[reg + i][j],
                Layout::Columns => c[reg + j][i],
            } as f64;
        }
    }
    m
}

/// Writes the first `rows` rows of `m` at `reg`.
pub fn write(c: &mut [[f32; 4]; CONSTANTS], reg: usize, layout: Layout, rows: usize, m: &Mat4) {
    for (i, row) in m.iter().enumerate().take(rows) {
        for (j, v) in row.iter().enumerate() {
            match layout {
                Layout::Rows => c[reg + i][j] = *v as f32,
                Layout::Columns => c[reg + j][i] = *v as f32,
            }
        }
    }
}

fn raw_bits(c: &[[f32; 4]; CONSTANTS], reg: usize, layout: Layout) -> [u32; 16] {
    let mut r = [0; 16];
    for i in 0..4 {
        for j in 0..4 {
            r[4 * i + j] = match layout {
                Layout::Rows => c[reg + i][j],
                Layout::Columns => c[reg + j][i],
            }
            .to_bits();
        }
    }
    r
}

/// Every perspective block, scanning upward and skipping the registers of
/// each one found.
fn perspective_blocks(c: &[[f32; 4]; CONSTANTS]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut r = 0;
    while r + 4 <= CONSTANTS {
        let mut hit = None;
        for layout in [Layout::Rows, Layout::Columns] {
            // Cheap rejection first: the w row must not be zero in xyz,
            // and the depth row must be parallel to it (checked in split).
            let m = read4(c, r, layout);
            if m[3][0] == 0.0 && m[3][1] == 0.0 && m[3][2] == 0.0 {
                continue;
            }
            if let Some(persp) = Persp::split(&m) {
                hit = Some(Candidate {
                    reg: r as u8,
                    layout,
                    matrix: m,
                    persp,
                    raw: raw_bits(c, r, layout),
                });
                break;
            }
        }
        match hit {
            Some(h) => {
                out.push(h);
                r += 4;
            }
            None => r += 1,
        }
    }
    out
}

fn classify(
    d: &ResolvedDraw,
    cands: &[Candidate],
    cam: Option<&ViewCamera>,
    view: ViewKey,
    position: Position,
) -> DrawModel {
    let mut blocks = Vec::new();
    let mut uses_camera = false;
    let inv = cam.and_then(|c| inverse(&c.camera.matrix()));
    for c in cands {
        let kind = match cam {
            Some(v) if v.raw == c.raw => {
                uses_camera = true;
                Kind::Camera
            }
            Some(v) => match inv
                .as_ref()
                .and_then(|i| world_of(i, &v.camera.matrix(), &c.matrix))
            {
                Some(w) => Kind::World(w),
                None => Kind::Local(c.persp),
            },
            None => Kind::Local(c.persp),
        };
        blocks.push(Block {
            reg: c.reg,
            layout: c.layout,
            kind,
        });
    }
    if uses_camera {
        let taken: Vec<(usize, usize)> = cands
            .iter()
            .map(|c| (c.reg as usize, c.reg as usize + 4))
            .collect();
        let c = &d.shader.constants;
        let mut r = 0;
        while r + 3 <= CONSTANTS {
            if let Some(&(_, end)) = taken.iter().find(|&&(s, e)| r + 3 > s && r < e) {
                r = end;
                continue;
            }
            match node_at(c, r) {
                Some(s) => {
                    blocks.push(Block {
                        reg: r as u8,
                        layout: Layout::Rows,
                        kind: Kind::Node(s),
                    });
                    r += 3;
                }
                None => r += 1,
            }
        }
        if let Some(v) = cam {
            let eye = v.camera.eye();
            let tol = 1e-4 * (1.0 + len3(eye));
            let taken: Vec<(usize, usize)> = blocks
                .iter()
                .map(|b| {
                    let n = if matches!(b.kind, Kind::Node(_)) {
                        3
                    } else {
                        4
                    };
                    (b.reg as usize, b.reg as usize + n)
                })
                .collect();
            for (r, reg) in c.iter().enumerate() {
                if taken.iter().any(|&(s, e)| r >= s && r < e) {
                    continue;
                }
                let p = [reg[0] as f64, reg[1] as f64, reg[2] as f64];
                // An empty register is not the eye, even with the eye at
                // the origin.
                let empty = reg[0] == 0.0 && reg[1] == 0.0 && reg[2] == 0.0;
                if !empty && (reg[3] == 0.0 || reg[3] == 1.0) && len3(sub3(p, eye)) <= tol {
                    blocks.push(Block {
                        reg: r as u8,
                        layout: Layout::Rows,
                        kind: Kind::Eye,
                    });
                }
            }
        }
    }
    let anchor = blocks.iter().find_map(|b| match b.kind {
        Kind::World(s) | Kind::Node(s) => Some(s.translation),
        _ => None,
    });
    DrawModel {
        view,
        position,
        sig: signature(d, view),
        blocks,
        anchor,
    }
}

/// The world transform `W` with `camera * W == m`, if it is a similarity
/// and rebuilds `m`.
fn world_of(camera_inv: &Mat4, camera: &Mat4, m: &Mat4) -> Option<Similarity> {
    let w = Similarity::from_affine(&mul(camera_inv, m), 1e-2)?;
    if !(1e-3..=1e3).contains(&w.scale) {
        return None;
    }
    (max_diff(&mul(camera, &w.to_mat4()), m) <= OBJECT_TOLERANCE * max_abs(m)).then_some(w)
}

/// A node matrix (three rows) at `r`.
fn node_at(c: &[[f32; 4]; CONSTANTS], r: usize) -> Option<Similarity> {
    let m = [
        c[r].map(f64::from),
        c[r + 1].map(f64::from),
        c[r + 2].map(f64::from),
        [0.0, 0.0, 0.0, 1.0],
    ];
    let s = Similarity::from_affine(&m, 1e-4)?;
    if !(1e-3..=1e3).contains(&s.scale) {
        return None;
    }
    (max_diff(&s.to_mat4(), &m) <= 1e-5 * max_abs(&m)).then_some(s)
}

fn signature(d: &ResolvedDraw, view: ViewKey) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    view.hash(&mut h);
    match &d.shader.vertex {
        VertexProcessing::FixedFunction { fvf } => (0u8, *fvf as u64).hash(&mut h),
        VertexProcessing::Program { microcode, .. } => (1u8, microcode.words_hash()).hash(&mut h),
    }
    d.primitive.hash(&mut h);
    d.vertex_count.hash(&mut h);
    d.indices.as_ref().map(|i| i.len()).hash(&mut h);
    for s in &d.streams {
        (s.stride, s.data.len()).hash(&mut h);
    }
    d.textures.hash(&mut h);
    h.finish()
}

/// For each draw of `cur`, the draw of `prev` it continues, if any.
pub fn match_draws(prev: &FrameModel, cur: &FrameModel) -> Vec<Option<usize>> {
    let mut groups: HashMap<u64, (Vec<usize>, Vec<usize>)> = HashMap::new();
    for (i, d) in prev.draws.iter().enumerate() {
        groups.entry(d.sig).or_default().0.push(i);
    }
    for (i, d) in cur.draws.iter().enumerate() {
        groups.entry(d.sig).or_default().1.push(i);
    }
    let mut out = vec![None; cur.draws.len()];
    for (p, c) in groups.values() {
        if p.is_empty() || c.is_empty() {
            continue;
        }
        let anchored = p.iter().all(|&i| prev.draws[i].anchor.is_some())
            && c.iter().all(|&i| cur.draws[i].anchor.is_some());
        if p.len() == 1 && c.len() == 1 || !anchored {
            // In draw order.
            for (&ci, &pi) in c.iter().zip(p) {
                out[ci] = Some(pi);
            }
            continue;
        }
        // Nearest pairs first.
        let mut pairs: Vec<(f64, usize, usize)> = Vec::with_capacity(p.len() * c.len());
        for &ci in c {
            for &pi in p {
                let d = len3(sub3(
                    cur.draws[ci].anchor.unwrap(),
                    prev.draws[pi].anchor.unwrap(),
                ));
                pairs.push((d, ci, pi));
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut used = std::collections::HashSet::new();
        for (_, ci, pi) in pairs {
            if out[ci].is_none() && !used.contains(&pi) {
                out[ci] = Some(pi);
                used.insert(pi);
            }
        }
    }
    out
}

/// What [`substitute`] did, for statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Substituted {
    /// Draws whose constants changed.
    pub draws: usize,
    /// Views whose camera was interpolated.
    pub cameras: usize,
    /// Object blocks blended between the two frames.
    pub blended: usize,
    /// Views or blocks snapped (a cut or a teleport).
    pub snapped: usize,
}

/// The constants each draw of `cur` replays with at `alpha` between `prev`
/// (0) and `cur` (1): `None` keeps the draw's own. At `alpha >= 1` every
/// draw keeps its own, so the replay is frame `cur` exactly.
pub fn substitute(
    prev: &FrameModel,
    cur: &FrameModel,
    matches: &[Option<usize>],
    alpha: f32,
    tuning: &Tuning,
    constants: &dyn Fn(usize) -> Constants,
) -> (Vec<Option<Constants>>, Substituted) {
    let mut stats = Substituted::default();
    let mut out = vec![None; cur.draws.len()];
    if alpha >= 1.0 || alpha.is_nan() {
        return (out, stats);
    }
    let t = alpha.max(0.0) as f64;
    // Each view's in-between camera, or None (no camera in one of the
    // frames, or a cut: the view replays at frame `cur`).
    let cams: Vec<(ViewKey, Option<Persp>, Persp)> = cur
        .cameras
        .iter()
        .map(|c| {
            let between = prev.camera(c.view).and_then(|p| {
                if p.raw == c.raw {
                    // Not moved: the uploaded value stands.
                    None
                } else if p.camera.cut(
                    &c.camera,
                    tuning.cut_distance,
                    tuning.cut_angle,
                    tuning.cut_zoom,
                ) {
                    stats.snapped += 1;
                    None
                } else {
                    stats.cameras += 1;
                    Some(p.camera.lerp_camera(&c.camera, t))
                }
            });
            (c.view, between, c.camera)
        })
        .collect();
    for (i, d) in cur.draws.iter().enumerate() {
        if d.blocks.is_empty() {
            continue;
        }
        let cam = cams.iter().find(|c| c.0 == d.view);
        let between_cam = cam.and_then(|c| c.1);
        let prev_draw = matches.get(i).copied().flatten().map(|p| &prev.draws[p]);
        let prev_block = |b: &Block| {
            prev_draw.and_then(|p| {
                p.blocks
                    .iter()
                    .find(|q| q.reg == b.reg && q.layout == b.layout)
                    .map(|q| q.kind)
            })
        };
        let mut writes: Vec<(usize, Layout, usize, Mat4)> = Vec::new();
        for b in &d.blocks {
            match b.kind {
                Kind::Camera => {
                    if let Some(c) = between_cam {
                        writes.push((b.reg as usize, b.layout, 4, c.matrix()));
                    }
                }
                Kind::World(w) => {
                    let w_between = match prev_block(b) {
                        Some(Kind::World(p))
                            if !p.jumps(&w, tuning.jump_distance, tuning.jump_angle) =>
                        {
                            stats.blended += 1;
                            Some(p.lerp(&w, t))
                        }
                        Some(Kind::World(_)) => {
                            stats.snapped += 1;
                            None
                        }
                        _ => None,
                    };
                    if between_cam.is_none() && w_between.is_none() {
                        continue;
                    }
                    let vp = between_cam
                        .or(cam.map(|c| c.2))
                        .expect("a World block has a camera");
                    let w = w_between.unwrap_or(w);
                    writes.push((b.reg as usize, b.layout, 4, mul(&vp.matrix(), &w.to_mat4())));
                }
                Kind::Local(l) => match prev_block(b) {
                    Some(Kind::Local(p))
                        if !p.cut(&l, tuning.jump_distance, tuning.jump_angle, tuning.cut_zoom) =>
                    {
                        stats.blended += 1;
                        writes.push((b.reg as usize, b.layout, 4, p.lerp_local(&l, t).matrix()));
                    }
                    Some(Kind::Local(_)) => stats.snapped += 1,
                    _ => {}
                },
                Kind::Eye => {
                    if let Some(c) = between_cam {
                        let e = c.eye();
                        let mut m = [[0.0; 4]; 4];
                        m[0] = [e[0], e[1], e[2], constants(i)[b.reg as usize][3] as f64];
                        writes.push((b.reg as usize, Layout::Rows, 1, m));
                    }
                }
                Kind::Node(n) => match prev_block(b) {
                    Some(Kind::Node(p)) if p != n => {
                        if p.jumps(&n, tuning.jump_distance, tuning.jump_angle) {
                            stats.snapped += 1;
                        } else {
                            stats.blended += 1;
                            writes.push((b.reg as usize, Layout::Rows, 3, p.lerp(&n, t).to_mat4()));
                        }
                    }
                    _ => {}
                },
            }
        }
        if writes.is_empty() {
            continue;
        }
        let mut block = *constants(i);
        for (reg, layout, rows, m) in writes {
            write(&mut block, reg, layout, rows, &m);
        }
        let mut c = crate::shader::spare_constants();
        *std::sync::Arc::get_mut(&mut c).expect("a spare block is unshared") = block;
        out[i] = Some(c);
        stats.draws += 1;
    }
    (out, stats)
}
