//! The document tabs without a window (docs/editor/studio-redesign.md,
//! "The shell"), on the scenario crate's invented cache with invented
//! weapon and item layouts (no game data).
use std::path::{Path, PathBuf};

use map::{FourCC, TagId};
use scenario_edit::fixture;
use serde_json::{Value, json};
use tags::registry::Registry;

use super::kinds;
use super::shell::tool_for;
use super::tag_api::set_field;
use super::tag_tests::{self, weap_id};
use super::*;

/// [`tag_tests::registry`]'s weapon, and an item collection with one
/// named field, so two tags of the map take edits.
pub(crate) fn registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let schema = tags::schema::Schema::parse(
            "group scnr 0x10 align 4\ngroup weap 0x10 align 4\n  0x8 block 0xc align 4\n    0x0 tagref\ngroup itmc 0x10 align 4\n",
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
        defs.parse_into("test-items", "group itmc 0x10\n  0x0 pad 0x8\n  0x8 u32 count\n  0xc pad 0x4\n")
            .unwrap();
        Registry::build(&schema, &defs)
    })
}

pub(crate) struct Temp(pub PathBuf);
impl Temp {
    pub(crate) fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("h2-editor-shell-{tag}-{}", std::process::id()));
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

/// An app with the invented map open in the Tags tool, over [`registry`].
pub(crate) fn app(t: &Path) -> App {
    tag_tests::app_with_registry(t, tag_tests::map(), registry()).0
}

pub(crate) fn kit_id() -> TagId {
    fixture::id(3)
}

fn scnr_id() -> TagId {
    fixture::id(1)
}

pub(crate) fn set(app: &mut App, tag: TagId, path: &str, v: Value) {
    let a = set_field(&app.tags, tag, path, &v).unwrap();
    app.dispatch(Action::Tag(a));
    app.dispatch(Action::Tag(TagAction::Seal));
}

fn speed(app: &App) -> f32 {
    let b = app.tags.read(weap_id(), tag_tests::WEAP, 4).unwrap();
    f32::from_le_bytes(b.try_into().unwrap())
}

fn count(app: &App) -> u32 {
    let at = app.tags.tag(kit_id()).unwrap().address + 8;
    let b = app.tags.read(kit_id(), at, 4).unwrap();
    u32::from_le_bytes(b.try_into().unwrap())
}

fn docs(app: &App) -> Vec<Doc> {
    app.tabs().iter().map(|t| t.doc).collect()
}

/// Two tag tabs over one map: each undoes and redoes its own edits, in
/// any order, and a new edit of one keeps the other's redo.
#[test]
fn two_tag_tabs_have_two_undo_stacks() {
    let t = Temp::new("two-stacks");
    let mut app = app(&t.0);
    let (weap, kit) = (Doc::Tag(weap_id()), Doc::Tag(kit_id()));
    let first = count(&app);
    app.dispatch(Action::OpenDoc(weap));
    app.dispatch(Action::OpenDoc(kit));
    assert_eq!(docs(&app), [Doc::Map, weap, kit]);
    assert_eq!(app.front_doc(), Some(kit));
    assert_eq!(
        (app.tool, app.tags.selected),
        (ToolKind::Tags, Some(kit_id()))
    );

    set(&mut app, weap_id(), "speed", json!(4.0));
    set(&mut app, kit_id(), "count", json!(first + 5));
    assert_eq!(app.doc_can(weap), (true, false));
    assert_eq!(app.doc_can(kit), (true, false));

    // The weapon's step is under the kit's newer one: its tab undoes it.
    app.dispatch(Action::FocusDoc(weap));
    assert_eq!(app.tags.selected, Some(weap_id()));
    app.dispatch(Action::Undo);
    assert_eq!(speed(&app), 2.5);
    assert_eq!(count(&app), first + 5, "the kit's edit stays");
    assert_eq!(app.doc_can(weap), (false, true));
    assert_eq!(app.doc_can(kit), (true, false));
    // An undo with nothing in the tab's own stack leaves the other's.
    app.dispatch(Action::UndoDoc(weap));
    assert_eq!(count(&app), first + 5);

    // A new kit edit keeps the weapon's redo.
    set(&mut app, kit_id(), "count", json!(first + 6));
    assert_eq!(app.doc_can(weap), (false, true));
    app.dispatch(Action::Redo);
    assert_eq!(speed(&app), 4.0);

    // The kit's stack, from its tab while the weapon's is in front.
    app.dispatch(Action::UndoDoc(kit));
    assert_eq!(count(&app), first + 5);
    app.dispatch(Action::UndoDoc(kit));
    assert_eq!(count(&app), first);
    assert_eq!(speed(&app), 4.0);
    assert_eq!(app.doc_can(kit), (false, true));
    // A new weapon edit drops the weapon's redo only.
    app.dispatch(Action::UndoDoc(weap));
    set(&mut app, weap_id(), "speed", json!(3.0));
    assert_eq!(app.doc_can(weap), (true, false));
    assert_eq!(app.doc_can(kit), (false, true));
    app.dispatch(Action::RedoDoc(kit));
    assert_eq!(count(&app), first + 5);
}

