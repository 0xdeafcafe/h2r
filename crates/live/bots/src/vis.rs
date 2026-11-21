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
//!
//! **Progress.** A build saves the grid so far to `<map>.<bsp>.vis.part`
//! ([`part_name`]) at most every [`SAVE_EVERY`] and when stopped, and the
//! next build carries on from it ([`rebuild_resumable`]), so a game quit
//! mid-build loses at most that much.

use std::io::Read;
use std::mem::size_of;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::sync::atomic::{AtomicUsize, Ordering};

use rustc_hash::FxHashMap;

use content::Sha256;

use crate::autonav::shared_cache;
use crate::cache::{self, MapData, Stamp};
use crate::los::{BuildRays, RayOrigin, V3, Walls};
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
/// A light host budgets the dense lookup at the size of its largest
/// generated pair matrix (2048 * 2047 / 2 bits, rounded up to 256 KiB).
const LIGHT_INDEX_BYTES: usize = 256 * 1024;
static INDEX_BUDGET: AtomicUsize = AtomicUsize::new(MAX_VOXELS * size_of::<u16>());

/// Set before the game starts its map loaders. The index is optional:
/// the file already contains the same cell numbers in sorted voxel order.
pub fn set_light_index_budget(light: bool) {
    let bytes = if light {
        LIGHT_INDEX_BYTES
    } else {
        MAX_VOXELS * size_of::<u16>()
    };
    INDEX_BUDGET.store(bytes, Ordering::Relaxed);
}

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
    /// Each voxel's cell, [`NONE`] for none; x fastest. Empty when a
    /// dense table exceeds the profile's budget.
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
        if self.index.is_empty() {
            return self.voxels.binary_search(&(i as u32)).ok().map(|cell| cell as u32);
        }
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
        Self::with_index_budget(origin, size, dims, voxels, walls, INDEX_BUDGET.load(Ordering::Relaxed))
    }

    fn with_index_budget(
        origin: V3, size: V3, dims: [u32; 3], voxels: Vec<u32>, walls: u64, budget: usize,
    ) -> Option<Self> {
        let total = dims.iter().try_fold(1usize, |a, &d| a.checked_mul(d as usize))?;
        if total == 0 || total > MAX_VOXELS || voxels.len() >= usize::from(NONE) {
            return None;
        }
        if voxels.last().is_some_and(|&v| v as usize >= total)
            || !voxels.windows(2).all(|w| w[0] < w[1])
        {
            return None;
        }
        let mut index = Vec::new();
        if total * size_of::<u16>() <= budget {
            index.resize(total, NONE);
            for (cell, &voxel) in voxels.iter().enumerate() {
                index[voxel as usize] = cell as u16;
            }
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
/// `map` and whose `.vis` is not current, and writes each as it finishes
/// (BSP 0 last), for a map whose nav files are up to date but whose grid
/// is not. Returns BSP 0's data with its grid.
pub fn rebuild(dir: &Path, map: &str) -> Result<MapData, String> {
    rebuild_with(dir, map, Threads::foreground())
}

/// [`rebuild`] on `threads`.
pub fn rebuild_with(dir: &Path, map: &str, threads: Threads) -> Result<MapData, String> {
    rebuild_resumable(dir, map, threads, &|| false)?.ok_or_else(|| "stopped".into())
}

/// [`rebuild_with`], resumable: each BSP's build carries on from its
/// `.vis.part` ([`part_name`]) and saves its progress there at least every
/// [`SAVE_EVERY`], so a build cut short (the game quit, the worker asked
/// for another map) loses at most that much. A BSP whose `.vis` is
/// current ([`current`]) is not built again. `stop` is asked between
/// chunks of pairs; when it says so, the progress is saved and the result
/// is `Ok(None)`.
pub fn rebuild_resumable(
    dir: &Path,
    map: &str,
    threads: Threads,
    stop: &dyn Fn() -> bool,
) -> Result<Option<MapData>, String> {
    rebuild_cached(dir, map, threads, stop, None).map(|done| done.map(|done| done.data))
}

/// A decoded BSP with the identity of the exact .bots file it came from.
pub(crate) struct GridInput {
    pub data: MapData,
    pub stamp: Stamp,
    pub digest: Option<Sha256>,
}

pub(crate) struct GridBuild {
    pub data: MapData,
    pub restored: usize,
}

pub(crate) fn rebuild_cached(
    dir: &Path,
    map: &str,
    threads: Threads,
    stop: &dyn Fn() -> bool,
    shared: Option<&Path>,
) -> Result<Option<GridBuild>, String> {
    let mut all: Vec<GridInput> = Vec::new();
    for k in 0.. {
        let path = dir.join(cache::file_name(map, k));
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if k > 0 && e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("reading {}: {e}", path.display())),
        };
        // The map can have shrunk to fewer BSPs. Only build grids for
        // the same generation as the BSP zero file.
        if let Some(first) = all.first()
            && let Some((_, stamp)) = cache::read_header(&bytes)
            && stamp != Some(first.stamp)
        {
            break;
        }
        let (data, stamp) =
            MapData::from_bytes(&bytes).ok_or_else(|| format!("{} is damaged", path.display()))?;
        all.push(GridInput {
            data,
            stamp,
            digest: shared.map(|_| Sha256::of(&bytes)),
        });
    }
    rebuild_prepared(dir, map, threads, stop, shared, all)
}

