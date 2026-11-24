//! Offline tests of the retail edits: the format, each edit on an invented
//! menu map whose UI tags are made here (every byte), the export's diff,
//! the rewrite's cache key, and (with a disc) the shipped files on the
//! user's own `mainmenu.map`.

use std::io::Cursor;

use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, TagId};
use tags::ui::{self, MapSource, Screen, UiTag, check::check_map};

use super::apply::place_of;
use super::*;

const SKINS: usize = 64;
const ANIMATIONS: usize = 40;
const LOBBY: &str = "ui\\screens\\invented\\lobby";
const OTHER: &str = "ui\\screens\\invented\\other";
/// The invented lobby's pane 0 text, button and bitmap places.
const TEXTS: [Rect; 3] = [[75, -260, 55, 70], [95, -260, 75, 70], [-100, -50, -120, 50]];
const BUTTONS: [Rect; 2] = [[173, -274, 153, -118], [173, -44, 153, 61]];
const BITMAPS: [(i16, i16); 2] = [(-279, 174), (100, 120)];

fn add(set: &mut TagSet, group: &[u8; 4], name: &str, block: Block) -> TagId {
    set.add_group(FourCC(*group), None, None);
    set.add_tag(FourCC(*group), name, TagPayload::Built(block))
        .unwrap()
}

fn put_rect(b: &mut Block, at: usize, r: Rect) {
    for (k, v) in r.into_iter().enumerate() {
        b.put_u16(at + 2 * k, v as u16);
    }
}

/// An invented screen: pane 0 with a list, three texts, two buttons and
/// two bitmaps; pane 1 with one text.
fn screen(set: &mut TagSet, id: i16, name: &str, unic: TagId, bitm: TagId) -> TagId {
    let mut root = Block::zeroed(0x68);
    root.put_u16(4, id as u16);
    root.tag_ref(0x18, unic);
    root.string_id(0x2c, "invented_header");
    let mut panes = Block::zeroed(0x4c * 2);
    let mut list = Block::zeroed(0x18);
    list.put_u16(4, 2)
        .put_u16(6, 6)
        .put_u16(8, (-254i16) as u16)
        .put_u16(0xa, 87);
    panes.block(0x0c, 1, list);
    let mut texts = Block::zeroed(0x2c * TEXTS.len());
    for (i, r) in TEXTS.into_iter().enumerate() {
        put_rect(&mut texts, 0x2c * i + 0x1c, r);
        texts.put_u32(0x2c * i, i as u32);
        texts.string_id((0x2c * i + 0x24) as u32, &format!("invented_text_{i}"));
    }
    panes.block(0x1c, TEXTS.len() as u32, texts);
    let mut buttons = Block::zeroed(0x3c * BUTTONS.len());
    for (i, r) in BUTTONS.into_iter().enumerate() {
        put_rect(&mut buttons, 0x3c * i + 0x1c, r);
        buttons.tag_ref((0x3c * i + 0x24) as u32, bitm);
        buttons.string_id((0x3c * i + 0x30) as u32, &format!("invented_button_{i}"));
    }
    panes.block(0x04, BUTTONS.len() as u32, buttons);
    let mut bitmaps = Block::zeroed(0x38 * BITMAPS.len());
    for (i, (x, y)) in BITMAPS.into_iter().enumerate() {
        bitmaps
            .put_u16(0x38 * i + 0xc, x as u16)
            .put_u16(0x38 * i + 0xe, y as u16);
        bitmaps.tag_ref((0x38 * i + 0x18) as u32, bitm);
    }
    panes.block(0x24, BITMAPS.len() as u32, bitmaps);
    let mut second = Block::zeroed(0x2c);
    put_rect(&mut second, 0x1c, [10, -10, 0, 10]);
    second.string_id(0x24, "invented_pane1_text");
    panes.block(0x4c + 0x1c, 1, second);
    root.block(0x20, 2, panes);
    add(set, b"wgit", name, root)
}

