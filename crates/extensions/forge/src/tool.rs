//! One local player's Objects and Links tools. Only confirming a preview or
//! a link's second end produces an edit.
use std::sync::Arc;

use crate::{BlockKind, Frame, Linkable, Session, camera::Camera};
use forge_doc::OpId;

/// Full stick deflection, in radians per second for look or object rotation.
pub const TURN_RATE: f32 = 2.4;
/// Full stick deflection while manipulating, in world units per second that
/// a held or new object is pushed away or pulled in. It is never pushed past
/// a nearer wall nor pulled nearer than [`Extent::near`].
pub const PUSH_RATE: f32 = 6.0;
/// The furthest an object is carried, in world units.
pub const MAX_HOLD: f32 = 80.0;

/// A new object is carried this many of its reaches ([`Extent::reach`])
/// in front of the camera, as Halo 3's Forge holds an object: whole in
/// view, with the world around it.
pub const HOLD_RADII: f32 = 2.5;
/// The nearest a held object is carried, in world units.
pub const MIN_HOLD: f32 = 1.0;
/// The gap kept between the camera and a held object's bounding sphere,
/// in world units, so the near plane never cuts it.
pub const NEAR_GAP: f32 = 0.25;

/// A palette item's bounding sphere, from its object definition: the
/// radius and the centre's offset from the object's origin in its own
/// axes, in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub radius: f32,
    pub offset: [f32; 3],
}

impl Default for Extent {
    /// What a tool with no definition data carries: a half-unit sphere.
    fn default() -> Self {
        Self {
            radius: 0.5,
            offset: [0.; 3],
        }
    }
}

impl Extent {
    /// The furthest the sphere reaches from the object's origin.
    pub fn reach(self) -> f32 {
        self.radius + self.offset.iter().map(|v| v * v).sum::<f32>().sqrt()
    }

    /// How far in front of the camera the object is carried.
    pub fn hold(self) -> f32 {
        (HOLD_RADII * self.reach()).max(MIN_HOLD)
    }

    /// The nearest the object may come, so the camera stays outside it.
    pub fn near(self) -> f32 {
        (self.reach() + NEAR_GAP).min(self.hold())
    }
}

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
    /// Switch to the Links tool, or start or cancel a link there.
    Link,
    /// Open the Tools picker.
    Tools,
    /// Open the Tool settings.
    Settings,
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
            "link" => Self::Link,
            "tools" => Self::Tools,
            "settings" => Self::Settings,
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
    Link { from: Linkable, to: Linkable },
    Unlink { link: OpId },
    Undo,
    Redo,
    Palette,
    Properties,
    Menu,
    Tools,
    Settings,
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
    /// The Links tool is selected instead of the Objects tool.
    pub links: bool,
    /// What the Links tool aims at.
    pub aimed_link: Option<Linkable>,
    /// The first end of a link being made.
    pub link_from: Option<Linkable>,
    /// Degrees between turn snaps.
    rotation_step: f32,
    rotation: [f32; 3],
    /// Each palette item's bounding sphere, in palette order.
    extents: Arc<[Extent]>,
    /// Orientation on pickup. Snap the *delta* so picking up never changes orientation.
    initial_rotation: [f32; 3],
    /// Offset from the aim point along the camera's forward, right and up,
    /// so a held object turns with the view and keeps its place in it.
    offset: [f32; 3],
    /// How far in front of the camera the object is carried: its
    /// [`Extent::hold`] for a new one, where it was taken for a picked-up
    /// one, then pushed and pulled.
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
            links: false,
            aimed_link: None,
            link_from: None,
            rotation_step: 15.0,
            rotation: [0.; 3],
            extents: Arc::from(Vec::new()),
            initial_rotation: [0.; 3],
            offset: [0.; 3],
            distance: 4.0,
        }
    }
}

impl Tool {
    /// A tool whose palette items have these bounding spheres, in palette
    /// order. An item without one carries [`Extent::default`].
    pub fn with_extents(extents: Arc<[Extent]>) -> Self {
        Self {
            extents,
            ..Self::default()
        }
    }

    /// The bounding sphere of what is placed or held, if anything.
    pub fn extent(&self, session: &Session) -> Option<Extent> {
        let index = match (self.placing, self.held) {
            (Some(index), _) => index,
            (None, Some(id)) => {
                let obj = session.objects().iter().find(|o| o.id == id)?;
                session.palette().iter().position(|p| *p == obj.item)?
            }
            (None, None) => return None,
        };
        Some(self.extents.get(index).copied().unwrap_or_default())
    }

    pub fn select(&mut self, index: usize) {
        self.cancel();
        self.placing = Some(index);
        self.distance = self.extents.get(index).copied().unwrap_or_default().hold();
    }

    /// Chooses the tool: the Links tool, or the Objects tool.
    pub fn set_links(&mut self, links: bool) {
        if self.links != links {
            self.clear_selection();
            self.links = links;
        }
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
        self.link_from = None;
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
        self.aimed_link = None;
    }