pub(crate) fn rebuild_prepared(
    dir: &Path,
    map: &str,
    threads: Threads,
    stop: &dyn Fn() -> bool,
    shared: Option<&Path>,
    all: Vec<GridInput>,
) -> Result<Option<GridBuild>, String> {
    let mut restored = 0;
    // BSP 0 last: once its grid is there, so are the rest.
    let order = (1..all.len()).chain(0..1);
    for k in order {
        let GridInput { data, stamp, digest } = &all[k];
        let entry = shared.zip(*digest);
        let hit = |(root, key)| shared_cache::restore_grid(root, key, *stamp, &data.walls);
        let grid = if let Some(grid) = current(dir, map, k, *stamp, &data.walls) {
            grid
        } else if let Some((grid, bytes)) = entry.and_then(hit) {
            let path = dir.join(file_name(map, k));
            crate::autonav::write_atomic(&path, &bytes)
                .map_err(|e| format!("writing {}: {e}", path.display()))?;
            let _ = std::fs::remove_file(dir.join(part_name(map, k)));
            restored += 1;
            grid
        } else {
            let Some(grid) = build_file(dir, map, k, *stamp, &data.nav, &data.walls, threads, stop)?
            else {
                return Ok(None);
            };
            if let Some((root, key)) = entry {
                shared_cache::save_grid(root, key, *stamp, &grid);
            }
            grid
        };
        data.walls.set_vis(Arc::new(grid));
    }
    Ok(all.into_iter().next().map(|input| GridBuild { data: input.data, restored }))
}

/// `dir/<map>.<bsp>.vis` when it is current for the `.bots` build with
/// `stamp` and its `walls`: this [`VERSION`], that stamp, those walls.
pub fn current(dir: &Path, map: &str, bsp: usize, stamp: Stamp, walls: &Walls) -> Option<Vis> {
    let bytes = std::fs::read(dir.join(file_name(map, bsp))).ok()?;
    let (vis, built) = Vis::from_bytes(&bytes).ok()?;
    (built == stamp && vis.fits(walls)).then_some(vis)
}

/// The progress file of a grid build, beside its `.vis`:
/// `<map>.<bsp>.vis.part`.
pub fn part_name(map: &str, bsp: usize) -> String {
    format!("{}.part", file_name(map, bsp))
}

/// A progress file's magic: then the first row not done (`u32`), then the
/// grid so far as a `.vis` file ([`Vis::to_bytes`]).
pub const PART_MAGIC: &[u8; 4] = b"H2VP";

/// How often a running build saves its progress, at most.
pub const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(10);

fn part_bytes(vis: &Vis, stamp: Stamp, row: usize) -> Vec<u8> {
    let mut o = PART_MAGIC.to_vec();
    o.extend_from_slice(&(row as u32).to_le_bytes());
    o.extend_from_slice(&vis.to_bytes(stamp));
    o
}

