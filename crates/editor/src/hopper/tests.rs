//! The tool's file handling and undo, without a window.

use super::model::tests::sample;
use super::*;

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
