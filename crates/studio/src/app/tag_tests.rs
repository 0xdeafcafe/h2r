//! The Tags tool without a window, on the scenario crate's invented cache
//! with its weapon tag given invented fields (no game data), and on the
//! owner's disc (opt-in).
use super::tag_api::{self, read_tag, set_field};
use super::tag_doc::{self, Access};
use super::*;
use map::{FourCC, TagId};
use scenario_edit::fixture::{self, META, META_FILE, put, putf};
use serde_json::json;
use tags::registry::Registry;

/// An invented weapon layout: a float, flags, an enum and a block of
/// (item reference, value) elements; and a scenario layout, so the
/// scenario tag has one.
pub(crate) fn registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let schema = tags::schema::Schema::parse(
            "group scnr 0x10 align 4\ngroup weap 0x10 align 4\n  0x8 block 0xc align 4\n    0x0 tagref\n",
        )
        .unwrap();
        let mut defs = tags::defs::Defs::default();
        defs.parse_into(
            "test",
            "\
group weap 0x10
  0x0 f32 speed
  0x4 flags16 flags bits=loud|quiet
  0x6 enum16 mode values=off|on|auto
  0x8 block parts 0xc align 4
    0x0 tagref item groups=itmc
    0x8 u32 value
",
        )
        .unwrap();
        Registry::build(&schema, &defs)
    })
}

pub(crate) const WEAP: u32 = META + 0x1100;
pub(crate) const PARTS: u32 = WEAP + 0x10;
pub(crate) fn weap_id() -> TagId {
    fixture::id(2)
}

/// The fixture with its weapon tag 0x40 bytes long: speed 2.5, flag
/// `loud`, mode `auto`, and two parts (the kit with value 7; none with 9).
pub(crate) fn map() -> Vec<u8> {
    let mut b = fixture::fixture();
    // The weapon's index entry (slot 2): its size.
    let tags = fixture::INDEX + 32 + 4 * 12;
    put(&mut b, tags + 2 * 16 + 12, 0x40);
    let w = META_FILE + 0x1100;
    putf(&mut b, w, 2.5);
    b[w + 4..w + 6].copy_from_slice(&1u16.to_le_bytes());
    b[w + 6..w + 8].copy_from_slice(&2u16.to_le_bytes());
    put(&mut b, w + 8, 2);
    put(&mut b, w + 0xc, PARTS);
    let p = w + 0x10;
    put(&mut b, p, FourCC(*b"itmc").as_u32());
    put(&mut b, p + 4, fixture::id(3).0);
    put(&mut b, p + 8, 7);
    put(&mut b, p + 0xc, u32::MAX);
    put(&mut b, p + 0x10, u32::MAX);
    put(&mut b, p + 0x14, 9);
    b
}

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2-editor-tags-{tag}-{}", std::process::id()));
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

/// An app with the invented map open in the Tags tool.
pub(crate) fn app(t: &Path) -> (App, PathBuf, Vec<u8>) {
    let src = map();
    let path = t.join("example.map");
    std::fs::write(&path, &src).unwrap();
    let mut app = App::new();
    app.set_data_root(t.join("data"));
    app.session.build_scenes = false;
    app.tags = TagDoc::new(registry());
    app.dispatch(Action::OpenSource(path.clone()));
    app.dispatch(Action::SetTool(ToolKind::Tags));
    (app, path, src)
}

fn tag(app: &mut App, a: TagAction) {
    app.dispatch(Action::Tag(a));
}

fn set(app: &mut App, path: &str, v: serde_json::Value) {
    let a = set_field(&app.tags, weap_id(), path, &v).unwrap();
    tag(app, a);
}

fn fields(app: &App) -> serde_json::Value {
    read_tag(&app.tags, weap_id()).unwrap()["fields"].clone()
}

/// Byte ranges where two equal-length buffers differ.
fn diff(a: &[u8], b: &[u8]) -> Vec<(usize, usize)> {
    assert_eq!(a.len(), b.len());
    let mut out = Vec::new();
    let mut k = 0;
    while k < a.len() {
        if a[k] == b[k] {
            k += 1;
            continue;
        }
        let s = k;
        while k < a.len() && a[k] != b[k] {
            k += 1;
        }
        out.push((s, k));
    }
    out
}

