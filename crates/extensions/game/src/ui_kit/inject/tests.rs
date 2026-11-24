//! Offline tests of the injected screens: a synthetic mainmenu whose UI
//! tags are invented here (every byte), the generated `wgit`s decoded with
//! our own decoders and checked as `h2 ui --all` checks a map.

use std::collections::BTreeSet;
use std::io::Cursor;

use map::build::TagSet;
use map::{Map, TagId};
use tags::ui::{self, MapSource, Screen, UiTag, check::check_map};

use super::fixture::{TEMPLATES, menu};
use super::tag::{Template, screen_block};
use super::*;

fn whole_plan() -> Vec<Entry> {
    plan().expect("every screen has a doc and a sample")
}

/// Screen tag `name` of `map` decoded, with its tag id.
fn screen_of(map: &mut Map<Cursor<Vec<u8>>>, name: &str) -> (TagId, Screen) {
    let id = map
        .tags()
        .iter()
        .find(|t| t.group == ui::SCREEN && t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no wgit {name}"))
        .id;
    match MapSource::new(map).decode_tag(id).unwrap().tag {
        UiTag::Screen(s) => (id, s),
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_screen_in_names_gets_a_tag_and_an_id() {
    let plan = whole_plan();
    assert_eq!(plan.len(), NAMES.len());
    let (mut ids, mut tags) = (BTreeSet::new(), BTreeSet::new());
    for (e, name) in plan.iter().zip(NAMES) {
        assert_eq!(e.name, name);
        assert_eq!(id_of(name), Some(e.id));
        assert!(is_injected_id(e.id), "{name}: {}", e.id);
        assert_eq!(e.tag, tag_name(name));
        assert!(ids.insert(e.id), "{name}'s id is another's");
        assert!(tags.insert(e.tag.clone()), "{name}'s tag name is another's");
    }
    assert!(!is_injected_id(ID_BASE - 1) && !is_injected_id(ID_BASE + NAMES.len() as i16));
    assert_eq!(id_of("no such screen"), None);

    // In a map: every name has a wgit with its id, and the wgtz lists it.
    let added = add_screens(&menu(), &plan).unwrap();
    assert_eq!(added.skipped, vec![]);
    assert_eq!(added.injected.len(), NAMES.len());
    let mut map = Map::from_reader(Cursor::new(added.map)).unwrap();
    let wgtz = map
        .tags()
        .iter()
        .find(|t| t.group == ui::UI_GLOBALS)
        .unwrap()
        .id;
    let listed: Vec<TagId> = match MapSource::new(&mut map).decode_tag(wgtz).unwrap().tag {
        UiTag::UiGlobals(g) => g.screens.iter().filter_map(|r| r.id).collect(),
        other => panic!("{other:?}"),
    };
    assert_eq!(listed.len(), TEMPLATES.len() + NAMES.len());
    for e in &plan {
        let (tag, screen) = screen_of(&mut map, &e.tag);
        assert_eq!(screen.screen_id, e.id, "{}", e.name);
        assert!(listed.contains(&tag), "{} is not in the wgtz", e.name);
    }
}

#[test]
fn a_screen_id_and_a_tag_name_are_stable_and_distinct_from_retail() {
    assert_eq!(id_of(NAMES[0]), Some(ID_BASE));
    assert_eq!(tag_name("coop.friends"), "ui\\screens\\h2recomp\\coop_friends");
    for id in TEMPLATES {
        assert!(!is_injected_id(id));
    }
    assert_eq!(template::CONTROLS, 39);
    assert_eq!(template::INGAME, 200);
}

#[test]
fn generated_screens_decode_and_pass_the_ui_cross_checks() {
    let added = add_screens(&menu(), &whole_plan()).unwrap();
    let mut map = Map::from_reader(Cursor::new(added.map)).unwrap();
    let c = check_map(&mut map);
    assert_eq!(c.failures, Vec::<String>::new());
    assert_eq!(c.groups[&ui::SCREEN].tags, TEMPLATES.len() + NAMES.len());
    assert_eq!(c.groups[&ui::SCREEN].failures, 0);
    assert_eq!(c.groups[&ui::UI_GLOBALS].failures, 0);
    // The invented map itself passes, so the check is a check of the tags.
    let mut plain = Map::from_reader(Cursor::new(menu())).unwrap();
    assert_eq!(check_map(&mut plain).failures, Vec::<String>::new());
}

/// What `list_class::copy_tag` writes: the template's root, one pane with
/// its other blocks, the list placed by the layout, the frame art left of
/// the layout's edge.
#[test]
fn a_list_screen_s_tag_is_the_copy_the_class_used_to_make() {
    let plan = whole_plan();
    let added = add_screens(&menu(), &plan).unwrap();
    let mut map = Map::from_reader(Cursor::new(added.map)).unwrap();
    let (_, template) = screen_of(&mut map, "ui\\screens\\retail_39");
    let mut moved = 0;
    for e in plan.iter().filter(|e| e.shape == Shape::List) {
        let (_, s) = screen_of(&mut map, &e.tag);
        assert_eq!(s.flags, template.flags, "{}: the root as it is", e.name);
        assert_eq!(s.strings, template.strings, "{}", e.name);
        assert_eq!(s.header, template.header, "{}", e.name);
        match e.list {
            ListPlace::Controls if e.animation == TransitionStyle::Default => {
                assert_eq!(s.panes, template.panes, "{}: retail's own pane", e.name);
            }
            ListPlace::Moved {
                skin,
                anchor,
                bitmaps_left_of,
                visible,
            } => {
                moved += 1;
                assert_eq!(s.panes.len(), 1, "{}", e.name);
                let (p, t) = (&s.panes[0], &template.panes[0]);
                assert_eq!(p.lists.len(), 1);
                let l = &p.lists[0];
                assert_eq!(l.skin, skin, "{}", e.name);
                assert_eq!([l.anchor.x, l.anchor.y], anchor, "{}", e.name);
                let rows = if visible > 0 { visible } else { t.lists[0].visible_items };
                assert_eq!(l.visible_items, rows, "{}", e.name);
                assert_eq!(l.item_value, t.lists[0].item_value);
                let kept = t
                    .bitmaps
                    .iter()
                    .filter(|b| b.top_left.x < bitmaps_left_of)
                    .count();
                assert_eq!(p.bitmaps.len(), kept.min(4), "{}: frame art kept", e.name);
                assert_eq!(p.text_blocks, t.text_blocks, "{}: the pane's other blocks", e.name);
            }
            _ => {}
        }
    }
    assert!(moved > 0, "the plan has screens with a moved list");
}

#[test]
fn an_animation_style_sets_the_pane_and_list_tracks() {
    let t = Template {
        id: 39,
        root: vec![0; 0x68],
        pane: Some(super::tag::PaneRaw {
            bytes: {
                let mut b = vec![0u8; 0x4c];
                b[2] = 5;
                b
            },
            list: Some(vec![0; 0x18]),
            bitmaps: vec![],
        }),
    };
    let wigl = super::tag::Wigl {
        animations: 30,
        skins: 10,
    };
    let entry = |animation, list| Entry {
        name: "t".into(),
        id: ID_BASE,
        tag: tag_name("t"),
        shape: Shape::List,
        list,
        animation,
    };
    // A slot past the collections is the template's own, as at run time.
    assert_eq!(tracks(TransitionStyle::Retail { pane: 31, rows: 2 }, 30), None);
    assert_eq!(tracks(TransitionStyle::Retail { pane: 23, rows: 24 }, 30), Some((23, 24)));
    assert_eq!(tracks(TransitionStyle::Off, 30), Some((0, 0)));
    assert_eq!(tracks(TransitionStyle::Default, 30), None);
    let b = screen_block(
        &entry(TransitionStyle::Retail { pane: 23, rows: 24 }, ListPlace::Controls),
        &t,
        &wigl,
    )
    .unwrap();
    let pane = match &b.fixups.iter().find(|f| f.offset == 0x20).unwrap().kind {
        map::build::FixupKind::Block { count: 1, child } => child,
        other => panic!("{other:?}"),
    };
    assert_eq!(&pane.bytes[2..4], 23u16.to_le_bytes());
    let list = match &pane.fixups.iter().find(|f| f.offset == 0x0c).unwrap().kind {
        map::build::FixupKind::Block { count: 1, child } => child,
        other => panic!("{other:?}"),
    };
    assert_eq!(&list.bytes[0xc..0xe], 24u16.to_le_bytes());
    // A skin the map lacks is a named error, not a quiet fallback.
    let far = ListPlace::Moved {
        skin: 10,
        anchor: [0, 0],
        bitmaps_left_of: 0,
        visible: 0,
    };
    let why = screen_block(&entry(TransitionStyle::Default, far), &t, &wigl).unwrap_err();
    assert!(why.contains("skin 10") && why.starts_with("t:"), "{why}");
    // A template with no pane cannot make one.
    let bare = Template { pane: None, ..t };
    assert!(
        screen_block(&entry(TransitionStyle::Off, ListPlace::Controls), &bare, &wigl)
            .unwrap_err()
            .contains("no pane")
    );
}

#[test]
fn the_kind_of_a_doc_picks_the_shape_and_the_template() {
    assert_eq!(entry("coop").unwrap().shape, Shape::List);
    assert_eq!(entry("forge.hud").unwrap().shape, Shape::Bare);
    assert_eq!(
        entry("kit.dialog").unwrap().shape,
        Shape::Dialog { base: 222 },
        "settings: Quick Options"
    );
    assert_eq!(entry("kit.dialog.large").unwrap().shape, Shape::Dialog { base: 240 });
    assert_eq!(entry("kit.picker").unwrap().shape, Shape::Dialog { base: 223 });
    assert_eq!(entry("coop").unwrap().templates(), vec![39, 200]);
    assert_eq!(entry("kit.picker").unwrap().templates(), vec![223]);
    assert!(entry("no such screen").unwrap_err().contains("NAMES"));
    // Only a list screen opens on its tag.
    assert_eq!(list_id("coop"), id_of("coop"));
    assert_eq!(list_id("kit.dialog"), None);
    assert_eq!(list_id("forge.hud"), None);
}

#[test]
fn a_screen_with_no_template_in_the_map_is_left_out_by_name() {
    // The in-game screens (200) are not in a mainmenu: nothing of the
    // plan's lists needs them there, but a dialog whose base is missing
    // says so, and the rest still go in.
    let mut plan = whole_plan();
    plan.retain(|e| e.name == "coop" || e.name == "kit.picker");
    plan[1].shape = Shape::Dialog { base: 99 };
    let added = add_screens(&menu(), &plan).unwrap();
    assert_eq!(added.injected.len(), 1);
    assert_eq!(added.skipped.len(), 1);
    assert_eq!(added.skipped[0].0, "kit.picker");
    assert!(added.skipped[0].1.contains("99"), "{:?}", added.skipped);
    // Nothing to add at all is an error, not an unchanged map.
    plan.remove(0);
    assert!(add_screens(&menu(), &plan).is_err());
    assert!(add_screens(b"not a map", &plan).is_err());
}

#[test]
fn the_rebuilt_map_round_trips() {
    let original = menu();
    let plan = whole_plan();
    let added = add_screens(&original, &plan).unwrap();
    assert_eq!(
        add_screens(&original, &plan).unwrap().map,
        added.map,
        "the same input builds the same map"
    );
    let mut rebuilt = Map::from_reader(Cursor::new(added.map.clone())).unwrap();
    rebuilt.validate_checksum().unwrap();
    let mut before = Map::from_reader(Cursor::new(original)).unwrap();
    assert_eq!(rebuilt.tags().len(), before.tags().len() + NAMES.len());
    // Every retail tag is as it was, but the wgtz that lists ours.
    for t in before.tags().to_vec() {
        if t.group == ui::UI_GLOBALS {
            continue;
        }
        assert_eq!(
            rebuilt.read_tag_bytes(t.id).unwrap(),
            before.read_tag_bytes(t.id).unwrap(),
            "{:?} changed",
            t.name
        );
    }
    // Taken apart again, it builds back to the same bytes.
    let again = TagSet::from_map(&mut rebuilt).unwrap().build().unwrap().0;
    assert_eq!(again, added.map);
}
