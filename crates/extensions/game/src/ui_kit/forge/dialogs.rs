use super::{Hud, LINK, MENU, PALETTE, PROPERTIES};
use crate::ui_kit::{
    dialog::{DialogBase, DialogSpec},
    input::{Glyph, Input, Prompt},
    picker,
    spec::{ListPlace, Press, RowSpec, ScreenSpec, ValueStyle},
    text_fit,
    view::{Art, Opener, PaneArt},
};

/// Rotation choices in degrees. Store the index only after confirming a choice.
pub const ANGLE_STEPS: [f32; 4] = [1., 5., 15., 45.];
/// Absolute object yaw in degrees about the world's vertical axis.
pub const HEADINGS: [f32; 8] = [0., 45., 90., 135., 180., 225., 270., 315.];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Select(usize),
    Heading(usize),
    RotationStep(usize),
    Save,
    SaveAs,
    Load(String),
    /// Remove the link the dialog shows.
    Unlink,
}

#[derive(Clone, Debug)]
struct Group {
    name: String,
    /// Original palette positions, alongside the display names from the map.
    items: Vec<(usize, String)>,
}

#[derive(Clone, Debug)]
enum Parent {
    Groups {
        groups: Vec<Group>,
        selected: usize,
    },
    Properties {
        object: String,
        angle: usize,
        heading: f32,
    },
    Menu {
        saves: Vec<String>,
    },
    /// A link's ends and what it writes, as (label, value) rows.
    Link {
        rows: Vec<(String, String)>,
    },
}

/// The parent retains its focus while its picker is open.
#[derive(Clone, Debug)]
pub struct Dialogs {
    parent: Parent,
    focus: usize,
    picker_focus: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Ignored,
    Changed,
    Action(Action),
}

impl Dialogs {
    /// Show the map's groups with an object picker over the selected group.
    /// Select keeps the map's palette index even after grouping and sorting.
    pub fn grouped_palette(items: Vec<(String, String)>, selected: usize) -> Self {
        let mut by_group = std::collections::BTreeMap::<String, Vec<(usize, String)>>::new();
        for (index, (group, name)) in items.into_iter().enumerate() {
            by_group.entry(group).or_default().push((index, name));
        }
        let groups: Vec<_> = by_group
            .into_iter()
            .map(|(name, mut items)| {
                items.sort_by(|(_, a), (_, b)| a.cmp(b));
                Group { name, items }
            })
            .collect();
        let focus = groups
            .iter()
            .position(|g| g.items.iter().any(|(i, _)| *i == selected))
            .unwrap_or(0);
        Self {
            parent: Parent::Groups { groups, selected },
            focus,
            picker_focus: None,
        }
    }

    pub fn properties(name: String, rotation_step: usize, heading_radians: f32) -> Self {
        let angle = rotation_step.min(ANGLE_STEPS.len() - 1);
        Self {
            parent: Parent::Properties {
                object: name,
                heading: heading_radians.to_degrees().rem_euclid(360.),
                angle,
            },
            focus: 0,
            picker_focus: None,
        }
    }

    /// A link's rows, then **Remove link**.
    pub fn link(rows: Vec<(String, String)>) -> Self {
        Self {
            parent: Parent::Link { rows },
            focus: 0,
            picker_focus: None,
        }
    }

    pub fn menu(saves: Vec<String>) -> Self {
        Self {
            parent: Parent::Menu { saves },
            focus: 0,
            picker_focus: None,
        }
    }

    fn count(&self) -> usize {
        match (&self.parent, self.picker_focus) {
            (Parent::Groups { groups, .. }, None) => groups.len(),
            (Parent::Groups { groups, .. }, Some(_)) => groups[self.focus].items.len(),
            (Parent::Properties { .. }, None) => 2,
            (Parent::Properties { heading, .. }, Some(_)) if self.focus == 0 => {
                HEADINGS.len() + usize::from(heading_index(*heading).is_none())
            }
            (Parent::Properties { .. }, Some(_)) => ANGLE_STEPS.len(),
            (Parent::Menu { .. }, None) => 3,
            (Parent::Menu { saves }, Some(_)) => saves.len(),
            (Parent::Link { rows }, _) => rows.len() + 1,
        }
    }

