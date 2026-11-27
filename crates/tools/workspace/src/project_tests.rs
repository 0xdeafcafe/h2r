//! Projects over an invented tree (`project`, `project_build`): tags of
//! invented names in a tag repository the test writes, no disc.
use std::path::{Path, PathBuf};

use map::FourCC;
use tags::defs::{Defs, Val, decode, default_record, encode};
use tags::repo::store::Manifest;
use tags::repo::{Body, LooseTag, Node, Region, TagKey, TagRepo, TagSource, Tree, Value};

use crate::edits::Origin;
use crate::project::{
    BuildMode, EditRef, Project, ProjectError, Roots, discover, git_checkout, load, open_stack,
};
use crate::project_build::{Installed, Keep, Kit, KitStep, Tools, build, fresh_roots, lossy};

const BASE: &str = "tiny";
const SCENARIO: &str = "invented\\scenarios\\tiny";
const A: &str = "invented\\dmg\\a";
const B: &str = "invented\\dmg\\b";

fn jpt_key(path: &str) -> TagKey {
    TagKey::new(FourCC(*b"jpt!"), path)
}

fn jpt(path: &str, lost: bool) -> LooseTag {
    let def = Defs::builtin().group(FourCC(*b"jpt!")).unwrap();
    let mut t = encode(def, jpt_key(path), &default_record(def), Region::Metadata).unwrap();
    if lost {
        let Body::Tree(tree) = &mut t.body else {
            panic!("a tree");
        };
        tree.losses.push("invented: a field left empty".into());
    }
    t
}

/// A tag of no definition: one element, naming `refs` by key.
fn plain(key: TagKey, refs: &[TagKey]) -> LooseTag {
    let n = refs.len().max(1) as u32 * 4;
    LooseTag {
        key,
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![Node {
                bytes: vec![0; n as usize],
                count: 1,
                size: n,
                align: 4,
                fields: refs
                    .iter()
                    .enumerate()
                    .map(|(i, k)| (i as u32 * 4, Value::Ref(k.clone())))
                    .collect(),
            }],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    }
}

