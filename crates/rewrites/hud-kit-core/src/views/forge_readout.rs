//! The Forge readout (docs/game/forge-map-variants-ui.md, "The
//! readout"): three rows of font 6 right of the motion tracker, at the
//! superbounce readout's places ([`super::superbounce_readout`]), and
//! marks on the geometry, with nothing in the middle of the screen.
//!
//! - `fgr.name`: the name of the thing aimed at or held. Hidden with
//!   nothing aimed at.
//! - `fgr.facts`: the kind, then its facts ([`facts_text`]); held, its
//!   position and heading. A fact that would overrun is dropped whole,
//!   last first.
//! - `fgr.guide`: aimed, where the thing is from, or `Not in <game
//!   type>`; held, the placement line (an arrow and the height above
//!   the ground, the snap and the axes) or why it cannot go there; a
//!   hold that starts with a tenth of the budget or less left, the
//!   limit that binds; after a refusal, its sentence ([`guide_text`]).
//! - `fgr.sel`, `fgr.ring`, `fgr.face`, `fgr.under`, `fgr.from`,
//!   `fgr.near.1` to `.12` and `fgr.zone.1` to `.8`: world marks
//!   ([`sample_poses`] gives the preview's poses; the runtime gives the
//!   real ones). A near mark is in its team's colour (`Style::Team`), so
//!   its label is short; where the map gives the team no colour the
//!   runtime keeps the team in the label. The zone marks are
//!   `Shape::Line` edges between the corners of the hill or territory
//!   the aimed goal belongs to ([`View::zone`]).
//!
//! The readout is fullscreen only, a HUD of its own beside the monitor's
//! `forge` HUD as the superbounce readout is beside its HUD. Its rows
//! are at the superbounce readout's places, and the two never show
//! together (a monitor has no bounce to read, the superbounce readout is
//! for a player's biped): [`Readout::never_with`] declares it, so the
//! `own` check does not compare them.
//!
//! [`View`] is what it shows; the game's runtime (`game::forge_readout`,
//! slice H3) fills it from what the session publishes each tick, so
//! `forge.status` and the HUD cannot disagree.

use crate::model::{Describe, Hud, Style, Value, Values};
use crate::world::Pose;

use super::superbounce_readout::{ARROW_DOWN, ARROW_UP, FALL_UP, INFO_UP, SPEED_UP};

pub const NAME: &str = "forge-readout";

pub const NAME_ROW: &str = "fgr.name";
pub const FACTS: &str = "fgr.facts";
pub const GUIDE: &str = "fgr.guide";
pub const SEL: &str = "fgr.sel";
pub const RING: &str = "fgr.ring";
pub const FACE: &str = "fgr.face";
pub const UNDER: &str = "fgr.under";
pub const FROM: &str = "fgr.from";
/// How many near marks.
pub const NEAR: usize = 12;
/// The near marks, nearest first.
pub const NEARS: [&str; NEAR] = [
    "fgr.near.1",
    "fgr.near.2",
    "fgr.near.3",
    "fgr.near.4",
    "fgr.near.5",
    "fgr.near.6",
    "fgr.near.7",
    "fgr.near.8",
    "fgr.near.9",
    "fgr.near.10",
    "fgr.near.11",
    "fgr.near.12",
];
/// The single world marks, in draw order.
pub const MARKS: [&str; 5] = [SEL, RING, FACE, UNDER, FROM];
/// How many zone lines.
pub const ZONE: usize = 8;
/// The zone lines: the outline of the hill or territory the aimed goal
/// belongs to, an edge between each pair of corners in order.
pub const ZONES: [&str; ZONE] = [
    "fgr.zone.1",
    "fgr.zone.2",
    "fgr.zone.3",
    "fgr.zone.4",
    "fgr.zone.5",
    "fgr.zone.6",
    "fgr.zone.7",
    "fgr.zone.8",
];

/// The rows' tops from the frame's bottom, the superbounce readout's:
/// y 376, 397 and 418 fullscreen.
pub const NAME_UP: i16 = SPEED_UP;
pub const FACTS_UP: i16 = FALL_UP;
pub const GUIDE_UP: i16 = INFO_UP;

