//! One local player's Objects and Links tools. Only confirming a preview or
//! a link's second end produces an edit.
//!
//! The Objects tool aims at every thing of the session
//! (docs/game/forge-map-variants.md, "Things"): the map's own objects and
//! the added ones, and the marks of starting points, goals, teleporters and
//! item spawners. The nearest thing along the view within its pick radius
//! wins. A thing of another game type than the session's is aimed at only
//! with [`Show::AllGameTypes`] ("Game types").
use std::sync::Arc;

use crate::{BlockKind, Frame, Linkable, Session, Thing, ThingKind, camera::Camera};
use forge_doc::{Endpoint, OpId, Value};
use forge_shapes::PieceDef;
use map::scenario_layout::game_types;

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

/// The pick radius of a thing with no body of its own, in world units: a
/// starting point, a goal, a teleporter and an item spawner are aimed at by
/// their world mark. A spawner's object, when it has one out, lies on its
/// mark, so the mark picks it too.
pub const MARK_RADIUS: f32 = 0.5;
/// The pick radius of a Links tool target, in world units.
pub const LINK_RADIUS: f32 = 0.25;
/// How much a pick radius widens per world unit of distance, so a far thing
/// stays as wide as the central reticle.
pub const RETICLE_SPREAD: f32 = 0.012;

/// What the Objects tool aims at besides the things of the session's game
/// type (`forge.show`, docs/game/forge-map-variants.md, "Game types").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    /// Only the things the session's game type has.
    #[default]
    ThisGameType,
    /// Every thing.
    AllGameTypes,
}

impl Show {
    /// The setting as `forge.status` names it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ThisGameType => "this_game_type",
            Self::AllGameTypes => "all_game_types",
        }
    }
}

/// The basis movement snap and object rotation use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axes {
    #[default]
    World,
    /// The object's axes as they were when it was picked up.
    Local,
}

impl Axes {
    pub const fn name(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Local => "local",
        }
    }
}

/// A drawing the monitor adds over the world, which the monitor's player
/// turns on or off (docs/game/forge-editing.md, "Overlays"). Presentation
/// only: never in the document, the log or the sim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// The game type markers: a spawn, flag, hill, teleporter, item
    /// spawner and ground label each.
    Markers,
    /// The rings of the session's other monitors.
    Monitors,
    /// The axes and outline at the held or aimed object.
    Gizmo,
    /// The last **Generate nav**'s problems (docs/game/forge-nav.md).
    Nav,
    /// Barriers and `Nodraw` faces, drawn in their family's colour
    /// (docs/game/forge-blockout.md, "The dev textures").
    Blockout,
}

impl Overlay {
    pub const ALL: [Overlay; 5] = [
        Overlay::Markers,
        Overlay::Monitors,
        Overlay::Gizmo,
        Overlay::Nav,
        Overlay::Blockout,
    ];

    /// The overlay as `forge.status` names it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Markers => "markers",
            Self::Monitors => "monitors",
            Self::Gizmo => "gizmo",
            Self::Nav => "nav",
            Self::Blockout => "blockout",
        }
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Which overlays are on. Every overlay is on until it is turned off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlays(u8);

impl Default for Overlays {
    fn default() -> Self {
        Self(Overlay::ALL.iter().fold(0, |bits, o| bits | o.bit()))
    }
}

impl Overlays {
    pub const fn on(self, overlay: Overlay) -> bool {
        self.0 & overlay.bit() != 0
    }

    pub fn set(&mut self, overlay: Overlay, on: bool) {
        if on {
            self.0 |= overlay.bit();
        } else {
            self.0 &= !overlay.bit();
        }
    }
}

/// Whether `thing` is one the game type with engine index `game` has (the
/// variant record's `+0x44`: 1 CTF, 2 Slayer, 3 Oddball, 4 King of the Hill,
/// 7 Juggernaut, 8 Territories, 9 Assault). With no game type known, every
/// thing is.
///
/// - An item spawner is when one of its four game-type slots matches the
///   engine, as the engine's matcher decides (`game_types::slot_matches`).
/// - A goal is its flag type's game type's: CTF's flag spawn and return
///   (0, 1), Assault's bomb spawn and return (2, 3), Oddball's ball spawn
///   (4), a territory (10) and a hill corner (11 to 18). A flag type with no
///   traced reader is in every game type.
/// - A starting point, a teleporter and an object are in every game type:
///   no retail reader of a starting point's game type was found, and an
///   object's set is the `games` property (slice G1).
pub fn in_game(thing: &Thing, game: Option<i32>) -> bool {
    let Some(game) = game else {
        return true;
    };
    match thing.kind {
        ThingKind::ItemSpawner => {
            // The matcher's code is the engine index but for King of the
            // Hill, whose code is 0.
            let code = if game == 4 { 0 } else { game };
            let slots: Vec<i16> = (1..=game_types::SLOT_COUNT)
                .filter_map(|n| match field(thing, &format!("game type {n}")) {
                    Some(Value::Int(v)) => i16::try_from(*v).ok(),
                    _ => None,
                })
                .collect();
            slots.is_empty()
                || slots
                    .iter()
                    .any(|&slot| game_types::slot_matches(slot, Some(code)))
        }
        ThingKind::Goal => match field(thing, "type") {
            Some(Value::Int(ty)) => {
                forge_doc::games::flag_games(*ty).is_none_or(|games| games.has(game))
            }
            _ => true,
        },
        ThingKind::StartingPoint
        | ThingKind::Teleporter
        | ThingKind::Object
        | ThingKind::Piece => true,
    }
}

