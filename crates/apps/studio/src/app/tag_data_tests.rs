//! `TagDoc::resize_data` on the scenario crate's invented cache: its
//! weapon tag is given one data reference holding six invented bytes.
use super::tag_doc::{self, TagAction};
use super::tag_tests::{WEAP, app_with_registry, map_at_end, weap_id};
use super::*;
use scenario_edit::fixture::{META, META_FILE, put};
use std::path::PathBuf;
use tags::registry::{Kind, Registry};

/// The weapon's root layout: one data reference at 0x30.
fn registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let schema = tags::schema::Schema::parse(
            "group scnr 0x10 align 4\ngroup weap 0x38 align 4\n  0x30 block 0x1 align 1\n",
        )
        .unwrap();
        let mut defs = tags::defs::Defs::default();
        defs.parse_into("test", "group weap 0x38\n  0x0 unknown 0x30\n  0x30 data blob align 1\n")
            .unwrap();
        Registry::build(&schema, &defs)
    })
}

const PAIR: u32 = WEAP + 0x30;
/// Where the invented bytes are in the source's metadata.
const DATA: u32 = META + 0x1800;
const PAYLOAD: [u8; 6] = [1, 2, 3, 4, 5, 6];
/// The first address past the metadata, where grown data goes.
const EXT: u32 = META + 0x2000;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2-editor-data-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn source() -> Vec<u8> {
    let mut b = map_at_end();
    let pair = META_FILE + 0x1100 + 0x30;
    put(&mut b, pair, PAYLOAD.len() as u32);
    put(&mut b, pair + 4, DATA);
    let at = META_FILE + 0x1800;
    b[at..at + PAYLOAD.len()].copy_from_slice(&PAYLOAD);
    b
}

fn resize(app: &mut App, bytes: &[u8]) {
    let layout = app.tags.layout(weap_id()).unwrap().clone();
    app.dispatch(Action::Tag(TagAction::ResizeData {
        tag: weap_id(),
        element: WEAP,
        layout,
        field: 0x30,
        bytes: bytes.to_vec(),
        label: "blob".into(),
    }));
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn open(t: &Temp) -> (App, Vec<u8>) {
    let (app, _, src) = app_with_registry(&t.0, source(), registry());
    (app, src)
}

/// A shorter payload is written in place and only the size changes
/// besides; the tail stays as it was, dead. Undo restores the pair.
#[test]
fn a_shorter_payload_is_written_in_place() {
    let t = Temp::new("shorter");
    let (mut app, src) = open(&t);
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((6, DATA)));
    resize(&mut app, &[9, 8]);
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((2, DATA)));
    assert_eq!(app.tags.read(weap_id(), DATA, 2), Some(vec![9, 8]));
    assert_eq!(
        app.tags.read(weap_id(), DATA + 2, 4),
        Some(vec![3, 4, 5, 6])
    );
    assert!(app.tags.grown().is_empty());
    assert!(app.tags.patches().iter().all(|(a, _)| *a < EXT));
    assert!(app.session.is_dirty());

    let out = t.0.join("shorter.map");
    app.dispatch(Action::SaveMapAs(out.clone()));
    let saved = std::fs::read(&out).unwrap();
    assert_eq!(saved.len(), src.len());
    let pair = META_FILE + 0x1100 + 0x30;
    assert_eq!(word(&saved, pair), 2);
    assert_eq!(word(&saved, pair + 4), DATA);
    let at = META_FILE + 0x1800;
    assert_eq!(saved[at..at + 6], [9, 8, 3, 4, 5, 6]);

    app.dispatch(Action::Undo);
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((6, DATA)));
    assert_eq!(app.tags.read(weap_id(), DATA, 6), Some(PAYLOAD.to_vec()));
    assert!(app.tags.patches().is_empty(), "{:x?}", app.tags.patches());
    // Back up to the source length is still in place, and no growth.
    resize(&mut app, &[7; 6]);
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((6, DATA)));
    assert!(app.tags.grown().is_empty());
}