/// The widest texts. The rows end inside the frame; a text longer than
/// these has facts or words dropped whole ([`fit_facts`], [`fit_words`]).
pub const NAME_LONGEST: &str = "Assault Rifle Ammo Cache Crate";
pub const FACTS_LONGEST: &str = "Starting point  Neutral  Slayer, CTF, KotH";
pub const GUIDE_LONGEST: &str = "Item spawners are full (100) in this map.";
/// The widest near mark label: an item's name, with room for the team
/// where the map gives it no colour and the label carries it.
pub const NEAR_LONGEST: &str = "Rocket Launcher Red";

/// The separator between a row's parts.
pub const SEPARATOR: &str = "  ";
/// Under this height (world units) above the ground a held thing has no
/// `under` mark.
pub const UNDER_MIN: f32 = 0.05;
/// The selection bracket is at least this half size.
pub const SEL_MIN: f32 = 0.25;
/// A hold that starts with this share of the budget or less left shows
/// the binding limit on the guide row (the runtime sets
/// [`View::budget`] by it).
pub const BUDGET_LOW: f32 = 0.1;
/// The largest number a coordinate shows.
pub const COORD_MAX: f32 = 999.99;
pub const HEIGHT_MAX: f32 = 99.99;

/// What a thing is, with what the readout says of it.
#[derive(Clone, Debug, PartialEq)]
pub enum Thing {
    Object { group: String },
    Spawner { respawn_secs: u32, types: String },
    StartingPoint { team: String, types: String },
    Goal { role: String, team: String, order: Option<u8> },
    Teleporter { role: String, channel: u8 },
}

impl Thing {
    pub fn kind(&self) -> &'static str {
        match self {
            Thing::Object { .. } => "Object",
            Thing::Spawner { .. } => "Item spawner",
            Thing::StartingPoint { .. } => "Starting point",
            Thing::Goal { .. } => "Goal",
            Thing::Teleporter { .. } => "Teleporter",
        }
    }

    /// Whether the thing has a facing and no body to show it: it gets
    /// the `fgr.face` arrow.
    pub fn faces(&self) -> bool {
        matches!(self, Thing::StartingPoint { .. } | Thing::Goal { .. } | Thing::Teleporter { .. })
    }
}

/// Where a thing is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    FromMap,
    FromMapMoved,
    FromMapChanged,
    Added,
}

impl Source {
    pub fn text(self) -> &'static str {
        match self {
            Source::FromMap => "From the map",
            Source::FromMapMoved => "From the map, moved",
            Source::FromMapChanged => "From the map, changed",
            Source::Added => "Added",
        }
    }
}

/// The thing aimed at or held.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// The header: the tag's base name or the role.
    pub name: String,
    pub thing: Thing,
    pub source: Source,
    /// The session's game type, when the thing is of another: the guide
    /// says `Not in <game type>`.
    pub not_in: Option<String>,
    /// Its facing, degrees.
    pub yaw: f32,
    /// Its bounding radius, world units: the selection bracket's size.
    pub radius: f32,
    /// The radius of the ring the engine uses for its kind, if it has
    /// one.
    pub ring: Option<f32>,
}

/// The movement axes in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axes {
    World,
    Local,
}

impl Axes {
    pub fn text(self) -> &'static str {
        match self {
            Axes::World => "World axes",
            Axes::Local => "Local axes",
        }
    }
}

/// A held thing.
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    /// Its position, world units.
    pub position: [f32; 3],
    /// The height above the ground under it, world units; negative under.
    pub height: f32,
    /// The movement snap, world units; `None` off.
    pub snap: Option<f32>,
    pub axes: Axes,
    /// Why it cannot be dropped here, if it cannot: the sentence the
    /// guide row gives.
    pub blocked: Option<String>,
    /// Where it was picked up; `None` for a new thing.
    pub from: Option<[f32; 3]>,
}