    pub fn applied(&mut self, command: &Command) {
        if matches!(
            command,
            Command::Place { .. }
                | Command::Move { .. }
                | Command::Delete { .. }
                | Command::Link { .. }
                | Command::Unlink { .. }
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
        if self.links {
            return Ok(self.update_links(session, input));
        }
        if input.action == Some(A::Link) {
            self.clear_selection();
            self.links = true;
            return Ok(None);
        }
        let previewing = self.placing.is_some() || self.held.is_some();
        let extent = self.extent(session).unwrap_or_default();
        let mut hit = None;
        if let Some(camera) = &mut self.camera {
            let turning = previewing && input.rotating;
            // Manipulating: the left stick's forward axis pushes the object
            // away and pulls it in.
            let mut push = 0.;
            if turning {
                self.rotation[0] += input.look[0];
                self.rotation[1] += input.look[1];
                self.rotation[2] += input.roll;
                push = input.movement[1] * PUSH_RATE * input.seconds.clamp(0., 0.1);
            } else {
                camera.fly(input.movement, input.look, input.seconds.clamp(0., 0.1));
            }
            let surface = surface(*camera).filter(|d| d.is_finite() && *d >= 0.);
            let max_distance = surface.unwrap_or(MAX_HOLD).min(MAX_HOLD);
            let forward = camera.forward();
            hit = session
                .objects()
                .iter()
                .filter_map(|obj| {
                    aim(camera, forward, max_distance, obj.frame.position)
                        .map(|along| (obj.id, along))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            self.aimed = hit.map(|(id, _)| id);
            if previewing {
                // A new object is carried at its hold distance and a
                // picked-up one where it was taken, each pushed and pulled
                // from there, pulled in to a nearer wall but never nearer
                // than its own sphere allows, so it never fills the view.
                let near = extent.near();
                let carried = |d: f32| d.min(max_distance).max(near);
                // A push or pull moves it from where it shows, so pushing
                // stops at the wall rather than building up behind it, and
                // with no push a wall pulls it in only while it is there.
                if push != 0. {
                    self.distance = carried(carried(self.distance) + push);
                }
                let distance = carried(self.distance);
                let pos = camera.point(distance);
                let offset = camera.to_world(self.offset);
                let step = self.rotation_step.to_radians();
                self.ghost = Some(Frame {
                    position: std::array::from_fn(|i| pos[i] + offset[i]),
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
            // B with nothing held is nothing: the Forge menu is Start's.
            Some(A::Back) => None,
            Some(A::Tools) => Some(C::Tools),
            Some(A::Settings) => Some(C::Settings),
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
                    let near = session
                        .palette()
                        .iter()
                        .position(|p| *p == obj.item)
                        .and_then(|i| self.extents.get(i))
                        .copied()
                        .unwrap_or_default()
                        .near();
                    let distance = distance.max(near);
                    self.held = Some(id);
                    self.distance = distance;
                    let p = camera.point(distance);
                    self.offset =
                        camera.to_local(std::array::from_fn(|i| obj.frame.position[i] - p[i]));
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

    /// The Links tool: aim at a teleporter flag or a placed machine, A to
    /// start a link and A again on the other end to make it.
    fn update_links(&mut self, session: &Session, input: Input) -> Option<Command> {
        use Action as A;
        use Command as C;
        if let Some(camera) = &mut self.camera {
            camera.fly(input.movement, input.look, input.seconds.clamp(0., 0.1));
            let forward = camera.forward();
            let camera = *camera;
            let teleporters = session.teleporters();
            let flags = teleporters
                .iter()
                .map(|t| (Linkable::Teleporter(t.index), t.position));
            let machines = session
                .objects()
                .iter()
                .filter(|o| o.item.block == BlockKind::Machines)
                .map(|o| (Linkable::Object(o.id), o.frame.position));
            self.aimed_link = flags
                .chain(machines)
                .filter_map(|(thing, at)| aim(&camera, forward, 80., at).map(|d| (thing, d)))
                .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                .map(|(thing, _)| thing);
        }
        let aimed = self.aimed_link.filter(|&t| Some(t) != self.link_from);
        match input.action {
            Some(A::Link) if self.link_from.is_some() => {
                self.link_from = None;
                None
            }
            Some(A::Link) if aimed.is_some() => {
                self.link_from = aimed;
                None
            }
            Some(A::Link) | Some(A::Palette) => {
                let palette = input.action == Some(A::Palette);
                self.clear_selection();
                self.links = false;
                palette.then_some(C::Palette)
            }
            Some(A::Accept) => match (self.link_from, aimed) {
                (Some(from), Some(to)) => Some(C::Link { from, to }),
                (None, Some(thing)) => {
                    self.link_from = Some(thing);
                    None
                }
                _ => None,
            },
            Some(A::Back) if self.link_from.is_some() => {
                self.link_from = None;
                None
            }
            Some(A::Back) => None,
            Some(A::Tools) => Some(C::Tools),
            Some(A::Settings) => Some(C::Settings),
            Some(A::Delete) if self.link_from.is_none() => {
                let link = session.links_of(self.aimed_link?).last()?.id;
                Some(C::Unlink { link })
            }
            Some(A::Properties) if self.link_from.is_none() && self.aimed_link.is_some() => {
                Some(C::Properties)
            }
            Some(A::Menu) => Some(C::Menu),
            Some(A::Undo) => Some(C::Undo),
            Some(A::Redo) => Some(C::Redo),
            _ => None,
        }
    }
}

/// How far along the view `at` lies when it is under the reticle.
fn aim(camera: &Camera, forward: [f32; 3], max_distance: f32, at: [f32; 3]) -> Option<f32> {
    let d: [f32; 3] = std::array::from_fn(|i| at[i] - camera.position[i]);
    let along: f32 = (0..3).map(|i| d[i] * forward[i]).sum();
    if !(0.1..=max_distance).contains(&along) {
        return None;
    }
    let side: f32 = (0..3).map(|i| (d[i] - along * forward[i]).powi(2)).sum();
    // The aim target is at least a quarter world unit wide, then grows with
    // distance so it subtends the same small angle as the central reticle.
    (side <= (0.25 + along * 0.012).powi(2)).then_some(along)
}