fn field<'a>(thing: &'a Thing, name: &str) -> Option<&'a Value> {
    thing
        .fields
        .iter()
        .find(|(field, _)| *field == name)
        .map(|(_, value)| value)
}

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
    /// Toggle selection of an aimed, ungrouped thing with empty hands.
    Select,
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
    AreaShape,
    AreaClose,
    AreaGrab,
    AreaRelease,
    /// Open the Overlays page.
    Overlays,
}

impl Action {
    /// The same gestures used by the controller and the session API.
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "toggle" => Self::Toggle,
            "accept" => Self::Accept,
            "select" => Self::Select,
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
            "area_shape" => Self::AreaShape,
            "area_close" => Self::AreaClose,
            "area_grab" => Self::AreaGrab,
            "area_release" => Self::AreaRelease,
            "overlays" => Self::Overlays,
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

/// What a tool's input asks of the session or the screens. `Move` and
/// `Delete` name an added object; `MoveThing` (`Session::move_thing`) and
/// `Remove` (`Session::remove`) a thing of the map's own.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Place {
        item: usize,
        frame: Frame,
    },
    Move {
        id: OpId,
        frame: Frame,
    },
    Delete {
        id: OpId,
    },
    MoveThing {
        id: Endpoint,
        frame: Frame,
    },
    /// Carry an existing thing without changing the document until the drop.
    HoldThing {
        id: Endpoint,
        frame: Frame,
    },
    /// Set a decoded property by name.
    SetField {
        id: Endpoint,
        name: &'static str,
        value: Value,
    },
    /// Duplicate the object `id` names at `frame` and hold the copy
    /// (Properties' **Duplicate** row, `Session::duplicate`).
    Duplicate {
        id: Endpoint,
        frame: Frame,
    },
    Remove {
        id: Endpoint,
    },
    /// Give an item spawner a team, or none (`Session::set_team`).
    SetTeam {
        id: Endpoint,
        team: Option<u8>,
    },
    /// Give an item spawner spare clips, or none
    /// (`Session::set_spare_clips`).
    SetSpareClips {
        id: Endpoint,
        clips: Option<u8>,
    },
    /// Give a starting point a weight, or Normal
    /// (`Session::set_spawn_weight`).
    SetSpawnWeight {
        id: Endpoint,
        weight: Option<forge_doc::SpawnWeight>,
    },
    /// Place an item spawner's item at a round's start, or hold it back
    /// (`Session::set_place_at_start`).
    SetPlaceAtStart { id: Endpoint, on: bool },
    /// Restore original: an own thing as the map has it
    /// (`Session::reset`).
    Reset {
        id: Endpoint,
    },
    Link {
        from: Linkable,
        to: Linkable,
    },
    Unlink {
        link: OpId,
    },
    /// Group things under a name ([`crate::Session::group`]).
    Group {
        name: String,
        members: Vec<Endpoint>,
    },
    /// End a group ([`crate::Session::ungroup`]).
    Ungroup {
        group: OpId,
    },
    /// Lock or unlock a group ([`crate::Session::set_group_locked`]).
    LockGroup {
        group: OpId,
        locked: bool,
    },
    SetGroupPivot {
        group: OpId,
        pivot_first: bool,
    },
    /// Give a thing a label, or clear it ([`crate::Session::set_label`]).
    SetLabel {
        id: Endpoint,
        label: Option<forge_doc::Label>,
    },
    /// Place a prefab with its pivot at a frame
    /// ([`crate::Session::place_prefab`]).
    PlacePrefab {
        prefab: Box<forge_doc::Prefab>,
        frame: Frame,
    },
    /// Place a block-out piece of this definition at a frame
    /// ([`crate::Session::place_piece`]).
    PlacePiece {
        def: PieceDef,
        frame: Frame,
    },
    /// Write the Resize draft: the definition and origin it ends with
    /// ([`crate::Session::set_piece`]).
    ResizePiece {
        id: OpId,
        def: PieceDef,
        origin: Frame,
    },
    /// Open the Size page on the Resize draft.
    PieceSize {
        id: OpId,
    },
    Undo,
    Redo,
    /// Label the object as a siege barrier of the phase it opens in
    /// ([`crate::Session::label_barrier`]).
    Barrier {
        id: Endpoint,
        phase: i32,
    },
    /// Place the next ground marker of this label at a frame
    /// ([`crate::Session::place_marker_next`]).
    Marker {
        label: forge_doc::GroundLabel,
        frame: Frame,
    },
    /// **Build AI ground**: build it now.
    BuildGround,
    /// **Generate nav**: generate it now (the Nav tool's A).
    GenerateNav,
    Goal {
        goal: crate::Goal,
        frame: Frame,
    },
    /// Add a starting point for a team in a kit session
    /// ([`crate::Session::place_spawn`]).
    Spawn {
        team: u8,
        frame: Frame,
    },
    /// Add a teleporter sender or receiver in a kit session
    /// ([`crate::Session::place_teleporter`]).
    Teleporter {
        sender: bool,
        frame: Frame,
    },
    /// Add an item spawner of a kit item ([`crate::Session::place_item`]).
    Item {
        item: forge_doc::TagKey,
        frame: Frame,
    },
    /// **Test waves**: build the ground if it is not current.
    TestWaves,
    Palette,
    Properties,
    Menu,
    Tools,
    Settings,
    Overlays,
    DialogUp,
    DialogDown,
    DialogAccept,
    DialogBack,
}