/// The binding limit when a hold starts with a tenth or less left.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub left: u32,
    pub singular: String,
    pub plural: String,
}

/// A near thing with no body: a starting point, a goal, a teleporter, a
/// spawner with nothing out.
#[derive(Clone, Debug, PartialEq)]
pub struct Near {
    /// `Spawn`, `Flag`, `Hill 2`, the item's name. Short: the team is the
    /// mark's colour; where the map gives the team no colour the runtime
    /// adds it here.
    pub label: String,
    /// The thing's team, `0..world::TEAMS`: the mark's colour
    /// (`Style::Team`). `None` is the user's HUD colour.
    pub team: Option<u8>,
    /// Whether the thing is in the session's game type.
    pub in_game_type: bool,
}

/// What the readout shows. `None` hides a piece.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub target: Option<Target>,
    /// With a target, that it is held.
    pub hold: Option<Hold>,
    /// For four seconds after a refusal: its sentence.
    pub refusal: Option<String>,
    pub budget: Option<Budget>,
    /// The nearest things, nearest first; at most [`NEAR`] show.
    pub near: Vec<Near>,
    /// The corners of the hill or territory the aimed goal belongs to,
    /// in order. An edge between each pair in turn is a `Shape::Line`
    /// mark, so a closed outline ends with the first corner again; at
    /// most [`ZONE`] edges show.
    pub zone: Vec<[f32; 3]>,
}

fn clamped(v: f32, max: f32) -> f32 {
    if v.is_finite() { v.clamp(-max, max) } else { 0.0 }
}

/// `parts` joined by [`SEPARATOR`], the last dropped whole until the
/// text is at most `longest` characters or one part is left. Empty parts
/// are skipped.
pub fn fit_facts(parts: &[String], longest: &str) -> String {
    let mut parts: Vec<&str> = parts.iter().map(String::as_str).filter(|p| !p.is_empty()).collect();
    let max = longest.chars().count();
    while parts.len() > 1 && parts.join(SEPARATOR).chars().count() > max {
        parts.pop();
    }
    parts.join(SEPARATOR)
}

/// `text` with whole words dropped from its end until it is at most
/// `longest` characters or one word is left; never cut with an
/// ellipsis.
pub fn fit_words(text: &str, longest: &str) -> String {
    // The text's own spacing is kept (the hold line's groups are two
    // spaces apart): only its last word goes, with the gap before it.
    let max = longest.chars().count();
    let mut out = text.trim();
    while out.chars().count() > max {
        match out.rfind(char::is_whitespace) {
            Some(at) => out = out[..at].trim_end(),
            None => break,
        }
    }
    out.to_owned()
}

/// The facts row for `v`: held, the position and heading; aimed, the
/// kind and its facts. `None` with nothing aimed at.
pub fn facts_text(v: &View) -> Option<String> {
    let t = v.target.as_ref()?;
    let parts: Vec<String> = match &v.hold {
        Some(h) => vec![position_text(h.position), yaw_text(t.yaw)],
        None => {
            let mut p = vec![t.thing.kind().to_owned()];
            match &t.thing {
                Thing::Object { group } => p.push(group.clone()),
                Thing::Spawner { respawn_secs, types } => {
                    p.push(format!("Respawn {respawn_secs} s"));
                    p.push(types.clone());
                }
                Thing::StartingPoint { team, types } => {
                    p.push(team.clone());
                    p.push(types.clone());
                }
                Thing::Goal { role, team, order } => {
                    p.push(role.clone());
                    p.push(team.clone());
                    p.extend(order.map(|o| format!("Order {o}")));
                }
                Thing::Teleporter { role, channel } => {
                    p.push(role.clone());
                    p.push(format!("Channel {channel}"));
                }
            }
            p
        }
    };
    Some(fit_facts(&parts, FACTS_LONGEST))
}

/// `X 12.34  Y -5.60  Z 1.20`, the Coordinates page's units (world units,
/// two places).
pub fn position_text(p: [f32; 3]) -> String {
    let c = |v| clamped(v, COORD_MAX);
    format!("X {:.2}  Y {:.2}  Z {:.2}", c(p[0]), c(p[1]), c(p[2]))
}

