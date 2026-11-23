use super::{Hud, LINK, LOOK, MENU, PALETTE, PROPERTIES};
use crate::ui_kit::{
    dialog::{DialogBase, DialogSpec},
    input::{Glyph, Input, Prompt},
    picker,
    spec::{ListPlace, PaneText, Press, RowSpec, ScreenSpec, ValueStyle},
    text_fit,
    view::{Art, Opener, PaneArt},
};

/// The tint steps, by the look's `tint`.
pub const TINTS: [&str; 4] = ["None", "Light", "Medium", "Strong"];
/// The light steps, by the look's `light`.
pub const LIGHTS: [&str; 3] = ["Off", "Dim", "Bright"];

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
    /// Open the monitor look page (the Forge menu's row).
    Look,
    /// The monitor look page: row `0..6` set to its choice
    /// ([`TINTS`], [`LIGHTS`] and the colour list's indices included).
    LookSet(usize, usize),
    /// The monitor look page's reset.
    LookReset,
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
    /// The monitor look page (docs/game/forge-monitor-appearance.md,
    /// "Choosing it"): the held look, its rows and the model names the
    /// catalogue offers.
    Look {
        held: forge::look::MonitorLook,
        rows: Vec<LookRow>,
        /// The catalogue's model names, in order, beside the marker.
        models: Vec<String>,
    },
}