/// How long a lean of the stick waits before it steps a Resize again, in
/// seconds.
pub const DRAFT_REPEAT: f32 = 0.18;
/// How far the stick leans before it steps a Resize.
pub const DRAFT_DEAD: f32 = 0.3;

/// A piece being resized (docs/game/forge-blockout.md, "In the monitor"):
/// tool state until A writes it, never in the document.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub id: OpId,
    /// The definition and origin as the draft has them.
    pub def: PieceDef,
    pub frame: Frame,
    /// What the piece was when the draft began.
    pub from: (PieceDef, Frame),
    /// The face the reticle picked, which the stick pushes and pulls.
    pub face: Option<crate::piece::Face>,
    stick: i8,
    repeat_in: f32,
}

impl Draft {
    /// Whether the draft is the piece as it was.
    pub fn unchanged(&self) -> bool {
        self.def == self.from.0 && self.frame == self.from.1
    }

    /// Set the size on `axis` to `units`, the origin face staying put. A
    /// size the definition refuses leaves the draft as it was.
    pub fn set_size(&mut self, axis: usize, units: i32) -> Result<(), String> {
        let face = crate::piece::Face { axis, far: true };
        let (def, frame) = crate::piece::resize_to(&self.def, self.frame, face, units)?;
        self.def = def;
        self.frame = frame;
        Ok(())
    }

