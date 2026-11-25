//! Every marker and its shape (docs/game/gametype-markers.md, "The
//! markers"). The numbers are ours, in world units; a player stands 0.7
//! tall.
use crate::LIFT;
use crate::area::Area;
use crate::mesh::{Mesh, Vec3};
use crate::palette::{self, Tint};
use crate::shapes::{self, Plane};

/// The lines a small ring has.
pub const RING: usize = 16;
/// The radius of the pad a goal or a small item stands on.
pub const PAD: f32 = 0.3;

/// What an item spawner makes, for its outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Weapon,
    Vehicle,
    /// Grenades, powerups and other equipment.
    Equipment,
}

/// What a volume does, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Volume {
    Trigger,
    Kill,
    /// A soft boundary: a player past it is warned and then killed.
    Soft,
    BspSwitch,
}

/// A volume's solid, round the marker's origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Solid {
    /// A box of these half sizes along the marker's axes, its middle at
    /// the origin.
    Box { half: Vec3 },
    /// An upright cylinder standing on the origin.
    Cylinder { radius: f32, height: f32 },
    /// A ball round the origin.
    Sphere { radius: f32 },
}

/// A marker.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Where a player starts or respawns: a standing figure on a pad and
    /// an arrow the way they face.
    Spawn,
    /// Where a team's flag stands: a pad, a pole and a pennant.
    FlagStand,
    /// Where a flag is brought: a pad with an inner ring and an arrow
    /// down onto it.
    FlagReturn,
    /// Where the bomb starts: a pad with a diamond over it.
    BombSpawn,
    /// Where the bomb is planted: a target lying on the floor.
    BombGoal,
    /// Where the ball starts: a pad with a ball over it.
    BallSpawn,
    /// A hill: its area's walls and a pip at the centre marker.
    Hill(Area),
    /// A territory: its area's walls and a post with a diamond on it.
    Territory(Area),
    /// A teleporter's sending end: a column with an arrow up.
    TeleporterSender,
    /// A teleporter's receiving end: a column with an arrow down and an
    /// arrow out the way a player leaves.
    TeleporterReceiver,
    /// An item spawner: a pad and the outline of what it makes.
    Item(Item),
    /// A volume: its solid, translucent.
    Volume(Volume, Solid),
    /// `ff_drop`, where a wave arrives: a pad, an arrow down and an
    /// arrow the way the wave faces.
    Drop,
    /// `ff_base`, a base the actors push at: a ring and a house.
    Base,
    /// `ff_cover`, a firing position: a shield and an arrow the way it
    /// faces.
    Cover,
    /// `siege_site`, a site the attackers take: a territory's shape in
    /// the siege colour.
    Site(Area),
    /// `siege_goal`, the final stand: a target with a diamond over it.
    Goal,
    /// `siege_line`, where a side spawns in a phase: a banner across the
    /// way they face.
    Line,
}

/// Half of the standing figure, as (y, z) from the feet up: the marker's
/// front view. The other half mirrors it and the crotch joins the two,
/// so the outline has `2 * FIGURE.len() + 1` lines.
pub const FIGURE: [[f32; 2]; 9] = [
    [0.04, 0.00],
    [0.12, 0.00],
    [0.12, 0.34],
    [0.20, 0.34],
    [0.20, 0.56],
    [0.06, 0.58],
    [0.08, 0.62],
    [0.08, 0.68],
    [0.04, 0.70],
];
/// Where the figure's legs join, as (y, z).
pub const CROTCH: [f32; 2] = [0.0, 0.28];

/// A weapon's outline from its side, as (x, z): a long gun.
pub const WEAPON: [[f32; 2]; 9] = [
    [-0.28, 0.40],
    [-0.10, 0.42],
    [0.28, 0.42],
    [0.28, 0.38],
    [0.02, 0.36],
    [-0.02, 0.26],
    [-0.08, 0.26],
    [-0.10, 0.34],
    [-0.28, 0.32],
];
/// A vehicle's outline from its side, as (x, z): a wedge with a cab.
pub const VEHICLE: [[f32; 2]; 6] = [
    [-0.80, 0.25],
    [-0.80, 0.60],
    [-0.20, 0.75],
    [0.30, 0.55],
    [0.85, 0.45],
    [0.85, 0.25],
];

