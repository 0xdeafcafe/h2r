//! What each Forge screen shows, as data (docs/game/forge-firefight-ui.md
//! "The screens", docs/game/forge-editing.md "The object menu" and
//! "Properties"). A [`Sheet`] is a screen's title, body, rows and focus.
//! The builders here make one from the session's facts, and tables decide
//! the rows: the object menu's groups ([`GROUPS`]), the Properties rows
//! (the doc's, [`super::props`]), the tools ([`TOOLS`]), the settings
//! ([`SETTINGS`]), Forge options ([`OPTIONS`]) and the controls (the
//! bindings table). A change of design is a change of table.

use forge::{BlockKind, Thing, Value};

use crate::ui_kit::ids::{self, Family};

/// The Forge screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The object menu: the map's groups, each opening the value picker
    /// on its objects.
    Objects,
    /// The aimed or held object's settings.
    Properties,
    /// Typed position and rotation, with nudges.
    Coordinates,
    /// The tools that are built.
    Tools,
    /// The tool settings, a row each.
    Settings,
    /// A link's two ends and what it writes.
    Link,
    /// Yes or no, No focused.
    Question,
    /// The Forge menu's page of what is not on GAME MENU.
    Options,
    /// Every control, from the bindings table.
    Controls,
    /// The live object pool and scenario block counts.
    MapLimits,
    /// The game types of an item spawner, ticked a row each.
    Games,
    /// The map variants of the open map, a row each.
    Variants,
    /// One map variant's actions, a dialog over [`Kind::Variants`].
    Variant,
    /// WAVES: the wave set's settings, its rounds and its problems
    /// (docs/game/forge-firefight-ui.md, "The Waves screens").
    Waves,
    /// The rounds of the set, each opening [`Kind::Round`].
    Rounds,
    /// ROUND n: its skull and its five waves.
    Round,
    /// WAVE n: its gap and its squads.
    Wave,
    /// A wave's squads, each opening [`Kind::Squad`].
    Squads,
    /// SQUAD n: its enemy, count and drop.
    Squad,
    /// What the set asks for that the AI ground lacks, each opening its
    /// squad.
    WaveProblems,
    /// The classes the AI ground builds, each On or Off.
    EnemyTypes,
}

/// How a screen is built, each on a retail screen tag the game maps have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// The large dialog (`error_dialog_large`, screen 240) laid out as
    /// `low_level_settings`: its list on `default6` and its description
    /// block, as the in-game value picker (`kit.picker.game`) is.
    Picker,
    /// A list page on the in-game Controller Settings (screen 200; the
    /// Controls tag, screen 39, in the main menu).
    List,
    /// The large dialog with a body and two answers, as
    /// `error_dialog_ok_cancel` asks.
    Question,
}

impl Kind {
    pub const ALL: [Kind; 21] = [
        Kind::Objects,
        Kind::Properties,
        Kind::Coordinates,
        Kind::Tools,
        Kind::Settings,
        Kind::Link,
        Kind::Question,
        Kind::Options,
        Kind::Controls,
        Kind::Games,
        Kind::Variants,
        Kind::Variant,
        Kind::MapLimits,
        Kind::Waves,
        Kind::Rounds,
        Kind::Round,
        Kind::Wave,
        Kind::Squads,
        Kind::Squad,
        Kind::WaveProblems,
        Kind::EnemyTypes,
    ];

    /// The name `Open` rows, `h2 ui preview` and the loader know it by.
    pub const fn screen(self) -> &'static str {
        match self {
            Kind::Objects => "forge.objects",
            Kind::Properties => "forge.properties",
            Kind::Coordinates => "forge.coordinates",
            Kind::Tools => "forge.tools",
            Kind::Settings => "forge.settings",
            Kind::Link => "forge.link",
            Kind::Question => "forge.question",
            Kind::Options => "forge.options",
            Kind::Controls => "forge.controls",
            Kind::Games => "forge.games",
            Kind::Variants => "forge.variants",
            Kind::Variant => "forge.variant",
            Kind::MapLimits => "forge.map-limits",
            Kind::Waves => "forge.waves",
            Kind::Rounds => "forge.rounds",
            Kind::Round => "forge.round",
            Kind::Wave => "forge.wave",
            Kind::Squads => "forge.squads",
            Kind::Squad => "forge.squad",
            Kind::WaveProblems => "forge.wave-problems",
            Kind::EnemyTypes => "forge.enemy-types",
        }
    }

    /// Its class's feature id (`features.<id>`).
    pub const fn feature(self) -> &'static str {
        match self {
            Kind::Objects => "forge-objects",
            Kind::Properties => "forge-properties",
            Kind::Coordinates => "forge-coordinates",
            Kind::Tools => "forge-tools",
            Kind::Settings => "forge-settings",
            Kind::Link => "forge-link",
            Kind::Question => "forge-question",
            Kind::Options => "forge-options",
            Kind::Controls => "forge-controls",
            Kind::Games => "forge-games",
            Kind::Variants => "forge-variants",
            Kind::Variant => "forge-variant",
            Kind::MapLimits => "forge-map-limits",
            Kind::Waves => "forge-waves",
            Kind::Rounds => "forge-rounds",
            Kind::Round => "forge-round",
            Kind::Wave => "forge-wave",
            Kind::Squads => "forge-squads",
            Kind::Squad => "forge-squad",
            Kind::WaveProblems => "forge-wave-problems",
            Kind::EnemyTypes => "forge-enemy-types",
        }
    }

    pub const fn family(self) -> Family {
        match self {
            Kind::Objects => const { ids::family("forge objects") },
            Kind::Properties => const { ids::family("forge properties") },
            Kind::Coordinates => const { ids::family("forge coordinates") },
            Kind::Tools => const { ids::family("forge tools") },
            Kind::Settings => const { ids::family("forge settings") },
            Kind::Link => const { ids::family("forge link") },
            Kind::Question => const { ids::family("forge question") },
            Kind::Options => const { ids::family("forge options") },
            Kind::Controls => const { ids::family("forge controls") },
            Kind::Games => const { ids::family("forge games") },
            Kind::Variants => const { ids::family("forge variants") },
            Kind::Variant => const { ids::family("forge variant") },
            Kind::MapLimits => const { ids::family("forge map limits") },
            Kind::Waves => const { ids::family("forge waves") },
            Kind::Rounds => const { ids::family("forge rounds") },
            Kind::Round => const { ids::family("forge round") },
            Kind::Wave => const { ids::family("forge wave") },
            Kind::Squads => const { ids::family("forge squads") },
            Kind::Squad => const { ids::family("forge squad") },
            Kind::WaveProblems => const { ids::family("forge wave problems") },
            Kind::EnemyTypes => const { ids::family("forge enemy types") },
        }
    }

    pub const fn look(self) -> Look {
        match self {
            Kind::Objects
            | Kind::Tools
            | Kind::Games
            | Kind::Variants
            | Kind::Variant
            | Kind::Rounds
            | Kind::Squads
            | Kind::WaveProblems => Look::Picker,
            Kind::Properties
            | Kind::Coordinates
            | Kind::Settings
            | Kind::Options
            | Kind::MapLimits
            | Kind::Controls
            | Kind::Waves
            | Kind::Round
            | Kind::Wave
            | Kind::Squad
            | Kind::EnemyTypes => Look::List,
            Kind::Link | Kind::Question => Look::Question,
        }
    }

    pub fn of_screen(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.screen() == name)
    }
}

impl Look {
    /// The retail screen it is built on in a game, by screen id: each must
    /// be in `shared.map` (the disc test holds it).
    pub const fn ingame_screen_id(self) -> i16 {
        match self {
            Look::Picker | Look::Question => crate::ui_kit::dialog::LARGE_SCREEN_ID,
            Look::List => crate::list_class::INGAME_SCREEN_ID as i16,
        }
    }
}

