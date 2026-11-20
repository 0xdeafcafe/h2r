//! A map's coarse visibility grid (docs/game/bots.md §3.1, "Visibility
//! grid"): which pairs of cells of walkable space can never see each
//! other, so a sight ray between them need not be cast.
//!
//! The grid cuts the space a player's eye or body can be in (just above
//! the nav mesh's polygons) into boxes, and keeps one bit for each pair of
//! boxes: set when **no** segment from any point of one to any point of
//! the other is clear ([`Walls::clear`] says `false` for every one of
//! them), unset ("maybe") otherwise or when that could not be proved.
//! [`Walls::clear`] asks the attached grid first and walks the collision
//! only for "maybe" pairs and points outside the grid, so a match plays
//! exactly as it does without the grid, with fewer rays.
//!
//! **Proof.** A pair is "never" only when, for every piece of an adaptive
//! split of the two boxes, a single collision triangle lies between them
//! with margins (`covers`): both boxes strictly on opposite sides of its
//! plane, and every crossing of the plane by a segment between the boxes
//! at least [`EDGE`] inside the triangle. The crossings of segments
//! between two convex boxes lie in the convex hull of the 64 crossings of
//! corner-to-corner segments (a central projection keeps a polytope's
//! hull), so checking those 64 points proves it for every segment. The
//! margins keep the proof true for `clear`'s `f32` Moller-Trumbore test.
//! Two triangles sharing an edge are never combined: that test is not
//! watertight along a shared edge, so a ray through one could slip
//! between them. Whatever cannot be proved within the budget stays
//! "maybe".
//!
//! **File.** `<data>/bots/<map>.<bsp>.vis`, written by the nav worker
//! (`crate::autonav`) beside the `.bots` file it is built from: magic
//! `H2VS`, [`VERSION`], that file's [`Stamp`], a hash of its walls, then
//! the grid. A missing, damaged, older or mismatched file is not used and
//! the log says so ([`load`]); every ray is then cast.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::cache::{self, MapData, Stamp};
use crate::los::{V3, Walls};
use crate::nav::NavMesh;

pub const MAGIC: &[u8; 4] = b"H2VS";
/// The file format and the build's version. **Bump it whenever what
/// [`build`] makes changes** (the cells, the proof, the margins): every
/// `.vis` of another version is built again.
pub const VERSION: u32 = 1;
/// Magic, version, the `.bots` stamp, the walls hash.
pub const HEADER_LEN: usize = 28;

/// At most this many cells; coarser cells are used until a map fits.
/// Pairs grow with its square: 2048 cells is 2.1 M pairs, 262 KB of bits.
pub const MAX_CELLS: usize = 2048;
/// The first cell size tried, world units (one is about 3 m): across, up.
const FIRST: [f32; 2] = [1.0, 0.5];
/// Each try after makes the cells this much bigger.
const GROW: f32 = 1.25;
/// The voxel table's largest size, entries.
const MAX_VOXELS: usize = 1 << 22;
/// A walkable surface makes the space from this far below it to this far
/// above it part of the grid: feet to above the eye (`mind::EYE` is 0.6).
const BELOW: f32 = 0.3;
const ABOVE: f32 = 0.9;
/// Walkable polygons are sampled this finely (world units).
const SAMPLE: f32 = 0.5;

/// A cell's box is widened by this in the proof: a point's cell is found
/// in `f32`, and may sit a rounding outside its box.
const PAD: f64 = 0.01;
/// Each box at least this far from an occluder's plane.
const PLANE_GAP: f64 = 0.005;
/// Every crossing at least this far (of the segment) from either end:
/// `clear` ignores hits within 1e-4 of an end.
const T_END: f64 = 1e-3;
/// Every segment at least this steep against the occluder's plane
/// (cosine to its normal): a grazing ray's crossing is imprecise.
const MIN_COS: f64 = 0.05;
/// Every crossing at least this far inside each of the triangle's edges,
/// plus [`EDGE_PER_LENGTH`] of the longest segment.
const EDGE: f64 = 0.01;
const EDGE_PER_LENGTH: f64 = 2e-5;
/// The proof of one pair splits its boxes into at most this many pieces,
/// at most this many times deep, before it gives up ("maybe").
const LEAVES: u32 = 24;
const DEPTH: u32 = 8;
/// The occluders a piece tries: its centre ray's and its parents'.
const CANDIDATES: usize = 24;

