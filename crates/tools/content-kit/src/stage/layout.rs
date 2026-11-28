//! The stage's layout: `content/stage/layout.yaml` resolved into absolute
//! places.
//!
//! The YAML describes one bay's furniture (the spawn, the subject pad, the
//! row of numbered pads, the references and the cameras), relative to the
//! bay's origin, and lists the bays by origin. [`Layout::parse`] places the
//! furniture in every bay, resolves the sizes named after the player
//! (`player`, `player_width`) from [`Player`], aims each camera at the
//! subject pad, and refuses a layout where two things take the same floor,
//! where something leaves its bay's floor, where two bays meet, or where a
//! camera's eye is inside a reference.
//!
//! Units are world units (1 = 3.048 m): +x forward (the spawn faces it),
//! +y left, +z up. Angles in the YAML are degrees; in [`Layout`] radians.

use serde::{Deserialize, Serialize};

use crate::stage::{Error, Result};

/// The player's size, read from the base map's biped when the stage is
/// built, so no figure of it is in the repository.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Player {
    /// Standing height.
    pub height: f32,
    /// Radius.
    pub radius: f32,
}

/// How a bay is lit (`content_kit::stage::light`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Light {
    /// Even grey light, no direction.
    Flat,
    /// The sky's sun and a sky fill.
    Day,
    /// A closed room at near-black.
    Dark,
}

/// The sun the day bay is lit by, and the sky's disc is drawn at.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sun {
    /// Degrees counter-clockwise from +x, seen from above, of the sun's
    /// side of the sky.
    pub azimuth: f32,
    /// Degrees above the horizon, over 0 and up to 90.
    pub elevation: f32,
    /// The light's colour.
    pub color: [f32; 3],
    /// A face square to the sun takes `color * power` from it.
    pub power: f32,
}

impl Sun {
    /// The unit vector from the ground towards the sun.
    pub fn direction(&self) -> [f32; 3] {
        let (az, el) = (self.azimuth.to_radians(), self.elevation.to_radians());
        [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
    }
}

/// The lights of the three bays (`content_kit::stage::light`). Levels are in
/// units of full light: 1 is what a lit surface shows its texture at.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lights {
    /// The day bay's sun.
    pub sun: Sun,
    /// The day bay's sky fill, at a face looking up; a face looking down
    /// takes none, and one looking aside half.
    pub fill: [f32; 3],
    /// The flat bay's even grey.
    pub flat: f32,
    /// The dark bay's grey: near-black.
    pub dark: f32,
}

impl Default for Lights {
    fn default() -> Self {
        Lights {
            sun: Sun {
                azimuth: 215.0,
                elevation: 45.0,
                color: [1.0, 0.95, 0.85],
                power: 1.0,
            },
            fill: [0.26, 0.30, 0.38],
            flat: 1.0,
            dark: 0.03,
        }
    }
}

impl Lights {
    fn check(&self) -> Result<()> {
        let s = &self.sun;
        finite("lights.sun", &[s.azimuth, s.elevation, s.power])?;
        finite("lights.sun.color", &s.color)?;
        finite("lights.fill", &self.fill)?;
        finite("lights", &[self.flat, self.dark])?;
        if !(s.elevation > 0.0 && s.elevation <= 90.0) {
            return Err(Error::Layout(format!(
                "lights.sun.elevation: {} degrees is not over 0 and up to 90",
                s.elevation
            )));
        }
        positive("lights.sun.power", s.power)?;
        positive("lights.flat", self.flat)?;
        if s.color.iter().chain(&self.fill).any(|v| *v < 0.0) || self.dark < 0.0 {
            return Err(Error::Layout("lights: a negative level".into()));
        }
        if self.dark >= self.flat {
            return Err(Error::Layout(format!(
                "lights.dark: {} is not under lights.flat {}",
                self.dark, self.flat
            )));
        }
        Ok(())
    }
}

/// Every bay's floor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Floor {
    /// The side of the square slab, centred on the bay's origin.
    pub size: f32,
    /// The distance labels' spacing along both axes (and the heavy lines').
    pub label_every: f32,
    /// The grid texture's side in texels (a power of two), spread over the
    /// whole slab.
    pub texels: u32,
}

impl Floor {
    /// Texels per world unit.
    pub fn texels_per_unit(&self) -> f32 {
        self.texels as f32 / self.size
    }
}

/// Walls and a roof around a bay's floor: the dark bay's closed room.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enclosure {
    /// The room's inside height.
    pub height: f32,
    /// The walls' and roof's thickness.
    pub thickness: f32,
}