/// Which question a [`Kind::Question`] asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Delete an object that has links (its links go too).
    DeleteLinked,
    /// Restore original: an own thing back to the map's.
    Restore,
    /// Save as over a map variant of that name.
    Overwrite,
    /// Leave Game with unsaved edits.
    Leave,
    /// Load a map variant over unsaved edits.
    Load,
    /// Delete a saved map variant.
    DeleteVariant,
    /// A Waves page's question (`super::waves::WavesAsk`).
    Waves(super::waves::WavesAsk),
}

/// A value a Properties or Tool settings row sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    /// The object's yaw, a value of [`HEADINGS`].
    Heading,
    /// The tool's rotation snap, a value of [`ANGLE_STEPS`].
    RotationSnap,
    /// The tool's movement snap, a value of [`MOVE_STEPS`].
    MovementSnap,
    /// World or Local axes.
    Axes,
    /// An overlay, On (value 0) or Off (value 1).
    Overlay(forge::tool::Overlay),
    /// Open the six-coordinate editor.
    Coordinates,
    /// A spawn or goal's team.
    Team,
    /// An item spawner's team, a property of the document.
    SpawnerTeam,
    /// A weapon spawner's spare clips, a property of the document: value 0
    /// the weapon's own reserve, then 0 to 10 clips.
    SpareClips,
    /// A starting point's weight, a property of the document: Low,
    /// Normal, High.
    SpawnWeight,
    /// An item spawner's place at start, a property of the document: Yes,
    /// No.
    PlaceAtStart,
    /// A goal's netgame flag type.
    Role,
    /// A goal's identifier.
    Order,
    /// An item entry's respawn time.
    RespawnTime,
    /// A flag's **Show in play**, On (value 0) or Off (value 1).
    ShowInPlay,
    /// The block-out grid, a value of [`BLOCK_GRIDS`].
    BlockGrid,
    /// Dev view, On (value 0) or Off (value 1).
    DevView,
}

/// A tool of the Tools picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Objects,
    Links,
    Nav,
    /// Aims at the AI ground's markers only.
    Ai,
}

/// One row of the Coordinates editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coordinate {
    X,
    Y,
    Z,
    Yaw,
    Pitch,
    Roll,
}

impl Coordinate {
    pub const ALL: [Coordinate; 6] = [
        Coordinate::X,
        Coordinate::Y,
        Coordinate::Z,
        Coordinate::Yaw,
        Coordinate::Pitch,
        Coordinate::Roll,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Coordinate::X => "X",
            Coordinate::Y => "Y",
            Coordinate::Z => "Z",
            Coordinate::Yaw => "Yaw",
            Coordinate::Pitch => "Pitch",
            Coordinate::Roll => "Roll",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Coordinate::X | Coordinate::Yaw => 0,
            Coordinate::Y | Coordinate::Pitch => 1,
            Coordinate::Z | Coordinate::Roll => 2,
        }
    }

    pub const fn is_angle(self) -> bool {
        matches!(self, Coordinate::Yaw | Coordinate::Pitch | Coordinate::Roll)
    }
}

/// The dimensions the area editor accepts from the Properties keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaField {
    /// A circle's or a sphere's radius.
    Radius,
    /// A box's half length along the marker's x.
    Length,
    /// A box's half length along the marker's y.
    Width,
    Below,
    Above,
}

impl AreaField {
    fn label(self) -> &'static str {
        match self {
            Self::Radius => "Radius",
            Self::Length => "Half length",
            Self::Width => "Half width",
            Self::Below => "Below",
            Self::Above => "Above",
        }
    }
}

/// A typed number's destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberField {
    Coordinate(Coordinate),
    Area(AreaField),
    RespawnTime,
    /// An item entry's return time (its abandon limit).
    ReturnTime,
    /// A thing's label text (the keyboard takes it as typed).
    Label,
    /// A thing's label number; empty takes the number off.
    LabelNumber,
    /// The name a prefab is saved as.
    PrefabName,
    /// The new name of the map variant the Map variants screen chose
    /// (`forge_screens::variants`).
    RenameVariant,
    /// The name of the copy of that map variant.
    DuplicateVariant,
    /// The wave set's name (the WAVES page's Name row).
    WavesName,
    /// A row of the Size page, in block-out units.
    Size(PieceField),
}

/// A row of the Size page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceField {
    /// Width, Depth or Height: the size on axis 0, 1 or 2.
    Axis(usize),
    /// Steps, Sides, Voussoirs or the seed, by shape.
    Param,
}

/// A row that opens the game's keyboard for a number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeSpec {
    pub field: NumberField,
    pub initial: String,
}

/// What a screen sends the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Hold a new object: palette index `i` (the object menu).
    Select(usize),
    /// Place the next ground marker of a label (the AI and objectives
    /// page).
    Marker(forge::GroundLabel),
    Goal(forge::Goal),
    /// Add a starting point for team `n` (the Spawns group, in a kit
    /// session).
    Spawn(u8),
    /// Add a teleporter sender (true) or receiver (false) (the Teleporters
    /// group, in a kit session).
    Teleporter(bool),
    /// Add an item spawner of the kit's item `i`
    /// (`forge::Session::kit_items`), from a Weapons, Vehicles or
    /// Equipment group.
    Item(usize),
    EditArea(forge::Endpoint),
    /// **Build AI ground**.
    BuildGround,
    /// **Test waves**: run the session's wave set on the running map,
    /// building the ground first when the map does not hold it.
    TestWaves,
    /// **Stop waves**: end the wave test, its squads erased.
    StopWaves,
    /// **Generate nav**: the bots' nav with the placed objects in.
    GenerateNav,
    /// **Rebuild map** (GAME MENU): build the map from the document and
    /// load it, the session going on over it.
    Rebuild,
    /// The placed objects browser: fly the monitor to an added object and
    /// look at it.
    FlyTo(forge::OpId),
    /// The placed objects browser: fly the monitor to the centre of a
    /// group and look at it.
    FlyToGroup(forge::OpId),
    /// Add a thing to the selection, or take it out
    /// (docs/game/forge-editing.md, "Selection, groups and prefabs").
    ToggleSelected(forge::Endpoint),
    /// **Group selection**: one group of everything selected.
    GroupSelection,
    /// **Ungroup**: end a group.
    Ungroup(forge::OpId),
    /// **Lock group** (true) or **Unlock group** (false).
    LockGroup(forge::OpId, bool),
    /// **Pivot**: change where the group turns.
    SetGroupPivot(forge::OpId, bool),
    /// **Clear label**: take the label off a thing.
    ClearLabel(forge::Endpoint),
    /// **Reset orientation**: pitch and roll to zero, the yaw to the
    /// rotation snap (docs/game/forge-map-variants-ui.md, "The Properties
    /// page").
    ResetOrientation,
    /// **Delete** the thing (asks first when it has links).
    Delete(forge::Endpoint),
    /// **Restore original**: ask, then put the own thing back as the map
    /// has it.
    Restore(forge::Endpoint),
    /// **Duplicate** the object and hold the copy.
    Duplicate(forge::Endpoint),
    /// Place prefab `i` (the object menu's Prefabs row) in front of the
    /// monitor.
    PlacePrefab(usize),
    /// Set a value to value `i` of its row's values.
    Set(Prop, usize),
    /// A number typed on the game's keyboard.
    Number(NumberField, String),
    /// Nudge a coordinate left (false) or right (true).
    Nudge(Coordinate, bool),
    /// Coordinates closed: drop once.
    CoordinatesDone,
    /// Use a tool.
    Tool(Tool),
    /// Remove the link the Link dialog shows.
    Unlink,
    /// A question's answer.
    Answer {
        ask: Ask,
        yes: bool,
    },
    /// **Save**: write the open variant under its name.
    Save,
    /// **Save as**: the name typed on the keyboard.
    SaveAs(String),
    /// Load the map variant of this file stem (`variants::Row::file`).
    Load(String),
    /// Give the saved map variant of this file stem the typed name.
    RenameVariant(String, String),
    /// Copy the saved map variant of this file stem under the typed name.
    DuplicateVariant(String, String),
    /// **Delete** on a variant's dialog: ask, then remove this file stem.
    DeleteVariant(String),
    /// The Game types dialog's Done: give the aimed or held item spawner
    /// this set, as one step.
    Games(forge_doc::games::GameSet),
    /// A Waves page's value: one change of the wave set
    /// (`super::waves::apply`), one `forge::Session::set_waves`.
    Waves(super::waves::Edit),
    /// An Enemy types row: ask the AI ground for the class (true) or not.
    EnemyType(String, bool),
    /// Hold a new block-out piece: the shape at its starting size, and the
    /// seed of a natural one (the Structure and Natural groups).
    Piece(forge::Shape, u16),
    /// **Resize** the piece: begin its draft.
    Resize(forge::OpId),
    /// Nudge a Size row left (false) or right (true) by the block-out grid.
    NudgeSize(PieceField, bool),
    /// **Reroll**: the piece's next seed.
    Reroll(forge::OpId),
    /// Give the piece a material on a role, or on every role with none.
    Material(forge::OpId, Option<forge_shapes::Role>, forge::Material),
    /// Why a row did nothing, for the HUD's notice.
    Notice(String),
}