/// A longer payload is refused while growth is off, by name, and changes
/// nothing.
#[test]
fn a_longer_payload_is_refused_while_growth_is_off() {
    let t = Temp::new("off");
    let (mut app, _) = open(&t);
    assert!(!app.tags.allow_growth);
    assert_eq!(
        app.tags.data_refusal(weap_id(), PAIR, 7),
        Some(tag_doc::DATA_GROWTH_REFUSAL)
    );
    let layout = app.tags.layout(weap_id()).unwrap().clone();
    assert_eq!(
        app.tags
            .resize_data(weap_id(), WEAP, &layout, 0x30, &[0; 7], "blob"),
        Err(tag_doc::DATA_GROWTH_REFUSAL.to_owned())
    );
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((6, DATA)));
    assert!(!app.tags.has_edits());
    assert!(app.tags.grown().is_empty());
    // Only a data reference may be resized.
    let kind = &layout.field_at(0x30).unwrap().kind;
    assert!(matches!(kind, Kind::Data));
    assert!(
        app.tags
            .resize_data(weap_id(), WEAP, &layout, 0, &[0; 2], "speed")
            .is_err()
    );
}

/// A longer payload is appended on save and its pair repointed; the old
/// bytes are no longer referenced; it reads back; undo takes it back.
#[test]
fn a_longer_payload_is_appended_on_save() {
    let t = Temp::new("longer");
    let (mut app, src) = open(&t);
    app.session.set_allow_growth(true);
    app.tags.allow_growth = true;
    let payload: Vec<u8> = (0..40).map(|k| 0xe0 ^ k as u8).collect();
    resize(&mut app, &payload);
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((40, EXT)));
    assert_eq!(app.tags.read(weap_id(), EXT, 40), Some(payload.clone()));
    let grown = app.tags.grown();
    assert_eq!((grown.len(), grown[0].root, grown[0].count), (1, PAIR, 40));
    assert_eq!(grown[0].bytes, payload);
    assert!(app.tags.patches().iter().all(|(a, _)| *a < EXT));
    assert!(
        app.tags
            .patches()
            .iter()
            .all(|(a, b)| *a + b.len() as u32 <= PAIR || *a >= PAIR + 8)
    );

    // Shrinking inside the new room stays there, and the save follows it.
    resize(&mut app, &payload[..10]);
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((10, EXT)));
    assert_eq!(app.tags.grown()[0].bytes, payload[..10]);
    resize(&mut app, &payload);

    let out = t.0.join("grown.map");
    app.dispatch(Action::SaveMapAs(out.clone()));
    let saved = std::fs::read(&out).unwrap();
    assert_eq!(saved.len(), src.len() + 0x1000);
    let pair = META_FILE + 0x1100 + 0x30;
    assert_eq!((word(&saved, pair), word(&saved, pair + 4)), (40, EXT));
    let n = src.len();
    assert_eq!(saved[n..n + 40], payload[..]);
    // The old bytes are where they were, and nothing points at them.
    let at = META_FILE + 0x1800;
    assert_eq!(saved[at..at + 6], PAYLOAD);
    assert_ne!(word(&saved, pair + 4), DATA);
    let mut reread = map::Map::from_reader(std::io::Cursor::new(saved)).unwrap();
    reread.validate_checksum().unwrap();
    let mut doc = TagDoc::new(registry());
    doc.load(&mut reread).unwrap();
    assert_eq!(doc.pair(weap_id(), PAIR), Some((40, EXT)));
    assert_eq!(doc.read(weap_id(), EXT, 40), Some(payload));

    // Undone, nothing is left of it.
    while app.tags.can_undo() {
        app.dispatch(Action::Undo);
    }
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((6, DATA)));
    assert!(app.tags.grown().is_empty());
    assert!(app.tags.patches().is_empty(), "{:x?}", app.tags.patches());
}

/// Redo after undo brings the grown pair back.
#[test]
fn redo_restores_the_grown_pair() {
    let t = Temp::new("redo");
    let (mut app, _) = open(&t);
    app.session.set_allow_growth(true);
    app.tags.allow_growth = true;
    resize(&mut app, &[5; 20]);
    app.dispatch(Action::Undo);
    assert!(app.tags.grown().is_empty());
    app.dispatch(Action::Redo);
    assert_eq!(app.tags.pair(weap_id(), PAIR), Some((20, EXT)));
    assert_eq!(app.tags.grown()[0].count, 20);
}