/// One row of the monitor look page: its value as shown, what it cannot
/// be and why (the lock), and the values a picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookRow {
    pub label: &'static str,
    pub value: String,
    /// The lock's sentence, shown as the row's description.
    pub locked: Option<String>,
    pub choices: Vec<String>,
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

    /// The monitor look page: the held look's rows from the catalogue and
    /// the palette's colour names.
    pub fn look(
        held: forge::look::MonitorLook,
        catalogue: &[forge::look::census::Entry],
        colours: &[String],
    ) -> Self {
        let rows = look_rows(&held, catalogue, colours);
        let mut models = Vec::with_capacity(catalogue.len() + 1);
        models.push("Marker".into());
        models.extend(catalogue.iter().map(|e| e.name.clone()));
        Self {
            parent: Parent::Look { held, rows, models },
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
            (Parent::Menu { .. }, None) => 4,
            (Parent::Look { rows, .. }, None) => rows.len(),
            (Parent::Look { rows, .. }, Some(_)) => {
                rows[self.focus].choices.len()
            }
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
            (Parent::Menu { .. }, None) if index == 3 => Effect::Action(Action::Look),
            (Parent::Menu { .. }, None) => {
                self.picker_focus = Some(0);
                Effect::Changed
            }
            (Parent::Look { rows, .. }, None) => {
                if rows[index].locked.is_some() {
                    return Effect::Ignored;
                }
                self.picker_focus = Some(0);
                Effect::Changed
            }
            (Parent::Look { .. }, Some(_)) => {
                Effect::Action(Action::LookSet(self.focus, index))
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
            Input::X if matches!(self.parent, Parent::Look { .. }) && self.picker_focus.is_none() => {
                Effect::Action(Action::LookReset)
            }
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
                    RowSpec::action("Monitor look"),
                ],
                self.focus,
                DialogBase::Large,
            ),
            Parent::Look {
                held,
                rows: look,
                models,
            } => {
                // While a picker is open the preview shows the focused
                // value, not the held one; B restores it.
                let previewed = self.picker_focus.map(|focus| {
                    preview_look(models, self.focus, focus, &look[0].value)
                });
                let list: Vec<_> = look
                    .iter()
                    .map(|row| match &row.locked {
                        Some(reason) => {
                            RowSpec::info(row.label, &text_fit::line(reason, 300))
                        }
                        None => RowSpec::value(row.label, &[&row.value], 0),
                    })
                    .collect();
                let mut s = rows(LOOK, "MONITOR LOOK", list, self.focus, DialogBase::Large);
                s.list = ListPlace::Moved {
                    skin: 2,
                    anchor: [-253, 0],
                    bitmaps_left_of: i16::MIN,
                    visible: look.len() as i16,
                };
                s.style = ValueStyle::Setting;
                // The preview: a Scene3d widget the compositor places in
                // the game (the look drawn by the native renderer); the
                // spec carries its frame and its model's name, which the
                // dialog draws as its pane text.
                let name = previewed.unwrap_or_else(|| look[0].value.clone());
                s.art.push(PaneArt {
                    art: Art::Solid {
                        rgba: [16, 24, 36, 255],
                    },
                    at: [100, 40],
                    size: [160, 120],
                    depth: 0,
                });
                s.texts.push(PaneText {
                    id: "preview".into(),
                    rect: [40, 100, 160, 260],
                    font: 1,
                    colour: [0.78, 0.84, 0.89],
                    right: false,
                    text: name,
                });
                s
            }
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
            Parent::Look { rows, .. } => {
                let row = &rows[self.focus];
                choices(
                    "forge.look.row",
                    &row.label.to_uppercase(),
                    &row.choices,
                    focus,
                    &row.locked.clone().unwrap_or_default(),
                    "",
                )
            }
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

/// The look page's rows: each field's value as shown, its lock with the
/// reason a row shows, and the values a picker offers.
fn look_rows(
    look: &forge::look::MonitorLook,
    catalogue: &[forge::look::census::Entry],
    colours: &[String],
) -> Vec<LookRow> {
    use forge::look::locks;
    let colour = |index: u8| {
        colours
            .get(usize::from(index))
            .cloned()
            .unwrap_or_else(|| format!("Colour {index}"))
    };
    let locked = locks(&look.model, catalogue);
    let entry = catalogue.iter().find(|e| e.key == look.model);
    let marker_locks = look.is_marker();
    let lock = |field: &str, marker: bool| -> Option<String> {
        if marker {
            return Some("The marker has none".into());
        }
        locked.get(field).cloned()
    };
    let mut models = Vec::with_capacity(catalogue.len() + 1);
    models.push("Marker".into());
    models.extend(catalogue.iter().map(|e| e.name.clone()));
    vec![
        LookRow {
            label: "Model",
            value: if look.is_marker() {
                "Marker".into()
            } else {
                entry.map(|e| e.name.clone()).unwrap_or_else(|| look.model.clone())
            },
            locked: None,
            choices: models,
        },
        LookRow {
            label: "Variant",
            value: look
                .variant
                .clone()
                .unwrap_or_else(|| "Default".into()),
            locked: lock("variant", marker_locks),
            choices: entry.map(|e| e.variants.clone()).unwrap_or_default(),
        },
        LookRow {
            label: "Primary colour",
            value: colour(look.primary),
            locked: lock("primary", marker_locks),
            choices: colours.to_vec(),
        },
        LookRow {
            label: "Secondary colour",
            value: colour(look.secondary),
            locked: lock("secondary", marker_locks),
            choices: colours.to_vec(),
        },
        LookRow {
            label: "Tint",
            value: TINTS[usize::from(look.tint.min(3))].into(),
            locked: lock("tint", marker_locks),
            choices: TINTS.iter().map(|s| s.to_string()).collect(),
        },
        LookRow {
            label: "Light",
            value: LIGHTS[usize::from(look.light.min(2))].into(),
            locked: lock("light", marker_locks),
            choices: LIGHTS.iter().map(|s| s.to_string()).collect(),
        },
        LookRow {
            label: "Light colour",
            value: colour(look.light_colour),
            locked: lock("light_colour", marker_locks),
            choices: colours.to_vec(),
        },
    ]
}

/// The preview while a picker is open: the focused value of `row` for
/// the Model picker, the held model's name for any other. B restores the
/// held one by closing the picker.
fn preview_look(
    models: &[String],
    row: usize,
    focus: usize,
    held_name: &str,
) -> String {
    if row == 0 {
        return models.get(focus).cloned().unwrap_or_else(|| held_name.to_owned());
    }
    held_name.to_owned()
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
