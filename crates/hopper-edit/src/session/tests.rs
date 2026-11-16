//! The session's file handling, undo and sources, without a UI.

use super::*;
use crate::fixture::{good_variant, sample, schema};
use live_proto::hopper::{RECORD, VARIANT_AT};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hopper-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn reads_both_forms_and_refuses_garbage() {
    let f = sample();
    let (a, fa) = read_file(&f.build()).unwrap();
    let (b, fb) = read_file(f.image()).unwrap();
    assert_eq!((fa, fb), (Format::Envelope, Format::Image));
    assert_eq!(a.image(), f.image());
    assert_eq!(b.image(), f.image());
    assert!(matches!(read_file(b"nope"), Err(Error::Invalid(_))));
}

#[test]
fn opens_a_path_and_saves_a_raw_image_byte_for_byte() {
    let f = sample();
    let path = scratch("raw.dat");
    std::fs::write(&path, f.image()).unwrap();
    let mut s = HopperSession::new();
    assert_eq!(s.open_path(&path).unwrap(), Format::Image);
    assert_eq!(s.path(), Some(path.as_path()));
    assert!(!s.is_dirty());
    let out = scratch("raw-out.dat");
    assert_eq!(s.save_to(&out).unwrap(), f.image().len());
    assert_eq!(std::fs::read(&out).unwrap(), f.image());
    assert_eq!(s.path(), Some(out.as_path()));
    s.close();
    assert!(!s.is_open() && !s.is_dirty());
}

#[test]
fn keyed_edits_are_one_step_until_sealed() {
    let mut s = HopperSession::new();
    s.open_bytes(sample().image(), None).unwrap();
    for t in ["O", "On", "One"] {
        s.edit_with(|hs, _| {
            hs[0].title = t.into();
            ((), Some(7))
        });
    }
    // Drawing a frame with nothing changed is no step.
    s.edit_with(|_, _| ((), Some(7)));
    s.seal();
    s.edit_with(|hs, _| {
        hs[0].title = "Two".into();
        ((), Some(7))
    });
    assert!(s.undo());
    assert_eq!(s.hoppers().unwrap()[0].title, "One");
    assert!(s.undo());
    assert_eq!(s.hoppers().unwrap()[0].title, "List 0");
    assert!(!s.can_undo() && !s.is_dirty());
}

#[test]
fn the_edit_view_carries_the_baseline_and_sources() {
    let mut s = HopperSession::new();
    assert!(s.edit_with(|_, _| ((), None)).is_none(), "no file open");
    s.open_bytes(sample().image(), None).unwrap();
    s.set_variant_sources(&schema(), None);
    s.set_catalog("disc.iso".into(), crate::catalog::Catalog::default());
    let (n, choices, catalog) = s
        .edit_with(|hs, v| {
            hs[1].title = "Changed".into();
            (
                (
                    v.baseline[1].title.clone(),
                    v.choices.map(|c| c.builtins.len()),
                    v.catalog.is_some(),
                ),
                None,
            )
        })
        .unwrap();
    assert_eq!((n.as_str(), choices, catalog), ("List 1", Some(2), true));
    assert_eq!(s.errors(), Vec::<String>::new());
    assert!(s.is_dirty());
}

#[test]
fn linking_a_variant_is_one_step_and_saves_byte_exact() {
    let f = sample();
    let mut s = HopperSession::new();
    s.open_bytes(f.image(), None).unwrap();
    let v = good_variant("Linked");
    s.set_entry_variant(1, 1, &v).unwrap();
    assert_eq!(s.entry_variant(1, 1).unwrap(), v);
    assert!(s.undo() && !s.is_dirty() && !s.can_undo());
    assert!(s.redo());
    let path = scratch("linked.dat");
    s.save_to(&path).unwrap();
    let (back, _) = read_file(&std::fs::read(&path).unwrap()).unwrap();
    let mut want = f.image().to_vec();
    let at = 0x6140 + 0x15cb8 + 0x48 + RECORD + VARIANT_AT;
    want[at..at + 0x130].copy_from_slice(v.bytes());
    assert_eq!(back.image(), &want[..]);
    // A rejected variant leaves nothing behind.
    let bad = variants::GameVariant::blank(variants::GameType::Territories);
    assert!(s.set_entry_variant(0, 0, &bad).is_err());
    assert!(!s.is_dirty());
}

#[test]
fn without_a_disc_there_is_a_catalog_note_and_the_fallback_schema() {
    let mut s = HopperSession::new();
    s.set_data_root(std::env::temp_dir().join(format!("hopper-edit-data-{}", std::process::id())));
    s.refresh_catalog(None);
    assert!(s.catalog().is_none());
    assert!(s.catalog_note().unwrap().contains("No disc"));
    s.refresh_variants(None);
    let c = s.variant_choices().unwrap();
    assert!(c.saved.is_empty());
    // Read once per source: a second call keeps what was read.
    s.refresh_variants(None);
    assert!(s.variant_choices().is_some());
}

#[test]
fn the_default_lineup_starts_clean_and_valid() {
    let mut s = HopperSession::new();
    s.new_default_set();
    assert!(!s.is_dirty() && s.path().is_none());
    assert_eq!(s.format(), Some(Format::Envelope));
    let used = s
        .hoppers()
        .unwrap()
        .iter()
        .filter(|h| !h.records.is_empty())
        .count();
    assert!(used > 0);
    assert!(s.errors().is_empty());
    assert!(s.build().is_ok());
}
