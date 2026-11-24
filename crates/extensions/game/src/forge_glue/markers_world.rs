//! Forge's game type markers, drawn as a world-line source
//! (docs/game/gametype-markers.md, "The game"). The monitor shows a marker
//! for each spawn, flag, hill, teleporter, item spawner and ground label of
//! the session: the shape [`game_markers`] makes for it, in the team's
//! colour, with the game type's other things faded, a distance fade, a
//! budget of [`BUDGET`] markers a view and a label over the near ones.
//!
//! The driver turns the session's things into [`Entry`]s ([`entries`]) every
//! few ticks and [`publish`]es them. The render owner's capture ([`Source`])
//! orders them by the eye's distance, builds the lines of the nearest
//! ([`draw`]) and keeps what it drew for `render.markers`.
//!
//! A running game's rules script shows the walls of the areas it names
//! (`show_area`, docs/game/gametype-markers.md, "The boundary in play"):
//! the rules runtime publishes them ([`publish_walls`]) and the same
//! source draws each alone, with no pip and no label ([`draw_walls`]),
//! whether or not a Forge session has markers.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent::Access;
use agent_proto::{MarkersParams, RpcError, code, method};
use forge::tool::{Show, in_game};
use forge::{Endpoint, GroundProblem, PlacedMarker, Thing, ThingKind, Value};
use game_markers::{Area, Item, Kind, Marker, Palette, Place, Tint};
use serde_json::{Value as Json, json};

/// The most markers a view draws; the nearest are kept.
pub const BUDGET: usize = 256;
/// A marker is at full strength to here, in world units, and gone at the
/// second.
pub const FADE: (f32, f32) = (40.0, 60.0);
/// An area's wall fades over this range.
pub const AREA_FADE: (f32, f32) = (80.0, 120.0);
/// A marker's label shows within this distance.
pub const LABEL_RANGE: f32 = 20.0;
/// The alpha factor of a marker of another game type.
pub const OTHER_GAME: f32 = 0.25;
/// A label the last ground build could not honour.
pub const PROBLEM: [f32; 3] = [1.0, 90.0 / 255.0, 0.0];
/// A marker aimed at or held.
pub const SELECTED: [f32; 3] = [1.0, 1.0, 1.0];
/// How far over a marker's top its label sits, in world units.
const LABEL_LIFT: f32 = 0.15;
const TEAM_NAMES: [&str; 8] = [
    "Red", "Blue", "Yellow", "Green", "Purple", "Orange", "Brown", "Pink",
];

/// What colours a marker besides its kind and team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// A ground label the last build could not honour.
    Problem,
}

/// One marker the session has, where it stands and what it is called.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The thing it stands for; a problem has none. The aimed and held
    /// things are drawn selected.
    pub id: Option<Endpoint>,
    /// `render.markers`' `thing`.
    pub name: String,
    pub marker: Marker,
    pub place: Place,
    pub tone: Tone,
    /// A thing of another game type, shown while the tool shows all.
    pub other_game: bool,
    /// The text over it: "Red spawn", "Hill 2".
    pub label: String,
    /// The path is still in hand and has a dashed closing edge.
    pub draft_path: bool,
}

/// What the markers are made from.
pub struct Facts<'a> {
    /// The monitor is on; with it off there are no markers.
    pub monitor: bool,
    pub things: &'a [Thing],
    pub ground: &'a [PlacedMarker],
    pub labels: &'a BTreeMap<Endpoint, forge_doc::Label>,
    pub areas: &'a BTreeMap<Endpoint, forge_doc::Area>,
    pub problems: &'a [GroundProblem],
    /// The session's game type, as `Tool::game`.
    pub game: Option<i32>,
    pub show: Show,
}

fn field<'a>(thing: &'a Thing, name: &str) -> Option<&'a Value> {
    thing
        .fields
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
}