    /// Set the param (steps, sides, voussoirs or seed). A value the shape
    /// refuses leaves the draft as it was.
    pub fn set_param(&mut self, param: u16) -> Result<(), String> {
        let mut next = self.def.clone();
        next.param = param;
        next.generate().map_err(|e| e.to_string())?;
        self.def = next;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub monitor: bool,
    pub camera: Option<Camera>,
    /// The added object aimed at, when the thing aimed at is one.
    pub aimed: Option<OpId>,
    /// The added object held, when the thing held is one.
    pub held: Option<OpId>,
    /// The thing aimed at, own or added.
    pub aimed_thing: Option<Endpoint>,
    /// The thing held, own or added.
    pub held_thing: Option<Endpoint>,
    /// The session's game type, as the variant record's engine index; none
    /// when it is not known, and then every thing is this game type's.
    pub game: Option<i32>,
    /// What is aimed at besides this game type's things.
    pub show: Show,
    /// The basis for movement snap and turning.
    pub axes: Axes,
    /// The drawings added over the world that are on.
    pub overlays: Overlays,
    /// World units between movement snaps; 0 is off.
    movement_step: f32,
    pub placing: Option<usize>,
    /// The block-out piece being placed, held until A drops it.
    pub placing_piece: Option<PieceDef>,
    /// The piece placed or carried is a block-out piece: its origin rounds
    /// to the block-out grid.
    held_piece: bool,
    /// The block-out grid in units ([`forge_shapes::units::GRIDS`]).
    pub block_grid: u16,
    /// The Resize draft in hand.
    pub draft: Option<Draft>,
    pub ghost: Option<Frame>,
    /// The Links tool is selected instead of the Objects tool.
    pub links: bool,
    /// The Nav tool is selected: it aims at nothing and draws the nav.
    pub nav: bool,
    /// The AI tool is selected: the Objects tool's gestures, aiming at
    /// ground markers only (docs/game/forge-firefight-ui.md, "The AI tool").
    pub ai: bool,
    /// What the Links tool aims at.
    pub aimed_link: Option<Linkable>,
    /// The first end of a link being made.
    pub link_from: Option<Linkable>,
    /// The things chosen to be grouped, in the order chosen: tool state,
    /// never in the document (docs/game/forge-editing.md, "Selection,
    /// groups and prefabs").
    selection: Vec<Endpoint>,
    /// Degrees between turn snaps.
    rotation_step: f32,
    pub(crate) rotation: [f32; 3],
    /// Each palette item's bounding sphere, in palette order.
    extents: Arc<[Extent]>,
    /// Orientation on pickup. Snap the *delta* so picking up never changes orientation.
    pub(crate) initial_rotation: [f32; 3],
    /// Position on pickup, the origin of a Local movement snap.
    pub(crate) initial_position: [f32; 3],
    /// Offset from the aim point along the camera's forward, right and up,
    /// so a held object turns with the view and keeps its place in it.
    pub(crate) offset: [f32; 3],
    /// How far in front of the camera the object is carried: its
    /// [`Extent::hold`] for a new one, where it was taken for a picked-up
    /// one, then pushed and pulled.
    pub(crate) distance: f32,
}

impl Default for Tool {
    fn default() -> Self {
        Self {
            monitor: false,
            camera: None,
            aimed: None,
            held: None,
            aimed_thing: None,
            held_thing: None,
            game: None,
            show: Show::default(),
            axes: Axes::default(),
            overlays: Overlays::default(),
            movement_step: 0.0,
            placing: None,
            placing_piece: None,
            held_piece: false,
            block_grid: forge_shapes::units::DEFAULT_GRID,
            draft: None,
            ghost: None,
            links: false,
            nav: false,
            ai: false,
            aimed_link: None,
            link_from: None,
            selection: Vec::new(),
            rotation_step: 15.0,
            rotation: [0.; 3],
            extents: Arc::from(Vec::new()),
            initial_rotation: [0.; 3],
            initial_position: [0.; 3],
            offset: [0.; 3],
            distance: 4.0,
        }
    }
}

impl Tool {
    /// Where the held thing was picked up: the readout's `from` mark.
    pub fn picked_up_at(&self) -> Option<[f32; 3]> {
        self.held_thing.map(|_| self.initial_position)
    }

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
        if let Some(def) = &self.placing_piece {
            return Some(crate::piece::extent(def));
        }
        match (self.placing, self.held_thing) {
            (Some(index), _) => Some(self.extents.get(index).copied().unwrap_or_default()),
            (None, Some(id)) => Some(self.thing_extent(session, &session.thing(id)?)),
            (None, None) => None,
        }
    }

    /// A thing's sphere: an object's palette item's, a piece's from its
    /// size ([`crate::piece::PieceInfo::extent`]), else its mark's, a
    /// sphere of [`MARK_RADIUS`]. An object whose tag is not in the Objects
    /// palette carries [`Extent::default`].
    pub fn thing_extent(&self, session: &Session, thing: &Thing) -> Extent {
        if thing.kind == ThingKind::Piece
            && let Some(piece) = session.piece(thing.id)
        {
            return piece.extent();
        }
        if thing.kind != ThingKind::Object {
            return Extent {
                radius: MARK_RADIUS,
                offset: [0.; 3],
            };
        }
        let Some(Value::Tag(Some(tag))) = field(thing, "palette index") else {
            return Extent::default();
        };
        session
            .palette()
            .iter()
            .position(|p| p.block == thing.block && p.tag == *tag)
            .and_then(|i| self.extents.get(i))
            .copied()
            .unwrap_or_default()
    }

    /// Whether the Objects tool aims at `thing`: one of the session's game
    /// type, or any with [`Show::AllGameTypes`].
    pub fn shows(&self, thing: &Thing) -> bool {
        self.show == Show::AllGameTypes || in_game(thing, self.game)
    }

    /// Choose what is aimed at besides this game type's things. What is
    /// aimed at or held is let go.
    pub fn set_show(&mut self, show: Show) {
        if self.show != show {
            self.show = show;
            self.clear_selection();
        }
    }

    /// The things chosen to be grouped, in the order chosen.
    pub fn selection(&self) -> &[Endpoint] {
        &self.selection
    }

    /// Add a thing to the selection, or take it out when it is in.
    pub fn toggle_selected(&mut self, id: Endpoint) {
        let id = id.identity();
        match self.selection.iter().position(|s| *s == id) {
            Some(i) => {
                self.selection.remove(i);
            }
            None => self.selection.push(id),
        }
    }

    /// Whether the thing is in the selection.
    pub fn is_selected(&self, id: Endpoint) -> bool {
        self.selection.contains(&id.identity())
    }

    /// Let the selection go (B with nothing held).
    pub fn clear_selected(&mut self) {
        self.selection.clear();
    }