/// What A on a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Send the choice and close the screen.
    Choose(Choice),
    /// Open the value picker over the screen.
    Pick(PickSpec),
    /// Open another Forge screen above this one; B comes back.
    Open(Kind),
    /// Open the game's keyboard for a typed number.
    Type(TypeSpec),
    /// A fact: A does nothing.
    Info,
    /// Close the screen, choosing nothing (Done).
    Close,
    /// Open this sheet above the screen; B comes back (a row whose
    /// dialog depends on the thing shown).
    Dialog(Box<Sheet>),
    /// The Game types dialog's tick rows.
    Tick(super::games::Tick),
    /// The Game types dialog's Done: [`super::games::done`].
    Done,
    /// A map variant's row on Map variants: the file stem it stands for
    /// is the target of the dialog's rows, which open above this screen.
    Variant(String, Box<Sheet>),
    /// Open a Waves page above this screen, built from the session's set
    /// as it is then (`super::waves::open`); B comes back.
    Waves(super::waves::Page),
}

/// A row's value picker: its values, the held one, what it says, and the
/// choice each value sends. With `closes`, choosing also closes the screen
/// beneath (the object menu, once an object is chosen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickSpec {
    pub title: String,
    pub values: Vec<String>,
    pub current: usize,
    /// One line per value (the object's line), else `description` for all.
    pub details: Vec<String>,
    pub description: String,
    pub choices: Vec<Choice>,
    pub closes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetRow {
    pub label: String,
    /// In the value column (Properties' values, the Controls page's
    /// buttons).
    pub value: Option<String>,
    /// Whether this row's source could not supply its value.
    pub(super) value_unavailable: bool,
    /// The focused row's line in the description block.
    pub description: Option<String>,
    pub act: Act,
}

impl SheetRow {
    pub(super) fn new(label: &str, act: Act) -> Self {
        SheetRow {
            label: label.into(),
            value: None,
            value_unavailable: false,
            description: None,
            act,
        }
    }
    pub(super) fn value(mut self, v: &str) -> Self {
        self.value = Some(v.into());
        self.value_unavailable = false;
        self
    }
    pub(super) fn unavailable(mut self) -> Self {
        self.value = Some("Unavailable".into());
        self.value_unavailable = true;
        self
    }
    pub(super) fn describe(mut self, d: &str) -> Self {
        self.description = Some(d.into());
        self
    }
}

/// One screen's content: what [`super::screen::ForgeScreen`] shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sheet {
    pub kind: Kind,
    pub title: String,
    /// The question's or the link's text; empty on the others.
    pub body: String,
    pub rows: Vec<SheetRow>,
    /// The row focused as it opens.
    pub focus: usize,
}

/// A group of the object menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Weapons,
    Vehicles,
    Equipment,
    Scenery,
    Crates,
    Machines,
    Teleporters,
    Spawns,
    Goals,
    /// Block-out pieces: Block, Wall, Floor plate and the rest
    /// (docs/game/forge-blockout.md, "In the monitor").
    Structure,
    /// The five rocks, eight seeds each.
    Natural,
}

/// A group's row: its name and what one of it is called in a line.
pub struct GroupDef {
    pub group: Group,
    pub name: &'static str,
    pub one: &'static str,
}

/// The object menu's groups, in order (docs/game/forge-editing.md "The
/// object menu"). A group with nothing on this map is absent. Structure,
/// Lights, Prefabs and My imports join with the slices that make them.
pub const GROUPS: &[GroupDef] = &[
    GroupDef {
        group: Group::Weapons,
        name: "Weapons",
        one: "Weapon",
    },
    GroupDef {
        group: Group::Vehicles,
        name: "Vehicles",
        one: "Vehicle",
    },
    GroupDef {
        group: Group::Equipment,
        name: "Equipment",
        one: "Equipment",
    },
    GroupDef {
        group: Group::Scenery,
        name: "Scenery",
        one: "Scenery",
    },
    GroupDef {
        group: Group::Crates,
        name: "Crates",
        one: "Crate",
    },
    GroupDef {
        group: Group::Machines,
        name: "Machines",
        one: "Machine",
    },
    GroupDef {
        group: Group::Teleporters,
        name: "Teleporters",
        one: "Teleporter",
    },
    GroupDef {
        group: Group::Spawns,
        name: "Spawns",
        one: "Spawn",
    },
    GroupDef {
        group: Group::Goals,
        name: "Goals",
        one: "Goal",
    },
    GroupDef {
        group: Group::Structure,
        name: "Structure",
        one: "Structure",
    },
    GroupDef {
        group: Group::Natural,
        name: "Natural",
        one: "Natural",
    },
];

impl Group {
    /// The group a palette block's objects are in: every block has one, so
    /// nothing a map offers is left out.
    pub const fn of_block(b: BlockKind) -> Group {
        match b {
            BlockKind::Weapons => Group::Weapons,
            BlockKind::Vehicles => Group::Vehicles,
            BlockKind::Equipment | BlockKind::NetgameEquipment | BlockKind::StartingEquipment => {
                Group::Equipment
            }
            BlockKind::Scenery | BlockKind::SoundScenery => Group::Scenery,
            BlockKind::Crates => Group::Crates,
            BlockKind::Machines => Group::Machines,
            BlockKind::StartingLocations => Group::Spawns,
            BlockKind::NetgameFlags => Group::Goals,
        }
    }

    pub fn def(self) -> &'static GroupDef {
        GROUPS
            .iter()
            .find(|d| d.group == self)
            .expect("every group is in GROUPS")
    }
}

/// One object of the map's palette: its group, its name, how many are
/// placed and how many more fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub group: Group,
    pub name: String,
    pub placed: usize,
    pub left: usize,
    /// A palette row the menu leaves out: a block-out definition's `bloc`,
    /// which the Structure and Natural groups place.
    pub hidden: bool,
}

/// An object's line in the object menu: what it is, how many are placed,
/// how many more fit.
pub fn object_line(group: Group, placed: usize, left: usize) -> String {
    let placed = match placed {
        0 => "None placed".to_string(),
        n => format!("{n} placed"),
    };
    let left = match left {
        0 => "no more fit.".to_string(),
        1 => "1 more fits.".to_string(),
        n => format!("{n} more fit."),
    };
    format!("{}. {placed}, {left}", group.def().one)
}

