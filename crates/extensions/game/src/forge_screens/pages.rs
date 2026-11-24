//! The Forge screens offline: each a [`Page`] over its real class
//! ([`ListPage`] on a [`ForgeScreen`]) showing sample sheets, for
//! `h2 ui preview forge.*`, the sheet and the table tests.

use std::sync::Arc;

use super::screen::ForgeScreen;
use super::sheet::{self, Ask, Group, Item, Kind, Sheet, Tool};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Driven, Focus, ListEffect, ListPage, Live, Page, described, focus_line};
use crate::ui_kit::spec::{Describe, ScreenSpec};

/// A sample palette: three groups, unsorted, as a map lists its blocks.
pub fn sample_items() -> Vec<Item> {
    [
        (Group::Machines, "Gate", 0, 12),
        (Group::Scenery, "Beacon", 2, 12),
        (Group::Machines, "Button", 1, 12),
        (Group::Scenery, "Lantern", 0, 12),
        (Group::Scenery, "Marker", 0, 12),
        (Group::Scenery, "Panel", 0, 12),
        (Group::Scenery, "Pillar", 0, 12),
        (Group::Scenery, "Ramp", 0, 1),
        (Group::Scenery, "Tray", 0, 0),
        (Group::Crates, "Barrel", 3, 9),
    ]
    .into_iter()
    .map(|(group, name, placed, left)| Item {
        group,
        name: name.into(),
        placed,
        left,
    })
    .collect()
}

pub(super) fn sample_thing(block: forge::BlockKind, kind: forge::ThingKind) -> forge::Thing {
    forge::Thing {
        id: forge::Endpoint::Base {
            block,
            index: 0,
            print: None,
        },
        block,
        kind,
        own: true,
        frame: forge::Frame::default(),
        fields: if kind == forge::ThingKind::Goal {
            vec![
                ("type", forge::Value::Int(0)),
                ("team", forge::Value::Int(0)),
                ("identifier", forge::Value::Int(0)),
            ]
        } else {
            Vec::new()
        },
        changed: false,
    }
}

/// Each screen's named sample sheets.
pub fn samples(kind: Kind) -> Vec<(&'static str, Sheet)> {
    let at = |sheet: &Sheet, focus: usize| Sheet {
        focus,
        ..sheet.clone()
    };
    match kind {
        Kind::Objects => {
            let items = sample_items();
            vec![
                ("open", sheet::objects(&items, 1).expect("objects")),
                ("first", sheet::objects(&items, 0).expect("objects")),
            ]
        }
        Kind::Properties => vec![
            (
                "beacon",
                sheet::properties_sheet(
                    "Beacon",
                    &sample_thing(forge::BlockKind::Scenery, forge::ThingKind::Object),
                    17.5,
                )
                .expect("rows"),
            ),
            (
                "gate",
                sheet::properties_sheet(
                    "Gate",
                    &sample_thing(forge::BlockKind::NetgameFlags, forge::ThingKind::Goal),
                    90.,
                )
                .expect("rows"),
            ),
        ],
        Kind::Coordinates => vec![(
            "beacon",
            sheet::coordinates_sheet(
                "Beacon",
                forge::Frame {
                    position: [1.25, -2.5, 3.75],
                    rotation: [0.5, 0.25, -0.5],
                },
            ),
        )],
        Kind::Tools => vec![
            ("objects", sheet::tools(Tool::Objects)),
            ("links", sheet::tools(Tool::Links)),
        ],
        Kind::Settings => vec![
            ("open", sheet::settings(2, 0, false)),
            ("fine", sheet::settings(0, 0, true)),
        ],
        Kind::Link => vec![
            (
                "teleporter",
                sheet::link("Teleporter sender 2", "Teleporter receiver 5", "Channel 3"),
            ),
            (
                "device",
                sheet::link("Gate", "Lift", "Its device group applies on Rebuild map."),
            ),
        ],
        Kind::Question => vec![
            ("delete-linked", sheet::question(Ask::DeleteLinked, "")),
            ("overwrite", sheet::question(Ask::Overwrite, "Courtyard")),
            ("leave", sheet::question(Ask::Leave, "")),
        ],
        Kind::Options => {
            let open = sheet::options();
            let last = at(&open, open.rows.len() - 1);
            vec![("open", open), ("last", last)]
        }
        Kind::Controls => {
            let open = sheet::controls();
            let last = at(&open, open.rows.len() - 1);
            vec![("open", open), ("last", last)]
        }
        Kind::Games => {
            use forge_doc::games::GameSet;
            let open = super::games::sheet(GameSet::of(&[2, 3]));
            let done = at(&super::games::sheet(GameSet::ALL), 8);
            vec![("open", open), ("all", done)]
        }
    }
}

/// A Forge screen in one of its sample sheets, its focus where the
/// presses left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub sample: usize,
    pub focus: usize,
}

pub struct SheetPage {
    kind: Kind,
    samples: Vec<(&'static str, Sheet)>,
}

impl SheetPage {
    pub fn new(kind: Kind) -> Self {
        SheetPage {
            kind,
            samples: samples(kind),
        }
    }

    /// The screen showing sample `s`: a class of its own, so a page's
    /// presses never reach another's.
    pub fn screen(&self, s: Shown) -> Arc<ForgeScreen> {
        let screen = ForgeScreen::new(self.kind);
        let sheet = Sheet {
            focus: s.focus,
            ..self.samples[s.sample].1.clone()
        };
        screen.show(sheet).expect("a sample fits its screen");
        Arc::new(screen)
    }
}

impl Page for SheetPage {
    type State = Shown;
    type Event = Input;
    type Effect = ListEffect;

    fn step(&self, s: &mut Shown, input: Input) -> ListEffect {
        let screen = self.screen(*s);
        let page = ListPage::new(screen.class(), described(&screen));
        let mut focus = Focus(s.focus);
        let e = page.step(&mut focus, input);
        s.focus = focus.0;
        e
    }

    fn handled(e: &ListEffect) -> bool {
        *e != ListEffect::None
    }

    fn spec(&self, s: &Shown) -> ScreenSpec {
        self.screen(*s).spec()
    }

    fn states(&self) -> Vec<(String, Shown)> {
        self.samples
            .iter()
            .enumerate()
            .map(|(i, (name, sheet))| {
                (
                    name.to_string(),
                    Shown {
                        sample: i,
                        focus: sheet.focus,
                    },
                )
            })
            .collect()
    }

    fn summary(&self, s: &Shown) -> String {
        let spec = self.spec(s);
        format!("{} | {}", spec.title, focus_line(&spec))
    }
}

/// The page of the Forge screen `name`, for `h2 ui preview`.
pub fn live(name: &str) -> Option<Box<dyn Live>> {
    Some(Box::new(Driven::new(SheetPage::new(Kind::of_screen(
        name,
    )?))))
}
