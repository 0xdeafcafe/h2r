//! The seam rich editors join the field panel by
//! (`docs/editor/rich-editors.md`, "One seam for every rich view").
//!
//! A rich view is one of two shapes. A **field view** is a `view=<word>`
//! on a field's definition line ([`tags::defs::FieldView`]); the row keeps
//! its plain form and `tag.fields` gives the word in its `view` key. A
//! **tag view** is attached to a tag's group and shows as a tab beside
//! **Fields** ([`tabs`]); the table is [`TAG_VIEWS`], and each slice that
//! adds a view adds one line to it (the Shader tab, `super::shader`).
//! Nothing here writes: a view reads and writes through `TagDoc` (or the
//! Scenario session) like the panel does.

use dear_imgui_rs::Ui;
use map::{FourCC, TagId};
use serde_json::{Value, json};
use tags::registry::{Field, FieldView};

use super::PanelState;
use crate::App;
use crate::app::Action;
use crate::ui::map_list::MapListUi;
use crate::viewport::Gpu;

/// The id of the plain field panel's tab, and of the tab `tag.view` shows
/// when none is named.
pub const FIELDS_TAB: &str = "fields";

/// What a tag view draws with beside the app: the GPU and the picture
/// cache for its images, the field panel's state (its colour and pick
/// dialogs, so a view's rows open the panel's own), and each view's own
/// state.
pub struct ViewCx<'a, 'g> {
    pub gpu: &'a mut Gpu<'g>,
    pub pictures: &'a mut MapListUi,
    pub panel: &'a mut PanelState,
    pub shader: &'a mut super::shader::ShaderUi,
}

/// How a tag view draws itself below the tab bar: the tag it shows and
/// the actions it pushes. A view that cannot model the tag says why and
/// leaves **Show fields** to the bar; it never draws part of a picture.
pub type DrawFn = fn(&Ui, &App, TagId, &mut ViewCx<'_, '_>, &mut Vec<Action>);

/// A tag view: a tab for every tag of its groups.
pub struct TagView {
    /// The id `tag.view`'s `tab` takes (`sound`, `shader`).
    pub id: &'static str,
    /// The tab's words.
    pub label: &'static str,
    /// The groups it applies to.
    pub groups: &'static [FourCC],
    pub draw: DrawFn,
}

/// The tag views, in tab order. Each rich-editor slice adds its own.
pub const TAG_VIEWS: &[TagView] = &[TagView {
    id: "shader",
    label: "Shader",
    groups: &[crate::app::shader::SHAD],
    draw: super::shader::draw,
}];

/// One tab of a tag's panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: &'static str,
    pub label: &'static str,
}

/// The tabs of a tag of `group` in `table`: **Fields** first, then each
/// view of the group in table order. One tab (**Fields**) means no tab bar.
pub fn tabs_in(table: &[TagView], group: FourCC) -> Vec<Tab> {
    let mut out = vec![Tab {
        id: FIELDS_TAB,
        label: "Fields",
    }];
    out.extend(
        table
            .iter()
            .filter(|v| v.groups.contains(&group))
            .map(|v| Tab {
                id: v.id,
                label: v.label,
            }),
    );
    out
}

/// [`tabs_in`] over [`TAG_VIEWS`].
pub fn tabs(group: FourCC) -> Vec<Tab> {
    tabs_in(TAG_VIEWS, group)
}

/// The tab shown for a tag of `group` when `want` is asked for: `want`
/// when the group has it, else **Fields** (a tab chosen for one tag does
/// not follow the selection to a tag that lacks it).
pub fn tab_in(table: &[TagView], group: FourCC, want: &str) -> &'static str {
    tabs_in(table, group)
        .into_iter()
        .find(|t| t.id == want)
        .map_or(FIELDS_TAB, |t| t.id)
}

/// [`tab_in`] over [`TAG_VIEWS`].
pub fn tab(group: FourCC, want: &str) -> &'static str {
    tab_in(TAG_VIEWS, group, want)
}

/// The tag view a tab id names, for a tag of `group`.
pub fn tag_view(group: FourCC, id: &str) -> Option<&'static TagView> {
    TAG_VIEWS
        .iter()
        .find(|v| v.id == id && v.groups.contains(&group))
}

