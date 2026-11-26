//! A marker's two forms and its parts
//! (docs/game/gametype-marker-forms.md, "The parts of a marker"): the
//! edit form an author sees from the monitor, and the play form players
//! see in a match. The play form is a part of the edit form, at the same
//! place and size: [`crate::Kind::mesh_in`] draws one list of pieces and
//! leaves the edit-only ones out of a play form.
use crate::area::Area;

/// Which of a marker's forms to draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Form {
    /// What an author sees: every part.
    #[default]
    Edit,
    /// What a match draws: the parts a player walks into.
    Play,
}

impl Form {
    pub const ALL: [Form; 2] = [Form::Edit, Form::Play];

    /// The name the agent API and a render's file use.
    pub fn name(self) -> &'static str {
        match self {
            Form::Edit => "edit",
            Form::Play => "play",
        }
    }

    /// The form of a name: `edit` or `play`.
    pub fn of_name(name: &str) -> Option<Form> {
        Form::ALL.into_iter().find(|f| f.name() == name)
    }
}

/// How tall the post from the pad to the plate is, in world units: the
/// plate clears a player standing on the pad.
pub const POST: f32 = 1.0;
/// The radius of the plate an icon sits on.
pub const PLATE: f32 = 0.14;
/// How far over the top of a marker's body its plate's rim stays.
pub const PLATE_GAP: f32 = 0.1;
/// Half the side of the square an icon is drawn in on its plate: the
/// square's corners lie inside the plate's rim.
pub const ICON_HALF: f32 = PLATE * 0.65;
/// How far in front of its plate an icon is drawn, so the two do not
/// fight.
pub const ICON_LIFT: f32 = 0.004;
/// The distance out to which a plate keeps its size in the world; past
/// it the drawer scales the plate with the distance ([`icon_scale`]).
pub const ICON_NEAR: f32 = 8.0;
/// The alpha of a wall's foot.
pub const WALL_LOW: f32 = 0.30;
/// The alpha of a wall's top: faint, so it does not hide what is behind.
pub const WALL_HIGH: f32 = 0.04;
/// How strong Forge draws the edit-only parts beside the play form's,
/// so the part players will see stands out.
pub const EDIT_ALPHA: f32 = 0.6;
/// How high the rim of a capture plate and a plant zone stands.
pub const RIM: f32 = 0.1;
/// How tall the stub of a flag stand's pole is in play.
pub const STUB: f32 = 0.25;

/// The radius of the area a capture plate, a plant zone and a siege goal
/// have when none is stored ([`Area::plate`]).
pub const PLATE_RADIUS: f32 = 0.5;
/// How far that area reaches under its marker.
pub const PLATE_BELOW: f32 = 0.1;
/// How far that area reaches over its marker.
pub const PLATE_ABOVE: f32 = 1.0;

impl Area {
    /// The area of a flag return, a bomb goal or a siege goal nobody has
    /// shaped: a radius of [`PLATE_RADIUS`], [`PLATE_BELOW`] under the
    /// marker and [`PLATE_ABOVE`] over it.
    pub fn plate() -> Self {
        Self {
            below: PLATE_BELOW,
            above: PLATE_ABOVE,
            ..Self::radius(PLATE_RADIUS)
        }
    }
}

/// How much the drawer scales a plate and its icon `distance` world
/// units from the eye: 1 out to [`ICON_NEAR`], then in step with the
/// distance, so the icon keeps its size on screen.
pub fn icon_scale(distance: f32) -> f32 {
    if distance.is_finite() {
        (distance / ICON_NEAR).max(1.0)
    } else {
        1.0
    }
}

/// One part of a marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// A ring and a thin disc on the floor.
    Pad,
    /// The kind's own hologram.
    Body,
    /// The upright line from the pad to the plate.
    Post,
    /// The disc with the icon, turned to the eye.
    Plate,
    /// The arrow on the floor the way a player, a vehicle or a wave
    /// faces.
    Facing,
    /// The area's wall.
    Boundary,
    /// The text over the anchor.
    Label,
}

impl Part {
    pub const ALL: [Part; 7] = [
        Part::Pad,
        Part::Body,
        Part::Post,
        Part::Plate,
        Part::Facing,
        Part::Boundary,
        Part::Label,
    ];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The parts a kind draws in one form ([`crate::Kind::parts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Parts(u8);

impl Parts {
    pub fn has(self, part: Part) -> bool {
        self.0 & part.bit() != 0
    }

    pub fn insert(&mut self, part: Part) {
        self.0 |= part.bit();
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The parts, in [`Part::ALL`]'s order.
    pub fn iter(self) -> impl Iterator<Item = Part> {
        Part::ALL.into_iter().filter(move |p| self.has(*p))
    }

    /// Whether every part of `self` is one of `other`'s.
    pub fn within(self, other: Parts) -> bool {
        self.0 & !other.0 == 0
    }
}

impl FromIterator<Part> for Parts {
    fn from_iter<I: IntoIterator<Item = Part>>(iter: I) -> Self {
        let mut parts = Parts::default();
        for p in iter {
            parts.insert(p);
        }
        parts
    }
}