const NONE: u16 = u16::MAX;

/// A map's visibility grid. Shared read-only (`Arc`) by every bot.
#[derive(Clone, Default, PartialEq)]
pub struct Vis {
    origin: V3,
    /// Cell size: x, y, z.
    size: V3,
    dims: [u32; 3],
    /// Each voxel's cell, [`NONE`] for none; x fastest.
    index: Vec<u16>,
    /// The occupied voxels, ascending; cell `i` is voxel `voxels[i]`.
    voxels: Vec<u32>,
    /// The upper triangle of the cell-pair matrix ([`pair_bit`]): set,
    /// never visible.
    bits: Vec<u64>,
    /// [`walls_hash`] of the walls it was built from.
    walls: u64,
}

impl std::fmt::Debug for Vis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Vis({})", self.summary())
    }
}

/// The bit of pair `i < j` among `n` cells, row by row.
fn pair_bit(n: usize, i: usize, j: usize) -> usize {
    debug_assert!(i < j && j < n);
    i * (2 * n - i - 1) / 2 + (j - i - 1)
}

fn pairs(n: usize) -> usize {
    n * n.saturating_sub(1) / 2
}

/// FNV-1a (64-bit) over every triangle's coordinates' bits.
pub fn walls_hash(walls: &Walls) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in walls.tris.iter().flatten().flatten() {
        for b in f.to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

impl Vis {
    /// The cell holding `p`, if any.
    #[inline]
    pub fn cell(&self, p: V3) -> Option<u32> {
        let mut v = [0u32; 3];
        for k in 0..3 {
            let q = (p[k] - self.origin[k]) / self.size[k];
            // Also false for NaN: no cell, the ray is cast.
            if !(0.0..self.dims[k] as f32).contains(&q) {
                return None;
            }
            v[k] = (q as u32).min(self.dims[k] - 1);
        }
        let i = (v[2] as usize * self.dims[1] as usize + v[1] as usize) * self.dims[0] as usize
            + v[0] as usize;
        let c = *self.index.get(i)?;
        (c != NONE).then_some(u32::from(c))
    }

    /// Whether the segment `a`-`b` is certainly blocked: both ends in
    /// cells of a pair that never see each other.
    #[inline]
    pub fn never(&self, a: V3, b: V3) -> bool {
        let (Some(i), Some(j)) = (self.cell(a), self.cell(b)) else {
            return false;
        };
        if i == j {
            return false;
        }
        let (i, j) = (i.min(j) as usize, i.max(j) as usize);
        let bit = pair_bit(self.voxels.len(), i, j);
        (self.bits[bit / 64] >> (bit % 64)) & 1 == 1
    }

    /// Whether it was built from `walls` (their [`walls_hash`]).
    pub fn fits(&self, walls: &Walls) -> bool {
        self.walls == walls_hash(walls)
    }

    pub fn cells(&self) -> usize {
        self.voxels.len()
    }

    /// How many cell pairs never see each other.
    pub fn never_pairs(&self) -> u64 {
        self.bits.iter().map(|w| u64::from(w.count_ones())).sum()
    }

    /// For the log: `812 cells of 1.0 x 0.5, 41.2% of pairs never visible`.
    pub fn summary(&self) -> String {
        let n = pairs(self.cells());
        format!(
            "{} cells of {:.2} x {:.2}, {:.1}% of pairs never visible",
            self.cells(),
            self.size[0],
            self.size[2],
            if n == 0 { 0.0 } else { self.never_pairs() as f64 * 100.0 / n as f64 }
        )
    }

    fn voxel_box(&self, voxel: u32) -> Box3 {
        let (dx, dy) = (self.dims[0], self.dims[1]);
        let v = [voxel % dx, voxel / dx % dy, voxel / (dx * dy)];
        let lo: [f64; 3] = std::array::from_fn(|k| {
            f64::from(self.origin[k]) + f64::from(v[k]) * f64::from(self.size[k]) - PAD
        });
        let hi: [f64; 3] = std::array::from_fn(|k| {
            f64::from(self.origin[k]) + f64::from(v[k] + 1) * f64::from(self.size[k]) + PAD
        });
        Box3 { lo, hi }
    }

    fn with_voxels(origin: V3, size: V3, dims: [u32; 3], voxels: Vec<u32>, walls: u64) -> Option<Self> {
        let total = dims.iter().try_fold(1usize, |a, &d| a.checked_mul(d as usize))?;
        if total == 0 || total > MAX_VOXELS || voxels.len() >= usize::from(NONE) {
            return None;
        }
        let mut index = vec![NONE; total];
        for (c, &v) in voxels.iter().enumerate() {
            *index.get_mut(v as usize)? = c as u16;
        }
        if !voxels.windows(2).all(|w| w[0] < w[1]) {
            return None;
        }
        Some(Vis {
            origin,
            size,
            dims,
            index,
            bits: vec![0; pairs(voxels.len()).div_ceil(64)],
            voxels,
            walls,
        })
    }

    /// The file: header ([`HEADER_LEN`]), then origin, size, dims, the
    /// cell count, the occupied voxels and the pair bits.
    pub fn to_bytes(&self, stamp: Stamp) -> Vec<u8> {
        let mut o = Vec::with_capacity(HEADER_LEN + 40 + 4 * self.voxels.len() + 8 * self.bits.len());
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&VERSION.to_le_bytes());
        o.extend_from_slice(&stamp.generator.to_le_bytes());
        o.extend_from_slice(&stamp.fingerprint.to_le_bytes());
        o.extend_from_slice(&self.walls.to_le_bytes());
        for f in self.origin.iter().chain(&self.size) {
            o.extend_from_slice(&f.to_le_bytes());
        }
        for d in self.dims {
            o.extend_from_slice(&d.to_le_bytes());
        }
        o.extend_from_slice(&(self.voxels.len() as u32).to_le_bytes());
        for v in &self.voxels {
            o.extend_from_slice(&v.to_le_bytes());
        }
        for w in &self.bits {
            o.extend_from_slice(&w.to_le_bytes());
        }
        o
    }

    /// The grid and the stamp of the `.bots` it was built from.
    pub fn from_bytes(b: &[u8]) -> Result<(Self, Stamp), Status> {
        let (version, stamp, walls) = read_header(b).ok_or(Status::Damaged)?;
        if version != VERSION {
            return Err(Status::OldVersion(version));
        }
        let mut at = HEADER_LEN;
        let mut u32_next = || -> Result<u32, Status> {
            let v = b.get(at..at + 4).ok_or(Status::Damaged)?;
            at += 4;
            Ok(u32::from_le_bytes(v.try_into().unwrap_or_default()))
        };
        let mut f = [0f32; 6];
        for v in &mut f {
            *v = f32::from_bits(u32_next()?);
        }
        let dims = [u32_next()?, u32_next()?, u32_next()?];
        let n = u32_next()? as usize;
        if n >= usize::from(NONE) {
            return Err(Status::Damaged);
        }
        let voxels = (0..n).map(|_| u32_next()).collect::<Result<Vec<_>, _>>()?;
        let origin = [f[0], f[1], f[2]];
        let size = [f[3], f[4], f[5]];
        if !f.iter().all(|v| v.is_finite()) || !size.iter().all(|&s| s > 0.0) {
            return Err(Status::Damaged);
        }
        let mut vis = Vis::with_voxels(origin, size, dims, voxels, walls).ok_or(Status::Damaged)?;
        let at = HEADER_LEN + 40 + 4 * n;
        let words = b.get(at..).ok_or(Status::Damaged)?;
        if words.len() != 8 * vis.bits.len() {
            return Err(Status::Damaged);
        }
        for (w, c) in vis.bits.iter_mut().zip(words.chunks_exact(8)) {
            *w = u64::from_le_bytes(c.try_into().unwrap_or_default());
        }
        // No bit past the last pair.
        let used = pairs(n);
        if used % 64 != 0 && vis.bits.last().is_some_and(|w| w >> (used % 64) != 0) {
            return Err(Status::Damaged);
        }
        Ok((vis, stamp))
    }
}