struct Dirs {
    root: PathBuf,
    roots: Roots,
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A tree holding map `tiny`: globals, a scenario naming damage tag A
/// (lossy when `a_lossy`), and damage tag B.
fn dirs(name: &str, a_lossy: bool) -> Dirs {
    let root = std::env::temp_dir().join(format!("workspace-proj-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let tree = root.join("tags");
    let globals = TagKey::new(FourCC(*b"matg"), "globals\\globals");
    let scenario = TagKey::new(FourCC(*b"scnr"), SCENARIO);
    let tags = [
        plain(globals.clone(), &[]),
        plain(scenario.clone(), &[jpt_key(A)]),
        jpt(A, a_lossy),
        jpt(B, false),
    ];
    let mut repo = TagRepo::create(&tree).unwrap();
    for t in &tags {
        repo.put(t).unwrap();
    }
    let m = Manifest {
        map: BASE.into(),
        cache_type: 1,
        name: BASE.into(),
        scenario: Some(scenario),
        globals: Some(globals),
        tags: tags.iter().map(|t| (t.key.clone(), None)).collect(),
        ..Default::default()
    };
    repo.put_manifest(&m, &[0; 0x800]).unwrap();
    let roots = Roots {
        tree,
        data: root.join("data"),
        checkout: Some(root.join("checkout")),
    };
    Dirs { root, roots }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn project_text(name: &str, extra: &str) -> String {
    format!(
        "name = \"{name}\"\nmode = \"fresh\"\nbase = \"{BASE}\"\nscenario = '{SCENARIO}'\noutput = \"package\"\n{extra}"
    )
}

fn local_project(d: &Dirs, name: &str, extra: &str) {
    write(
        &d.roots
            .workspace()
            .join("projects")
            .join(format!("{name}.toml")),
        &project_text(name, extra),
    );
}

fn set_a(value: f32) -> String {
    format!("- tag: 'jpt!:{A}'\n  set:\n    - {{path: damage_lower_bound, value: {value}}}\n")
}

fn tools<'a>(kits: &'a [&'a dyn Kit]) -> Tools<'a> {
    Tools {
        kits,
        keep: Keep::NONE,
    }
}

#[test]
fn the_stage_project_parses_and_bad_ones_name_their_field() {
    let stage = include_str!("../../../../art/stage/project.toml");
    let p = Project::parse(stage, "art/stage/project.toml").unwrap();
    assert_eq!(
        (p.name.as_str(), p.mode, p.kits.as_slice()),
        ("stage", BuildMode::Fresh, &["stage".to_owned()][..])
    );
    let bad = |extra: &str, mode: &str| {
        let text = format!(
            "name = \"x\"\nmode = \"{mode}\"\nbase = \"lockout\"\noutput = \"package\"\n{extra}"
        );
        Project::parse(&text, "p.toml").unwrap_err()
    };
    assert!(bad("", "fresh").contains("names its scenario"));
    assert!(bad("scenario = 's'\n", "onto").contains("drop `scenario`"));
    assert!(bad("edits = [\"a/b.yaml\"]\n", "onto").contains("edits entry"));
    assert!(bad("edits = [\"x\", \"x\"]\n", "onto").contains("named twice"));
    assert!(bad("imports = [\"../m\"]\n", "onto").contains("import layer"));
    assert!(bad("colour = 1\n", "onto").contains("colour"));
    assert_eq!(
        EditRef::parse("assets/tag-edits/pogo.yaml"),
        Ok(EditRef::Git("assets/tag-edits/pogo.yaml".into()))
    );
    assert_eq!(EditRef::parse("mine"), Ok(EditRef::Local("mine".into())));
}

#[test]
fn projects_are_found_by_name_and_a_name_two_files_give_is_an_error() {
    let d = dirs("find", false);
    let checkout = d.roots.checkout.clone().unwrap();
    write(
        &checkout.join("art/one/project.toml"),
        &project_text("one", ""),
    );
    local_project(&d, "two", "");
    let found = discover(&d.roots);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].origin, Origin::Git);
    assert_eq!(load(&d.roots, "two").unwrap().origin, Origin::Local);
    let e = load(&d.roots, "three").unwrap_err().to_string();
    assert!(e.contains("one, two"), "{e}");
    local_project(&d, "one", "");
    let e = load(&d.roots, "one").unwrap_err().to_string();
    assert!(e.contains("two project files are named \"one\""), "{e}");
    // A broken file at the name's place is that file's error.
    write(
        &d.roots.workspace().join("projects/four.toml"),
        "name = 4\n",
    );
    let e = load(&d.roots, "four").unwrap_err().to_string();
    assert!(e.contains("four.toml"), "{e}");
}

#[test]
fn the_stack_applies_listed_and_own_edit_files_over_the_tree() {
    let d = dirs("stack", false);
    let checkout = d.roots.checkout.clone().unwrap();
    write(&checkout.join("assets/tag-edits/tuning.yaml"), &set_a(2.0));
    local_project(&d, "p", "edits = [\"assets/tag-edits/tuning.yaml\"]\n");
    write(&d.roots.workspace().join("edits/p.yaml"), &set_a(3.0));
    let f = load(&d.roots, "p").unwrap();
    let (mut stack, layers) = open_stack(&d.roots, &f.project).unwrap();
    let kinds: Vec<(&str, &str)> = layers.iter().map(|l| (l.kind, l.name.as_str())).collect();
    assert_eq!(
        kinds,
        vec![
            ("edits", "assets/tag-edits/tuning.yaml"),
            ("edits", "p"),
            ("tree", BASE)
        ]
    );
    assert!(stack.check().is_empty());
    let a = stack.read(&jpt_key(A)).unwrap().unwrap();
    let def = Defs::builtin().group(FourCC(*b"jpt!")).unwrap();
    assert_eq!(
        decode(def, &a).unwrap().at("damage_lower_bound"),
        Some(&Val::Float(3.0))
    );
    assert_eq!(stack.overrides().len(), 1);
    assert_eq!(stack.read(&jpt_key(B)).unwrap().unwrap(), jpt(B, false));
}

#[test]
fn a_missing_tree_base_import_or_edit_file_is_a_named_error() {
    let d = dirs("missing", false);
    let mut p = Project::parse(&project_text("p", ""), "p").unwrap();
    let no_tree = Roots {
        tree: d.root.join("nowhere"),
        ..d.roots.clone()
    };
    assert!(matches!(
        open_stack(&no_tree, &p),
        Err(ProjectError::NoTree(_))
    ));
    p.base = "other".into();
    let e = open_stack(&d.roots, &p).err().unwrap().to_string();
    assert!(e.contains("has no map other"), "{e}");
    p.base = BASE.into();
    p.imports = vec!["community".into()];
    let e = open_stack(&d.roots, &p).err().unwrap().to_string();
    assert!(e.contains("import layer community is not at"), "{e}");
    p.imports.clear();
    p.edits = vec!["gone".into()];
    let e = open_stack(&d.roots, &p).err().unwrap().to_string();
    assert!(e.contains("edit file gone is not at"), "{e}");
}

#[test]
fn a_fresh_build_refuses_a_lossy_tag_by_name() {
    let d = dirs("lossy", true);
    local_project(&d, "p", "");
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[])).err().unwrap().to_string();
    assert!(e.contains("a fresh build refuses"), "{e}");
    assert!(e.contains(&format!("jpt!:{A}")), "{e}");
    assert!(e.contains("a field left empty"), "{e}");
    assert_eq!(lossy(&[jpt(A, true), jpt(B, false)]).len(), 1);
}

#[test]
fn an_edit_problem_stops_the_build() {
    let d = dirs("problem", false);
    local_project(&d, "p", "");
    write(
        &d.roots.workspace().join("edits/p.yaml"),
        "- tag: 'jpt!:invented\\dmg\\nowhere'\n  set:\n    - {path: damage_lower_bound, value: 1.0}\n",
    );
    let f = load(&d.roots, "p").unwrap();
    match build(&d.roots, &f, &tools(&[])) {
        Err(ProjectError::Edits(p)) => {
            assert_eq!((p[0].file.as_str(), p[0].line), ("p", 1));
        }
        other => panic!("{:?}", other.map(|b| b.receipt.key)),
    }
}

/// A kit that lays out the whole map itself, as the stage's does.
struct WholeMap;

impl Kit for WholeMap {
    fn name(&self) -> &str {
        "whole"
    }
    fn fingerprint(&self) -> String {
        "whole 1".into()
    }
    fn run(&self, step: &mut KitStep<'_>) -> Result<Installed, String> {
        step.set = None;
        step.bytes = Some(vec![0; 0x10]);
        Ok(Installed::default())
    }
}

/// A kit that needs a tag set to install into.
struct Adds;

impl Kit for Adds {
    fn name(&self) -> &str {
        "adds"
    }
    fn fingerprint(&self) -> String {
        "adds 1".into()
    }
    fn run(&self, step: &mut KitStep<'_>) -> Result<Installed, String> {
        step.set.as_ref().ok_or("no tag set")?;
        Ok(Installed::default())
    }
}

#[test]
fn kits_run_in_order_and_one_that_lays_out_the_map_must_be_last() {
    let d = dirs("kits", false);
    local_project(&d, "p", "kits = [\"missing\"]\n");
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[&WholeMap, &Adds]))
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains("kit \"missing\" is not one this build has (it has: whole, adds)"),
        "{e}"
    );

    local_project(&d, "p", "kits = [\"whole\", \"adds\"]\n");
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[&WholeMap, &Adds]))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("put whole last"), "{e}");

    // The whole map is read back: these bytes are not a map.
    local_project(&d, "p", "kits = [\"whole\"]\n");
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[&WholeMap]))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("does not read back"), "{e}");
}

