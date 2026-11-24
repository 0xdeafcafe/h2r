//! AI ground in Studio (docs/game/game-types-without-carriers.md, "In
//! Studio"): the ground labels placed on the open map, **Build AI ground**
//! and what it made.
//!
//! A label is a netgame flag with a label (`scenario_edit::FlagLabel`),
//! placed by [`App::place_label_at`], the one path the Scene tab's AI and
//! objectives group and `scenario.add {block: "label"}` both take. The
//! build is `firefight_kit::ground::build` over the open map as edited,
//! with the labels, on its own thread; nothing builds while placing and
//! nothing at open. The built ground's pathfinding is shown in the
//! Navigation layers in place of the map's own, its problems go to
//! Problems (source `ground`), and `ground.status` says whether it is
//! current.
//!
//! **The stale rule** is the kit's ([`Receipt::stale`]) with Studio's own
//! base: the digest of the map's name and [`Scenario::collision_stamp`]
//! (the placed solids and the Modeling tool's writes), so a spawn moved
//! or a script edited leaves the ground current, and a crate moved marks
//! it stale.
//!
//! The labels and the built ground live in the open session. They are not
//! yet written with the map's package: the `map` item's `ground` table
//! comes with the package model's whole-map items.
use std::io::Cursor;
use std::sync::mpsc;

use content::Sha256;
use firefight_kit::{
    Bases, Costs, Mode, Team,
    ground::{
        self, BaseMarker, Labels, LineMarker, Marker, Problem as GroundProblem, Receipt, Recipe,
        Stale,
    },
};
use scenario_edit::nav_view::LevelPathfinding;
use scenario_edit::{BlockKind, LabelKind, Scenario};
use serde_json::{Value, json};

use super::App;
use super::problems::Problem;
use crate::api::checks::Severity;

/// What a finished build keeps: the counts, the problems, the cost and the
/// receipt of its inputs. The built map is not kept; its pathfinding is
/// the session's.
#[derive(Clone, Debug)]
pub struct Built {
    pub receipt: Receipt,
    pub zones: usize,
    pub squads: usize,
    pub sectors: usize,
    pub links: usize,
    pub costs: Costs,
    pub bases: Bases,
    pub problems: Vec<GroundProblem>,
}

struct Made {
    built: Built,
    pathfinding: LevelPathfinding,
}

/// The ground build for one open map.
#[derive(Default)]
pub struct GroundBuild {
    /// The session world (`Session::world_rev`) this state is for.
    world: Option<u64>,
    rx: Option<mpsc::Receiver<Result<Made, String>>>,
    /// The inputs of the build running.
    running: Option<u64>,
    /// How many builds this map has started.
    jobs: u64,
    built: Option<Built>,
    /// Why the last build failed; cleared by the next one.
    error: Option<String>,
}

/// Where the ground stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundState {
    NoMap,
    /// Never built for this map.
    Never,
    Building,
    Current,
    Stale(Stale),
    /// The last build failed and nothing current is built.
    Failed,
}

impl GroundState {
    pub const fn name(self) -> &'static str {
        match self {
            Self::NoMap => "no_map",
            Self::Never => "never",
            Self::Building => "building",
            Self::Current => "current",
            Self::Stale(_) => "stale",
            Self::Failed => "failed",
        }
    }
}

/// The stale reason as `ground.status` names it.
pub fn stale_name(s: Stale) -> &'static str {
    match s {
        Stale::KitVersion => "kit_version",
        Stale::Collision => "collision",
        Stale::Recipe => "recipe",
        Stale::Labels => "labels",
    }
}

/// The base the receipt covers: the map and the collision under the ground.
fn base_of(name: &str, scenario: &Scenario) -> Sha256 {
    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(scenario.collision_stamp().0);
    Sha256::of(&bytes)
}

fn recipe() -> Recipe {
    Recipe::new(Mode::Kit)
}