/// Version, stamp and walls hash from a file's first [`HEADER_LEN`]
/// bytes; `None`: not a visibility file. Another version's stamp and hash
/// are read as they would be in this one and not trusted.
fn read_header(b: &[u8]) -> Option<(u32, Stamp, u64)> {
    if b.get(..4)? != MAGIC {
        return None;
    }
    let u32_at = |at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    let u64_at = |at: usize| Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?));
    Some((
        u32_at(4)?,
        Stamp {
            generator: u32_at(8)?,
            fingerprint: u64_at(12)?,
        },
        u64_at(20)?,
    ))
}

/// The file name for `map`'s structure BSP `bsp`, beside its `.bots`.
pub fn file_name(map: &str, bsp: usize) -> String {
    let bots = cache::file_name(map, bsp);
    format!("{}vis", &bots[..bots.len() - "bots".len()])
}

/// Where a map's `<map>.0.vis` stands against its `<map>.0.bots`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Missing,
    /// Not a visibility file, or cut short.
    Damaged,
    /// Built by another [`VERSION`].
    OldVersion(u32),
    /// Built from another `.bots` (another stamp), or from other walls.
    OtherBuild,
    Fresh,
}

impl Status {
    pub fn stale(self) -> bool {
        self != Status::Fresh
    }

    /// Why it is built or not used, for the log.
    pub fn why(self) -> String {
        match self {
            Status::Missing => "none yet".into(),
            Status::Damaged => "the old file was damaged".into(),
            Status::OldVersion(v) => format!("version {v}, now {VERSION}"),
            Status::OtherBuild => "built for another nav build".into(),
            Status::Fresh => "up to date".into(),
        }
    }
}

