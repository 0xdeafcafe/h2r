//! PK10 (docs/editor/studio-redesign.md, "Conversions"): `convert` items
//! with an invented converter. No disc bytes: the converter's "disc" is a
//! word and its outputs are small files it writes.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use content::package::{Item, Kind, Maps};

use super::*;
use crate::convert::{ConvertSpec, Input, Stamp, Unit, stamp_path};
use crate::tests::pkg;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("workspace-pk10-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const RECIPE: &str = "convert/fake.convert.toml";

/// One package, `conv`, of one `convert` item naming the `fake` converter.
fn package(recipe: &str) -> Loaded {
    pkg(
        "conv",
        &[],
        &[("convert", RECIPE, None, "all")],
        &[(RECIPE, recipe.as_bytes())],
    )
}

/// An invented converter: its disc is a word (or none), its units are
/// named with keys, and each writes `<out>/<unit>`.
struct Fake {
    disc: Option<String>,
    units: Vec<(String, String)>,
    out: PathBuf,
    /// Convert this many, then stop as an interrupted run does.
    stop_after: Option<usize>,
    /// Units that fail.
    fail: Vec<String>,
    /// Output its own command made is in place.
    kept: bool,
    ran: RefCell<Vec<String>>,
}

impl Fake {
    fn new(out: &Path, units: &[(&str, &str)]) -> Self {
        Self {
            disc: Some("disc-1".into()),
            units: units
                .iter()
                .map(|(n, k)| ((*n).to_owned(), (*k).to_owned()))
                .collect(),
            out: out.to_owned(),
            stop_after: None,
            fail: Vec::new(),
            kept: false,
            ran: RefCell::new(Vec::new()),
        }
    }

    fn take_ran(&self) -> Vec<String> {
        std::mem::take(&mut *self.ran.borrow_mut())
    }
}

impl Converter for Fake {
    fn name(&self) -> &str {
        "fake"
    }

    fn version(&self) -> String {
        "1".into()
    }

    fn input(&self, _: &ConvertSpec) -> Result<Input, String> {
        let disc = self
            .disc
            .clone()
            .ok_or_else(|| "no invented disc beside the game disc".to_owned())?;
        Ok(Input {
            path: PathBuf::from(&disc),
            label: disc.clone(),
            identity: disc,
        })
    }

    fn kept(&self, _: &ConvertSpec) -> bool {
        self.kept
    }

    fn units(&self, _: &Input, spec: &ConvertSpec) -> Result<Vec<Unit>, String> {
        if let Some(k) = spec.settings.keys().find(|k| *k != "only") {
            return Err(format!("unknown setting {k:?}"));
        }
        let only = spec.settings.get("only").and_then(|v| v.as_str());
        Ok(self
            .units
            .iter()
            .filter(|(n, _)| only.is_none_or(|o| o == n))
            .map(|(n, k)| Unit {
                name: n.clone(),
                key: k.clone(),
                output: Some(self.out.join(n)),
            })
            .collect())
    }

    fn convert(
        &self,
        _: &Input,
        _: &ConvertSpec,
        todo: &[Unit],
        done: &mut dyn FnMut(&str, Result<(), String>),
    ) {
        for (n, u) in todo.iter().enumerate() {
            if self.stop_after.is_some_and(|s| n >= s) {
                return;
            }
            self.ran.borrow_mut().push(u.name.clone());
            if self.fail.contains(&u.name) {
                done(&u.name, Err("the invented map does not convert".into()));
                continue;
            }
            std::fs::create_dir_all(&self.out).unwrap();
            std::fs::write(self.out.join(&u.name), b"converted").unwrap();
            done(&u.name, Ok(()));
        }
    }
}

fn run(data: &Path, p: &Loaded, conv: &Fake, force: bool) -> Converted {
    convert(
        data,
        std::slice::from_ref(p),
        &[conv as &dyn Converter],
        force,
        &mut |_| {},
    )
}

const ABC: [(&str, &str); 3] = [("a", "1"), ("b", "1"), ("c", "1")];

#[test]
fn no_disc_leaves_the_package_off_with_its_reason() {
    let data = temp("no-disc");
    let p = package("converter = \"fake\"\n");
    let mut conv = Fake::new(&data.join("out"), &ABC);
    conv.disc = None;
    let out = run(&data, &p, &conv, false);
    assert_eq!(out.off.len(), 1, "{out:?}");
    assert_eq!(out.off[0].package, "conv");
    assert!(out.off[0].why.contains("no invented disc"), "{out:?}");
    assert!(conv.take_ran().is_empty());
    let off = inputs_off(&data);
    assert_eq!(
        off.get("conv").map(String::as_str),
        Some(out.off[0].why.as_str())
    );

    // The game's set lists it off, with the reason; prepare still sees it.
    let set = GameSet::read(vec![p.clone()], &data, &|_| true);
    assert!(set.on.is_empty(), "{:?}", set.listed);
    let row = set.listed.iter().find(|l| l.id == "conv").unwrap();
    assert!(!row.on);
    assert!(
        row.why
            .as_deref()
            .unwrap_or_default()
            .contains("no invented disc"),
        "{row:?}"
    );
    let unprepared = GameSet::read_unprepared(vec![p.clone()], &data, &|_| true);
    assert_eq!(unprepared.on.len(), 1);

    // The disc arrives: the next run converts and the package is on.
    conv.disc = Some("disc-1".into());
    let out = run(&data, &p, &conv, false);
    assert!(out.off.is_empty());
    assert_eq!(conv.take_ran(), ["a", "b", "c"]);
    assert!(inputs_off(&data).is_empty());
    assert_eq!(GameSet::read(vec![p], &data, &|_| true).on.len(), 1);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_stale_stamp_reruns_one_map() {
    let data = temp("stale");
    let p = package("converter = \"fake\"\n");
    let mut conv = Fake::new(&data.join("out"), &ABC);
    let first = run(&data, &p, &conv, false);
    assert_eq!(conv.take_ran(), ["a", "b", "c"]);
    assert_eq!(first.ran.len(), 3);
    assert!(stamp_path(&data, "conv").is_file());

    let again = run(&data, &p, &conv, false);
    assert!(conv.take_ran().is_empty());
    assert_eq!(again.held, 3);

    // Unit b's own key moves: b alone converts again.
    conv.units[1].1 = "2".into();
    let stale = run(&data, &p, &conv, false);
    assert_eq!(conv.take_ran(), ["b"]);
    assert_eq!(stale.held, 2);

    // Its output goes: that one again.
    std::fs::remove_file(data.join("out").join("c")).unwrap();
    run(&data, &p, &conv, false);
    assert_eq!(conv.take_ran(), ["c"]);

    // Another disc: every unit's key moves.
    conv.disc = Some("disc-2".into());
    run(&data, &p, &conv, false);
    assert_eq!(conv.take_ran(), ["a", "b", "c"]);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn an_interrupted_run_resumes() {
    let data = temp("resume");
    let p = package("converter = \"fake\"\n");
    let mut conv = Fake::new(&data.join("out"), &ABC);
    conv.stop_after = Some(1);
    let cut = run(&data, &p, &conv, false);
    assert_eq!(conv.take_ran(), ["a"]);
    let stopped: Vec<_> = cut.failed.iter().filter_map(|f| f.unit.clone()).collect();
    assert_eq!(stopped, ["b", "c"]);
    // The stamp holds what finished.
    let stamp = Stamp::read(&stamp_path(&data, "conv")).unwrap();
    assert_eq!(stamp.items[0].done.keys().collect::<Vec<_>>(), ["a"]);

    conv.stop_after = None;
    let rest = run(&data, &p, &conv, false);
    assert_eq!(conv.take_ran(), ["b", "c"]);
    assert_eq!(rest.held, 1);
    assert!(rest.failed.is_empty(), "{rest:?}");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_failed_unit_waits_for_a_new_key_or_force() {
    let data = temp("failed");
    let p = package("converter = \"fake\"\n");
    let mut conv = Fake::new(&data.join("out"), &ABC);
    conv.fail = vec!["b".into()];
    let first = run(&data, &p, &conv, false);
    assert_eq!(first.failed.len(), 1);
    assert_eq!(first.failed[0].unit.as_deref(), Some("b"));
    conv.take_ran();

    let again = run(&data, &p, &conv, false);
    assert!(conv.take_ran().is_empty());
    assert_eq!((again.held, again.not_retried), (2, 1));

    conv.fail.clear();
    run(&data, &p, &conv, true);
    assert_eq!(conv.take_ran(), ["a", "b", "c"]);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_disc_taken_away_keeps_what_was_converted() {
    let data = temp("gone");
    let p = package("converter = \"fake\"\n");
    let mut conv = Fake::new(&data.join("out"), &ABC);
    run(&data, &p, &conv, false);
    conv.take_ran();
    conv.disc = None;
    let out = run(&data, &p, &conv, false);
    assert!(out.off.is_empty(), "{out:?}");
    assert_eq!(out.held, 3);
    assert!(inputs_off(&data).is_empty());

    // Nor is a package off whose converter's own command made its output.
    let fresh = temp("gone-kept");
    let mut own = Fake::new(&fresh.join("out"), &ABC);
    own.disc = None;
    own.kept = true;
    let out = run(&fresh, &p, &own, false);
    assert!(out.off.is_empty(), "{out:?}");
    assert!(inputs_off(&fresh).is_empty());
    let _ = std::fs::remove_dir_all(&fresh);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn settings_change_the_units_and_their_keys() {
    let data = temp("settings");
    let conv = Fake::new(&data.join("out"), &ABC);
    run(
        &data,
        &package("converter = \"fake\"\n[settings]\nonly = \"a\"\n"),
        &conv,
        false,
    );
    assert_eq!(conv.take_ran(), ["a"]);
    // Other settings are another key, so a is converted again with them.
    run(&data, &package("converter = \"fake\"\n"), &conv, false);
    assert_eq!(conv.take_ran(), ["a", "b", "c"]);
    let bad = run(
        &data,
        &package("converter = \"fake\"\n[settings]\nwho = 1\n"),
        &conv,
        false,
    );
    assert!(bad.failed[0].why.contains("who"), "{bad:?}");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_converter_left_out_of_the_run_leaves_its_stamp() {
    let data = temp("skipped");
    let p = package("converter = \"fake\"\n");
    let conv = Fake::new(&data.join("out"), &ABC);
    run(&data, &p, &conv, false);
    let before = std::fs::read_to_string(stamp_path(&data, "conv")).unwrap();
    let out = convert(&data, std::slice::from_ref(&p), &[], false, &mut |_| {});
    assert_eq!(out.skipped.len(), 1);
    assert_eq!(out.skipped[0].converter, "fake");
    assert_eq!(
        std::fs::read_to_string(stamp_path(&data, "conv")).unwrap(),
        before
    );
    let _ = std::fs::remove_dir_all(&data);
}

fn check_recipe(text: &str) -> Result<(), Failure> {
    let item = Item {
        kind: Kind::Convert,
        source: Some(RECIPE.into()),
        inputs: vec![],
        tag: None,
        maps: Maps::default(),
    };
    let files = BTreeMap::new();
    (builder(Kind::Convert).unwrap().check)(&Job {
        item: &item,
        source: text.as_bytes(),
        files: &files,
    })
}

#[test]
fn the_convert_builder_checks_the_recipe_and_changes_no_map() {
    assert!(check_recipe("converter = \"halo1\"\n").is_ok());
    assert!(check_recipe("converter = \"map-pack\"\n").is_ok());
    let why = check_recipe("converter = \"halo3\"\n")
        .unwrap_err()
        .why;
    assert!(why.contains("halo3"), "{why}");
    assert!(check_recipe("converter = \"halo1\"\nmaps = 3\n").is_err());
    let item = Item {
        kind: Kind::Convert,
        source: Some(RECIPE.into()),
        inputs: vec![],
        tag: None,
        maps: Maps::default(),
    };
    assert!(!changes_map(&item));
}

#[test]
fn our_conversion_packages_check_clean_and_name_their_converter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content");
    for (id, converter) in [("halo1", "halo1"), ("map-pack", "map-pack")] {
        let loaded = Loaded::from_dir(&root.join(id)).unwrap();
        assert_eq!(check_package(&loaded), vec![], "{id}");
        let names: Vec<String> = loaded
            .package
            .manifest
            .items
            .iter()
            .filter(|i| i.kind == Kind::Convert)
            .map(|i| {
                let bytes = &loaded.files[i.source.as_ref().unwrap()];
                ConvertSpec::parse(bytes).unwrap().converter
            })
            .collect();
        assert_eq!(names, [converter], "{id}");
    }
}