/// The `view` key of a `tag.fields` row: the field's `view=` word, or
/// null when it has none.
pub fn view_json(field: &Field) -> Value {
    field.view.map_or(Value::Null, |v| json!(v.word()))
}

/// What a view word names, in words: the stored form, for the row's
/// tooltip and the dialogs that open from it.
pub fn stored_form(view: FieldView) -> &'static str {
    match view {
        FieldView::Rgb => "three floats, 0 to 1",
        FieldView::Argb => "four floats, alpha first",
        FieldView::Rgb8 => "packed 32-bit colour, top byte unused",
        FieldView::Argb8 => "packed 32-bit colour, alpha in the top byte",
        FieldView::Xrgb8 => "packed 32-bit colour, top byte unused",
        FieldView::HdrRgb => "three floats, values above 1 allowed",
        FieldView::Function => "a function curve in a data reference",
    }
}

/// Draw the tab bar of a tag whose group has tag views, and say which tab
/// is shown (`shown` as [`tab`] gives it). Returns the tab clicked, if it
/// is not the shown one. Draws nothing for a group with only **Fields**.
pub fn tab_bar(ui: &Ui, group: FourCC, shown: &str) -> Option<&'static str> {
    let tabs = tabs(group);
    if tabs.len() < 2 {
        return None;
    }
    let mut chosen = None;
    for (i, t) in tabs.iter().enumerate() {
        if i > 0 {
            ui.same_line();
        }
        if t.id == shown {
            ui.text(t.label);
        } else if ui.small_button(format!("{}##tab-{}", t.label, t.id)) {
            chosen = Some(t.id);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing(_: &Ui, _: &App, _: TagId, _: &mut ViewCx<'_, '_>, _: &mut Vec<Action>) {}

    const SOUND: FourCC = FourCC(*b"snd!");
    const SHADER: FourCC = FourCC(*b"shad");

    fn table() -> Vec<TagView> {
        vec![
            TagView {
                id: "sound",
                label: "Sound",
                groups: &[SOUND],
                draw: nothing,
            },
            TagView {
                id: "wave",
                label: "Wave",
                groups: &[SOUND, SHADER],
                draw: nothing,
            },
        ]
    }

    #[test]
    fn a_group_with_no_view_has_only_the_fields_tab() {
        let t = tabs_in(&table(), FourCC(*b"weap"));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].id, FIELDS_TAB);
        assert_eq!(tabs_in(&[], SOUND).len(), 1);
    }

    #[test]
    fn views_follow_fields_in_table_order() {
        let ids: Vec<_> = tabs_in(&table(), SOUND).iter().map(|t| t.id).collect();
        assert_eq!(ids, ["fields", "sound", "wave"]);
        let ids: Vec<_> = tabs_in(&table(), SHADER).iter().map(|t| t.id).collect();
        assert_eq!(ids, ["fields", "wave"]);
    }

    #[test]
    fn a_tab_the_group_lacks_falls_back_to_fields() {
        let t = table();
        assert_eq!(tab_in(&t, SOUND, "sound"), "sound");
        assert_eq!(tab_in(&t, SHADER, "sound"), FIELDS_TAB);
        assert_eq!(tab_in(&t, SOUND, "nonsense"), FIELDS_TAB);
        assert_eq!(tab_in(&t, SOUND, ""), FIELDS_TAB);
    }

    fn field(view: Option<FieldView>) -> Field {
        Field {
            offset: 0,
            name: "tint".into(),
            section: String::new(),
            named: true,
            kind: tags::registry::Kind::Float {
                count: 3,
                hint: tags::registry::Hint::Colour,
            },
            meaning: String::new(),
            doc: None,
            applies: None,
            label: None,
            unit: None,
            axes: Vec::new(),
            view,
        }
    }

    #[test]
    fn a_rows_view_key_is_its_word_or_null() {
        assert_eq!(view_json(&field(None)), Value::Null);
        for v in FieldView::ALL {
            assert_eq!(view_json(&field(Some(v))), json!(v.word()));
        }
    }

    #[test]
    fn every_view_word_has_words_for_its_form() {
        for v in FieldView::ALL {
            assert!(!stored_form(v).is_empty());
        }
    }
}
