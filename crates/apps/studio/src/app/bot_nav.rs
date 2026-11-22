//! The bots' nav mesh and superbounce spots for the open map, for the
//! Bot nav mesh and Bot superbounce spots layers (docs/editor/editor.md,
//! "Navigation"; docs/game/bots.md, §3.2 and "Superbounce routes").
//!
//! Loaded on its own thread when one of those layers is first shown for a
//! map, or when `nav.bots` asks:
//!
//! - each structure BSP's `<data>/bots/<map>.<bsp>.bots`, when its stamp is
//!   this generator's and this map file's (`bots::cache`);
//! - otherwise the mesh is built in memory with the bots' own builder
//!   (`bots::autonav::build_map`, what `h2 bots nav` runs) and not written,
//!   with a note saying why;
//! - the spots: `<map>.spots` when its stamp is current, else scanned now
//!   (`bots::spots::scan`); the hand-measured ones (`bots::glitches`); and
//!   what `<map>.verified`, `<map>.learned` and `<map>.ghosts` beside them
//!   say of each ([`spot_state`]).
//!
//! The result goes to the session ([`scenario_edit::Session::set_bot_nav`]),
//! which keeps it with the map.
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use scenario_edit::MapSource;
use scenario_edit::layers::LayerId;
use scenario_edit::nav_view::{
    BotLink, BotLinkKind, BotNav, BotPoly, BotSpot, SpotKind, SpotState,
};

use super::App;

/// The bots' data load for one open map.
#[derive(Default)]
pub struct BotNavLoad {
    /// The session world (`Session::world_rev`) it was started for.
    world: Option<u64>,
    rx: Option<mpsc::Receiver<Result<BotNav, String>>>,
    /// Why the last load for this world failed.
    error: Option<String>,
}

/// Where a load stands, as `nav.bots` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotNavState {
    NoMap,
    /// Not asked for yet: no bot layer has been shown.
    Idle,
    Loading,
    Ready,
    Failed,
}

impl BotNavState {
    pub const fn name(self) -> &'static str {
        match self {
            Self::NoMap => "no_map",
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl App {
    /// Where the open map's bot data stands.
    pub fn bot_nav_state(&self) -> BotNavState {
        if !self.session.is_open() {
            return BotNavState::NoMap;
        }
        if self.session.bot_nav().is_some() {
            return BotNavState::Ready;
        }
        let current = self.bot_nav.world == Some(self.session.world_rev());
        match (current, &self.bot_nav.rx, &self.bot_nav.error) {
            (true, Some(_), _) => BotNavState::Loading,
            (true, None, Some(_)) => BotNavState::Failed,
            _ => BotNavState::Idle,
        }
    }

    /// Why the open map's bot data did not load.
    pub fn bot_nav_error(&self) -> Option<&str> {
        (self.bot_nav.world == Some(self.session.world_rev()))
            .then_some(self.bot_nav.error.as_deref())
            .flatten()
    }

    /// Start loading the open map's bot data, once per map.
    pub fn want_bot_nav(&mut self) {
        let Some(source) = self.session.source().cloned() else {
            return;
        };
        let world = self.session.world_rev();
        if self.bot_nav.world == Some(world) || self.session.bot_nav().is_some() {
            return;
        }
        let dir = bots::cache::dir(
            &self
                .data_root
                .clone()
                .unwrap_or_else(content::store::default_data_root),
        );
        let bsps = self
            .session
            .pathfinding()
            .map_or(0, |p| p.bsps.len())
            .max(self.session.collision().map_or(0, |c| c.bsps.len()));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                load(&source, &dir, bsps)
            }))
            .unwrap_or_else(|_| Err("the bots' loader panicked".into()));
            let _ = tx.send(r);
        });
        self.bot_nav = BotNavLoad {
            world: Some(world),
            rx: Some(rx),
            error: None,
        };
        self.info("reading the bots' nav mesh...");
    }

    /// Each frame: start a load while a bot layer shows, and pick up a
    /// finished one.
    pub fn poll_bot_nav(&mut self) {
        if self.layers.visible(LayerId::BotNav) || self.layers.visible(LayerId::BotSpots) {
            self.want_bot_nav();
        }
        let Some(rx) = self.bot_nav.rx.as_ref() else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the bots' loader stopped".into()),
        };
        self.finish_bot_nav(result);
    }

    /// Wait for the load in progress (tests, `nav.bots` with `wait`).
    pub fn wait_bot_nav(&mut self) {
        if let Some(rx) = self.bot_nav.rx.as_ref() {
            let result = rx
                .recv()
                .unwrap_or_else(|_| Err("the bots' loader stopped".into()));
            self.finish_bot_nav(result);
        }
    }

    fn finish_bot_nav(&mut self, result: Result<BotNav, String>) {
        self.bot_nav.rx = None;
        // A map opened since the load started is not its map.
        if self.bot_nav.world != Some(self.session.world_rev()) {
            self.bot_nav.world = None;
            return;
        }
        match result {
            Ok(nav) => {
                self.info(format!(
                    "bots: {}: {} polygons ({} out of the map), {} links, {} spots",
                    nav.map,
                    nav.polys.len(),
                    nav.out_of_map_count(),
                    nav.links.len(),
                    nav.spots.len()
                ));
                self.session.set_bot_nav(Some(Arc::new(nav)));
            }
            Err(e) => {
                self.error(format!("the bots' nav mesh: {e}"));
                self.bot_nav.error = Some(e);
            }
        }
    }
}