    fn confirm(&mut self, index: usize) -> Effect {
        if index >= self.count() {
            return Effect::Ignored;
        }
        match (&self.parent, self.picker_focus) {
            (Parent::Groups { groups, selected }, None) => {
                self.picker_focus = Some(
                    groups[index]
                        .items
                        .iter()
                        .position(|(i, _)| i == selected)
                        .unwrap_or(0),
                );
                Effect::Changed
            }
            (Parent::Groups { groups, .. }, Some(_)) => {
                Effect::Action(Action::Select(groups[self.focus].items[index].0))
            }
            (Parent::Properties { heading, .. }, None) if index == 0 => {
                self.picker_focus = Some(heading_index(*heading).unwrap_or(0));
                Effect::Changed
            }
            (Parent::Properties { heading, .. }, Some(_)) if self.focus == 0 => {
                let offset = usize::from(heading_index(*heading).is_none());
                if let Some(index) = index.checked_sub(offset) {
                    Effect::Action(Action::Heading(index))
                } else {
                    // The object already has the selected unsnapped heading.
                    self.picker_focus = None;
                    Effect::Changed
                }
            }
            (Parent::Properties { angle, .. }, None) => {
                self.picker_focus = Some(*angle);
                Effect::Changed
            }
            (Parent::Properties { .. }, Some(_)) => Effect::Action(Action::RotationStep(index)),
            (Parent::Menu { .. }, None) if index == 0 => Effect::Action(Action::Save),
            (Parent::Menu { .. }, None) if index == 1 => Effect::Action(Action::SaveAs),
            (Parent::Menu { .. }, None) => {
                self.picker_focus = Some(0);
                Effect::Changed
            }
            (Parent::Menu { saves }, Some(_)) => Effect::Action(Action::Load(saves[index].clone())),
            (Parent::Link { rows }, _) if index == rows.len() => Effect::Action(Action::Unlink),
            (Parent::Link { .. }, _) => Effect::Ignored,
        }
    }

    pub(super) fn step(&mut self, input: Input) -> Effect {
        let count = self.count();
        let focus = self.picker_focus.as_mut().unwrap_or(&mut self.focus);
        match input {
            Input::Up if *focus > 0 => {
                *focus -= 1;
                Effect::Changed
            }
            Input::Down if *focus + 1 < count => {
                *focus += 1;
                Effect::Changed
            }
            Input::A => {
                let index = *focus;
                self.confirm(index)
            }
            Input::Back | Input::B if self.picker_focus.take().is_some() => Effect::Changed,
            Input::Back | Input::B => Effect::Action(Action::Close),
            _ => Effect::Ignored,
        }
    }

    pub fn press(&mut self, input: Input) -> Action {
        match self.step(input) {
            Effect::Action(action) => action,
            _ => Action::None,
        }
    }

    /// A hit-tested widget in the topmost dialog. The index comes from
    /// its row id, including the first visible row of a scrolled list.
    pub fn click(&mut self, row: usize) -> Action {
        if row < self.count() {
            *self.picker_focus.as_mut().unwrap_or(&mut self.focus) = row;
            self.press(Input::A)
        } else {
            Action::None
        }
    }