/// A menu shaped like the real one where the UI cares (as
/// `inject::tests::menu`): one `wigl`, one `wgtz` listing the invented
/// lobby (screen 39, so it is the list screens' template) and another.
fn menu() -> Vec<u8> {
    let mut set = TagSet::new(CacheType::MainMenu, "mainmenu", "ui\\mainmenu");
    set.globals = add(&mut set, b"matg", "globals", Block::zeroed(0x284));
    set.scenario = add(&mut set, b"scnr", "ui\\mainmenu", Block::zeroed(0x3e0));
    let unic = add(&mut set, b"unic", "ui\\strings", Block::zeroed(0x20));
    let bitm = add(&mut set, b"bitm", "ui\\frame", Block::zeroed(0x4c));
    let mut skin = Block::zeroed(0x3c);
    skin.tag_ref(4, TagId::NULL);
    skin.block(0x14, 3, Block::zeroed(0x30));
    let skin = add(&mut set, b"skin", "ui\\skin", skin);
    let mut wigl = Block::zeroed(0x1c4);
    for i in 0..16 {
        wigl.tag_ref(0x90 + 8 * i, TagId::NULL);
    }
    for at in [0x110, 0x118, 0x140, 0x148, 0x150, 0x1b8] {
        wigl.tag_ref(at, TagId::NULL);
    }
    wigl.block(0x120, ANIMATIONS as u32, Block::zeroed(0x2c * ANIMATIONS));
    let mut skins = Block::zeroed(8 * SKINS);
    for i in 0..SKINS {
        skins.tag_ref(8 * i as u32, skin);
    }
    wigl.block(0x138, SKINS as u32, skins);
    let wigl = add(&mut set, b"wigl", "ui\\globals", wigl);
    let screens = [
        screen(&mut set, 39, LOBBY, unic, bitm),
        screen(&mut set, 41, OTHER, unic, bitm),
    ];
    let mut list = Block::zeroed(8 * screens.len());
    for (i, s) in screens.iter().enumerate() {
        list.tag_ref(8 * i as u32, *s);
    }
    let mut wgtz = Block::zeroed(0x20);
    wgtz.tag_ref(0, wigl);
    wgtz.block(8, screens.len() as u32, list);
    wgtz.tag_ref(0x10, TagId::NULL);
    wgtz.tag_ref(0x18, unic);
    add(&mut set, b"wgtz", "ui\\wgtz", wgtz);
    set.build().unwrap().0
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn tag_of(map: &Map<Cursor<Vec<u8>>>, name: &str) -> map::Tag {
    map.tags()
        .iter()
        .find(|t| t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no tag {name}"))
        .clone()
}

/// Screen `name`'s pane `pane` as its elements' bytes, read from the
/// tag's own bytes; a block another tag holds is left empty.
fn pane_bytes(map: &[u8], name: &str, pane: usize) -> PaneBytes {
    let mut m = Map::from_reader(Cursor::new(map.to_vec())).unwrap();
    let t = tag_of(&m, name);
    let base = t.location.unwrap().address;
    let b = m.read_tag_bytes(t.id).unwrap();
    let at = |a: u32, n: usize| {
        let o = a.checked_sub(base)? as usize;
        (o + n <= b.len()).then_some(o)
    };
    let mut out = PaneBytes {
        header: u32_at(&b, 0x2c),
        ..PaneBytes::default()
    };
    let p = at(u32_at(&b, 0x24), (pane + 1) * 0x4c).expect("the panes are in the tag") + pane * 0x4c;
    for kind in BlockKind::ALL {
        let f = p + kind.pane_field() as usize;
        let n = u32_at(&b, f) as usize * kind.size();
        if let Some(s) = at(u32_at(&b, f + 4), n).filter(|_| n > 0) {
            let bytes = b[s..s + n].to_vec();
            out.of_mut(kind).extend_from_slice(&bytes);
        }
    }
    out
}

/// Element `i` of `kind` in `p`.
fn element(p: &PaneBytes, kind: BlockKind, i: usize) -> &[u8] {
    &p.of(kind)[i * kind.size()..(i + 1) * kind.size()]
}

/// The name of string ID `id` in `map`.
fn string_name(map: &[u8], id: u32) -> Option<String> {
    let m = Map::from_reader(Cursor::new(map.to_vec())).unwrap();
    let s = m.strings().get((id & 0x00ff_ffff) as usize)?.clone()?;
    (s.len() == (id >> 24) as usize).then_some(s)
}

fn decoded(map: &[u8], name: &str) -> Screen {
    let mut m = Map::from_reader(Cursor::new(map.to_vec())).unwrap();
    let id = tag_of(&m, name).id;
    match MapSource::new(&mut m).decode_tag(id).unwrap().tag {
        UiTag::Screen(s) => s,
        other => panic!("{other:?}"),
    }
}

fn file(screen: &str, pane: usize, edits: Vec<Edit>) -> EditFile {
    EditFile {
        screen: screen.into(),
        map: DEFAULT_MAP.into(),
        pane,
        edits,
    }
}

fn b(s: &str) -> BlockName {
    s.parse().unwrap()
}

fn apply_one(map: &[u8], f: EditFile) -> Edited {
    apply_all(map, &[("invented".into(), f)]).unwrap()
}

#[test]
fn the_shipped_pregame_lobby_file_parses_as_the_lobby_needs() {
    let files = shipped_edits();
    let (_, lobby) = files
        .iter()
        .find(|(n, _)| n == "pregame_lobby")
        .expect("pregame_lobby.yaml is baked in");
    assert_eq!(lobby.screen, r"ui\screens\game_shell\pregame_lobby\pregame_lobby");
    assert_eq!(lobby.map, "mainmenu");
    assert_eq!(lobby.pane, 0);
    assert_eq!(
        lobby.edits,
        vec![
            Edit::Duplicate {
                block: b("text.10"),
                rect: [95, -260, 75, 70],
                copy: b("text.48"),
            },
            Edit::SetRect {
                block: b("text.0"),
                rect: [135, -258, 95, 45],
            },
            Edit::SetString {
                block: b("text.48"),
                name: "lobby_mode_change".into(),
            },
        ]
    );
}

#[test]
fn block_names_read_and_write_as_the_editor_names_them() {
    for s in ["title", "text.0", "text.48", "button.1", "bitmap.3"] {
        assert_eq!(b(s).to_string(), s);
    }
    assert_eq!(b("text.10"), BlockName::Block(BlockKind::Text, 10));
    for bad in ["", "text", "text.", "text.x", "text.-1", "list.0", "table.0", "Title", "text.1.2"] {
        assert!(bad.parse::<BlockName>().is_err(), "{bad:?}");
    }
    assert_eq!(file_name(r"ui\screens\a\pregame_lobby", 0), "pregame_lobby.yaml");
    assert_eq!(file_name(r"ui\screens\a\pregame_lobby", 1), "pregame_lobby.pane1.yaml");
}

#[test]
fn a_bad_file_is_refused_with_its_name_and_the_edit() {
    let head = "screen: 'ui\\screens\\x'\npane: 0\nedits:\n";
    let cases: &[(&str, &str)] = &[
        ("  - op: grow\n    block: text.0\n", "grow"),
        ("  - op: hide\n    block: text.0\n    rect: [1, 0, 0, 1]\n", "rect"),
        ("  - op: hide\n    block: text.x\n", "block name"),
        ("  - op: hide\n    block: button.0\n", "focus order"),
        ("  - op: hide\n    block: title\n", "no_header"),
        ("  - op: set_rect\n    block: title\n    rect: [1, 0, 0, 1]\n", "header layout"),
        ("  - op: set_rect\n    block: text.0\n    rect: [0, 0, 1, 1]\n", "not [top"),
        (
            "  - op: duplicate\n    block: text.0\n    rect: [1, 0, 0, 1]\n    as: button.4\n",
            "not a text block",
        ),
        (
            "  - op: duplicate\n    block: text.0\n    rect: [1, 0, 0, 1]\n    as: text.4\n  - op: duplicate\n    block: text.1\n    rect: [1, 0, 0, 1]\n    as: text.6\n",
            "next text.5",
        ),
        ("  - op: set_string\n    block: bitmap.0\n    name: a\n", "no string ID"),
        ("  - op: set_string\n    block: text.0\n    name: Lobby Mode\n", "string ID's name"),
    ];
    for (edits, want) in cases {
        let e = parse("broken", &format!("{head}{edits}")).unwrap_err();
        assert!(e.starts_with("broken.yaml: "), "{e}");
        assert!(e.contains(want), "{edits}: {e}");
    }
    let long = "a".repeat(MAX_NAME + 1);
    let e = parse(
        "long",
        &format!("{head}  - op: set_string\n    block: text.0\n    name: {long}\n"),
    )
    .unwrap_err();
    assert!(e.contains("string ID's name"), "{e}");
    // The file's own fields.
    for (text, want) in [
        ("screen: ''\nedits:\n  - op: hide\n    block: text.0\n", "screen"),
        ("screen: 'ui/screens/x'\nedits:\n  - op: hide\n    block: text.0\n", "backslashes"),
        ("screen: 'x'\nmap: 02_pier\nedits:\n  - op: hide\n    block: text.0\n", "02_pier"),
        ("screen: 'x'\nedits: []\n", "none"),
        ("screen: 'x'\ncolour: red\nedits:\n  - op: hide\n    block: text.0\n", "colour"),
    ] {
        let e = parse("f", text).unwrap_err();
        assert!(e.contains(want), "{text}: {e}");
    }
    // Every file's error, not the first.
    let two = Edits {
        files: vec![("a".into(), "nonsense: [".into()), ("b".into(), "screen: x\n".into())],
    };
    let e = two.parse().unwrap_err();
    assert!(e.contains("a.yaml") && e.contains("b.yaml"), "{e}");
}

#[test]
fn a_file_written_as_yaml_reads_back() {
    let f = file(
        r"ui\screens\it's\lobby",
        2,
        vec![
            Edit::Duplicate {
                block: b("bitmap.1"),
                rect: [12, -3, 12, -3],
                copy: b("bitmap.2"),
            },
            Edit::SetRect {
                block: b("button.0"),
                rect: [1, -2, 0, 3],
            },
            Edit::Hide { block: b("text.2") },
            Edit::SetString {
                block: b("title"),
                name: "lobby_title".into(),
            },
        ],
    );
    let text = to_yaml(&f, "two lines\nof comment");
    assert!(text.starts_with("# two lines\n# of comment\n"), "{text}");
    assert!(text.contains("rect: [1, -2, 0, 3]"), "{text}");
    assert_eq!(parse("round", &text).unwrap(), f);
}

#[test]
fn each_edit_lands_in_the_screen_tag() {
    let original = menu();
    let before = pane_bytes(&original, LOBBY, 0);
    let edited = apply_one(
        &original,
        file(
            LOBBY,
            0,
            vec![
                Edit::SetRect {
                    block: b("text.1"),
                    rect: [90, -250, 70, 60],
                },
                Edit::SetRect {
                    block: b("button.1"),
                    rect: [160, -40, 140, 70],
                },
                Edit::SetRect {
                    block: b("bitmap.0"),
                    rect: [150, -270, 100, -200],
                },
                Edit::Hide { block: b("text.2") },
                Edit::Hide { block: b("bitmap.1") },
                Edit::SetString {
                    block: b("title"),
                    name: "lobby_title".into(),
                },
                Edit::SetString {
                    block: b("text.0"),
                    name: "lobby_mode_change".into(),
                },
                Edit::SetString {
                    block: b("button.0"),
                    name: "invented_text_2".into(),
                },
            ],
        ),
    );
    assert_eq!(edited.failed, vec![]);
    assert_eq!(edited.applied.len(), 1);
    assert_eq!(edited.applied[0].lines.len(), 8);
    let map = &edited.map;
    let after = pane_bytes(map, LOBBY, 0);
    use BlockKind::{Bitmap, Button, Text};
    assert_eq!(place_of(Text, element(&after, Text, 1)), [90, -250, 70, 60]);
    assert_eq!(place_of(Button, element(&after, Button, 1)), [160, -40, 140, 70]);
    // A bitmap takes the rect's top-left; its size is its image's.
    assert_eq!(place_of(Bitmap, element(&after, Bitmap, 0)), [150, -270, 150, -270]);
    // Hidden: a text keeps its left and right, a bitmap goes to the corner.
    assert_eq!(place_of(Text, element(&after, Text, 2)), [PARKED, -50, PARKED, 50]);
    assert_eq!(place_of(Bitmap, element(&after, Bitmap, 1)), [PARKED; 4]);
    // Names: new ones added to the map's table, an existing one reused.
    assert_eq!(string_name(map, after.header).as_deref(), Some("lobby_title"));
    let s0 = u32_at(element(&after, Text, 0), 0x24);
    assert_eq!(string_name(map, s0).as_deref(), Some("lobby_mode_change"));
    let k0 = u32_at(element(&after, Button, 0), 0x30);
    assert_eq!(k0, u32_at(element(&before, Text, 2), 0x24), "invented_text_2 is the map's own");
    // Nothing else of the pane changed, and the other pane and screen not at all.
    assert_eq!(after.texts.len(), before.texts.len());
    assert_eq!(element(&after, Text, 0)[..0x1c], element(&before, Text, 0)[..0x1c]);
    assert_eq!(pane_bytes(map, LOBBY, 1).texts, pane_bytes(&original, LOBBY, 1).texts);
    assert_eq!(pane_bytes(map, OTHER, 0), pane_bytes(&original, OTHER, 0));
    // Our decoders read the same.
    let s = decoded(map, LOBBY);
    assert_eq!(s.panes[0].text_blocks[1].rect.left, -250);
    assert_eq!(s.panes[0].bitmaps[0].top_left.x, -270);
    assert_eq!(s.panes[0].buttons[1].rect.top, 160);
}

#[test]
fn a_duplicate_grows_the_block_inside_the_tag_and_passes_the_checks() {
    let original = menu();
    let edited = apply_one(
        &original,
        file(
            LOBBY,
            0,
            vec![
                Edit::Duplicate {
                    block: b("text.1"),
                    rect: [95, -260, 75, 70],
                    copy: b("text.3"),
                },
                Edit::SetString {
                    block: b("text.3"),
                    name: "lobby_mode_change".into(),
                },
                Edit::Duplicate {
                    block: b("button.0"),
                    rect: [120, -10, 100, 10],
                    copy: b("button.2"),
                },
                Edit::Duplicate {
                    block: b("bitmap.1"),
                    rect: [5, 6, 5, 6],
                    copy: b("bitmap.2"),
                },
                Edit::Duplicate {
                    block: b("text.3"),
                    rect: [30, -20, 10, 20],
                    copy: b("text.4"),
                },
            ],
        ),
    );
    assert_eq!(edited.failed, vec![]);
    let map = &edited.map;
    let (before, after) = (pane_bytes(&original, LOBBY, 0), pane_bytes(map, LOBBY, 0));
    use BlockKind::{Bitmap, Button, Text};
    assert_eq!(after.texts.len(), before.texts.len() + 2 * Text.size());
    assert_eq!(after.buttons.len(), before.buttons.len() + Button.size());
    assert_eq!(after.bitmaps.len(), before.bitmaps.len() + Bitmap.size());
    // The blocks before the copies keep their index and bytes.
    assert_eq!(after.texts[..before.texts.len()], before.texts[..]);
    // A copy is its original's bytes but for its place and string ID.
    let copy = element(&after, Text, 3);
    assert_eq!(place_of(Text, copy), [95, -260, 75, 70]);
    assert_eq!(copy[..0x1c], element(&before, Text, 1)[..0x1c]);
    assert_eq!(string_name(map, u32_at(copy, 0x24)).as_deref(), Some("lobby_mode_change"));
    // A copy of a copy takes the copy's string.
    assert_eq!(u32_at(element(&after, Text, 4), 0x24), u32_at(copy, 0x24));
    assert_eq!(place_of(Bitmap, element(&after, Bitmap, 2)), [5, 6, 5, 6]);

    // The arrays are inside the tag's own payload: the schema walks the
    // tag and finds every array within its bytes, as it does a retail one.
    let mut m = Map::from_reader(Cursor::new(map.clone())).unwrap();
    m.validate_checksum().unwrap();
    let r = tags::schema::Relocator::new(tags::schema::Schema::builtin(), &mut m).unwrap();
    let id = tag_of(&m, LOBBY).id;
    let walk = r.check(id).expect("the edited tag tiles its own bytes");
    assert!(walk.foreign.is_empty(), "{:?}", walk.foreign);
    // Our decoders and the `h2 ui --all` cross-checks accept it.
    let s = decoded(map, LOBBY);
    assert_eq!(s.panes[0].text_blocks.len(), TEXTS.len() + 2);
    assert_eq!(s.panes[0].buttons.len(), BUTTONS.len() + 1);
    assert_eq!(s.panes[0].bitmaps.len(), BITMAPS.len() + 1);
    assert_eq!(s.panes[0].text_blocks[3].rect.top, 95);
    let c = check_map(&mut m);
    assert_eq!(c.failures, Vec::<String>::new());
    assert_eq!(c.groups[&ui::SCREEN].failures, 0);
    let mut plain = Map::from_reader(Cursor::new(original)).unwrap();
    assert_eq!(check_map(&mut plain).failures, Vec::<String>::new());
}

#[test]
fn the_edited_map_round_trips() {
    let original = menu();
    let f = file(
        LOBBY,
        0,
        vec![
            Edit::Duplicate {
                block: b("text.1"),
                rect: [95, -260, 75, 70],
                copy: b("text.3"),
            },
            Edit::SetString {
                block: b("text.3"),
                name: "lobby_mode_change".into(),
            },
        ],
    );
    let edited = apply_one(&original, f.clone());
    assert_eq!(apply_one(&original, f.clone()).map, edited.map, "the same input, the same map");
    let mut m = Map::from_reader(Cursor::new(edited.map.clone())).unwrap();
    m.validate_checksum().unwrap();
    // Every other tag is as it was.
    let mut before = Map::from_reader(Cursor::new(original)).unwrap();
    assert_eq!(m.tags().len(), before.tags().len());
    for t in before.tags().to_vec() {
        if t.name.as_deref() == Some(LOBBY) {
            continue;
        }
        assert_eq!(
            m.read_tag_bytes(t.id).unwrap(),
            before.read_tag_bytes(t.id).unwrap(),
            "{:?} changed",
            t.name
        );
    }
    // Taken apart again, it builds back to the same bytes.
    let again = TagSet::from_map(&mut m).unwrap().build().unwrap().0;
    assert_eq!(again, edited.map);
    // Applied again, the copy's `as` no longer fits: the file is refused
    // whole and the map is served as it is.
    let twice = apply_one(&edited.map, f);
    assert_eq!(twice.applied, vec![]);
    assert_eq!(twice.failed.len(), 1);
    assert!(twice.failed[0].1.contains("text.4"), "{:?}", twice.failed);
    assert_eq!(twice.map, edited.map);
}

#[test]
fn a_file_that_does_not_fit_the_map_is_named_and_leaves_its_screen() {
    let original = menu();
    let good = file(
        OTHER,
        0,
        vec![Edit::SetRect {
            block: b("text.0"),
            rect: [1, -1, 0, 1],
        }],
    );
    let cases: Vec<(EditFile, &str)> = vec![
        (file(r"ui\screens\nowhere", 0, vec![Edit::Hide { block: b("text.0") }]), "no screen tag"),
        (file(LOBBY, 2, vec![Edit::Hide { block: b("text.0") }]), "no pane 2"),
        (file(LOBBY, 0, vec![Edit::Hide { block: b("text.9") }]), "no text.9"),
        (file(LOBBY, 1, vec![Edit::Hide { block: b("bitmap.0") }]), "no bitmap.0"),
        (
            file(
                LOBBY,
                0,
                vec![
                    Edit::SetRect {
                        block: b("text.0"),
                        rect: [1, -1, 0, 1],
                    },
                    Edit::Duplicate {
                        block: b("text.0"),
                        rect: [1, -1, 0, 1],
                        copy: b("text.7"),
                    },
                ],
            ),
            "text.3",
        ),
    ];
    for (bad, want) in cases {
        let edited = apply_all(
            &original,
            &[("bad".into(), bad.clone()), ("good".into(), good.clone())],
        )
        .unwrap();
        assert_eq!(edited.failed.len(), 1, "{bad:?}");
        assert_eq!(edited.failed[0].0, "bad");
        assert!(edited.failed[0].1.contains(want), "{want}: {:?}", edited.failed);
        assert_eq!(edited.applied.len(), 1);
        assert_eq!(edited.applied[0].file, "good");
        // Applied whole or not at all: the lobby is as it was.
        assert_eq!(pane_bytes(&edited.map, LOBBY, 0), pane_bytes(&original, LOBBY, 0));
        assert_eq!(pane_bytes(&edited.map, LOBBY, 1), pane_bytes(&original, LOBBY, 1));
        assert_ne!(pane_bytes(&edited.map, OTHER, 0), pane_bytes(&original, OTHER, 0));
    }
    // Nothing applied: the input's bytes.
    let none = apply_one(&original, file(r"ui\screens\nowhere", 0, vec![Edit::Hide { block: b("text.0") }]));
    assert_eq!(none.map, original);
    assert!(apply_all(b"not a map", &[]).is_err());
}

#[test]
fn a_pane_diff_gives_back_the_edits_that_made_it() {
    let original = menu();
    let edits = vec![
        Edit::Duplicate {
            block: b("text.1"),
            rect: [95, -260, 75, 70],
            copy: b("text.3"),
        },
        Edit::Duplicate {
            block: b("bitmap.0"),
            rect: [7, 8, 7, 8],
            copy: b("bitmap.2"),
        },
        Edit::SetRect {
            block: b("text.0"),
            rect: [135, -258, 95, 45],
        },
        Edit::Hide { block: b("text.2") },
        Edit::SetRect {
            block: b("button.1"),
            rect: [160, -40, 140, 70],
        },
        Edit::Hide { block: b("bitmap.1") },
        Edit::SetString {
            block: b("title"),
            name: "lobby_title".into(),
        },
        Edit::SetString {
            block: b("text.3"),
            name: "lobby_mode_change".into(),
        },
        Edit::SetString {
            block: b("button.0"),
            name: "invented_text_2".into(),
        },
    ];
    let edited = apply_one(&original, file(LOBBY, 0, edits.clone())).map;
    let (source, now) = (pane_bytes(&original, LOBBY, 0), pane_bytes(&edited, LOBBY, 0));
    let name = |id: u32| string_name(&edited, id);
    let d = diff_pane(LOBBY, "mainmenu", 0, &source, &now, &name);
    assert_eq!(d.skipped, Vec::<String>::new());
    let back = d.file.expect("edits");
    assert_eq!((back.screen.as_str(), back.map.as_str(), back.pane), (LOBBY, "mainmenu", 0));
    // The same edits, in the export's order: copies, then places, then
    // strings, each by kind (text, button, bitmap) and index.
    assert_eq!(back.edits, edits);
    back.check().unwrap();
    // And applied, they make the same pane.
    let again = apply_one(&original, back).map;
    assert_eq!(pane_bytes(&again, LOBBY, 0), now);

    // Nothing changed: no file.
    assert_eq!(diff_pane(LOBBY, "mainmenu", 0, &source, &source, &name), Diff::default());
}

#[test]
fn what_a_file_cannot_say_is_listed_not_dropped() {
    let source = pane_bytes(&menu(), LOBBY, 0);
    let mut now = source.clone();
    // Text 0's font changed and its rect moved; button 1 removed; a text
    // that is a copy of nothing added; the header set to an unnamed ID.
    now.texts[0xa] = 7;
    now.texts[0x1c] = 50;
    now.buttons.truncate(BlockKind::Button.size());
    let mut stranger = vec![0xeeu8; BlockKind::Text.size()];
    stranger[0x24..0x28].copy_from_slice(&0u32.to_le_bytes());
    now.texts.extend_from_slice(&stranger);
    now.header = 0x0500_7777;
    let d = diff_pane(LOBBY, "mainmenu", 0, &source, &now, &|_| None);
    let all = d.skipped.join("\n");
    assert!(all.contains("title") && all.contains("no name"), "{all}");
    assert!(all.contains("text.0: fields other"), "{all}");
    assert!(all.contains("button: 1 of 2 blocks removed"), "{all}");
    assert!(all.contains("text.3: added"), "{all}");
    // What it can say, it says.
    let f = d.file.expect("the move");
    assert_eq!(f.edits.len(), 1);
    assert!(matches!(f.edits[0], Edit::SetRect { block, .. } if block == b("text.0")));
}

fn shipped_edits() -> Vec<(String, EditFile)> {
    let name = "pregame_lobby";
    let text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/ui/retail-edits/pregame_lobby.yaml"
    ));
    vec![(name.into(), parse(name, text).unwrap())]
}

