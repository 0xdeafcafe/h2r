//! The superbounce and forced-crouch spots the bots know for a map
//! (docs/game/bots.md, "Superbounce routes").
//!
//! Two sources, measured first:
//!
//! - the hand-measured spots ([`crate::glitches::SPOTS`]), a place a game
//!   run bounced from;
//! - the scanned spots (`superbounce::Spot`, found in the map's collision
//!   geometry), made while the nav mesh is built (`crate::autonav`) and
//!   saved beside it as `<map>.spots`, stamped like the nav files, so a
//!   map's spots are found once per map and generator.
//!
//! The scans of the maps the worker has seen sit in a process-wide table
//! ([`publish`], [`scanned`]); a brain asks it for its map's ([`Known`]).
//! Pure data, no guest memory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use superbounce::{Kind, Spot};

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
/// The most a bounce site may need to fall onto it (world units) for a
/// bot's standing jump to reach it.
pub const STANDING_FALL: f32 = 0.5;

/// A bounce the planner can use: the spot as the bounce run takes it, and
/// what the planner needs to know of its launch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Site {
    pub bounce: Bounce,
    /// Launch speed straight up (world units a second).
    pub launch: f32,
    /// How far the landing must fall onto the slot ([`STANDING_FALL`] is
    /// what a standing jump gives).
    pub fall: f32,
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
            },
            launch: s.launch,
            fall: s.fall,
        })
    }

    /// A measured spot as a site.
    pub fn measured(b: Bounce) -> Site {
        Site {
            bounce: b,
            launch: MEASURED_LAUNCH,
            fall: 0.0,
        }
    }

    /// A standing jump from the spot's `stand` reaches it.
    pub fn standing(&self) -> bool {
        self.fall <= STANDING_FALL
    }
}

/// What a bot knows of one map's spots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Known {
    /// Bounce sites, measured first, then scanned best first.
    pub sites: Vec<Site>,
    /// Where a standing bot is forced into a crouch (the places to prime).
    pub crouches: Vec<Vec3>,
}

impl Known {
    /// The spots of `map` (its short name, any case): measured, then
    /// scanned ([`scanned`]).
    pub fn for_map(map: &str) -> Known {
        Known::merge(crate::glitches::bounces(map), &scanned(map))
    }

    /// Measured spots first; scans after, best first, without a scanned
    /// bounce at a measured one's place.
    pub fn merge(measured: Vec<Bounce>, scan: &[Spot]) -> Known {
        let mut sites: Vec<Site> = measured.into_iter().map(Site::measured).collect();
        let kept = sites.len();
        let mut scan: Vec<&Spot> = scan.iter().filter(|s| s.score >= MIN_SCORE).collect();
        scan.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut crouches = Vec::new();
        for s in scan {
            match s.kind {
                Kind::Bounce => {
                    let same = sites[..kept]
                        .iter()
                        .any(|m| dist_xy(m.bounce.stand, s.stand) < SAME_SPOT);
                    if !same && let Some(site) = Site::from_spot(s) {
                        sites.push(site);
                    }
                }
                Kind::Crouch => crouches.push(s.stand),
            }
        }
        Known { sites, crouches }
    }

    /// The crouch spot nearest `at` (straight line, XY), if any.
    pub fn nearest_crouch(&self, at: Vec3) -> Option<Vec3> {
        self.crouches
            .iter()
            .copied()
            .min_by(|a, b| dist_xy(*a, at).total_cmp(&dist_xy(*b, at)))
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
/// first. A measured spot's edge is the point it stands on, its score 1.
pub fn listing(map: &str) -> Vec<Listed> {
    let mut out: Vec<Listed> = crate::glitches::bounces(map)
        .into_iter()
        .map(|b| Listed {
            kind: "bounce",
            source: "measured",
            spot: Spot {
                kind: Kind::Bounce,
                edge: [b.stand, b.stand],
                stand: b.stand,
                fall: 0.0,
                launch: MEASURED_LAUNCH,
                drift: b.drift,
                score: 1.0,
            },
        })
        .collect();
    let scan = scanned(map);
    let mut scan: Vec<&Spot> = scan.iter().collect();
    scan.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.extend(scan.into_iter().map(|s| Listed {
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

/// Changes whenever [`publish`] stores a scan: a brain compares it with
/// the one it last read to know its map's spots may have arrived.
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

// ---- the file ----

/// A spots file's first bytes and format version.
pub const MAGIC: &[u8; 4] = b"H2SP";
pub const VERSION: u32 = 1;
const HEADER: usize = 4 + 4 + 4 + 8 + 4;
const SPOT_BYTES: usize = 1 + 4 * (6 + 3 + 1 + 1 + 2 + 1);

/// The scanner's version. **Bump it whenever `superbounce`'s scan changes
/// what it finds**: every file made by another version is scanned again.
pub const SCAN: u32 = 1;

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
        o.push(match s.kind {
            Kind::Bounce => 0,
            Kind::Crouch => 1,
        });
        let floats = s
            .edge
            .iter()
            .flatten()
            .chain(&s.stand)
            .chain([&s.fall, &s.launch])
            .chain(&s.drift)
            .chain([&s.score]);
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
            let f = |k: usize| f32::from_le_bytes(c[1 + 4 * k..5 + 4 * k].try_into().unwrap());
            Some(Spot {
                kind: match c[0] {
                    0 => Kind::Bounce,
                    1 => Kind::Crouch,
                    _ => return None,
                },
                edge: [[f(0), f(1), f(2)], [f(3), f(4), f(5)]],
                stand: [f(6), f(7), f(8)],
                fall: f(9),
                launch: f(10),
                drift: [f(11), f(12)],
                score: f(13),
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