/// The status of `dir/<map>.0.vis` against `dir/<map>.0.bots`. Reads
/// both headers.
pub fn status(dir: &Path, map: &str) -> Status {
    let head = |name: String, len: usize| -> Result<Vec<u8>, Status> {
        let mut f = match std::fs::File::open(dir.join(name)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Status::Missing),
            Err(_) => return Err(Status::Damaged),
        };
        let mut b = vec![0u8; len];
        f.read_exact(&mut b).map_err(|_| Status::Damaged)?;
        Ok(b)
    };
    let bots = match head(cache::file_name(map, 0), cache::HEADER_LEN) {
        Ok(b) => cache::read_header(&b).and_then(|(_, s)| s),
        Err(_) => None,
    };
    let vis = match head(file_name(map, 0), HEADER_LEN) {
        Ok(b) => b,
        Err(s) => return s,
    };
    match read_header(&vis) {
        None => Status::Damaged,
        Some((v, ..)) if v != VERSION => Status::OldVersion(v),
        Some((_, s, _)) if Some(s) != bots => Status::OtherBuild,
        Some(_) => Status::Fresh,
    }
}

/// Reads `dir/<map>.<bsp>.vis` and attaches it to `walls` when it was
/// built from them: by the `.bots` build with `stamp`, from walls with
/// the same hash. Returns the log line; on any failure the walls keep no
/// grid and every sight ray is cast.
pub fn load(dir: &Path, map: &str, bsp: usize, stamp: Stamp, walls: &Walls) -> String {
    let name = file_name(map, bsp);
    let not = |why: &str| format!("bots: {map}: visibility grid not used ({name}: {why}); casting every sight ray");
    let bytes = match std::fs::read(dir.join(&name)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return not("missing"),
        Err(e) => return not(&e.to_string()),
    };
    let (vis, built) = match Vis::from_bytes(&bytes) {
        Ok(v) => v,
        Err(s) => return not(&s.why()),
    };
    if built != stamp {
        return not(&Status::OtherBuild.why());
    }
    if !vis.fits(walls) {
        return not("built from other walls");
    }
    let line = format!("bots: {map}: visibility grid: {}", vis.summary());
    walls.set_vis(Arc::new(vis));
    line
}

/// Builds the grid of every structure BSP whose `.bots` is in `dir` for
/// `map` and writes it (BSP 0 last), for a map whose nav files are up to
/// date but whose grid is not. Returns BSP 0's data with its grid.
pub fn rebuild(dir: &Path, map: &str) -> Result<MapData, String> {
    rebuild_with(dir, map, Threads::foreground())
}

/// [`rebuild`] on `threads`.
pub fn rebuild_with(dir: &Path, map: &str, threads: Threads) -> Result<MapData, String> {
    let mut built = Vec::new();
    for k in 0.. {
        let path = dir.join(cache::file_name(map, k));
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if k > 0 && e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("reading {}: {e}", path.display())),
        };
        let (d, stamp) =
            MapData::from_bytes(&bytes).ok_or_else(|| format!("{} is damaged", path.display()))?;
        let vis = build_with(&d.nav, &d.walls, threads);
        let file = vis.to_bytes(stamp);
        d.walls.set_vis(Arc::new(vis));
        built.push((k, file, d));
    }
    let mut first = None;
    // BSP 0 last: once its grid is there, so are the rest.
    built.rotate_left(1);
    for (k, file, d) in built {
        let path = dir.join(file_name(map, k));
        crate::autonav::write_atomic(&path, &file).map_err(|e| format!("writing {}: {e}", path.display()))?;
        if k == 0 {
            first = Some(d);
        }
    }
    first.ok_or_else(|| "no BSP".into())
}