/// The object menu: the groups of [`GROUPS`] that have objects on this map,
/// in its order, each row opening the value picker on its objects (by
/// name), the group and object last chosen (`selected`, a palette index)
/// focused. Choosing an object holds it and closes both. `None` when the
/// map has nothing to place.
pub fn objects(items: &[Item], selected: usize) -> Option<Sheet> {
    let mut focus = 0;
    let rows: Vec<SheetRow> = GROUPS
        .iter()
        .filter_map(|g| {
            let mut members: Vec<(usize, &Item)> = items
                .iter()
                .enumerate()
                .filter(|(_, i)| !i.hidden && i.group == g.group)
                .collect();
            if members.is_empty() {
                return None;
            }
            members.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
            Some((g, members))
        })
        .enumerate()
        .map(|(row, (g, members))| {
            let current = members.iter().position(|(i, _)| *i == selected);
            if current.is_some() {
                focus = row;
            }
            let n = members.len();
            let pick = PickSpec {
                title: g.name.to_string(),
                values: members.iter().map(|(_, m)| m.name.clone()).collect(),
                current: current.unwrap_or(0),
                details: members
                    .iter()
                    .map(|(_, m)| object_line(m.group, m.placed, m.left))
                    .collect(),
                description: String::new(),
                choices: members.iter().map(|(i, _)| Choice::Select(*i)).collect(),
                closes: true,
            };
            let count = if n == 1 {
                "1 object.".to_string()
            } else {
                format!("{n} objects.")
            };
            SheetRow::new(g.name, Act::Pick(pick)).describe(&count)
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    Some(Sheet {
        kind: Kind::Objects,
        title: "OBJECTS".into(),
        body: String::new(),
        rows,
        focus,
    })
}

/// The name of the object menu's page of ground labels
/// (docs/game/game-types-without-carriers.md, "In Forge").
pub const AI_PAGE: &str = "AI and objectives";

/// A ground label on the AI and objectives page, and how many are placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiItem {
    pub label: forge::GroundLabel,
    pub placed: usize,
}

/// A label's line on the page: what it is and how many are placed.
pub fn ai_line(item: &AiItem) -> String {
    let placed = match item.placed {
        0 => "None placed".to_string(),
        n => format!("{n} placed"),
    };
    format!("{}. {placed}", item.label.text())
}

/// The name of the object menu's page of the Siege layout recipe: Site,
/// Flag stand, Spawn line and Barrier (docs/game/rules-engine.md, "Siege
/// layouts").
pub const SIEGE_PAGE: &str = "Siege";

/// One page of labels: a row opening the value picker on them, each
/// choice a [`Choice::Marker`].
fn label_page(page: &str, items: &[&AiItem], title: fn(forge::GroundLabel) -> &'static str, what: &str) -> SheetRow {
    let pick = PickSpec {
        title: page.into(),
        values: items.iter().map(|a| title(a.label).to_string()).collect(),
        current: 0,
        details: items.iter().map(|a| ai_line(a)).collect(),
        description: String::new(),
        choices: items.iter().map(|a| Choice::Marker(a.label)).collect(),
        closes: true,
    };
    SheetRow::new(page, Act::Pick(pick)).describe(&format!("{} {what}.", items.len()))
}

/// [`objects`] with the AI and objectives page and the Siege page after the
/// map's groups: the Firefight labels on the first, the Siege labels
/// ([`forge::GroundLabel::SIEGE`]) on the second, each page one row opening
/// the value picker on its labels. A page with no labels is left out, so
/// with none it is [`objects`], and a map with nothing to place still has
/// the pages.
pub fn objects_with_ai(items: &[Item], selected: usize, ai: &[AiItem]) -> Option<Sheet> {
    let sheet = objects(items, selected);
    if ai.is_empty() {
        return sheet;
    }
    let (siege, firefight): (Vec<&AiItem>, Vec<&AiItem>) = ai.iter().partition(|a| a.label.is_siege());
    let mut sheet = sheet.unwrap_or(Sheet {
        kind: Kind::Objects,
        title: "OBJECTS".into(),
        body: String::new(),
        rows: Vec::new(),
        focus: 0,
    });
    if !firefight.is_empty() {
        sheet.rows.push(label_page(AI_PAGE, &firefight, forge::GroundLabel::title, "markers"));
    }
    if !siege.is_empty() {
        sheet.rows.push(label_page(SIEGE_PAGE, &siege, forge::GroundLabel::short_title, "labels"));
    }
    Some(sheet)
}

/// A prefab of the object menu's Prefabs row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrefabItem {
    /// Its index among the prefabs the editor holds: what
    /// [`Choice::PlacePrefab`] sends.
    pub index: usize,
    pub name: String,
    pub line: String,
}

/// The name of the object menu's row of prefabs.
pub const PREFABS_PAGE: &str = "Prefabs";

/// A prefab's line in the picker: how many objects, and whether it places
/// as a group.
pub fn prefab_line(pieces: usize, grouped: bool) -> String {
    let count = if pieces == 1 {
        "1 object".to_string()
    } else {
        format!("{pieces} objects")
    };
    format!("Prefab. {count}{}.", if grouped { ", grouped" } else { "" })
}

/// `sheet` with the Prefabs row after the groups: one row, opening the
/// value picker on the prefabs that fit this map, each choice a
/// [`Choice::PlacePrefab`]. With none it is `sheet`, and a map with
/// nothing to place still has the row.
pub fn with_prefabs(sheet: Option<Sheet>, prefabs: &[PrefabItem]) -> Option<Sheet> {
    if prefabs.is_empty() {
        return sheet;
    }
    let pick = PickSpec {
        title: PREFABS_PAGE.into(),
        values: prefabs.iter().map(|p| p.name.clone()).collect(),
        current: 0,
        details: prefabs.iter().map(|p| p.line.clone()).collect(),
        description: String::new(),
        choices: prefabs
            .iter()
            .map(|p| Choice::PlacePrefab(p.index))
            .collect(),
        closes: true,
    };
    let count = if prefabs.len() == 1 {
        "1 prefab.".to_string()
    } else {
        format!("{} prefabs.", prefabs.len())
    };
    let row = SheetRow::new(PREFABS_PAGE, Act::Pick(pick)).describe(&count);
    let mut sheet = sheet.unwrap_or(Sheet {
        kind: Kind::Objects,
        title: "OBJECTS".into(),
        body: String::new(),
        rows: Vec::new(),
        focus: 0,
    });
    sheet.rows.push(row);
    Some(sheet)
}

/// Offer objective centre flags in the Objects menu's Goals group.
pub fn with_goals(s: Option<Sheet>) -> Sheet {
    with_goals_for(s, &forge::Goal::ALL)
}

/// [`with_goals`] for the goals a game type reads (`forge::Session::goals`).
/// With none the group is absent.
pub fn with_goals_for(s: Option<Sheet>, offered: &[forge::Goal]) -> Sheet {
    let mut sheet = s.unwrap_or(Sheet {
        kind: Kind::Objects,
        title: "OBJECTS".into(),
        body: String::new(),
        rows: Vec::new(),
        focus: 0,
    });
    if offered.is_empty() {
        return sheet;
    }
    let title = Group::Goals.def().name;
    let mut goals = PickSpec {
        title: title.into(),
        values: Vec::new(),
        current: 0,
        details: Vec::new(),
        description: String::new(),
        choices: Vec::new(),
        closes: true,
    };
    let existing = sheet.rows.iter().position(|row| row.label == title);
    if let Some(i) = existing
        && let Act::Pick(pick) = &sheet.rows[i].act
    {
        goals = pick.clone();
    }
    for &goal in offered {
        goals.values.push(goal.title().into());
        goals.details.push("Place a centre marker with a radius of 1.".into());
        goals.choices.push(Choice::Goal(goal));
    }
    let row = SheetRow::new(title, Act::Pick(goals));
    if let Some(i) = existing {
        sheet.rows[i] = row;
    } else {
        let i = sheet.rows.iter().position(|row| row.label == AI_PAGE || row.label == SIEGE_PAGE).unwrap_or(sheet.rows.len());
        sheet.rows.insert(i, row);
        if sheet.focus >= i && sheet.rows.len() > 1 {
            sheet.focus += 1;
        }
    }
    sheet
}

