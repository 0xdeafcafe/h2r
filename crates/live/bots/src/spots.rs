//! The superbounce and forced-crouch spots the bots know for a map
//! (docs/game/bots.md, "Superbounce routes").
//!
//! Three sources, measured first:
//!
//! - the hand-measured spots ([`crate::glitches::SPOTS`]), a place a game
//!   run bounced from;
//! - the scanned spots (`superbounce::Spot`, found in the map's collision
//!   geometry), made while the nav mesh is built (`crate::autonav`) and
//!   saved beside it as `<map>.spots`, stamped like the nav files, so a
//!   map's spots are found once per map and generator;
//! - the map's verified file (`<map>.verified` beside them,
//!   [`superbounce::verified`]): what `superbounce.verify` found when it
//!   tried the spots in a game. A spot it proved is preferred, with the
//!   offset that worked; one it failed is left out.
//!
//! The scans of the maps the worker has seen sit in a process-wide table
//! ([`publish`], [`scanned`]); a brain asks it for its map's ([`Known`]).
//! Pure data, no guest memory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use superbounce::verified::{self, Entry};
use superbounce::{Kind, Lip, MAX_LIP, Shape, Spot, Status};

use crate::brain::Vec3;
use crate::cache::Stamp;
use crate::glitches::Bounce;

/// How far from its `stand` a scanned bounce is retried between tries
/// (world units). Smaller than a measured spot's: a scanned slot is
/// narrower than a trace says it is.
pub const SCAN_SPREAD: f32 = 0.03;
/// A scanned spot scoring under this is left out.
pub const MIN_SCORE: f32 = 0.3;
/// A scanned bounce within this of a measured one is the same place.
pub const SAME_SPOT: f32 = 1.0;
/// The launch speed assumed for a measured spot (world units a second):
/// the player's own launch from docs/game/superbounce.md ("The pair word",
/// +12.3 to +12.45).
pub const MEASURED_LAUNCH: f32 = 12.3;
/// The touchdown speed the measured Ascension landing came in at (world
/// units a second): docs/game/superbounce.md, "The forced crouch leaves state", the recipe, step 4.
pub const MEASURED_NEED: f32 = 5.4;
/// The fall onto a measured spot that has a takeoff (world units): the
/// recipe's jump off the upper level (z -0.76) onto the patch (z -3.19),
/// docs/game/superbounce.md, "The forced crouch leaves state". A standing
/// jump falls 0 and lands at -3: never a bounce.
pub const MEASURED_FALL: f32 = 2.43;
/// The most a bounce site may need to fall onto it (world units) for a
/// bot's standing jump to reach it.
pub const STANDING_FALL: f32 = 0.5;
/// A slot's width when the spot does not say (world units): Ascension's
/// strip, y 3.8 to 4.0.
pub const SLOT_WIDTH: f32 = 0.2;

/// A bounce the planner can use: the spot as the bounce run takes it, and
/// what the planner needs to know of its launch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Site {
    pub bounce: Bounce,
    /// Launch speed straight up (world units a second).
    pub launch: f32,
    /// How far the landing must fall onto the slot ([`STANDING_FALL`] is
    /// what a standing jump gives).
    pub fall: f32,
    /// The downward speed the landing must touch down at (world units a
    /// second; 0 unknown).
    pub need: f32,
    /// The slot's width (world units; 0 unknown, read as [`SLOT_WIDTH`]).
    pub width: f32,
    /// `Proven` when a game run bounced from it (measured, or verified).
    pub status: Status,
    /// Where a verified drop landed: the point to aim at instead of
    /// `stand`.
    pub aim: Option<Vec3>,
    /// The verified drop height over the strip (world units).
    pub height: Option<f32>,
}

impl Site {
    /// A scanned bounce spot as a site; `None` for a crouch spot.
    pub fn from_spot(s: &Spot) -> Option<Site> {
        (s.kind == Kind::Bounce).then_some(Site {
            bounce: Bounce {
                // A scanned spot belongs to the map the brain holds it
                // for, not to a name.
                map: "",
                name: "a scanned slot",
                stand: s.stand,
                spread: SCAN_SPREAD,
                drift: s.drift,
                source: "scan",
                takeoff: None,
            },
            launch: s.launch,
            fall: s.fall,
            need: s.need,
            width: s.width,
            status: s.status,
            aim: None,
            height: None,
        })
    }