    pub fn select(&mut self, index: usize) {
        self.cancel();
        self.placing = Some(index);
        self.distance = self.extents.get(index).copied().unwrap_or_default().hold();
    }

    /// Hold a new block-out piece of `def` (the object menu's Structure and
    /// Natural rows).
    pub fn select_piece(&mut self, def: PieceDef) {
        self.cancel();
        self.distance = crate::piece::extent(&def).hold();
        self.placing_piece = Some(def);
        self.held_piece = true;
    }

    /// Set the block-out grid; a value that is not a grid is ignored.
    pub fn set_block_grid(&mut self, units: u16) {
        if forge_shapes::units::GRIDS.contains(&units) {
            self.block_grid = units;
        }
    }

    /// Begin the Resize draft of the piece `id`.
    pub fn begin_resize(&mut self, session: &Session, id: OpId) -> Result<(), String> {
        let piece = session
            .piece(Endpoint::Added(id))
            .ok_or("Aim at a block-out piece.")?;
        self.clear_selection();
        self.draft = Some(Draft {
            id,
            def: piece.def.clone(),
            frame: piece.frame,
            from: (piece.def, piece.frame),
            face: None,
            stick: 0,
            repeat_in: 0.,
        });
        Ok(())
    }

    /// Chooses the tool: the Links tool, or the Objects tool.
    pub fn set_links(&mut self, links: bool) {
        if self.links != links || (links && (self.nav || self.ai)) {
            self.clear_selection();
            self.links = links;
            if links {
                self.nav = false;
                self.ai = false;
            }
        }
    }

    /// Chooses the Nav tool, or leaves it for the one that was.
    pub fn set_nav(&mut self, nav: bool) {
        if self.nav != nav {
            self.clear_selection();
            self.nav = nav;
            if nav {
                self.links = false;
                self.ai = false;
                self.link_from = None;
            }
        }
    }

    /// Chooses the AI tool, or leaves it for the Objects tool. What is
    /// aimed at or held is let go.
    pub fn set_ai(&mut self, ai: bool) {
        if self.ai != ai || (ai && (self.nav || self.links)) {
            self.clear_selection();
            self.ai = ai;
            if ai {
                self.nav = false;
                self.links = false;
                self.link_from = None;
            }
        }
    }

    pub fn set_rotation_step(&mut self, degrees: f32) {
        if degrees.is_finite() && degrees >= 0.0 {
            self.rotation_step = degrees;
        }
    }

    pub fn set_movement_step(&mut self, units: f32) {
        if units.is_finite() && units >= 0.0 {
            self.movement_step = units;
        }
    }

    /// Change basis without moving what is held: its current frame is the
    /// new Local baseline.
    pub fn set_axes(&mut self, axes: Axes) {
        if self.axes != axes {
            self.axes = axes;
            if let Some(ghost) = self.ghost {
                self.initial_position = ghost.position;
                self.initial_rotation = ghost.rotation;
                self.rotation = [0.; 3];
            }
        }
    }

    pub fn set_heading(&mut self, radians: f32) {
        if radians.is_finite()
            && (self.placing.is_some() || self.placing_piece.is_some() || self.held_thing.is_some())
        {
            self.initial_rotation[0] = radians;
            self.rotation[0] = 0.;
            if let Some(ghost) = &mut self.ghost {
                ghost.rotation[0] = radians;
            }
        }
    }

    /// Reset orientation (Properties): pitch and roll to zero and the yaw
    /// to `radians`, which the caller rounds to the rotation snap. What is
    /// placed or held turns to it at once; an aimed thing is sent as a
    /// move, as a heading is.
    pub fn set_orientation(&mut self, radians: f32) {
        if radians.is_finite()
            && (self.placing.is_some() || self.placing_piece.is_some() || self.held_thing.is_some())
        {
            self.initial_rotation = [radians, 0., 0.];
            self.rotation = [0.; 3];
            if let Some(ghost) = &mut self.ghost {
                ghost.rotation = [radians, 0., 0.];
            }
        }
    }

    /// Carry `id` from `frame`: the state a pickup sets, so Duplicate's
    /// copy is held at once where the add put it. `object` is the added
    /// object's id, none for a thing of the map's own.
    pub fn take_up(&mut self, session: &Session, id: Endpoint, object: Option<OpId>, frame: Frame) {
        let thing = session.thing(id);
        self.cancel();
        self.held_thing = Some(id);
        self.held = object;
        self.held_piece = thing.as_ref().is_some_and(|t| t.kind == ThingKind::Piece);
        self.distance = thing
            .map(|t| self.thing_extent(session, &t).hold())
            .unwrap_or(4.0);
        self.offset = [0.; 3];
        self.initial_rotation = frame.rotation;
        self.initial_position = frame.position;
        self.ghost = Some(frame);
    }