/// The team names a starting point is offered for, by team number.
pub const TEAMS: [&str; 8] = [
    "Red", "Blue", "Yellow", "Green", "Purple", "Orange", "Brown", "Pink",
];

/// An item the kit adds as an item spawner: one of
/// `forge::Session::kit_items`, by its place in that list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitItem {
    pub index: usize,
    pub group: Group,
    pub name: String,
}

/// What a kit session adds to the object menu: the items its spawners can
/// name and how many rows of each block the kit has left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KitMenu {
    pub items: Vec<KitItem>,
    pub items_left: u32,
    pub spawns_left: u32,
    pub flags_left: u32,
}

/// What a row of a kit group says: what it adds and how many more fit.
pub fn kit_line(what: &str, left: u32) -> String {
    let left = match left {
        0 => "no more fit.".to_string(),
        1 => "1 more fits.".to_string(),
        n => format!("{n} more fit."),
    };
    format!("{what}. {left}")
}

/// [`objects`] with a kit session's additions: the kit's items under
/// Weapons, Vehicles and Equipment as item spawners, a Spawns group of
/// starting points by team and a Teleporters group of a sender and a
/// receiver. The rows go in the groups' places, among the map's own.
pub fn with_kit(s: Option<Sheet>, kit: &KitMenu) -> Sheet {
    let mut sheet = s.unwrap_or(Sheet {
        kind: Kind::Objects,
        title: "OBJECTS".into(),
        body: String::new(),
        rows: Vec::new(),
        focus: 0,
    });
    for group in [Group::Weapons, Group::Vehicles, Group::Equipment] {
        let rows: Vec<_> = kit
            .items
            .iter()
            .filter(|i| i.group == group)
            .map(|i| {
                (
                    format!("{} spawner", i.name),
                    kit_line("Item spawner", kit.items_left),
                    Choice::Item(i.index),
                )
            })
            .collect();
        add_pick_rows(&mut sheet, group, rows);
    }
    let spawns = TEAMS
        .iter()
        .enumerate()
        .map(|(team, name)| {
            (
                format!("{name} start"),
                kit_line("Starting point", kit.spawns_left),
                Choice::Spawn(team as u8),
            )
        })
        .collect();
    add_pick_rows(&mut sheet, Group::Spawns, spawns);
    let teleporters = [(true, "Sender"), (false, "Receiver")]
        .into_iter()
        .map(|(sender, name)| {
            (
                name.to_string(),
                kit_line("Teleporter", kit.flags_left),
                Choice::Teleporter(sender),
            )
        })
        .collect();
    add_pick_rows(&mut sheet, Group::Teleporters, teleporters);
    sheet
}

/// Add `rows` (a value, its line and its choice) to the picker of
/// `group`'s row, making the row where the map has none: in the table's
/// order among the groups, before the AI and objectives page and the
/// prefabs.
pub(super) fn add_pick_rows(sheet: &mut Sheet, group: Group, rows: Vec<(String, String, Choice)>) {
    if rows.is_empty() {
        return;
    }
    let title = group.def().name;
    let existing = sheet.rows.iter().position(|row| row.label == title);
    let mut pick = match existing.map(|i| &sheet.rows[i].act) {
        Some(Act::Pick(pick)) => pick.clone(),
        _ => PickSpec {
            title: title.into(),
            values: Vec::new(),
            current: 0,
            details: Vec::new(),
            description: String::new(),
            choices: Vec::new(),
            closes: true,
        },
    };
    for (value, line, choice) in rows {
        pick.values.push(value);
        pick.details.push(line);
        pick.choices.push(choice);
    }
    let count = if pick.values.len() == 1 {
        "1 object.".to_string()
    } else {
        format!("{} objects.", pick.values.len())
    };
    let row = SheetRow::new(title, Act::Pick(pick)).describe(&count);
    if let Some(i) = existing {
        sheet.rows[i] = row;
        return;
    }
    let after = GROUPS
        .iter()
        .skip_while(|g| g.group != group)
        .skip(1)
        .filter_map(|g| sheet.rows.iter().position(|r| r.label == g.name))
        .min();
    let at = after
        .or_else(|| {
            sheet
                .rows
                .iter()
                .position(|r| r.label == AI_PAGE || r.label == SIEGE_PAGE || r.label == PREFABS_PAGE)
        })
        .unwrap_or(sheet.rows.len());
    sheet.rows.insert(at, row);
    if sheet.focus >= at && sheet.rows.len() > 1 {
        sheet.focus += 1;
    }
}

/// Add the entry point for the area editor of a goal with an area: a
/// hill, a territory, a site, a flag return, a bomb goal or a siege goal.
pub fn add_area_row(sheet: &mut Sheet, id: forge::Endpoint) {
    sheet.rows.push(SheetRow::new("Edit area", Act::Choose(Choice::EditArea(id))));
}

/// **Show in play** on a flag's Properties page: On or Off, the value the
/// document gives it (`forge::Session::shown`).
pub fn add_show_row(sheet: &mut Sheet, on: bool) {
    let values = ["On".to_string(), "Off".to_string()];
    sheet.rows.push(text_value_row(
        forge::props::SHOW_IN_PLAY,
        "Whether players see this flag's marker in a match.",
        Prop::ShowInPlay,
        &values,
        usize::from(!on),
    ));
}

