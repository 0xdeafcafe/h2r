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
//! Each entry draws one of its marker's two forms
//! (docs/game/gametype-marker-forms.md, "When a form shows"): with the
//! monitor on, the edit form, its play form's parts at full strength and
//! the edit-only parts at [`EDIT_ALPHA`]; its plate stands on its post
//! turned about the upright to face the eye and scaled with the distance
//! ([`plate`]).
//!
//! A running game's rules script shows the walls of the areas it names
//! (`show_area`, docs/game/gametype-markers.md, "In a script"):
//! the rules runtime publishes them ([`publish_walls`]) and the same
//! source draws each alone, with no pip and no label ([`draw_walls`]),
//! whether or not a Forge session has markers.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent::Access;
use agent_proto::{MarkerRow, MarkersParams, RpcError, code, method};
use forge::tool::{Show, in_game};
use forge::{Endpoint, GroundProblem, PlacedMarker, Thing, ThingKind, Value};
use game_markers::form::{EDIT_ALPHA, PLATE, icon_scale};
use game_markers::{
    Area, Form, Item, Kind, LINE_ALPHA, Marker, Mesh, Palette, Part, Place, Shape, SpawnUse, Tint,
    Tri,
};
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
    /// Which of the marker's forms draws: the edit form in Forge.
    pub form: Form,
    /// The number on its plate: a hill's, a territory's or a site's,
    /// counted from 1.
    pub number: Option<u32>,
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

/// Where a marker of `kind` at `at` facing `yaw` stands: a box area turns
/// with its marker, as Studio draws it and `Area::contains_turned` scores
/// it; any other area (a circle, a sphere, a path, whose points are
/// offsets on the world's axes) lies on the world's axes; a kind with no
/// area faces `yaw`.
pub fn place_of(kind: &Kind, at: [f32; 3], yaw: f32) -> Place {
    match kind.area() {
        Some(area) => area_place(area, at, yaw),
        None => Place::facing(at, yaw),
    }
}

fn area_place(area: &Area, at: [f32; 3], yaw: f32) -> Place {
    match area.shape {
        Shape::Box { .. } => Place::facing(at, yaw),
        _ => Place::facing(at, 0.0),
    }
}