/// The labels placed on `scenario`, in map units: each labelled flag's
/// own position, facing and team.
pub fn labels_of(scenario: &Scenario) -> Result<Labels, String> {
    let flags = BlockKind::NetgameFlags;
    let mut out = Labels::default();
    for (index, label) in scenario.flag_labels() {
        let read = |field: &str| {
            scenario
                .get(flags, index, field)
                .map_err(|e| format!("netgame flag {index}: {e}"))
        };
        let at = read("position")?
            .as_vec3()
            .ok_or_else(|| format!("netgame flag {index} has no position"))?;
        let facing = read("facing")?.as_f32().unwrap_or(0.0);
        let team = match read("team")?.as_int() {
            Some(0) | None => Team::Red,
            Some(1) => Team::Blue,
            Some(other) => {
                return Err(format!(
                    "{} on netgame flag {index}: team {other} is not red (0) or blue (1)",
                    label.kind.text()
                ));
            }
        };
        match label.kind {
            LabelKind::Drop => out.drops.push(Marker {
                position: at,
                facing,
            }),
            LabelKind::Cover => out.covers.push(Marker {
                position: at,
                facing,
            }),
            LabelKind::Base => out.bases.push(BaseMarker { position: at, team }),
            LabelKind::Site => out.sites.push(at),
            LabelKind::Goal => out.goals.push(at),
            LabelKind::Line => out.lines.push(LineMarker {
                position: at,
                phase: label.phase,
                team,
            }),
        }
    }
    Ok(out)
}

/// The team and phase the next label of `kind` takes in the window: a
/// base or a siege line takes the team the placed ones leave short (red,
/// then blue, as the build wants one base for each team), and a siege line
/// the phase its pair of sides is in. The same rule Forge's marker
/// placing has.
pub fn next_team_phase(scenario: &Scenario, kind: LabelKind) -> (u8, u16) {
    let placed = scenario
        .flag_labels()
        .iter()
        .filter(|(_, l)| l.kind == kind)
        .count();
    let team = if kind.has_team() {
        (placed % 2) as u8
    } else {
        0
    };
    let phase = if kind.has_phase() {
        (placed / 2) as u16
    } else {
        0
    };
    (team, phase)
}

impl App {
    /// [`Action::PlaceLabelAt`].
    pub(super) fn place_label_at(
        &mut self,
        kind: LabelKind,
        at: [f32; 3],
        facing: f32,
        team: u8,
        phase: u16,
    ) {
        let r = self.session.add_label_at(kind, at, facing, team, phase);
        if let Some(n) = self.ok_or_log(r) {
            self.info(format!(
                "placed {} on netgame flag {n} at {:.2}, {:.2}, {:.2}",
                kind.text(),
                at[0],
                at[1],
                at[2]
            ));
        }
    }

    /// Forget a build that was for another map.
    fn sync_ground(&mut self) {
        let world = self.session.world_rev();
        if self.ground.world != Some(world) {
            self.ground = GroundBuild {
                world: Some(world),
                ..GroundBuild::default()
            };
        }
    }

    /// Where the ground stands for the open map.
    pub fn ground_state(&mut self) -> GroundState {
        self.sync_ground();
        let Some(scenario) = self.session.scenario() else {
            return GroundState::NoMap;
        };
        if self.ground.rx.is_some() {
            return GroundState::Building;
        }
        let Some(built) = &self.ground.built else {
            return if self.ground.error.is_some() {
                GroundState::Failed
            } else {
                GroundState::Never
            };
        };
        let name = self.session.map_name().unwrap_or_default();
        let labels = labels_of(scenario).unwrap_or_default();
        match built
            .receipt
            .stale(base_of(name, scenario), &labels, &recipe())
        {
            Some(why) => GroundState::Stale(why),
            None if self.ground.error.is_some() => GroundState::Failed,
            None => GroundState::Current,
        }
    }

    /// The last finished build for the open map.
    pub fn ground_built(&mut self) -> Option<&Built> {
        self.sync_ground();
        self.ground.built.as_ref()
    }

