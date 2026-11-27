//! The Forge menu (docs/game/forge-firefight-ui.md "The screens", the
//! Forge menu): Back's screen while the player flies the monitor, a list
//! page on the in-game Controller Settings host titled FORGE, its pages
//! tabs across the top.
//!
//! - **Tabs** ([`Tab`]): Options, Map variants, Waves, Objects and Limits.
//!   LB and RB (the game's White and Black) step to the tab before and
//!   after, round the ends; Left and Right do the same, for a keyboard.
//!   The rows change in place ([`sheet`]): the screen stays up and the
//!   list grows or shrinks to the tab's rows.
//! - **The strip** ([`panel`], [`strip`]): each tab has a bright and a
//!   muted text in the same place, the one not showing empty, as Theater's
//!   library tabs are drawn; the bumpers' glyphs stand at its ends.
//! - **Which tab** a sheet shows is its body ([`tab_of`]), which a list
//!   page never draws: a sheet says everything its screen shows.
//! - **Only in the monitor.** The bindings table gives Back to the menu
//!   while the monitor flies (`crate::forge_controls`); in the player's
//!   own body Back is the game's scoreboard, and a swap back to the body
//!   closes the menu (`super::publish`).

use super::sheet::{Act, Choice, Kind, PickSpec, Sheet, SheetRow, placed_row_with};
use super::{Facts, map_limits, variants};
use crate::list_class::{BUTTON_EVENT, LEFT_EVENT, Panel, PanelText, RIGHT_EVENT, TextBlock};
use crate::mods_screen::page_layout as lobby;
use crate::ui_kit::spec::PaneText;

/// The menu's title.
pub const TITLE: &str = "FORGE";

/// A page of the Forge menu, in the strip's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Options,
    Variants,
    Waves,
    Placed,
    Limits,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Options,
        Tab::Variants,
        Tab::Waves,
        Tab::Placed,
        Tab::Limits,
    ];

    /// Its name on the strip.
    pub const fn label(self) -> &'static str {
        match self {
            Tab::Options => "Options",
            Tab::Variants => "Map variants",
            Tab::Waves => "Waves",
            Tab::Placed => "Objects",
            Tab::Limits => "Limits",
        }
    }

    /// Its sample's name for `h2 ui preview forge.menu --state`.
    pub const fn state(self) -> &'static str {
        match self {
            Tab::Options => "options",
            Tab::Variants => "variants",
            Tab::Waves => "waves",
            Tab::Placed => "objects",
            Tab::Limits => "limits",
        }
    }

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).expect("a tab of ALL")
    }

    /// The tab after (`right`) or before, round the ends.
    pub fn next(self, right: bool) -> Tab {
        let n = Tab::ALL.len();
        Tab::ALL[(self.index() + if right { 1 } else { n - 1 }) % n]
    }

    /// The left edge and the width of its cell on the strip.
    const fn cell(self) -> (i16, i16) {
        match self {
            Tab::Options => (-228, 70),
            Tab::Variants => (-150, 112),
            Tab::Waves => (-30, 60),
            Tab::Placed => (38, 72),
            Tab::Limits => (118, 64),
        }
    }
}

/// The game's Black (the right bumper) and White (the left) as a button
/// event's word, after A, B, X and Y (`crate::list_class::BUTTON_EVENT`).
const BLACK_BUTTON: u32 = 4;
const WHITE_BUTTON: u32 = 5;
/// The bumpers' glyphs in the fonts (docs/input/button-icons.md).
const LB: &str = "\u{e105}";
const RB: &str = "\u{e104}";

/// The step an event asks of the tabs: right (true) or left; `None` for an
/// event the menu leaves to its list.
pub fn turn_of(kind: u32, button: u32) -> Option<bool> {
    match (kind, button) {
        (BUTTON_EVENT, WHITE_BUTTON) | (LEFT_EVENT, _) => Some(false),
        (BUTTON_EVENT, BLACK_BUTTON) | (RIGHT_EVENT, _) => Some(true),
        _ => None,
    }
}