    /// A measured spot as a site: a drop from its takeoff
    /// ([`MEASURED_FALL`]) when it has one, else a standing jump.
    pub fn measured(b: Bounce) -> Site {
        Site {
            bounce: b,
            launch: MEASURED_LAUNCH,
            fall: if b.takeoff.is_some() { MEASURED_FALL } else { 0.0 },
            need: MEASURED_NEED,
            width: SLOT_WIDTH,
            status: Status::Proven,
            aim: None,
            height: None,
        }
    }

    /// With what a verified entry about it says: its status, and where and
    /// from how high the drop that worked landed.
    pub fn verified(mut self, e: Option<&Entry>) -> Site {
        if let Some(e) = e {
            self.status = e.status;
            if e.status == Status::Proven {
                self.aim = Some(e.working());
                self.height = e.height;
                if e.best > 0.0 {
                    self.launch = e.best;
                }
            }
        }
        self
    }

    /// A standing jump from the spot's `stand` reaches it.
    pub fn standing(&self) -> bool {
        self.fall <= STANDING_FALL && self.height.is_none() && self.bounce.takeoff.is_none()
    }

    /// Where to land: the verified point, else `stand`.
    pub fn target(&self) -> Vec3 {
        self.aim.unwrap_or(self.bounce.stand)
    }

    /// The height over the strip a drop starts from: the verified one, else
    /// the scan's `fall`.
    pub fn drop_height(&self) -> f32 {
        self.height.unwrap_or(self.fall)
    }

    pub fn proven(&self) -> bool {
        self.status == Status::Proven
    }
}

/// A forced-crouch place a bot primes at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Crouch {
    /// Where the forced crouch lets go: the spot's way out.
    pub stand: Vec3,
    /// The unit direction (X and Y) from `stand` to the standing room
    /// beside it; zero when not known (the bot then primes in place).
    pub out: [f32; 2],
    pub status: Status,
    /// What a verified trial found: the offset from `stand` it started at
    /// (X and Y) and its approach yaw (degrees), when one primed.
    pub start: Option<([f32; 2], Option<f32>)>,
}

impl Crouch {
    /// A crouch place with no way out known.
    pub fn at(stand: Vec3) -> Crouch {
        Crouch {
            stand,
            ..Crouch::default()
        }
    }

    /// A scanned crouch spot, with what a verified entry about it says.
    pub fn from_spot(s: &Spot, e: Option<&Entry>) -> Crouch {
        let mut c = Crouch {
            stand: s.stand,
            out: s.out,
            status: s.status,
            start: None,
        };
        if let Some(e) = e {
            c.status = e.status;
            if e.status == Status::Proven {
                c.start = Some((e.offset, e.yaw));
            }
        }
        c
    }

    pub fn proven(&self) -> bool {
        self.status == Status::Proven
    }
}

/// What a bot knows of one map's spots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Known {
    /// Bounce sites: measured, then scanned, proven before the rest, best
    /// first. None a game run failed.
    pub sites: Vec<Site>,
    /// Where a standing bot is forced into a crouch (the places to prime),
    /// proven first. None a game run failed.
    pub crouches: Vec<Crouch>,
}

impl Known {
    /// The spots of `map` (its short name, any case): measured, then
    /// scanned ([`scanned`]), with its verified file ([`verified_for`]).
    pub fn for_map(map: &str) -> Known {
        Known::merge_verified(
            crate::glitches::bounces(map),
            &scanned(map),
            &verified_for(map),
        )
    }

    /// [`Known::merge_verified`] with no verified file.
    pub fn merge(measured: Vec<Bounce>, scan: &[Spot]) -> Known {
        Known::merge_verified(measured, scan, &[])
    }