fn int(thing: &Thing, name: &str) -> Option<i64> {
    match field(thing, name)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

/// A team field as a team: 0 to 7; neutral, none and the rest are none.
fn team_of(thing: &Thing) -> Option<u8> {
    int(thing, "team")
        .and_then(|t| u8::try_from(t).ok())
        .filter(|t| *t < 8)
}

/// What an item spawner makes, from its item's tag group.
fn item_class(thing: &Thing) -> Item {
    match field(thing, "item") {
        Some(Value::Tag(Some(key))) => match key.group.as_str() {
            "vehc" | "vehi" => Item::Vehicle,
            "weap" => Item::Weapon,
            _ if key.path.to_ascii_lowercase().contains("weapon") => Item::Weapon,
            _ => Item::Equipment,
        },
        _ => Item::Equipment,
    }
}

/// `render.markers`' name of a thing: its block and index, or the add op.
pub fn thing_name(id: Endpoint) -> String {
    match id.identity() {
        Endpoint::Base { block, index, .. } => format!("{}:{index}", block.name()),
        Endpoint::Added(op) => format!("added:{op}"),
    }
}

/// The text over a marker.
fn label_of(kind: &Kind, team: Option<u8>, number: Option<i32>) -> String {
    if matches!(kind, Kind::Hill(_)) {
        return format!("Hill {}", number.map_or(1, |n| n + 1));
    }
    let what = kind.name().replace('_', " ");
    match team.and_then(|t| TEAM_NAMES.get(usize::from(t))) {
        Some(team) => format!("{team} {what}"),
        None => {
            let mut chars = what.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn entry(
    id: Option<Endpoint>,
    name: String,
    kind: Kind,
    team: Option<u8>,
    number: Option<i32>,
    at: [f32; 3],
    yaw: f32,
    tone: Tone,
    other_game: bool,
) -> Entry {
    Entry {
        id,
        name,
        label: label_of(&kind, team, number),
        place: Place::facing(at, if kind.area().is_some() { 0.0 } else { yaw }),
        marker: Marker::new(kind, team),
        tone,
        other_game,
        draft_path: false,
    }
}

pub fn with_area(kind: Kind, area: Area) -> Kind {
    match kind {
        Kind::Hill(_) => Kind::Hill(area),
        Kind::Territory(_) => Kind::Territory(area),
        Kind::Site(_) => Kind::Site(area),
        _ => kind,
    }
}

/// Every marker of the session: one for each thing a marker stands for,
/// one hill for a hill's corners, one for each ground label placed and
/// each problem. A thing of another game type is left out while the tool
/// shows this game type's, and marked while it shows all. None with the
/// monitor off.
pub fn entries(f: &Facts<'_>) -> Vec<Entry> {
    if !f.monitor {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut hills: BTreeMap<i32, Vec<&Thing>> = BTreeMap::new();
    for t in f.things {
        let ground_label = t.block == forge::BlockKind::NetgameFlags
            && f.labels.get(&t.id.identity()).is_some_and(|label| {
                forge::GroundLabel::of(&label.text).is_some()
            });
        let other_game = !ground_label && !in_game(t, f.game);
        if other_game && f.show == Show::ThisGameType {
            continue;
        }
        // A ground label's flag is drawn as its label, below.
        if let Endpoint::Added(op) = t.id.identity()
            && f.ground.iter().any(|m| m.id == op)
        {
            continue;
        }
        let mut area = f.areas.get(&t.id.identity()).map(forge_doc::Area::marker_area);
        let mut number = None;
        let kind = match t.kind {
            ThingKind::Object => continue,
            ThingKind::StartingPoint => Kind::Spawn,
            ThingKind::ItemSpawner => Kind::Item(item_class(t)),
            ThingKind::Goal | ThingKind::Teleporter => {
                let Some(label) = forge::flag_label(t, f.labels) else {
                    continue;
                };
                number = label.number;
                if label.text == "hill_corner" {
                    if let Some(area) = area.take() {
                        Kind::Hill(area)
                    } else {
                        hills.entry(number.unwrap_or(0)).or_default().push(t);
                        continue;
                    }
                } else {
                    match Kind::of_label(&label.text) {
                        Some(kind) => kind,
                        None => continue,
                    }
                }
            }
        };
        let kind = match area {
            Some(area) => with_area(kind, area),
            None => kind,
        };
        out.push(entry(
            Some(t.id),
            thing_name(t.id),
            kind,
            team_of(t),
            number,
            t.frame.position,
            t.frame.rotation[0],
            Tone::Normal,
            other_game,
        ));
    }
    for (number, corners) in hills {
        let points: Vec<[f32; 3]> = corners.iter().map(|t| t.frame.position).collect();
        let whole = (corners.len() >= 3)
            .then(|| Area::of_corners(&points))
            .flatten();
        let other_game = corners.iter().all(|t| !in_game(t, f.game));
        let hill = |t: &Thing, at: [f32; 3], area: Area| {
            entry(
                Some(t.id),
                thing_name(t.id),
                Kind::Hill(area),
                team_of(t),
                Some(number),
                at,
                0.0,
                Tone::Normal,
                other_game,
            )
        };
        match whole {
            Some((centre, area)) => out.push(hill(corners[0], centre, area)),
            None => {
                for t in &corners {
                    out.push(hill(t, t.frame.position, Area::default()));
                }
            }
        }
    }
    let problem_at = |p: [f32; 3]| {
        f.problems
            .iter()
            .any(|q| q.position.map(f32::to_bits) == p.map(f32::to_bits))
    };
    for m in f.ground {
        if problem_at(m.frame.position) {
            continue;
        }
        let Some(kind) = Kind::of_label(m.label.text()) else {
            continue;
        };
        let kind = match f.areas.get(&Endpoint::Added(m.id)) {
            Some(area) => with_area(kind, area.marker_area()),
            None => kind,
        };
        let team = m.label.has_team().then_some(m.team);
        out.push(entry(
            Some(Endpoint::Added(m.id)),
            thing_name(Endpoint::Added(m.id)),
            kind,
            team,
            m.number,
            m.frame.position,
            m.frame.rotation[0],
            Tone::Normal,
            false,
        ));
    }
    for p in f.problems {
        let Some(kind) = Kind::of_label(&p.label) else {
            continue;
        };
        out.push(entry(
            None,
            format!("problem:{}", p.label),
            kind,
            None,
            None,
            p.position,
            0.0,
            Tone::Problem,
            false,
        ));
    }
    out
}

// ---- what the session publishes ----

#[derive(Clone, Default)]
struct Published {
    entries: Arc<Vec<Entry>>,
    aimed: Option<Endpoint>,
    held: Option<Endpoint>,
}

static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static WALLS: Mutex<Vec<Wall>> = Mutex::new(Vec::new());
static WALLS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// An area's wall a running script shows in play
/// (docs/game/gametype-markers.md, "The boundary in play"): the area round
/// its centre marker's place, in the colour of a team, or neutral.
#[derive(Clone, Debug, PartialEq)]
pub struct Wall {
    /// `render.markers`' `thing`: the zone object, `zone:<hex>`.
    pub name: String,
    pub team: Option<u8>,
    pub at: [f32; 3],
    pub area: Area,
}

/// The walls the window draws from now on, in place of the last ones; none
/// when no script shows any.
pub fn publish_walls(walls: Vec<Wall>) {
    WALLS_ACTIVE.store(!walls.is_empty(), Ordering::Release);
    *WALLS.lock().unwrap_or_else(|e| e.into_inner()) = walls;
}

#[cfg(any(feature = "gpu", test))]
fn walls() -> Vec<Wall> {
    WALLS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The markers the window draws from now on, and the things aimed at and
/// held, which are drawn selected and never fade.
pub fn publish(entries: &Arc<Vec<Entry>>, aimed: Option<Endpoint>, held: Option<Endpoint>) {
    ACTIVE.store(!entries.is_empty(), Ordering::Release);
    *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Published {
        entries: entries.clone(),
        aimed,
        held,
    });
}

/// No session: nothing is drawn and nothing was drawn.
pub fn clear() {
    ACTIVE.store(false, Ordering::Release);
    *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Report::default();
}

#[cfg(any(feature = "gpu", test))]
fn published() -> Option<Published> {
    PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// ---- one view ----

/// One marker's lines for the window: one group, one colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub edges: Vec<[[f32; 3]; 2]>,
    pub anchor: [f32; 3],
    pub colour: [f32; 3],
    pub alpha: f32,
    /// The distance fade, or none for a selected marker.
    pub fade: Option<(f32, f32)>,
    pub label: Option<String>,
}

/// What a view drew, for `render.markers`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub shown: usize,
    pub hidden_by_budget: usize,
    pub markers: Vec<Shown>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub thing: String,
    pub kind: &'static str,
    pub team: Option<u8>,
    pub anchor: [f32; 3],
    pub lines: usize,
    pub faded: bool,
}

static LAST: Mutex<Report> = Mutex::new(Report {
    shown: 0,
    hidden_by_budget: 0,
    markers: Vec::new(),
});

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f32>()
        .sqrt()
}

/// How far an area's wall reaches from its centre marker.
fn reach(kind: &Kind) -> f32 {
    kind.area().map_or(0.0, reach_of)
}

fn reach_of(area: &Area) -> f32 {
    area.outline()
        .iter()
        .map(|p| p[0].hypot(p[1]))
        .fold(0.0, f32::max)
}

/// The groups of `walls` as seen from `eye`, and what they show for
/// `render.markers` (kind `wall`): each wall alone in its team's colour
/// (the neutral colour for none), fading over [`AREA_FADE`], with no label
/// and no pip, and not counted against [`BUDGET`] (a script shows at most
/// eight). A wall past its fade is not drawn. With no eye every wall draws.
pub fn draw_walls(walls: &[Wall], eye: Option<[f32; 3]>, palette: &Palette) -> (Vec<Group>, Vec<Shown>) {
    let mut groups = Vec::with_capacity(walls.len());
    let mut shown = Vec::with_capacity(walls.len());
    for w in walls {
        if eye.is_some_and(|eye| distance(eye, w.at) > AREA_FADE.1 + reach_of(&w.area)) {
            continue;
        }
        let colour = palette.colour(Tint::Team, w.team);
        let mesh = Kind::wall(&w.area, colour).placed(&Place::facing(w.at, 0.0));
        let top = mesh.bounds().map_or(w.at[2], |(_, max)| max[2]);
        let anchor = [w.at[0], w.at[1], top];
        groups.push(Group {
            edges: mesh.lines.iter().map(|l| [l.a, l.b]).collect(),
            anchor,
            colour,
            alpha: mesh.lines.first().map_or(1.0, |l| l.colour[3]),
            fade: Some(AREA_FADE),
            label: None,
        });
        shown.push(Shown {
            thing: w.name.clone(),
            kind: WALL,
            team: w.team,
            anchor,
            lines: mesh.lines.len(),
            faded: false,
        });
    }
    (groups, shown)
}

/// `render.markers`' kind of a wall a script shows.
pub const WALL: &str = "wall";

/// The groups of `entries` as seen from `eye`, and the report of them.
/// The aimed and held things come first, drawn white and never faded; the
/// rest are the ones within their fade range, the nearest `budget` of
/// them; the ones the budget leaves out are counted. With no eye the
/// entries keep their order and nothing is out of range.
pub fn draw(
    entries: &[Entry],
    pins: [&Option<Endpoint>; 2],
    eye: Option<[f32; 3]>,
    palette: &Palette,
    budget: usize,
) -> (Vec<Group>, Report) {
    let pinned = |e: &Entry| {
        e.id.is_some_and(|id| {
            pins.iter()
                .any(|p| p.is_some_and(|p| p.identity() == id.identity()))
        })
    };
    let mut seen: Vec<(bool, f32, &Entry)> = entries
        .iter()
        .filter_map(|e| {
            let d = eye.map_or(0.0, |eye| distance(eye, e.place.at));
            let pin = pinned(e);
            let end = if e.marker.kind.area().is_some() {
                AREA_FADE.1
            } else {
                FADE.1
            };
            (pin || d <= end + reach(&e.marker.kind)).then_some((pin, d, e))
        })
        .collect();
    seen.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)));
    let hidden = seen.len().saturating_sub(budget);
    seen.truncate(budget);
    let mut groups = Vec::with_capacity(seen.len());
    let mut report = Report {
        shown: 0,
        hidden_by_budget: hidden,
        markers: Vec::with_capacity(seen.len()),
    };
    for (pin, d, e) in seen {
        let colour = if pin {
            SELECTED
        } else if e.tone == Tone::Problem {
            PROBLEM
        } else {
            e.marker.colour(palette)
        };
        let alpha = if e.other_game && !pin {
            OTHER_GAME
        } else {
            1.0
        };
        let mesh = match e.marker.kind.area().filter(|_| e.draft_path) {
            Some(area) => area.path_preview(colour),
            None => e.marker.kind.mesh(colour),
        }
        .placed(&e.place)
        .faded(alpha);
        let top = mesh.bounds().map_or(e.place.at[2], |(_, max)| max[2]);
        let anchor = [e.place.at[0], e.place.at[1], top + LABEL_LIFT];
        let area = e.marker.kind.area().is_some();
        let near = eye.is_none() || d <= LABEL_RANGE;
        let lines = mesh.lines.len();
        groups.push(Group {
            edges: mesh.lines.iter().map(|l| [l.a, l.b]).collect(),
            anchor,
            colour,
            alpha: mesh.lines.first().map_or(alpha, |l| l.colour[3]),
            fade: (!pin).then_some(if area { AREA_FADE } else { FADE }),
            label: (pin || near).then(|| e.label.clone()),
        });
        report.markers.push(Shown {
            thing: e.name.clone(),
            kind: e.marker.kind.name(),
            team: e.marker.team,
            anchor,
            lines,
            faded: e.other_game,
        });
    }
    report.shown = groups.len();
    (groups, report)
}

