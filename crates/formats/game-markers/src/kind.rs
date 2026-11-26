//! Every marker and its shape (docs/game/gametype-markers.md, "The
//! markers"; docs/game/gametype-marker-forms.md, "The kinds"). The
//! numbers are ours, in world units; a player stands 0.7 tall.
use crate::LIFT;
use crate::area::Area;
use crate::form::{Form, PLATE, PLATE_GAP, POST, Part, Parts, RIM, STUB};
use crate::icon::{self, Icon};
use crate::mesh::{Mesh, Place, Vec3};
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

/// What a player starting location is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SpawnUse {
    /// Any spawn in a round: every starting location until a document
    /// says otherwise (docs/game/gametype-marker-forms.md, slice MK16).
    #[default]
    Respawn,
    /// The round's first spawn only.
    Start,
}

impl SpawnUse {
    /// The name the agent API uses.
    pub fn name(self) -> &'static str {
        match self {
            SpawnUse::Respawn => "respawn",
            SpawnUse::Start => "start",
        }
    }
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
    /// an arrow the way they face; a start's pad is a double ring.
    Spawn(SpawnUse),
    /// Where a team's flag stands: a pad, a pole and a pennant.
    FlagStand,
    /// Where a flag is brought: the capture plate of its area.
    FlagReturn(Area),
    /// Where the bomb starts: a pad with a diamond over it.
    BombSpawn,
    /// Where the bomb is planted: the plant zone of its area.
    BombGoal(Area),
    /// Where the ball starts: a pad with a ball over it.
    BallSpawn,
    /// A hill: its area's wall and a pip at the centre marker.
    Hill(Area),
    /// A territory: its area's wall, and its post and plate in play.
    Territory(Area),
    /// A teleporter's sending end: a column with an arrow up over the
    /// portal.
    TeleporterSender,
    /// A teleporter's receiving end: a column with an arrow down over
    /// the portal, and an arrow out the way a player leaves.
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
    /// `siege_goal`, the final stand: a plant zone in the siege colour.
    Goal(Area),
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

/// The radius of a teleporter's column and its portal's ring.
pub const PORTAL: f32 = 0.4;
/// How tall a teleporter's column stands.
pub const COLUMN: f32 = 1.0;

/// Which pieces a build keeps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    Edit,
    Play,
    EditOnly,
}

/// One pass over a kind's pieces. Every piece is drawn whatever is kept,
/// so a part's place (the plate's height over the body) is the same in
/// every form, and a play piece is the edit form's piece itself.
struct Build {
    keep: Keep,
    c: [f32; 3],
    mesh: Mesh,
    parts: Parts,
    /// The top of the pad, the body and the facing arrow.
    top: f32,
}

impl Build {
    fn new(keep: Keep, c: [f32; 3]) -> Self {
        Self {
            keep,
            c,
            mesh: Mesh::default(),
            parts: Parts::default(),
            top: LIFT,
        }
    }

    fn keeps(&self, play: bool) -> bool {
        match self.keep {
            Keep::Edit => true,
            Keep::Play => play,
            Keep::EditOnly => !play,
        }
    }

    /// A piece of `part`, in play too when `play`.
    fn put(&mut self, part: Part, play: bool, draw: impl FnOnce(&mut Mesh, [f32; 3])) {
        let mut piece = Mesh::default();
        draw(&mut piece, self.c);
        if matches!(part, Part::Pad | Part::Body | Part::Facing)
            && let Some((_, max)) = piece.bounds()
        {
            self.top = self.top.max(max[2]);
        }
        if self.keeps(play) {
            self.parts.insert(part);
            self.mesh.extend(piece);
        }
    }

    /// A part with no lines of its own here: the plate, the label.
    fn mark(&mut self, part: Part, play: bool) {
        if self.keeps(play) {
            self.parts.insert(part);
        }
    }

    /// The facing arrow on the floor from `from` to `to` along x.
    fn facing(&mut self, from: f32, to: f32, play: bool) {
        self.put(Part::Facing, play, |m, c| {
            shapes::arrow(m, [from, 0.0, LIFT], [to, 0.0, LIFT], [0.0, 1.0, 0.0], c);
        });
    }