    /// Measured spots first; scans after, without a scanned bounce at a
    /// measured one's place, proven before likely, best first. A spot a
    /// verified entry failed is left out, measured or not.
    pub fn merge_verified(measured: Vec<Bounce>, scan: &[Spot], ver: &[Entry]) -> Known {
        let probe = |kind: Kind, stand: Vec3| Spot {
            kind,
            stand,
            ..Spot::default()
        };
        let mut sites: Vec<Site> = measured
            .into_iter()
            .map(|b| Site::measured(b).verified(verified::entry_for(ver, &probe(Kind::Bounce, b.stand))))
            .collect();
        let kept = sites.len();
        let mut scan: Vec<&Spot> = scan.iter().filter(|s| s.score >= MIN_SCORE).collect();
        scan.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut crouches = Vec::new();
        let mut scanned_sites = Vec::new();
        for s in scan {
            let e = verified::entry_for(ver, s);
            match s.kind {
                Kind::Bounce => {
                    let same = sites[..kept]
                        .iter()
                        .any(|m| dist_xy(m.bounce.stand, s.stand) < SAME_SPOT);
                    if !same && let Some(site) = Site::from_spot(s) {
                        scanned_sites.push(site.verified(e));
                    }
                }
                Kind::Crouch => crouches.push(Crouch::from_spot(s, e)),
            }
        }
        // Proven first; the sort is stable, so best first within each.
        scanned_sites.sort_by_key(|s| !s.proven());
        crouches.sort_by_key(|c| !c.proven());
        sites.extend(scanned_sites);
        sites.retain(|s| s.status != Status::Failed);
        crouches.retain(|c| c.status != Status::Failed);
        Known { sites, crouches }
    }

    /// The crouch spot nearest `at` (straight line, XY), if any.
    pub fn nearest_crouch(&self, at: Vec3) -> Option<Crouch> {
        self.crouches
            .iter()
            .copied()
            .min_by(|a, b| dist_xy(a.stand, at).total_cmp(&dist_xy(b.stand, at)))
    }
}

pub(crate) fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// One spot as the agent API lists it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listed {
    /// `bounce` or `crouch`.
    pub kind: &'static str,
    /// `measured` or `scan`.
    pub source: &'static str,
    pub spot: Spot,
}

/// Every spot of `map`: the measured ones first, then the scan, best
/// first, each with its verified status (failed ones too). A measured
/// spot's edge is the point it stands on, its score 1, its status proven.
pub fn listing(map: &str) -> Vec<Listed> {
    let ver = verified_for(map);
    let mut out: Vec<Listed> = crate::glitches::bounces(map)
        .into_iter()
        .map(|b| Listed {
            kind: "bounce",
            source: "measured",
            spot: Spot {
                kind: Kind::Bounce,
                edge: [b.stand, b.stand],
                stand: b.stand,
                fall: Site::measured(b).fall,
                launch: MEASURED_LAUNCH,
                drift: b.drift,
                score: 1.0,
                need: MEASURED_NEED,
                status: Status::Proven,
                ..Spot::default()
            },
        })
        .collect();
    let mut scan: Vec<Spot> = scanned(map).as_ref().clone();
    verified::apply(&mut scan, &ver);
    for l in &mut out {
        verified::apply(std::slice::from_mut(&mut l.spot), &ver);
    }
    scan.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.extend(scan.iter().map(|s| Listed {
        kind: match s.kind {
            Kind::Bounce => "bounce",
            Kind::Crouch => "crouch",
        },
        source: "scan",
        spot: *s,
    }));
    out
}

/// Whether the nav worker has published a scan of `map`.
pub fn has_scan(map: &str) -> bool {
    let t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    t.as_ref()
        .is_some_and(|t| t.maps.contains_key(&map.to_ascii_lowercase()))
}

// ---- the table ----