/// A box, `f64`, for the proof.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Box3 {
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Box3 {
    fn center(&self) -> V3 {
        std::array::from_fn(|k| ((self.lo[k] + self.hi[k]) / 2.0) as f32)
    }

    fn corners(&self) -> [[f64; 3]; 8] {
        std::array::from_fn(|c| {
            std::array::from_fn(|k| if c >> k & 1 == 0 { self.lo[k] } else { self.hi[k] })
        })
    }

    /// The longest side and the box cut in half across it.
    fn split(&self) -> (f64, Box3, Box3) {
        let k = (0..3)
            .max_by(|&a, &b| (self.hi[a] - self.lo[a]).total_cmp(&(self.hi[b] - self.lo[b])))
            .unwrap_or(0);
        let mid = (self.lo[k] + self.hi[k]) / 2.0;
        let (mut a, mut b) = (*self, *self);
        a.hi[k] = mid;
        b.lo[k] = mid;
        (self.hi[k] - self.lo[k], a, b)
    }
}

type D3 = [f64; 3];
fn d3(v: V3) -> D3 {
    v.map(f64::from)
}
fn dsub(a: D3, b: D3) -> D3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn ddot(a: D3, b: D3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn dcross(a: D3, b: D3) -> D3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dlen(a: D3) -> f64 {
    ddot(a, a).sqrt()
}

/// Whether triangle `t` alone blocks every segment from box `a` to box
/// `b`, with the margins in the module docs.
fn covers(t: &[V3; 3], a: &Box3, b: &Box3) -> bool {
    let v = t.map(d3);
    let n = dcross(dsub(v[1], v[0]), dsub(v[2], v[0]));
    let area2 = dlen(n);
    if area2 < 1e-6 {
        return false;
    }
    let nh = n.map(|c| c / area2);
    let side = |p: D3| ddot(nh, dsub(p, v[0]));
    let (ca, cb) = (a.corners(), b.corners());
    let mut fa = ca.map(side);
    let mut fb = cb.map(side);
    if fa[0] < 0.0 {
        fa = fa.map(|f| -f);
        fb = fb.map(|f| -f);
    }
    // `a` in front by PLANE_GAP, `b` behind by as much; `gb` its depth.
    let gb = fb.map(|f| -f);
    let (fa_min, fa_max) = fa.iter().fold((f64::MAX, f64::MIN), |(l, h), &f| (l.min(f), h.max(f)));
    let (gb_min, gb_max) = gb.iter().fold((f64::MAX, f64::MIN), |(l, h), &f| (l.min(f), h.max(f)));
    if fa_min < PLANE_GAP || gb_min < PLANE_GAP {
        return false;
    }
    // The crossing's place along a segment, `fa / (fa + gb)`, rises with
    // `fa` and falls with `gb`: its bounds are at the corners.
    if fa_min / (fa_min + gb_max) < T_END || fa_max / (fa_max + gb_min) > 1.0 - T_END {
        return false;
    }
    // Moller-Trumbore's determinant is `area2 * (fa + gb)`; `clear`
    // refuses below 1e-9.
    if area2 * (fa_min + gb_min) < 1e-5 {
        return false;
    }
    let longest = ca
        .iter()
        .flat_map(|p| cb.iter().map(move |q| dlen(dsub(*q, *p))))
        .fold(0.0, f64::max);
    if (fa_min + gb_min) < MIN_COS * longest {
        return false;
    }
    let edge = EDGE + EDGE_PER_LENGTH * longest;
    // Each edge's inward normal in the plane.
    let mut inward = [[0.0; 3]; 3];
    for k in 0..3 {
        let (p, q, r) = (v[k], v[(k + 1) % 3], v[(k + 2) % 3]);
        let mut m = dcross(nh, dsub(q, p));
        let l = dlen(m);
        if l < 1e-9 {
            return false;
        }
        m = m.map(|c| c / l);
        let alt = ddot(m, dsub(r, p));
        if alt < 0.0 {
            m = m.map(|c| -c);
        }
        if alt.abs() <= 2.0 * edge {
            return false;
        }
        inward[k] = m;
    }
    for (i, p) in ca.iter().enumerate() {
        for (j, q) in cb.iter().enumerate() {
            let (f, g) = (fa[i], gb[j]);
            let x: D3 = std::array::from_fn(|k| (g * p[k] + f * q[k]) / (f + g));
            for k in 0..3 {
                if ddot(inward[k], dsub(x, v[k])) < edge {
                    return false;
                }
            }
        }
    }
    true
}

/// Whether every segment from `a` to `b` is certainly blocked, within
/// `budget` pieces.
fn prove(walls: &Walls, a: Box3, b: Box3, inherited: &[u32], budget: &mut u32, depth: u32) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    let mut hit = Vec::new();
    walls.crossed(a.center(), b.center(), &mut hit);
    if hit.is_empty() {
        // A clear ray: they can see each other.
        return false;
    }
    if hit.iter().chain(inherited).any(|&t| covers(&walls.tris[t as usize], &a, &b)) {
        return true;
    }
    if depth == 0 {
        // Before splitting: another clear ray settles it at once.
        let (ca, cb) = (a.corners(), b.corners());
        let f = |p: [f64; 3]| p.map(|c| c as f32);
        if (0..8).any(|c| walls.clear(f(ca[c]), f(cb[7 - c]))) {
            return false;
        }
    }
    if depth >= DEPTH {
        return false;
    }
    for t in inherited {
        if hit.len() >= CANDIDATES {
            break;
        }
        if !hit.contains(t) {
            hit.push(*t);
        }
    }
    let (la, a1, a2) = a.split();
    let (lb, b1, b2) = b.split();
    if la >= lb {
        prove(walls, a1, b, &hit, budget, depth + 1) && prove(walls, a2, b, &hit, budget, depth + 1)
    } else {
        prove(walls, a, b1, &hit, budget, depth + 1) && prove(walls, a, b2, &hit, budget, depth + 1)
    }
}

