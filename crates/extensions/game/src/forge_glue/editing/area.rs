//! One area in hand: a circle, a box, a sphere or a path
//! (docs/game/gametype-marker-forms.md, "Interfaces"). Y cycles the four
//! shapes; only accepting the draft writes to the Forge document.
use forge::camera::Camera;
use forge::tool::{Action, Input};
use forge::{BlockKind, Thing};
use forge_doc::Endpoint;
use game_markers::area::{DEFAULT_ABOVE, DEFAULT_BELOW, MAX_REACH, MIN_POINTS, MIN_RADIUS};
use game_markers::{Area, Shape};
use serde_json::{Value, json};

use super::{After, Editor, Kind, screens, sheet};
use crate::forge_controls::AreaMode;
use sheet::AreaField;

/// Minimum upward normal of a surface accepted as a floor.
pub(super) const FLOOR_NORMAL: f32 = 0.5;
const POINT_AIM: f32 = 0.09;
const DRAG_THRESHOLD: f32 = 0.025;
const STICK_REPEAT: f32 = 0.18;
/// How far a stick axis leans before it steps a length.
const STICK_DEAD: f32 = 0.12;
const NO_AREA: &str = "Aim at a goal with an area: a hill, territory, site, flag return or bomb goal.";

/// The area a flag of label `text` starts with when it has none stored:
/// a hill's, a territory's and a site's default circle, the capture plate
/// of a flag return, a bomb goal and a siege goal ([`Area::plate`]). `None`
/// for a label whose marker has no area.
pub(super) fn default_area(text: &str) -> Option<Area> {
    if text == "hill_corner" {
        return Some(Area::default());
    }
    game_markers::Kind::of_label(text)?.area().cloned()
}

#[derive(Clone, Copy)]
struct Drag {
    index: usize,
    grabbed: [f32; 2],
    vertex: [f32; 2],
    moved: bool,
}

pub(super) struct Draft {
    pub id: Endpoint,
    pub centre: [f32; 3],
    /// The marker's facing, which a box turns with.
    pub yaw: f32,
    pub area: Area,
    /// The radius a path or a box left, for the shape after it.
    radius: f32,
    /// The heights a sphere set aside, for the shape after it.
    heights: (f32, f32),
    /// Each stick axis's lean, -1, 0 or 1: right, then forward.
    stick: [i8; 2],
    repeat_in: [f32; 2],
    drag: Option<Drag>,
}

pub(super) enum Next {
    Keep,
    Cancel,
    Save,
    Dimensions,
}

impl Draft {
    fn new(id: Endpoint, centre: [f32; 3], yaw: f32, area: Area) -> Self {
        let radius = match area.shape {
            Shape::Radius(r) | Shape::Sphere(r) => r,
            _ => area.equal_floor_radius().unwrap_or(game_markers::area::DEFAULT_RADIUS),
        };
        let heights = match area.shape {
            Shape::Sphere(_) => (DEFAULT_BELOW, DEFAULT_ABOVE),
            _ => (area.below, area.above),
        };
        Self {
            id,
            centre,
            yaw,
            area,
            radius,
            heights,
            stick: [0; 2],
            repeat_in: [0.; 2],
            drag: None,
        }
    }

    pub fn mode(&self) -> AreaMode {
        match self.area.shape {
            // A box and a sphere take the stick as a radius does.
            Shape::Radius(_) | Shape::Box { .. } | Shape::Sphere(_) => AreaMode::Radius,
            Shape::Path(_) => AreaMode::Path,
        }
    }

    pub fn status(&self) -> Value {
        let shape = match &self.area.shape {
            Shape::Radius(radius) => json!({"shape":"radius","radius":radius}),
            Shape::Path(points) => json!({"shape":"path","points":points}),
            Shape::Box { half } => json!({"shape":"box","half":half}),
            Shape::Sphere(radius) => json!({"shape":"sphere","radius":radius}),
        };
        let mut state = shape;
        state["target"] = json!(self.id);
        state["below"] = json!(self.area.below);
        state["above"] = json!(self.area.above);
        state
    }