    /// A pad of `radius`: its ring in play when `ring_in_play`, its disc
    /// in the edit form only.
    fn pad(&mut self, radius: f32, ring_in_play: bool) {
        let centre = [0.0, 0.0, LIFT];
        self.put(Part::Pad, ring_in_play, |m, c| {
            shapes::ring(m, centre, radius, RING, Plane::Floor, c);
        });
        self.put(Part::Pad, false, |m, c| {
            shapes::disc(m, centre, radius, RING, c);
        });
    }

    /// The post from the floor to the plate and the plate, in play too
    /// when `play`: last, once the body's top is known.
    fn sign(&mut self, play: bool) {
        let low = plate_low(self.top);
        self.put(Part::Post, play, |m, c| {
            shapes::line(m, [0.0, 0.0, LIFT], [0.0, 0.0, low], c);
        });
        self.mark(Part::Plate, play);
    }
}

/// The height of the bottom of a plate over a body whose top is `top`.
fn plate_low(top: f32) -> f32 {
    POST.max(top + PLATE_GAP)
}

/// The capture plate of `area` (a flag return): its rim, whose foot is
/// the area's ring on the floor, and an inner ring at half its radius.
/// For an outline of k points: 3k + k / step lines, 2k faces.
fn capture_plate(m: &mut Mesh, area: &Area, c: [f32; 3]) {
    let outline = area.outline();
    shapes::wall_of(m, &outline, LIFT, LIFT + RIM, shapes::post_step(area), c);
    shapes::path(
        m,
        &shapes::level(&shapes::shrunk(&outline, 0.5), LIFT),
        true,
        c,
    );
}

/// The plant zone of `area` (a bomb goal, a siege goal): its rim, two
/// target rings at 0.6 and 0.3 of it and a cross out to the outer target
/// ring at its nearest. For an outline of k points: 4k + k / step + 2 lines, 2k faces.
fn plant_zone(m: &mut Mesh, area: &Area, c: [f32; 3]) {
    let outline = area.outline();
    shapes::wall_of(m, &outline, LIFT, LIFT + RIM, shapes::post_step(area), c);
    for scale in [0.6, 0.3] {
        shapes::path(
            m,
            &shapes::level(&shapes::shrunk(&outline, scale), LIFT),
            true,
            c,
        );
    }
    let near = outline
        .iter()
        .map(|p| p[0].hypot(p[1]))
        .fold(f32::INFINITY, f32::min);
    let arm = 0.6 * if near.is_finite() { near } else { 0.0 };
    shapes::line(m, [-arm, 0.0, LIFT], [arm, 0.0, LIFT], c);
    shapes::line(m, [0.0, -arm, LIFT], [0.0, arm, LIFT], c);
}

