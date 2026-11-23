//! The element model (docs/game/hud-kit.md, "The model"): a [`Hud`] is a
//! name and a fixed list of [`Element`]s; a state of it is a [`Values`],
//! one [`Value`] per element. One layout per HUD: a state never adds,
//! removes or moves an element.

use crate::Mode;
use crate::motion::Motion;
use crate::world::{Pose, Shape};

/// A HUD of ours.
#[derive(Clone, Debug, PartialEq)]
pub struct Hud {
    pub name: &'static str,
    pub elements: Vec<Element>,
}

impl Hud {
    pub fn element(&self, id: &str) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// The model's own faults: an empty or repeated id.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, e) in self.elements.iter().enumerate() {
            if e.id.is_empty() {
                out.push(format!("element {i} has no id"));
            }
            if self.elements[..i].iter().any(|o| o.id == e.id) {
                out.push(format!("{} is named twice", e.id));
            }
        }
        out
    }

    /// Every [`Art`] the elements draw, once each (a navpoint's arrow
    /// included).
    pub fn arts(&self) -> Vec<Art> {
        let mut out: Vec<Art> = Vec::new();
        for a in self.elements.iter().filter_map(|e| match e.kind {
            Kind::Navpoint { .. } => Some(Art::NavpointArrow),
            _ => e.kind.art(),
        }) {
            if !out.contains(&a) {
                out.push(a);
            }
        }
        out
    }

    /// The [`Art`] the widget elements draw, once each: what the runtime
    /// compiles (the navpoint's arrow is the game's to draw).
    pub fn widget_arts(&self) -> Vec<Art> {
        let mut out: Vec<Art> = Vec::new();
        for a in self.elements.iter().filter_map(|e| e.kind.art()) {
            if !out.contains(&a) {
                out.push(a);
            }
        }
        out
    }

    /// Whether any element is a text widget (and needs the text template).
    pub fn has_text(&self) -> bool {
        self.elements
            .iter()
            .any(|e| matches!(e.kind, Kind::Text { .. }))
    }

    /// Whether any element is placed in the grenade row (and needs the
    /// row's template).
    pub fn uses_grenade_row(&self) -> bool {
        self.elements.iter().any(|e| {
            e.at.origin != Origin::Anchor || e.at.split_origin.is_some_and(|o| o != Origin::Anchor)
        })
    }
}

/// One element: its id (unique in its HUD), what it is, where it sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub id: &'static str,
    pub kind: Kind,
    /// For the channels ([`Kind::is_channel`]) only the screen modes it
    /// has count: the game places them.
    pub at: Place,
    /// The element of the same HUD this one is drawn on by design, as a
    /// grenade count is on its pill: the fit and own checks let the two
    /// overlap.
    pub over: Option<&'static str>,
    /// Bits of the game's HUD state word D that hide it, as a retail
    /// widget's `hide_when_d` (`nhdt` `+0x16`): while the game would draw
    /// a retail widget of its own in its place, ours gives way. The
    /// runtime's gate reads the word (E `0x0013ac87`, its third stack
    /// argument); the record's own words stay zero.
    pub hide_when_d: u16,
    /// HUD pixels it may sit outside the view's HUD frame, in the
    /// screen's margin: 0 for every element but one placed round retail
    /// art that is on the frame's edge itself (the motion tracker's rim).
    /// The frame check lets it out this far and no further, and never off
    /// the screen.
    pub outside: i16,
}

