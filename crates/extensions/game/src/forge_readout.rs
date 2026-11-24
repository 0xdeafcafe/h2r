//! The Forge readout's runtime (docs/game/forge-map-variants-ui.md, "The
//! readout", slice H3): what the monitor sees each tick, as the readout's
//! [`View`] and the poses of its world marks.
//!
//! - [`Scene`] is what the session publishes ([`publish`]): whether the
//!   monitor is up, the eye, the thing aimed at or held, a refusal, the
//!   budget, the hill or territory outline, and the things with no body.
//! - [`frame`] is pure: the scene's [`Values`] and poses. With the monitor
//!   off every element is hidden and no mark has a pose. The near marks are
//!   the [`NEAR`] nearest bodiless things within [`NEAR_RADIUS`], by
//!   distance then identity; the thing aimed at has marks of its own and is
//!   not one of them. A held thing has an `under` mark on the ground below
//!   it and a `from` mark where it was picked up.
//! - [`placed`] and [`aimed`] make a scene's entries from the session's
//!   [`forge::Thing`]s; the layout's field names are the ones of
//!   `map::scenario_layout`.
//! - [`hook`] registers the HUD with the HUD kit and feeds it every tick
//!   from the published scene. It is part of the `forge` feature, which has
//!   no switch of its own for it.
//!
//! The readout is presentation: it reads the session and never writes it.
//! A fact the session does not give yet is absent: a hold needs the
//! ground's height (no ray is kept for it yet), a refusal's sentence and the
//! budget by kind wait for S6, and no kind has a ring (its radius is not
//! traced). The zone is the aimed hill corner's hill ([`zone_of`]).

use std::sync::{Mutex, MutexGuard, OnceLock};

use forge::{Endpoint, Thing as Entry, ThingKind, Value as Field};
use hud_kit_core::views::forge_readout::{
    self as fr, Budget, Hold, NEAR, NEARS, Near, SEL_MIN, Source, Target, Thing, UNDER_MIN, View,
    ZONES, zone_edges,
};
use hud_kit_core::model::Values;
use hud_kit_core::world::{Pose, TEAMS};

use crate::hud_kit::{self, Handle};

/// A near mark's reach, world units.
pub const NEAR_RADIUS: f32 = 12.0;

/// A thing with no body, as a near mark.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub id: Endpoint,
    pub at: [f32; 3],
    pub near: Near,
}

/// The thing aimed at or held, and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Aimed {
    pub id: Endpoint,
    pub target: Target,
    pub at: [f32; 3],
}

/// What the monitor sees this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    /// The monitor is up. Off, the readout shows nothing.
    pub monitor: bool,
    pub eye: [f32; 3],
    pub target: Option<Aimed>,
    pub hold: Option<Hold>,
    pub refusal: Option<String>,
    pub budget: Option<Budget>,
    pub zone: Vec<[f32; 3]>,
    pub things: Vec<Placed>,
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}

/// The things the near marks show: those within [`NEAR_RADIUS`] of `eye`,
/// the nearest [`NEAR`], by distance then identity. A thing whose identity
/// is `skip` (the one aimed at) is left out.
pub fn choose_near(things: &[Placed], eye: [f32; 3], skip: Option<Endpoint>) -> Vec<&Placed> {
    let skip = skip.map(Endpoint::identity);
    let mut near: Vec<(f32, &Placed)> = things
        .iter()
        .filter(|p| Some(p.id.identity()) != skip)
        .map(|p| (dist2(p.at, eye), p))
        .filter(|(d, _)| d.is_finite() && *d <= NEAR_RADIUS * NEAR_RADIUS)
        .collect();
    near.sort_by(|(a, pa), (b, pb)| {
        a.total_cmp(b)
            .then_with(|| pa.id.identity().cmp(&pb.id.identity()))
    });
    near.into_iter().take(NEAR).map(|(_, p)| p).collect()
}

fn heading(yaw: f32) -> [f32; 2] {
    let r = if yaw.is_finite() {
        yaw.to_radians()
    } else {
        0.0
    };
    [r.cos(), r.sin()]
}