    /// The carried frame in the basis in force: movement snap is on the
    /// world grid or the pickup's own grid, and turning is about those axes.
    fn snapped_frame(&self, raw: [f32; 3]) -> Frame {
        let turn = self
            .rotation
            .map(|v| snap(v, self.rotation_step.to_radians()));
        let rotation = match self.axes {
            Axes::World => std::array::from_fn(|i| self.initial_rotation[i] + turn[i]),
            Axes::Local => to_euler(mat_mul(from_euler(turn), from_euler(self.initial_rotation))),
        };
        let position = match (self.axes, self.movement_step > 0.0) {
            (_, false) => raw,
            (Axes::World, true) => raw.map(|v| snap(v, self.movement_step)),
            (Axes::Local, true) => {
                let delta = to_local(
                    from_euler(self.initial_rotation),
                    sub(raw, self.initial_position),
                );
                let snapped = delta.map(|v| snap(v, self.movement_step));
                add(
                    self.initial_position,
                    to_world(from_euler(self.initial_rotation), snapped),
                )
            }
        };
        // A block-out piece at a quarter turn rounds its origin to the
        // block-out grid on the world's axes.
        let position = if self.held_piece && quarter_turn(rotation) {
            let step = f32::from(self.block_grid.max(1)) / 100.0;
            raw.map(|v| snap(v, step))
        } else {
            position
        };
        Frame { position, rotation }
    }

    fn cancel(&mut self) {
        self.placing_piece = None;
        self.held_piece = false;
        self.link_from = None;
        self.held = None;
        self.held_thing = None;
        self.placing = None;
        self.ghost = None;
        self.initial_rotation = [0.; 3];
        self.initial_position = [0.; 3];
        self.rotation = [0.; 3];
        self.offset = [0.; 3];
        self.distance = 4.0;
    }

    /// A load or external edit can invalidate both the aimed ID and the held preview.
    pub fn clear_selection(&mut self) {
        self.cancel();
        self.draft = None;
        self.aimed = None;
        self.aimed_thing = None;
        self.aimed_link = None;
    }