/// `Yaw 90`, whole degrees, 0 to 359.
pub fn yaw_text(yaw: f32) -> String {
    let d = if yaw.is_finite() { yaw.round() as i64 } else { 0 };
    format!("Yaw {}", d.rem_euclid(360))
}

/// The placement line of a hold: an arrow and the height above the
/// ground under it, the snap and the axes, `(arrow) 0.85 m  Snap 0.25
/// World axes`.
pub fn hold_text(h: &Hold) -> String {
    let height = clamped(h.height, HEIGHT_MAX);
    let arrow = if height < 0.0 { ARROW_DOWN } else { ARROW_UP };
    let snap = match h.snap {
        Some(s) if s.is_finite() && s > 0.0 => format!("Snap {s}"),
        _ => "Snap off".to_owned(),
    };
    format!("{arrow} {:.2} m{SEPARATOR}{snap}{SEPARATOR}{}", height.abs(), h.axes.text())
}

/// `3 item spawners left`.
pub fn budget_text(b: &Budget) -> String {
    let noun = if b.left == 1 { &b.singular } else { &b.plural };
    format!("{} {noun} left", b.left)
}

/// The guide row: a refusal's sentence, the budget's binding limit, why
/// a hold cannot go here, the placement line, `Not in <game type>`, or
/// where the thing is from.
pub fn guide_text(v: &View) -> Option<String> {
    let line = if let Some(s) = &v.refusal {
        s.clone()
    } else if let (Some(b), Some(_)) = (&v.budget, &v.hold) {
        budget_text(b)
    } else if let Some(why) = v.hold.as_ref().and_then(|h| h.blocked.as_ref()) {
        why.clone()
    } else if let Some(h) = &v.hold {
        hold_text(h)
    } else {
        let t = v.target.as_ref()?;
        match &t.not_in {
            Some(g) => format!("Not in {g}"),
            None => t.source.text().to_owned(),
        }
    };
    Some(fit_words(&line, GUIDE_LONGEST))
}

/// A near mark's label: its label alone. The team is the mark's colour
/// (`Style::Team`), so the label stays short; where the map gives the
/// team no colour the runtime puts the team in the label itself.
pub fn near_text(n: &Near) -> String {
    fit_words(&n.label, NEAR_LONGEST)
}

/// A near mark's style: `Inactive` out of the session's game type, else
/// its team's colour, or the user's HUD colour with no team.
pub fn near_style(n: &Near) -> Style {
    if !n.in_game_type {
        Style::Inactive
    } else {
        match n.team {
            Some(t) => Style::Team(t),
            None => Style::Normal,
        }
    }
}

/// How many zone edges `v` shows: one between each pair of corners in
/// order, at most [`ZONE`].
pub fn zone_edges(v: &View) -> usize {
    v.zone.len().saturating_sub(1).min(ZONE)
}

/// The elements, in draw order: the three rows, then the marks.
pub fn hud() -> Hud {
    crate::data::builtin(NAME)
}

/// The values for `v`: every element of [`hud`], once.
pub fn values(v: &View) -> Values {
    let mut out = Values::default();
    let text = |t: Option<String>| t.map_or_else(Value::hidden, Value::text);
    let shown = |on: bool| if on { Value::shown() } else { Value::hidden() };
    let target = v.target.as_ref();
    let hold = target.and(v.hold.as_ref());
    out.set(NAME_ROW, text(target.map(|t| fit_words(&t.name, NAME_LONGEST))));
    out.set(FACTS, text(facts_text(v)));
    out.set(GUIDE, text(guide_text(v)));
    let sel = match hold {
        Some(h) if h.blocked.is_some() => Style::Inactive,
        Some(_) => Style::Go,
        None => Style::Normal,
    };
    out.set(
        SEL,
        target.map_or_else(Value::hidden, |t| Value::text(fit_words(&t.name, NAME_LONGEST)).styled(sel)),
    );
    out.set(RING, shown(target.is_some_and(|t| t.ring.is_some())));
    out.set(FACE, shown(target.is_some_and(|t| t.thing.faces())));
    out.set(UNDER, shown(hold.is_some_and(|h| h.height.is_finite() && h.height > UNDER_MIN)));
    out.set(
        FROM,
        match hold.and_then(|h| h.from) {
            Some(_) => Value::shown().styled(Style::Dim),
            None => Value::hidden(),
        },
    );
    for (i, id) in NEARS.iter().enumerate() {
        out.set(
            id,
            match v.near.get(i) {
                Some(n) => Value::text(near_text(n)).styled(near_style(n)),
                None => Value::hidden(),
            },
        );
    }
    let edges = zone_edges(v);
    for (i, id) in ZONES.iter().enumerate() {
        out.set(id, shown(i < edges));
    }
    out
}