/// The typed dimensions of a draft: a circle's radius, a box's two half
/// lengths, a sphere's radius, then the heights of every shape but a
/// sphere, which has none. Closing the page returns to the monitor with
/// the draft still in hand.
pub fn area_fields(area: &game_markers::Area) -> Sheet {
    use game_markers::Shape;
    let mut fields = match &area.shape {
        Shape::Radius(r) | Shape::Sphere(r) => vec![(AreaField::Radius, *r)],
        Shape::Box { half } => vec![(AreaField::Length, half[0]), (AreaField::Width, half[1])],
        Shape::Path(_) => Vec::new(),
    };
    if !matches!(area.shape, Shape::Sphere(_)) {
        fields.extend([(AreaField::Below, area.below), (AreaField::Above, area.above)]);
    }
    let rows = fields.into_iter().map(|(field, value)| {
        let value = number(value);
        SheetRow::new(field.label(), Act::Type(TypeSpec {
            field: NumberField::Area(field),
            initial: value.clone(),
        }))
        .value(&value)
    }).collect();
    Sheet {
        kind: Kind::Properties,
        title: "EDIT AREA".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

/// `angle` in degrees as the rows write it: "45 degrees", "1 degree",
/// "17.5 degrees".
pub fn degrees(angle: f32) -> String {
    let n = format!("{angle:.2}");
    let n = n.trim_end_matches('0').trim_end_matches('.');
    format!("{n} {}", if n == "1" { "degree" } else { "degrees" })
}

/// The heading of [`HEADINGS`] nearest `degrees` (0 to 360).
pub(super) fn nearest_heading(deg: f32) -> usize {
    let d = deg.rem_euclid(360.);
    (0..HEADINGS.len())
        .min_by(|&a, &b| {
            let off = |i: usize| {
                let x = (HEADINGS[i] - d).abs();
                x.min(360. - x)
            };
            off(a).total_cmp(&off(b))
        })
        .unwrap_or(0)
}

/// A value row: its value shown, A opening the picker on its values.
pub(super) fn value_row(
    label: &str,
    description: &str,
    prop: Prop,
    values: &[f32],
    shown: String,
    current: usize,
) -> SheetRow {
    let pick = PickSpec {
        title: label.to_string(),
        values: values.iter().copied().map(degrees).collect(),
        current,
        details: Vec::new(),
        description: description.into(),
        choices: (0..values.len()).map(|i| Choice::Set(prop, i)).collect(),
        closes: false,
    };
    SheetRow::new(label, Act::Pick(pick))
        .value(&shown)
        .describe(description)
}

pub(super) fn int_of(thing: &Thing, name: &str) -> i64 {
    thing
        .fields
        .iter()
        .find(|(field, _)| *field == name)
        .and_then(|(_, value)| match value {
            Value::Int(v) => Some(*v),
            _ => None,
        })
        .unwrap_or(0)
}

/// What the Properties page reads from the session and not from the thing
/// itself: its label, and how many objects Save as prefab would take. With
/// the default the page has no Label or Save as prefab rows
/// (docs/game/forge-editing.md, "Properties").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    /// The thing can be labelled.
    pub labellable: bool,
    /// Its label's text and number.
    pub label: Option<(String, Option<i32>)>,
    /// How many objects Save as prefab saves: the thing's group, or the
    /// selection it is in, or itself; none when one of them is not an
    /// object the session placed.
    pub savable: usize,
    /// The session runs on a map with the kit, so a spawner, a starting
    /// point, a goal and a teleporter can be duplicated.
    pub kit: bool,
    /// An item spawner's team, from the document's property.
    pub team: Option<u8>,
    /// A weapon spawner's spare clips, from the document's property: none
    /// for a thing with no such row, `Some(None)` for the weapon's own
    /// reserve.
    pub spare_clips: Option<Option<u8>>,
    /// A starting point's weight, from the document's property: none for
    /// a thing with no such row, `Some(None)` for Normal.
    pub weight: Option<Option<forge_doc::SpawnWeight>>,
    /// An item spawner's place at start, from the document's property:
    /// none for a thing with no such row.
    pub place_at_start: Option<bool>,
}

/// Properties of `object` (its name, the header), heading `heading` in
/// degrees: the rows of `forge.properties`' doc for the thing's kind, each
/// showing its value ([`super::props::rows`]). `None` when no row is for
/// it or the page has no doc.
pub fn properties_sheet(object: &str, thing: &Thing, heading: f32) -> Option<Sheet> {
    properties_sheet_with(object, thing, heading, &Extras::default())
}

/// [`properties_sheet`] with the rows that read `extras`.
pub fn properties_sheet_with(
    object: &str,
    thing: &Thing,
    heading: f32,
    extras: &Extras,
) -> Option<Sheet> {
    let doc = crate::ui_kit::data::screen(Kind::Properties.screen())?;
    let rows = super::props::rows_with(&doc, thing, heading, extras);
    (!rows.is_empty()).then(|| Sheet {
        kind: Kind::Properties,
        title: object.to_uppercase(),
        body: String::new(),
        rows,
        focus: 0,
    })
}

pub(crate) fn number(v: f32) -> String {
    let text = format!("{v:.4}");
    text.trim_end_matches('0').trim_end_matches('.').into()
}

/// The six coordinates, position in world units and turn in degrees.
/// A opens the keyboard; left and right nudge.
pub fn coordinates_sheet(object: &str, frame: forge::Frame) -> Sheet {
    let rows = Coordinate::ALL.iter().map(|c| {
        let (label, value) = if c.is_angle() {
            (c.label(), number(frame.rotation[c.index()].to_degrees()))
        } else {
            (c.label(), number(frame.position[c.index()]))
        };
        SheetRow::new(
            label,
            Act::Type(TypeSpec {
                field: NumberField::Coordinate(*c),
                initial: value.clone(),
            }),
        )
        .value(&value)
        .describe("Type a number, or nudge with Left and Right.")
        .clone()
    });
    Sheet {
        kind: Kind::Coordinates,
        title: object.to_uppercase(),
        body: String::new(),
        rows: rows.collect(),
        focus: 0,
    }
}

/// A tool of the Tools picker: its name and what it aims at.
pub struct ToolDef {
    pub tool: Tool,
    pub label: &'static str,
    pub description: &'static str,
}

/// The tools that are built, in order (docs/game/forge-editing.md
/// "Tools"). A tool that is not built is absent.
pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        tool: Tool::Objects,
        label: "Objects",
        description: "Place, move and delete objects.",
    },
    ToolDef {
        tool: Tool::Links,
        label: "Links",
        description: "Link teleporters and machines.",
    },
    ToolDef {
        tool: Tool::Nav,
        label: "Nav",
        description: "See the bots' nav and generate it.",
    },
    ToolDef {
        tool: Tool::Ai,
        label: "AI",
        description: "Place drop zones, bases and cover.",
    },
];

/// The Tools picker, the tool in use focused.
pub fn tools(current: Tool) -> Sheet {
    Sheet {
        kind: Kind::Tools,
        title: "TOOLS".into(),
        body: String::new(),
        rows: TOOLS
            .iter()
            .map(|t| {
                SheetRow::new(t.label, Act::Choose(Choice::Tool(t.tool))).describe(t.description)
            })
            .collect(),
        focus: TOOLS.iter().position(|t| t.tool == current).unwrap_or(0),
    }
}

/// A tool setting: its value, label and what it does.
pub struct SettingDef {
    pub prop: Prop,
    pub label: &'static str,
    pub description: &'static str,
}

/// Rotation snap choices in degrees.
pub const ANGLE_STEPS: [f32; 4] = [1., 5., 15., 45.];
/// Absolute object yaw in degrees about the world's vertical axis.
pub const HEADINGS: [f32; 8] = [0., 45., 90., 135., 180., 225., 270., 315.];

/// Movement snap choices, world units; 0 is off.
pub const MOVE_STEPS: &[f32] = &[0.0, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0];

/// The tool settings, in order (docs/game/forge-editing.md "Precision").
pub const SETTINGS: &[SettingDef] = &[
    SettingDef {
        prop: Prop::RotationSnap,
        label: "Rotation snap",
        description: "How far each turn of a held object goes.",
    },
    SettingDef {
        prop: Prop::MovementSnap,
        label: "Movement snap",
        description: "The grid a held object's position lands on.",
    },
    SettingDef {
        prop: Prop::Axes,
        label: "Axes",
        description: "World axes, or the object's own axes at pickup.",
    },
    SettingDef {
        prop: Prop::BlockGrid,
        label: "Block-out grid",
        description: "The grid, in units of a hundredth of a world unit, a block-out piece lands on.",
    },
];

/// The block-out grids in units, as `forge_shapes::units::GRIDS`.
pub const BLOCK_GRIDS: [u16; 7] = forge_shapes::units::GRIDS;

/// The index of the default block-out grid in [`BLOCK_GRIDS`].
pub const DEFAULT_GRID_INDEX: usize = 4;

fn snap_text(v: f32, unit: &str) -> String {
    if v == 0.0 {
        "Off".into()
    } else if unit.is_empty() {
        format!("{v}")
    } else {
        format!("{v} {unit}")
    }
}

pub(super) fn text_value_row(
    label: &str,
    description: &str,
    prop: Prop,
    values: &[String],
    current: usize,
) -> SheetRow {
    let pick = PickSpec {
        title: label.into(),
        values: values.to_vec(),
        current,
        details: Vec::new(),
        description: description.into(),
        choices: (0..values.len()).map(|i| Choice::Set(prop, i)).collect(),
        closes: false,
    };
    SheetRow::new(label, Act::Pick(pick))
        .value(&values[current])
        .describe(description)
}

/// The Tool settings page: [`SETTINGS`]' rows, `rotation_snap` an index
/// into [`ANGLE_STEPS`], `movement_snap` into [`MOVE_STEPS`], and
/// `local_axes` whether Local is selected.
pub fn settings(rotation_snap: usize, movement_snap: usize, local_axes: bool) -> Sheet {
    settings_with(rotation_snap, movement_snap, local_axes, DEFAULT_GRID_INDEX)
}

