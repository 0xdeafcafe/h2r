//! What each Forge screen shows, as data (docs/game/forge-firefight-ui.md
//! "The screens", docs/game/forge-editing.md "The object menu" and
//! "Properties"). A [`Sheet`] is a screen's title, body, rows and focus.
//! The builders here make one from the session's facts, and tables decide
//! the rows: the object menu's groups ([`GROUPS`]), the Properties rows
//! (the doc's, [`super::props`]), the tools ([`TOOLS`]), the settings
//! ([`SETTINGS`]), Forge options ([`OPTIONS`]) and the controls (the
//! bindings table). A change of design is a change of table.

use forge::{BlockKind, Thing, Value};

use crate::ui_kit::forge::{ANGLE_STEPS, HEADINGS};
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
    /// The game types of an item spawner, ticked a row each.
    Games,
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
    pub const ALL: [Kind; 10] = [
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
        }
    }

    pub const fn look(self) -> Look {
        match self {
            Kind::Objects | Kind::Tools | Kind::Games => Look::Picker,
            Kind::Properties
            | Kind::Coordinates
            | Kind::Settings
            | Kind::Options
            | Kind::Controls => Look::List,
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
    /// Save as over a saved map of that name.
    Overwrite,
    /// Leave Game with unsaved edits.
    Leave,
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
    /// Open the six-coordinate editor.
    Coordinates,
    /// A spawn or goal's team.
    Team,
    /// A goal's netgame flag type.
    Role,
    /// A goal's identifier.
    Order,
    /// An item entry's respawn time.
    RespawnTime,
}

/// A tool of the Tools picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Objects,
    Links,
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

/// A typed number's destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberField {
    Coordinate(Coordinate),
    RespawnTime,
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
    /// **Build AI ground**.
    BuildGround,
    /// **Test waves**: build the ground when it is not current.
    TestWaves,
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
    Save,
    SaveAs(String),
    Load(String),
    /// The Game types dialog's Done: give the aimed or held item spawner
    /// this set, as one step.
    Games(forge_doc::games::GameSet),
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
    /// The focused row's line in the description block.
    pub description: Option<String>,
    pub act: Act,
}

impl SheetRow {
    pub(super) fn new(label: &str, act: Act) -> Self {
        SheetRow {
            label: label.into(),
            value: None,
            description: None,
            act,
        }
    }
    pub(super) fn value(mut self, v: &str) -> Self {
        self.value = Some(v.into());
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
                .filter(|(_, i)| i.group == g.group)
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

/// [`objects`] with the AI and objectives page after the map's groups: one
/// row, opening the value picker on the labels, each choice a
/// [`Choice::Marker`]. With no labels it is [`objects`], and a map with
/// nothing to place still has the page.
pub fn objects_with_ai(items: &[Item], selected: usize, ai: &[AiItem]) -> Option<Sheet> {
    let sheet = objects(items, selected);
    if ai.is_empty() {
        return sheet;
    }
    let pick = PickSpec {
        title: AI_PAGE.into(),
        values: ai.iter().map(|a| a.label.title().to_string()).collect(),
        current: 0,
        details: ai.iter().map(ai_line).collect(),
        description: String::new(),
        choices: ai.iter().map(|a| Choice::Marker(a.label)).collect(),
        closes: true,
    };
    let page = SheetRow::new(AI_PAGE, Act::Pick(pick)).describe(&format!("{} markers.", ai.len()));
    let mut sheet = sheet.unwrap_or(Sheet {
        kind: Kind::Objects,
        title: "OBJECTS".into(),
        body: String::new(),
        rows: Vec::new(),
        focus: 0,
    });
    sheet.rows.push(page);
    Some(sheet)
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

/// Properties of `object` (its name, the header), heading `heading` in
/// degrees: the rows of `forge.properties`' doc for the thing's kind, each
/// showing its value ([`super::props::rows`]). `None` when no row is for
/// it or the page has no doc.
pub fn properties_sheet(object: &str, thing: &Thing, heading: f32) -> Option<Sheet> {
    let doc = crate::ui_kit::data::screen(Kind::Properties.screen())?;
    let rows = super::props::rows(&doc, thing, heading);
    (!rows.is_empty()).then(|| Sheet {
        kind: Kind::Properties,
        title: object.to_uppercase(),
        body: String::new(),
        rows,
        focus: 0,
    })
}

pub(super) fn number(v: f32) -> String {
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
];

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

/// A row of Forge options: its label, its help (what it opens, for the
/// help block under the rows when the page is the shared settings dialog,
/// docs/game/ui-builder.md "The settings dialog") and the screen it opens.
pub struct OptionDef {
    pub label: &'static str,
    pub description: &'static str,
    pub opens: Kind,
}

/// Forge options, in order (docs/game/forge-firefight-ui.md "The screens").
/// Monitor look joins when it is built; Placed objects and Map limits in
/// P4, Generate nav in F9, Players in F5, Map settings and Publish in F14.
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
];

/// The Forge options page.
pub fn options() -> Sheet {
    Sheet {
        kind: Kind::Options,
        title: "FORGE OPTIONS".into(),
        body: String::new(),
        rows: OPTIONS
            .iter()
            .map(|o| SheetRow::new(o.label, Act::Open(o.opens)).describe(o.description))
            .chain([
                SheetRow::new("Build AI ground", Act::Choose(Choice::BuildGround))
                    .describe("Build the nav mesh and zones for the placed labels now."),
                SheetRow::new("Test waves", Act::Choose(Choice::TestWaves))
                    .describe("Build the AI ground if it is stale, then test."),
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

/// The question `ask`, about `name` (the map's name for an overwrite),
/// **Yes** and **No**, No focused.
pub fn question(ask: Ask, name: &str) -> Sheet {
    let (title, body) = match ask {
        Ask::DeleteLinked => (
            "DELETE OBJECT",
            "This object has links. Delete it and its links?".to_string(),
        ),
        Ask::Overwrite => (
            "SAVE AS",
            format!("A Forge map named {name} is saved. Replace it?"),
        ),
        Ask::Leave => (
            "LEAVE GAME",
            "This Forge map has edits that are not saved. Leave without saving?".to_string(),
        ),
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
