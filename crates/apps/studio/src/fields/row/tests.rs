//! The row model on the invented weapon of `app/tag_tests.rs` (no game
//! data).
use super::*;
use crate::app::tag_api::set_field;
use crate::app::tag_doc::SCENARIO_OWNED;
use crate::app::{Action, tag_tests};
use scenario_edit::fixture;

fn context(link: Link) -> Context<'static> {
    Context {
        link,
        issues: IssueIndex::empty(),
        prefix: "",
    }
}

#[test]
fn derived_labels_read_as_words() {
    assert_eq!(words("speed"), "Speed");
    assert_eq!(words("lod_offset"), "LOD offset");
    assert_eq!(words("base_map"), "Base map");
    assert_eq!(words("ai_fov_id"), "AI FOV ID");
    assert_eq!(words("hud_uv_bsp"), "HUD UV BSP");
    assert_eq!(words("_trailing__gap_"), "Trailing gap");
}

#[test]
fn every_kind_has_its_type_words() {
    use map::FourCC;
    use std::sync::Arc;
    use tags::registry::Layout;
    let float = |hint| Kind::Float { count: 1, hint };
    let block = Kind::Block(Arc::new(Layout {
        name: "test".into(),
        size: 4,
        align: 4,
        fields: Vec::new(),
    }));
    for (kind, words) in [
        (
            Kind::Int {
                bytes: 2,
                signed: false,
                count: 1,
            },
            "Whole number",
        ),
        (float(Hint::Plain), "Decimal number"),
        (float(Hint::Angle), "Angle"),
        (float(Hint::Colour), "Colour"),
        (float(Hint::Bounds), "Range"),
        (
            Kind::Enum {
                bytes: 2,
                values: Vec::new(),
            },
            "Choice",
        ),
        (
            Kind::Flags {
                bytes: 2,
                bits: Vec::new(),
            },
            "Switches",
        ),
        (Kind::StringId, "Name"),
        (
            Kind::TagRef {
                groups: vec![FourCC(*b"itmc")],
            },
            "Tag reference",
        ),
        (Kind::Datum { groups: Vec::new() }, "Tag reference"),
        (block, "List"),
        (Kind::Data, "Bytes"),
        (Kind::Raw, "Internal value"),
        (Kind::Bsp, "Internal value"),
        (Kind::Word, "Internal value"),
        (Kind::Unknown { len: 4 }, "Unnamed value"),
        (Kind::Pad { len: 8 }, "Padding"),
    ] {
        assert_eq!(type_words(&kind), words, "{kind:?}");
    }
}

#[test]
fn read_only_rows_say_why_in_a_sentence() {
    let plain = Kind::Unknown { len: 4 };
    assert_eq!(reason(&plain, false, None), None);
    assert_eq!(reason(&Kind::Raw, false, None), Some(STRUCTURAL));
    assert_eq!(
        STRUCTURAL,
        "Set by the editor. Add or remove list items to change it."
    );
    assert_eq!(reason(&Kind::Pad { len: 4 }, false, None), Some(PADDING));
    assert_eq!(reason(&plain, false, Some("refused")), Some("refused"));
    assert_eq!(reason(&plain, true, Some("refused")), Some(EDITED_IN_SCENE));
    // Data and lists are edited (as hex; by add and remove).
    assert_eq!(reason(&Kind::Data, false, None), None);
}