fn along(yaw: f32) -> [f32; 2] {
    let r = if yaw.is_finite() { yaw.to_radians() } else { 0.0 };
    [r.cos(), r.sin()]
}

/// The preview's poses for `v` in a sample scene: the thing at the
/// origin, its ground under it, where it was picked up to its left, and
/// the near things in two rows ahead. Only the marks `v` shows have one.
/// The runtime's are the real ones (`Handle::world_mark`, slice H3).
pub fn sample_poses(v: &View) -> Vec<(&'static str, Pose)> {
    let mut out = Vec::new();
    let hold = v.target.as_ref().and(v.hold.as_ref());
    if let Some(t) = &v.target {
        let up = hold.map_or(0.0, |h| clamped(h.height, HEIGHT_MAX).max(0.0));
        let dir = along(t.yaw);
        let r = t.radius.max(SEL_MIN);
        out.push((SEL, Pose::at([0.0; 3], [r, r]).heading(dir)));
        if let Some(ring) = t.ring {
            out.push((RING, Pose::at([0.0, 0.0, up], [ring, ring])));
        }
        if t.thing.faces() {
            out.push((FACE, Pose::at([0.0, 0.0, up], [0.3, 0.1]).heading(dir)));
        }
        if let Some(h) = hold {
            if h.height.is_finite() && h.height > UNDER_MIN {
                out.push((UNDER, Pose::at([0.0; 3], [0.1, 0.0])));
            }
            if h.from.is_some() {
                out.push((FROM, Pose::at([-1.2, 0.0, 0.0], [r, r]).heading(dir)));
            }
        }
    }
    for (i, id) in NEARS.iter().enumerate().take(v.near.len()) {
        let (col, row) = ((i % 6) as f32, (i / 6) as f32);
        out.push((*id, Pose::at([-1.5 + 0.6 * col, 1.0 + 0.6 * row, 0.0], [0.08, 0.0])));
    }
    for (i, id) in ZONES.iter().enumerate().take(zone_edges(v)) {
        let [a, b] = [v.zone[i], v.zone[i + 1]];
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = d[0].hypot(d[1]);
        out.push((*id, Pose::at(mid, [len / 2.0, 0.0]).heading(d)));
    }
    out
}

fn target(name: &str, thing: Thing, source: Source, radius: f32, ring: Option<f32>) -> Target {
    Target {
        name: name.into(),
        thing,
        source,
        not_in: None,
        yaw: 90.0,
        radius,
        ring,
    }
}

fn near(label: &str, team: Option<u8>, in_game_type: bool) -> Near {
    Near {
        label: label.into(),
        team,
        in_game_type,
    }
}

fn spawner() -> Target {
    target(
        "Rocket Launcher",
        Thing::Spawner {
            respawn_secs: 30,
            types: "Slayer, CTF".into(),
        },
        Source::FromMap,
        0.3,
        None,
    )
}

fn held(t: Target, height: f32, snap: Option<f32>, blocked: Option<&str>) -> View {
    View {
        target: Some(t),
        hold: Some(Hold {
            position: [12.34, -5.6, 1.2],
            height,
            snap,
            axes: Axes::World,
            blocked: blocked.map(Into::into),
            from: Some([10.0, -5.6, 0.0]),
        }),
        ..View::default()
    }
}