    pub fn spec(&self, hud: &Hud) -> ScreenSpec {
        let mut parent = match &self.parent {
            Parent::Groups { groups, .. } => {
                let names: Vec<_> = groups.iter().map(|g| g.name.clone()).collect();
                let mut s = choices(
                    PALETTE,
                    "PALETTE",
                    &names,
                    self.focus,
                    "Choose an object group.",
                    "This map has no objects to place.",
                );
                for row in &mut s.rows {
                    row.press = Press::Open("forge.objects".into());
                }
                if let Some(dialog) = &mut s.dialog {
                    dialog.rows.clone_from(&s.rows);
                }
                s
            }
            Parent::Properties {
                object,
                angle,
                heading,
            } => {
                let current = degrees(*heading);
                let heading = RowSpec::value("Heading", &[&current], 0)
                    .press(Press::Open("forge.heading".into()));
                let values = angles();
                let row = RowSpec::value(
                    "Rotation step",
                    &values.iter().map(String::as_str).collect::<Vec<_>>(),
                    *angle,
                )
                .press(Press::Open("forge.rotation".into()));
                let mut s = rows(
                    PROPERTIES,
                    "PROPERTIES",
                    vec![heading, row],
                    self.focus,
                    DialogBase::Large,
                );
                // The Controls skin supplies the two text columns inside the large panel.
                s.list = ListPlace::Moved {
                    skin: 2,
                    anchor: [-253, 0],
                    bitmaps_left_of: i16::MIN,
                    visible: 2,
                };
                s.style = ValueStyle::Setting;
                s.dialog.as_mut().expect("a dialog").body = text_fit::line(object, 460);
                s
            }
            Parent::Link { rows: info } => {
                let mut list: Vec<_> = info
                    .iter()
                    .map(|(label, value)| RowSpec::info(label, &text_fit::line(value, 300)))
                    .collect();
                list.push(RowSpec::action("Remove link"));
                let visible = list.len() as i16;
                let mut s = rows(LINK, "LINK", list, self.focus, DialogBase::Large);
                s.list = ListPlace::Moved {
                    skin: 2,
                    anchor: [-253, 0],
                    bitmaps_left_of: i16::MIN,
                    visible,
                };
                s.style = ValueStyle::Setting;
                // Only Remove link takes A; an info row offers only Back.
                if self.focus < info.len() {
                    s.prompts.retain(|p| !p.glyphs.contains(&Glyph::A));
                }
                s
            }
            Parent::Menu { .. } => rows(
                MENU,
                "FORGE",
                vec![
                    RowSpec::action("Save"),
                    RowSpec::action("Save as"),
                    RowSpec::action("Load").press(Press::Open("forge.load".into())),
                ],
                self.focus,
                DialogBase::Large,
            ),
        };
        over(&mut parent, hud.spec());
        let Some(focus) = self.picker_focus else {
            return parent;
        };
        let mut picker = match &self.parent {
            Parent::Groups { groups, .. } => {
                let group = &groups[self.focus];
                let names: Vec<_> = group.items.iter().map(|(_, name)| name.clone()).collect();
                choices(
                    "forge.objects",
                    &text_fit::line(&group.name.to_uppercase(), 460),
                    &names,
                    focus,
                    "Choose an object to place.",
                    "",
                )
            }
            Parent::Properties { heading, .. } if self.focus == 0 => {
                let mut options: Vec<_> = HEADINGS.into_iter().map(degrees).collect();
                if heading_index(*heading).is_none() {
                    options.insert(0, degrees(*heading));
                }
                choices(
                    "forge.heading",
                    "HEADING",
                    &options,
                    focus,
                    "Set the object's heading.",
                    "",
                )
            }
            Parent::Properties { .. } => choices(
                "forge.rotation",
                "ROTATION STEP",
                &angles(),
                focus,
                "Choose how far each turn rotates the object.",
                "",
            ),
            Parent::Link { .. } => return parent,
            Parent::Menu { saves } => choices(
                "forge.load",
                "LOAD",
                saves,
                focus,
                "Open a saved Forge map.",
                "No Forge maps saved.",
            ),
        };
        over(&mut picker, parent);
        picker
    }
}

fn over(spec: &mut ScreenSpec, parent: ScreenSpec) {
    spec.panel.push(PaneArt {
        art: Art::Solid {
            rgba: [10, 19, 29, 218],
        },
        at: [-257, 100],
        size: [514, 204],
        depth: 0,
    });
    spec.underlay = Some(Box::new(parent));
}

fn heading_index(degrees: f32) -> Option<usize> {
    HEADINGS.iter().position(|n| (*n - degrees).abs() < 0.0001)
}

fn degrees(angle: f32) -> String {
    let n = format!("{angle:.2}");
    let n = n.trim_end_matches('0').trim_end_matches('.');
    format!("{n} {}", if n == "1" { "degree" } else { "degrees" })
}

fn angles() -> Vec<String> {
    ANGLE_STEPS.into_iter().map(degrees).collect()
}

/// Use the in-game picker's tag geometry, list viewport, help and legend.
fn choices(
    name: &str,
    title: &str,
    values: &[String],
    focus: usize,
    help: &str,
    empty: &str,
) -> ScreenSpec {
    if values.is_empty() {
        let mut s = ScreenSpec::new(name, title);
        s.dialog = Some(DialogSpec::new(empty, Opener::MAIN_MENU));
        s.content.empty_text = Some("body".into());
        s.prompts.push(Prompt::new(&[Glyph::B], "Back"));
        s.inputs.push(Input::B);
        return s;
    }
    let labels = values
        .iter()
        .map(|v| text_fit::line(v, text_fit::ROW))
        .collect::<Vec<_>>();
    let mut s = picker::playback_spec(title, &labels, focus, help);
    s.name = name.into();
    s
}

fn rows(name: &str, title: &str, rows: Vec<RowSpec>, focus: usize, base: DialogBase) -> ScreenSpec {
    let mut s = ScreenSpec::new(name, title);
    s.inputs = vec![Input::A, Input::B, Input::Up, Input::Down];
    s.prompts = vec![
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Back"),
    ];
    s.style = base.style();
    s.rows = rows;
    s.focus = focus;
    s.footer = true;
    s.dialog = Some(DialogSpec {
        base,
        rows: s.rows.clone(),
        ..DialogSpec::new("", Opener::MAIN_MENU)
    });
    s
}