#[test]
fn an_onto_build_needs_the_base_maps_skeleton_by_name() {
    let d = dirs("onto", false);
    write(
        &d.roots.workspace().join("projects/p.toml"),
        &format!("name = \"p\"\nmode = \"onto\"\nbase = \"{BASE}\"\noutput = \"package\"\n"),
    );
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[])).err().unwrap().to_string();
    assert!(e.contains("holds no skeleton of tiny"), "{e}");
}

#[test]
fn fresh_roots_take_kept_groups_and_prefixes_but_never_the_bases_level() {
    let globals = TagKey::new(FourCC(*b"matg"), "globals\\globals");
    let scenario = TagKey::new(FourCC(*b"scnr"), "ours");
    let m = Manifest {
        tags: vec![
            (TagKey::new(FourCC(*b"scnr"), "base"), None),
            (TagKey::new(FourCC(*b"ugh!"), "sounds"), None),
            (TagKey::new(FourCC(*b"shad"), "shaders\\default"), None),
            (jpt_key(A), None),
        ],
        ..Default::default()
    };
    const KEEP: Keep = Keep {
        groups: &[b"scnr", b"ugh!"],
        prefixes: &["shaders\\"],
    };
    let r = fresh_roots(&m, &globals, &scenario, KEEP);
    let paths: Vec<&str> = r.iter().map(|k| k.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["globals\\globals", "ours", "sounds", "shaders\\default"]
    );
}

#[test]
fn a_root_in_a_git_checkout_is_refused() {
    let d = dirs("checkout", false);
    let co = d.root.join("co");
    std::fs::create_dir_all(co.join(".git")).unwrap();
    assert!(git_checkout(&co.join("data")).is_some());
    let roots = Roots {
        data: co.join("data"),
        ..d.roots.clone()
    };
    let e = roots.refuse_checkout().unwrap_err().to_string();
    assert!(e.contains("inside the Git checkout"), "{e}");
    assert!(d.roots.refuse_checkout().is_ok());
}
