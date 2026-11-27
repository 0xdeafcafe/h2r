//! Edit files over loose tags (`edits`): invented `jpt!` tags built from
//! the definition's defaults, in a source held in memory.
use std::collections::BTreeMap;
use std::path::PathBuf;

use map::FourCC;
use tags::defs::{Defs, StructDef, Val, decode, default_record, encode};
use tags::repo::{Body, LooseTag, RawRes, Region, TagKey, TagSource};

use crate::edits::{self, EditFile, Edited, Origin, loose_hash, set_line};

const A: &str = "invented\\dmg\\a";
const B: &str = "invented\\dmg\\b";

fn jpt() -> &'static StructDef {
    Defs::builtin()
        .group(FourCC(*b"jpt!"))
        .expect("jpt! is defined")
}

fn key(path: &str) -> TagKey {
    TagKey::new(FourCC(*b"jpt!"), path)
}

fn tag(path: &str) -> LooseTag {
    encode(jpt(), key(path), &default_record(jpt()), Region::Metadata).unwrap()
}

/// Tags held in memory.
#[derive(Default)]
struct Mem(BTreeMap<TagKey, LooseTag>);

impl Mem {
    fn of(tags: &[LooseTag]) -> Self {
        Self(tags.iter().map(|t| (t.key.clone(), t.clone())).collect())
    }
}

impl TagSource for Mem {
    fn keys(&self) -> Vec<TagKey> {
        self.0.keys().cloned().collect()
    }
    fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
        Ok(self.0.get(key).cloned())
    }
    fn raw(&mut self, _: &RawRes) -> tags::repo::Result<Vec<u8>> {
        Err(tags::repo::Error::Invalid("no raw here".into()))
    }
    fn groups(&self) -> Vec<map::TagGroup> {
        Vec::new()
    }
}

fn file(name: &str, origin: Origin, text: &str) -> EditFile {
    EditFile::parse(name, origin, PathBuf::from(name), text.to_owned()).unwrap()
}

fn set(path: &str, field: &str, value: &str) -> String {
    format!("- tag: 'jpt!:{path}'\n  set:\n    - {{path: {field}, value: {value}}}\n")
}

fn field(t: &LooseTag, path: &str) -> Val {
    decode(jpt(), t).unwrap().at(path).unwrap().clone()
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("workspace-edits-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn an_edited_tag_reads_with_its_lines_and_others_read_as_the_layers_give_them() {
    let f = file(
        "assets/tag-edits/tuning.yaml",
        Origin::Git,
        &format!(
            "{}{}",
            set(A, "damage_lower_bound", "12.5"),
            "- tag: 'jpt!:invented\\dmg\\a'\n  block:\n    - {path: player_responses, op: add}\n"
        ),
    );
    let mut s = Edited::new(Mem::of(&[tag(A), tag(B)]), vec![f]).unwrap();
    assert!(s.check().is_empty());
    let a = s.read(&key(A)).unwrap().unwrap();
    assert_eq!(field(&a, "damage_lower_bound"), Val::Float(12.5));
    let Val::Block(responses) = field(&a, "player_responses") else {
        panic!("a block");
    };
    assert_eq!(responses.len(), 1);
    assert_eq!(s.read(&key(B)).unwrap().unwrap(), tag(B));
    assert_eq!(s.edited(), vec![key(A)]);
}

#[test]
fn a_git_file_with_was_is_refused_with_its_line() {
    let text = format!(
        "{}- tag: 'jpt!:{A}'\n  was: {}\n  set:\n    - {{path: damage_lower_bound, value: 1.0}}\n",
        set(B, "damage_lower_bound", "2.0"),
        loose_hash(&tag(A))
    );
    let p = EditFile::parse("assets/tag-edits/x.yaml", Origin::Git, "x".into(), text).unwrap_err();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].line, 4);
    assert!(
        p[0].why.contains("`was` is for local edit files"),
        "{}",
        p[0]
    );
    assert_eq!(p[0].tag, format!("jpt!:{A}"));
}

#[test]
fn a_dead_path_is_refused_when_the_file_is_read() {
    let p = EditFile::parse(
        "local",
        Origin::Local,
        "local".into(),
        set(A, "no_such_field", "1.0"),
    )
    .unwrap_err();
    assert_eq!((p[0].file.as_str(), p[0].line), ("local", 1));
    assert!(p[0].why.contains("no_such_field"), "{}", p[0]);
}

#[test]
fn a_stale_was_stops_with_the_file_and_line() {
    let ok = loose_hash(&tag(A));
    let mut changed = decode(jpt(), &tag(A)).unwrap();
    changed.set("damage_lower_bound", Val::Float(99.0)).unwrap();
    let other = loose_hash(&encode(jpt(), key(A), &changed, Region::Metadata).unwrap());
    assert_ne!(ok, other);
    let text = |was: &content::Sha256| {
        format!(
            "- tag: 'jpt!:{A}'\n  was: {was}\n  set:\n    - {{path: damage_lower_bound, value: 1.0}}\n"
        )
    };
    let mut fine = Edited::new(
        Mem::of(&[tag(A)]),
        vec![file("mine", Origin::Local, &text(&ok))],
    )
    .unwrap();
    assert!(fine.check().is_empty());
    let mut stale = Edited::new(
        Mem::of(&[tag(A)]),
        vec![file("mine", Origin::Local, &text(&other))],
    )
    .unwrap();
    let p = stale.check();
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].file.as_str(), p[0].line), ("mine", 1));
    assert!(p[0].why.contains("stale edit"), "{}", p[0]);
    // A read of the tag fails with the same words; nothing is dropped.
    let e = stale.read(&key(A)).unwrap_err().to_string();
    assert!(e.contains("mine:1") && e.contains("stale edit"), "{e}");
}

