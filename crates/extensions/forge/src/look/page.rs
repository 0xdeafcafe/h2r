//! The rows of the monitor look's page (docs/game/
//! forge-monitor-appearance.md, "Choosing it", ML6): the seven rows, each
//! with its value, the values its picker lists, the patch each value
//! sends, and the reason it is locked when it is.
//!
//! Pure: it reads a look and the catalogue and names no widget, so the
//! page's table tests, the agent and the Forge screen read the same rows.
//! The page's screen turns each [`Row`] into a sheet row and each
//! [`Choice`] into a picker value; a game with the page open calls
//! [`choose`] when A picks a value and [`focused`] while a value is only
//! focused.
use super::census::Entry;
use super::{LIGHT_MAX, MARKER, MonitorLook, PALETTE, Patch, Refusal, TINT_MAX, apply, locks};

/// The page's rows, in order: the field and the label shown.
pub const ROWS: [(&str, &str); 7] = [
    ("model", "Model"),
    ("variant", "Variant"),
    ("primary", "Primary colour"),
    ("secondary", "Secondary colour"),
    ("tint", "Tint"),
    ("light", "Light"),
    ("light_colour", "Light colour"),
];

/// The tint steps' names, by value.
pub const TINTS: [&str; 4] = ["None", "Light", "Medium", "Strong"];
/// The light's names, by value.
pub const LIGHTS: [&str; 3] = ["Off", "Dim", "Bright"];
/// The marker's name in the model picker.
pub const MARKER_NAME: &str = "Marker";
/// The variant row's value for the model's own variant.
pub const DEFAULT_VARIANT: &str = "Default";

/// One value of a row's picker and what choosing it sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub patch: Patch,
}

/// One row of the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The look's field the row changes.
    pub field: &'static str,
    pub label: &'static str,
    /// The value the row shows in its second column.
    pub value: String,
    /// The reason the row cannot be changed, which is also its
    /// description; `None` for a row that can.
    pub locked: Option<String>,
    /// The picker's values, in order, and the held one's index.
    pub choices: Vec<Choice>,
    pub current: usize,
    /// One line of description when the row is not locked.
    pub help: &'static str,
}

fn help(field: &str) -> &'static str {
    match field {
        "model" => "The body you fly as.",
        "variant" => "Which of the model's looks.",
        "primary" => "The model's main colour.",
        "secondary" => "The model's second colour.",
        "tint" => "How far the model is washed with the primary colour.",
        "light" => "A light on the body.",
        _ => "The light's colour.",
    }
}

fn colour_choices(
    field: &'static str,
    held: u8,
    colour_name: &dyn Fn(u8) -> String,
) -> (Vec<Choice>, usize) {
    let choices = (0..PALETTE)
        .map(|i| Choice {
            label: colour_name(i),
            patch: match field {
                "primary" => Patch {
                    primary: Some(i),
                    ..Patch::default()
                },
                "secondary" => Patch {
                    secondary: Some(i),
                    ..Patch::default()
                },
                _ => Patch {
                    light_colour: Some(i),
                    ..Patch::default()
                },
            },
        })
        .collect();
    (choices, usize::from(held.min(PALETTE - 1)))
}

fn named_choices(
    names: &[&str],
    held: u8,
    patch: impl Fn(u8) -> Patch,
) -> (Vec<Choice>, usize) {
    let choices = names
        .iter()
        .enumerate()
        .map(|(i, name)| Choice {
            label: (*name).to_owned(),
            patch: patch(i as u8),
        })
        .collect();
    (choices, usize::from(held).min(names.len() - 1))
}

