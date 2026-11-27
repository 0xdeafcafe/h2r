//! The Properties page's rows from its doc (docs/game/
//! forge-map-variants-ui.md, "The Properties page"): each `props:` row of
//! `content/ui/screens/forge.properties.yaml` is for some kinds of thing,
//! names a property and says what A does. [`rows`] binds them to one
//! thing: the rows of its kind, each showing the value the thing holds. A
//! property the thing does not have (its block lacks the field, or the
//! layout does not let it be edited) has no row, and neither does one the
//! binder cannot make.

use forge::{Thing, ThingKind};

use super::sheet::{
    Act, Choice, Extras, HEADINGS, Kind, NumberField, Prop, SheetRow, TypeSpec, degrees, int_of,
    nearest_heading, text_value_row, value_row,
};
use crate::ui_kit::data::{PropDoc, ScreenDoc};

/// The name a doc calls `kind` by ([`crate::ui_kit::data::PROP_KINDS`]).
pub fn kind_name(kind: ThingKind) -> &'static str {
    match kind {
        ThingKind::Object | ThingKind::Piece => "object",
        ThingKind::ItemSpawner => "spawner",
        ThingKind::StartingPoint => "starting-point",
        ThingKind::Goal => "goal",
        ThingKind::Teleporter => "teleporter",
    }
}

/// Whether row `p` is for a thing of `kind`.
pub fn is_for(p: &PropDoc, kind: ThingKind) -> bool {
    p.on.is_empty() || p.on.iter().any(|n| n == "any" || n == kind_name(kind))
}

/// Whether `thing` has the property `p` names: a field of its block that
/// can be edited, or one of the two that are not fields.
fn has(p: &PropDoc, thing: &Thing) -> bool {
    crate::ui_kit::data::PROP_VIRTUAL.contains(&p.prop.as_str())
        || forge::props::prop(&p.prop).is_some_and(|d| d.read(thing).is_some())
}

/// The doc rows for `thing`, in the doc's order. `heading` is its yaw in
/// degrees.
pub fn rows(doc: &ScreenDoc, thing: &Thing, heading: f32) -> Vec<SheetRow> {
    rows_with(doc, thing, heading, &Extras::default())
}

/// [`rows`] with the rows that read the session: Label, Label number,
/// Clear label and Save as prefab, each only where `extras` says the thing
/// has it.
pub fn rows_with(doc: &ScreenDoc, thing: &Thing, heading: f32, extras: &Extras) -> Vec<SheetRow> {
    doc.props
        .values()
        .filter(|p| is_for(p, thing.kind) && has(p, thing))
        .filter_map(|p| row(p, thing, heading, extras))
        .collect()
}

fn row(p: &PropDoc, thing: &Thing, heading: f32, extras: &Extras) -> Option<SheetRow> {
    let help = p.help.as_deref().unwrap_or("");
    let values = p.values.as_deref().unwrap_or(&[]);
    let last = values.len().saturating_sub(1);
    match (p.open.as_str(), p.prop.as_str()) {
        ("picker", "heading") => Some(value_row(
            &p.text,
            help,
            Prop::Heading,
            &HEADINGS,
            degrees(heading.rem_euclid(360.)),
            nearest_heading(heading),
        )),
        ("picker", "team") => {
            let team = int_of(thing, "team");
            let current = if team < 0 { 0 } else { (team + 1) as usize };
            picker(p, help, Prop::Team, values, current.min(last))
        }
        ("picker", "role") => {
            let current = int_of(thing, "type").clamp(0, last as i64) as usize;
            picker(p, help, Prop::Role, values, current)
        }
        ("picker", "order") => {
            let current = int_of(thing, "identifier").clamp(0, last as i64) as usize;
            picker(p, help, Prop::Order, values, current)
        }
        ("keyboard", "respawn time") => {
            let value = int_of(thing, "respawn time").to_string();
            Some(
                SheetRow::new(
                    &p.text,
                    Act::Type(TypeSpec {
                        field: NumberField::RespawnTime,
                        initial: value.clone(),
                    }),
                )
                .value(&value)
                .describe(help),
            )
        }
        ("keyboard", "return time") => {
            // Absent where the block has no abandon limit.
            forge::props::prop("return time")?.read(thing)?;
            let value = int_of(thing, "abandon limit").to_string();
            Some(
                SheetRow::new(
                    &p.text,
                    Act::Type(TypeSpec {
                        field: NumberField::ReturnTime,
                        initial: value.clone(),
                    }),
                )
                .value(&value)
                .describe(help),
            )
        }
        ("forge.games", "games") => {
            let set = forge::props::games(thing)?;
            Some(
                SheetRow::new(&p.text, Act::Dialog(Box::new(super::games::sheet(set))))
                    .describe(help),
            )
        }
        ("keyboard", "label") if extras.labellable => {
            let text = extras.label.as_ref().map_or("", |(text, _)| text.as_str());
            Some(
                SheetRow::new(
                    &p.text,
                    Act::Type(TypeSpec {
                        field: NumberField::Label,
                        initial: text.into(),
                    }),
                )
                .value(if text.is_empty() { "None" } else { text })
                .describe(help),
            )
        }
        ("keyboard", "label number") => {
            let (_, number) = extras.label.as_ref()?;
            let text = number.map(|n| n.to_string());
            Some(
                SheetRow::new(
                    &p.text,
                    Act::Type(TypeSpec {
                        field: NumberField::LabelNumber,
                        initial: text.clone().unwrap_or_default(),
                    }),
                )
                .value(text.as_deref().unwrap_or("None"))
                .describe(help),
            )
        }
        ("act", "clear label") => {
            extras.label.as_ref()?;
            Some(SheetRow::new(&p.text, Act::Choose(Choice::ClearLabel(thing.id))).describe(help))
        }
        // Any thing with a rotation, or a facing to round: the block's turn
        // field, which the session writes as one set.
        ("act", "reset orientation") => forge::props::prop("rotation")
            .and_then(|d| d.field(thing.block))
            .map(|_| SheetRow::new(&p.text, Act::Choose(Choice::ResetOrientation)).describe(help)),
        // A copy of a thing, held at once: a placed object (not the ghost
        // of one being placed), or one of the kit's kinds in a kit session.
        ("act", "duplicate") => {
            let placed = match thing.kind {
                ThingKind::Object | ThingKind::Piece => {
                    thing.fields.iter().any(|(name, _)| *name == "palette index")
                }
                _ => extras.kit,
            };
            placed.then(|| {
                SheetRow::new(&p.text, Act::Choose(Choice::Duplicate(thing.id))).describe(help)
            })
        }
        ("keyboard", "prefab") if extras.savable > 0 => {
            let value = if extras.savable == 1 {
                "1 object".to_string()
            } else {
                format!("{} objects", extras.savable)
            };
            Some(
                SheetRow::new(
                    &p.text,
                    Act::Type(TypeSpec {
                        field: NumberField::PrefabName,
                        initial: String::new(),
                    }),
                )
                .value(&value)
                .describe(help),
            )
        }
        ("keyboard", "label" | "prefab") => None,
        (screen, _) => {
            let kind = Kind::of_screen(screen)?;
            Some(SheetRow::new(&p.text, Act::Open(kind)).describe(help))
        }
    }
}

fn picker(
    p: &PropDoc,
    help: &str,
    prop: Prop,
    values: &[String],
    current: usize,
) -> Option<SheetRow> {
    (!values.is_empty()).then(|| text_value_row(&p.text, help, prop, values, current))
}

#[cfg(test)]
#[path = "props_tests.rs"]
mod tests;