/// The strip's top and bottom: under the title (the header layout's
/// rectangle ends at 180) and over the first row, whose focus frame rises
/// to 160 from the list's anchor at 130.
const STRIP_TOP: i16 = 180;
const STRIP_BOTTOM: i16 = 160;
/// The bumpers' cells, left and right of the tabs: left edge and width.
const LB_CELL: (i16, i16) = (-254, 20);
const RB_CELL: (i16, i16) = (190, 20);

/// The strip's texts: a bright and a muted one per tab, then the two
/// bumpers.
pub const STRIP_TEXTS: usize = 2 * Tab::ALL.len() + 2;

fn cell((left, width): (i16, i16), bright: bool) -> PanelText {
    PanelText {
        block: TextBlock {
            rect: [STRIP_TOP, left, STRIP_BOTTOM, left + width],
            colour: if bright { lobby::WHITE } else { lobby::PALE_BLUE },
            ..lobby::BODY
        },
        right: false,
        bias: lobby::TEXT_BIAS,
    }
}

/// The strip as the class's panel: text `2 * i` is tab `i` bright, the
/// next the same tab muted, then LB and RB.
pub fn panel() -> Panel {
    let mut texts: Vec<PanelText> = Tab::ALL
        .into_iter()
        .flat_map(|t| [cell(t.cell(), true), cell(t.cell(), false)])
        .collect();
    texts.push(cell(LB_CELL, false));
    texts.push(cell(RB_CELL, false));
    Panel {
        art: Vec::new(),
        texts,
    }
}

/// What each of the strip's texts reads with `tab` showing: its name in
/// capitals on its bright text, every other tab's on its muted one, the
/// rest empty.
pub fn strip(tab: Tab) -> Vec<String> {
    let mut v: Vec<String> = Tab::ALL
        .into_iter()
        .flat_map(|t| {
            let on = t == tab;
            [
                if on { t.label().to_uppercase() } else { String::new() },
                if on { String::new() } else { t.label().to_string() },
            ]
        })
        .collect();
    v.push(LB.into());
    v.push(RB.into());
    v
}

/// The strip as the screen's spec draws it, for the preview and the
/// checks.
pub fn pane_texts(tab: Tab) -> Vec<PaneText> {
    panel()
        .texts
        .iter()
        .zip(strip(tab))
        .enumerate()
        .map(|(i, (t, text))| PaneText {
            id: format!("forge.menu.tab.{i}"),
            rect: t.block.rect,
            font: t.block.font,
            colour: t.block.colour,
            right: t.right,
            text,
        })
        .collect()
}

/// The tab a Forge menu sheet shows: its body names it. A sheet that
/// names none (the screen before its first opening) is the first tab's.
pub fn tab_of(sheet: &Sheet) -> Tab {
    Tab::ALL
        .into_iter()
        .find(|t| t.label() == sheet.body)
        .unwrap_or_default()
}

/// The Forge menu on `tab`, from the session's facts.
pub fn sheet(tab: Tab, f: &Facts) -> Sheet {
    let rows = match tab {
        Tab::Options => options(f),
        Tab::Variants => variant_rows(f),
        Tab::Waves => wave_rows(f),
        Tab::Placed => vec![placed_row_with(&f.folders, &f.placed)],
        Tab::Limits => map_limits::sheet(&f.map_limits).rows,
    };
    Sheet {
        kind: Kind::Menu,
        title: TITLE.into(),
        body: tab.label().into(),
        rows,
        focus: 0,
    }
}

/// Options: Rebuild map with the pending count as its value, the tool
/// settings, the controls and the bots' nav.
fn options(f: &Facts) -> Vec<SheetRow> {
    let rebuild = SheetRow::new("Rebuild map", Act::Choose(Choice::Rebuild))
        .describe("Build the map from your edits and load it.");
    vec![
        match f.pending_rebuild {
            0 => rebuild,
            n => rebuild.value(&n.to_string()),
        },
        SheetRow::new("Tool settings...", Act::Open(Kind::Settings))
            .describe("Rotation snap and the other tool settings."),
        SheetRow::new("Controls", Act::Open(Kind::Controls))
            .describe("Every Forge control, with its button."),
        SheetRow::new("Generate nav", Act::Choose(Choice::GenerateNav))
            .describe("Build the bots' nav with the placed objects in."),
    ]
}