    fn change(&mut self, edit: impl FnOnce(&mut Area)) -> Result<(), String> {
        let mut candidate = self.area.clone();
        edit(&mut candidate);
        candidate
            .validate_draft()
            .map_err(|error| error.to_string())?;
        self.area = candidate;
        Ok(())
    }

    fn number(&mut self, field: AreaField, text: &str) -> Result<(), String> {
        let value = text.parse::<f32>().map_err(|_| "Type a number.")?;
        let sphere = matches!(self.area.shape, Shape::Sphere(_));
        if sphere && matches!(field, AreaField::Below | AreaField::Above) {
            return Err("A sphere has no height: its radius is all of it.".into());
        }
        self.change(|area| match (field, &mut area.shape) {
            (AreaField::Radius, Shape::Sphere(r)) => *r = value,
            (AreaField::Radius, shape) => *shape = Shape::Radius(value),
            (AreaField::Length, Shape::Box { half }) => half[0] = value,
            (AreaField::Width, Shape::Box { half }) => half[1] = value,
            (AreaField::Length | AreaField::Width, shape) => {
                *shape = Shape::Box { half: [value; 2] }
            }
            (AreaField::Below, _) => area.below = value,
            (AreaField::Above, _) => area.above = value,
        })
    }

    /// Y: a circle becomes a box of its radius each way, a box a sphere
    /// of the same floor, a sphere an empty path, and a path a circle of
    /// the same floor (or the radius from before it, with fewer than
    /// three points). A sphere sets the heights aside and the shape after
    /// it takes them back.
    fn switch_shape(&mut self) -> Result<(), String> {
        let (below, above) = self.heights;
        match self.area.shape.clone() {
            Shape::Radius(radius) => {
                self.radius = radius;
                self.change(|area| area.shape = Shape::Box { half: [radius; 2] })?;
            }
            Shape::Box { .. } => {
                let radius = self
                    .area
                    .equal_floor_radius()
                    .unwrap_or(self.radius)
                    .clamp(MIN_RADIUS, MAX_REACH);
                self.heights = (self.area.below, self.area.above);
                self.change(|area| *area = Area::sphere(radius))?;
            }
            Shape::Sphere(radius) => {
                self.radius = radius;
                self.area = Area {
                    shape: Shape::Path(Vec::new()),
                    below,
                    above,
                };
            }
            Shape::Path(_) => {
                if self.points().len() >= MIN_POINTS {
                    self.area.validate().map_err(|error| error.to_string())?;
                }
                let radius = self.area.equal_floor_radius().unwrap_or(self.radius);
                self.change(|area| area.shape = Shape::Radius(radius))?;
            }
        }
        self.stick = [0; 2];
        self.drag = None;
        Ok(())
    }

    /// The step stick axis `i` (0 right, 1 forward) makes this sample: its
    /// sign when it leans anew, or again every [`STICK_REPEAT`] seconds
    /// while it is held; 0 otherwise.
    fn lean(&mut self, i: usize, axis: f32, seconds: f32) -> f32 {
        let stick = if axis.abs() > STICK_DEAD {
            axis.signum() as i8
        } else {
            0
        };
        let step = stick != 0 && (stick != self.stick[i] || self.repeat_in[i] <= seconds);
        self.stick[i] = stick;
        if step {
            self.repeat_in[i] = STICK_REPEAT;
            f32::from(stick)
        } else {
            self.repeat_in[i] -= seconds;
            0.
        }
    }