/// The page's rows for `look` over `catalogue`. `team` is the reason the
/// host's team colours lock the primary row, when they do. `colour_name`
/// is the game's own string for a palette index, read at run time.
pub fn rows(
    look: &MonitorLook,
    catalogue: &[Entry],
    team: Option<&str>,
    colour_name: &dyn Fn(u8) -> String,
) -> Vec<Row> {
    let mut locked = locks(&look.model, catalogue);
    if let Some(reason) = team {
        locked.insert("primary", reason.to_owned());
    }
    let entry = catalogue.iter().find(|e| e.key == look.model);
    ROWS.iter()
        .map(|&(field, label)| {
            let (choices, current) = match field {
                "model" => {
                    let mut choices = vec![Choice {
                        label: MARKER_NAME.into(),
                        patch: Patch {
                            model: Some(MARKER.into()),
                            ..Patch::default()
                        },
                    }];
                    choices.extend(catalogue.iter().map(|e| Choice {
                        label: e.name.clone(),
                        patch: Patch {
                            model: Some(e.key.clone()),
                            ..Patch::default()
                        },
                    }));
                    let current = if look.is_marker() {
                        0
                    } else {
                        catalogue
                            .iter()
                            .position(|e| e.key == look.model)
                            .map_or(0, |i| i + 1)
                    };
                    (choices, current)
                }
                "variant" => {
                    let mut choices = vec![Choice {
                        label: DEFAULT_VARIANT.into(),
                        patch: Patch {
                            variant: Some(None),
                            ..Patch::default()
                        },
                    }];
                    choices.extend(entry.iter().flat_map(|e| e.variants.iter()).map(|v| {
                        Choice {
                            label: v.clone(),
                            patch: Patch {
                                variant: Some(Some(v.clone())),
                                ..Patch::default()
                            },
                        }
                    }));
                    let current = look
                        .variant
                        .as_ref()
                        .and_then(|held| choices.iter().position(|c| c.label == *held))
                        .unwrap_or(0);
                    (choices, current)
                }
                "primary" => colour_choices(field, look.primary, colour_name),
                "secondary" => colour_choices(field, look.secondary, colour_name),
                "tint" => named_choices(&TINTS, look.tint.min(TINT_MAX), |i| Patch {
                    tint: Some(i),
                    ..Patch::default()
                }),
                "light" => named_choices(&LIGHTS, look.light.min(LIGHT_MAX), |i| Patch {
                    light: Some(i),
                    ..Patch::default()
                }),
                _ => colour_choices(field, look.light_colour, colour_name),
            };
            Row {
                field,
                label,
                value: choices[current].label.clone(),
                locked: locked.get(field).cloned(),
                choices,
                current,
                help: help(field),
            }
        })
        .collect()
}

/// A picks value `index` of `row`: the look it makes, or the refusal that
/// names why not (a locked row's value that would change, a model or
/// variant the catalogue lacks). A pick of the held value changes nothing.
pub fn choose(
    look: &MonitorLook,
    catalogue: &[Entry],
    row: &Row,
    index: usize,
    team: Option<&str>,
) -> Result<MonitorLook, Refusal> {
    let choice = row
        .choices
        .get(index)
        .ok_or_else(|| Refusal::invalid(row.field, "there is no such value"))?;
    if let Some(reason) = team.filter(|_| row.field == "primary")
        && choice.patch.primary.is_some_and(|p| p != look.primary)
    {
        return Err(Refusal::locked(row.field, reason));
    }
    apply(look, &choice.patch, catalogue)
}