#[test]
fn the_tags_tool_lists_every_tag_with_its_fields_named() {
    let t = Temp::new("list");
    let (app, ..) = app(&t.0);
    let doc = &app.tags;
    assert_eq!(doc.tags().len(), 4);
    let access = |slot: u32| doc.tag(fixture::id(slot)).unwrap().access.clone();
    assert_eq!(access(0), Access::Read(tag_doc::NO_LAYOUT));
    assert_eq!(access(1), Access::Read(tag_doc::SCENARIO_OWNED));
    assert_eq!(access(2), Access::Edit);
    let f = fields(&app);
    assert_eq!(f["speed"], json!(2.5));
    assert_eq!(f["flags"], json!(["loud"]));
    assert_eq!(f["mode"], json!("auto"));
    assert_eq!(f["parts"].as_array().unwrap().len(), 2);
    assert_eq!(f["parts"][0]["item"], json!("itmc:objects\\test\\kit"));
    assert_eq!(f["parts"][0]["value"], json!(7));
    assert_eq!(f["parts"][1]["item"], json!(null));
    assert_eq!(app.title(), "example - h2-editor");
}

#[test]
fn a_field_edit_saves_byte_exactly_and_undoes() {
    let t = Temp::new("save");
    let (mut app, _, src) = app(&t.0);
    set(&mut app, "speed", json!(4.0));
    set(&mut app, "parts[1].item", json!("itmc:objects\\test\\kit"));
    assert_eq!(fields(&app)["speed"], json!(4.0));
    assert!(app.session.is_dirty());
    assert!(app.tags.is_changed(weap_id()));
    assert_eq!(app.title(), "example * - h2-editor");
    let out = t.0.join("out.map");
    app.dispatch(Action::SaveMapAs(out.clone()));
    assert!(!app.session.is_dirty(), "{:?}", app.log().last());
    let saved = std::fs::read(&out).unwrap();
    let w = (META_FILE + 0x1100) as usize;
    let item = w + 0x10 + 0xc;
    for (lo, hi) in diff(&src, &saved) {
        let ok = (lo >= w && hi <= w + 4)
            || (lo >= item && hi <= item + 8)
            || (lo >= 0x2d0 && hi <= 0x2d4);
        assert!(ok, "{lo:#x}..{hi:#x} changed");
    }
    assert_eq!(saved[w..w + 4], 4.0f32.to_le_bytes());
    // The saved map reads back with the edits.
    let mut again = TagDoc::new(registry());
    again
        .load(&mut map::Map::from_reader(std::io::Cursor::new(saved)).unwrap())
        .unwrap();
    assert_eq!(
        read_tag(&again, weap_id()).unwrap()["fields"]["parts"][1]["item"],
        json!("itmc:objects\\test\\kit")
    );
    // Undone, a save is the source again.
    app.dispatch(Action::Undo);
    app.dispatch(Action::Undo);
    assert!(app.tags.patches().is_empty());
    app.dispatch(Action::SaveMapAs(out.clone()));
    assert_eq!(std::fs::read(&out).unwrap(), src);
    app.dispatch(Action::Redo);
    assert_eq!(fields(&app)["speed"], json!(4.0));
}

#[test]
fn a_drag_is_one_undo_step() {
    let t = Temp::new("drag");
    let (mut app, ..) = app(&t.0);
    for x in [3.0f32, 3.5, 4.0] {
        tag(
            &mut app,
            TagAction::Write(TagWrite {
                tag: weap_id(),
                address: WEAP,
                bytes: x.to_le_bytes().to_vec(),
                label: "speed".into(),
                merge: Some(1),
            }),
        );
    }
    tag(&mut app, TagAction::Seal);
    set(&mut app, "mode", json!("on"));
    app.dispatch(Action::Undo);
    assert_eq!(fields(&app)["speed"], json!(4.0));
    assert_eq!(fields(&app)["mode"], json!("auto"));
    app.dispatch(Action::Undo);
    assert_eq!(fields(&app)["speed"], json!(2.5));
    assert!(!app.tags.can_undo());
}

#[test]
fn blocks_change_within_the_elements_they_had() {
    let t = Temp::new("block");
    let (mut app, ..) = app(&t.0);
    let id = weap_id();
    let block = |app: &mut App, op: BlockOp| {
        let a = tag_api::block(&app.tags, id, "parts", op);
        a.map(|a| tag(app, a))
    };
    // Full: no room to add or duplicate.
    assert_eq!(
        block(&mut app, BlockOp::Add),
        Err(tag_doc::GROWTH_REFUSAL.into())
    );
    assert_eq!(
        block(&mut app, BlockOp::Duplicate(0)),
        Err(tag_doc::GROWTH_REFUSAL.into())
    );
    block(&mut app, BlockOp::Remove(0)).unwrap();
    let f = fields(&app);
    assert_eq!(f["parts"].as_array().unwrap().len(), 1);
    assert_eq!(f["parts"][0]["value"], json!(9));
    block(&mut app, BlockOp::Duplicate(0)).unwrap();
    assert_eq!(fields(&app)["parts"][1]["value"], json!(9));
    block(&mut app, BlockOp::Remove(1)).unwrap();
    block(&mut app, BlockOp::Add).unwrap();
    let f = fields(&app);
    assert_eq!(f["parts"][1]["item"], json!(null));
    assert_eq!(f["parts"][1]["value"], json!(0));
    assert_eq!(app.tags.capacity(WEAP + 8), 2);
    for _ in 0..4 {
        app.dispatch(Action::Undo);
    }
    assert!(app.tags.patches().is_empty(), "{:x?}", app.tags.patches());
    assert!(!app.session.is_dirty());
}

