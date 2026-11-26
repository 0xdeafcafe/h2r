//! The map variant store over a temporary directory and invented documents
//! (docs/game/forge-map-variants.md, "The map variant file").
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use forge_doc::{BlockKind, BuilderVersions, Edit, ForgeDoc, MapRef, Op, OpId, Placement, Sha256};

use crate::files::variants::{self, Error, Problem, Source};

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "h2-variants-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, stem: &str) -> PathBuf {
        self.0
            .join("map-variants/fixture")
            .join(format!("{stem}.h2mapvariant"))
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hash() -> Sha256 {
    Sha256::of(b"an invented map")
}

fn doc(title: &str) -> ForgeDoc {
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "fixture".into(),
            sha256: hash(),
        },
        sources: vec![],
        title: title.into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![Op {
            id: OpId(1),
            edit: Edit::Placement(Placement::Add {
                block: BlockKind::NetgameFlags,
                like: None,
                fields: vec![],
            }),
        }],
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    doc
}

/// A document of the map tier: it removes a placement by index.
fn map_tier(title: &str) -> ForgeDoc {
    let mut doc = doc(title);
    doc.ops[0].edit = Edit::Placement(Placement::Remove {
        block: BlockKind::NetgameFlags,
        index: 0,
    });
    doc
}

fn list(dir: &Dir) -> Vec<variants::Listed> {
    variants::list(&dir.0, "fixture", Some(&hash())).unwrap()
}

#[test]
fn a_save_reads_back_to_the_same_id() {
    let dir = Dir::new();
    let d = doc("Two Towers");
    let file = variants::save(&dir.0, &d, false).unwrap();
    assert_eq!(file, "Two_Towers");
    let back = variants::load(&dir.0, "fixture", &file, Some(&hash())).unwrap();
    assert_eq!(back.id().unwrap(), d.id().unwrap());
    let listed = list(&dir);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "Two Towers");
    assert_eq!(listed[0].source, Source::Saved);
    assert_eq!(listed[0].id, Some(d.id().unwrap()));
    assert_eq!(listed[0].problem, None);
}

#[test]
fn save_writes_the_play_form_without_the_kit() {
    let dir = Dir::new();
    let session = forge_doc::session_form(&doc("Kit"));
    assert_eq!(session.tier(), forge_doc::Tier::Variant);
    let file = variants::save(&dir.0, &session, false).unwrap();
    let back = variants::load(&dir.0, "fixture", &file, Some(&hash())).unwrap();
    assert!(back.recipes.is_empty());
    assert_eq!(back.id().unwrap(), doc("Kit").id().unwrap());
}

#[test]
fn a_taken_name_is_refused_without_case_and_replace_overwrites_it() {
    let dir = Dir::new();
    variants::save(&dir.0, &doc("Two Towers"), false).unwrap();
    let again = doc("TWO towers");
    let err = variants::save(&dir.0, &again, false).unwrap_err();
    assert_eq!(err, Error::NameTaken("TWO towers".into()));
    assert_eq!(err.code(), "variant_name_taken");
    assert_eq!(list(&dir).len(), 1);
    let file = variants::save(&dir.0, &again, true).unwrap();
    assert_eq!(file, "Two_Towers");
    let listed = list(&dir);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "TWO towers");
}

#[test]
fn names_that_make_one_stem_get_two_files() {
    let dir = Dir::new();
    assert_eq!(variants::save(&dir.0, &doc("A B"), false).unwrap(), "A_B");
    assert_eq!(variants::save(&dir.0, &doc("A_B"), false).unwrap(), "A_B-2");
    assert_eq!(list(&dir).len(), 2);
}

#[test]
fn a_bad_name_or_a_map_tier_document_is_not_saved() {
    let dir = Dir::new();
    let long = variants::save(&dir.0, &doc(&"x".repeat(17)), false).unwrap_err();
    assert_eq!(long.code(), "variant_name");
    let blank = variants::save(&dir.0, &doc("  "), false).unwrap_err();
    assert_eq!(blank.code(), "variant_name");
    let tier = variants::save(&dir.0, &map_tier("Map"), false).unwrap_err();
    assert_eq!(tier.code(), "variant_tier");
    assert_eq!(tier.to_string(), "Map is a Forge map, not a map variant.");
    assert!(list(&dir).is_empty());
}