/// The grid so far and the first row not done, from a progress file made
/// for the `.bots` build with `stamp` with the same cells as `fresh`;
/// `None` for anything else (it is then built from the start).
pub fn read_part(path: &Path, stamp: Stamp, fresh: &Vis) -> Option<(Vis, usize)> {
    let b = std::fs::read(path).ok()?;
    if b.get(..4)? != PART_MAGIC {
        return None;
    }
    let row = u32::from_le_bytes(b.get(4..8)?.try_into().ok()?) as usize;
    let (vis, built) = Vis::from_bytes(b.get(8..)?).ok()?;
    let same = built == stamp
        && vis.walls == fresh.walls
        && vis.origin == fresh.origin
        && vis.size == fresh.size
        && vis.dims == fresh.dims
        && vis.voxels == fresh.voxels
        && row <= vis.cells();
    same.then_some((vis, row))
}

/// Builds `dir/<map>.<bsp>.vis` for the `.bots` build with `stamp`,
/// carrying on from its progress file and saving to it, then writes the
/// grid and removes the progress file. `Ok(None)`: stopped, progress saved.
#[allow(clippy::too_many_arguments)]
pub fn build_file(
    dir: &Path,
    map: &str,
    bsp: usize,
    stamp: Stamp,
    nav: &NavMesh,
    walls: &Walls,
    threads: Threads,
    stop: &dyn Fn() -> bool,
) -> Result<Option<Vis>, String> {
    let path = dir.join(file_name(map, bsp));
    let part = dir.join(part_name(map, bsp));
    let fresh = layout(nav, walls);
    let (start, row) = read_part(&part, stamp, &fresh).unwrap_or((fresh, 0));
    let mut save = |v: &Vis, row: usize| crate::autonav::write_atomic(&part, &part_bytes(v, stamp, row));
    let done = fill(start, row, walls, threads, &mut save, stop)
        .map_err(|e| format!("writing {}: {e}", part.display()))?;
    let Some(vis) = done else {
        return Ok(None);
    };
    crate::autonav::write_atomic(&path, &vis.to_bytes(stamp))
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    let _ = std::fs::remove_file(&part);
    Ok(Some(vis))
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

    /// The corners with the minimum and maximum projection on `normal`.
    fn plane_extremes(&self, normal: D3) -> (D3, D3) {
        let low = std::array::from_fn(|k| if normal[k] < 0.0 { self.hi[k] } else { self.lo[k] });
        let high = std::array::from_fn(|k| if normal[k] < 0.0 { self.lo[k] } else { self.hi[k] });
        (low, high)
    }

    fn longest_to(&self, other: &Self) -> f64 {
        let far = std::array::from_fn(|k| {
            (other.hi[k] - self.lo[k]).abs().max((other.lo[k] - self.hi[k]).abs())
        });
        dlen(far)
    }

    fn coordinate_scale(&self) -> f64 {
        self.lo.iter().chain(&self.hi).fold(1.0_f64, |m, c| m.max(c.abs()))
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

struct Occluder {
    vertices: [D3; 3],
    normal: D3,
    area2: f64,
    inward: [D3; 3],
    min_altitude: f64,
    coordinate_scale: f64,
}

impl Occluder {
    fn new(t: &[V3; 3]) -> Option<Self> {
        let vertices = t.map(d3);
        let n = dcross(dsub(vertices[1], vertices[0]), dsub(vertices[2], vertices[0]));
        let area2 = dlen(n);
        if !area2.is_finite() || area2 < 1e-6 {
            return None;
        }
        let normal = n.map(|c| c / area2);
        let mut inward = [[0.0; 3]; 3];
        let mut min_altitude = f64::MAX;
        for k in 0..3 {
            let (p, q, r) = (vertices[k], vertices[(k + 1) % 3], vertices[(k + 2) % 3]);
            let m = dcross(normal, dsub(q, p));
            let length = dlen(m);
            if length < 1e-9 {
                return None;
            }
            let m = m.map(|c| c / length);
            let altitude = ddot(m, dsub(r, p));
            min_altitude = min_altitude.min(altitude.abs());
            inward[k] = if altitude < 0.0 { m.map(|c| -c) } else { m };
        }
        let coordinate_scale = vertices.iter().flatten().fold(1.0_f64, |m, c| m.max(c.abs()));
        Some(Self {
            vertices,
            normal,
            area2,
            inward,
            min_altitude,
            coordinate_scale,
        })
    }

    /// Whether this triangle alone blocks every segment between the boxes.
    fn covers(&self, a: &Box3, b: &Box3) -> bool {
        let side = |p: D3| ddot(self.normal, dsub(p, self.vertices[0]));
        let (a_lo, a_hi) = a.plane_extremes(self.normal);
        let (b_lo, b_hi) = b.plane_extremes(self.normal);
        // The ordered f64 operations in `side` are monotonic on each
        // axis, so these corners give the same distances as the full scan.
        let sign = if side(a.lo) < 0.0 { -1.0 } else { 1.0 };
        let (fa_min, fa_max, gb_min, gb_max) = if sign < 0.0 {
            (-side(a_hi), -side(a_lo), side(b_lo), side(b_hi))
        } else {
            (side(a_lo), side(a_hi), -side(b_hi), -side(b_lo))
        };
        if fa_min < PLANE_GAP || gb_min < PLANE_GAP {
            return false;
        }
        // t = fa / (fa + gb) rises with fa and falls with gb.
        if fa_min / (fa_min + gb_max) < T_END || fa_max / (fa_max + gb_min) > 1.0 - T_END {
            return false;
        }
        if self.area2 * (fa_min + gb_min) < 1e-5 {
            return false;
        }
        // Corner choices are independent across the three axes. Find the
        // longest segment with one norm instead of 64.
        let longest = a.longest_to(b);
        if (fa_min + gb_min) < MIN_COS * longest {
            return false;
        }
        let edge = EDGE + EDGE_PER_LENGTH * longest;
        if self.min_altitude <= 2.0 * edge {
            return false;
        }

        let (ca, cb) = (a.corners(), b.corners());
        let fa = ca.map(|p| sign * side(p));
        let gb = cb.map(|p| -sign * side(p));
        let inv_a = fa.map(f64::recip);
        let inv_b = gb.map(f64::recip);
        let scale = a.coordinate_scale().max(b.coordinate_scale()).max(self.coordinate_scale);
        // For signed edge distance E, the crossing of corners p and q
        // passes iff (E(p)-edge)/fa + (E(q)-edge)/gb >= 0. The scale of
        // the terms is bounded by the input coordinate magnitude. Both the
        // original projection/dot and the factored subtractions, dot and
        // division contribute less than 128 epsilons at that scale (three
        // unit-normal components, positive weights and no f64 overflow for
        // f32 map coordinates). The inverse minimum depths carry this
        // absolute error into the scaled sum. At ambiguous values, use
        // the original rounded projection and comparison.
        let tolerance = scale * (128.0 * f64::EPSILON) * (fa_min.recip() + gb_min.recip());
        let mut ambiguous = false;
        for k in 0..3 {
            let distance = |p| ddot(self.inward[k], dsub(p, self.vertices[k])) - edge;
            let least = |corners: &[D3; 8], inverse: [f64; 8]| {
                corners.iter().zip(inverse).fold(f64::MAX, |min, (&p, inv)| {
                    min.min(distance(p) * inv)
                })
            };
            let sum = least(&ca, inv_a) + least(&cb, inv_b);
            if sum < -tolerance {
                return false;
            }
            ambiguous |= sum <= tolerance;
        }
        if !ambiguous {
            return true;
        }
        for (i, p) in ca.iter().enumerate() {
            for (j, q) in cb.iter().enumerate() {
                let (f, g) = (fa[i], gb[j]);
                let x = std::array::from_fn(|k| (g * p[k] + f * q[k]) / (f + g));
                for k in 0..3 {
                    if ddot(self.inward[k], dsub(x, self.vertices[k])) < edge {
                        return false;
                    }
                }
            }
        }
        true
    }
}

struct RootRay<'a> {
    origin: &'a mut RayOrigin,
    candidates: Option<&'a [u32]>,
}

struct Proof<'a> {
    walls: &'a Walls,
    rays: BuildRays<'a>,
    occluders: Vec<Option<Occluder>>,
}