/// [`settings`] with the block-out grid, an index into [`BLOCK_GRIDS`].
pub fn settings_with(
    rotation_snap: usize,
    movement_snap: usize,
    local_axes: bool,
    block_grid: usize,
) -> Sheet {
    let rows = SETTINGS
        .iter()
        .map(|d| match d.prop {
            Prop::RotationSnap => {
                let i = rotation_snap.min(ANGLE_STEPS.len() - 1);
                let values: Vec<String> = ANGLE_STEPS
                    .iter()
                    .map(|v| snap_text(*v, "degrees"))
                    .collect();
                text_value_row(d.label, d.description, d.prop, &values, i)
            }
            Prop::MovementSnap => {
                let i = movement_snap.min(MOVE_STEPS.len() - 1);
                let values: Vec<String> = MOVE_STEPS.iter().map(|v| snap_text(*v, "")).collect();
                text_value_row(d.label, d.description, d.prop, &values, i)
            }
            Prop::Axes => {
                let values = vec!["World".into(), "Local".into()];
                text_value_row(
                    d.label,
                    d.description,
                    d.prop,
                    &values,
                    usize::from(local_axes),
                )
            }
            Prop::BlockGrid => {
                let i = block_grid.min(BLOCK_GRIDS.len() - 1);
                let values: Vec<String> = BLOCK_GRIDS.iter().map(|v| v.to_string()).collect();
                text_value_row(d.label, d.description, d.prop, &values, i)
            }
            _ => unreachable!("a property, not a tool setting"),
        })
        .collect();
    Sheet {
        kind: Kind::Settings,
        title: "TOOL SETTINGS".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

/// An overlay of the Overlays page: its name and what it draws.
pub struct OverlayDef {
    pub overlay: forge::tool::Overlay,
    pub label: &'static str,
    pub description: &'static str,
}

/// The overlays, in order (docs/game/forge-editing.md "Overlays"). An
/// overlay that is not built is absent.
pub const OVERLAYS: &[OverlayDef] = &[
    OverlayDef {
        overlay: forge::tool::Overlay::Markers,
        label: "Markers",
        description: "A marker at each spawn, flag, hill, teleporter and item spawner.",
    },
    OverlayDef {
        overlay: forge::tool::Overlay::Monitors,
        label: "Other monitors",
        description: "A ring at each other player's monitor.",
    },
    OverlayDef {
        overlay: forge::tool::Overlay::Gizmo,
        label: "Axes",
        description: "The axes and outline at the held or aimed object.",
    },
    OverlayDef {
        overlay: forge::tool::Overlay::Nav,
        label: "Nav problems",
        description: "A mark at each problem the last Generate nav found.",
    },
    OverlayDef {
        overlay: forge::tool::Overlay::Blockout,
        label: "Block-out",
        description: "Barriers and Nodraw faces, in their family's colour.",
    },
];

/// The Overlays page: a row for each of [`OVERLAYS`], showing On or Off;
/// A opens the picker on the two. `on` is [`forge::tool::Overlays`].
pub fn overlays(on: forge::tool::Overlays) -> Sheet {
    let values = vec!["On".to_string(), "Off".to_string()];
    Sheet {
        kind: Kind::Settings,
        title: "OVERLAYS".into(),
        body: String::new(),
        rows: OVERLAYS
            .iter()
            .map(|d| {
                text_value_row(
                    d.label,
                    d.description,
                    Prop::Overlay(d.overlay),
                    &values,
                    usize::from(!on.on(d.overlay)),
                )
            })
            .collect(),
        focus: 0,
    }
}

/// [`overlays`] with the **Dev view** row after the overlays: the block-out
/// pieces drawn with their dev textures, On or Off.
pub fn overlays_with(on: forge::tool::Overlays, dev_view: bool) -> Sheet {
    let mut sheet = overlays(on);
    let values = vec!["On".to_string(), "Off".to_string()];
    sheet.rows.push(text_value_row(
        "Dev view",
        "Draw every block-out piece with its dev texture, whatever its material.",
        Prop::DevView,
        &values,
        usize::from(!dev_view),
    ));
    sheet
}

/// A row of Forge options: its label, its help (what it opens, for the
/// help block under the rows when the page is the shared settings dialog,
/// docs/game/ui-builder.md "The settings dialog") and the screen it opens.
pub struct OptionDef {
    pub label: &'static str,
    pub description: &'static str,
    pub opens: Kind,
}

/// Forge options, in order (docs/game/forge-firefight-ui.md "The screens").
/// The placed objects browser and the actions follow these pages.
pub const OPTIONS: &[OptionDef] = &[
    OptionDef {
        label: "Tool settings...",
        description: "Rotation snap and the other tool settings.",
        opens: Kind::Settings,
    },
    OptionDef {
        label: "Controls",
        description: "Every Forge control, with its button.",
        opens: Kind::Controls,
    },
    OptionDef {
        label: "Map variants...",
        description: "Load, rename, copy or delete this map's variants.",
        opens: Kind::Variants,
    },
    OptionDef {
        label: "Map limits",
        description: "See the object pool and scenario counts.",
        opens: Kind::MapLimits,
    },
];

/// The Forge options page.
pub fn options() -> Sheet {
    options_with(&[])
}

/// An added object of the map, a row of the placed objects browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub id: forge::OpId,
    pub group: Group,
    /// What the row says: the object's name, numbered when the map has
    /// more than one of it ("Beacon 2").
    pub name: String,
    /// The row's line in the picker: its group and where it stands.
    pub line: String,
}

/// The browser's rows for `objects` (an id, a group, a name and a
/// position each, in the order the session holds them): grouped as the
/// object menu groups ([`GROUPS`]), then by name, then in the order
/// placed; an object of a name the map holds more than once is numbered
/// in that order.
pub fn placed_rows(objects: &[(forge::OpId, Group, String, [f32; 3])]) -> Vec<Placed> {
    let mut rows: Vec<(usize, &(forge::OpId, Group, String, [f32; 3]))> =
        objects.iter().enumerate().collect();
    rows.sort_by(|(ia, a), (ib, b)| (a.1, &a.2, ia).cmp(&(b.1, &b.2, ib)));
    let mut out = Vec::with_capacity(rows.len());
    for (i, (_, (id, group, name, at))) in rows.iter().enumerate() {
        let same = |j: usize| rows[j].1.1 == *group && rows[j].1.2 == *name;
        let first = (0..=i).find(|j| same(*j)).unwrap_or(i);
        let many = first < i || rows.get(i + 1).is_some_and(|_| same(i + 1));
        out.push(Placed {
            id: *id,
            group: *group,
            name: if many {
                format!("{name} {}", i - first + 1)
            } else {
                name.clone()
            },
            line: format!(
                "{}. At {}, {}, {}.",
                group.def().one,
                number(at[0]),
                number(at[1]),
                number(at[2])
            ),
        });
    }
    out
}

/// A group of the map, a folder row of the placed objects browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// The op that made the group.
    pub id: forge::OpId,
    /// The group's name.
    pub name: String,
    /// What the row says in the picker: how many objects, and whether the
    /// group is locked.
    pub line: String,
}

/// The browser's folder rows for the active groups, in the order made.
pub fn folder_rows(groups: &[forge::GroupInfo]) -> Vec<Folder> {
    groups
        .iter()
        .map(|g| Folder {
            id: g.id,
            name: g.name.clone(),
            line: format!(
                "Group. {} objects{}.",
                g.members.len(),
                if g.locked { ", locked" } else { "" }
            ),
        })
        .collect()
}

/// The Placed objects row: A opens a picker over every group, then every
/// added object, each flying the monitor to it. With none placed and no
/// group it is a fact.
pub fn placed_row(placed: &[Placed]) -> SheetRow {
    placed_row_with(&[], placed)
}