/// Read (or build) the bots' data for the map at `source`, with `bsps`
/// structure BSPs, from the bots directory `dir`.
pub fn load(source: &MapSource, dir: &Path, bsps: usize) -> Result<BotNav, String> {
    let mut map = source.open().map_err(|e| format!("opening the map: {e}"))?;
    let header = map
        .read_file_range(0, bots::autonav::MAP_HEADER_LEN as u64)
        .map_err(|e| format!("reading the map's header: {e}"))?;
    let header = bots::autonav::MapHeader::parse(&header)?;
    let name = header.name.clone();
    let mut nav = BotNav {
        map: name.clone(),
        ..BotNav::default()
    };
    let want = bots::cache::Stamp::now(header.fingerprint);
    let mut cached = Vec::new();
    let mut stale = None;
    for k in 0..bsps.max(1) {
        let path = dir.join(bots::cache::file_name(&name, k));
        let shown = path.display().to_string();
        let why = match std::fs::read(&path) {
            Err(_) => format!("no {shown}"),
            Ok(bytes) => match bots::cache::MapData::from_bytes(&bytes) {
                Some((data, stamp)) if stamp == want => {
                    cached.push((shown, data));
                    continue;
                }
                Some((_, stamp)) if stamp.generator != want.generator => format!(
                    "{shown} was built by nav generator {}, this one is {}",
                    stamp.generator, want.generator
                ),
                Some(_) => format!("{shown} was built from another map file"),
                None => format!("{shown} is another format or damaged"),
            },
        };
        stale = Some(why);
        break;
    }
    let data = match stale {
        None => cached,
        Some(why) => {
            nav.notes.push(format!(
                "{why}: built in memory with the bots' builder and not written \
                 (`h2 bots nav {name}` writes it)"
            ));
            bots::autonav::build_map(&mut map)?
                .into_iter()
                .map(|d| ("built".to_owned(), d))
                .collect()
        }
    };
    for (bsp, (origin, d)) in data.into_iter().enumerate() {
        add_mesh(&mut nav, bsp, origin, &d);
    }
    nav.mark_one_way();
    let scan = match bots::spots::read(dir, &name, bots::spots::stamp(header.fingerprint)) {
        Some(found) => found,
        None => {
            let n = nav.origins.len().max(1);
            let scanned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                bots::spots::scan(&mut map, n)
            }))
            .unwrap_or_else(|_| Err("the scan panicked".into()));
            match scanned {
                Ok(found) => {
                    nav.notes.push(format!(
                        "no current {}: the spots were scanned now",
                        bots::spots::file_name(&name)
                    ));
                    found
                }
                Err(e) => {
                    nav.notes.push(format!("the spots scan failed: {e}"));
                    Vec::new()
                }
            }
        }
    };
    nav.spots = spots(dir, &name, &scan, &mut nav.notes);
    Ok(nav)
}

/// One structure BSP's mesh, appended: its polygons, and its links with
/// their ends moved past the polygons already there.
pub fn add_mesh(nav: &mut BotNav, bsp: usize, origin: String, d: &bots::cache::MapData) {
    let base = nav.polys.len();
    nav.origins.push(origin);
    nav.walls += d.walls.tris.len();
    nav.polys
        .extend(d.nav.polys.iter().enumerate().map(|(i, p)| BotPoly {
            bsp,
            poly: i,
            verts: p.verts.clone(),
            normal: p.normal,
            out_of_map: !d.nav.in_map(i as u32),
        }));
    nav.links
        .extend(d.nav.edges.iter().enumerate().map(|(i, e)| BotLink {
            bsp,
            link: i,
            from: base + e.from as usize,
            to: base + e.to as usize,
            a: e.a,
            b: e.b,
            kind: link_kind(e.kind),
            one_way: false,
        }));
}

pub fn link_kind(k: bots::nav::Kind) -> BotLinkKind {
    use bots::nav::Kind;
    match k {
        Kind::Walk => BotLinkKind::Walk,
        Kind::Drop => BotLinkKind::Drop,
        Kind::Jump => BotLinkKind::Jump,
        Kind::CrouchJump => BotLinkKind::CrouchJump,
        Kind::Teleporter => BotLinkKind::Teleporter,
        Kind::Lift => BotLinkKind::Lift,
        Kind::Elevator => BotLinkKind::Elevator,
    }
}