impl<'a> Proof<'a> {
    fn new(walls: &'a Walls) -> Self {
        Self {
            walls,
            rays: BuildRays::new(walls),
            occluders: walls.tris.iter().map(Occluder::new).collect(),
        }
    }

    fn covers(&self, t: u32, a: &Box3, b: &Box3) -> bool {
        self.occluders[t as usize]
            .as_ref()
            .is_some_and(|triangle| triangle.covers(a, b))
    }

    /// Whether every segment from `a` to `b` is certainly blocked, within
    /// `budget` pieces. Each recursion level reuses its hit list.
    #[allow(clippy::too_many_arguments)]
    fn prove(
        &self,
        a: Box3,
        b: Box3,
        inherited: &[u32],
        budget: &mut u32,
        scratch: &mut [Vec<u32>],
        root: Option<RootRay<'_>>,
    ) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        let check_corners = root.is_some();
        let (hit, scratch) = scratch.split_first_mut().expect("one candidate list per depth");
        hit.clear();
        let test = |t| {
            if self.covers(t, &a, &b) {
                return false;
            }
            hit.push(t);
            true
        };
        let all = match root {
            Some(ray) => self.rays.crossed_from(ray.origin, b.center(), ray.candidates, test),
            None => self.rays.crossed(a.center(), b.center(), test),
        };
        if !all {
            return true;
        }
        if hit.is_empty() {
            // A clear ray: they can see each other.
            return false;
        }
        if inherited.iter().any(|&t| self.covers(t, &a, &b)) {
            return true;
        }
        if check_corners {
            let (ca, cb) = (a.corners(), b.corners());
            let f = |p: D3| p.map(|c| c as f32);
            if (0..8).any(|c| self.walls.clear(f(ca[c]), f(cb[7 - c]))) {
                return false;
            }
        }
        if scratch.is_empty() {
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
            self.prove(a1, b, hit, budget, scratch, None)
                && self.prove(a2, b, hit, budget, scratch, None)
        } else {
            self.prove(a, b1, hit, budget, scratch, None)
                && self.prove(a, b2, hit, budget, scratch, None)
        }
    }
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