/// The number a kind's plate shows of its flag's `number` (from 0): one
/// more, and a hill with none is Hill 1, as its label says.
fn plate_number(kind: &Kind, number: Option<i32>) -> Option<u32> {
    if !kind.numbered() {
        return None;
    }
    match (kind, number) {
        (_, Some(n)) => u32::try_from(n).ok().map(|n| n + 1),
        (Kind::Hill(_), None) => Some(1),
        _ => None,
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
        place: place_of(&kind, at, yaw),
        number: plate_number(&kind, number),
        marker: Marker::new(kind, team),
        tone,
        other_game,
        draft_path: false,
        form: Form::Edit,
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
            ThingKind::Object | ThingKind::Piece => continue,
            ThingKind::StartingPoint => Kind::Spawn(SpawnUse::Respawn),
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
            Some(area) => kind.with_area(area),
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
            Some(area) => kind.with_area(area.marker_area()),
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

/// The markers of the last **Generate nav**'s problems
/// (docs/game/forge-nav.md, "The overlay"), in the problem tone: a start
/// off the mesh as a spawn, a teleporter end as that end, a solid left out
/// as a cover mark. Each is named `nav:` and its subject.
pub fn nav_problems(problems: &[forge::NavProblem]) -> Vec<Entry> {
    problems
        .iter()
        .map(|p| {
            let kind = match p.kind.as_str() {
                "start_off_mesh" => Kind::Spawn(SpawnUse::Respawn),
                "teleporter_off_mesh" if p.what.ends_with("receiver") => Kind::TeleporterReceiver,
                "teleporter_off_mesh" => Kind::TeleporterSender,
                _ => Kind::Cover,
            };
            entry(
                None,
                format!("nav:{}", p.what),
                kind,
                None,
                None,
                p.position,
                0.0,
                Tone::Problem,
                false,
            )
        })
        .collect()
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
/// (docs/game/gametype-markers.md, "In a script"): the area round
/// its centre marker's place, in the colour of a team, or neutral.
#[derive(Clone, Debug, PartialEq)]
pub struct Wall {
    /// `render.markers`' `thing`: the zone object, `zone:<hex>`.
    pub name: String,
    pub team: Option<u8>,
    pub at: [f32; 3],
    /// The centre marker's facing, which turns a box.
    pub yaw: f32,
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

/// The play forms a match draws (docs/game/gametype-marker-forms.md, "When
/// a form shows"): the flags the document shows, read from the loaded
/// map's census when the map was activated ([`refresh_play`]).
static PLAY: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static PLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
/// A map was activated and its play forms are not read yet.
static PLAY_STALE: AtomicBool = AtomicBool::new(false);

/// The play entry of each netgame flag of `things` that is shown in play
/// (`LabelledThing::show`): the flag's marker in its play form, at
/// `flags`' position and facing by flag index (`engine::objectives`). A
/// flag with no position or no marker (a label with no kind; a hill's
/// corner, which has no area of its own) draws nothing, and so does any
/// other thing.
pub fn play_entries(things: &[forge_doc::LabelledThing], flags: &[([f32; 3], f32)]) -> Vec<Entry> {
    let mut out = Vec::new();
    for t in things {
        if t.block != forge::BlockKind::NetgameFlags || !t.show {
            continue;
        }
        let Some(&(at, yaw)) = flags.get(t.index as usize) else {
            continue;
        };
        let area = t.area.clone();
        let kind = if t.label.text == "hill_corner" {
            match area {
                Some(area) => Kind::Hill(area),
                None => continue,
            }
        } else {
            let Some(kind) = Kind::of_label(&t.label.text) else {
                continue;
            };
            match area {
                Some(area) => kind.with_area(area),
                None => kind,
            }
        };
        let id = Endpoint::base(t.block, t.index);
        let mut e = entry(
            Some(id),
            thing_name(id),
            kind,
            t.team,
            t.label.number,
            at,
            yaw,
            Tone::Normal,
            false,
        );
        e.form = Form::Play;
        out.push(e);
    }
    out
}

/// A map was activated: its play forms are read again at the next frame
/// ([`refresh_play`]).
pub fn play_stale() {
    PLAY_STALE.store(true, Ordering::Release);
}

/// The play forms a match draws from now on, in place of the last ones.
pub fn publish_play(entries: Vec<Entry>) {
    PLAY_ACTIVE.store(!entries.is_empty(), Ordering::Release);
    *PLAY.lock().unwrap_or_else(|e| e.into_inner()) = entries;
    PLAY_STALE.store(false, Ordering::Release);
}

/// Reads the play forms of the loaded map when a map was activated since
/// the last read: its census (`rules::census::things`, with the areas
/// record the map carries) and its flags' places. A scenario that is not
/// loaded yet leaves them to the next frame. True when it read them.
#[cfg(feature = "rules")]
pub fn refresh_play(m: &dyn engine::GuestMemory) -> bool {
    if !PLAY_STALE.load(Ordering::Acquire) {
        return false;
    }
    if m.u32(engine::points::SCENARIO.base).unwrap_or(0) == 0 {
        return false;
    }
    let Ok(things) = crate::rules::census::things(m) else {
        return false;
    };
    let facings = engine::objectives::netgame_flag_facings(m).unwrap_or_default();
    let flags: Vec<([f32; 3], f32)> = engine::objectives::netgame_flags(m)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, (at, _, _))| (at, facings.get(i).copied().unwrap_or(0.0)))
        .collect();
    publish_play(play_entries(&things, &flags));
    true
}

/// What a frame draws: the session's markers, or with none (no session, or
/// the monitor off) the play forms of the map.
#[cfg(any(feature = "gpu", test))]
fn frame_entries(session: Arc<Vec<Entry>>) -> Arc<Vec<Entry>> {
    if !session.is_empty() {
        return session;
    }
    Arc::new(PLAY.lock().unwrap_or_else(|e| e.into_inner()).clone())
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

/// One marker's lines for the window: one group, one colour. Its faces
/// are the native renderer's skin ([`gpu::batch`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// The lines drawn at the group's alpha: the play form's.
    pub edges: Vec<[[f32; 3]; 2]>,
    /// The lines drawn at [`EDIT_ALPHA`] of it: the edit-only parts.
    pub dim: Vec<[[f32; 3]; 2]>,
    /// The placed mesh's faces, with their alpha, faded as the marker is.
    pub tris: Vec<Tri>,
    pub anchor: [f32; 3],
    pub colour: [f32; 3],
    pub alpha: f32,
    /// The distance fade, or none for a selected marker.
    pub fade: Option<(f32, f32)>,
    pub label: Option<String>,
}

impl Group {
    /// The alpha of the [`Group::dim`] lines.
    pub fn dim_alpha(&self) -> f32 {
        self.alpha * EDIT_ALPHA
    }
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
    pub form: Form,
    /// The icon on its plate; none for a wall and a volume.
    pub icon: Option<&'static str>,
    /// How much its plate is scaled with the eye's distance; none with no
    /// plate drawn.
    pub plate_scale: Option<f32>,
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
        let mesh = Kind::wall(&w.area, colour).placed(&area_place(&w.area, w.at, w.yaw));
        let top = mesh.bounds().map_or(w.at[2], |(_, max)| max[2]);
        let anchor = [w.at[0], w.at[1], top];
        groups.push(Group {
            edges: edges(&mesh),
            dim: Vec::new(),
            tris: mesh.tris.clone(),
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
            form: Form::Play,
            icon: None,
            plate_scale: None,
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
        let shape = drawn(e, colour, eye);
        let (full, dim) = (shape.full.faded(alpha), shape.dim.faded(alpha));
        let top = full
            .bounds()
            .into_iter()
            .chain(dim.bounds())
            .map(|(_, max)| max[2])
            .reduce(f32::max)
            .unwrap_or(e.place.at[2]);
        let anchor = [e.place.at[0], e.place.at[1], top + LABEL_LIFT];
        let area = e.marker.kind.area().is_some();
        let near = eye.is_none() || d <= LABEL_RANGE;
        let lines = full.lines.len() + dim.lines.len();
        let (bright, faint) = (edges(&full), edges(&dim));
        let mut tris = full.tris;
        tris.extend(dim.tris);
        groups.push(Group {
            edges: bright,
            dim: faint,
            tris,
            anchor,
            colour,
            alpha: alpha * LINE_ALPHA,
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
            form: e.form,
            icon: e.marker.kind.icon().map(|i| i.name()),
            plate_scale: shape.plate_scale,
        });
    }
    report.shown = groups.len();
    (groups, report)
}

fn edges(mesh: &Mesh) -> Vec<[[f32; 3]; 2]> {
    mesh.lines.iter().map(|l| [l.a, l.b]).collect()
}

/// One entry's lines and faces in the world, as seen from one eye.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {
    /// What draws at full strength: the play form's parts.
    pub full: Mesh,
    /// What draws at [`EDIT_ALPHA`]: the edit form's other parts, already
    /// faded so.
    pub dim: Mesh,
    /// The plate's scale ([`icon_scale`]), when a plate is drawn.
    pub plate_scale: Option<f32>,
}

/// What `e` draws in `colour` seen from `eye`
/// (docs/game/gametype-marker-forms.md, "When a form shows"). In the edit
/// form the play form's parts are [`Drawn::full`] and the rest dim; in
/// the play form only the play form draws. The plate ([`plate`]) goes
/// with the part it is in that kind's play form. A path still in hand
/// is its preview alone, with no plate.
pub fn drawn(e: &Entry, colour: [f32; 3], eye: Option<[f32; 3]>) -> Drawn {
    let kind = &e.marker.kind;
    if let Some(area) = kind.area().filter(|_| e.draft_path) {
        return Drawn {
            full: area.path_preview(colour).placed(&e.place),
            ..Drawn::default()
        };
    }
    let mut full = kind.mesh_in(Form::Play, colour).placed(&e.place);
    let mut dim = match e.form {
        Form::Edit => kind.edit_only(colour).placed(&e.place).faded(EDIT_ALPHA),
        Form::Play => Mesh::default(),
    };
    let plate_in_play = kind.parts(Form::Play).has(Part::Plate);
    let sign = (e.form == Form::Edit || plate_in_play)
        .then(|| plate(e, colour, eye))
        .flatten();
    let plate_scale = sign.as_ref().map(|(_, scale)| *scale);
    match sign {
        Some((mesh, _)) if plate_in_play => full.extend(mesh),
        Some((mesh, _)) => dim.extend(mesh.faded(EDIT_ALPHA)),
        None => {}
    }
    Drawn {
        full,
        dim,
        plate_scale,
    }
}

/// Where `e`'s plate stands seen from `eye`, and its scale: on the
/// marker's post ([`Kind::plate_at`]), turned about the upright so its
/// face (the plate's +x) points at the eye, and scaled by
/// [`icon_scale`] of the eye's distance with its foot kept on the post.
/// With no eye, or the eye straight over it, it faces the marker's
/// front. `None` for a kind with no plate.
pub fn plate_place(e: &Entry, eye: Option<[f32; 3]>) -> Option<(Place, f32)> {
    let centre = e.place.point(e.marker.kind.plate_at()?);
    let front = e.place.forward[1].atan2(e.place.forward[0]);
    let (yaw, scale) = match eye {
        Some(eye) => {
            let (dx, dy) = (eye[0] - centre[0], eye[1] - centre[1]);
            let yaw = if dx.hypot(dy) > 1e-4 { dy.atan2(dx) } else { front };
            (yaw, icon_scale(distance(eye, centre)))
        }
        None => (front, 1.0),
    };
    let at = [centre[0], centre[1], centre[2] + PLATE * (scale - 1.0)];
    Some((Place::facing(at, yaw), scale))
}

/// `e`'s plate and icon in the world in `colour`, at [`plate_place`], and
/// its scale.
pub fn plate(e: &Entry, colour: [f32; 3], eye: Option<[f32; 3]>) -> Option<(Mesh, f32)> {
    let (place, scale) = plate_place(e, eye)?;
    let mesh = e.marker.kind.plate(e.number, colour)?;
    Some((mesh.scaled(scale).placed(&place), scale))
}

// ---- render.markers ----

/// `render.markers`' row of one marker.
pub fn row(m: &Shown) -> MarkerRow {
    MarkerRow {
        thing: m.thing.clone(),
        kind: m.kind.to_owned(),
        team: m.team,
        anchor: m.anchor,
        lines: m.lines,
        faded: m.faded,
        form: m.form.name().to_owned(),
        icon: m.icon.map(str::to_owned),
        plate_scale: m.plate_scale,
    }
}

/// `render.markers`' answer from `report`, the markers of `kind` and of
/// `form` only when one is asked for (the counts cover every marker).
pub fn report_json(report: &Report, kind: Option<&str>, form: Option<Form>) -> Json {
    let markers: Vec<MarkerRow> = report
        .markers
        .iter()
        .filter(|m| kind.is_none_or(|k| m.kind == k) && form.is_none_or(|f| m.form == f))
        .map(row)
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
    let form = match p.form.as_deref() {
        None => None,
        Some(name) => Some(Form::of_name(name).ok_or_else(|| {
            RpcError::new(
                code::INVALID_PARAMS,
                format!("unknown marker form `{name}`; one of: edit, play"),
            )
        })?),
    };
    Ok(report_json(report, p.kind.as_deref(), form))
}

/// `render.markers` (docs/runtime/agent-api.md, "render.markers"): what the
/// last present drew of the game type markers.
pub fn add_method(agent: &agent::Agent) {
    agent.add_method(method::RENDER_MARKERS, Access::Read, |params| {
        answer(params, &LAST.lock().unwrap_or_else(|e| e.into_inner()))
    });
}

#[cfg(feature = "gpu")]
pub use gpu::{batch, register};

#[cfg(feature = "gpu")]
mod gpu {
    use engine::GuestMemory;

    use super::*;
    use crate::world_lines::{ImageMarks, LineStyle, WorldLine, WorldLineSource};
    use gfx::world_lines::{Fade, LabelFace};

    /// Draws the markers while a Forge session has any.
    pub struct Source;

    /// Opacity left at `depth` world units from the eye: as the window's
    /// per-line fade (1 up to the start, 0 from the end, linear between).
    fn fade_factor(fade: Option<(f32, f32)>, depth: f32) -> f32 {
        match fade {
            Some((start, end)) if end > start => (1.0 - (depth - start) / (end - start)).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }

    /// The groups' faces and lines as the native renderer's batch
    /// (docs/game/gametype-markers.md, "The game"): each vertex's alpha is
    /// the group's, times its fade at the vertex's distance from `eye`
    /// (none: no fade). A line is the group's colour at the group's alpha,
    /// a dim line at [`Group::dim_alpha`]; a group's dim lines follow its
    /// own.
    pub fn batch(groups: &[Group], eye: Option<[f32; 3]>) -> native_render::markers::Batch {
        let mut batch = native_render::markers::Batch::default();
        for g in groups {
            let fade = |at: [f32; 3]| eye.map_or(1.0, |eye| fade_factor(g.fade, distance(eye, at)));
            let with = |colour: [f32; 4], at: [f32; 3]| [colour[0], colour[1], colour[2], colour[3] * fade(at)];
            for t in &g.tris {
                batch.tris.extend(t.points.iter().zip(t.colours).map(|(&position, colour)| {
                    native_render::markers::Vertex {
                        position,
                        colour: with(colour, position),
                    }
                }));
            }
            for (edges, alpha) in [(&g.edges, g.alpha), (&g.dim, g.dim_alpha())] {
                let line = [g.colour[0], g.colour[1], g.colour[2], alpha];
                for [a, b] in edges {
                    batch.lines.extend([*a, *b].map(|position| native_render::markers::Vertex {
                        position,
                        colour: with(line, position),
                    }));
                }
            }
        }
        batch
    }

    /// `impl.render.native` ([`hooks::switches::NATIVE`]) as the process
    /// started: the native frame graph may draw the markers.
    fn native_on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| {
            hooks::switches::native_render_on(&hooks::Switches::from_env().unwrap_or_default())
        })
    }

    pub fn register(host: &mut crate::Host) {
        host.add_world_line_source(Arc::new(Source));
        crate::forge_glue::nav_world::register(host);
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
            let on = ACTIVE.load(Ordering::Acquire)
                || WALLS_ACTIVE.load(Ordering::Acquire)
                || PLAY_ACTIVE.load(Ordering::Acquire)
                || PLAY_STALE.load(Ordering::Acquire)
                || crate::forge_glue::pieces_world::active();
            if !on {
                // Nothing to draw: the frame graph's batch goes too.
                native_render::markers::publish(None);
            }
            on
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let p = published().unwrap_or_default();
            #[cfg(feature = "rules")]
            refresh_play(guest);
            let entries = frame_entries(p.entries);
            let (eye, palette) = (eye(), palette(guest));
            let (mut groups, mut report) = draw(&entries, [&p.aimed, &p.held], eye, &palette, BUDGET);
            let (walls, shown) = draw_walls(&walls(), eye, &palette);
            report.shown += walls.len();
            report.markers.extend(shown);
            groups.extend(walls);
            // The pending pieces, the ghost and the Block-out overlay.
            groups.extend(crate::forge_glue::pieces_world::published());
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = report;
            // With the native renderer on, the frame graph draws the faces
            // and the lines (with their X-ray look); the window then draws
            // the labels alone, so no line is drawn twice. Off, or while
            // the graph is not drawing, the window draws the lines.
            let native = native_on();
            native_render::markers::publish(native.then(|| batch(&groups, eye)));
            if groups.is_empty() {
                return None;
            }
            let face = crate::superbounce_font::look(guest).face;
            let native_lines =
                native_render::markers::window_skips_lines(native, native_render::markers::drawn_recently());
            Some(Arc::new(Sample { groups, face, native_lines }))
        }
    }

    /// What the window's world-line pass draws of the markers.
    #[derive(Debug)]
    pub(super) struct Sample {
        pub(super) groups: Vec<Group>,
        pub(super) face: LabelFace,
        /// The native renderer draws the lines: only the labels are left.
        pub(super) native_lines: bool,
    }

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            let style = |g: &Group, alpha: f32| LineStyle {
                width: 2.0,
                alpha,
                fade: g.fade.map(|(start, end)| Fade { start, end }),
                label_scale: 2.0,
                label_centered: true,
                label_face: self.face,
                ..LineStyle::default()
            };
            for g in &self.groups {
                if !self.native_lines || g.label.is_some() {
                    visitor(WorldLine {
                        edges: if self.native_lines { &g.edges[..0] } else { &g.edges[..] },
                        anchor: g.anchor,
                        color: g.colour,
                        dashed: false,
                        label: g.label.as_deref(),
                        style: style(g, g.alpha),
                    });
                }
                // The edit-only parts: their own visit at the dim alpha,
                // with no label.
                if !self.native_lines && !g.dim.is_empty() {
                    visitor(WorldLine {
                        edges: &g.dim[..],
                        anchor: g.anchor,
                        color: g.colour,
                        dashed: false,
                        label: None,
                        style: style(g, g.dim_alpha()),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