impl Kind {
    /// Every kind, with the default area or a unit solid where one has
    /// parameters: for a legend, a contact sheet and the tests.
    pub fn samples() -> Vec<Kind> {
        let mut out = vec![
            Kind::Spawn,
            Kind::FlagStand,
            Kind::FlagReturn,
            Kind::BombSpawn,
            Kind::BombGoal,
            Kind::BallSpawn,
            Kind::Hill(Area::default()),
            Kind::Territory(Area::default()),
            Kind::TeleporterSender,
            Kind::TeleporterReceiver,
            Kind::Item(Item::Weapon),
            Kind::Item(Item::Vehicle),
            Kind::Item(Item::Equipment),
        ];
        for volume in [
            Volume::Trigger,
            Volume::Kill,
            Volume::Soft,
            Volume::BspSwitch,
        ] {
            out.push(Kind::Volume(
                volume,
                Solid::Box {
                    half: [1.0, 1.0, 1.0],
                },
            ));
        }
        out.extend([
            Kind::Volume(
                Volume::Trigger,
                Solid::Cylinder {
                    radius: 1.0,
                    height: 2.0,
                },
            ),
            Kind::Volume(Volume::Trigger, Solid::Sphere { radius: 1.0 }),
            Kind::Drop,
            Kind::Base,
            Kind::Cover,
            Kind::Site(Area::default()),
            Kind::Goal,
            Kind::Line,
        ]);
        out
    }

