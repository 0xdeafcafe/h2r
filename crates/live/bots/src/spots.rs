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
//! - the map's learned file (`<map>.learned` beside them,
//!   [`superbounce::learned`], docs/game/bounce-search.md): the knobs a
//!   training run's search found for each bounce site ([`Site::knobs`]).
//!   A site it found dead is left out unless a game run proved it.
//!
//! The scans of the maps the worker has seen sit in a process-wide table
//! ([`publish`], [`scanned`]); a brain asks it for its map's ([`Known`]).
//! Pure data, no guest memory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use superbounce::learned::{self, Knobs};
use superbounce::verified::{self, Entry};
use superbounce::{Arm, Kind, Lip, MAX_LIP, Shape, Spot, Status};

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

/// A bounce the planner can use: the spot as the bounce run takes it, and
/// what the planner needs to know of its launch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Site {
    pub bounce: Bounce,
    /// Launch speed straight up (world units a second).
    pub launch: f32,
    /// How far the landing must fall onto the slot, as measured
    /// ([`STANDING_FALL`] is what a standing jump gives); 0 for a scanned
    /// site, which has no measured drop ([`Site::drop_known`]).
    pub fall: f32,
    /// The downward speed the landing must touch down at (world units a
    /// second): a measured one, else the one threshold every seam shares
    /// (`superbounce::speed::BOUNCE_SPEED`).
    pub need: f32,
    /// `Proven` when a game run bounced from it (measured, or verified).
    pub status: Status,
    /// Where a verified drop landed: the point to aim at instead of
    /// `stand`.
    pub aim: Option<Vec3>,
    /// The verified drop height over the strip (world units).
    pub height: Option<f32>,
    /// What the map's learned file says of it
    /// (docs/game/bounce-search.md): the knobs the route plays here
    /// (none set: the route's own, `bounce_route::flown`), and its line's
    /// state, `None` with no line.
    pub knobs: Knobs,
    pub line: Option<learned::State>,
    /// A single crease the scan found (`superbounce::Spot::candidate`):
    /// the sim's training flies it, a route or a show-off does not until
    /// it is proved ([`Site::plays`]).
    pub candidate: bool,
    /// A V's arm (`superbounce::Arm`): the side of the V's bisector it is
    /// approached from, the mirror side; the training also flies it from
    /// its own side as a control ([`Site::same_side`]). `None` for every
    /// other site.
    pub arm: Option<Arm>,
    /// The floor's shape (`superbounce::Shape`): a slot, a single crease, a
    /// V or a busy point. A measured site is a slot.
    pub shape: Shape,
    /// The scan's score (`superbounce::Spot::score`, 0 to 1); 0 for a
    /// measured site, which no scan scored.
    pub score: f32,
}