impl Kind {
    /// Every kind, with the default area or a unit solid where one has
    /// parameters, and both uses of a spawn: for a legend, a contact
    /// sheet and the tests.
    pub fn samples() -> Vec<Kind> {
        let mut out = vec![
            Kind::Spawn(SpawnUse::Respawn),
            Kind::Spawn(SpawnUse::Start),
            Kind::FlagStand,
            Kind::FlagReturn(Area::plate()),
            Kind::BombSpawn,
            Kind::BombGoal(Area::plate()),
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
            Kind::Goal(Area::plate()),
            Kind::Line,
        ]);
        out
    }

    /// The name the agent API, a legend and a render's file use.
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Spawn(_) => "spawn",
            Kind::FlagStand => "flag_stand",
            Kind::FlagReturn(_) => "flag_return",
            Kind::BombSpawn => "bomb_spawn",
            Kind::BombGoal(_) => "bomb_goal",
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
            Kind::Goal(_) => "siege_goal",
            Kind::Line => "siege_line",
        }
    }

    /// The marker of a label's text: the built-in labels of the netgame
    /// flag types (`forge_doc::builtin_label`) and the ground labels
    /// (`forge_doc::GroundLabel`), an area kind with its default area.
    /// A `hill_corner` is `None`, since one flag does not say what it
    /// draws: a hill's only flag is a [`Kind::Hill`] with its own area,
    /// and three or more corners together are one, with the area
    /// [`Area::of_corners`] gives.
    pub fn of_label(text: &str) -> Option<Kind> {
        Some(match text {
            "ctf_flag_spawn" => Kind::FlagStand,
            "ctf_flag_return" => Kind::FlagReturn(Area::plate()),
            "assault_bomb_spawn" => Kind::BombSpawn,
            "assault_bomb_return" => Kind::BombGoal(Area::plate()),
            "oddball_spawn" => Kind::BallSpawn,
            "teleporter_sender" => Kind::TeleporterSender,
            "teleporter_receiver" => Kind::TeleporterReceiver,
            "territory" => Kind::Territory(Area::default()),
            "ff_drop" => Kind::Drop,
            "ff_base" => Kind::Base,
            "ff_cover" => Kind::Cover,
            "siege_site" => Kind::Site(Area::default()),
            "siege_goal" => Kind::Goal(Area::plate()),
            "siege_line" => Kind::Line,
            _ => return None,
        })
    }

    /// The kind's area, where it has one.
    pub fn area(&self) -> Option<&Area> {
        match self {
            Kind::FlagReturn(area)
            | Kind::BombGoal(area)
            | Kind::Hill(area)
            | Kind::Territory(area)
            | Kind::Site(area)
            | Kind::Goal(area) => Some(area),
            _ => None,
        }
    }

    /// The kind's area to change, where it has one.
    pub fn area_mut(&mut self) -> Option<&mut Area> {
        match self {
            Kind::FlagReturn(area)
            | Kind::BombGoal(area)
            | Kind::Hill(area)
            | Kind::Territory(area)
            | Kind::Site(area)
            | Kind::Goal(area) => Some(area),
            _ => None,
        }
    }

    /// The kind with `area` in place of its own; a kind with no area is
    /// itself.
    pub fn with_area(mut self, area: Area) -> Kind {
        if let Some(own) = self.area_mut() {
            *own = area;
        }
        self
    }

    /// Whether the marker's facing shows: it has an arrow or a front.
    pub fn faces(&self) -> bool {
        matches!(
            self,
            Kind::Spawn(_)
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
            Kind::Spawn(_)
            | Kind::FlagStand
            | Kind::FlagReturn(_)
            | Kind::BombSpawn
            | Kind::BombGoal(_)
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
            Kind::Site(_) | Kind::Goal(_) => Tint::Fixed(palette::SIEGE),
            Kind::Line => Tint::TeamOr(palette::SIEGE),
        }
    }

    /// The icon on the kind's plate; a volume has none, and no plate.
    pub fn icon(&self) -> Option<Icon> {
        Some(match self {
            Kind::Spawn(SpawnUse::Respawn) => Icon::Respawn,
            Kind::Spawn(SpawnUse::Start) => Icon::Start,
            Kind::FlagStand | Kind::Territory(_) => Icon::Pennant,
            Kind::FlagReturn(_) => Icon::FlagReturn,
            Kind::BombSpawn => Icon::Bomb,
            Kind::BombGoal(_) => Icon::Target,
            Kind::BallSpawn => Icon::Ball,
            Kind::Hill(_) => Icon::Crown,
            Kind::TeleporterSender => Icon::Sender,
            Kind::TeleporterReceiver => Icon::Receiver,
            Kind::Item(Item::Weapon) => Icon::Weapon,
            Kind::Item(Item::Vehicle) => Icon::Vehicle,
            Kind::Item(Item::Equipment) => Icon::Equipment,
            Kind::Volume(..) => return None,
            Kind::Drop => Icon::Drop,
            Kind::Base => Icon::Base,
            Kind::Cover => Icon::Shield,
            Kind::Site(_) => Icon::Site,
            Kind::Goal(_) => Icon::Goal,
            Kind::Line => Icon::Banner,
        })
    }

    /// Whether the kind's plate carries its flag's number.
    pub fn numbered(&self) -> bool {
        matches!(self, Kind::Hill(_) | Kind::Territory(_) | Kind::Site(_))
    }

    /// The boundary of `area` alone, in the centre marker's own space, in
    /// `c`: the wall a game draws for a zone in play
    /// (docs/game/gametype-marker-forms.md, "Boundaries"), with none of a
    /// hill's or a site's pip, post or plate ([`shapes::wall`]).
    pub fn wall(area: &Area, c: [f32; 3]) -> Mesh {
        let mut m = Mesh::default();
        shapes::wall(&mut m, area, c);
        m
    }

    /// The parts the kind draws in `form`
    /// (docs/game/gametype-marker-forms.md, "The kinds"). Empty for a
    /// kind with no play form.
    pub fn parts(&self, form: Form) -> Parts {
        self.build(keep_of(form), [1.0; 3]).parts
    }

    /// The kind's lines and faces in `form`, in the marker's own space,
    /// in `c`, without the plate ([`Kind::plate`]). Every piece of the
    /// play form is a piece of the edit form, at the same place.
    pub fn mesh_in(&self, form: Form, c: [f32; 3]) -> Mesh {
        self.build(keep_of(form), c).mesh
    }

    /// The pieces of the edit form that the play form leaves out: what
    /// Forge dims beside the play form ([`crate::form::EDIT_ALPHA`]).
    pub fn edit_only(&self, c: [f32; 3]) -> Mesh {
        self.build(Keep::EditOnly, c).mesh
    }

    /// The centre of the kind's plate in the marker's own space: over
    /// the top of its post, which clears its body. `None` for a kind
    /// with no plate.
    pub fn plate_at(&self) -> Option<Vec3> {
        self.icon()?;
        let top = self.build(Keep::Edit, [1.0; 3]).top;
        Some([0.0, 0.0, plate_low(top) + PLATE])
    }

    /// The kind's plate and icon in the plate's own space
    /// ([`icon::plate`]), with `number` on a hill, a territory or a site.
    /// A drawer places it at [`Kind::plate_at`], turned about the upright
    /// to face the eye and scaled by [`crate::form::icon_scale`].
    pub fn plate(&self, number: Option<u32>, c: [f32; 3]) -> Option<Mesh> {
        let icon = self.icon()?;
        Some(icon::plate(icon, number.filter(|_| self.numbered()), c))
    }

    /// The edit form with its plate facing the marker's front: one mesh,
    /// for a drawer that does not turn plates to the eye.
    pub fn mesh(&self, c: [f32; 3]) -> Mesh {
        let mut m = self.mesh_in(Form::Edit, c);
        if let (Some(at), Some(plate)) = (self.plate_at(), self.plate(None, c)) {
            m.extend(plate.placed(&Place::facing(at, 0.0)));
        }
        m
    }

    /// The pieces of the kind. Each arm's comment counts the lines and
    /// faces of the edit form, then the play form's.
    fn build(&self, keep: Keep, c: [f32; 3]) -> Build {
        let mut b = Build::new(keep, c);
        let floor = [0.0, 0.0, LIFT];
        let along = [1.0, 0.0, 0.0];
        match self {
            // Edit: 16 (+ 16 for a start) + 19 + 3 + 1 lines, 16 faces.
            // Play: none.
            Kind::Spawn(using) => {
                b.pad(0.35, false);
                if *using == SpawnUse::Start {
                    b.put(Part::Pad, false, |m, c| {
                        shapes::ring(m, floor, 0.28, RING, Plane::Floor, c);
                    });
                }
                b.put(Part::Body, false, |m, c| {
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
                    shapes::path(m, &figure, true, c);
                });
                b.facing(0.35, 0.70, false);
                b.sign(false);
            }
            // Edit: 16 + 1 + 1 + 3 + 1 lines, 16 + 1 faces.
            // Play: 16 + 1 lines: the pad's ring and the pole's stub.
            Kind::FlagStand => {
                b.pad(PAD, true);
                b.put(Part::Body, true, |m, c| {
                    shapes::line(m, floor, [0.0, 0.0, STUB], c);
                });
                b.put(Part::Body, false, |m, c| {
                    shapes::line(m, [0.0, 0.0, STUB], [0.0, 0.0, 0.9], c);
                    let pennant = [[0.0, 0.0, 0.9], [-0.3, 0.0, 0.8], [0.0, 0.0, 0.7]];
                    shapes::path(m, &pennant, true, c);
                    shapes::fan(m, &pennant, c);
                });
                b.sign(false);
            }
            // For an outline of k points and a post every `step`:
            // edit 3k + k / step + 1 lines, 2k faces; play the same
            // without the post.
            Kind::FlagReturn(area) => {
                b.put(Part::Body, true, |m, c| capture_plate(m, area, c));
                b.sign(false);
            }
            // Edit: 16 + 12 + 1 lines, 16 + 8 faces. Play: 16 lines.
            Kind::BombSpawn => {
                b.pad(PAD, true);
                b.put(Part::Body, false, |m, c| {
                    shapes::diamond(m, [0.0, 0.0, 0.3], 0.15, c);
                });
                b.sign(false);
            }
            // Edit: 4k + k / step + 2 + 1 lines, 2k faces; play the same
            // without the post.
            Kind::BombGoal(area) | Kind::Goal(area) => {
                b.put(Part::Body, true, |m, c| plant_zone(m, area, c));
                b.sign(false);
            }
            // Edit: 16 + 3 * 12 + 1 lines, 16 faces. Play: 16 lines.
            Kind::BallSpawn => {
                b.pad(PAD, true);
                b.put(Part::Body, false, |m, c| {
                    shapes::ball(m, [0.0, 0.0, 0.3], 0.12, 12, c);
                });
                b.sign(false);
            }
            // Edit: 2k + k / step + 8 + 1 lines, 2k faces. Play: the
            // wall, 2k + k / step lines.
            Kind::Hill(area) => {
                b.put(Part::Boundary, true, |m, c| shapes::wall(m, area, c));
                b.put(Part::Body, false, |m, c| {
                    shapes::ring(m, floor, 0.15, 8, Plane::Floor, c);
                });
                b.sign(false);
            }
            // Edit and play: 2k + k / step + 1 lines, 2k faces, and the
            // plate.
            Kind::Territory(area) | Kind::Site(area) => {
                b.put(Part::Boundary, true, |m, c| shapes::wall(m, area, c));
                b.sign(true);
            }
            // Edit: 16 + 32 + 16 + 3 + 1 lines, 32 faces. Play: the
            // portal, 16 + 16 lines.
            Kind::TeleporterSender => {
                Self::portal(&mut b);
                b.put(Part::Body, false, |m, c| {
                    shapes::arrow(m, [0.0, 0.0, 0.25], [0.0, 0.0, 0.75], along, c);
                });
                b.sign(false);
            }
            // Edit: 16 + 32 + 16 + 3 + 3 + 1 lines, 32 faces. Play: the
            // portal and the way out, 16 + 16 + 3 lines.
            Kind::TeleporterReceiver => {
                Self::portal(&mut b);
                b.put(Part::Body, false, |m, c| {
                    shapes::arrow(m, [0.0, 0.0, 0.75], [0.0, 0.0, 0.25], along, c);
                });
                b.facing(PORTAL, 0.8, true);
                b.sign(false);
            }
            // Edit: 16 + 9 + 1 lines, 16 faces. Play: none.
            Kind::Item(Item::Weapon) => {
                b.pad(PAD, false);
                b.put(Part::Body, false, |m, c| {
                    let outline = WEAPON.map(|p| Plane::Side.point([0.0; 3], p[0], p[1]));
                    shapes::path(m, &outline, true, c);
                });
                b.sign(false);
            }
            // Edit: 16 + 6 + 3 + 1 lines, 16 faces. Play: none.
            Kind::Item(Item::Vehicle) => {
                b.pad(1.0, false);
                b.put(Part::Body, false, |m, c| {
                    let outline = VEHICLE.map(|p| Plane::Side.point([0.0; 3], p[0], p[1]));
                    shapes::path(m, &outline, true, c);
                });
                b.facing(1.0, 1.4, false);
                b.sign(false);
            }
            // Edit: 16 + 12 + 1 lines, 16 + 8 faces. Play: none.
            Kind::Item(Item::Equipment) => {
                b.pad(PAD, false);
                b.put(Part::Body, false, |m, c| {
                    shapes::diamond(m, [0.0, 0.0, 0.35], 0.12, c);
                });
                b.sign(false);
            }
            // A box: 12 lines, 12 faces; a cylinder: 96 lines, 64 faces;
            // a ball: 96 lines. Play: the same for a soft boundary, none
            // for any other volume. No post and no plate.
            Kind::Volume(volume, solid) => {
                let play = *volume == Volume::Soft;
                b.put(Part::Body, play, |m, c| match *solid {
                    Solid::Box { half } => shapes::cuboid(m, half, c),
                    Solid::Cylinder { radius, height } => {
                        let outline = shapes::circle(radius, crate::area::SEGMENTS);
                        shapes::prism(m, &outline, 0.0, height, c);
                    }
                    Solid::Sphere { radius } => {
                        shapes::ball(m, [0.0; 3], radius, crate::area::SEGMENTS, c);
                    }
                });
            }
            // Edit: 16 + 3 + 3 + 1 lines, 16 faces. Play: none.
            Kind::Drop => {
                b.pad(0.5, false);
                b.put(Part::Body, false, |m, c| {
                    shapes::arrow(m, [0.0, 0.0, 1.2], [0.0, 0.0, 0.2], along, c);
                });
                b.facing(0.5, 0.9, false);
                b.sign(false);
            }
            // Edit: 16 + 5 + 1 lines, 3 faces. Play: none.
            Kind::Base => {
                b.put(Part::Pad, false, |m, c| {
                    shapes::ring(m, floor, 0.6, RING, Plane::Floor, c);
                });
                b.put(Part::Body, false, |m, c| {
                    let house = [
                        [0.0, -0.3, LIFT],
                        [0.0, 0.3, LIFT],
                        [0.0, 0.3, 0.4],
                        [0.0, 0.0, 0.65],
                        [0.0, -0.3, 0.4],
                    ];
                    shapes::path(m, &house, true, c);
                    shapes::fan(m, &house, c);
                });
                b.sign(false);
            }
            // Edit: 16 + 4 + 3 + 1 lines, 2 faces. Play: none.
            Kind::Cover => {
                b.put(Part::Pad, false, |m, c| {
                    shapes::ring(m, floor, 0.25, RING, Plane::Floor, c);
                });
                b.put(Part::Body, false, |m, c| {
                    let shield = [
                        [0.15, -0.3, LIFT],
                        [0.15, 0.3, LIFT],
                        [0.15, 0.3, 0.5],
                        [0.15, -0.3, 0.5],
                    ];
                    shapes::path(m, &shield, true, c);
                    shapes::fan(m, &shield, c);
                });
                b.facing(0.25, 0.6, false);
                b.sign(false);
            }
            // Edit: 4 + 3 + 1 lines, 2 faces. Play: none.
            Kind::Line => {
                b.put(Part::Body, false, |m, c| {
                    let banner = [
                        [0.0, -1.0, LIFT],
                        [0.0, 1.0, LIFT],
                        [0.0, 1.0, 0.5],
                        [0.0, -1.0, 0.5],
                    ];
                    shapes::path(m, &banner, true, c);
                    shapes::fan(m, &banner, c);
                });
                b.facing(0.0, 0.5, false);
                b.sign(false);
            }
        }
        b.mark(Part::Label, false);
        b
    }

    /// A teleporter's portal and column: the ring on the floor and an
    /// upright hoop one unit tall in play (16 + 16 lines), the column's
    /// sides over the ring in the edit form (32 lines, 32 faces).
    fn portal(b: &mut Build) {
        let ring = shapes::circle(PORTAL, RING);
        b.put(Part::Body, true, |m, c| {
            shapes::path(m, &shapes::level(&ring, LIFT), true, c);
            shapes::ring(m, [0.0, 0.0, LIFT + 0.5], 0.5, RING, Plane::Front, c);
        });
        b.put(Part::Body, false, |m, c| {
            shapes::sides(m, &ring, LIFT, COLUMN, c);
        });
    }
}

fn keep_of(form: Form) -> Keep {
    match form {
        Form::Edit => Keep::Edit,
        Form::Play => Keep::Play,
    }
}