    /// The name the agent API, a legend and a render's file use.
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Spawn => "spawn",
            Kind::FlagStand => "flag_stand",
            Kind::FlagReturn => "flag_return",
            Kind::BombSpawn => "bomb_spawn",
            Kind::BombGoal => "bomb_goal",
            Kind::BallSpawn => "ball_spawn",
            Kind::Hill(_) => "hill",
            Kind::Territory(_) => "territory",
            Kind::TeleporterSender => "teleporter_sender",
            Kind::TeleporterReceiver => "teleporter_receiver",
            Kind::Item(Item::Weapon) => "weapon_spawn",
            Kind::Item(Item::Vehicle) => "vehicle_spawn",
            Kind::Item(Item::Equipment) => "equipment_spawn",
            Kind::Volume(Volume::Trigger, _) => "trigger_volume",
            Kind::Volume(Volume::Kill, _) => "kill_volume",
            Kind::Volume(Volume::Soft, _) => "soft_volume",
            Kind::Volume(Volume::BspSwitch, _) => "bsp_switch",
            Kind::Drop => "ff_drop",
            Kind::Base => "ff_base",
            Kind::Cover => "ff_cover",
            Kind::Site(_) => "siege_site",
            Kind::Goal => "siege_goal",
            Kind::Line => "siege_line",
        }
    }

    /// The marker of a label's text: the built-in labels of the netgame
    /// flag types (`forge_doc::builtin_label`) and the ground labels
    /// (`forge_doc::GroundLabel`), an area kind with the default area.
    /// A `hill_corner` is `None`, since one flag does not say what it
    /// draws: a hill's only flag is a [`Kind::Hill`] with its own area,
    /// and three or more corners together are one, with the area
    /// [`Area::of_corners`] gives.
    pub fn of_label(text: &str) -> Option<Kind> {
        Some(match text {
            "ctf_flag_spawn" => Kind::FlagStand,
            "ctf_flag_return" => Kind::FlagReturn,
            "assault_bomb_spawn" => Kind::BombSpawn,
            "assault_bomb_return" => Kind::BombGoal,
            "oddball_spawn" => Kind::BallSpawn,
            "teleporter_sender" => Kind::TeleporterSender,
            "teleporter_receiver" => Kind::TeleporterReceiver,
            "territory" => Kind::Territory(Area::default()),
            "ff_drop" => Kind::Drop,
            "ff_base" => Kind::Base,
            "ff_cover" => Kind::Cover,
            "siege_site" => Kind::Site(Area::default()),
            "siege_goal" => Kind::Goal,
            "siege_line" => Kind::Line,
            _ => return None,
        })
    }

    /// The kind's area, where it has one.
    pub fn area(&self) -> Option<&Area> {
        match self {
            Kind::Hill(area) | Kind::Territory(area) | Kind::Site(area) => Some(area),
            _ => None,
        }
    }

    /// Whether the marker's facing shows: it has an arrow or a front.
    pub fn faces(&self) -> bool {
        matches!(
            self,
            Kind::Spawn
                | Kind::FlagStand
                | Kind::TeleporterReceiver
                | Kind::Item(_)
                | Kind::Drop
                | Kind::Base
                | Kind::Cover
                | Kind::Line
        )
    }

    /// How the kind is coloured.
    pub fn tint(&self) -> Tint {
        match self {
            Kind::Spawn
            | Kind::FlagStand
            | Kind::FlagReturn
            | Kind::BombSpawn
            | Kind::BombGoal
            | Kind::BallSpawn
            | Kind::Hill(_)
            | Kind::Territory(_)
            | Kind::Base => Tint::Team,
            Kind::TeleporterSender => Tint::Fixed(palette::SENDER),
            Kind::TeleporterReceiver => Tint::Fixed(palette::RECEIVER),
            Kind::Item(Item::Weapon) => Tint::TeamOr(palette::WEAPON),
            Kind::Item(Item::Vehicle) => Tint::TeamOr(palette::VEHICLE),
            Kind::Item(Item::Equipment) => Tint::TeamOr(palette::EQUIPMENT),
            Kind::Volume(Volume::Trigger, _) => Tint::Fixed(palette::TRIGGER),
            Kind::Volume(Volume::Kill, _) => Tint::Fixed(palette::KILL),
            Kind::Volume(Volume::Soft, _) => Tint::Fixed(palette::SOFT),
            Kind::Volume(Volume::BspSwitch, _) => Tint::Fixed(palette::BSP_SWITCH),
            Kind::Drop => Tint::Fixed(palette::DROP),
            Kind::Cover => Tint::Fixed(palette::COVER),
            Kind::Site(_) | Kind::Goal => Tint::Fixed(palette::SIEGE),
            Kind::Line => Tint::TeamOr(palette::SIEGE),
        }
    }

    /// The boundary of `area` alone, in the centre marker's own space, in
    /// `c`: the wall a game draws for a zone in play
    /// (docs/game/gametype-markers.md, "The boundary in play"), with none
    /// of a hill's or a site's pip. For an outline of k points: 3k lines,
    /// 2k faces.
    pub fn wall(area: &Area, c: [f32; 3]) -> Mesh {
        let mut m = Mesh::default();
        shapes::prism(&mut m, &area.outline(), -area.below, area.above, c);
        m
    }

    /// The kind's lines and faces in the marker's own space, in `c`.
    /// Each arm's comment counts what it adds.
    pub fn mesh(&self, c: [f32; 3]) -> Mesh {
        let mut m = Mesh::default();
        let floor = [0.0, 0.0, LIFT];
        let across = [0.0, 1.0, 0.0];
        let along = [1.0, 0.0, 0.0];
        match self {
            // 16 + 19 + 3 lines, 16 faces.
            Kind::Spawn => {
                shapes::pad(&mut m, 0.35, RING, c);
                let mut figure = vec![Plane::Front.point([0.0; 3], CROTCH[0], CROTCH[1])];
                figure.extend(
                    FIGURE
                        .iter()
                        .map(|p| Plane::Front.point([0.0; 3], p[0], p[1])),
                );
                figure.extend(
                    FIGURE
                        .iter()
                        .rev()
                        .map(|p| Plane::Front.point([0.0; 3], -p[0], p[1])),
                );
                shapes::path(&mut m, &figure, true, c);
                shapes::arrow(&mut m, [0.35, 0.0, LIFT], [0.70, 0.0, LIFT], across, c);
            }
            // 16 + 1 + 3 lines, 16 + 1 faces.
            Kind::FlagStand => {
                shapes::pad(&mut m, PAD, RING, c);
                shapes::line(&mut m, floor, [0.0, 0.0, 0.9], c);
                let pennant = [[0.0, 0.0, 0.9], [-0.3, 0.0, 0.8], [0.0, 0.0, 0.7]];
                shapes::path(&mut m, &pennant, true, c);
                shapes::fan(&mut m, &pennant, c);
            }
            // 16 + 16 + 3 lines, 16 faces.
            Kind::FlagReturn => {
                shapes::pad(&mut m, PAD, RING, c);
                shapes::ring(&mut m, floor, 0.18, RING, Plane::Floor, c);
                shapes::arrow(&mut m, [0.0, 0.0, 0.6], [0.0, 0.0, 0.15], along, c);
            }
            // 16 + 12 lines, 16 + 8 faces.
            Kind::BombSpawn => {
                shapes::pad(&mut m, PAD, RING, c);
                shapes::diamond(&mut m, [0.0, 0.0, 0.3], 0.15, c);
            }
            // 16 + 16 + 2 lines, 16 faces.
            Kind::BombGoal => {
                shapes::pad(&mut m, 0.4, RING, c);
                shapes::ring(&mut m, floor, 0.2, RING, Plane::Floor, c);
                shapes::line(&mut m, [-0.4, 0.0, LIFT], [0.4, 0.0, LIFT], c);
                shapes::line(&mut m, [0.0, -0.4, LIFT], [0.0, 0.4, LIFT], c);
            }
            // 16 + 3 * 12 lines, 16 faces.
            Kind::BallSpawn => {
                shapes::pad(&mut m, PAD, RING, c);
                shapes::ball(&mut m, [0.0, 0.0, 0.3], 0.12, 12, c);
            }
            // For an outline of k points: 3k + 8 + 1 lines, 2k faces.
            Kind::Hill(area) => {
                shapes::prism(&mut m, &area.outline(), -area.below, area.above, c);
                shapes::ring(&mut m, floor, 0.15, 8, Plane::Floor, c);
                shapes::line(&mut m, floor, [0.0, 0.0, 0.5], c);
            }
            // For an outline of k points: 3k + 1 + 12 lines, 2k + 8 faces.
            Kind::Territory(area) | Kind::Site(area) => {
                shapes::prism(&mut m, &area.outline(), -area.below, area.above, c);
                shapes::line(&mut m, floor, [0.0, 0.0, 0.8], c);
                shapes::diamond(&mut m, [0.0, 0.0, 0.95], 0.15, c);
            }
            // 48 + 3 lines, 32 faces.
            Kind::TeleporterSender => {
                shapes::prism(&mut m, &shapes::circle(0.4, RING), LIFT, 1.0, c);
                shapes::arrow(&mut m, [0.0, 0.0, 0.25], [0.0, 0.0, 0.75], along, c);
            }
            // 48 + 3 + 3 lines, 32 faces.
            Kind::TeleporterReceiver => {
                shapes::prism(&mut m, &shapes::circle(0.4, RING), LIFT, 1.0, c);
                shapes::arrow(&mut m, [0.0, 0.0, 0.75], [0.0, 0.0, 0.25], along, c);
                shapes::arrow(&mut m, [0.4, 0.0, LIFT], [0.8, 0.0, LIFT], across, c);
            }
            // 16 + 9 lines, 16 faces.
            Kind::Item(Item::Weapon) => {
                shapes::pad(&mut m, PAD, RING, c);
                let outline = WEAPON.map(|p| Plane::Side.point([0.0; 3], p[0], p[1]));
                shapes::path(&mut m, &outline, true, c);
            }
            // 16 + 6 + 3 lines, 16 faces.
            Kind::Item(Item::Vehicle) => {
                shapes::pad(&mut m, 1.0, RING, c);
                let outline = VEHICLE.map(|p| Plane::Side.point([0.0; 3], p[0], p[1]));
                shapes::path(&mut m, &outline, true, c);
                shapes::arrow(&mut m, [1.0, 0.0, LIFT], [1.4, 0.0, LIFT], across, c);
            }
            // 16 + 12 lines, 16 + 8 faces.
            Kind::Item(Item::Equipment) => {
                shapes::pad(&mut m, PAD, RING, c);
                shapes::diamond(&mut m, [0.0, 0.0, 0.35], 0.12, c);
            }
            // 12 lines, 12 faces.
            Kind::Volume(_, Solid::Box { half }) => shapes::cuboid(&mut m, *half, c),
            // 96 lines, 64 faces.
            Kind::Volume(_, Solid::Cylinder { radius, height }) => {
                let outline = shapes::circle(*radius, crate::area::SEGMENTS);
                shapes::prism(&mut m, &outline, 0.0, *height, c);
            }
            // 96 lines.
            Kind::Volume(_, Solid::Sphere { radius }) => {
                shapes::ball(&mut m, [0.0; 3], *radius, crate::area::SEGMENTS, c);
            }
            // 16 + 3 + 3 lines, 16 faces.
            Kind::Drop => {
                shapes::pad(&mut m, 0.5, RING, c);
                shapes::arrow(&mut m, [0.0, 0.0, 1.2], [0.0, 0.0, 0.2], along, c);
                shapes::arrow(&mut m, [0.5, 0.0, LIFT], [0.9, 0.0, LIFT], across, c);
            }
            // 16 + 5 lines, 3 faces.
            Kind::Base => {
                shapes::ring(&mut m, floor, 0.6, RING, Plane::Floor, c);
                let house = [
                    [0.0, -0.3, LIFT],
                    [0.0, 0.3, LIFT],
                    [0.0, 0.3, 0.4],
                    [0.0, 0.0, 0.65],
                    [0.0, -0.3, 0.4],
                ];
                shapes::path(&mut m, &house, true, c);
                shapes::fan(&mut m, &house, c);
            }
            // 16 + 4 + 3 lines, 2 faces.
            Kind::Cover => {
                shapes::ring(&mut m, floor, 0.25, RING, Plane::Floor, c);
                let shield = [
                    [0.15, -0.3, LIFT],
                    [0.15, 0.3, LIFT],
                    [0.15, 0.3, 0.5],
                    [0.15, -0.3, 0.5],
                ];
                shapes::path(&mut m, &shield, true, c);
                shapes::fan(&mut m, &shield, c);
                shapes::arrow(&mut m, [0.25, 0.0, LIFT], [0.6, 0.0, LIFT], across, c);
            }
            // 16 + 16 + 1 + 12 lines, 8 faces.
            Kind::Goal => {
                shapes::ring(&mut m, floor, 0.5, RING, Plane::Floor, c);
                shapes::ring(&mut m, floor, 0.25, RING, Plane::Floor, c);
                shapes::line(&mut m, floor, [0.0, 0.0, 0.4], c);
                shapes::diamond(&mut m, [0.0, 0.0, 0.6], 0.2, c);
            }
            // 4 + 3 lines, 2 faces.
            Kind::Line => {
                let banner = [
                    [0.0, -1.0, LIFT],
                    [0.0, 1.0, LIFT],
                    [0.0, 1.0, 0.5],
                    [0.0, -1.0, 0.5],
                ];
                shapes::path(&mut m, &banner, true, c);
                shapes::fan(&mut m, &banner, c);
                shapes::arrow(&mut m, floor, [0.5, 0.0, LIFT], across, c);
            }
        }
        m
    }
}