/// [`placed_row`] with the groups listed first.
pub fn placed_row_with(folders: &[Folder], placed: &[Placed]) -> SheetRow {
    const HELP: &str = "Fly the monitor to a group or an object you placed.";
    if placed.is_empty() && folders.is_empty() {
        return SheetRow::new("Placed objects", Act::Info)
            .value("None")
            .describe("Nothing is placed yet. Place one from the object menu.");
    }
    let pick = PickSpec {
        title: "Placed objects".into(),
        values: folders
            .iter()
            .map(|f| f.name.clone())
            .chain(placed.iter().map(|p| p.name.clone()))
            .collect(),
        current: 0,
        details: folders
            .iter()
            .map(|f| f.line.clone())
            .chain(placed.iter().map(|p| p.line.clone()))
            .collect(),
        description: String::new(),
        choices: folders
            .iter()
            .map(|f| Choice::FlyToGroup(f.id))
            .chain(placed.iter().map(|p| Choice::FlyTo(p.id)))
            .collect(),
        closes: false,
    };
    SheetRow::new("Placed objects", Act::Pick(pick))
        .value(&(folders.len() + placed.len()).to_string())
        .describe(HELP)
}

/// What the Properties page knows of selection and groups for the thing
/// it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupFacts {
    /// The thing the page is on.
    pub thing: forge::Endpoint,
    /// The thing is in the selection.
    pub selected: bool,
    /// How many things are selected.
    pub selection: usize,
    /// The thing's active group.
    pub group: Option<forge::GroupInfo>,
    /// The session can change the pivot as one edit.
    pub pivot_editable: bool,
}

/// The Properties page's rows for selecting and grouping, after the doc's
/// rows. A member can edit its group. A free thing can be selected, and
/// two or more selected things can be grouped.
pub fn group_rows(f: &GroupFacts) -> Vec<SheetRow> {
    if let Some(group) = &f.group {
        let mut rows = vec![
            SheetRow::new("Group", Act::Info)
                .value(&group.name)
                .describe("The group this object moves with."),
            SheetRow::new(
                if group.locked {
                    "Unlock group"
                } else {
                    "Lock group"
                },
                Act::Choose(Choice::LockGroup(group.id, !group.locked)),
            )
            .describe(if group.locked {
                "Aim at the group's objects again."
            } else {
                "Stop aiming at the group's objects."
            }),
        ];
        if f.pivot_editable {
            let values = ["Centre", "First member"];
            let current = usize::from(group.pivot_first);
            const HELP: &str = "Choose where the group turns.";
            let pick = PickSpec {
                title: "Group pivot".into(),
                values: values.map(String::from).to_vec(),
                current,
                details: vec![],
                description: HELP.into(),
                choices: vec![
                    Choice::SetGroupPivot(group.id, false),
                    Choice::SetGroupPivot(group.id, true),
                ],
                closes: true,
            };
            rows.push(
                SheetRow::new("Pivot", Act::Pick(pick))
                    .value(values[current])
                    .describe(HELP),
            );
        }
        rows.push(
            SheetRow::new("Ungroup", Act::Choose(Choice::Ungroup(group.id)))
                .describe("Let the objects move on their own."),
        );
        return rows;
    }
    let mut rows = vec![
        SheetRow::new(
            if f.selected { "Deselect" } else { "Select" },
            Act::Choose(Choice::ToggleSelected(f.thing)),
        )
        .describe("Choose this object to group with others."),
    ];
    if f.selection >= forge::MIN_GROUP_MEMBERS {
        rows.push(
            SheetRow::new("Group selection", Act::Choose(Choice::GroupSelection))
                .value(&f.selection.to_string())
                .describe("Move the selected objects as one."),
        );
    }
    rows
}

/// The Forge options page with the placed objects browser over `placed`
/// ([`placed_rows`]).
pub fn options_with(placed: &[Placed]) -> Sheet {
    options_with_groups(&[], placed)
}

/// [`options_with`] with the groups' folders listed first.
pub fn options_with_groups(folders: &[Folder], placed: &[Placed]) -> Sheet {
    options_for(folders, placed, false)
}

/// [`options_with_groups`] while a wave test runs or not (`testing`): the
/// Test waves row reads **Stop waves** while one does
/// (docs/game/forge-firefight.md, "Test waves").
pub fn options_for(folders: &[Folder], placed: &[Placed], testing: bool) -> Sheet {
    options_with_waves(folders, placed, testing, false, false)
}

/// [`options_for`] with **Waves...** when the session has waves to edit
/// (`waves`) and **Enemy types...** when the recipe names classes
/// (`enemy_types`), after the placed objects browser
/// (docs/game/forge-firefight-ui.md, "The Waves screens").
pub fn options_with_waves(
    folders: &[Folder],
    placed: &[Placed],
    testing: bool,
    waves: bool,
    enemy_types: bool,
) -> Sheet {
    let test = if testing {
        SheetRow::new("Stop waves", Act::Choose(Choice::StopWaves))
            .describe("Erase the test's enemies and stop the waves.")
    } else {
        SheetRow::new("Test waves", Act::Choose(Choice::TestWaves))
            .describe("Fight the waves here, building the AI ground first if needed.")
    };
    Sheet {
        kind: Kind::Options,
        title: "FORGE OPTIONS".into(),
        body: String::new(),
        rows: OPTIONS
            .iter()
            .map(|o| SheetRow::new(o.label, Act::Open(o.opens)).describe(o.description))
            .chain([placed_row_with(folders, placed)])
            .chain(waves.then(|| {
                SheetRow::new("Waves...", Act::Open(Kind::Waves))
                    .describe("The rounds, waves and squads this map plays.")
            }))
            .chain(enemy_types.then(|| {
                SheetRow::new("Enemy types...", Act::Open(Kind::EnemyTypes))
                    .describe("The enemies the AI ground builds.")
            }))
            .chain([
                SheetRow::new("Build AI ground", Act::Choose(Choice::BuildGround))
                    .describe("Build the nav mesh and zones for the placed labels now."),
                test,
                SheetRow::new("Generate nav", Act::Choose(Choice::GenerateNav))
                    .describe("Build the bots' nav with the placed objects in."),
            ])
            .collect(),
        focus: 0,
    }
}

/// A link's dialog: its two ends and what it writes in the body; **Remove
/// link** and **Done**, Done focused.
pub fn link(from: &str, to: &str, writes: &str) -> Sheet {
    Sheet {
        kind: Kind::Link,
        title: "LINK".into(),
        body: format!("From {from}\nTo {to}\n{writes}"),
        rows: vec![
            SheetRow::new("Remove link", Act::Choose(Choice::Unlink)),
            SheetRow::new("Done", Act::Close),
        ],
        focus: 1,
    }
}

/// The question `ask`, about `name` (the variant's name for an overwrite),
/// **Yes** and **No**, No focused.
pub fn question(ask: Ask, name: &str) -> Sheet {
    let (title, body) = match ask {
        Ask::DeleteLinked => (
            "DELETE OBJECT",
            "This object has links. Delete it and its links?".to_string(),
        ),
        Ask::Restore => (
            "RESTORE ORIGINAL",
            "Put this back as the map has it? Your changes to it are lost.".to_string(),
        ),
        Ask::Overwrite => ("SAVE AS", super::variants::replace_question(name)),
        Ask::Leave => (
            "LEAVE GAME",
            "This Forge map has edits that are not saved. Leave without saving?".to_string(),
        ),
        Ask::Load => ("LOAD", super::variants::LOAD_QUESTION.to_string()),
        Ask::DeleteVariant => ("DELETE", super::variants::delete_question(name)),
        Ask::Waves(ask) => super::waves::question_text(ask, name),
    };
    let answer = |yes| Act::Choose(Choice::Answer { ask, yes });
    Sheet {
        kind: Kind::Question,
        title: title.into(),
        body,
        rows: vec![
            SheetRow::new("Yes", answer(true)),
            SheetRow::new("No", answer(false)),
        ],
        focus: 1,
    }
}

/// The Controls page: a row a control of the bindings table, in its order,
/// its pad button in the value column and its keys in the description.
pub fn controls() -> Sheet {
    let rows = crate::forge_controls::bindings::page()
        .into_iter()
        .map(|(what, pad, keys)| {
            SheetRow::new(&what, Act::Info)
                .value(&pad)
                .describe(&format!("With a keyboard and mouse, {keys}."))
        })
        .collect();
    Sheet {
        kind: Kind::Controls,
        title: "CONTROLS".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}
