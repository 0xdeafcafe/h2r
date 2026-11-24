//! The document tabs without a window (docs/editor/studio-redesign.md,
//! "The shell"), on the scenario crate's invented cache with invented
//! weapon and item layouts (no game data).
use std::path::{Path, PathBuf};

use map::{FourCC, TagId};
use scenario_edit::{BlockKind, fixture};
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

/// The browser's open (`Action::OpenAsset`): a tag of the open map opens
/// as a tab in its editor, and a tag the map lacks is an error in the
/// log, not a tab.
#[test]
fn opening_an_asset_opens_its_tag_of_the_open_map() {
    let t = Temp::new("open-asset");
    let mut app = app(&t.0);
    let (group, name) = {
        let tag = app.tags.tag(kit_id()).unwrap();
        (tags::defs::fourcc_text(tag.group), tag.name.clone())
    };
    app.dispatch(Action::OpenAsset {
        asset: format!("tag:{group}:{name}"),
        map: None,
    });
    assert_eq!(app.front_doc(), Some(Doc::Tag(kit_id())));
    let tabs = app.tabs().len();
    let before = app.log().len();
    app.dispatch(Action::OpenAsset {
        asset: "tag:weap:objects\\nothing".into(),
        map: None,
    });
    assert_eq!(app.tabs().len(), tabs);
    assert!(app.log().len() > before);
    assert!(app.status().is_some_and(|s| s.text().contains("not in the open map")));
}

/// The script editor is a document: opening a script gives it a tab in
/// front, showing the Variants panels with its tab picked; closing the tab
/// keeps the text and does not touch the variant.
#[test]
fn the_script_is_a_document_tab() {
    let t = Temp::new("rules-doc");
    let mut app = app(&t.0);
    assert!(!docs(&app).contains(&Doc::Rules));
    app.rules.open_starter("slayer").unwrap();
    app.sync_tabs();
    assert_eq!(app.front_doc(), Some(Doc::Rules));
    assert_eq!(app.surface(), Surface::Variants);
    assert_eq!(app.doc_id(Doc::Rules), "rules");
    assert_eq!(app.doc_title(Doc::Rules), app.rules.title);
    assert!(app.doc_dirty(Doc::Rules), "a starter has no file yet");

    let text = app.rules.text.clone();
    app.dispatch(Action::CloseDoc(Doc::Rules));
    assert!(!docs(&app).contains(&Doc::Rules));
    assert_eq!(app.rules.text, text);

    app.rules.open_starter("slayer").unwrap();
    app.dispatch(Action::FocusDoc(Doc::Rules));
    assert_eq!(app.front_doc(), Some(Doc::Rules));
    assert!(app.rules.show, "the script's tab is picked among the panels");
}

/// The window has no tool tabs: a surface is asked for by an action and
/// read back from the app.
#[test]
fn show_changes_the_surface() {
    let t = Temp::new("surface");
    let mut app = app(&t.0);
    for (surface, _) in Surface::EDITORS {
        app.dispatch(Action::Show(surface));
        assert_eq!(app.surface(), surface);
        assert_eq!(Surface::from(surface.tool()), surface);
    }
    assert!(Surface::Tags.is_map() && Surface::Scripts.is_map() && !Surface::Hud.is_map());
}

/// [`crate::api::sound::tests::mouth_map`]'s sound with one field nothing
/// reads while it is auditioned (the playback index, 0 in the fixture), so
/// its document takes a field edit beside the redraws of its mouth curve.
fn sound_registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let schema =
            tags::schema::Schema::parse("group scnr 0x10 align 4\ngroup snd! 0x14 align 4\n")
                .unwrap();
        let mut defs = tags::defs::Defs::default();
        defs.parse_into(
            "test",
            "\
group snd! 0x14
  0x0 pad 0x6
  0x6 u16 playback_index
  0x8 pad 0xc
",
        )
        .unwrap();
        Registry::build(&schema, &defs)
    })
}

const SND: &str = r"snd!:objects\test\blaster";

