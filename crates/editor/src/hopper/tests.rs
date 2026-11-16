//! The tool's file handling and undo, without a window.

use super::*;
use hopper_edit::fixture::sample;
use live_proto::hopper::INFLATED_SIZE;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("h2-editor-hopper-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn status() -> Option<crate::tool::Status> {
    None
}

macro_rules! cx {
    ($status:ident) => {
        ToolContext {
            source: None,
            selected_map: None,
            render_state: None,
            status: &mut $status,
        }
    };
}

#[test]
fn reads_both_forms() {
    let f = sample();
    let (a, fa) = read_file(&f.build()).unwrap();
    let (b, fb) = read_file(f.image()).unwrap();
    assert_eq!((fa, fb), (Format::Envelope, Format::Image));
    assert_eq!(a.image(), f.image());
    assert_eq!(b.image(), f.image());
    assert!(matches!(read_file(b"nope"), Err(ToolError::Invalid(_))));
}

#[test]
fn a_raw_image_saves_back_byte_for_byte() {
    let f = sample();
    let path = scratch("raw.dat");
    std::fs::write(&path, f.image()).unwrap();
    let mut t = HopperTool::new();
    let mut st = status();
    t.open(&path, &mut cx!(st)).unwrap();
    assert!(!t.is_dirty());
    let out = scratch("raw-out.dat");
    t.save_as(&out, &mut cx!(st)).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), f.image());
    assert_eq!(t.path(), Some(out.as_path()));
}

#[test]
fn an_envelope_saves_the_same_image() {
    let f = sample();
    let path = scratch("env.dat");
    std::fs::write(&path, f.build()).unwrap();
    let mut t = HopperTool::new();
    let mut st = status();
    t.open(&path, &mut cx!(st)).unwrap();
    t.edit(|h| h[1].records[0].dwords[0] = 77);
    assert!(t.is_dirty());
    t.save(&mut cx!(st)).unwrap();
    assert!(!t.is_dirty());
    let (back, format) = read_file(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(format, Format::Envelope);
    assert_eq!(back.record(1, 0).unwrap().dwords[0], 77);
    let mut want = f.clone();
    let mut r = want.record(1, 0).unwrap();
    r.dwords[0] = 77;
    want.set_record(1, 0, &r).unwrap();
    assert_eq!(back.image(), want.image(), "only the weight changed");
}

#[test]
fn undo_redo_and_dirty_follow_the_edits_across_a_save() {
    let mut t = HopperTool::new();
    t.open_bytes(sample().image(), None).unwrap();
    assert!(!t.can_undo());
    t.edit(|h| h[0].title = "One".into());
    t.edit(|h| {
        model::duplicate_record(&mut h[0], 0);
    });
    assert!(t.is_dirty());
    assert!(t.undo());
    assert_eq!(t.hoppers().unwrap()[0].records.len(), 2);
    assert!(t.undo());
    assert_eq!(t.hoppers().unwrap()[0].title, "List 0");
    assert!(!t.is_dirty(), "back to the file as opened");
    assert!(t.redo());
    assert!(t.is_dirty());

    // Save without a path asks the shell for one.
    let mut st = status();
    assert!(matches!(t.save(&mut cx!(st)), Err(ToolError::NoPath)));
    let path = scratch("undo.dat");
    t.save_as(&path, &mut cx!(st)).unwrap();
    assert!(!t.is_dirty());
    assert!(matches!(st, Some(crate::tool::Status::Info(ref m)) if m.starts_with("saved")));
    // Undo past the save: dirty again, relative to the old image.
    assert!(t.undo());
    assert!(t.is_dirty());
    assert_eq!(t.hoppers().unwrap()[0].title, "One");
    assert!(t.undo());
    assert_eq!(t.hoppers().unwrap()[0].title, "List 0");
    assert!(t.is_dirty(), "the file on disk has the edit");
    assert!(t.redo() && t.redo());
    assert!(!t.is_dirty());
}

#[test]
fn invalid_edits_do_not_save() {
    let mut t = HopperTool::new();
    t.open_bytes(sample().image(), None).unwrap();
    t.edit(|h| h[2].title.clear());
    assert_eq!(t.errors(), ["Playlist 3: the playlist needs a title"]);
    let mut st = status();
    let path = scratch("invalid.dat");
    let _ = std::fs::remove_file(&path);
    assert!(matches!(
        t.save_as(&path, &mut cx!(st)),
        Err(ToolError::Invalid(_))
    ));
    assert!(!path.exists());
}

#[test]
fn an_oversized_envelope_is_refused() {
    // Incompressible noise in every slot overflows the 128 KiB buffer.
    let mut f = HopperFile::blank();
    let mut x = 0x1234_5678u32;
    for b in f.image_mut().iter_mut() {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = x as u8;
    }
    assert!(matches!(
        write_file(&f, Format::Envelope),
        Err(ToolError::Invalid(m)) if m.contains("built-in playlists")
    ));
    assert_eq!(write_file(&f, Format::Image).unwrap().len(), INFLATED_SIZE);
}

#[test]
fn a_blank_file_starts_clean() {
    let mut t = HopperTool::new();
    assert!(!t.is_open());
    t.new_blank();
    assert!(!t.is_dirty());
    assert_eq!(t.format(), Some(Format::Envelope));
    t.edit(|h| {
        model::add_record(&mut h[0]);
        h[0].hopper_type = 2;
        h[0].title = "First".into();
    });
    assert!(t.errors().is_empty());
    let built = t.build().unwrap();
    assert_eq!(built.text(0).unwrap().title, "First");
}

/// Draws the panel headless with a map list and an open preset, and checks
/// that drawing alone changes nothing.
#[test]
fn the_panel_draws_without_changing_anything() {
    let mut t = HopperTool::new();
    t.open_bytes(sample().image(), None).unwrap();
    t.set_catalog(
        "disc.iso".into(),
        Catalog {
            maps: vec![catalog::CatalogMap {
                id: 10,
                name: "Alpha".into(),
                description: String::new(),
                key: None,
            }],
        },
    );
    t.preset = Some(0);
    t.selected = 2;
    let ctx = egui::Context::default();
    for _ in 0..3 {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut st = None;
            t.ui(ui, &mut cx!(st));
        })
        .drop_without_applying_deltas();
    }
    assert!(!t.is_dirty());
    assert!(!t.can_undo());
    // No source in the context: the catalog set above stays.
    assert!(t.catalog().is_some());
}