/// The readout's values and the poses of the marks it shows.
pub fn frame(scene: &Scene) -> (Values, Vec<(&'static str, Pose)>) {
    if !scene.monitor {
        return (fr::values(&View::default()), Vec::new());
    }
    // The aimed thing's marks are its own.
    let near = choose_near(
        &scene.things,
        scene.eye,
        scene.target.as_ref().map(|a| a.id),
    );
    let view = View {
        target: scene.target.as_ref().map(|a| a.target.clone()),
        hold: scene.target.as_ref().and(scene.hold.clone()),
        refusal: scene.refusal.clone(),
        budget: scene.budget.clone(),
        near: near.iter().map(|p| p.near.clone()).collect(),
        zone: scene.zone.clone(),
    };
    let mut poses = Vec::new();
    if let Some(a) = &scene.target {
        let hold = view.hold.as_ref();
        let dir = heading(a.target.yaw);
        let r = a.target.radius.max(SEL_MIN);
        let height = hold.map_or(0.0, |h| h.height);
        let ground = [
            a.at[0],
            a.at[1],
            a.at[2] - if height.is_finite() { height } else { 0.0 },
        ];
        poses.push((fr::SEL, Pose::at(ground, [r, r]).heading(dir)));
        if let Some(ring) = a.target.ring {
            poses.push((fr::RING, Pose::at(a.at, [ring, ring])));
        }
        if a.target.thing.faces() {
            poses.push((fr::FACE, Pose::at(a.at, [0.3, 0.1]).heading(dir)));
        }
        if let Some(h) = hold {
            if h.height.is_finite() && h.height > UNDER_MIN {
                poses.push((fr::UNDER, Pose::at(ground, [0.1, 0.0])));
            }
            if let Some(from) = h.from {
                poses.push((fr::FROM, Pose::at(from, [r, r]).heading(dir)));
            }
        }
    }
    for (id, p) in NEARS.iter().zip(&near) {
        poses.push((*id, Pose::at(p.at, [0.08, 0.0])));
    }
    for (i, id) in ZONES.iter().enumerate().take(zone_edges(&view)) {
        let [a, b] = [view.zone[i], view.zone[i + 1]];
        let mid = [
            (a[0] + b[0]) / 2.0,
            (a[1] + b[1]) / 2.0,
            (a[2] + b[2]) / 2.0,
        ];
        let d = [b[0] - a[0], b[1] - a[1]];
        poses.push((*id, Pose::at(mid, [d[0].hypot(d[1]) / 2.0, 0.0]).heading(d)));
    }
    (fr::values(&view), poses)
}

fn int(thing: &Entry, name: &str) -> Option<i64> {
    thing.fields.iter().find_map(|(field, value)| match value {
        Field::Int(v) if *field == name => Some(*v),
        _ => None,
    })
}

/// A team index the HUD can colour: `0..TEAMS`; the engine's no-team
/// values have none.
fn team_of(thing: &Entry) -> Option<u8> {
    int(thing, "team")
        .and_then(|t| u8::try_from(t).ok())
        .filter(|t| *t < TEAMS)
}

fn team_name(team: Option<u8>) -> String {
    match team {
        Some(t) => format!("Team {}", t + 1),
        None => "Neutral".into(),
    }
}

/// The tag's base name of the first tag field, `Rocket Launcher`.
fn tag_name(thing: &Entry) -> Option<String> {
    thing.fields.iter().find_map(|(_, value)| match value {
        Field::Tag(Some(tag)) => Some(
            tag.path
                .rsplit('\\')
                .next()
                .unwrap_or(&tag.path)
                .replace('_', " "),
        ),
        _ => None,
    })
}

fn goal_role(thing: &Entry) -> &'static str {
    match int(thing, "type") {
        Some(0 | 1) => "Flag",
        Some(2 | 3) => "Bomb",
        Some(4) => "Ball",
        Some(10) => "Territory",
        Some(11..=18) => "Hill",
        _ => "Goal",
    }
}

/// A short near label: `Spawn`, `Flag`, `Hill 2`, the item's name.
fn near_label(thing: &Entry) -> String {
    match thing.kind {
        ThingKind::StartingPoint => "Spawn".into(),
        ThingKind::Teleporter => match int(thing, "type") {
            Some(7) => "Sender".into(),
            Some(8) => "Receiver".into(),
            _ => "Teleporter".into(),
        },
        ThingKind::Goal => match (goal_role(thing), int(thing, "identifier")) {
            ("Hill", Some(n)) => format!("Hill {n}"),
            (role, _) => role.into(),
        },
        ThingKind::ItemSpawner => tag_name(thing).unwrap_or_else(|| "Item".into()),
        ThingKind::Object => tag_name(thing).unwrap_or_else(|| "Object".into()),
    }
}

/// A thing with no body as a near mark; none for an object, which has one.
pub fn placed(thing: &Entry, game: Option<i32>) -> Option<Placed> {
    if thing.kind == ThingKind::Object {
        return None;
    }
    Some(Placed {
        id: thing.id,
        at: thing.frame.position,
        near: Near {
            label: near_label(thing),
            team: match thing.kind {
                ThingKind::StartingPoint | ThingKind::Goal => team_of(thing),
                _ => None,
            },
            in_game_type: forge::tool::in_game(thing, game),
        },
    })
}