    /// Why the last build failed.
    pub fn ground_error(&self) -> Option<&str> {
        self.ground.error.as_deref()
    }

    /// The build that is running, by its number for this map.
    pub fn ground_job(&self) -> Option<u64> {
        self.ground.running
    }

    /// Start building the ground over the open map as edited, with the
    /// labels placed: [`Action::BuildGround`] and `ground.build`. Answers
    /// the build's number; a build already running is refused.
    pub fn start_ground(&mut self) -> Result<u64, String> {
        self.sync_ground();
        if self.ground.rx.is_some() {
            return Err("the AI ground is already building".into());
        }
        let scenario = self.session.scenario().ok_or("no map is open")?;
        let source = self.session.source().ok_or("no map is open")?;
        let name = self.session.map_name().ok_or("no map is open")?.to_owned();
        let labels = labels_of(scenario)?;
        labels.check().map_err(|e| format!("{e:#}"))?;
        let recipe = recipe();
        let receipt = Receipt::of(base_of(&name, scenario), &labels, &recipe);
        let bytes = scenario_edit::package::edited_bytes(source, scenario)
            .map_err(|e| format!("the map does not save, so no ground is built over it: {e}"))?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                make(&bytes, &name, &labels, &recipe, receipt)
            }))
            .unwrap_or_else(|_| Err("the ground builder panicked".into()));
            let _ = tx.send(r);
        });
        self.ground.jobs += 1;
        self.ground.running = Some(self.ground.jobs);
        self.ground.rx = Some(rx);
        self.ground.error = None;
        self.info("building the AI ground...");
        Ok(self.ground.jobs)
    }

    /// [`Action::BuildGround`].
    pub(super) fn build_ground(&mut self) {
        if let Err(e) = self.start_ground() {
            self.error(e);
        }
    }

    /// Each frame: pick up a finished build.
    pub fn poll_ground(&mut self) {
        self.sync_ground();
        let Some(rx) = self.ground.rx.as_ref() else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the ground builder stopped".into()),
        };
        self.finish_ground(result);
    }

    /// Wait for the build in progress (`ground.build` with `wait`, tests).
    pub fn wait_ground(&mut self) {
        self.sync_ground();
        if let Some(rx) = self.ground.rx.as_ref() {
            let result = rx
                .recv()
                .unwrap_or_else(|_| Err("the ground builder stopped".into()));
            self.finish_ground(result);
        }
    }

    fn finish_ground(&mut self, result: Result<Made, String>) {
        self.ground.rx = None;
        self.ground.running = None;
        match result {
            Ok(made) => {
                let b = &made.built;
                self.info(format!(
                    "AI ground built: {} zones, {} squad slots, {} sectors, {} problems",
                    b.zones,
                    b.squads,
                    b.sectors,
                    b.problems.len()
                ));
                self.session.set_ground_pathfinding(Some(made.pathfinding));
                self.ground.built = Some(made.built);
            }
            Err(e) => {
                self.error(format!("AI ground: {e}"));
                self.ground.error = Some(e);
            }
        }
        self.refresh_problems(Some("ground"));
    }

    /// The Problems rows of the ground: what the last build could not
    /// honour, or why it failed.
    pub fn ground_problem_rows(&self) -> Vec<Problem> {
        let row = |code: &str, severity, message: String, target: String| Problem {
            source: "ground",
            area: "ground".to_owned(),
            code: code.to_owned(),
            severity,
            message,
            target,
            path: None,
            asset: None,
            tool: Some(super::ToolKind::Scenario),
            field: None,
            at: None,
        };
        let mut rows = Vec::new();
        if let Some(why) = &self.ground.error {
            rows.push(row(
                "ground.build",
                Severity::Error,
                why.clone(),
                String::new(),
            ));
        }
        if let Some(b) = &self.ground.built {
            rows.extend(b.problems.iter().map(|p| {
                row(
                    "ground.label",
                    Severity::Warning,
                    format!(
                        "{} at {:.2}, {:.2}, {:.2}: {}",
                        p.label, p.position[0], p.position[1], p.position[2], p.reason
                    ),
                    format!("{}[{}]", p.label, p.index),
                )
            }));
        }
        rows
    }

    /// `ground.status`: where the ground stands, what is placed and what
    /// the last build made.
    pub fn ground_status(&mut self) -> Value {
        let state = self.ground_state();
        let placed = self
            .session
            .scenario()
            .map(|s| {
                let labels = s.flag_labels();
                let mut counts = serde_json::Map::new();
                for kind in LabelKind::ALL {
                    let n = labels.iter().filter(|(_, l)| l.kind == kind).count();
                    counts.insert(kind.text().to_owned(), json!(n));
                }
                counts.insert("total".to_owned(), json!(labels.len()));
                Value::Object(counts)
            })
            .unwrap_or(Value::Null);
        let built = self.ground.built.as_ref().map(|b| {
            json!({
                "zones": b.zones,
                "squads": b.squads,
                "sectors": b.sectors,
                "links": b.links,
                "problems": b.problems,
                "costs": b.costs,
                "headroom": {
                    "tag_growth": b.costs.tag_growth,
                    "available": b.costs.available,
                    "fits": b.costs.tag_growth <= b.costs.available,
                },
                "bases": b.bases,
            })
        });
        json!({
            "state": state.name(),
            "stale": match state {
                GroundState::Stale(why) => Some(stale_name(why)),
                _ => None,
            },
            "job": self.ground.running,
            "error": self.ground.error,
            "labels": placed,
            "built": built,
            "shown": self.session.shows_ground_pathfinding(),
        })
    }
}

