//! One local player's Objects tool. Only confirming a preview produces an edit.
use crate::{Frame, Session, camera::Camera};
use forge_doc::OpId;

/// Full stick deflection, in radians per second for look or object rotation.
pub const TURN_RATE: f32 = 2.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Accept,
    Back,
    Delete,
    Palette,
    Properties,
    Menu,
    Undo,
    Redo,
    Up,
    Down,
}

impl Action {
    /// The same gestures used by the controller and the session API.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "toggle" => Self::Toggle,
            "accept" => Self::Accept,
            "back" => Self::Back,
            "delete" => Self::Delete,
            "palette" => Self::Palette,
            "properties" => Self::Properties,
            "menu" => Self::Menu,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            "up" => Self::Up,
            "down" => Self::Down,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub action: Option<Action>,
    pub movement: [f32; 3],
    /// Angular deltas for this tick in radians, in the director's coordinate system.
    pub look: [f32; 2],
    pub roll: f32,
    pub rotating: bool,
    pub seconds: f32,
    pub dialog: bool,
    pub click: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Place { item: usize, frame: Frame },
    Move { id: OpId, frame: Frame },
    Delete { id: OpId },
    Undo,
    Redo,
    Palette,
    Properties,
    Menu,
    DialogUp,
    DialogDown,
    DialogAccept,
    DialogBack,
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub monitor: bool,
    pub camera: Option<Camera>,
    pub aimed: Option<OpId>,
    pub held: Option<OpId>,
    pub placing: Option<usize>,
    pub ghost: Option<Frame>,
    /// Degrees between turn snaps.
    rotation_step: f32,
    rotation: [f32; 3],
    /// Orientation on pickup. Snap the *delta* so picking up never changes orientation.
    initial_rotation: [f32; 3],
    /// Offset from the aim point. Preserves the exact position on pickup.
    offset: [f32; 3],
    distance: f32,
}

impl Default for Tool {
    fn default() -> Self {
        Self {
            monitor: false,
            camera: None,
            aimed: None,
            held: None,
            placing: None,
            ghost: None,
            rotation_step: 15.0,
            rotation: [0.; 3],
            initial_rotation: [0.; 3],
            offset: [0.; 3],
            distance: 4.0,
        }
    }
}

impl Tool {
    pub fn select(&mut self, index: usize) {
        self.cancel();
        self.placing = Some(index);
    }

    pub fn set_rotation_step(&mut self, degrees: f32) {
        if degrees.is_finite() && degrees > 0.0 {
            self.rotation_step = degrees;
        }
    }

    pub fn set_heading(&mut self, radians: f32) {
        if radians.is_finite() && (self.placing.is_some() || self.held.is_some()) {
            self.initial_rotation[0] = radians;
            self.rotation[0] = 0.;
            if let Some(ghost) = &mut self.ghost {
                ghost.rotation[0] = radians;
            }
        }
    }

    fn cancel(&mut self) {
        self.held = None;
        self.placing = None;
        self.ghost = None;
        self.initial_rotation = [0.; 3];
        self.rotation = [0.; 3];
        self.offset = [0.; 3];
        self.distance = 4.0;
    }

    /// A load or external edit can invalidate both the aimed ID and the held preview.
    pub fn clear_selection(&mut self) {
        self.cancel();
        self.aimed = None;
    }

    pub fn applied(&mut self, command: &Command) {
        if matches!(
            command,
            Command::Place { .. }
                | Command::Move { .. }
                | Command::Delete { .. }
                | Command::Undo
                | Command::Redo
        ) {
            self.clear_selection();
        }
    }

    pub fn update(
        &mut self,
        session: &Session,
        _tick: u32,
        input: Input,
    ) -> Result<Option<Command>, String> {
        self.update_with_surface(session, _tick, input, |_| None)
    }

