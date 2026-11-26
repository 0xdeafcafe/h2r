//! One area in hand. Only accepting it writes to the Forge document.
use forge::camera::Camera;
use forge::tool::{Action, Input};
use forge::{BlockKind, Goal, Thing};
use forge_doc::Endpoint;
use game_markers::{Area, Shape, area::MIN_POINTS};
use serde_json::{Value, json};

use super::{After, Editor, Kind, screens, sheet};
use crate::forge_controls::AreaMode;
use sheet::AreaField;

/// Minimum upward normal of a surface accepted as a floor.
pub(super) const FLOOR_NORMAL: f32 = 0.5;
const POINT_AIM: f32 = 0.09;
const DRAG_THRESHOLD: f32 = 0.025;
const STICK_REPEAT: f32 = 0.18;
/// Said when a box or a sphere area is opened for editing: Edit area
/// handles a radius and a path until the four-shape editor lands.
const UNEDITABLE: &str = "A box or sphere area cannot be edited here yet.";

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
    pub area: Area,
    radius: f32,
    stick: i8,
    repeat_in: f32,
    drag: Option<Drag>,
}

pub(super) enum Next {
    Keep,
    Cancel,
    Save,
    Dimensions,
}

impl Draft {
    fn new(id: Endpoint, centre: [f32; 3], area: Area) -> Self {
        let radius = game_markers::area::DEFAULT_RADIUS;
        Self {
            id,
            centre,
            area,
            radius,
            stick: 0,
            repeat_in: 0.,
            drag: None,
        }
    }

    pub fn mode(&self) -> AreaMode {
        match self.area.shape {
            // A box or a sphere is refused before a draft is made
            // (`begin_area`), so these never reach a draft.
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
        self.change(|area| match field {
            AreaField::Radius => area.shape = Shape::Radius(value),
            AreaField::Below => area.below = value,
            AreaField::Above => area.above = value,
        })
    }

    fn switch_shape(&mut self) -> Result<(), String> {
        match self.area.shape {
            Shape::Box { .. } | Shape::Sphere(_) => return Err(UNEDITABLE.into()),
            Shape::Radius(radius) => {
                self.radius = radius;
                self.area.shape = Shape::Path(Vec::new());
            }
            Shape::Path(_) => {
                if self.points().len() >= MIN_POINTS {
                    self.area.validate().map_err(|error| error.to_string())?;
                }
                let radius = self.area.equal_floor_radius().unwrap_or(self.radius);
                self.change(|area| area.shape = Shape::Radius(radius))?;
            }
        }
        self.stick = 0;
        self.drag = None;
        Ok(())
    }

    fn nudge_radius(
        &mut self,
        radius: f32,
        axis: f32,
        step: f32,
        seconds: f32,
    ) -> Result<(), String> {
        let stick = if axis.abs() > 0.12 {
            axis.signum() as i8
        } else {
            0
        };
        let nudge = stick != 0 && (stick != self.stick || self.repeat_in <= seconds);
        self.stick = stick;
        if nudge {
            self.repeat_in = STICK_REPEAT;
            self.change(|area| area.shape = Shape::Radius(radius + f32::from(stick) * step))?;
        } else {
            self.repeat_in -= seconds;
        }
        Ok(())
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
                Shape::Box { .. } | Shape::Sphere(_) => Err(UNEDITABLE.into()),
                Shape::Radius(radius) => {
                    let step = if step > 0. { step } else { 0.25 };
                    self.nudge_radius(radius, input.movement[1], step, seconds)?;
                    if let Some(camera) = camera {
                        camera.fly(
                            [input.movement[0], 0., input.movement[2]],
                            input.look,
                            seconds,
                        );
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
    pub(super) fn area_goal(&self, thing: &Thing) -> Option<Goal> {
        let labels = self.session.labels().ok()?;
        Goal::of_label(&forge::flag_label(thing, &labels)?.text)
    }

    pub(super) fn begin_area(&mut self, id: Endpoint) -> Result<(), String> {
        let thing = self.session.thing(id).ok_or("The goal is gone.")?;
        let labels = self.session.labels()?;
        let label =
            forge::flag_label(&thing, &labels).ok_or("Aim at a hill, territory or site.")?;
        let goal = Goal::of_label(&label.text).ok_or("Aim at a hill, territory or site.")?;
        if goal == Goal::Hill
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
            .unwrap_or_default();
        if matches!(area.shape, Shape::Box { .. } | Shape::Sphere(_)) {
            return Err(UNEDITABLE.into());
        }
        self.tool.clear_selection();
        self.area = Some(Draft::new(thing.id, thing.frame.position, area));
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