/// The scenario tag is the map's own document: opening it brings the
/// map's tab forward in the Scenario tool.
#[test]
fn the_scenario_tag_opens_as_the_maps_tab() {
    let t = Temp::new("scenario");
    let mut app = app(&t.0);
    app.dispatch(Action::OpenDoc(Doc::Tag(scnr_id())));
    assert_eq!(docs(&app), [Doc::Map]);
    assert_eq!(
        (app.front_doc(), app.tool),
        (Some(Doc::Map), ToolKind::Scenario)
    );
}

/// A tag tab closes and its edits stay the map's (reopened, its stack is
/// there); the map's tab asks about unsaved edits first, and closing it
/// closes its tags' tabs.
#[test]
fn a_tag_tab_closes_keeping_its_edits_and_the_map_asks_first() {
    let t = Temp::new("close");
    let mut app = app(&t.0);
    let (weap, kit) = (Doc::Tag(weap_id()), Doc::Tag(kit_id()));
    app.dispatch(Action::OpenDoc(weap));
    app.dispatch(Action::OpenDoc(kit));
    set(&mut app, kit_id(), "count", json!(9));
    assert!(app.doc_dirty(kit) && !app.doc_dirty(weap));
    app.dispatch(Action::CloseDoc(kit));
    assert_eq!(docs(&app), [Doc::Map, weap]);
    assert_eq!(app.front_doc(), Some(weap), "the tab to its left");
    assert_eq!(app.tags.selected, Some(weap_id()));
    assert_eq!(count(&app), 9);
    assert!(app.is_dirty());
    app.dispatch(Action::OpenDoc(kit));
    assert_eq!(app.doc_can(kit), (true, false));

    app.dispatch(Action::CloseDoc(Doc::Map));
    assert!(
        app.pending().is_some(),
        "held behind the unsaved-changes prompt"
    );
    assert_eq!(docs(&app), [Doc::Map, weap, kit]);
    app.resolve(Answer::Discard);
    assert!(!app.session.is_open());
    assert!(docs(&app).is_empty());
    assert_eq!(app.front_doc(), None);
    assert!(
        !app.log().iter().any(|l| l.status.is_error()),
        "{:?}",
        app.log()
            .iter()
            .map(|l| l.status.text())
            .collect::<Vec<_>>()
    );
}

/// A tool's document gets a tab when its tool opens it, the old tool tabs
/// bring that tab forward, and closing it shows the tab to its left.
#[test]
fn tool_documents_get_tabs_and_the_tool_tabs_follow_them() {
    let t = Temp::new("tools");
    let mut app = app(&t.0);
    app.dispatch(Action::NewHopper);
    assert!(app.hoppers.is_open());
    assert_eq!(docs(&app), [Doc::Map, Doc::Playlists]);
    assert_eq!(app.front_doc(), Some(Doc::Playlists));
    app.dispatch(Action::SetTool(ToolKind::Scenario));
    assert_eq!(app.front_doc(), Some(Doc::Map));
    app.dispatch(Action::SetTool(ToolKind::Playlists));
    assert_eq!(app.front_doc(), Some(Doc::Playlists));
    app.perform(Action::CloseDoc(Doc::Playlists), true).unwrap();
    assert!(!app.hoppers.is_open());
    assert_eq!(docs(&app), [Doc::Map]);
    assert_eq!(
        (app.front_doc(), app.tool),
        (Some(Doc::Map), ToolKind::Scenario)
    );
}

/// A tag's tab opens in the editor its group's row names: bitmaps in the
/// Textures tool, the scenario in the Scenario tool, the rest in the Tags
/// tool's panel until their own documents dock.
#[test]
fn a_tags_editor_comes_from_its_group() {
    let tool = |g: &[u8; 4]| tool_for(kinds::kind(FourCC(*g)).editor);
    assert_eq!(tool(b"bitm"), ToolKind::Textures);
    assert_eq!(tool(b"scnr"), ToolKind::Scenario);
    for g in [b"weap", b"snd!", b"nhdt", b"wgit", b"mode", b"matg"] {
        assert_eq!(tool(g), ToolKind::Tags, "{}", String::from_utf8_lossy(g));
    }
}