/// The invented sound (a 52-tick mouth curve on permutation 0) open as the
/// tab in front, in the Tags tool.
fn sound_app(t: &Path) -> App {
    let (mut app, ..) =
        tag_tests::app_with_registry(t, crate::api::sound::tests::mouth_map(), sound_registry());
    app.audition.headless = true;
    app.dispatch(Action::OpenAsset {
        asset: format!("tag:{SND}"),
        map: None,
    });
    app
}

/// The sound's playback index, the field the tests write.
fn playback(app: &App) -> u16 {
    let b = app.tags.read(weap_id(), tag_tests::WEAP + 6, 2).unwrap();
    u16::from_le_bytes(b.try_into().unwrap())
}

/// Whether the sound's permutation 0 has a redrawn mouth curve not saved.
fn redrawn(app: &App) -> bool {
    app.sound_mouth_is_edited(weap_id(), 0, 0)
}

/// A sound's document is one stack (docs/editor/studio-flow.md, "Shared
/// patterns"): a field edit and a mouth redraw undo in the order they were
/// made, with Cmd+Z and with the editor's own Undo, and redo in reverse.
#[test]
fn a_mouth_redraw_and_a_field_edit_undo_in_the_order_made() {
    let t = Temp::new("one-undo");
    let mut app = sound_app(&t.0);
    let (weap, snd) = (weap_id(), Doc::Tag(weap_id()));
    assert_eq!(app.front_doc(), Some(snd));
    let ticks = app.sound_mouth(weap, 0, 0).unwrap().unwrap().len();
    // All open: unlike every tick of the file's rising curve.
    let open = vec![255; ticks];
    assert_eq!(app.doc_can(snd), (false, false));

    // A field edit, then a redraw: the redraw, made last, goes first.
    set(&mut app, weap, "playback_index", json!(7));
    assert_eq!((playback(&app), redrawn(&app)), (7, false));
    app.sound_mouth_write(weap, 0, 0, &open).unwrap();
    assert_eq!((playback(&app), redrawn(&app)), (7, true));
    assert_eq!(app.doc_can(snd), (true, false));
    assert_eq!((app.can_undo(), app.can_redo()), (true, false));
    app.dispatch(Action::Undo);
    assert_eq!(
        (playback(&app), redrawn(&app)),
        (7, false),
        "Cmd+Z takes the redraw, the last step made"
    );
    assert_eq!(app.doc_can(snd), (true, true));
    app.dispatch(Action::Undo);
    assert_eq!(
        (playback(&app), redrawn(&app)),
        (0, false),
        "then the field edit"
    );
    assert_eq!(app.doc_can(snd), (false, true));
    app.dispatch(Action::Redo);
    assert_eq!((playback(&app), redrawn(&app)), (7, false));
    app.dispatch(Action::Redo);
    assert_eq!((playback(&app), redrawn(&app)), (7, true));
    assert_eq!(app.doc_can(snd), (true, false));

    // The sound editor's Undo runs the same action: a redraw first, then a
    // field edit, undo the other way round, and redo back in order.
    app.dispatch(Action::UndoDoc(snd));
    app.dispatch(Action::UndoDoc(snd));
    assert_eq!((playback(&app), redrawn(&app)), (0, false));
    app.sound_mouth_write(weap, 0, 0, &open).unwrap();
    set(&mut app, weap, "playback_index", json!(9));
    assert_eq!((playback(&app), redrawn(&app)), (9, true));
    app.dispatch(Action::UndoDoc(snd));
    assert_eq!(
        (playback(&app), redrawn(&app)),
        (0, true),
        "the field edit, made last, goes first"
    );
    app.dispatch(Action::UndoDoc(snd));
    assert_eq!((playback(&app), redrawn(&app)), (0, false));
    app.dispatch(Action::RedoDoc(snd));
    assert_eq!(
        (playback(&app), redrawn(&app)),
        (0, true),
        "the redraw, made first, is redone first"
    );
    app.dispatch(Action::RedoDoc(snd));
    assert_eq!((playback(&app), redrawn(&app)), (9, true));

    // A new step leaves nothing undone before it to redo.
    app.dispatch(Action::UndoDoc(snd));
    set(&mut app, weap, "playback_index", json!(11));
    assert_eq!(
        (playback(&app), redrawn(&app), app.doc_can(snd)),
        (11, true, (true, false))
    );
    // `sound.mouth`'s undo keeps its way: the last redraw of the map, under
    // a field edit made after it, which it leaves.
    assert_eq!(app.sound_mouth_undo(), Some((weap, 0, 0)));
    assert_eq!((playback(&app), redrawn(&app)), (11, false));
    assert_eq!(
        app.doc_can(snd),
        (true, false),
        "the field edit made after the redraw leaves its redo wrong"
    );
    app.dispatch(Action::UndoDoc(snd));
    assert_eq!(
        (playback(&app), redrawn(&app), app.doc_can(snd)),
        (0, false, (false, true))
    );
}