/// The resolved stage.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layout {
    /// [`crate::stage::version()`].
    pub version: u32,
    pub player: Player,
    pub floor: Floor,
    pub lights: Lights,
    pub bays: Vec<Bay>,
}

/// One bay, every point absolute.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bay {
    pub name: String,
    pub origin: [f32; 3],
    pub light: Light,
    pub enclosure: Option<Enclosure>,
    /// Where the player starts, on the floor.
    pub spawn: Spawn,
    /// The pad the content under test stands on (number 0).
    pub subject: Pad,
    /// The numbered pads, from 1.
    pub pads: Vec<Pad>,
    pub references: Vec<Reference>,
    pub cameras: Vec<Camera>,
}

/// A player start.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Spawn {
    pub point: [f32; 3],
    /// Radians from +x towards +y.
    pub facing: f32,
}

/// A pad: a marked square of floor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pad {
    /// 0 for the subject pad, else its number.
    pub number: u32,
    /// Its centre, on the floor.
    pub point: [f32; 3],
    /// Half its side.
    pub radius: f32,
}

/// Where the subject stands in a bay ([`Layout::subject_place`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SubjectPlace {
    pub bay: String,
    /// The subject pad's centre, on the floor.
    pub point: [f32; 3],
    /// Radians from +x towards +y: towards the player's spawn.
    pub facing: f32,
}

/// A scale or movement reference.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reference {
    pub name: String,
    pub shape: Shape,
    /// The centre of its footprint, on the floor.
    pub point: [f32; 3],
    /// Radians: the shape's +x (a ramp's or stair's way up) turned by this.
    pub facing: f32,
    /// `[x min, x max, y min, y max, z min, z max]`.
    pub bounds: [f32; 6],
}

/// What a reference is, in its own frame: footprint centred on the origin,
/// standing on `z = 0`, +x forward.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shape {
    /// A box of this size.
    Box { size: [f32; 3] },
    /// A frame around an opening: two posts and a lintel, `post` thick,
    /// `depth` deep (along y).
    Doorway {
        opening: [f32; 2],
        post: f32,
        depth: f32,
    },
    /// A wedge rising `rise` along +x at `angle` degrees.
    Ramp { width: f32, rise: f32, angle: f32 },
    /// `steps` steps of `rise` up along +x, `tread` deep each.
    Stair {
        width: f32,
        rise: f32,
        tread: f32,
        steps: u32,
    },
}

/// A named eye looking at a bay's subject pad.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Camera {
    pub name: String,
    pub eye: [f32; 3],
    /// The point it looks at: over the subject pad.
    pub target: [f32; 3],
    /// Radians from +x towards +y.
    pub yaw: f32,
    /// Radians up from level.
    pub pitch: f32,
    /// Horizontal field of view, degrees.
    pub fov: f32,
}

impl Camera {
    /// The unit direction it looks along.
    pub fn forward(&self) -> [f32; 3] {
        let (c, s) = (self.pitch.cos(), self.pitch.sin());
        [c * self.yaw.cos(), c * self.yaw.sin(), s]
    }
}