    /// The left stick grows a circle and a sphere on its forward axis,
    /// and a box on both. A box turns with its marker, so each stick axis
    /// steps the box's axis nearer to it as the monitor `look`s (its yaw):
    /// forward the half length along the marker's facing and right the
    /// half width while the monitor looks more along the facing than
    /// across it, the other way round while it looks across. Each lean is
    /// one `step`.
    fn grow(
        &mut self,
        movement: [f32; 3],
        step: f32,
        seconds: f32,
        look: Option<f32>,
    ) -> Result<(), String> {
        let forward = self.lean(1, movement[1], seconds) * step;
        match self.area.shape.clone() {
            Shape::Radius(r) if forward != 0. => {
                self.change(|area| area.shape = Shape::Radius(r + forward))
            }
            Shape::Sphere(r) if forward != 0. => {
                self.change(|area| area.shape = Shape::Sphere(r + forward))
            }
            Shape::Box { half } => {
                let right = self.lean(0, movement[0], seconds) * step;
                if forward == 0. && right == 0. {
                    return Ok(());
                }
                let across = look.is_some_and(|look| {
                    let (s, c) = (look - self.yaw).sin_cos();
                    s.abs() > c.abs()
                });
                let (length, width) = if across { (right, forward) } else { (forward, right) };
                self.change(|area| {
                    area.shape = Shape::Box {
                        half: [half[0] + length, half[1] + width],
                    }
                })
            }
            _ => Ok(()),
        }
    }

    fn path_input(
        &mut self,
        action: Option<Action>,
        floor: Option<[f32; 3]>,
    ) -> Result<Next, String> {
        use Action as A;
        let point = floor.map(|p| [p[0] - self.centre[0], p[1] - self.centre[1]]);
        if action == Some(A::Properties) {
            self.drag = None;
            if let Shape::Path(points) = &mut self.area.shape {
                points.pop();
            }
            return Ok(Next::Keep);
        }
        if action == Some(A::AreaClose) {
            return Ok(Next::Save);
        }
        if let Some(mut drag) = self.drag {
            let released = action == Some(A::AreaRelease);
            if released {
                self.drag = None;
            }
            if let Some(point) = point {
                let offset = [point[0] - drag.grabbed[0], point[1] - drag.grabbed[1]];
                drag.moved |= offset[0].hypot(offset[1]) > DRAG_THRESHOLD;
                if !released {
                    self.drag = Some(drag);
                }
                if drag.moved {
                    self.change(|area| {
                        if let Shape::Path(points) = &mut area.shape {
                            points[drag.index] =
                                [drag.vertex[0] + offset[0], drag.vertex[1] + offset[1]];
                        }
                    })?;
                } else if released && drag.index == 0 && self.points().len() >= MIN_POINTS {
                    return Ok(Next::Save);
                }
            }
            return Ok(Next::Keep);
        }
        if !matches!(action, Some(A::Accept | A::AreaGrab)) {
            return Ok(Next::Keep);
        }
        let point = point.ok_or("Aim at the floor to place a point.")?;
        let aimed = self
            .points()
            .iter()
            .enumerate()
            .filter(|(_, p)| distance(**p, point) <= POINT_AIM)
            .min_by(|(_, a), (_, b)| distance(**a, point).total_cmp(&distance(**b, point)));
        if let Some((index, &vertex)) = aimed {
            if action == Some(A::AreaGrab) {
                self.drag = Some(Drag {
                    index,
                    grabbed: point,
                    vertex,
                    moved: false,
                });
            } else if index == 0 && self.points().len() >= MIN_POINTS {
                return Ok(Next::Save);
            }
        } else {
            self.change(|area| {
                if let Shape::Path(points) = &mut area.shape {
                    points.push(point);
                }
            })?;
        }
        Ok(Next::Keep)
    }