/// The thread budget and priority for a bot nav or visibility build.
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
    match fill(layout(nav, walls), 0, walls, threads, &mut |_, _| Ok(()), &|| false) {
        Ok(Some(v)) => v,
        // Neither saves nor stops.
        _ => unreachable!("a build that never stops finishes"),
    }
}

/// The grid's cells for `nav`, no pair set yet.
fn layout(nav: &NavMesh, walls: &Walls) -> Vis {
    let hash = walls_hash(walls);
    let points = samples(nav);
    if points.is_empty() {
        return Vis {
            walls: hash,
            ..Vis::empty()
        };
    }
    let mut size = [FIRST[0], FIRST[0], FIRST[1]];
    loop {
        if let Some((origin, dims, voxels)) = voxelise(&points, size)
            && voxels.len() <= MAX_CELLS
            && let Some(v) = Vis::with_voxels(origin, size, dims, voxels, hash)
        {
            return v;
        }
        size = size.map(|s| s * GROW);
    }
}

/// Cell groups with the same projected XY center. Their cell numbers
/// remain ascending so each row still tests only its original `j > i` pairs.
fn columns(vis: &Vis) -> Vec<Vec<usize>> {
    let xy = vis.dims[0] * vis.dims[1];
    let mut indices: FxHashMap<u32, usize> = FxHashMap::default();
    let mut columns: Vec<Vec<usize>> = Vec::new();
    for (i, &voxel) in vis.voxels.iter().enumerate() {
        let k = *indices.entry(voxel % xy).or_insert_with(|| {
            columns.push(Vec::new());
            columns.len() - 1
        });
        columns[k].push(i);
    }
    columns
}

struct RowScratch {
    origin: RayOrigin,
    levels: [Vec<u32>; DEPTH as usize + 1],
    candidates: Vec<u32>,
    bits: Vec<u64>,
}

impl RowScratch {
    fn new(triangles: usize) -> Self {
        Self {
            origin: RayOrigin::new(triangles),
            levels: Default::default(),
            candidates: Vec::new(),
            bits: Vec::new(),
        }
    }

