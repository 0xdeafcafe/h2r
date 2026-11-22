//! `ui.export_edits` over the invented scenario map with an invented
//! screen tag added, saved as `mainmenu.map` (no game data): edits made
//! in the map's tag document come back as a retail edit file.

use boot::ui_kit::retail_edits::{self, BlockName, Edit};
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, Map, TagId};

use super::*;
use crate::app::{TagAction, tag_api};

const SCREEN: &str = r"ui\screens\invented\lobby";

/// The invented map with one screen tag: a pane with two texts.
fn menu_map(t: &Temp) -> PathBuf {
    let mut m = Map::from_reader(std::io::Cursor::new(fixture::fixture())).unwrap();
    let mut set = TagSet::from_map(&mut m).unwrap();
    // Room in the index for one more tag, as the screen injector makes it.
    let pinned = set.pinned.as_mut().unwrap();
    pinned.clear_index_tail();
    pinned.index_entries_end = pinned.index.len() as u32;
    set.check_tag_memory = false;
    set.add_group(FourCC(*b"wgit"), None, None);
    let mut root = Block::zeroed(0x68);
    root.put_u16(4, 14);
    root.tag_ref(0x18, TagId::NULL);
    root.string_id(0x2c, "invented_header");
    let mut pane = Block::zeroed(0x4c);
    let mut texts = Block::zeroed(0x2c * 2);
    for (i, r) in [[75i16, -260, 55, 70], [95, -260, 75, 70]].into_iter().enumerate() {
        for (k, v) in r.into_iter().enumerate() {
            texts.put_u16(0x2c * i + 0x1c + 2 * k, v as u16);
        }
        texts.string_id((0x2c * i + 0x24) as u32, &format!("invented_text_{i}"));
    }
    pane.block(0x1c, 2, texts);
    root.block(0x20, 1, pane);
    set.add_tag(FourCC(*b"wgit"), SCREEN, TagPayload::Built(root))
        .unwrap();
    let path = t.path("mainmenu.map");
    std::fs::write(&path, set.build().unwrap().0).unwrap();
    path
}

#[test]
fn ui_export_edits_writes_the_open_map_s_retail_edits_as_a_file() {
    let t = Temp::new("ui-export");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = menu_map(&t);
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    assert_eq!(app.loaded_map(), Some("mainmenu"));
    app.sync_tags();
    assert!(app.tags.is_open(), "{:?}", app.tags.error);
    let id = app
        .tags
        .tags()
        .iter()
        .find(|e| e.group == ::tags::ui::SCREEN)
        .expect("the invented screen")
        .id;
    let out = t.path("edits");
    let export = |api: &mut Api, app: &mut App| {
        call(api, app, m::UI_EXPORT_EDITS, json!({"path": out.display().to_string()}))
    };

    // Nothing edited: nothing written.
    let r = export(&mut api, &mut app);
    assert_eq!(r["map"], "mainmenu");
    assert_eq!(r["written"], json!([]));
    assert_eq!(r["skipped"], json!([]));

    // A rect and a string ID set in the tag document, as the UI Screens
    // tool sets them.
    match tag_api::set_field(
        &app.tags,
        id,
        "panes[0].text_blocks[0].rect",
        &json!([135, -258, 95, 45]),
    )
    .unwrap()
    {
        TagAction::Write(w) => app.tags.write(w).unwrap(),
        other => panic!("{other:?}"),
    }
    match tag_api::set_field(
        &app.tags,
        id,
        "panes[0].text_blocks[1].text",
        &json!("lobby_mode_change"),
    )
    .unwrap()
    {
        TagAction::SetString {
            tag, address, text, ..
        } => app.tags.set_string(tag, address, &text, "named").unwrap(),
        TagAction::Write(w) => app.tags.write(w).unwrap(),
        other => panic!("{other:?}"),
    }
    let r = export(&mut api, &mut app);
    assert_eq!(r["skipped"], json!([]), "{r}");
    let written = r["written"].as_array().unwrap();
    assert_eq!(written.len(), 1, "{r}");
    assert_eq!(written[0]["screen"], SCREEN);
    assert_eq!(written[0]["pane"], 0);
    assert_eq!(written[0]["edits"], 2);
    let path = written[0]["path"].as_str().unwrap();
    assert!(path.ends_with("lobby.yaml"), "{path}");
    let file = retail_edits::parse("lobby", &std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(file.screen, SCREEN);
    assert_eq!(file.map, "mainmenu");
    assert_eq!(file.pane, 0);
    let b = |s: &str| s.parse::<BlockName>().unwrap();
    assert_eq!(
        file.edits,
        vec![
            Edit::SetRect {
                block: b("text.0"),
                rect: [135, -258, 95, 45],
            },
            Edit::SetString {
                block: b("text.1"),
                name: "lobby_mode_change".into(),
            },
        ]
    );

    // One screen by name; a name the map lacks is the caller's mistake.
    let one = call(
        &mut api,
        &mut app,
        m::UI_EXPORT_EDITS,
        json!({"path": out.display().to_string(), "screen": SCREEN}),
    );
    assert_eq!(one["written"].as_array().unwrap().len(), 1);
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::UI_EXPORT_EDITS,
            json!({"path": out.display().to_string(), "screen": r"ui\screens\nowhere"})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_EXPORT_EDITS, json!({}))),
        code::INVALID_PARAMS
    );
}