    pub(super) fn input(
        &mut self,
        camera: &mut Option<Camera>,
        input: Input,
        step: f32,
        surface: impl FnOnce(Camera) -> Option<(f32, [f32; 3])>,
    ) -> Result<Next, String> {
        use Action as A;
        if input.dialog {
            return Ok(Next::Keep);
        }
        let seconds = input.seconds.clamp(0., 0.1);
        match input.action {
            Some(A::Back) => Ok(Next::Cancel),
            Some(A::AreaShape | A::Delete) => {
                self.switch_shape()?;
                Ok(Next::Keep)
            }
            action => match self.area.shape {
                Shape::Radius(_) | Shape::Box { .. } | Shape::Sphere(_) => {
                    let step = if step > 0. { step } else { 0.25 };
                    let look = camera.as_ref().map(|camera| camera.yaw);
                    self.grow(input.movement, step, seconds, look)?;
                    // The axes that size the shape do not fly the monitor.
                    let right = match self.area.shape {
                        Shape::Box { .. } => 0.,
                        _ => input.movement[0],
                    };
                    if let Some(camera) = camera {
                        camera.fly([right, 0., input.movement[2]], input.look, seconds);
                    }
                    Ok(match action {
                        Some(A::Accept) => Next::Save,
                        Some(A::Properties) => Next::Dimensions,
                        _ => Next::Keep,
                    })
                }
                Shape::Path(_) => {
                    let floor = camera.as_mut().and_then(|camera| {
                        camera.fly(input.movement, input.look, seconds);
                        let (distance, normal) = surface(*camera)?;
                        (distance.is_finite() && distance >= 0. && normal[2] >= FLOOR_NORMAL)
                            .then(|| camera.point(distance))
                    });
                    self.path_input(action, floor)
                }
            },
        }
    }

    fn points(&self) -> &[[f32; 2]] {
        match &self.area.shape {
            Shape::Path(points) => points,
            Shape::Radius(_) | Shape::Box { .. } | Shape::Sphere(_) => &[],
        }
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

impl Editor {
    /// The area a goal's Edit area starts from when it has none stored;
    /// `None` for a thing that has no area.
    pub(super) fn area_default(&self, thing: &Thing) -> Option<Area> {
        let labels = self.session.labels().ok()?;
        default_area(&forge::flag_label(thing, &labels)?.text)
    }

    pub(super) fn begin_area(&mut self, id: Endpoint) -> Result<(), String> {
        let thing = self.session.thing(id).ok_or("The goal is gone.")?;
        let labels = self.session.labels()?;
        let label = forge::flag_label(&thing, &labels).ok_or(NO_AREA)?;
        let default = default_area(&label.text).ok_or(NO_AREA)?;
        if label.text == "hill_corner"
            && self
                .session
                .things()
                .iter()
                .filter(|t| {
                    t.block == BlockKind::NetgameFlags
                        && forge::flag_label(t, &labels).as_ref() == Some(&label)
                })
                .count()
                >= MIN_POINTS
        {
            return Err("a hill with corners takes no area".into());
        }
        let areas = self.session.areas()?;
        let area = areas
            .get(&id.identity())
            .map(forge_doc::Area::marker_area)
            .unwrap_or(default);
        self.tool.clear_selection();
        self.area = Some(Draft::new(
            thing.id,
            thing.frame.position,
            thing.frame.rotation[0],
            area,
        ));
        Ok(())
    }

    pub(super) fn area_number(&mut self, field: AreaField, text: &str) -> Result<(), String> {
        let draft = self.area.as_mut().ok_or("Edit area is not open.")?;
        draft.number(field, text)?;
        screens::screen(Kind::Properties).show(sheet::area_fields(&draft.area))
    }

    pub(super) fn apply_area_input(&mut self, tick: u32, next: Next) -> Result<(), String> {
        let draft = self.area.as_ref().expect("area input needs a draft");
        match next {
            Next::Keep => Ok(()),
            Next::Dimensions => screens::open(sheet::area_fields(&draft.area)),
            Next::Cancel => {
                self.area = None;
                Ok(())
            }
            Next::Save => {
                draft.area.validate().map_err(|error| error.to_string())?;
                let (id, area) = (draft.id, draft.area.clone());
                self.stage(After::Area, |s, w| s.set_area(w, tick, id, Some(area)))
            }
        }
    }
}