    pub fn applied(&mut self, command: &Command) {
        if matches!(
            command,
            Command::Place { .. }
                | Command::PlacePiece { .. }
                | Command::ResizePiece { .. }
                | Command::Move { .. }
                | Command::Delete { .. }
                | Command::MoveThing { .. }
                | Command::Remove { .. }
                | Command::Reset { .. }
                | Command::Link { .. }
                | Command::Unlink { .. }
                | Command::Goal { .. }
                | Command::Spawn { .. }
                | Command::Teleporter { .. }
                | Command::Item { .. }
                | Command::PlacePrefab { .. }
                | Command::Undo
                | Command::Redo
        ) {
            self.clear_selection();
        }
        if matches!(command, Command::Group { .. } | Command::Ungroup { .. }) {
            self.selection.clear();
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
        if self.draft.is_some() {
            return self.update_draft(input);
        }
        if self.nav {
            return Ok(self.update_nav(input));
        }
        if self.links {
            return Ok(self.update_links(session, input));
        }
        if input.action == Some(A::Link) {
            self.clear_selection();
            self.links = true;
            self.ai = false;
            return Ok(None);
        }
        let previewing =
            self.placing.is_some() || self.placing_piece.is_some() || self.held_thing.is_some();
        // Things leave the selection when they disappear or join a group.
        let groups = session.groups();
        self.selection
            .retain(|id| session.thing(*id).is_some() && !groups.iter().any(|g| g.has(*id)));
        // A locked group's things are not aimed at.
        let locked: Vec<Endpoint> = groups
            .iter()
            .filter(|g| g.locked)
            .flat_map(|g| g.members.iter().copied())
            .collect();
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
            let eye = *camera;
            let labels = session.labels().unwrap_or_default();
            let marker = |thing: &Thing| {
                thing.block == BlockKind::NetgameFlags
                    && labels.get(&thing.id.identity()).is_some_and(|label| {
                        forge_doc::GroundLabel::of(&label.text).is_some()
                    })
            };
            let only_markers = self.ai;
            hit = session
                .things()
                .into_iter()
                .filter(|thing| {
                    if only_markers {
                        marker(thing)
                    } else {
                        self.shows(thing) || marker(thing)
                    }
                })
                .filter(|thing| !locked.iter().any(|m| m.identity() == thing.id.identity()))
                .filter_map(|thing| {
                    let radius = self.thing_extent(session, &thing).radius;
                    aim(&eye, forward, max_distance, thing.frame.position, radius)
                        .map(|along| (thing, along))
                })
                // The nearest wins; at one distance, the first in the
                // session's order of things.
                .reduce(|best, next| if next.1 < best.1 { next } else { best });
            self.aimed_thing = hit.as_ref().map(|(thing, _)| thing.id);
            self.aimed = hit.as_ref().and_then(|(thing, _)| object_id(thing));
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
                let pos = eye.point(distance);
                let offset = eye.to_world(self.offset);
                self.ghost = Some(self.snapped_frame(std::array::from_fn(|i| pos[i] + offset[i])));
            }
        }
        Ok(match input.action {
            Some(A::Palette) => Some(C::Palette),
            Some(A::Properties) if self.aimed_thing.is_some() || previewing => Some(C::Properties),
            Some(A::Menu) => Some(C::Menu),
            Some(A::Back) if previewing => {
                self.cancel();
                None
            }
            // B with nothing held lets the selection go; the Forge menu is
            // Back's.
            Some(A::Back) => {
                self.selection.clear();
                None
            }
            Some(A::Tools) => Some(C::Tools),
            Some(A::Settings) => Some(C::Settings),
            Some(A::Overlays) => Some(C::Overlays),
            Some(A::Undo) => Some(C::Undo),
            Some(A::Redo) => Some(C::Redo),
            Some(A::Delete) => {
                let (target, object) = if self.held_thing.is_some() {
                    (self.held_thing, self.held)
                } else {
                    (self.aimed_thing, self.aimed)
                };
                target.map(|id| match object {
                    Some(id) => C::Delete { id },
                    None => C::Remove { id },
                })
            }
            Some(A::Select) if !previewing => {
                if let Some((thing, _)) = &hit
                    && !groups.iter().any(|g| g.has(thing.id))
                {
                    self.toggle_selected(thing.id);
                }
                None
            }
            Some(A::Accept) => {
                if let (Some(def), Some(frame)) = (self.placing_piece.clone(), self.ghost) {
                    Some(C::PlacePiece { def, frame })
                } else if let (Some(item), Some(frame)) = (self.placing, self.ghost) {
                    Some(C::Place { item, frame })
                } else if let (Some(id), Some(frame)) = (self.held_thing, self.ghost) {
                    Some(match self.held {
                        Some(id) => C::Move { id, frame },
                        None => C::MoveThing { id, frame },
                    })
                } else if let (Some((thing, distance)), Some(camera)) = (hit, self.camera) {
                    let near = self.thing_extent(session, &thing).near();
                    let distance = distance.max(near);
                    self.held_thing = Some(thing.id);
                    self.held_piece = thing.kind == ThingKind::Piece;
                    self.held = object_id(&thing);
                    self.distance = distance;
                    let p = camera.point(distance);
                    self.offset =
                        camera.to_local(std::array::from_fn(|i| thing.frame.position[i] - p[i]));
                    self.initial_rotation = thing.frame.rotation;
                    self.initial_position = thing.frame.position;
                    self.ghost = Some(thing.frame);
                    None
                } else {
                    None
                }
            }
            _ => None,
        })
    }

    /// The Resize draft: fly, the reticle picks a face, the left stick's
    /// forward and back push and pull it by a grid step, A writes the
    /// draft, X opens Size and B drops it. A step the definition refuses is
    /// refused by name and leaves the draft as it was.
    fn update_draft(&mut self, input: Input) -> Result<Option<Command>, String> {
        use Action as A;
        use Command as C;
        let seconds = input.seconds.clamp(0., 0.1);
        let grid = self.block_grid;
        let Some(draft) = self.draft.as_mut() else {
            return Ok(None);
        };
        if let Some(camera) = &mut self.camera {
            camera.fly([input.movement[0], 0., input.movement[2]], input.look, seconds);
            // The face stays while the stick leans, though it moves.
            if draft.stick == 0 {
                draft.face =
                    crate::piece::pick_face(&draft.def, draft.frame, camera.position, camera.forward())
                        .or(draft.face);
            }
        }
        let lean = if input.movement[1].abs() > DRAFT_DEAD {
            input.movement[1].signum() as i8
        } else {
            0
        };
        let step = lean != 0 && (lean != draft.stick || draft.repeat_in <= seconds);
        draft.stick = lean;
        if step {
            draft.repeat_in = DRAFT_REPEAT;
            let face = draft.face.ok_or("Aim at a face of the piece.")?;
            let (def, frame) =
                crate::piece::resize_step(&draft.def, draft.frame, face, i32::from(lean), grid)?;
            draft.def = def;
            draft.frame = frame;
        } else if lean != 0 {
            draft.repeat_in -= seconds;
        } else {
            draft.repeat_in = 0.;
        }
        Ok(match input.action {
            Some(A::Back) => {
                self.draft = None;
                None
            }
            Some(A::Accept) if draft.unchanged() => {
                self.draft = None;
                None
            }
            Some(A::Accept) => Some(C::ResizePiece {
                id: draft.id,
                def: draft.def.clone(),
                origin: draft.frame,
            }),
            Some(A::Properties) => Some(C::PieceSize { id: draft.id }),
            _ => None,
        })
    }

    /// The Nav tool: fly, and A generates the nav; the generated sectors
    /// and links are drawn while it is selected (docs/game/forge-nav.md).
    /// It aims at nothing and edits nothing in the document.
    fn update_nav(&mut self, input: Input) -> Option<Command> {
        use Action as A;
        use Command as C;
        if let Some(camera) = &mut self.camera {
            camera.fly(input.movement, input.look, input.seconds.clamp(0., 0.1));
        }
        match input.action {
            Some(A::Accept) => Some(C::GenerateNav),
            Some(A::Link) | Some(A::Palette) => {
                let palette = input.action == Some(A::Palette);
                self.nav = false;
                palette.then_some(C::Palette)
            }
            Some(A::Tools) => Some(C::Tools),
            Some(A::Settings) => Some(C::Settings),
            Some(A::Overlays) => Some(C::Overlays),
            Some(A::Menu) => Some(C::Menu),
            Some(A::Undo) => Some(C::Undo),
            Some(A::Redo) => Some(C::Redo),
            _ => None,
        }
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
            let added = session
                .added_teleporters()
                .into_iter()
                .map(|t| (Linkable::Flag(t.id), t.position))
                .collect::<Vec<_>>();
            let machines = session
                .objects()
                .iter()
                .filter(|o| o.item.block == BlockKind::Machines)
                .map(|o| (Linkable::Object(o.id), o.frame.position));
            self.aimed_link = flags
                .chain(added)
                .chain(machines)
                .filter_map(|(thing, at)| {
                    aim(&camera, forward, 80., at, LINK_RADIUS).map(|d| (thing, d))
                })
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
            Some(A::Overlays) => Some(C::Overlays),
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

fn object_id(thing: &Thing) -> Option<OpId> {
    match thing.id {
        Endpoint::Added(id) if thing.block != BlockKind::NetgameFlags => Some(id),
        _ => None,
    }
}

/// How far along the view `at` lies when it is under the reticle: the view
/// passes within `radius` of it, widened by [`RETICLE_SPREAD`] per unit of
/// distance so a far thing subtends the central reticle's small angle.
fn aim(
    camera: &Camera,
    forward: [f32; 3],
    max_distance: f32,
    at: [f32; 3],
    radius: f32,
) -> Option<f32> {
    let d: [f32; 3] = std::array::from_fn(|i| at[i] - camera.position[i]);
    let along: f32 = (0..3).map(|i| d[i] * forward[i]).sum();
    if !(0.1..=max_distance).contains(&along) {
        return None;
    }
    let side: f32 = (0..3).map(|i| (d[i] - along * forward[i]).powi(2)).sum();
    (side <= (radius + along * RETICLE_SPREAD).powi(2)).then_some(along)
}

/// Whether a turn is a whole quarter turn of yaw with no pitch or roll.
fn quarter_turn([yaw, pitch, roll]: [f32; 3]) -> bool {
    const EPS: f32 = 1e-4;
    let quarters = yaw / std::f32::consts::FRAC_PI_2;
    (quarters - quarters.round()).abs() < EPS
        && pitch.sin().abs() < EPS
        && roll.sin().abs() < EPS
}

fn snap(v: f32, step: f32) -> f32 {
    if step > 0.0 {
        (v / step).round() * step
    } else {
        v
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

/// `Rz(yaw) · Ry(−pitch) · Rx(roll)` as row vectors, the scenario's basis.
fn from_euler([yaw, pitch, roll]: [f32; 3]) -> [[f32; 3]; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = roll.sin_cos();
    [
        [cy * cp, sy * cp, sp],
        [-sy * cr - cy * sp * sr, cy * cr - sy * sp * sr, -cp * sr],
        [sy * sr - cy * sp * cr, -cy * sr - sy * sp * cr, cp * cr],
    ]
}

fn to_euler(m: [[f32; 3]; 3]) -> [f32; 3] {
    let pitch = m[0][2].clamp(-1.0, 1.0).asin();
    // With forward vertical, yaw is ambiguous: keep it zero and put the
    // remaining turn in roll.
    if pitch.abs() >= std::f32::consts::FRAC_PI_2 - 1e-6 {
        return [0.0, pitch, (-pitch.signum() * m[1][0]).atan2(m[1][1])];
    }
    [
        m[0][1].atan2(m[0][0]).rem_euclid(std::f32::consts::TAU),
        pitch,
        (-m[1][2]).atan2(m[2][2]),
    ]
}

fn to_local(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    m.map(|axis| axis[0] * v[0] + axis[1] * v[1] + axis[2] * v[2])
}

fn to_world(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[0][i] * v[0] + m[1][i] * v[1] + m[2][i] * v[2])
}

fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|r| {
        std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c])
    })
}