/// Map variants: Save (the keyboard first for a session with no name),
/// Save as, Load (a picker over this map's variants that load) and the
/// page that renames, copies and deletes them.
fn variant_rows(f: &Facts) -> Vec<SheetRow> {
    let save = match variants::save(f.filename.as_deref()) {
        variants::SaveStep::Keyboard => SheetRow::new("Save", Act::SaveAs)
            .describe("Name this map variant and save it."),
        variants::SaveStep::Write { name, .. } => {
            SheetRow::new("Save", Act::Choose(Choice::Save)).describe(&format!("Save {name}."))
        }
    };
    let loadable = variants::loadable(&f.variants);
    let load = if loadable.is_empty() {
        SheetRow::new("Load", Act::Info)
            .value("None")
            .describe(variants::NONE_SAVED)
    } else {
        let current = f
            .filename
            .as_deref()
            .and_then(|n| {
                loadable
                    .iter()
                    .position(|r| variants::same_name(&r.name, n))
            })
            .unwrap_or(0);
        SheetRow::new(
            "Load",
            Act::Pick(PickSpec {
                title: "Load".into(),
                values: loadable.iter().map(|r| r.name.clone()).collect(),
                current,
                details: Vec::new(),
                description: "A map variant of this map.".into(),
                choices: loadable
                    .iter()
                    .map(|r| Choice::Load(r.file.clone()))
                    .collect(),
                // The menu closes with the pick: Load asks before it
                // replaces edits not saved, and its question needs no
                // screen of ours up.
                closes: true,
            }),
        )
        .describe("Open another map variant of this map.")
    };
    vec![
        save,
        SheetRow::new("Save as", Act::SaveAs).describe("Save a copy under a new name."),
        load,
        SheetRow::new("Map variants...", Act::Open(Kind::Variants))
            .describe("Load, rename, copy or delete this map's variants."),
    ]
}

/// Waves: the test (Stop waves while one runs), the Waves screens while
/// the session has waves to edit, Enemy types while the recipe names
/// classes, and the AI ground.
fn wave_rows(f: &Facts) -> Vec<SheetRow> {
    let test = if f.testing {
        SheetRow::new("Stop waves", Act::Choose(Choice::StopWaves))
            .describe("Erase the test's enemies and stop the waves.")
    } else {
        SheetRow::new("Test waves", Act::Choose(Choice::TestWaves))
            .describe("Fight the waves here, building the AI ground first if needed.")
    };
    std::iter::once(test)
        .chain(f.waves.is_some().then(|| {
            SheetRow::new("Waves...", Act::Open(Kind::Waves))
                .describe("The rounds, waves and squads this map plays.")
        }))
        .chain((!f.enemy_types.is_empty()).then(|| {
            SheetRow::new("Enemy types...", Act::Open(Kind::EnemyTypes))
                .describe("The enemies the AI ground builds.")
        }))
        .chain([
            SheetRow::new("Build AI ground", Act::Choose(Choice::BuildGround))
                .describe("Build the nav mesh and zones for the placed labels now."),
        ])
        .collect()
}

/// Sample facts for the preview: a named session with edits to rebuild,
/// saved variants, waves and placed objects.
pub fn sample_facts() -> Facts {
    Facts {
        active: true,
        monitor: true,
        pending_rebuild: 2,
        filename: Some("Courtyard".into()),
        variants: super::pages::sample_variants(),
        placed: super::sheet::placed_rows(&[
            (forge::OpId(2), super::sheet::Group::Crates, "Barrel".into(), [0., 0., 0.5]),
            (forge::OpId(5), super::sheet::Group::Scenery, "Beacon".into(), [-4.25, 0., 0.]),
        ]),
        map_limits: map_limits::Usage::sample(),
        waves: Some(super::waves::sample_facts()),
        enemy_types: super::waves::sample_enemy_types(),
        ..Facts::default()
    }
}