/// Two documents keep two stacks (docs/editor/studio-flow.md, "Shared
/// patterns"): a sound's redraw and the map's placement are each undone in
/// their own document, and each keeps its redo when the other steps.
#[test]
fn a_sound_and_the_map_keep_two_stacks() {
    let t = Temp::new("one-undo-two");
    let mut app = sound_app(&t.0);
    let (weap, snd) = (weap_id(), Doc::Tag(weap_id()));
    let ticks = app.sound_mouth(weap, 0, 0).unwrap().unwrap().len();
    let open = vec![255; ticks];
    app.sound_mouth_write(weap, 0, 0, &open).unwrap();
    app.dispatch(Action::MovePlacement {
        block: BlockKind::NetgameFlags,
        index: 0,
        position: [1.0, 2.0, 3.0],
        ypr: None,
    });
    assert_eq!(
        (app.doc_can(snd), app.doc_can(Doc::Map)),
        ((true, false), (true, false))
    );
    app.dispatch(Action::UndoDoc(snd));
    assert!(!redrawn(&app));
    assert!(app.session.can_undo(), "the map's step stays");
    assert_eq!(app.doc_can(snd), (false, true));
    app.dispatch(Action::UndoDoc(Doc::Map));
    assert!(!app.session.can_undo() && app.session.can_redo());
    assert_eq!(
        app.doc_can(snd),
        (false, true),
        "the sound's redo is its own"
    );
    app.dispatch(Action::RedoDoc(snd));
    assert!(redrawn(&app) && app.session.can_redo());
}

/// No window module draws an Undo or Redo button of its own: each runs
/// `Action::UndoDoc` or `Action::RedoDoc` on its document
/// (docs/editor/studio-flow.md, "Shared patterns").
#[test]
fn every_undo_button_runs_undo_doc() {
    /// The first string literal at or after `at`.
    fn quoted(source: &str, at: usize) -> Option<&str> {
        let open = source[at..].find('"')? + at;
        let close = source[open + 1..].find('"')? + open + 1;
        Some(&source[open + 1..close])
    }
    fn check(path: &Path, buttons: &mut usize) {
        if path.is_dir() {
            for entry in path.read_dir().unwrap() {
                check(&entry.unwrap().path(), buttons);
            }
            return;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }
        let source = std::fs::read_to_string(path).unwrap();
        for at in source.match_indices("button(").map(|(at, _)| at) {
            // The label is written on the line that draws the button.
            let end = source[at..].find('\n').map_or(source.len(), |n| at + n);
            let Some(label) = quoted(&source[..end], at) else {
                continue;
            };
            if !label.contains("Undo") && !label.contains("Redo") {
                continue;
            }
            *buttons += 1;
            // What the `if` that draws it does, to the line that closes it.
            let line = source[..at].rfind('\n').map_or(0, |n| n + 1);
            let rest = &source[line..];
            let end = rest
                .match_indices('\n')
                .map(|(n, _)| n + 1)
                .find(|&start| rest[start..].trim_start().starts_with('}'))
                .unwrap_or(rest.len());
            let body = &rest[..end];
            assert!(
                body.contains("Action::UndoDoc") || body.contains("Action::RedoDoc"),
                "{}: the {label:?} button does not run UndoDoc:\n{body}",
                path.display()
            );
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut buttons = 0;
    check(&src.join("ui.rs"), &mut buttons);
    check(&src.join("ui"), &mut buttons);
    assert!(buttons >= 2, "{buttons} undo buttons found");
}