    /// A read-only ray against the active structure BSP, traced from the updated camera.
    pub fn update_with_surface(
        &mut self,
        session: &Session,
        _tick: u32,
        input: Input,
        surface: impl FnOnce(Camera) -> Option<f32>,
    ) -> Result<Option<Command>, String> {
        use Action as A;
        use Command as C;
        if input.dialog {
            return Ok(match input.action {
                Some(A::Up) => Some(C::DialogUp),
                Some(A::Down) => Some(C::DialogDown),
                Some(A::Accept) => Some(C::DialogAccept),
                Some(A::Back) => Some(C::DialogBack),
                _ => None,
            });
        }
        if input.action == Some(A::Toggle) {
            self.monitor = !self.monitor;
            self.clear_selection();
            self.camera = None;
            return Ok(None);
        }
        if !self.monitor {
            return Ok(None);
        }
        let previewing = self.placing.is_some() || self.held.is_some();
        let mut hit = None;
        if let Some(camera) = &mut self.camera {
            let turning = previewing && input.rotating;
            if turning {
                self.rotation[0] += input.look[0];
                self.rotation[1] += input.look[1];
                self.rotation[2] += input.roll;
            } else {
                camera.fly(input.movement, input.look, input.seconds.clamp(0., 0.1));
            }
            let surface = surface(*camera).filter(|d| d.is_finite() && *d >= 0.);
            let max_distance = surface.unwrap_or(80.).min(80.);
            let forward = camera.forward();
            hit = session
                .objects()
                .iter()
                .filter_map(|obj| {
                    let d: [f32; 3] =
                        std::array::from_fn(|i| obj.frame.position[i] - camera.position[i]);
                    let along: f32 = (0..3).map(|i| d[i] * forward[i]).sum();
                    if !(0.1..=max_distance).contains(&along) {
                        return None;
                    }
                    let side: f32 = (0..3).map(|i| (d[i] - along * forward[i]).powi(2)).sum();
                    // The aim target is at least a quarter world unit wide, then grows with
                    // distance so it subtends the same small angle as the central reticle.
                    (side <= (0.25 + along * 0.012).powi(2)).then_some((obj.id, along))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            self.aimed = hit.map(|(id, _)| id);
            if previewing {
                let distance = if self.placing.is_some() {
                    surface.unwrap_or(self.distance)
                } else {
                    self.distance.min(max_distance)
                };
                let pos = camera.point(distance);
                let step = self.rotation_step.to_radians();
                self.ghost = Some(Frame {
                    position: std::array::from_fn(|i| pos[i] + self.offset[i]),
                    rotation: std::array::from_fn(|i| {
                        self.initial_rotation[i] + (self.rotation[i] / step).round() * step
                    }),
                });
            }
        }
        Ok(match input.action {
            Some(A::Palette) => Some(C::Palette),
            Some(A::Properties) if self.aimed.is_some() || previewing => Some(C::Properties),
            Some(A::Menu) => Some(C::Menu),
            Some(A::Back) if previewing => {
                self.cancel();
                None
            }
            Some(A::Back) => Some(C::Menu),
            Some(A::Undo) => Some(C::Undo),
            Some(A::Redo) => Some(C::Redo),
            Some(A::Delete) => self.held.or(self.aimed).map(|id| C::Delete { id }),
            Some(A::Accept) => {
                if let (Some(item), Some(frame)) = (self.placing, self.ghost) {
                    Some(C::Place { item, frame })
                } else if let (Some(id), Some(frame)) = (self.held, self.ghost) {
                    Some(C::Move { id, frame })
                } else if let (Some((id, distance)), Some(camera)) = (hit, self.camera) {
                    let obj = session
                        .objects()
                        .iter()
                        .find(|o| o.id == id)
                        .ok_or("Object is no longer present")?;
                    self.held = Some(id);
                    self.distance = distance;
                    let p = camera.point(distance);
                    self.offset = std::array::from_fn(|i| obj.frame.position[i] - p[i]);
                    self.initial_rotation = obj.frame.rotation;
                    self.ghost = Some(obj.frame);
                    None
                } else {
                    None
                }
            }
            _ => None,
        })
    }
}