/// The look the preview shows while value `index` of `row` is only
/// focused: the held look with the value laid over it, locks ignored (the
/// preview is not a choice). The held look when the value does not fit.
pub fn focused(look: &MonitorLook, row: &Row, index: usize) -> MonitorLook {
    row.choices
        .get(index)
        .and_then(|c| look.with(&c.patch).ok())
        .unwrap_or_else(|| look.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::VERSION;

    fn entry(key: &str, variants: &[&str], colours: bool) -> Entry {
        Entry {
            key: key.into(),
            name: key.to_uppercase(),
            source: "invented".into(),
            sha256: "a".repeat(64),
            variants: variants.iter().map(|v| (*v).into()).collect(),
            change_colours: colours,
            light: true,
            nodes: 3,
            triangles: 9,
        }
    }

    fn look(model: &str) -> MonitorLook {
        MonitorLook {
            version: VERSION,
            model: model.into(),
            variant: None,
            primary: 3,
            secondary: 5,
            tint: 1,
            light: 2,
            light_colour: 7,
        }
    }

    fn name(i: u8) -> String {
        format!("colour {i}")
    }

    fn catalogue() -> Vec<Entry> {
        vec![
            entry("sentinel", &["default", "major"], true),
            entry("dervish", &["default"], false),
        ]
    }

    fn row<'a>(rows: &'a [Row], field: &str) -> &'a Row {
        rows.iter().find(|r| r.field == field).expect("a row")
    }

    #[test]
    fn the_page_has_its_seven_rows_in_order_with_their_values() {
        let rows = rows(&look("sentinel"), &catalogue(), None, &name);
        let fields: Vec<_> = rows.iter().map(|r| r.field).collect();
        assert_eq!(
            fields,
            ["model", "variant", "primary", "secondary", "tint", "light", "light_colour"]
        );
        let values: Vec<_> = rows.iter().map(|r| r.value.as_str()).collect();
        assert_eq!(
            values,
            ["SENTINEL", "Default", "colour 3", "colour 5", "Light", "Bright", "colour 7"]
        );
        // The model picker lists the marker, then the catalogue.
        let models: Vec<_> = row(&rows, "model").choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(models, ["Marker", "SENTINEL", "DERVISH"]);
        assert_eq!(row(&rows, "model").current, 1);
        // The variant picker lists the model's own.
        let variants: Vec<_> = row(&rows, "variant").choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(variants, ["Default", "default", "major"]);
        assert_eq!(row(&rows, "primary").choices.len(), usize::from(PALETTE));
        assert!(rows.iter().all(|r| r.locked.is_none()), "nothing is locked here");
    }

    #[test]
    fn a_named_variant_is_the_held_one() {
        let mut held = look("sentinel");
        held.variant = Some("major".into());
        let rows = rows(&held, &catalogue(), None, &name);
        assert_eq!(row(&rows, "variant").value, "major");
        assert_eq!(row(&rows, "variant").current, 2);
    }

    #[test]
    fn each_lock_names_its_reason_on_its_row() {
        // No colour areas and one variant.
        let rows_ = rows(&look("dervish"), &catalogue(), None, &name);
        assert_eq!(row(&rows_, "primary").locked.as_deref(), Some("No colour areas"));
        assert_eq!(row(&rows_, "secondary").locked.as_deref(), Some("No colour areas"));
        assert_eq!(
            row(&rows_, "variant").locked.as_deref(),
            Some("This model has one variant")
        );
        assert_eq!(row(&rows_, "tint").locked, None);
        // The marker has no variant, tint or light.
        let mut marker = look(MARKER);
        (marker.tint, marker.light) = (0, 0);
        let rows_ = rows(&marker, &catalogue(), None, &name);
        for field in ["variant", "tint", "light"] {
            assert!(row(&rows_, field).locked.is_some(), "{field}");
        }
        assert_eq!(row(&rows_, "model").value, "Marker");
        assert_eq!(row(&rows_, "primary").locked, None);
        // The host's team colours lock the primary row.
        let rows_ = rows(&look("sentinel"), &catalogue(), Some("The host set team colours"), &name);
        assert_eq!(
            row(&rows_, "primary").locked.as_deref(),
            Some("The host set team colours")
        );
        assert_eq!(row(&rows_, "secondary").locked, None);
    }

    #[test]
    fn a_pick_makes_the_look_and_a_locked_change_is_refused() {
        let cat = catalogue();
        let held = look("sentinel");
        let page = rows(&held, &cat, None, &name);
        let next = choose(&held, &cat, row(&page, "tint"), 3, None).unwrap();
        assert_eq!(next.tint, 3);
        // The model change drops the variant.
        let mut with_variant = held.clone();
        with_variant.variant = Some("major".into());
        let dervish = choose(&with_variant, &cat, row(&page, "model"), 2, None).unwrap();
        assert_eq!((dervish.model.as_str(), dervish.variant), ("dervish", None));
        // A model with no colour areas refuses a colour change, and takes
        // the held value without one.
        let page = rows(&look("dervish"), &cat, None, &name);
        let held = look("dervish");
        let refused = choose(&held, &cat, row(&page, "primary"), 9, None).unwrap_err();
        assert_eq!(refused.code(), "monitor_look_locked");
        assert_eq!(choose(&held, &cat, row(&page, "primary"), 3, None).unwrap(), held);
        // The team lock refuses a primary change the same way.
        let page = rows(&look("sentinel"), &cat, Some("team"), &name);
        let refused = choose(&look("sentinel"), &cat, row(&page, "primary"), 9, Some("team")).unwrap_err();
        assert_eq!(refused.code(), "monitor_look_locked");
        // Past the picker's values.
        assert_eq!(
            choose(&look("sentinel"), &cat, row(&page, "tint"), 99, None).unwrap_err().code(),
            "monitor_look_invalid"
        );
    }

    #[test]
    fn focus_shows_the_value_without_choosing_it() {
        let cat = catalogue();
        let held = look("sentinel");
        let page = rows(&held, &cat, None, &name);
        let shown = focused(&held, row(&page, "secondary"), 11);
        assert_eq!(shown.secondary, 11);
        assert_eq!(held.secondary, 5, "the held look is untouched");
        // A locked row still previews, since the preview chooses nothing.
        let dervish = look("dervish");
        let page = rows(&dervish, &cat, None, &name);
        assert_eq!(focused(&dervish, row(&page, "primary"), 9).primary, 9);
        // A value that does not fit leaves the held look.
        assert_eq!(focused(&held, row(&page, "tint"), 99), held);
    }
}