#[test]
fn a_file_of_the_map_tier_lists_and_loads_as_variant_tier() {
    let dir = Dir::new();
    let bytes = map_tier("Big").canonical_json().unwrap();
    fs::create_dir_all(dir.file("x").parent().unwrap()).unwrap();
    fs::write(dir.file("Big"), bytes).unwrap();
    let listed = list(&dir);
    assert_eq!(listed[0].problem, Some(Problem::Tier));
    assert_eq!(
        listed[0].sentence.as_deref(),
        Some("Big is a Forge map, not a map variant.")
    );
    let err = variants::load(&dir.0, "fixture", "Big", Some(&hash())).unwrap_err();
    assert_eq!(err.code(), "variant_tier");
}

#[test]
fn a_base_of_another_hash_or_no_map_lists_with_its_problem() {
    let dir = Dir::new();
    variants::save(&dir.0, &doc("Two Towers"), false).unwrap();
    let other = Sha256::of(b"another version");
    let listed = variants::list(&dir.0, "fixture", Some(&other)).unwrap();
    assert_eq!(listed[0].problem, Some(Problem::BaseHash));
    assert_eq!(
        listed[0].sentence.as_deref(),
        Some("Two Towers was made for another version of fixture.")
    );
    let err = variants::load(&dir.0, "fixture", "Two_Towers", Some(&other)).unwrap_err();
    assert_eq!(err.code(), "variant_base_hash");
    let listed = variants::list(&dir.0, "fixture", None).unwrap();
    assert_eq!(listed[0].problem, Some(Problem::BaseMissing));
    assert_eq!(
        listed[0].sentence.as_deref(),
        Some("Two Towers needs fixture, which is not installed.")
    );
}

#[test]
fn another_builder_or_format_is_named() {
    let dir = Dir::new();
    let mut old = doc("Old");
    old.builder.forge_build += 1;
    fs::create_dir_all(dir.file("x").parent().unwrap()).unwrap();
    fs::write(dir.file("Old"), old.canonical_json().unwrap()).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_str(&doc("Newer").canonical_json().unwrap()).unwrap();
    value["format"] = 99.into();
    fs::write(dir.file("Newer"), serde_json::to_vec(&value).unwrap()).unwrap();
    fs::write(dir.file("Junk"), b"not json").unwrap();
    let by: Vec<_> = list(&dir)
        .into_iter()
        .map(|l| (l.file, l.problem))
        .collect();
    assert!(by.contains(&("Old".into(), Some(Problem::Builder))));
    assert!(by.contains(&("Newer".into(), Some(Problem::Format))));
    assert!(by.contains(&("Junk".into(), Some(Problem::Unreadable))));
    let err = variants::load(&dir.0, "fixture", "Newer", Some(&hash())).unwrap_err();
    assert_eq!(err.code(), "variant_format");
    assert_eq!(
        err.to_string(),
        "Newer was made with another version of Forge."
    );
}

#[test]
fn a_document_in_another_maps_directory_is_unreadable() {
    let dir = Dir::new();
    let mut other = doc("Stray");
    other.base.map = "elsewhere".into();
    fs::create_dir_all(dir.file("x").parent().unwrap()).unwrap();
    fs::write(dir.file("Stray"), other.canonical_json().unwrap()).unwrap();
    assert_eq!(list(&dir)[0].problem, Some(Problem::Unreadable));
}

#[test]
fn list_is_by_name_without_case_and_has_the_live_copies_after() {
    let dir = Dir::new();
    variants::save(&dir.0, &doc("beta"), false).unwrap();
    variants::save(&dir.0, &doc("Alpha"), false).unwrap();
    let live = doc("Fetched");
    let live_dir = dir.0.join("map-variants/fixture/live");
    fs::create_dir_all(&live_dir).unwrap();
    fs::write(
        live_dir.join("abc.h2mapvariant"),
        live.canonical_json().unwrap(),
    )
    .unwrap();
    let names: Vec<_> = list(&dir).into_iter().map(|l| (l.name, l.source)).collect();
    assert_eq!(
        names,
        vec![
            ("Alpha".into(), Source::Saved),
            ("beta".into(), Source::Saved),
            ("Fetched".into(), Source::Live),
        ]
    );
    assert!(variants::list(&dir.0, "nothing", None).unwrap().is_empty());
}