/// The build itself: the kit's, then the built map's pathfinding for the
/// Navigation layers.
fn make(
    bytes: &[u8],
    name: &str,
    labels: &Labels,
    recipe: &Recipe,
    receipt: Receipt,
) -> Result<Made, String> {
    let ground = ground::build(bytes, name, labels, recipe).map_err(|e| format!("{e:#}"))?;
    let mut map = map::Map::from_reader(Cursor::new(&ground.bytes[..]))
        .map_err(|e| format!("the built map does not read: {e}"))?;
    let pathfinding = LevelPathfinding::read(&mut map)
        .map_err(|e| format!("the built map's pathfinding does not read: {e}"))?;
    let built = Built {
        // The receipt of the inputs as Studio keys them: the base is the
        // map's collision stamp, not the whole edited file.
        receipt,
        zones: ground.zones,
        squads: ground.squads,
        sectors: pathfinding.sectors.len(),
        links: pathfinding.links.len(),
        costs: ground.costs,
        bases: ground.bases,
        problems: ground.problems,
    };
    Ok(Made { built, pathfinding })
}

/// [`make`] for tests: the build over `bytes` and what it made.
#[cfg(test)]
pub(super) fn make_for_tests(
    bytes: &[u8],
    name: &str,
    labels: &Labels,
) -> Result<(Built, LevelPathfinding), String> {
    let receipt = Receipt::of(Sha256::of(bytes), labels, &recipe());
    make(bytes, name, labels, &recipe(), receipt).map(|m| (m.built, m.pathfinding))
}

#[cfg(test)]
impl App {
    /// Record a build of the ground as it stands now, with nothing made.
    pub(super) fn ground_pretend_built(&mut self) {
        self.sync_ground();
        let scenario = self.session.scenario().expect("a map is open");
        let name = self.session.map_name().unwrap_or_default();
        let labels = labels_of(scenario).unwrap();
        let receipt = Receipt::of(base_of(name, scenario), &labels, &recipe());
        self.ground.built = Some(Built {
            receipt,
            zones: 0,
            squads: 0,
            sectors: 0,
            links: 0,
            costs: Costs::default(),
            bases: Bases {
                points: [[0.0; 3]; 2],
                source: firefight_kit::Source::Labels,
                home: Team::Red,
            },
            problems: Vec::new(),
        });
    }
}