/// A seam scoring this or more has at least two creases converging on
/// its line (docs/game/superbounce.md, "Monster bounces"): a Monster's
/// shape.
pub const MONSTER_SCORE: f32 = 0.85;
/// A launch over this (world units a second) is a Monster Bounce
/// (docs/game/superbounce.md, "The rule"; the game's
/// `superbounce_chain::MONSTER_VZ`).
pub const MONSTER_VZ: f32 = 16.0;

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
                landing: None,
            },
            launch: s.launch,
            fall: s.fall,
            // The one threshold every seam shares, unless measured.
            need: superbounce::speed::need(s),
            status: s.status,
            aim: None,
            height: None,
            knobs: Knobs::default(),
            line: None,
            candidate: s.candidate(),
            arm: s.arm,
            shape: s.shape,
            score: s.score,
        })
    }

    /// A measured spot as a site: a drop from its takeoff
    /// ([`MEASURED_FALL`]) when it has one, else a standing jump, aimed
    /// at its traced landing when it has one.
    pub fn measured(b: Bounce) -> Site {
        Site {
            bounce: b,
            launch: MEASURED_LAUNCH,
            fall: if b.takeoff.is_some() {
                MEASURED_FALL
            } else {
                0.0
            },
            need: MEASURED_NEED,
            status: Status::Proven,
            aim: b.landing,
            height: None,
            knobs: Knobs::default(),
            line: None,
            candidate: false,
            arm: None,
            shape: Shape::Slot,
            score: 0.0,
        }
    }

    /// A V's arm approached from its own side instead of the mirror: the
    /// training's control (docs/game/bounce-search.md, "Vs: the mirror
    /// and its control"). The approach side and the drift turn round, and
    /// the learned knobs go (they are the mirror's: the control's search
    /// keeps its own line). Any other site as it is.
    pub fn same_side(self) -> Site {
        let Some(a) = self.arm else {
            return self;
        };
        let d = self.bounce.drift;
        Site {
            bounce: Bounce {
                drift: [-d[0], -d[1]],
                ..self.bounce
            },
            knobs: Knobs::default(),
            line: None,
            arm: Some(a.same_side()),
            ..self
        }
    }

    /// Whether `xy` is on the side this site is approached from: anywhere
    /// but a V's arm's other side.
    pub fn approached_from(&self, xy: [f32; 2]) -> bool {
        self.arm.is_none_or(|a| a.approached_from(xy))
    }

    /// With what a verified entry about it says: its status, and where and
    /// from how high the drop that worked landed. A measured spot keeps
    /// its traced landing as the aim: the middle of every traced bounce,
    /// where an entry's is one landing, the origin a tick before contact.
    pub fn verified(mut self, e: Option<&Entry>) -> Site {
        if let Some(e) = e {
            self.status = e.status;
            if e.status == Status::Proven {
                self.aim = self.bounce.landing.or(Some(e.working()));
                self.height = e.height;
                if e.best > 0.0 {
                    self.launch = e.best;
                }
            }
        }
        self
    }

    /// With its line of the map's learned file: the knobs of an `open`,
    /// `partial` or `learned` line ([`learned::knobs_for`]) and the line's
    /// state.
    pub fn learned(mut self, lines: &[learned::Entry]) -> Site {
        self.knobs = learned::knobs_for(lines, self.bounce.stand);
        self.line = learned::entry_for(lines, self.bounce.stand).map(|e| e.state);
        self
    }

    /// Whether a route or a show-off may bounce here: anything but a
    /// candidate (a single crease or a V), which only plays once a game run
    /// proved it. A learned line proves nothing (the training writes the
    /// word, docs/game/bounce-search.md, "Rules and limits").
    pub fn plays(&self) -> bool {
        !self.candidate || self.proven()
    }

    /// Whether anything measured the drop onto it: a measured fall, a
    /// verified height or a recipe's takeoff. A scanned site has none (the
    /// scan carries no `fall`), so the training tries the drops as well as
    /// the stand (`mind::practice::site_in`), from a floor the speed
    /// model's drop over it (`bounce_route::launch_floor`).
    pub fn drop_known(&self) -> bool {
        self.fall > 0.0 || self.height.is_some() || self.bounce.takeoff.is_some()
    }

    /// A standing jump from the spot's `stand` reaches it. A learned `how`
    /// says so first: `stand` is a standing jump, `runoff` and `jump` a
    /// drop.
    pub fn standing(&self) -> bool {
        match self.knobs.how {
            Some(h) => !h.is_drop(),
            None => {
                self.fall <= STANDING_FALL && self.height.is_none() && self.bounce.takeoff.is_none()
            }
        }
    }

    /// Where to land: the learned aim (`ax ay`), else the verified point,
    /// else `stand`.
    pub fn target(&self) -> Vec3 {
        let own = self.own_target();
        match self.knobs.aim {
            Some([x, y]) => [x, y, own[2]],
            None => own,
        }
    }

    /// Where to land without the learned line: the verified point, else
    /// `stand`. A training try's miss is measured from it, so moving the
    /// aim moves the miss (`learn::Try::from_leap`).
    pub fn own_target(&self) -> Vec3 {
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

    /// Whether it has a Monster's shape or launch (docs/game/superbounce.md,
    /// "Monster bounces"): lines converging on it (a seam scoring
    /// [`MONSTER_SCORE`] or more, a V, a busy point), or a launch over
    /// [`MONSTER_VZ`] (a verified best, or the scan's estimate).
    pub fn monster(&self) -> bool {
        let shape = match self.shape {
            Shape::Vee | Shape::Busy => true,
            Shape::Slot | Shape::Crease => self.score >= MONSTER_SCORE,
        };
        shape || self.launch > MONSTER_VZ
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
    /// scanned ([`scanned`]), with its verified file ([`verified_for`])
    /// and its learned file ([`learned_for`], [`Known::with_learned`]).
    pub fn for_map(map: &str) -> Known {
        Known::merge_verified(
            crate::glitches::bounces(map),
            &scanned(map),
            &verified_for(map),
        )
        .with_learned(&learned_for(map))
    }

    /// With the map's learned file: each site gets its line's knobs
    /// ([`Site::learned`]); a site whose line is `dead` is left out, as a
    /// verified `failed` one is, unless it is proven (a measured spot, or
    /// one a verified entry proved). A `blocked` line changes nothing.
    pub fn with_learned(mut self, lines: &[learned::Entry]) -> Known {
        if lines.is_empty() {
            return self;
        }
        self.sites = self
            .sites
            .into_iter()
            .map(|s| s.learned(lines))
            .filter(|s| s.proven() || s.line != Some(learned::State::Dead))
            .collect();
        self
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
            .map(|b| {
                Site::measured(b).verified(verified::entry_for(ver, &probe(Kind::Bounce, b.stand)))
            })
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
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    /// `bounce` or `crouch`.
    pub kind: &'static str,
    /// `measured` or `scan`.
    pub source: &'static str,
    pub spot: Spot,
    /// A bounce spot's line of the map's learned file, when it has one.
    pub learned: Option<learned::Entry>,
}

/// Every spot of `map`: the measured ones first, then the scan, best
/// first, each with its verified status (failed ones too) and, a bounce
/// spot, its learned line. A measured spot's edge is the point it stands
/// on, its score 1, its status proven.
pub fn listing(map: &str) -> Vec<Listed> {
    let ver = verified_for(map);
    let lines = learned_for(map);
    let line = |kind: Kind, stand: Vec3| {
        (kind == Kind::Bounce)
            .then(|| learned::entry_for(&lines, stand).cloned())
            .flatten()
    };
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
            learned: line(Kind::Bounce, b.stand),
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
        learned: line(s.kind, s.stand),
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

/// Changes whenever [`publish`] stores a scan, [`store_verified`] a
/// verified file or [`store_learned`] a learned one: a brain compares it with the one it last read to know
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
    /// The learned files read from there ([`learned_for`]).
    learned: HashMap<String, Arc<Vec<learned::Entry>>>,
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
        v.learned.clear();
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

// ---- the learned files ----

/// `<map>.learned`, in the bots directory beside the verified files
/// ([`crate::harvest::learned_file_name`]).
pub fn learned_file_name(map: &str) -> String {
    crate::harvest::learned_file_name(map)
}

/// Reads `map`'s learned file from `dir`; empty when there is none. A
/// refused file (`learned::parse`) reads as an error naming the line.
pub fn read_learned(dir: &Path, map: &str) -> Result<Vec<learned::Entry>, String> {
    match std::fs::read_to_string(dir.join(learned_file_name(map))) {
        Ok(text) => learned::parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// What `map`'s learned file says (any case): read once from the
/// verified directory ([`set_verified_dir`]), as [`verified_for`] reads
/// its file, then kept; empty with no directory or no file, and empty with
/// one log line for a refused file (it is trained again from nothing).
pub fn learned_for(map: &str) -> Arc<Vec<learned::Entry>> {
    let key = map.to_ascii_lowercase();
    let mut v = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    let v = v.get_or_insert_with(Verified::default);
    if let Some(e) = v.learned.get(&key) {
        return Arc::clone(e);
    }
    let entries = match &v.dir {
        Some(dir) if !key.is_empty() => read_learned(dir, &key).unwrap_or_else(|why| {
            eprintln!("bots: {key}'s learned file not read: {why}");
            Vec::new()
        }),
        _ => Vec::new(),
    };
    let entries = Arc::new(entries);
    v.learned.insert(key, Arc::clone(&entries));
    entries
}

/// `old` with each of `lines` in place of the line about its site
/// (`learned::replace`): a training run's lines are its search, further
/// on.
fn with_lines(old: &[learned::Entry], lines: &[learned::Entry]) -> Vec<learned::Entry> {
    lines
        .iter()
        .fold(old.to_vec(), |all, l| learned::replace(&all, l.clone()))
}

/// Writes `map`'s learned file in `dir` (atomic) with `lines` in place of
/// the lines about their sites, the others kept. The path and what it
/// holds now.
pub fn write_learned(
    dir: &Path,
    map: &str,
    lines: &[learned::Entry],
) -> std::io::Result<(PathBuf, Vec<learned::Entry>)> {
    let key = map.to_ascii_lowercase();
    let old = read_learned(dir, &key).unwrap_or_default();
    let all = with_lines(&old, lines);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(learned_file_name(&key));
    crate::autonav::write_atomic(&path, learned::to_text(&all).as_bytes())?;
    Ok((path, all))
}

/// A training run's round is over (docs/game/bounce-search.md,
/// "Scheduling a batch"): `lines` replace the lines about their sites in
/// `map`'s learned file, written (atomic) when there is a verified
/// directory, and the [`generation`] is bumped so brains read their spots
/// again. The path written, if any. A game never calls it.
pub fn store_learned(map: &str, lines: &[learned::Entry]) -> std::io::Result<Option<PathBuf>> {
    let key = map.to_ascii_lowercase();
    let (written, all) = match verified_dir() {
        Some(dir) if !key.is_empty() => {
            let (path, all) = write_learned(&dir, &key, lines)?;
            (Some(path), all)
        }
        _ => (None, with_lines(&learned_for(&key), lines)),
    };
    let mut v = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    v.get_or_insert_with(Verified::default)
        .learned
        .insert(key, Arc::new(all));
    GENERATION.fetch_add(1, Ordering::Release);
    Ok(written)
}

// ---- reloading from the files ----

/// What [`reload_in`] read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reloaded {
    /// `read` (the scan was replaced by the file's), `absent` (no file;
    /// the scan stays) or `unread: <why>` (damaged, another format or
    /// another scanner's; the scan stays).
    pub spots_file: String,
    /// The lines of the verified and learned files now kept.
    pub verified: usize,
    pub learned: usize,
}

/// Reads `map`'s `.spots`, `.verified` and `.learned` files from `dir`
/// again (any case of `map`) and keeps them in place of what was kept,
/// bumping the [`generation`] so every reader (the mode, the brains)
/// applies them again. The `.spots` file is taken when it is the current
/// scanner's ([`stamp`]'s generator): its map fingerprint is not checked,
/// since the game has no map to check it against here. A file that is not
/// read leaves the kept data as it was.
pub fn reload_in(dir: &Path, map: &str) -> Reloaded {
    let key = map.to_ascii_lowercase();
    let spots_file = match std::fs::read(dir.join(file_name(&key))) {
        Ok(bytes) => match from_bytes(&bytes) {
            Some((found, stamp)) if stamp.generator == self::stamp(0).generator => {
                let mut t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
                t.get_or_insert_with(Table::default)
                    .maps
                    .insert(key.clone(), Arc::new(found));
                "read".to_string()
            }
            Some(_) => "unread: made by another scanner version".to_string(),
            None => "unread: not a spots file of this format".to_string(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent".to_string(),
        Err(e) => format!("unread: {e}"),
    };
    let verified = read_verified(dir, &key).unwrap_or_else(|why| {
        eprintln!("bots: {key}'s verified spots not read: {why}");
        Vec::new()
    });
    let learned = read_learned(dir, &key).unwrap_or_else(|why| {
        eprintln!("bots: {key}'s learned file not read: {why}");
        Vec::new()
    });
    let out = Reloaded {
        spots_file,
        verified: verified.len(),
        learned: learned.len(),
    };
    let mut v = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    let v = v.get_or_insert_with(Verified::default);
    v.maps.insert(key.clone(), Arc::new(verified));
    v.learned.insert(key, Arc::new(learned));
    GENERATION.fetch_add(1, Ordering::Release);
    out
}

/// [`reload_in`] the verified directory ([`set_verified_dir`]); `None`
/// without one.
pub fn reload(map: &str) -> Option<(PathBuf, Reloaded)> {
    let dir = verified_dir()?;
    let r = reload_in(&dir, map);
    Some((dir, r))
}

// ---- the file ----

/// A spots file's first bytes and format version.
pub const MAGIC: &[u8; 4] = b"H2SP";
pub const VERSION: u32 = 5;
const HEADER: usize = 4 + 4 + 4 + 8 + 4;
/// Kind (0 a slot, 1 crouch, 2 a single crease, 3 a V, 4 a busy point),
/// status, lip count and arm (0 none, 1 a V's arm) bytes, then the
/// floats: edge, stand, fall, launch, drift, score, need, out,
/// [`MAX_LIP`] lip edges, and the arm's pivot, bisector and approach side
/// (zero for none).
const SPOT_FLOATS: usize = 6 + 3 + 1 + 1 + 2 + 1 + 1 + 2 + 6 * MAX_LIP + 3 + 2 + 2;
const SPOT_BYTES: usize = 4 + 4 * SPOT_FLOATS;
/// The first arm float.
const ARM_AT: usize = 17 + 6 * MAX_LIP;

/// The scanner's version. **Bump it whenever `superbounce`'s scan changes
/// what it finds**: every file made by another version is scanned again.
pub const SCAN: u32 = 7;

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
            (Kind::Bounce, Shape::Vee) => 3,
            (Kind::Bounce, Shape::Busy) => 4,
        });
        o.push(match s.status {
            Status::Likely => 0,
            Status::Proven => 1,
            Status::Failed => 2,
        });
        o.push(s.lip.len() as u8);
        o.push(u8::from(s.arm.is_some()));
        let arm = s.arm.unwrap_or_default();
        let mut lip = [[[0.0f32; 3]; 2]; MAX_LIP];
        lip[..s.lip.len()].copy_from_slice(s.lip.edges());
        let floats = s
            .edge
            .iter()
            .flatten()
            .chain(&s.stand)
            .chain([&s.fall, &s.launch])
            .chain(&s.drift)
            .chain([&s.score, &s.need])
            .chain(&s.out)
            .chain(lip.iter().flatten().flatten())
            .chain(&arm.pivot)
            .chain(&arm.bisector)
            .chain(&arm.from);
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
            let f = |k: usize| f32::from_le_bytes(c[4 + 4 * k..8 + 4 * k].try_into().unwrap());
            let v = |k: usize| [f(k), f(k + 1), f(k + 2)];
            let lip_len = usize::from(c[2]);
            if lip_len > MAX_LIP {
                return None;
            }
            let lip: Vec<[[f32; 3]; 2]> = (0..lip_len)
                .map(|e| [v(17 + 6 * e), v(20 + 6 * e)])
                .collect();
            let (kind, shape) = match c[0] {
                0 => (Kind::Bounce, Shape::Slot),
                1 => (Kind::Crouch, Shape::Slot),
                2 => (Kind::Bounce, Shape::Crease),
                3 => (Kind::Bounce, Shape::Vee),
                4 => (Kind::Bounce, Shape::Busy),
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
                need: f(14),
                out: [f(15), f(16)],
                lip: Lip::new(&lip),
                arm: match c[3] {
                    0 => None,
                    1 => Some(Arm {
                        pivot: v(ARM_AT),
                        bisector: [f(ARM_AT + 3), f(ARM_AT + 4)],
                        from: [f(ARM_AT + 5), f(ARM_AT + 6)],
                    }),
                    _ => return None,
                },
                // The cache holds the scan; measured landings are applied after.
                landed: false,
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
    from_bytes(&bytes)
        .filter(|(_, s)| *s == stamp)
        .map(|(v, _)| v)
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