// ---------- game variants ----------

use crate::dialog::{Picked, ScriptedDialog};
use hopper_edit::fixture::good_variant;
use live_proto::hopper::{RECORD, VARIANT_AT};

#[test]
fn linking_a_variant_is_one_undo_step_and_saves_byte_exact() {
    let f = sample();
    let mut t = HopperTool::new();
    t.open_bytes(f.image(), None).unwrap();
    let v = good_variant("Linked");
    t.set_entry_variant(1, 1, &v).unwrap();
    assert!(t.is_dirty());
    assert_eq!(t.entry_variant(1, 1).unwrap(), v);
    assert!(t.undo());
    assert!(!t.is_dirty());
    assert_ne!(t.entry_variant(1, 1).unwrap(), v);
    assert!(!t.can_undo(), "the link was a single step");
    assert!(t.redo());

    let path = scratch("linked.dat");
    let mut st = status();
    t.save_as(&path, &mut cx!(st)).unwrap();
    let (back, _) = read_file(&std::fs::read(&path).unwrap()).unwrap();
    let mut want = f.image().to_vec();
    let at = 0x6140 + 0x15cb8 + 0x48 + RECORD + VARIANT_AT;
    want[at..at + 0x130].copy_from_slice(v.bytes());
    assert_eq!(back.image(), &want[..], "only the variant's bytes changed");
    // After the save the entry reads back from the new image, unpicked.
    assert!(t.hoppers().unwrap()[1].records[1].picked.is_none());
    assert_eq!(t.entry_variant(1, 1).unwrap(), v);
}

#[test]
fn a_rejected_variant_leaves_no_trace() {
    let mut t = HopperTool::new();
    t.open_bytes(sample().image(), None).unwrap();
    // Territories with a zero territory count: the clean-up resets it.
    let bad = variants::GameVariant::blank(variants::GameType::Territories);
    let err = t.set_entry_variant(0, 0, &bad).unwrap_err();
    assert!(err.to_string().contains("the game would reject"), "{err}");
    assert!(!t.is_dirty() && !t.can_undo());
    assert!(matches!(
        t.set_entry_variant(0, 9, &good_variant("x")),
        Err(ToolError::Invalid(_))
    ));
}