/// Whether the two boxes certainly never see each other.
fn never_between(walls: &Walls, a: Box3, b: Box3) -> bool {
    let mut budget = LEAVES;
    prove(walls, a, b, &[], &mut budget, 0)
}

/// Points over the walkable polygons: their corners, sides and inside.
fn samples(nav: &NavMesh) -> Vec<V3> {
    let mut out = Vec::new();
    for p in &nav.polys {
        let n = p.verts.len();
        if n < 3 || p.normal[2].abs() < 0.1 {
            continue;
        }
        let z_at = |x: f32, y: f32| (p.d - p.normal[0] * x - p.normal[1] * y) / p.normal[2];
        out.push(p.center);
        for k in 0..n {
            let (a, b) = (p.verts[k], p.verts[(k + 1) % n]);
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let steps = (len / SAMPLE).ceil().max(1.0) as u32;
            for s in 0..steps {
                let t = s as f32 / steps as f32;
                out.push(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
            }
        }
        let lo = |k: usize| p.verts.iter().map(|v| v[k]).fold(f32::MAX, f32::min);
        let hi = |k: usize| p.verts.iter().map(|v| v[k]).fold(f32::MIN, f32::max);
        let inside = |x: f32, y: f32| {
            let mut c = false;
            for k in 0..n {
                let (a, b) = (p.verts[k], p.verts[(k + 1) % n]);
                if (a[1] > y) != (b[1] > y) && x < a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
                    c = !c;
                }
            }
            c
        };
        let (x0, y0, x1, y1) = (lo(0), lo(1), hi(0), hi(1));
        let (nx, ny) = (((x1 - x0) / SAMPLE) as u32, ((y1 - y0) / SAMPLE) as u32);
        for i in 0..=nx.min(4096) {
            for j in 0..=ny.min(4096) {
                let (x, y) = (x0 + i as f32 * SAMPLE, y0 + j as f32 * SAMPLE);
                if inside(x, y) {
                    out.push([x, y, z_at(x, y)]);
                }
            }
        }
    }
    out.retain(|p| p.iter().all(|v| v.is_finite()));
    out
}

