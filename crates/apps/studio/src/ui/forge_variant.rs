//! The Forge map variant document (`Doc::Forge`,
//! docs/editor/studio-flow.md, "Forge variants and items"): the variant's
//! things as rows, and the fields the variant writes of the selected one
//! as text boxes. A box that loses focus after an edit sets the field
//! ([`ForgeAction::Set`]); the header's Undo, Redo and Save act on the
//! document (`app::forge_variants`).
use std::collections::HashMap;

use dear_imgui_rs::{Condition, Ui};

use crate::app::forge_variants::{
    ForgeAction, Thing, target_text, things, value_from_text, value_text, value_type,
};
use crate::app::{Action, App, Doc};
use crate::theme::{self, icon};

/// The window's UI-only state.
#[derive(Default)]
pub struct ForgeVariantUi {
    /// The variant the drafts are of.
    shown: Option<String>,
    /// A field's box by `<target>/<field>`: the value's text when the box
    /// was filled, and what the box holds. A value changed elsewhere (an
    /// undo, the API) fills the box again.
    drafts: HashMap<String, (String, String)>,
    /// Why the last text was not a value.
    error: Option<String>,
}

/// A thing's row: what it places, its block and where it comes from.
pub(crate) fn row_label(thing: &Thing) -> String {
    let item = thing.item().map_or_else(
        || thing.block.name().to_owned(),
        |tag| {
            let leaf = tag.path.rsplit('\\').next().unwrap_or(&tag.path).to_owned();
            format!("{leaf}  {}", thing.block.name())
        },
    );
    let origin = match (thing.added(), thing.removed) {
        (true, _) => "added",
        (false, true) => "removed from the map",
        (false, false) => "the map's own, edited",
    };
    format!("{item}  [{origin}]  {} fields", thing.fields.len())
}

/// The window, when the front document is a Forge map variant.
pub fn window(ui: &Ui, app: &App, st: &mut ForgeVariantUi, actions: &mut Vec<Action>) {
    if app.front_doc() != Some(Doc::Forge) || app.tool != app.doc_tool(Doc::Forge) {
        return;
    }
    let Some(open) = &app.forge_variants.open else {
        return;
    };
    let asset = open.asset();
    if st.shown.as_deref() != Some(asset.as_str()) {
        *st = ForgeVariantUi {
            shown: Some(asset),
            ..ForgeVariantUi::default()
        };
    }
    let rows = things(&open.doc);
    ui.window("Forge variant###forge-variant")
        .size([640.0, 700.0], Condition::FirstUseEver)
        .build(|| {
            ui.text(format!("{} {}", icon::MAP, open.doc.title));
            ui.same_line();
            ui.text_disabled(format!("map variant of {}", open.map));
            if !open.doc.author.is_empty() {
                ui.same_line();
                ui.text_disabled(format!("by {}", open.doc.author));
            }
            ui.text_disabled(match open.package() {
                Some(package) => format!("a map-variant item of {package}"),
                None => "saved in the game: Save adds it to the edit target as a map-variant item".to_owned(),
            });
            ui.text_disabled(format!(
                "{} things{}",
                rows.len(),
                if open.dirty() { "  (unsaved edits)" } else { "" }
            ));
            if let Some(e) = &st.error {
                ui.text_colored(theme::ERROR, e);
            }
            ui.separator();
            if rows.is_empty() {
                ui.text_disabled("This variant places and edits nothing.");
            }
            let mut error = None;
            let mut edited = false;
            for thing in &rows {
                let target = target_text(thing.target);
                let selected = open.selected.as_deref() == Some(target.as_str());
                if ui
                    .selectable_config(format!("{}##forge-thing-{target}", row_label(thing)))
                    .selected(selected)
                    .build()
                {
                    actions.push(Action::Forge(ForgeAction::Select(
                        (!selected).then(|| target.clone()),
                    )));
                }
                if !selected {
                    continue;
                }
                ui.indent();
                ui.text_disabled(&target);
                for (field, value) in &thing.fields {
                    let key = format!("{target}/{field}");
                    let now = value_text(value);
                    let draft = st
                        .drafts
                        .entry(key.clone())
                        .or_insert_with(|| (now.clone(), now.clone()));
                    if draft.0 != now {
                        *draft = (now.clone(), now.clone());
                    }
                    ui.set_next_item_width(280.0);
                    ui.input_text(format!("{field}##forge-field-{key}"), &mut draft.1)
                        .build();
                    if ui.is_item_deactivated_after_edit() {
                        edited = true;
                        match value_from_text(value, &draft.1) {
                            Ok(value) => actions.push(Action::Forge(ForgeAction::Set {
                                target: target.clone(),
                                field: field.clone(),
                                value,
                            })),
                            Err(e) => error = Some(format!("{field}: {e}")),
                        }
                    }
                    ui.same_line();
                    ui.text_disabled(value_type(value));
                }
                if thing.fields.is_empty() {
                    ui.text_disabled("The variant writes no field of this entry.");
                }
                if let Some(piece) = &thing.piece {
                    ui.text_disabled(format!("Block-out piece: {piece}"));
                }
                for note in &thing.notes {
                    ui.text_disabled(note);
                }
                ui.unindent();
            }
            if edited {
                st.error = error;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_doc::{BlockKind, Endpoint, OpId, TagKey, Value};

    #[test]
    fn a_row_names_what_it_places_its_block_and_where_it_comes_from() {
        let mut thing = Thing {
            target: Endpoint::Added(OpId(1)),
            block: BlockKind::Crates,
            removed: false,
            fields: vec![(
                "palette index".to_owned(),
                Value::Tag(Some(TagKey {
                    group: "bloc".into(),
                    path: "objects\\invented\\crate".into(),
                })),
            )],
            notes: Vec::new(),
            piece: None,
        };
        let block = BlockKind::Crates.name();
        assert_eq!(row_label(&thing), format!("crate  {block}  [added]  1 fields"));
        thing.target = Endpoint::base(BlockKind::Crates, 4);
        thing.fields.clear();
        thing.removed = true;
        assert_eq!(row_label(&thing), format!("{block}  [removed from the map]  0 fields"));
    }
}
