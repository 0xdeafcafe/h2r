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
    weapon(fixture::fixture())
}

/// [`map`] with the metadata ending the file, so a block can grow.
pub(crate) fn map_at_end() -> Vec<u8> {
    weapon(fixture::fixture_at_end())
}

fn weapon(mut b: Vec<u8>) -> Vec<u8> {
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
    app_on(t, map())
}

fn app_on(t: &Path, src: Vec<u8>) -> (App, PathBuf, Vec<u8>) {
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
    let w = META_FILE + 0x1100;
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

/// A full block grows only while growth is allowed; it saves as a copy
/// appended to the metadata with its pair pointed there, nothing else in
/// the source changing.
#[test]
fn a_full_block_grows_when_allowed_and_saves_appended() {
    let t = Temp::new("grow");
    let (mut app, _, src) = app_on(&t.0, map_at_end());
    let id = weap_id();
    let block = |app: &mut App, op: BlockOp| {
        let a = tag_api::block(&app.tags, id, "parts", op);
        a.map(|a| tag(app, a))
    };
    assert_eq!(
        block(&mut app, BlockOp::Add),
        Err(tag_doc::GROWTH_REFUSAL.into())
    );
    app.dispatch(Action::SetAllowGrowth(true));
    block(&mut app, BlockOp::Duplicate(1)).unwrap();
    // Room for twice the elements: the next add fits where they went.
    assert_eq!(app.tags.capacity(WEAP + 8), 4);
    block(&mut app, BlockOp::Add).unwrap();
    set(&mut app, "parts[3].value", json!(5));
    let f = fields(&app);
    let values: Vec<_> = f["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["value"].clone())
        .collect();
    assert_eq!(values, [json!(7), json!(9), json!(9), json!(5)]);
    // The grown elements are no patch; the pair is written at save.
    let ext = META + 0x2000;
    assert!(app.tags.patches().iter().all(|(a, _)| *a < ext));
    assert!(
        app.tags
            .patches()
            .iter()
            .all(|(a, b)| *a + b.len() as u32 <= WEAP + 8 || *a >= WEAP + 0x10),
        "{:x?}",
        app.tags.patches()
    );
    let grown = app.tags.grown();
    assert_eq!((grown.len(), grown[0].count), (1, 4));
    assert!(app.session.is_dirty());

    let out = t.0.join("grown.map");
    app.dispatch(Action::SaveMapAs(out.clone()));
    let saved = std::fs::read(&out).unwrap();
    assert_eq!(saved.len(), src.len() + 0x1000);
    let pair = META_FILE + 0x1100 + 8;
    for (lo, hi) in diff(&src, &saved[..src.len()]) {
        let ok = [(8, 12), (0x18, 0x20), (0x2d0, 0x2d4), (pair, pair + 8)]
            .iter()
            .any(|&(a, b)| lo >= a && hi <= b);
        assert!(ok, "{lo:#x}..{hi:#x}");
    }
    let word = |o: usize| u32::from_le_bytes(saved[o..o + 4].try_into().unwrap());
    assert_eq!((word(pair), word(pair + 4)), (4, ext));
    let parts = META_FILE + 0x1110;
    let n = src.len();
    assert_eq!(saved[n..n + 0x18], src[parts..parts + 0x18]);
    assert_eq!(saved[n + 0x18..n + 0x24], src[parts + 0xc..parts + 0x18]);
    assert_eq!(word(n + 0x24), u32::MAX);
    assert_eq!(word(n + 0x2c), 5);
    map::Map::from_reader(std::io::Cursor::new(saved))
        .unwrap()
        .validate_checksum()
        .unwrap();

    // Undone, nothing is left of it.
    for _ in 0..3 {
        app.dispatch(Action::Undo);
    }
    assert!(app.tags.grown().is_empty());
    assert!(app.tags.patches().is_empty(), "{:x?}", app.tags.patches());
    assert_eq!(fields(&app)["parts"].as_array().unwrap().len(), 2);
}

/// A string ID the map lacks is added after its own, saved with the map,
/// and taken back by undo.
#[test]
fn a_string_the_map_lacks_is_added_and_saved() {
    let t = Temp::new("string");
    let (mut app, _, src) = app(&t.0);
    let n = map::Map::from_reader(std::io::Cursor::new(src.clone()))
        .unwrap()
        .strings()
        .len();
    let set_string = |app: &mut App, address: u32, text: &str| {
        tag(
            app,
            TagAction::SetString {
                tag: weap_id(),
                address,
                text: text.into(),
                label: "name".into(),
            },
        )
    };
    let id = (9 << 24) | n as u32;
    set_string(&mut app, WEAP + 0x2c, "brand_new");
    assert_eq!(app.tags.read(WEAP + 0x2c, 4).unwrap(), id.to_le_bytes());
    assert_eq!(app.tags.string(id), Some("brand_new"));
    assert_eq!(app.tags.string_id("brand_new"), Some(id));
    // The same text again is the same string.
    set_string(&mut app, WEAP + 0x30, "brand_new");
    assert_eq!(app.tags.new_strings(), ["brand_new"]);
    assert_eq!(app.tags.read(WEAP + 0x30, 4).unwrap(), id.to_le_bytes());
    // Too long for its slot.
    set_string(&mut app, WEAP + 0x34, &"x".repeat(0x80));
    assert_eq!(app.tags.new_strings().len(), 1);

    let out = t.0.join("strings.map");
    app.dispatch(Action::SaveMapAs(out.clone()));
    let saved = std::fs::read(&out).unwrap();
    let mut m = map::Map::from_reader(std::io::Cursor::new(saved.clone())).unwrap();
    m.validate_checksum().unwrap();
    assert_eq!(m.strings().len(), n + 1);
    assert_eq!(m.strings()[n].as_deref(), Some("brand_new"));
    let at = META_FILE + 0x1100 + 0x2c;
    assert_eq!(saved[at..at + 4], id.to_le_bytes());

    app.dispatch(Action::Undo);
    assert_eq!(app.tags.new_strings().len(), 1, "still used by the first");
    app.dispatch(Action::Undo);
    assert!(app.tags.new_strings().is_empty());
    assert!(app.tags.patches().is_empty());
    assert_eq!(app.tags.string(id), None);
    app.dispatch(Action::Redo);
    assert_eq!(app.tags.new_strings(), ["brand_new"]);
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
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

// Tag bundles, browsing and resources (app::tag_bundle, app::tag_browse).

use super::tag_bundle::{self as bundles, ExportOptions, Fit, ImportOptions};

/// Export the weapon of `app`'s open map as a bundle.
fn export_weapon(app: &App, opts: &ExportOptions) -> (editor_core::tag_bundle::Bundle, bundles::ExportReport) {
    bundles::export(
        &app.tags,
        &[weap_id()],
        editor_core::tag_bundle::Origin {
            map: "example".into(),
            build: "02.09.27.09809".into(),
        },
        opts,
        None,
    )
    .unwrap()
}

#[test]
fn a_tag_round_trips_through_a_bundle_file_as_one_undo_step() {
    let t = Temp::new("bundle");
    let (mut app, ..) = app(&t.0);
    let before = fields(&app);
    let out = t.0.join("bundles").join("blaster.h2tag");
    app.dispatch(Action::TagExport {
        tags: vec![weap_id()],
        out: Some(out.clone()),
        options: ExportOptions::default(),
    });
    let (path, report) = app.last_tag_export.clone().expect("exported");
    assert_eq!(path, out);
    assert_eq!(report.tags, ["weap:objects\\test\\blaster"]);
    // Self-describing: the format, the origin, the reference by name.
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("h2recomp.tag-bundle"), "{text}");
    assert!(text.contains("\"map\": \"example\""), "{text}");
    assert!(text.contains("itmc:objects\\\\test\\\\kit"), "{text}");
    let b = bundles::read(&out).unwrap();
    assert_eq!(b.tags[0].fields["speed"], json!(2.5));

    // Edit it three ways: a field, a reference, a block's count.
    set(&mut app, "speed", json!(9.0));
    set(&mut app, "parts[0].item", json!(null));
    let a = tag_api::block(&app.tags, weap_id(), "parts", BlockOp::Remove(1)).unwrap();
    tag(&mut app, a);
    assert_ne!(fields(&app), before);
    let steps = app.tags.undo_len();

    app.dispatch(Action::TagImport {
        path: out.clone(),
        options: ImportOptions::default(),
    });
    let r = app.last_tag_import.clone().expect("imported");
    assert_eq!(r.replaced, ["weap:objects\\test\\blaster"]);
    assert_eq!(fields(&app), before, "the bundle's tag is back");
    assert_eq!(app.tags.undo_len(), steps + 1, "one undo step");
    app.dispatch(Action::Undo);
    let f = fields(&app);
    assert_eq!(f["speed"], json!(9.0));
    assert_eq!(f["parts"].as_array().unwrap().len(), 1);
    app.dispatch(Action::Redo);
    assert_eq!(fields(&app), before);
}

#[test]
fn an_import_resolves_references_by_name_and_refuses_what_the_map_lacks() {
    let t = Temp::new("bundle-refs");
    let (mut app, ..) = app(&t.0);
    let (b, report) = export_weapon(
        &app,
        &ExportOptions {
            dependencies: true,
            resources: false,
        },
    );
    // The kit it references has no layout here: left out, and said so.
    assert_eq!(report.tags, ["weap:objects\\test\\blaster"]);
    assert!(
        report.left_out.iter().any(|l| l.starts_with("itmc:objects\\test\\kit")),
        "{:?}",
        report.left_out
    );
    // From another map (no local pointers or IDs carried): the reference
    // is found by its name.
    set(&mut app, "parts[0].item", json!(null));
    let other = Fit::default();
    bundles::import(&mut app.tags, &b, other, &ImportOptions::default()).unwrap();
    assert_eq!(
        fields(&app)["parts"][0]["item"],
        json!("itmc:objects\\test\\kit")
    );

    // A reference the map lacks refuses the whole import, leaving the
    // document as it was.
    set(&mut app, "speed", json!(3.0));
    let steps = app.tags.undo_len();
    let mut lacking = b.clone();
    for f in &mut lacking.tags[0].root.fixups {
        if let editor_core::tag_bundle::Fixup::Block { elements, .. } = f {
            elements[0].fixups = vec![editor_core::tag_bundle::Fixup::Tag {
                at: 0,
                target: Some("itmc:objects\\test\\nothing".into()),
            }];
        }
    }
    let e = bundles::import(&mut app.tags, &lacking, other, &ImportOptions::default())
        .unwrap_err();
    assert!(e.contains("this map has no itmc:objects\\test\\nothing"), "{e}");
    assert_eq!(app.tags.undo_len(), steps);
    assert_eq!(fields(&app)["speed"], json!(3.0));

    // A tag the map lacks: refused, or skipped when asked.
    let mut renamed = b.clone();
    renamed.tags[0].name = "objects\\test\\elsewhere".into();
    let e = bundles::import(&mut app.tags, &renamed, other, &ImportOptions::default())
        .unwrap_err();
    assert!(e.contains("does not add them"), "{e}");
    renamed.tags.push(b.tags[0].clone());
    let skip = ImportOptions {
        skip_missing: true,
        ..ImportOptions::default()
    };
    let r = bundles::import(&mut app.tags, &renamed, other, &skip).unwrap();
    assert_eq!(r.skipped, ["weap:objects\\test\\elsewhere"]);
    assert_eq!(r.replaced, ["weap:objects\\test\\blaster"]);
    assert_eq!(fields(&app)["speed"], json!(2.5));

    // Into another tag of the group, named.
    let into = ImportOptions {
        target: Some(weap_id()),
        ..ImportOptions::default()
    };
    let mut one = b.clone();
    one.tags[0].name = "objects\\test\\other".into();
    bundles::import(&mut app.tags, &one, other, &into).unwrap();

    // Read with another layout: refused.
    let mut stale = b.clone();
    stale.tags[0].layout = "0000000000000000".into();
    let e = bundles::import(&mut app.tags, &stale, other, &ImportOptions::default())
        .unwrap_err();
    assert!(e.contains("another layout"), "{e}");
}

#[test]
fn a_bundle_with_more_elements_grows_the_block_only_when_allowed() {
    let t = Temp::new("bundle-grow");
    let (mut app, ..) = app_on(&t.0, map_at_end());
    app.dispatch(Action::SetAllowGrowth(true));
    let a = tag_api::block(&app.tags, weap_id(), "parts", BlockOp::Duplicate(0)).unwrap();
    tag(&mut app, a);
    set(&mut app, "parts[1].value", json!(8));
    let (b, _) = export_weapon(&app, &ExportOptions::default());
    // Back to the source's two elements, and growth off.
    while app.tags.can_undo() {
        app.dispatch(Action::Undo);
    }
    app.dispatch(Action::SetAllowGrowth(false));
    let fit = Fit {
        same_map: true,
        same_build: true,
    };
    let e = bundles::import(&mut app.tags, &b, fit, &ImportOptions::default()).unwrap_err();
    assert!(e.contains("Allow growing"), "{e}");
    assert!(app.tags.patches().is_empty());
    app.dispatch(Action::SetAllowGrowth(true));
    bundles::import(&mut app.tags, &b, fit, &ImportOptions::default()).unwrap();
    let values: Vec<_> = fields(&app)["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["value"].clone())
        .collect();
    assert_eq!(values, [json!(7), json!(8), json!(9)]);
}

#[test]
fn exports_stay_out_of_git_checkouts() {
    let t = Temp::new("bundle-git");
    let (mut app, ..) = app(&t.0);
    let repo = t.0.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    app.dispatch(Action::TagExport {
        tags: vec![weap_id()],
        out: Some(repo.join("blaster.h2tag")),
        options: ExportOptions::default(),
    });
    assert!(app.last_tag_export.is_none());
    assert!(!repo.join("blaster.h2tag").exists());
    // With no path: under the data root.
    app.dispatch(Action::TagExport {
        tags: vec![weap_id()],
        out: None,
        options: ExportOptions::default(),
    });
    let (path, _) = app.last_tag_export.clone().expect("exported");
    assert!(path.starts_with(t.0.join("data").join("tag-bundles")), "{}", path.display());
    assert_eq!(
        path.file_name().unwrap(),
        "weap-objects.test.blaster.h2tag"
    );
}

#[test]
fn a_shared_cache_is_browsed_read_only_beside_the_open_map() {
    let t = Temp::new("browse");
    let (mut app, ..) = app(&t.0);
    // A sibling of the loose map stands in for the disc's shared.map.
    std::fs::write(t.0.join("shared.map"), map()).unwrap();
    set(&mut app, "speed", json!(5.0));
    app.dispatch(Action::BrowseTags(Some("shared".into())));
    assert_eq!(app.browsing(), Some("shared"));
    let doc = app.shown_tags();
    assert_eq!(doc.tags().len(), 4);
    assert_eq!(
        doc.tag(weap_id()).unwrap().access,
        Access::Read(super::tag_browse::BROWSING)
    );
    assert_eq!(read_tag(doc, weap_id()).unwrap()["fields"]["speed"], json!(2.5));
    assert_eq!(app.title(), "shared - h2-editor");
    // Selecting works; writing is refused, and the open map's edit stays.
    tag(&mut app, TagAction::Select(weap_id()));
    assert_eq!(app.shown_tags().selected, Some(weap_id()));
    let a = set_field(app.shown_tags(), weap_id(), "speed", &json!(1.0)).unwrap();
    tag(&mut app, a);
    assert!(matches!(app.log().last().unwrap().status, editor_core::Status::Error(_)));
    // Exports read the browsed map.
    let out = t.0.join("shared-blaster.h2tag");
    app.dispatch(Action::TagExport {
        tags: vec![weap_id()],
        out: Some(out.clone()),
        options: ExportOptions::default(),
    });
    assert_eq!(bundles::read(&out).unwrap().origin.map, "shared");
    // Imports go to the open map only.
    app.dispatch(Action::TagImport {
        path: out.clone(),
        options: ImportOptions::default(),
    });
    assert!(app.last_tag_import.is_none());
    // Back to the open map, its edit intact.
    app.dispatch(Action::BrowseTags(None));
    assert_eq!(app.browsing(), None);
    assert_eq!(fields(&app)["speed"], json!(5.0));
    // Picking the shared cache in the Maps list browses it in the Tags
    // tool, with no unsaved-changes prompt.
    app.dispatch(Action::SetTool(ToolKind::Scenario));
    app.dispatch(Action::SelectMap("shared".into()));
    assert!(app.pending().is_none());
    assert_eq!(app.browsing(), Some("shared"));
    assert_eq!(app.tool, ToolKind::Tags);
    assert_eq!(app.loaded_map(), Some("example"));
    assert_eq!(fields(&app)["speed"], json!(5.0), "the open map keeps its edits");
}

#[test]
fn resources_and_lookups_read_the_shown_document() {
    let t = Temp::new("resources");
    let (app, ..) = app(&t.0);
    let r = tag_api::resources(&app.tags, weap_id()).unwrap();
    assert_eq!(r["data"], json!([]));
    assert_eq!(r["raw"], json!([]));
    assert!(tag_api::resources(&app.tags, fixture::id(0)).is_err(), "no layout");
    let doc = &app.tags;
    let id = |v: serde_json::Value| tag_api::lookup(doc, &v);
    assert_eq!(id(json!("weap")), Ok(weap_id()));
    assert_eq!(id(json!("weap:objects\\test\\blaster")), Ok(weap_id()));
    assert_eq!(id(json!("objects\\test\\blaster")), Ok(weap_id()));
    assert_eq!(id(json!("objects\\test\\blaster.weap")), Ok(weap_id()));
    assert_eq!(id(json!(format!("{:#010x}", weap_id().0))), Ok(weap_id()));
    assert_eq!(id(json!(weap_id().0)), Ok(weap_id()));
    assert!(id(json!("nothing")).is_err());
    // The tree marks nothing away: the fixture has no raw data.
    assert_eq!(app.tags.homes(weap_id()).map(|h| h.away()), Some(0));
}

/// The invented weapon with a model reference, an hlmt and its render model.
pub(crate) fn placement_map() -> Vec<u8> {
    let mut b = map();
    let groups = fixture::INDEX + 32;
    let old_tags = groups + 4 * 12;
    let tags = groups + 6 * 12;
    b.copy_within(old_tags..old_tags + 4 * 16, tags);
    put(&mut b, fixture::INDEX + 4, 6);
    put(&mut b, fixture::INDEX + 8, 0x8000_0020 + 6 * 12);
    put(&mut b, fixture::INDEX + 24, 6);
    let names_size = u32::from_le_bytes(b[0x2c8..0x2cc].try_into().unwrap());
    let mut name_offset = names_size;
    for (slot, name) in [(4, &b"test_model\0"[..]), (5, &b"test_mesh\0"[..])] {
        put(&mut b, 0x900 + slot * 4, name_offset);
        let start = 0x980 + name_offset as usize;
        b[start..start + name.len()].copy_from_slice(name);
        name_offset += name.len() as u32;
    }
    put(&mut b, 0x2c0, 6);
    put(&mut b, 0x2c8, name_offset);
    for (slot, group, at, size) in [
        (4, FourCC(*b"hlmt"), 0x1300, 0x10),
        (5, FourCC(*b"mode"), 0x1400, 0x10),
    ] {
        let g = groups + slot * 12;
        put(&mut b, g, group.as_u32());
        put(&mut b, g + 4, u32::MAX);
        put(&mut b, g + 8, u32::MAX);
        let t = tags + slot * 16;
        put(&mut b, t, group.as_u32());
        put(&mut b, t + 4, fixture::id(slot as u32).0);
        put(&mut b, t + 8, META + at);
        put(&mut b, t + 12, size);
    }
    // Keep the fixture's weapon fields and put the model after the parts.
    put(&mut b, META_FILE + 0x1130, FourCC(*b"hlmt").as_u32());
    put(&mut b, META_FILE + 0x1134, fixture::id(4).0);
    put(&mut b, META_FILE + 0x1300, FourCC(*b"mode").as_u32());
    put(&mut b, META_FILE + 0x1304, fixture::id(5).0);
    // An ordinary reference back to the weapon exercises cycle handling.
    put(&mut b, META_FILE + 0x1400, FourCC(*b"weap").as_u32());
    put(&mut b, META_FILE + 0x1404, weap_id().0);
    b
}

pub(crate) fn placement_app(path: &Path) -> App {
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    let registry = REGISTRY.get_or_init(|| {
        let schema = tags::schema::Schema::parse(
            "\
group weap 0x38 align 4
  0x8 block 0xc align 4
    0x0 tagref
  0x30 tagref
group hlmt 0x8 align 4
  0x0 tagref
group mode 0x8 align 4
  0x0 tagref
",
        ).unwrap();
        let mut defs = tags::defs::Defs::default();
        defs.parse_into("placement fixture", "\
group weap 0x38
  0x0 f32 speed
  0x8 block parts 0xc align 4
    0x0 tagref item groups=itmc
    0x8 u32 value
  0x30 tagref model groups=hlmt
group hlmt 0x8
  0x0 tagref render_model groups=mode
group mode 0x8
  0x0 tagref example_back_reference
").unwrap();
        Registry::build(&schema, &defs)
    });
    let (mut app, _, _) = app_on(path, placement_map());
    app.tags = TagDoc::new(registry);
    app.dispatch(Action::SetTool(ToolKind::Scenario));
    app.sync_tags();
    app
}