// The YAML.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    floor: Floor,
    #[serde(default)]
    lights: Lights,
    bays: Vec<BaySource>,
    spawn: SpawnSource,
    subject: PadSource,
    pads: RowSource,
    references: Vec<RefSource>,
    cameras: CameraSource,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BaySource {
    name: String,
    origin: [f32; 3],
    light: Light,
    #[serde(default)]
    enclosure: Option<Enclosure>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnSource {
    at: [f32; 2],
    facing: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PadSource {
    at: [f32; 2],
    radius: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RowSource {
    first: [f32; 2],
    step: [f32; 2],
    count: u32,
    radius: f32,
}

/// One reference: a name, a place and exactly one shape.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefSource {
    name: String,
    at: [f32; 2],
    #[serde(default)]
    facing: f32,
    #[serde(default, rename = "box")]
    cuboid: Option<BoxSource>,
    #[serde(default)]
    doorway: Option<DoorwaySource>,
    #[serde(default)]
    ramp: Option<RampSource>,
    #[serde(default)]
    stair: Option<StairSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoxSource {
    size: [Dim; 3],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DoorwaySource {
    opening: [Dim; 2],
    post: f32,
    depth: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RampSource {
    width: f32,
    rise: f32,
    angle: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StairSource {
    width: f32,
    rise: f32,
    tread: f32,
    steps: u32,
}

/// A length: a number, or a size of the player's.
#[derive(Deserialize)]
#[serde(untagged)]
enum Dim {
    Units(f32),
    Named(String),
}

impl Dim {
    fn resolve(&self, player: &Player, what: &str) -> Result<f32> {
        match self {
            Dim::Units(v) => Ok(*v),
            Dim::Named(n) => match n.as_str() {
                "player" => Ok(player.height),
                "player_width" => Ok(2.0 * player.radius),
                other => Err(Error::Layout(format!(
                    "{what}: {other:?} is not a length (a number, `player` or `player_width`)"
                ))),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CameraSource {
    /// Horizontal field of view, degrees.
    fov: f32,
    /// The height over the subject pad every camera looks at.
    aim: f32,
    /// Eyes, from the subject pad.
    eyes: Vec<EyeSource>,
    orbit: OrbitSource,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EyeSource {
    name: String,
    at: [f32; 3],
}

/// `count` eyes round the subject pad, `orbit-0` on +x, then every
/// `360 / count` degrees towards +y.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrbitSource {
    count: u32,
    radius: f32,
    height: f32,
}

/// Things that take floor: overlap is refused (touching is allowed).
const TOUCH: f32 = 1e-4;

fn finite(what: &str, v: &[f32]) -> Result<()> {
    if v.iter().all(|x| x.is_finite()) {
        Ok(())
    } else {
        Err(Error::Layout(format!("{what}: a value that is not finite")))
    }
}

fn positive(what: &str, v: f32) -> Result<()> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(Error::Layout(format!(
            "{what}: {v} is not a positive length"
        )))
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// A floor rectangle `[x min, x max, y min, y max]`.
type Rect = [f32; 4];

fn square(c: [f32; 3], r: f32) -> Rect {
    [c[0] - r, c[0] + r, c[1] - r, c[1] + r]
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a[0] < b[1] - TOUCH && b[0] < a[1] - TOUCH && a[2] < b[3] - TOUCH && b[2] < a[3] - TOUCH
}

fn inside(r: &Rect, outer: &Rect) -> bool {
    r[0] >= outer[0] - TOUCH
        && r[1] <= outer[1] + TOUCH
        && r[2] >= outer[2] - TOUCH
        && r[3] <= outer[3] + TOUCH
}

impl Layout {
    /// The layout shipped with the repository ([`crate::stage::SOURCE`]).
    pub fn builtin(player: Player) -> Result<Self> {
        Self::parse(crate::stage::SOURCE, player)
    }

    /// Parses and checks a layout (module docs).
    pub fn parse(yaml: &str, player: Player) -> Result<Self> {
        positive("the player's height", player.height)?;
        positive("the player's radius", player.radius)?;
        let src: Source = serde_yaml_ng::from_str(yaml)?;
        let floor = src.floor;
        positive("floor.size", floor.size)?;
        positive("floor.label_every", floor.label_every)?;
        if !floor.texels.is_power_of_two() || !(4..=2048).contains(&floor.texels) {
            return Err(Error::Layout(format!(
                "floor.texels: {} is not a power of two from 4 to 2048",
                floor.texels
            )));
        }
        src.lights.check()?;
        if src.bays.is_empty() {
            return Err(Error::Layout("no bays".into()));
        }
        let cams = &src.cameras;
        positive("cameras.fov", cams.fov)?;
        if cams.fov >= 180.0 {
            return Err(Error::Layout(format!("cameras.fov: {} degrees", cams.fov)));
        }
        finite("cameras.aim", &[cams.aim])?;
        if cams.orbit.count == 0 {
            return Err(Error::Layout("cameras.orbit.count: no eyes".into()));
        }
        positive("cameras.orbit.radius", cams.orbit.radius)?;
        positive("pads.radius", src.pads.radius)?;
        positive("subject.radius", src.subject.radius)?;
        finite(
            "spawn",
            &[src.spawn.at[0], src.spawn.at[1], src.spawn.facing],
        )?;
        finite("subject.at", &src.subject.at)?;
        finite(
            "pads",
            &[
                src.pads.first[0],
                src.pads.first[1],
                src.pads.step[0],
                src.pads.step[1],
            ],
        )?;

        // The furniture, relative to a bay's origin.
        let shapes = src
            .references
            .iter()
            .map(|r| shape(r, &player))
            .collect::<Result<Vec<_>>>()?;
        let mut names: Vec<&str> = Vec::new();
        for r in &src.references {
            if names.contains(&r.name.as_str()) {
                return Err(Error::Layout(format!(
                    "two references are named {:?}",
                    r.name
                )));
            }
            names.push(&r.name);
        }
        let mut eye_names: Vec<String> = Vec::new();
        for e in &src.cameras.eyes {
            finite(&format!("camera {}", e.name), &e.at)?;
            eye_names.push(e.name.clone());
        }
        for k in 0..src.cameras.orbit.count {
            eye_names.push(format!("orbit-{k}"));
        }
        for (i, n) in eye_names.iter().enumerate() {
            if eye_names[..i].contains(n) {
                return Err(Error::Layout(format!("two cameras are named {n:?}")));
            }
        }

        let mut bays = Vec::with_capacity(src.bays.len());
        for b in &src.bays {
            finite(&format!("bay {}", b.name), &b.origin)?;
            if let Some(e) = &b.enclosure {
                positive(&format!("bay {}: enclosure.height", b.name), e.height)?;
                positive(&format!("bay {}: enclosure.thickness", b.name), e.thickness)?;
            }
            if bays.iter().any(|o: &Bay| o.name == b.name) {
                return Err(Error::Layout(format!("two bays are named {:?}", b.name)));
            }
            let o = b.origin;
            let at = |p: [f32; 2]| add(o, [p[0], p[1], 0.0]);
            let subject = Pad {
                number: 0,
                point: at(src.subject.at),
                radius: src.subject.radius,
            };
            let pads = (0..src.pads.count)
                .map(|k| Pad {
                    number: k + 1,
                    point: at([
                        src.pads.first[0] + src.pads.step[0] * k as f32,
                        src.pads.first[1] + src.pads.step[1] * k as f32,
                    ]),
                    radius: src.pads.radius,
                })
                .collect();
            let references = src
                .references
                .iter()
                .zip(&shapes)
                .map(|(r, s)| {
                    let point = at(r.at);
                    let facing = r.facing.to_radians();
                    let bounds = crate::stage::mesh::shape_bounds(s, point, facing);
                    Reference {
                        name: r.name.clone(),
                        shape: *s,
                        point,
                        facing,
                        bounds,
                    }
                })
                .collect();
            let target = add(subject.point, [0.0, 0.0, cams.aim]);
            let mut cameras = Vec::new();
            for e in &cams.eyes {
                cameras.push(look_at(&e.name, add(subject.point, e.at), target, cams.fov));
            }
            let n = cams.orbit.count;
            for k in 0..n {
                let a = (k as f32) * std::f32::consts::TAU / n as f32;
                let eye = add(
                    subject.point,
                    [
                        cams.orbit.radius * a.cos(),
                        cams.orbit.radius * a.sin(),
                        cams.orbit.height,
                    ],
                );
                cameras.push(look_at(&format!("orbit-{k}"), eye, target, cams.fov));
            }
            bays.push(Bay {
                name: b.name.clone(),
                origin: o,
                light: b.light,
                enclosure: b.enclosure,
                spawn: Spawn {
                    point: at(src.spawn.at),
                    facing: src.spawn.facing.to_radians(),
                },
                subject,
                pads,
                references,
                cameras,
            });
        }
        let layout = Layout {
            version: crate::stage::version(),
            player,
            floor,
            lights: src.lights,
            bays,
        };
        layout.check()?;
        Ok(layout)
    }

    /// The floor square of `bay`, with its enclosure's walls.
    pub fn bay_rect(&self, bay: &Bay) -> [f32; 4] {
        let h = self.floor.size / 2.0 + bay.enclosure.map_or(0.0, |e| e.thickness);
        square(bay.origin, h)
    }

    /// The checks [`Self::parse`] runs (module docs).
    pub fn check(&self) -> Result<()> {
        for (i, a) in self.bays.iter().enumerate() {
            for b in &self.bays[..i] {
                if overlaps(&self.bay_rect(a), &self.bay_rect(b)) {
                    return Err(Error::Overlap {
                        bay: a.name.clone(),
                        a: format!("bay {}", a.name),
                        b: format!("bay {}", b.name),
                    });
                }
            }
        }
        for bay in &self.bays {
            let floor = square(bay.origin, self.floor.size / 2.0);
            let mut taken: Vec<(String, Rect)> = Vec::new();
            taken.push((
                "the spawn".into(),
                square(bay.spawn.point, self.player.radius),
            ));
            taken.push((
                "the subject pad".into(),
                square(bay.subject.point, bay.subject.radius),
            ));
            for p in &bay.pads {
                taken.push((format!("pad {}", p.number), square(p.point, p.radius)));
            }
            for r in &bay.references {
                let b = r.bounds;
                taken.push((format!("reference {}", r.name), [b[0], b[1], b[2], b[3]]));
            }
            for (i, (name, rect)) in taken.iter().enumerate() {
                if !inside(rect, &floor) {
                    return Err(Error::Layout(format!(
                        "bay {}: {name} is not on the floor",
                        bay.name
                    )));
                }
                for (other, o) in &taken[..i] {
                    if overlaps(rect, o) {
                        return Err(Error::Overlap {
                            bay: bay.name.clone(),
                            a: name.clone(),
                            b: other.clone(),
                        });
                    }
                }
            }
            let top = bay
                .enclosure
                .map_or(f32::INFINITY, |e| bay.origin[2] + e.height);
            for c in &bay.cameras {
                if c.eye[2] <= bay.origin[2] || c.eye[2] >= top {
                    return Err(Error::Layout(format!(
                        "bay {}: camera {} is not between the floor and the roof",
                        bay.name, c.name
                    )));
                }
                let flat = [c.eye[0], c.eye[0], c.eye[1], c.eye[1]];
                if !inside(&flat, &floor) {
                    return Err(Error::Layout(format!(
                        "bay {}: camera {} is not over the floor",
                        bay.name, c.name
                    )));
                }
                for r in &bay.references {
                    let b = r.bounds;
                    let within = (0..3).all(|a| c.eye[a] > b[2 * a] && c.eye[a] < b[2 * a + 1]);
                    if within {
                        return Err(Error::Layout(format!(
                            "bay {}: camera {} is inside reference {}",
                            bay.name, c.name, r.name
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Where content under test stands in the bay `name`: its subject
    /// pad, and the way a thing there faces the player's spawn.
    pub fn subject_place(&self, name: &str) -> Result<SubjectPlace> {
        let bay = self
            .bays
            .iter()
            .find(|b| b.name == name)
            .ok_or_else(|| Error::Layout(format!("no bay named {name}")))?;
        Ok(SubjectPlace {
            bay: bay.name.clone(),
            point: bay.subject.point,
            facing: bay.spawn.facing + std::f32::consts::PI,
        })
    }

    /// Every bay's camera with this name, in bay order.
    pub fn cameras<'a>(&'a self, name: &'a str) -> impl Iterator<Item = (&'a Bay, &'a Camera)> {
        self.bays.iter().flat_map(move |b| {
            b.cameras
                .iter()
                .filter(move |c| c.name == name)
                .map(move |c| (b, c))
        })
    }
}

fn shape(r: &RefSource, player: &Player) -> Result<Shape> {
    let what = format!("reference {}", r.name);
    finite(&what, &[r.at[0], r.at[1], r.facing])?;
    let given = [
        r.cuboid.is_some(),
        r.doorway.is_some(),
        r.ramp.is_some(),
        r.stair.is_some(),
    ];
    if given.iter().filter(|&&g| g).count() != 1 {
        return Err(Error::Layout(format!(
            "{what}: give exactly one of box, doorway, ramp and stair"
        )));
    }
    let s = if let Some(b) = &r.cuboid {
        let size = [
            b.size[0].resolve(player, &what)?,
            b.size[1].resolve(player, &what)?,
            b.size[2].resolve(player, &what)?,
        ];
        for v in size {
            positive(&what, v)?;
        }
        Shape::Box { size }
    } else if let Some(d) = &r.doorway {
        let opening = [
            d.opening[0].resolve(player, &what)?,
            d.opening[1].resolve(player, &what)?,
        ];
        for v in [opening[0], opening[1], d.post, d.depth] {
            positive(&what, v)?;
        }
        Shape::Doorway {
            opening,
            post: d.post,
            depth: d.depth,
        }
    } else if let Some(p) = &r.ramp {
        positive(&what, p.width)?;
        positive(&what, p.rise)?;
        if !(p.angle > 0.0 && p.angle < 90.0) {
            return Err(Error::Layout(format!(
                "{what}: a ramp of {} degrees",
                p.angle
            )));
        }
        Shape::Ramp {
            width: p.width,
            rise: p.rise,
            angle: p.angle,
        }
    } else if let Some(s) = &r.stair {
        positive(&what, s.width)?;
        positive(&what, s.rise)?;
        positive(&what, s.tread)?;
        if s.steps == 0 {
            return Err(Error::Layout(format!("{what}: a stair of no steps")));
        }
        Shape::Stair {
            width: s.width,
            rise: s.rise,
            tread: s.tread,
            steps: s.steps,
        }
    } else {
        unreachable!("one shape is given")
    };
    Ok(s)
}

fn look_at(name: &str, eye: [f32; 3], target: [f32; 3], fov: f32) -> Camera {
    let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
    Camera {
        name: name.to_owned(),
        eye,
        target,
        yaw: d[1].atan2(d[0]),
        pitch: d[2].atan2(flat),
        fov,
    }
}