#[test]
fn rename_changes_the_id_and_replaces_nothing() {
    let dir = Dir::new();
    let file = variants::save(&dir.0, &doc("Two Towers"), false).unwrap();
    variants::save(&dir.0, &doc("Other"), false).unwrap();
    let before = list(&dir);
    let taken = variants::rename(&dir.0, "fixture", &file, "other").unwrap_err();
    assert_eq!(taken, Error::NameTaken("other".into()));
    assert_eq!(list(&dir), before);
    variants::rename(&dir.0, "fixture", &file, "Three Towers").unwrap();
    let renamed = list(&dir).into_iter().find(|l| l.file == file).unwrap();
    assert_eq!(renamed.name, "Three Towers");
    assert_ne!(renamed.id, Some(doc("Two Towers").id().unwrap()));
    assert_eq!(renamed.id, Some(doc("Three Towers").id().unwrap()));
    // A change of case alone is the same variant, not a taken name.
    variants::rename(&dir.0, "fixture", &file, "THREE TOWERS").unwrap();
    assert_eq!(
        variants::rename(&dir.0, "fixture", &file, "x".repeat(17).as_str())
            .unwrap_err()
            .code(),
        "variant_name"
    );
    assert_eq!(
        variants::rename(&dir.0, "fixture", "nope", "Fine")
            .unwrap_err()
            .code(),
        "variant_file_missing"
    );
}

#[test]
fn a_variant_with_a_problem_can_still_be_renamed_and_deleted() {
    let dir = Dir::new();
    let file = variants::save(&dir.0, &doc("Two Towers"), false).unwrap();
    let other = Sha256::of(b"another version");
    assert!(variants::load(&dir.0, "fixture", &file, Some(&other)).is_err());
    variants::rename(&dir.0, "fixture", &file, "Renamed").unwrap();
    variants::delete(&dir.0, "fixture", &file).unwrap();
    assert!(list(&dir).is_empty());
    fs::write(dir.file("Junk"), b"not json").unwrap();
    variants::delete(&dir.0, "fixture", "Junk").unwrap();
    assert_eq!(
        variants::delete(&dir.0, "fixture", "Junk")
            .unwrap_err()
            .code(),
        "variant_file_missing"
    );
}

#[test]
fn duplicate_copies_the_edits_under_a_new_name_and_id() {
    let dir = Dir::new();
    let file = variants::save(&dir.0, &doc("Two Towers"), false).unwrap();
    let copy = variants::duplicate(&dir.0, "fixture", &file, "Copy").unwrap();
    assert_eq!(copy, "Copy");
    let a = variants::load(&dir.0, "fixture", &file, Some(&hash())).unwrap();
    let b = variants::load(&dir.0, "fixture", &copy, Some(&hash())).unwrap();
    assert_eq!(a.ops, b.ops);
    assert_ne!(a.id().unwrap(), b.id().unwrap());
    let taken = variants::duplicate(&dir.0, "fixture", &file, "copy").unwrap_err();
    assert_eq!(taken, Error::NameTaken("copy".into()));
    assert_eq!(list(&dir).len(), 2);
}

#[test]
fn a_map_name_or_file_handle_that_is_a_path_is_refused() {
    let dir = Dir::new();
    assert_eq!(
        variants::list(&dir.0, "../x", None).unwrap_err().code(),
        "variant_name"
    );
    assert_eq!(
        variants::delete(&dir.0, "fixture", "../x")
            .unwrap_err()
            .code(),
        "variant_file_missing"
    );
    let mut stray = doc("Stray");
    stray.base.map = "../x".into();
    assert!(variants::save(&dir.0, &stray, false).is_err());
}

#[test]
fn suggestable_lists_every_maps_usable_variants_and_leaves_the_rest_out() {
    let dir = Dir::new();
    // None on a machine with no variant directory.
    assert!(variants::suggestable(&dir.0).unwrap().is_empty());
    let a = doc("Zeta");
    let b = doc("alpha");
    variants::save(&dir.0, &a, false).unwrap();
    variants::save(&dir.0, &b, false).unwrap();
    // A map-tier file, an unparsable file and a file in another map's
    // directory are not suggestable.
    fs::write(dir.file("Tiered"), map_tier("Tiered").canonical_json().unwrap()).unwrap();
    fs::write(dir.file("Broken"), b"not a document").unwrap();
    let other = dir.0.join("map-variants/elsewhere");
    fs::create_dir_all(&other).unwrap();
    fs::copy(dir.file("Zeta"), other.join("Stray.h2mapvariant")).unwrap();
    let got = variants::suggestable(&dir.0).unwrap();
    let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["alpha", "Zeta"], "by name without case, one map's");
    assert_eq!(got[1].id, a.id().unwrap());
    assert_eq!(got[1].base, hash());
    assert_eq!((got[1].map.as_str(), got[1].file.as_str()), ("fixture", "Zeta"));
    assert!(got.iter().all(|s| s.source == Source::Saved));
}