/// The preview's states, by name, as [`View`]s.
pub fn views() -> Vec<(&'static str, View)> {
    let aim = |t: Target| View {
        target: Some(t),
        ..View::default()
    };
    let object = target(
        "Crate, Metal, Small",
        Thing::Object { group: "Scenery".into() },
        Source::FromMapMoved,
        0.5,
        None,
    );
    let start = target(
        "Starting point",
        Thing::StartingPoint {
            team: "Red team".into(),
            types: "Slayer, CTF".into(),
        },
        Source::FromMap,
        0.4,
        Some(0.4),
    );
    let hill = target(
        "Hill 2 corner 3",
        Thing::Goal {
            role: "Hill".into(),
            team: "Neutral".into(),
            order: Some(2),
        },
        Source::FromMapChanged,
        0.3,
        Some(1.0),
    );
    let tele = target(
        "Teleporter",
        Thing::Teleporter {
            role: "Sender".into(),
            channel: 3,
        },
        Source::Added,
        0.3,
        Some(0.6),
    );
    let mut other = spawner();
    other.not_in = Some("Slayer".into());
    let mut refused = held(spawner(), 0.0, Some(0.25), None);
    refused.refusal = Some("Item spawners are full (100).".into());
    let mut low = held(spawner(), 0.85, Some(0.25), None);
    low.budget = Some(Budget {
        left: 3,
        singular: "item spawner".into(),
        plural: "item spawners".into(),
    });
    // The sample teams: 0 red, 1 blue.
    let idle = View {
        near: vec![near("Spawn", Some(0), true), near("Flag", Some(1), true), near("Hill 2", None, false)],
        ..View::default()
    };
    let full = View {
        near: (1..=NEAR)
            .map(|i| match i % 4 {
                0 => near("Rocket Launcher", Some(0), true),
                1 => near("Spawn", Some(1), true),
                2 => near("Flag", None, false),
                _ => near(&format!("Hill {i}"), None, true),
            })
            .collect(),
        ..View::default()
    };
    let mut zone = aim(hill.clone());
    // A closed outline of nine corners, so every zone line shows.
    zone.zone = vec![
        [-1.0, -1.0, 0.0],
        [0.0, -1.4, 0.0],
        [1.0, -1.0, 0.0],
        [1.4, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.4, 0.0],
        [-1.0, 1.0, 0.0],
        [-1.4, 0.0, 0.0],
        [-1.0, -1.0, 0.0],
    ];
    vec![
        ("idle", idle),
        ("aim-object", aim(object)),
        ("aim-spawner", aim(spawner())),
        ("aim-starting-point-red", aim(start)),
        ("aim-goal-hill", aim(hill)),
        ("aim-teleporter", aim(tele)),
        ("aim-other-game-type", aim(other)),
        ("hold-allowed", held(spawner(), 0.85, None, None)),
        (
            "hold-refused",
            held(spawner(), 0.3, Some(0.25), Some("Too close to another spawner")),
        ),
        ("hold-snapped", held(spawner(), 0.5, Some(0.25), None)),
        ("refused", refused),
        ("budget-low", low),
        ("near-full", full),
        ("zone", zone),
    ]
}

/// The readout's description.
pub struct Readout;

impl Describe for Readout {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        views().into_iter().map(|(n, v)| (n, values(&v))).collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("Forge edits a multiplayer map's own placements")
    }

    /// The rows are at the superbounce readout's places; the two never
    /// show together (a monitor has no bounce to read, the superbounce
    /// readout is for a player's biped), so the `own` check does not
    /// compare them.
    fn never_with(&self) -> &'static [&'static str] {
        &[super::superbounce_readout::NAME]
    }

    fn world(&self, state: &str) -> Vec<(&'static str, Pose)> {
        views()
            .into_iter()
            .find(|(n, _)| *n == state)
            .map(|(_, v)| sample_poses(&v))
            .unwrap_or_default()
    }
}