impl Element {
    pub fn text(
        id: &'static str,
        font: Font,
        justify: Justify,
        longest: &'static str,
        at: Place,
    ) -> Self {
        Element {
            id,
            kind: Kind::Text {
                font,
                justify,
                longest,
            },
            at,
            over: None,
            hide_when_d: 0,
            outside: 0,
        }
    }
    pub fn meter(id: &'static str, art: Art, at: Place) -> Self {
        Element {
            id,
            kind: Kind::Meter { art },
            at,
            over: None,
            hide_when_d: 0,
            outside: 0,
        }
    }
    pub fn icon(id: &'static str, art: Art, at: Place) -> Self {
        Element {
            id,
            kind: Kind::Icon { art },
            at,
            over: None,
            hide_when_d: 0,
            outside: 0,
        }
    }
    pub fn pointer(id: &'static str, art: Art, at: Place) -> Self {
        Element {
            id,
            kind: Kind::Pointer { art },
            at,
            over: None,
            hide_when_d: 0,
            outside: 0,
        }
    }
    /// A channel element: [`Kind::Message`], [`Kind::Prompt`],
    /// [`Kind::Training`], [`Kind::Navpoint`] or [`Kind::WorldMark`],
    /// fullscreen only ([`Place::GAME`]).
    pub fn channel(id: &'static str, kind: Kind) -> Self {
        debug_assert!(kind.is_channel());
        Element {
            id,
            kind,
            at: Place::GAME,
            over: None,
            hide_when_d: 0,
            outside: 0,
        }
    }
    /// Drawn on `id` by design (see [`Element::over`]).
    pub fn over(mut self, id: &'static str) -> Self {
        self.over = Some(id);
        self
    }
    /// Hidden while any of `bits` is set in the game's word D (see
    /// [`Element::hide_when_d`]).
    pub fn hidden_on_d(mut self, bits: u16) -> Self {
        self.hide_when_d = bits;
        self
    }
    /// Let `pixels` outside the HUD frame (see [`Element::outside`]).
    pub fn outside(mut self, pixels: i16) -> Self {
        self.outside = pixels;
        self
    }
}

/// What an element is.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A line in a HUD font. `longest` is the widest text any state gives it.
    Text {
        font: Font,
        justify: Justify,
        longest: &'static str,
    },
    /// A strip that fills from the left with [`Value::fill`], 0 to 1.
    Meter { art: Art },
    /// A sprite of a retail HUD bitmap.
    Icon { art: Art },
    /// An icon turned to [`Value::angle`] about its centre.
    Pointer { art: Art },
    /// A notification in the pickup list, top left, that fades (the
    /// notification queue, `0x0024cbbf`).
    Message { longest: &'static str },
    /// The prompt line, top left (the user's prompt fields, while the game
    /// shows no prompt of its own).
    Prompt { longest: &'static str },
    /// The framed box in the middle of the screen (the scripted training
    /// text).
    Training { longest: &'static str },
    /// A navpoint over a point of the world: the `hudg` waypoint marker
    /// and arrow (E `0x0024e59f`), with the value's text, a whole number,
    /// under them in [`NAVPOINT_FONT`]. Its screen point is the runtime's
    /// (the preview's is a sample).
    Navpoint { longest: &'static str },
    /// A mark lying on the level's geometry (`crate::world`): `shape`'s
    /// lines at the element's [`Pose`], drawn by the window's world-line
    /// pass under the HUD, with the value's text as a label over it in
    /// [`WORLD_FONT`]. Its pose is the runtime's, as a navpoint's point
    /// is (the preview's is the HUD's sample, [`Describe::world`]).
    /// `longest` is the widest label any state gives it, empty for a mark
    /// with none.
    WorldMark {
        shape: Shape,
        longest: &'static str,
    },
}

/// The font a world mark's label is in: the HUD font, engine font 6, at
/// the size the world labels use.
pub const WORLD_FONT: Font = Font::Hud;

/// The font a navpoint's number is in: the grenade counts' (font byte 0,
/// engine font 6).
pub const NAVPOINT_FONT: Font = Font::Hud;

/// The UTF-16 units of a notification the game keeps: the insert copies 63
/// (E `0x0024cbee`, `push 3Fh`). A [`Kind::Message`] text longer than
/// this is refused by the prompt check and by the runtime's
/// `Handle::notify`.
pub const MESSAGE_UNITS: usize = 63;

/// Why `text` cannot be a notification, if it cannot: more than
/// [`MESSAGE_UNITS`] UTF-16 units, or a line break (a notification is one
/// line).
pub fn message_fault(text: &str) -> Option<String> {
    let units = text.encode_utf16().count();
    if units > MESSAGE_UNITS {
        Some(format!("{units} characters, over {MESSAGE_UNITS}"))
    } else if text.contains(['\n', '\r']) {
        Some("more than one line".into())
    } else {
        None
    }
}

impl Kind {
    /// The name the JSON and the findings use.
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Text { .. } => "text",
            Kind::Meter { .. } => "meter",
            Kind::Icon { .. } => "icon",
            Kind::Pointer { .. } => "pointer",
            Kind::Message { .. } => "message",
            Kind::Prompt { .. } => "prompt",
            Kind::Training { .. } => "training",
            Kind::Navpoint { .. } => "navpoint",
            Kind::WorldMark { .. } => "world mark",
        }
    }
    /// A channel into the message or training parts, not a widget.
    pub fn is_channel(&self) -> bool {
        matches!(
            self,
            Kind::Message { .. }
                | Kind::Prompt { .. }
                | Kind::Training { .. }
                | Kind::Navpoint { .. }
                | Kind::WorldMark { .. }
        )
    }
    /// A mark on the level's geometry, and its shape.
    pub fn world(&self) -> Option<Shape> {
        match self {
            Kind::WorldMark { shape, .. } => Some(*shape),
            _ => None,
        }
    }
    /// A channel into the game's text lines: a message, the prompt or the
    /// training box (the prompt check's).
    pub fn is_lines(&self) -> bool {
        matches!(
            self,
            Kind::Message { .. } | Kind::Prompt { .. } | Kind::Training { .. }
        )
    }
    pub fn art(&self) -> Option<Art> {
        match self {
            Kind::Meter { art } | Kind::Icon { art } | Kind::Pointer { art } => Some(art.clone()),
            _ => None,
        }
    }
    pub fn longest(&self) -> Option<&'static str> {
        match self {
            Kind::Text { longest, .. }
            | Kind::Message { longest }
            | Kind::Prompt { longest }
            | Kind::Training { longest }
            | Kind::Navpoint { longest }
            | Kind::WorldMark { longest, .. } => Some(longest),
            _ => None,
        }
    }
}