#[test]
fn what_cannot_be_written_is_refused_with_why() {
    let t = Temp::new("refuse");
    let (mut app, ..) = app(&t.0);
    // The scenario tag: the Scenario tool's.
    let scnr = fixture::id(1);
    tag(
        &mut app,
        TagAction::Write(TagWrite {
            tag: scnr,
            address: META + 0x100,
            bytes: vec![1],
            label: "x".into(),
            merge: None,
        }),
    );
    assert!(app.tags.patches().is_empty());
    let last = app.log().last().unwrap().status.clone();
    assert_eq!(
        last,
        editor_core::Status::Error(tag_doc::SCENARIO_OWNED.into())
    );
    // Structural fields, values out of range, strings the map lacks.
    assert!(set_field(&app.tags, weap_id(), "parts", &json!(1)).is_err());
    assert!(set_field(&app.tags, weap_id(), "parts[0].value", &json!(-1)).is_err());
    assert!(set_field(&app.tags, weap_id(), "parts[2].value", &json!(1)).is_err());
    assert!(set_field(&app.tags, weap_id(), "mode", &json!("loud")).is_err());
    // An item reference takes items only.
    assert!(
        set_field(
            &app.tags,
            weap_id(),
            "parts[0].item",
            &json!("weap:objects\\test\\blaster")
        )
        .is_err()
    );
    // Opening another map drops the edits with the old map's.
    set(&mut app, "speed", json!(1.0));
    app.session.close();
    app.sync_tags();
    assert!(!app.tags.is_open());
}

/// On the owner's disc: an unnamed word of a retail tag set through the
/// registry saves with only its bytes and the checksum changed.
#[test]
#[ignore = "needs disc/halo2.iso (or $H2_DISC)"]
fn disc_a_retail_tag_field_saves_byte_exactly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let disc =
        std::env::var_os("H2_DISC").map_or_else(|| root.join("disc/halo2.iso"), PathBuf::from);
    let t = Temp::new("disc");
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.session.build_scenes = false;
    app.dispatch(Action::OpenSource(disc));
    app.dispatch(Action::SelectMap("lockout".into()));
    app.dispatch(Action::SetTool(ToolKind::Tags));
    let doc = &app.tags;
    assert!(doc.is_open(), "{:?}", doc.error);
    // The first editable weapon, its first named float.
    let weap = doc
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"weap") && t.access == Access::Edit)
        .expect("an editable weapon")
        .clone();
    let layout = doc.layout(weap.id).unwrap().clone();
    let f = layout
        .fields
        .iter()
        .find(|f| f.named && matches!(f.kind, tags::registry::Kind::Float { count: 1, .. }))
        .expect("a named float")
        .clone();
    let json = read_tag(doc, weap.id).unwrap();
    assert!(json["fields"].as_object().unwrap().len() > 50);
    let old = json["fields"][&f.name].as_f64().unwrap();
    let a = set_field(doc, weap.id, &f.name, &json!(old + 1.0)).unwrap();
    app.dispatch(Action::Tag(a));
    let out = t.0.join("lockout.map");
    app.dispatch(Action::SaveMapAs(out.clone()));
    let src = app.session.source().unwrap().read_base().unwrap();
    let saved = std::fs::read(&out).unwrap();
    let space = map::Map::from_reader(std::io::Cursor::new(&src[..]))
        .unwrap()
        .metadata_space()
        .unwrap();
    let at = space.file_offset as usize + (weap.address + f.offset - space.virtual_base) as usize;
    let d = diff(&src, &saved);
    for (lo, hi) in &d {
        assert!(
            (*lo >= at && *hi <= at + 4) || (*lo >= 0x2d0 && *hi <= 0x2d4),
            "{lo:#x}..{hi:#x} changed ({} at {at:#x})",
            f.name
        );
    }
    assert!(!d.is_empty());
    eprintln!("{}: {} {} -> {}", weap.name, f.name, old, old + 1.0);
}