#[test]
fn one_row_per_field_with_labels_types_and_paths() {
    let t = crate::play::testing::Temp::new("field-rows-model");
    let (app, _, _) = tag_tests::app(&t.0);
    let weap = tag_tests::weap_id();
    let layout = app.tags.layout(weap).unwrap().clone();
    let root = rows(
        &app.tags,
        weap,
        tag_tests::WEAP,
        &layout,
        &context(Link::Off),
    );
    assert_eq!(root.len(), layout.fields.len());
    let seen: Vec<_> = root
        .iter()
        .map(|r| (r.label.as_str(), r.type_words, r.path.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("Speed", "Decimal number", "speed"),
            ("Flags", "Switches", "flags"),
            ("Mode", "Choice", "mode"),
            ("Parts", "List", "parts"),
        ]
    );
    assert!(root.iter().all(|r| r.read_only.is_none() && !r.edited));
    let parts = &root[3];
    assert_eq!(parts.list.map(|l| l.count), Some(2));
    assert_eq!(parts.summary.as_deref(), Some("2 items"));
    let json = parts.json(&app.tags, weap, tag_tests::WEAP);
    assert_eq!(json["value"], 2);
    assert_eq!(json["count"], 2);
    let flags = root[1].json(&app.tags, weap, tag_tests::WEAP);
    assert_eq!(
        flags["bits"],
        serde_json::json!([{"bit": 0, "name": "loud"}, {"bit": 1, "name": "quiet"}])
    );
    let mode = root[2].json(&app.tags, weap, tag_tests::WEAP);
    assert_eq!(
        mode["choices"][2],
        serde_json::json!({"value": 2, "name": "auto"})
    );
    assert_eq!(mode["value"], "auto");

    // A list element's rows carry their path from the root.
    let (at, child) = crate::app::tag_api::field_element(&app.tags, weap, "parts[1]").unwrap();
    let cx = Context {
        prefix: "parts[1].",
        ..context(Link::Off)
    };
    let nested = rows(&app.tags, weap, at, &child, &cx);
    let seen: Vec<_> = nested
        .iter()
        .map(|r| (r.label.as_str(), r.type_words, r.path.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("Item", "Tag reference", "parts[1].item"),
            ("Value", "Whole number", "parts[1].value"),
        ]
    );
    let item = nested[0].json(&app.tags, weap, at);
    assert_eq!(item["groups"], serde_json::json!(["itmc"]));

    // The scenario tag's rows are read-only with the Tags tool's reason.
    let scenario = fixture::id(1);
    let layout = app.tags.layout(scenario).unwrap().clone();
    let address = app.tags.tag(scenario).unwrap().address;
    let owned = rows(&app.tags, scenario, address, &layout, &context(Link::Off));
    assert!(!owned.is_empty());
    assert!(owned.iter().all(|r| r.read_only == Some(SCENARIO_OWNED)));
    assert!(
        owned
            .iter()
            .all(|r| !r.field.named && r.label.contains(" at +0x"))
    );
}

#[test]
fn a_changed_row_has_its_file_value_and_chips_need_a_game() {
    let t = crate::play::testing::Temp::new("field-rows-edited");
    let (mut app, _, _) = tag_tests::app(&t.0);
    let weap = tag_tests::weap_id();
    let edit = set_field(&app.tags, weap, "speed", &serde_json::json!(3.0)).unwrap();
    app.dispatch(Action::Tag(edit));
    let layout = app.tags.layout(weap).unwrap().clone();

    let off = rows(
        &app.tags,
        weap,
        tag_tests::WEAP,
        &layout,
        &context(Link::Off),
    );
    let speed = &off[0];
    assert!(speed.edited);
    assert!(!off[1].edited);
    let json = speed.json(&app.tags, weap, tag_tests::WEAP);
    assert_eq!(
        (json["value"].as_f64(), json["file_value"].as_f64()),
        (Some(3.0), Some(2.5))
    );
    assert_eq!(json["effect"]["id"], "not_known");
    assert!(off.iter().all(|r| r.chip.is_none()), "no chip with no game");

    let viewing = rows(
        &app.tags,
        weap,
        tag_tests::WEAP,
        &layout,
        &context(Link::Viewing),
    );
    let chips: Vec<_> = viewing.iter().map(|r| r.chip).collect();
    // A changed field of unknown effect says so; an unchanged one has no
    // chip; a list applies after reload.
    assert_eq!(chips, [Some("Not known"), None, None, Some("After reload")]);
    assert_eq!(
        chip(Link::Editing, Effect::Now, false, true),
        None,
        "read-only"
    );
    assert_eq!(chip(Link::Editing, Effect::Now, false, false), Some("Now"));
}

#[test]
fn summaries_count_items_and_room() {
    assert_eq!(items(0, 0), "Empty");
    assert_eq!(items(1, 1), "1 item");
    assert_eq!(items(2, 4), "2 items, room for 4");
    assert_eq!(bytes(1), "1 byte");
    assert_eq!(bytes(16), "16 bytes");
}