#[test]
fn a_tag_no_layer_holds_is_a_problem_not_a_skip() {
    let mut s = Edited::new(
        Mem::of(&[tag(B)]),
        vec![file(
            "mine",
            Origin::Local,
            &set(A, "damage_lower_bound", "1.0"),
        )],
    )
    .unwrap();
    let p = s.check();
    assert_eq!(p.len(), 1);
    assert!(p[0].why.contains("no layer holds this tag"), "{}", p[0]);
}

#[test]
fn a_from_the_field_does_not_hold_stops_the_build() {
    let text = format!(
        "- tag: 'jpt!:{A}'\n  set:\n    - {{path: damage_lower_bound, from: 5.0, value: 1.0}}\n"
    );
    let mut s = Edited::new(Mem::of(&[tag(A)]), vec![file("m", Origin::Local, &text)]).unwrap();
    let p = s.check();
    assert_eq!(p.len(), 1);
    assert!(p[0].why.contains("`from` damage_lower_bound"), "{}", p[0]);
    let held = format!(
        "- tag: 'jpt!:{A}'\n  set:\n    - {{path: damage_lower_bound, from: 0.0, value: 1.0}}\n"
    );
    let mut s = Edited::new(Mem::of(&[tag(A)]), vec![file("m", Origin::Local, &held)]).unwrap();
    assert!(s.check().is_empty());
}

#[test]
fn two_files_on_one_field_leave_the_later_and_list_it_and_on_two_fields_both_apply() {
    let git = file(
        "assets/tag-edits/a.yaml",
        Origin::Git,
        &format!(
            "{}{}",
            set(A, "damage_lower_bound", "2.0"),
            set(A, "stun_time", "0.5")
        ),
    );
    let local = file("mine", Origin::Local, &set(A, "damage_lower_bound", "3.0"));
    let mut s = Edited::new(Mem::of(&[tag(A)]), vec![git, local]).unwrap();
    assert!(s.check().is_empty());
    let a = s.read(&key(A)).unwrap().unwrap();
    assert_eq!(field(&a, "damage_lower_bound"), Val::Float(3.0));
    assert_eq!(field(&a, "stun_time"), Val::Float(0.5));
    let o = s.overrides();
    assert_eq!(o.len(), 1);
    assert_eq!(
        (
            o[0].path.as_str(),
            o[0].earlier.as_str(),
            o[0].later.as_str()
        ),
        ("damage_lower_bound", "assets/tag-edits/a.yaml", "mine")
    );
}

#[test]
fn an_edit_keeps_what_extraction_lost() {
    let mut lossy = tag(A);
    let Body::Tree(t) = &mut lossy.body else {
        panic!("a tree");
    };
    t.losses.push("invented: a field left empty".into());
    let mut s = Edited::new(
        Mem::of(&[lossy]),
        vec![file(
            "m",
            Origin::Local,
            &set(A, "damage_lower_bound", "1.0"),
        )],
    )
    .unwrap();
    let a = s.read(&key(A)).unwrap().unwrap();
    assert_eq!(
        a.tree().unwrap().losses,
        vec!["invented: a field left empty"]
    );
}

#[test]
fn set_line_writes_a_file_that_applies_and_replaces_its_own_line() {
    let ws = temp("set");
    let value = serde_json::json!(7.0);
    let was = loose_hash(&tag(A));
    let w = set_line(
        &ws,
        "mine",
        &format!("jpt!:{A}"),
        "damage_lower_bound",
        &value,
        Some(was),
    )
    .unwrap();
    assert!(!w.replaced);
    assert_eq!(w.was, Some(was.to_string()));
    let w = set_line(
        &ws,
        "mine",
        &format!("jpt!:{A}"),
        "damage_lower_bound",
        &serde_json::json!(8.0),
        Some(was),
    )
    .unwrap();
    assert!(w.replaced);
    let path = edits::local_path(&ws, "mine").unwrap();
    let f = EditFile::read("mine", Origin::Local, &path).unwrap();
    assert_eq!(f.entries.len(), 1);
    let mut s = Edited::new(Mem::of(&[tag(A)]), vec![f]).unwrap();
    let a = s.read(&key(A)).unwrap().unwrap();
    assert_eq!(field(&a, "damage_lower_bound"), Val::Float(8.0));
    // A value the field does not take, and a field the group lacks.
    assert!(
        set_line(
            &ws,
            "mine",
            &format!("jpt!:{A}"),
            "damage_lower_bound",
            &serde_json::json!("x"),
            None
        )
        .is_err()
    );
    assert!(set_line(&ws, "mine", &format!("jpt!:{A}"), "nope", &value, None).is_err());
    // An entry made on another hash is not mixed with this one.
    let e = set_line(
        &ws,
        "mine",
        &format!("jpt!:{A}"),
        "stun_time",
        &value,
        Some(content::Sha256::of(b"other")),
    )
    .unwrap_err();
    assert!(e.contains("was made on the tag with hash"), "{e}");
    let listed = edits::list(None, &ws);
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].name.as_str(), listed[0].entries), ("mine", 1));
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn edit_file_names_are_checked() {
    let ws = PathBuf::from("/ws");
    assert!(edits::local_path(&ws, "../x").is_err());
    assert!(edits::git_path(&ws, "assets/tag-edits/pogo.yaml").is_ok());
    assert!(edits::git_path(&ws, "assets/other/pogo.yaml").is_err());
    assert!(edits::git_path(&ws, "assets/tag-edits/../x.yaml").is_err());
}