/// A widget's anchor: a corner of the view's HUD frame (E `0x001396c7`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopRight = 0,
    TopLeft = 1,
    BottomLeft = 2,
    BottomRight = 3,
    Centre = 4,
}

/// Anchor and offsets in HUD pixels, per screen mode. `None` hides the
/// element in that mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// Unused with an [`Origin`] in the grenade row: the row's anchor.
    pub anchor: Anchor,
    pub full: (i16, i16),
    pub half: Option<(i16, i16)>,
    pub quarter: Option<(i16, i16)>,
    /// The point of the sprite (fractions of its size) put on the offset;
    /// unused by texts, whose point is their top.
    pub registration: [f32; 2],
    /// What the offsets are from.
    pub origin: Origin,
    /// What the half and quarter offsets are from, when it is not
    /// `origin`: an element beside the motion tracker fullscreen that
    /// sits in the grenade row in split screen. A widget has one anchor,
    /// so with a row origin in split screen `anchor` is the row's
    /// (`records::placed` refuses another).
    pub split_origin: Option<Origin>,
}

/// What a [`Place`]'s offsets are from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The anchor's corner of the HUD frame.
    Anchor,
    /// The top-left corner of the next pill of the player's grenade row:
    /// the frag pill's stepped on by the step from the plasma count to
    /// the frag count, in each screen mode (`templates::GrenadeRow`).
    NextGrenadePill,
    /// Where the next pill's count sits: the frag count's place stepped
    /// on by the same step.
    NextGrenadeCount,
}

impl Place {
    /// The place the channels carry: the game places them, fullscreen
    /// only (`Place::GAME.split((0, 0), (0, 0))` shows one in split screen).
    pub const GAME: Place = Place::new(Anchor::TopLeft, (0, 0));

    /// Fullscreen only, registered at the top left.
    pub const fn new(anchor: Anchor, full: (i16, i16)) -> Self {
        Place {
            anchor,
            full,
            half: None,
            quarter: None,
            registration: [0.0, 0.0],
            origin: Origin::Anchor,
            split_origin: None,
        }
    }
    /// Offsets from `origin` in the grenade row, in every screen mode the
    /// row has (the anchor is the row's).
    pub const fn in_grenade_row(origin: Origin) -> Self {
        Place::new(Anchor::TopLeft, (0, 0))
            .split((0, 0), (0, 0))
            .from(origin)
    }
    pub const fn from(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }
    /// The half and quarter offsets from `origin`, the fullscreen one
    /// as it was ([`Place::split_origin`]).
    pub const fn split_from(mut self, origin: Origin) -> Self {
        self.split_origin = Some(origin);
        self
    }
    /// What `mode`'s offset is from.
    pub fn origin_in(&self, mode: Mode) -> Origin {
        match (mode, self.split_origin) {
            (Mode::Half | Mode::Quarter, Some(o)) => o,
            _ => self.origin,
        }
    }
    pub const fn registered(mut self, registration: [f32; 2]) -> Self {
        self.registration = registration;
        self
    }
    pub const fn split(mut self, half: (i16, i16), quarter: (i16, i16)) -> Self {
        self.half = Some(half);
        self.quarter = Some(quarter);
        self
    }
    pub fn offset(&self, mode: Mode) -> Option<(i16, i16)> {
        match mode {
            Mode::Full => Some(self.full),
            Mode::Half => self.half,
            Mode::Quarter => self.quarter,
        }
    }
}