/// The shipped files on the user's own `mainmenu.map`: the lobby gains
/// block 48 where the lobby code looks for it. Skipped without a disc.
#[test]
fn the_shipped_edits_apply_to_the_disc_mainmenu() {
    use std::fs::File;
    use std::io::BufReader;
    let iso = std::env::var_os("H2_DISC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
        });
    if !iso.exists() {
        eprintln!("no disc at {}; skipped", iso.display());
        return;
    }
    let mut x = xiso::Xiso::open(BufReader::new(File::open(&iso).unwrap())).unwrap();
    let entry = x
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.eq_ignore_ascii_case("/maps/mainmenu.map"))
        .expect("mainmenu.map on the disc");
    let original = x.read(&entry).unwrap();
    const PREGAME: &str = r"ui\screens\game_shell\pregame_lobby\pregame_lobby";
    let before = decoded(&original, PREGAME);
    assert_eq!(before.panes[0].text_blocks.len(), 48, "the lobby's pane 0 has blocks 0 to 47");

    let files = shipped_edits();
    let edited = apply_all(&original, &files).unwrap();
    assert_eq!(edited.failed, vec![]);
    assert!(edited.applied.iter().any(|a| a.file == "pregame_lobby"));
    let after = pane_bytes(&edited.map, PREGAME, 0);
    use BlockKind::Text;
    assert_eq!(after.texts.len(), 49 * Text.size());
    let block48 = element(&after, Text, 48);
    assert_eq!(place_of(Text, block48), [95, -260, 75, 70]);
    assert_eq!(place_of(Text, element(&after, Text, 0)), [135, -258, 95, 45]);
    assert_eq!(
        string_name(&edited.map, u32_at(block48, 0x24)).as_deref(),
        Some("lobby_mode_change")
    );
    // Block 48 is block 10 but for its place and string ID.
    assert_eq!(block48[..0x1c], element(&after, Text, 10)[..0x1c]);
    let s = decoded(&edited.map, PREGAME);
    assert_eq!(s.panes[0].text_blocks.len(), 49);
    assert_eq!(s.panes[1..], before.panes[1..], "the other panes as they were");
    let mut m = Map::from_reader(Cursor::new(edited.map.clone())).unwrap();
    m.validate_checksum().unwrap();
    assert_eq!(check_map(&mut m).failures, Vec::<String>::new());
}