/// What the files say of a spot, the strongest first: a verified (or
/// measured) status, then the learned line's state.
pub fn spot_state(
    status: superbounce::Status,
    line: Option<superbounce::learned::State>,
) -> SpotState {
    use superbounce::Status;
    use superbounce::learned::State;
    match (status, line) {
        (Status::Proven, _) => SpotState::Proven,
        (Status::Failed, _) => SpotState::Failed,
        (_, Some(State::Dead)) => SpotState::Dead,
        (_, Some(State::Learned | State::Partial)) => SpotState::Learned,
        (_, Some(State::Open | State::Blocked)) => SpotState::Training,
        (Status::Likely, None) => SpotState::Likely,
    }
}

/// Read a text file beside the spots, `None` when it is not there; a
/// file that does not read or parse adds a note and reads as empty.
fn side_file<T>(
    dir: &Path,
    file: String,
    parse: impl FnOnce(&str) -> Result<Vec<T>, String>,
    notes: &mut Vec<String>,
) -> Vec<T> {
    let path: PathBuf = dir.join(&file);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).unwrap_or_else(|e| {
            notes.push(format!("{file} not read: {e}"));
            Vec::new()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            notes.push(format!("{file} not read: {e}"));
            Vec::new()
        }
    }
}

/// The map's spots: the hand-measured ones, then the scan, each with its
/// verified entry, learned line and ghost run.
pub fn spots(
    dir: &Path,
    map: &str,
    scan: &[superbounce::Spot],
    notes: &mut Vec<String>,
) -> Vec<BotSpot> {
    use superbounce::{Kind, Shape, Status, ghost, learned, verified};
    let ver = side_file(
        dir,
        bots::spots::verified_file_name(map),
        verified::parse,
        notes,
    );
    let lines = side_file(
        dir,
        bots::spots::learned_file_name(map),
        learned::parse,
        notes,
    );
    let ghosts = side_file(
        dir,
        bots::harvest::ghosts_file_name(map),
        ghost::parse,
        notes,
    );
    let measured: Vec<superbounce::Spot> = bots::glitches::bounces(map)
        .into_iter()
        .map(|b| superbounce::Spot {
            kind: Kind::Bounce,
            edge: [b.stand, b.stand],
            stand: b.stand,
            fall: bots::spots::Site::measured(b).fall,
            launch: bots::spots::MEASURED_LAUNCH,
            score: 1.0,
            need: bots::spots::MEASURED_NEED,
            status: Status::Proven,
            ..superbounce::Spot::default()
        })
        .collect();
    let all = measured
        .iter()
        .map(|s| ("measured", s))
        .chain(scan.iter().map(|s| ("scan", s)));
    all.map(|(source, s)| {
        let entry = verified::entry_for(&ver, s);
        let line = (s.kind == Kind::Bounce)
            .then(|| learned::entry_for(&lines, s.stand))
            .flatten();
        let status = entry.map_or(s.status, |e| e.status);
        BotSpot {
            kind: Some(match (s.kind, s.shape) {
                (Kind::Crouch, _) => SpotKind::Crouch,
                (Kind::Bounce, Shape::Slot) => SpotKind::Bounce,
                (Kind::Bounce, Shape::Crease) => SpotKind::Strip,
            }),
            source,
            stand: s.stand,
            edge: s.edge,
            lip: s.lip.edges().to_vec(),
            out: s.out,
            state: Some(spot_state(status, line.map(|l| l.state))),
            score: s.score,
            launch: s.launch,
            fall: s.fall,
            line: line.map(|l| l.state.name()),
            tries: line.map_or(0, |l| l.tries),
            hits: line.map_or(0, |l| l.hits),
            trials: entry.map_or(0, |e| e.trials),
            ok: entry.map_or(0, |e| e.ok),
            ghost: s.kind == Kind::Bounce && ghost::for_spot(&ghosts, s.stand).is_some(),
        }
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbounce::Status;
    use superbounce::learned::State;

    #[test]
    fn a_verified_status_outranks_the_learned_line() {
        assert_eq!(
            spot_state(Status::Proven, Some(State::Dead)),
            SpotState::Proven
        );
        assert_eq!(
            spot_state(Status::Failed, Some(State::Learned)),
            SpotState::Failed
        );
        assert_eq!(
            spot_state(Status::Likely, Some(State::Dead)),
            SpotState::Dead
        );
        assert_eq!(
            spot_state(Status::Likely, Some(State::Partial)),
            SpotState::Learned
        );
        assert_eq!(
            spot_state(Status::Likely, Some(State::Blocked)),
            SpotState::Training
        );
        assert_eq!(spot_state(Status::Likely, None), SpotState::Likely);
    }

    #[test]
    fn link_kinds_keep_their_names() {
        use bots::nav::Kind;
        let names: Vec<_> = [
            Kind::Walk,
            Kind::Drop,
            Kind::Jump,
            Kind::CrouchJump,
            Kind::Teleporter,
            Kind::Lift,
            Kind::Elevator,
        ]
        .into_iter()
        .map(|k| link_kind(k).name())
        .collect();
        assert_eq!(
            names,
            [
                "walk",
                "drop",
                "jump",
                "crouch_jump",
                "teleporter",
                "lift",
                "elevator"
            ]
        );
    }
}