#[derive(Default)]
struct Table {
    maps: HashMap<String, Arc<Vec<Spot>>>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Changes whenever [`publish`] stores a scan or [`store_verified`] a
/// verified file: a brain compares it with the one it last read to know
/// its map's spots may have changed.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Stores `map`'s scan (any case), replacing the last.
pub fn publish(map: &str, spots: Vec<Spot>) {
    let mut t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    t.get_or_insert_with(Table::default)
        .maps
        .insert(map.to_ascii_lowercase(), Arc::new(spots));
    GENERATION.fetch_add(1, Ordering::Release);
}

/// The scan of `map` (any case); empty until the worker has made or read
/// it.
pub fn scanned(map: &str) -> Arc<Vec<Spot>> {
    let t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    t.as_ref()
        .and_then(|t| t.maps.get(&map.to_ascii_lowercase()))
        .cloned()
        .unwrap_or_default()
}

// ---- the verified files ----

#[derive(Default)]
struct Verified {
    /// Where the files are: the bots directory ([`set_verified_dir`]).
    dir: Option<PathBuf>,
    maps: HashMap<String, Arc<Vec<Entry>>>,
}

static VERIFIED: Mutex<Option<Verified>> = Mutex::new(None);

/// `<map>.verified`, in the bots directory beside the spots files.
pub fn verified_file_name(map: &str) -> String {
    crate::cache::file_name(map, 0).replace(".0.bots", ".verified")
}

/// Where the verified files are read from and written to: the bots
/// directory (`crate::cache::dir`). Forgets what was read from elsewhere.
pub fn set_verified_dir(dir: PathBuf) {
    let mut v = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    let v = v.get_or_insert_with(Verified::default);
    if v.dir.as_ref() != Some(&dir) {
        v.dir = Some(dir);
        v.maps.clear();
        GENERATION.fetch_add(1, Ordering::Release);
    }
}

/// The verified directory, once set.
pub fn verified_dir() -> Option<PathBuf> {
    let v = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    v.as_ref().and_then(|v| v.dir.clone())
}

/// Reads `map`'s verified file from `dir`; empty when there is none. A
/// damaged file reads as empty, with the reason.
pub fn read_verified(dir: &Path, map: &str) -> Result<Vec<Entry>, String> {
    match std::fs::read_to_string(dir.join(verified_file_name(map))) {
        Ok(text) => verified::parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// What `map`'s verified file says (any case): read once from the
/// verified directory, then kept; empty with no directory or no file.
pub fn verified_for(map: &str) -> Arc<Vec<Entry>> {
    let key = map.to_ascii_lowercase();
    let mut v = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    let v = v.get_or_insert_with(Verified::default);
    if let Some(e) = v.maps.get(&key) {
        return Arc::clone(e);
    }
    let entries = match &v.dir {
        Some(dir) if !key.is_empty() => read_verified(dir, &key).unwrap_or_else(|why| {
            eprintln!("bots: {key}'s verified spots not read: {why}");
            Vec::new()
        }),
        _ => Vec::new(),
    };
    let entries = Arc::new(entries);
    v.maps.insert(key, Arc::clone(&entries));
    entries
}

/// Merges `new` into `map`'s verified entries ([`verified::merge`]), writes
/// the file when there is a verified directory, and bumps the
/// [`generation`] so brains read their spots again. The path written, if
/// any.
pub fn store_verified(map: &str, new: &[Entry]) -> std::io::Result<Option<PathBuf>> {
    let key = map.to_ascii_lowercase();
    let old = verified_for(&key);
    let merged = verified::merge(&old, new);
    let dir = verified_dir();
    let written = match &dir {
        Some(dir) if !key.is_empty() => {
            std::fs::create_dir_all(dir)?;
            let path = dir.join(verified_file_name(&key));
            crate::autonav::write_atomic(&path, verified::to_text(&merged).as_bytes())?;
            Some(path)
        }
        _ => None,
    };
    let mut v = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    v.get_or_insert_with(Verified::default)
        .maps
        .insert(key, Arc::new(merged));
    GENERATION.fetch_add(1, Ordering::Release);
    Ok(written)
}

// ---- the file ----

/// A spots file's first bytes and format version.
pub const MAGIC: &[u8; 4] = b"H2SP";
pub const VERSION: u32 = 3;
const HEADER: usize = 4 + 4 + 4 + 8 + 4;
/// Kind (0 a slot, 1 crouch, 2 a single crease), status and lip count
/// bytes, then the floats: edge, stand, fall,
/// launch, drift, score, width, need, out, and [`MAX_LIP`] lip edges.
const SPOT_FLOATS: usize = 6 + 3 + 1 + 1 + 2 + 1 + 1 + 1 + 2 + 6 * MAX_LIP;
const SPOT_BYTES: usize = 3 + 4 * SPOT_FLOATS;

/// The scanner's version. **Bump it whenever `superbounce`'s scan changes
/// what it finds**: every file made by another version is scanned again.
pub const SCAN: u32 = 3;

/// The stamp of a map's spots file: the nav generator and the scan
/// version, for the map with `fingerprint`.
pub fn stamp(fingerprint: u64) -> Stamp {
    Stamp {
        generator: crate::cache::GENERATOR | (SCAN << 16),
        fingerprint,
    }
}

/// `<map>.spots`, in the bots directory beside the nav files.
pub fn file_name(map: &str) -> String {
    crate::cache::file_name(map, 0).replace(".0.bots", ".spots")
}

/// The file: magic, version, `stamp`, the count, the spots.
pub fn to_bytes(spots: &[Spot], stamp: Stamp) -> Vec<u8> {
    let mut o = Vec::with_capacity(HEADER + spots.len() * SPOT_BYTES);
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&VERSION.to_le_bytes());
    o.extend_from_slice(&stamp.generator.to_le_bytes());
    o.extend_from_slice(&stamp.fingerprint.to_le_bytes());
    o.extend_from_slice(&(spots.len() as u32).to_le_bytes());
    for s in spots {
        o.push(match (s.kind, s.shape) {
            (Kind::Bounce, Shape::Slot) => 0,
            (Kind::Crouch, _) => 1,
            (Kind::Bounce, Shape::Crease) => 2,
        });
        o.push(match s.status {
            Status::Likely => 0,
            Status::Proven => 1,
            Status::Failed => 2,
        });
        o.push(s.lip.len() as u8);
        let mut lip = [[[0.0f32; 3]; 2]; MAX_LIP];
        lip[..s.lip.len()].copy_from_slice(s.lip.edges());
        let floats = s
            .edge
            .iter()
            .flatten()
            .chain(&s.stand)
            .chain([&s.fall, &s.launch])
            .chain(&s.drift)
            .chain([&s.score, &s.width, &s.need])
            .chain(&s.out)
            .chain(lip.iter().flatten().flatten());
        for f in floats {
            o.extend_from_slice(&f.to_le_bytes());
        }
    }
    o
}

/// The spots and their stamp; `None` for another format or a damaged
/// file.
pub fn from_bytes(b: &[u8]) -> Option<(Vec<Spot>, Stamp)> {
    if b.get(..4)? != MAGIC {
        return None;
    }
    let u32_at = |at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    if u32_at(4)? != VERSION {
        return None;
    }
    let stamp = Stamp {
        generator: u32_at(8)?,
        fingerprint: u64::from_le_bytes(b.get(12..20)?.try_into().ok()?),
    };
    let n = u32_at(20)? as usize;
    let body = b.get(HEADER..)?;
    if body.len() != n.checked_mul(SPOT_BYTES)? {
        return None;
    }
    let spots = body
        .chunks_exact(SPOT_BYTES)
        .map(|c| {
            let f = |k: usize| f32::from_le_bytes(c[3 + 4 * k..7 + 4 * k].try_into().unwrap());
            let v = |k: usize| [f(k), f(k + 1), f(k + 2)];
            let lip_len = usize::from(c[2]);
            if lip_len > MAX_LIP {
                return None;
            }
            let lip: Vec<[[f32; 3]; 2]> = (0..lip_len).map(|e| [v(18 + 6 * e), v(21 + 6 * e)]).collect();
            let (kind, shape) = match c[0] {
                0 => (Kind::Bounce, Shape::Slot),
                1 => (Kind::Crouch, Shape::Slot),
                2 => (Kind::Bounce, Shape::Crease),
                _ => return None,
            };
            Some(Spot {
                kind,
                shape,
                status: match c[1] {
                    0 => Status::Likely,
                    1 => Status::Proven,
                    2 => Status::Failed,
                    _ => return None,
                },
                edge: [v(0), v(3)],
                stand: v(6),
                fall: f(9),
                launch: f(10),
                drift: [f(11), f(12)],
                score: f(13),
                width: f(14),
                need: f(15),
                out: [f(16), f(17)],
                lip: Lip::new(&lip),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((spots, stamp))
}

/// Writes `map`'s spots to `dir`, stamped.
pub fn write(dir: &Path, map: &str, stamp: Stamp, spots: &[Spot]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(map));
    crate::autonav::write_atomic(&path, &to_bytes(spots, stamp))?;
    Ok(path)
}

/// Reads `map`'s spots from `dir` when the file is there and stamped
/// `stamp`.
pub fn read(dir: &Path, map: &str, stamp: Stamp) -> Option<Vec<Spot>> {
    let bytes = std::fs::read(dir.join(file_name(map))).ok()?;
    from_bytes(&bytes).filter(|(_, s)| *s == stamp).map(|(v, _)| v)
}

// ---- the scan ----

/// Every spot on every structure BSP of `map`: the only place that calls
/// the scanner, so a change to its signature (it may take a raw-geometry
/// source) lands here.
pub fn scan<R: std::io::Read + std::io::Seek>(
    map: &mut map::Map<R>,
    bsps: usize,
) -> Result<Vec<Spot>, String> {
    let mut out = Vec::new();
    for k in 0..bsps {
        out.extend(superbounce::scan_map(map, k).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