    fn prove(
        &mut self,
        proof: &Proof<'_>,
        boxes: &[Box3],
        columns: &[Vec<usize>],
        i: usize,
        found: &mut Vec<(usize, u64)>,
    ) {
        let a = boxes[i];
        let source = a.center();
        self.origin.reset(source);
        let n = boxes.len();
        let first = pair_bit(n, i, i + 1);
        let base_word = first / 64;
        self.bits.clear();
        self.bits.resize((first + n - 1 - i).div_ceil(64) - base_word, 0);
        for column in columns {
            let destinations = &column[column.partition_point(|&j| j <= i)..];
            let candidates = if let [j, _, ..] = destinations {
                Some(proof.rays.candidates(source, boxes[*j].center(), &mut self.candidates))
            } else {
                None
            };
            for &j in destinations {
                let mut budget = LEAVES;
                let root = RootRay {
                    origin: &mut self.origin,
                    candidates,
                };
                if proof.prove(a, boxes[j], &[], &mut budget, &mut self.levels, Some(root)) {
                    let bit = pair_bit(n, i, j);
                    self.bits[bit / 64 - base_word] |= 1 << (bit % 64);
                }
            }
        }
        found.extend(self.bits.iter().enumerate().filter_map(|(word, &bits)| {
            (bits != 0).then_some((base_word + word, bits))
        }));
    }
}

/// About this many chunks of pairs a grid is built in; progress is saved
/// and `stop` asked between them.
const CHUNKS: usize = 64;

/// Proves the pairs of rows `row..` of `vis` (row `i` is the pairs
/// `(i, j > i)`). Persistent workers take rows from one chunk at a
/// time. After a chunk the build
/// saves (`save(grid, first row not done)`) when [`SAVE_EVERY`] has passed,
/// and when `stop` says so it saves and returns `None`. The rows done are
/// the same whatever the chunks, so a build carried on from a save ends
/// with the grid a build in one go makes.
fn fill(
    mut vis: Vis,
    mut row: usize,
    walls: &Walls,
    threads: Threads,
    save: &mut dyn FnMut(&Vis, usize) -> std::io::Result<()>,
    stop: &dyn Fn() -> bool,
) -> std::io::Result<Option<Vis>> {
    let n = vis.cells();
    if row + 1 >= n || walls.tris.is_empty() {
        return Ok(Some(vis));
    }
    let proof = Proof::new(walls);
    let boxes: Vec<Box3> = vis.voxels.iter().map(|&v| vis.voxel_box(v)).collect();
    let columns = columns(&vis);
    let per_chunk = pairs(n).div_ceil(CHUNKS).max(1);
    let next = AtomicUsize::new(row);
    let mut saved = std::time::Instant::now();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.count.max(1).min(n - 1 - row))
            .map(|_| {
                let (send, work) = mpsc::channel();
                let (done, receive) = mpsc::channel();
                let (proof, boxes, columns, next) = (&proof, &boxes, &columns, &next);
                scope.spawn(move || {
                    // A QoS class does not carry to a spawned thread.
                    if threads.background {
                        background::lower_current();
                    }
                    let mut scratch = RowScratch::new(proof.occluders.len());
                    while let Ok(end) = work.recv() {
                        let mut found = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            if i >= end {
                                break;
                            }
                            scratch.prove(proof, boxes, columns, i, &mut found);
                        }
                        if done.send(found).is_err() {
                            break;
                        }
                    }
                });
                (send, receive)
            })
            .collect();
        while row + 1 < n {
            let (mut end, mut count) = (row, 0);
            // Late rows have less work. Keep enough in the checkpoint to
            // use the workers, while keeping the same contiguous row prefix.
            while end + 1 < n && (count < per_chunk || end - row < workers.len()) {
                count += n - 1 - end;
                end += 1;
            }
            next.store(row, Ordering::Relaxed);
            for (work, _) in &workers {
                work.send(end).expect("visibility worker stopped");
            }
            for (_, done) in &workers {
                for (index, word) in done.recv().expect("visibility worker stopped") {
                    vis.bits[index] |= word;
                }
            }
            row = end;
            if row + 1 >= n {
                break;
            }
            if stop() {
                save(&vis, row)?;
                return Ok(None);
            }
            if saved.elapsed() >= SAVE_EVERY {
                save(&vis, row)?;
                saved = std::time::Instant::now();
            }
        }
        Ok(Some(vis))
    })
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
mod legacy;
#[cfg(test)]
mod tests;