/// A text widget's font (its font byte `+0x30`: 0 engine font 6, 1 engine
/// font 8, E `0x0022a85b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// The HUD font, font id 6 (`conduit-13` on the disc).
    Hud,
    /// The count font, font id 8 (`MSLCD-14`).
    Lcd,
}

impl Font {
    /// The widget's font byte.
    pub fn byte(self) -> i8 {
        match self {
            Font::Hud => 0,
            Font::Lcd => 1,
        }
    }
    /// The engine font (`font_table.txt` line): font byte 0 is engine
    /// font 6, 1 is 8 (E `0x0022a85b`).
    pub fn engine(self) -> i16 {
        match self {
            Font::Hud => 6,
            Font::Lcd => 8,
        }
    }
}

/// A text's justification across its point (E `0x0022ac0b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Start = 0,
    Centre = 1,
    End = 2,
}

/// Retail art, by reference: never pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Art {
    /// The energy sword's battery strip and its meter shader.
    StripMeter,
    /// The strip's last `n` columns: [`Art::StripMeter`]'s template with a
    /// crop, and the blue ramp of those columns, so a fill of 0 to 1 lights
    /// the cropped part from its left end to its right
    /// (`templates::ArtTemplate::ramp`).
    StripMeterEnd(u8),
    /// `hudg` `arrow_assets[0]`: the waypoint arrow's image, shader and
    /// sequence.
    WaypointArrow,
    /// `hudg` `arrow_assets[hudg::NAVPOINT_ARROW]`: the arrow the game's
    /// navpoints draw.
    NavpointArrow,
    /// A part of the player's frag grenade pill: of the `nhdt`'s first
    /// bitmap widget shown on word A `0x04` alone
    /// (`templates::FRAG_SELECTED`), the frag pill's part of its sprite
    /// (`templates::GrenadeRow::crop`).
    GrenadePill(PillPart),
    /// A retail widget's bitmap, shader and sequences, by its name string.
    Widget { hud: HudRef, name: &'static str },
}

/// A part of a grenade pill, in pill pixels from its left edge. Retail's
/// pill (`ui\hud\masterchief`, 50 by 19) has its count at x 12 and its
/// "x" and grenade picture from x 22 (V: `grenade_bkd`, image 0): a body
/// of the first [`PILL_BODY`] pixels and the last [`PILL_END`] put side by
/// side make a pill with nothing baked in but its outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillPart {
    /// The pill's left end and body, x 0 to [`PILL_BODY`].
    Body,
    /// Its right end, the last [`PILL_END`] pixels.
    End,
}

/// The width of [`PillPart::Body`]: clear of the "x" at 22 and the seam.
pub const PILL_BODY: i16 = 20;
/// The width of [`PillPart::End`]: the right end's curve (V: `grenade_bkd`).
pub const PILL_END: i16 = 7;

/// Which retail `nhdt` an [`Art::Widget`] is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HudRef {
    /// The player's, the `nhdt` the kit's widgets are added to.
    Player,
    /// An `nhdt` by tag name.
    Named(&'static str),
}

/// How a text is coloured on top of the colour the game picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Normal,
    /// The game's alpha, scaled down.
    Dim,
    /// Greyed and a little faded: off, as an unselected grenade's pill.
    Inactive,
    /// Run through a `hudg` flash record.
    Flash(Pulse),
    /// The go green (`world::GO_GREEN`), for a world mark alone: "this
    /// will work", as the approach guide's will-bounce call. A
    /// widget or a text channel in it is a finding of the `world` check
    /// and is not drawn.
    Go,
    /// Team `n`'s colour (`0..world::TEAMS`), as the game gives it
    /// (docs/game/hud-kit.md, "Colours"), for a world mark alone: a
    /// widget or a text channel in it, or a team outside the game's, is a
    /// finding of the `world` check. Where the game has no colour for the
    /// team the mark is in the user's HUD colour.
    Team(u8),
}

impl Style {
    pub fn name(self) -> String {
        match self {
            Style::Normal => "normal".into(),
            Style::Dim => "dim".into(),
            Style::Inactive => "inactive".into(),
            Style::Flash(p) => format!("flash {}", p.name()),
            Style::Go => "go".into(),
            Style::Team(n) => format!("team {n}"),
        }
    }
}