// ---- render.markers ----

/// `render.markers`' answer from `report`, the markers of `kind` only when
/// one is asked for (the counts cover every marker).
pub fn report_json(report: &Report, kind: Option<&str>) -> Json {
    let markers: Vec<Json> = report
        .markers
        .iter()
        .filter(|m| kind.is_none_or(|k| m.kind == k))
        .map(|m| {
            json!({
                "thing": m.thing, "kind": m.kind, "team": m.team,
                "anchor": m.anchor, "lines": m.lines, "faded": m.faded,
            })
        })
        .collect();
    json!({
        "shown": report.shown,
        "hidden_by_budget": report.hidden_by_budget,
        "markers": markers,
    })
}

fn answer(params: Json, report: &Report) -> Result<Json, RpcError> {
    let params = if params.is_null() { json!({}) } else { params };
    let p: MarkersParams = serde_json::from_value(params)
        .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
    if let Some(kind) = &p.kind {
        let mut names: Vec<&str> = Kind::samples().iter().map(Kind::name).collect();
        names.push(WALL);
        if !names.contains(&kind.as_str()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("unknown marker kind `{kind}`; one of: {}", names.join(", ")),
            ));
        }
    }
    Ok(report_json(report, p.kind.as_deref()))
}

/// `render.markers` (docs/runtime/agent-api.md, "render.markers"): what the
/// last present drew of the game type markers.
pub fn add_method(agent: &agent::Agent) {
    agent.add_method(method::RENDER_MARKERS, Access::Read, |params| {
        answer(params, &LAST.lock().unwrap_or_else(|e| e.into_inner()))
    });
}