/// The readout's target for `thing`, called `name`, at `at` (the ghost's
/// while it is carried) facing `yaw` degrees, with its bounding `radius`.
pub fn aimed(
    thing: &Entry,
    name: String,
    at: [f32; 3],
    yaw: f32,
    radius: f32,
    game: Option<i32>,
) -> Aimed {
    let kind = match thing.kind {
        ThingKind::Object => Thing::Object {
            group: String::new(),
        },
        ThingKind::ItemSpawner => Thing::Spawner {
            respawn_secs: int(thing, "respawn time")
                .and_then(|s| u32::try_from(s).ok())
                .unwrap_or(0),
            types: String::new(),
        },
        ThingKind::StartingPoint => Thing::StartingPoint {
            team: team_name(team_of(thing)),
            types: String::new(),
        },
        ThingKind::Goal => Thing::Goal {
            role: goal_role(thing).into(),
            team: team_name(team_of(thing)),
            order: int(thing, "type")
                .filter(|t| (11..=18).contains(t))
                .map(|t| (t - 10) as u8),
        },
        ThingKind::Teleporter => Thing::Teleporter {
            role: match int(thing, "type") {
                Some(7) => "Sender".into(),
                _ => "Receiver".into(),
            },
            channel: int(thing, "identifier")
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(0),
        },
    };
    let source = match (thing.own, thing.changed) {
        (false, _) => Source::Added,
        (true, false) => Source::FromMap,
        (true, true) => Source::FromMapChanged,
    };
    Aimed {
        id: thing.id,
        target: Target {
            name,
            thing: kind,
            source,
            not_in: (!forge::tool::in_game(thing, game)).then(|| "this game type".to_owned()),
            yaw,
            radius,
            ring: None,
        },
        at,
    }
}

/// The readout's hold line for a thing carried at `at`, `height` over the
/// ground, with the movement snap (`None` off), the axes in force and where
/// it was picked up. Not blocked: the tool reports no refusal here.
pub fn hold(
    at: [f32; 3],
    height: f32,
    snap: Option<f32>,
    local: bool,
    from: Option<[f32; 3]>,
) -> Hold {
    Hold {
        position: at,
        height,
        snap,
        axes: if local {
            fr::Axes::Local
        } else {
            fr::Axes::World
        },
        blocked: None,
        from,
    }
}

/// The outline of the hill `goal` is a corner of: the positions of the
/// hill's corners in order (type 11 to 18, the corner's number) with the
/// first again at the end, or none for a goal that is not a hill corner or
/// the only corner there is. A hill is the corners that share an
/// identifier.
pub fn zone_of(goal: &Entry, all: &[Entry]) -> Vec<[f32; 3]> {
    let corner = |t: &Entry| {
        (t.kind == ThingKind::Goal)
            .then(|| int(t, "type"))
            .flatten()
            .filter(|ty| (11..=18).contains(ty))
    };
    let (Some(_), Some(hill)) = (corner(goal), int(goal, "identifier")) else {
        return Vec::new();
    };
    let mut corners: Vec<(i64, [f32; 3])> = all
        .iter()
        .filter(|t| int(t, "identifier") == Some(hill))
        .filter_map(|t| corner(t).map(|ty| (ty, t.frame.position)))
        .collect();
    corners.sort_by_key(|(ty, _)| *ty);
    if corners.len() < 2 {
        return Vec::new();
    }
    let mut out: Vec<[f32; 3]> = corners.into_iter().map(|(_, at)| at).collect();
    out.push(out[0]);
    out
}

static SCENE: Mutex<Option<Scene>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the readout shows from the next tick on.
pub fn publish(scene: Scene) {
    *lock(&SCENE) = Some(scene);
}

/// The readout shows nothing from the next tick on.
pub fn clear() {
    *lock(&SCENE) = None;
}

fn marks() -> impl Iterator<Item = &'static str> {
    fr::MARKS.into_iter().chain(NEARS).chain(ZONES)
}

struct Live {
    handle: Handle,
    last: Option<(Values, Vec<(&'static str, Pose)>)>,
}

impl Live {
    fn tick(&mut self) {
        let scene = lock(&SCENE).clone().unwrap_or_default();
        let now = frame(&scene);
        if self.last.as_ref() == Some(&now) {
            return;
        }
        let (values, poses) = &now;
        self.handle.publish(values.clone());
        for id in marks() {
            let pose = poses.iter().find(|(i, _)| *i == id).map(|(_, p)| *p);
            self.handle.world_mark(id, pose);
        }
        self.last = Some(now);
    }
}

/// Registers the readout with the HUD kit and feeds it each tick. The
/// `forge` feature's build calls it.
pub fn hook(b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
    static HANDLE: OnceLock<Handle> = OnceLock::new();
    let handle = *HANDLE.get_or_init(|| {
        let handle = hud_kit::register(std::sync::Arc::new(fr::Readout));
        handle.publish(fr::values(&View::default()));
        handle
    });
    let live = Mutex::new(Live { handle, last: None });
    b.frame()
        .on_tick("forge readout: values", move |_| lock(&live).tick());
    Ok(())
}

#[cfg(test)]
#[path = "forge_readout_tests.rs"]
mod tests;