#[test]
fn variant_files_load_by_path_or_dialog() {
    let v = good_variant("Shared");
    let path = scratch("shared.h2variant");
    std::fs::write(&path, variants::portable::export(&v)).unwrap();
    let mut t = HopperTool::new();
    t.set_variant_sources(&hopper_edit::fixture::schema(), None);
    assert_eq!(t.variant_choices().unwrap().builtins.len(), 2);

    // Typed path.
    assert_eq!(
        t.load_variant_file(&path).unwrap(),
        "Shared - Slayer - teams"
    );
    assert!(t.load_variant_file(&scratch("nope.h2variant")).is_err());

    // The dialog: cancelled does nothing, a pick loads, unavailable says
    // to type the path.
    let d = ScriptedDialog::new([Picked::Cancelled, Picked::Path(path.clone())]);
    let asked = d.asked.clone();
    t.set_file_dialog(Box::new(d));
    assert_eq!(t.browse_variant_file(), None);
    assert!(t.browse_variant_file().unwrap().is_ok());
    assert!(t.browse_variant_file().unwrap().is_err());
    let asked = asked.borrow();
    assert_eq!(asked[0].filters[0].extensions, ["h2variant"]);
    assert_eq!(t.variant_path, path.display().to_string());
    let c = t.variant_choices().unwrap();
    assert_eq!(c.files.len(), 1, "the same file twice is one choice");
    assert_eq!(c.files[0].variant, v);
}

#[test]
fn edit_in_variant_editor_hands_the_variant_to_the_variant_tool() {
    let mut t = HopperTool::new();
    t.open_bytes(sample().image(), None).unwrap();
    let v = good_variant("To edit");
    t.set_entry_variant(0, 1, &v).unwrap();
    assert!(t.edit_entry_variant(0, 1));
    let mut shell = crate::shell::Shell::new();
    shell.add_tool(Box::new(t));
    shell.add_tool(Box::new(crate::variants::VariantTool::new()));
    shell.dispatch_handoff();
    assert_eq!(shell.active(), 1, "the variant editor is shown");
    assert!(shell.tools()[1].is_dirty(), "holding an unsaved copy");
    assert!(matches!(shell.status(), Some(crate::tool::Status::Info(m)) if m.contains("To edit")));
    // Nothing more to hand over.
    shell.dispatch_handoff();
    assert_eq!(shell.active(), 1);
}

#[test]
fn a_handoff_nobody_takes_is_reported() {
    let mut t = HopperTool::new();
    t.open_bytes(sample().image(), None).unwrap();
    assert!(t.edit_entry_variant(0, 0));
    assert!(!t.edit_entry_variant(0, 50), "no such entry");
    let mut shell = crate::shell::Shell::new();
    shell.add_tool(Box::new(t));
    shell.dispatch_handoff();
    assert_eq!(shell.active(), 0);
    assert!(matches!(
        shell.status(),
        Some(crate::tool::Status::Error(_))
    ));
}

/// Draws the entries with the picker's sources set, a picked entry among
/// them, and checks drawing changes nothing.
#[test]
fn the_panel_draws_the_picker_without_changing_anything() {
    let mut t = HopperTool::new();
    t.open_bytes(sample().image(), None).unwrap();
    t.set_variant_sources(&hopper_edit::fixture::schema(), None);
    t.set_entry_variant(0, 0, &good_variant("Shown")).unwrap();
    t.set_file_dialog(Box::new(ScriptedDialog::default()));
    let ctx = egui::Context::default();
    for _ in 0..3 {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut st = None;
            t.ui(ui, &mut cx!(st));
        })
        .drop_without_applying_deltas();
    }
    assert!(t.can_undo());
    assert!(t.undo());
    assert!(!t.can_undo(), "drawing added no steps");
    // Sources set by hand are not re-read from the (absent) disc.
    assert_eq!(t.variant_choices().unwrap().builtins.len(), 2);
    assert!(t.entry_variant(0, 0).is_some());
}