#[cfg(feature = "gpu")]
pub use gpu::register;

#[cfg(feature = "gpu")]
mod gpu {
    use engine::GuestMemory;

    use super::*;
    use crate::world_lines::{ImageMarks, LineStyle, WorldLine, WorldLineSource};
    use gfx::world_lines::{Fade, LabelFace};

    /// Draws the markers while a Forge session has any.
    pub struct Source;

    pub fn register(host: &mut crate::Host) {
        host.add_world_line_source(Arc::new(Source));
    }

    /// The first world view's eye in the last frame's scene.
    fn eye() -> Option<[f32; 3]> {
        let scene = render_bridge::scene_step::last()?;
        scene
            .views
            .iter()
            .find(|v| v.key.kind == frame_scene::ViewKind::Game && v.key.view == 0)
            .map(|v| v.camera.position)
    }

    /// The map's team colours over the stand-ins.
    fn palette(guest: &dyn GuestMemory) -> Palette {
        let mut palette = Palette::default();
        for (n, slot) in palette.teams.iter_mut().enumerate() {
            if let Some(c) = crate::pad_lightbar::team_rgb(guest, n as u8) {
                *slot = c.map(|v| f32::from(v) / 255.0);
            }
        }
        palette
    }

    impl WorldLineSource for Source {
        fn active(&self) -> bool {
            ACTIVE.load(Ordering::Acquire) || WALLS_ACTIVE.load(Ordering::Acquire)
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let p = published().unwrap_or_default();
            let (eye, palette) = (eye(), palette(guest));
            let (mut groups, mut report) = draw(&p.entries, [&p.aimed, &p.held], eye, &palette, BUDGET);
            let (walls, shown) = draw_walls(&walls(), eye, &palette);
            report.shown += walls.len();
            report.markers.extend(shown);
            groups.extend(walls);
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = report;
            if groups.is_empty() {
                return None;
            }
            let face = crate::superbounce_font::look(guest).face;
            Some(Arc::new(Sample { groups, face }))
        }
    }

    #[derive(Debug)]
    struct Sample {
        groups: Vec<Group>,
        face: LabelFace,
    }

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            for g in &self.groups {
                visitor(WorldLine {
                    edges: &g.edges,
                    anchor: g.anchor,
                    color: g.colour,
                    dashed: false,
                    label: g.label.as_deref(),
                    style: LineStyle {
                        width: 2.0,
                        alpha: g.alpha,
                        fade: g.fade.map(|(start, end)| Fade { start, end }),
                        label_scale: 2.0,
                        label_centered: true,
                        label_face: self.face,
                        ..LineStyle::default()
                    },
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