/// The cells for `samples` at cell size `size`: origin, dims and the
/// occupied voxels; `None` when the voxel table would be too big.
fn voxelise(samples: &[V3], size: V3) -> Option<(V3, [u32; 3], Vec<u32>)> {
    let lo = |k: usize| samples.iter().map(|v| v[k]).fold(f32::MAX, f32::min);
    let hi = |k: usize| samples.iter().map(|v| v[k]).fold(f32::MIN, f32::max);
    let origin = [lo(0), lo(1), lo(2) - BELOW];
    let top = [hi(0), hi(1), hi(2) + ABOVE];
    let dims: [u32; 3] = std::array::from_fn(|k| ((top[k] - origin[k]) / size[k]).floor() as u32 + 1);
    let total = dims.iter().try_fold(1usize, |a, &d| a.checked_mul(d as usize))?;
    if total > MAX_VOXELS {
        return None;
    }
    let at = |k: usize, v: f32| (((v - origin[k]) / size[k]).floor().max(0.0) as u32).min(dims[k] - 1);
    let mut voxels = Vec::new();
    for p in samples {
        let (x, y) = (at(0, p[0]), at(1, p[1]));
        for z in at(2, p[2] - BELOW)..=at(2, p[2] + ABOVE) {
            voxels.push((z * dims[1] + y) * dims[0] + x);
        }
    }
    voxels.sort_unstable();
    voxels.dedup();
    Some((origin, dims, voxels))
}

/// The threads a grid build spreads its pairs over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Threads {
    pub count: usize,
    /// Each one lowers itself ([`background::lower_current`]).
    pub background: bool,
}

impl Threads {
    /// Half the machine's threads, at most 8, at the default priority: a
    /// build someone waits for (`h2 bots nav`).
    pub fn foreground() -> Self {
        Threads {
            count: std::thread::available_parallelism()
                .map_or(1, |t| t.get() / 2)
                .clamp(1, 8),
            background: false,
        }
    }

    /// [`background::workers`] threads below the game's priority: the
    /// game's own nav worker, which nothing waits for.
    pub fn background() -> Self {
        Threads {
            count: background::workers(),
            background: true,
        }
    }
}

/// Builds a structure BSP's grid from its nav mesh and walls on
/// [`Threads::foreground`].
pub fn build(nav: &NavMesh, walls: &Walls) -> Vis {
    build_with(nav, walls, Threads::foreground())
}

/// Builds a structure BSP's grid, spreading the pairs over `threads`; the
/// result does not depend on how many.
pub fn build_with(nav: &NavMesh, walls: &Walls, threads: Threads) -> Vis {
    let Threads { count: threads, background: lower } = threads;
    let threads = threads.max(1);
    let hash = walls_hash(walls);
    let points = samples(nav);
    if points.is_empty() {
        return Vis {
            walls: hash,
            ..Vis::empty()
        };
    }
    let mut size = [FIRST[0], FIRST[0], FIRST[1]];
    let mut vis = loop {
        if let Some((origin, dims, voxels)) = voxelise(&points, size)
            && voxels.len() <= MAX_CELLS
            && let Some(v) = Vis::with_voxels(origin, size, dims, voxels, hash)
        {
            break v;
        }
        size = size.map(|s| s * GROW);
    };
    let n = vis.cells();
    let boxes: Vec<Box3> = vis.voxels.iter().map(|&v| vis.voxel_box(v)).collect();
    let found: Vec<Vec<(u32, u32)>> = std::thread::scope(|s| {
        let jobs: Vec<_> = (0..threads)
            .map(|k| {
                let boxes = &boxes;
                s.spawn(move || {
                    // A QoS class does not carry to a spawned thread: each
                    // lowers itself.
                    if lower {
                        background::lower_current();
                    }
                    let mut never = Vec::new();
                    for i in (k..n).step_by(threads) {
                        for j in i + 1..n {
                            if never_between(walls, boxes[i], boxes[j]) {
                                never.push((i as u32, j as u32));
                            }
                        }
                    }
                    never
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap_or_default()).collect()
    });
    for (i, j) in found.into_iter().flatten() {
        let bit = pair_bit(n, i as usize, j as usize);
        vis.bits[bit / 64] |= 1 << (bit % 64);
    }
    vis
}

impl Vis {
    /// No cells: every ray is cast.
    fn empty() -> Self {
        Vis {
            size: [1.0; 3],
            dims: [1; 3],
            index: vec![NONE],
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;