/// The `hudg` flash records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pulse {
    Help,
    Objective,
    TimeWarning,
    TimeExpired,
}

impl Pulse {
    pub const ALL: [Pulse; 4] = [
        Pulse::Help,
        Pulse::Objective,
        Pulse::TimeWarning,
        Pulse::TimeExpired,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Pulse::Help => "help",
            Pulse::Objective => "objective",
            Pulse::TimeWarning => "time warning",
            Pulse::TimeExpired => "time expired",
        }
    }
}

/// One element's state.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Value {
    pub shown: bool,
    pub text: String,
    /// A meter's fill, 0 to 1.
    pub fill: f32,
    /// A pointer's turn, radians clockwise from straight up.
    pub angle: f32,
    pub style: Style,
    /// The tick the style started (a flash's phase).
    pub since_tick: u32,
    /// How the fill and visibility move after a change, on wall time
    /// (`crate::motion`).
    pub motion: Motion,
    /// The fill a [`Value::motion`] starts from: the runtime's, the fill
    /// shown when the value changed (`motion::restamp`). A HUD leaves it 0.
    pub from: f32,
}

impl Value {
    pub fn hidden() -> Self {
        Value::default()
    }
    pub fn text(text: impl Into<String>) -> Self {
        Value {
            shown: true,
            text: text.into(),
            ..Value::default()
        }
    }
    pub fn fill(fill: f32) -> Self {
        Value {
            shown: true,
            fill,
            ..Value::default()
        }
    }
    pub fn angle(angle: f32) -> Self {
        Value {
            shown: true,
            angle,
            ..Value::default()
        }
    }
    pub fn shown() -> Self {
        Value {
            shown: true,
            ..Value::default()
        }
    }
    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
    pub fn moving(mut self, motion: Motion) -> Self {
        self.motion = motion;
        self
    }
}

/// A state: per element its value.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Values(pub Vec<(&'static str, Value)>);

impl Values {
    pub fn get(&self, id: &str) -> Option<&Value> {
        self.0.iter().find(|(i, _)| *i == id).map(|(_, v)| v)
    }
    pub fn set(&mut self, id: &'static str, v: Value) {
        match self.0.iter_mut().find(|(i, _)| *i == id) {
            Some(slot) => slot.1 = v,
            None => self.0.push((id, v)),
        }
    }

    /// Where this state breaks the HUD's one layout: an element without a
    /// value, a value for no element, an element valued twice.
    pub fn layout_problems(&self, hud: &Hud) -> Vec<String> {
        let mut out = Vec::new();
        for e in &hud.elements {
            match self.0.iter().filter(|(i, _)| *i == e.id).count() {
                0 => out.push(format!("{} has no value", e.id)),
                1 => {}
                n => out.push(format!("{} has {n} values", e.id)),
            }
        }
        for (id, _) in &self.0 {
            if hud.element(id).is_none() {
                out.push(format!("{id} is not an element of {}", hud.name));
            }
        }
        out.dedup();
        out
    }
}

/// A HUD of ours and the named states its preview sheet draws.
pub trait Describe {
    fn hud(&self) -> Hud;
    /// The named states the preview sheet draws.
    fn states(&self) -> Vec<(&'static str, Values)>;
    /// The games the HUD is for: `Some(why)` when it is for multiplayer
    /// maps only, and a campaign map's player HUD (which may lack what its
    /// templates name, the grenade row of `ui\hud\masterchief` on
    /// `00a_introduction`) is no failure of it.
    fn multiplayer_only(&self) -> Option<&'static str> {
        None
    }
    /// The HUDs, by `Hud::name`, that never show together with this one:
    /// two that declare each other cannot meet on the screen (one is for
    /// a monitor, the other for a biped), so the `own` check does not
    /// compare them. The runtime hides the one whose user is not in its
    /// mode.
    fn never_with(&self) -> &'static [&'static str] {
        &[]
    }
    /// The sample poses of the HUD's world marks in the state `state`,
    /// for the preview and the checks: world units in a scene of the
    /// state's own, whose origin the preview puts at its world sample.
    /// The runtime gives the real ones (`Handle::world_mark`). A world
    /// mark a state shows needs one.
    fn world(&self, _state: &str) -> Vec<(&'static str, Pose)> {
        Vec::new()
    }
}
