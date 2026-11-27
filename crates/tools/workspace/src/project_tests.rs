//! Projects over an invented tree (`project`, `authored`,
//! `project_build`): tags of invented names in a tag repository the test
//! writes, packages the test writes, no disc.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use map::FourCC;
use tags::defs::{Defs, Val, decode, default_record, encode};
use tags::repo::store::Manifest;
use tags::repo::{Body, LooseTag, Node, Region, TagKey, TagRepo, TagSource, Tree, Value};

use crate::authored::{COMPLETE_FILE, GENERATED_DIR, InMemory};
use crate::edits::{Edited, Origin};
use crate::layer::Layered;
use crate::maps::{BaseMaps, MapSpec, Mode, NoFresh, TREE_KEY, build_maps_held, held_in};
use crate::project::{
    EditRef, ProjectError, ProjectFile, Roots, discover, git_checkout, load, open_stack,
    wanted_layers,
};
use crate::project_build::{
    Built, Installed, Keep, Kit, KitStep, Receipt, Tools, build, fresh_roots, lossy, to_add,
    write as write_build,
};

const BASE: &str = "tiny";
const SCENARIO: &str = "invented\\scenarios\\tiny";
const A: &str = "invented\\dmg\\a";
const B: &str = "invented\\dmg\\b";
const C: &str = "invented\\dmg\\c";

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

fn lower_bound(t: &LooseTag) -> Option<Val> {
    let def = Defs::builtin().group(FourCC(*b"jpt!")).unwrap();
    decode(def, t).unwrap().at("damage_lower_bound").cloned()
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

fn spec_text(name: &str, extra: &str) -> String {
    format!(
        "mode = \"fresh\"\nbase = \"{BASE}\"\nname = \"{name}\"\nscenario = '{SCENARIO}'\n{extra}"
    )
}

/// A package `id` in `dir`: a `map` item (`map.toml`, `spec`) when given,
/// then `items` and the files they name.
fn package(dir: &Path, id: &str, spec: Option<&str>, items: &str, files: &[(&str, &str)]) {
    let mut text = format!("[package]\nformat = 1\nid = \"{id}\"\nname = \"{id}\"\n");
    if let Some(spec) = spec {
        text.push_str("[[item]]\nkind = \"map\"\nsource = \"map.toml\"\nmaps = \"all\"\n");
        write(&dir.join("map.toml"), spec);
    }
    text.push_str(items);
    write(&dir.join("package.toml"), &text);
    for (name, body) in files {
        write(&dir.join(name), body);
    }
}

/// A local package `id` whose map item is the fresh map `name` over
/// `tiny`, with the spec lines `extra`.
fn local_project(d: &Dirs, name: &str, extra: &str) {
    let dir = d.roots.data.join("packages").join(name);
    package(&dir, name, Some(&spec_text(name, extra)), "", &[]);
}

/// A `tag` item adding the damage tag `path` to map `map`: like A, with
/// its lower bound `value`.
fn tag_item(file: &str, path: &str, map: &str) -> String {
    format!("[[item]]\nkind = \"tag\"\nsource = \"{file}\"\ntag = '{path}'\nmaps = [\"{map}\"]\n")
}

fn like_a(value: f32) -> String {
    format!("like: 'jpt!:{A}'\nfields:\n  damage_lower_bound: {value:.1}\n")
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
fn the_stage_map_item_is_a_project_and_bad_specs_name_their_field() {
    let stage = include_bytes!("../../../../content/stage/map.toml");
    let p = MapSpec::parse(stage).unwrap();
    assert_eq!(
        (p.name.as_str(), p.mode, p.kits.as_slice()),
        ("stage", Mode::Fresh, &["stage".to_owned()][..])
    );
    let bad = |extra: &str, mode: &str| {
        let text = format!("name = \"x\"\nmode = \"{mode}\"\nbase = \"lockout\"\n{extra}");
        MapSpec::parse(text.as_bytes()).unwrap_err()
    };
    assert!(bad("kits = [\"k\"]\n", "fresh").contains("needs a scenario"));
    assert!(bad("scenario = 's'\n", "fresh").contains("needs kits"));
    assert!(bad("scenario = 's'\n", "onto").contains("for a fresh map"));
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
fn projects_are_map_items_found_by_name_and_a_name_two_items_give_is_an_error() {
    let d = dirs("find", false);
    let checkout = d.roots.checkout.clone().unwrap();
    let one = checkout.join("content/one");
    package(
        &one,
        "one",
        Some(&spec_text("one", "kits = [\"k\"]\n")),
        "",
        &[],
    );
    local_project(&d, "two", "kits = [\"k\"]\n");
    // The package cache is not a package of the set.
    package(
        &d.roots.data.join("packages/cache"),
        "cache",
        Some(&spec_text("two", "kits = [\"k\"]\n")),
        "",
        &[],
    );
    let found = discover(&d.roots);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].origin, Origin::Git);
    assert_eq!(found[0].item, Some(0));
    let two = load(&d.roots, "two").unwrap();
    assert_eq!((two.origin, two.package.as_str()), (Origin::Local, "two"));
    assert_eq!(two.packages.len(), 2);
    let by_dir = load(&d.roots, &one.display().to_string()).unwrap();
    assert_eq!(by_dir.project.name, "one");
    let e = load(&d.roots, "three").unwrap_err().to_string();
    assert!(e.contains("one, two"), "{e}");
    local_project(&d, "dup", "kits = [\"k\"]\n");
    write(
        &d.roots.data.join("packages/dup/map.toml"),
        &spec_text("one", "kits = [\"k\"]\n"),
    );
    let e = load(&d.roots, "one").unwrap_err().to_string();
    assert!(e.contains("two map items are named \"one\""), "{e}");
    // A package that does not load stops every tree build, named.
    write(
        &d.roots.data.join("packages/broken/package.toml"),
        "[package]\n",
    );
    let e = load(&d.roots, "two").unwrap_err().to_string();
    assert!(e.contains("broken") && e.contains("does not load"), "{e}");
}

#[test]
fn the_stack_applies_listed_and_own_edit_files_over_the_tree() {
    let d = dirs("stack", false);
    let checkout = d.roots.checkout.clone().unwrap();
    write(&checkout.join("assets/tag-edits/tuning.yaml"), &set_a(2.0));
    local_project(
        &d,
        "p",
        "kits = [\"k\"]\nedits = [\"assets/tag-edits/tuning.yaml\"]\n",
    );
    write(&d.roots.workspace().join("edits/p.yaml"), &set_a(3.0));
    let f = load(&d.roots, "p").unwrap();
    let mut o = open_stack(&d.roots, &f, false).unwrap();
    let kinds: Vec<(&str, &str)> = o.layers.iter().map(|l| (l.kind, l.name.as_str())).collect();
    assert_eq!(
        kinds,
        vec![
            ("edits", "assets/tag-edits/tuning.yaml"),
            ("edits", "p"),
            ("tree", BASE)
        ]
    );
    assert!(o.authored.is_none());
    assert!(o.stack.check().is_empty());
    let a = o.stack.read(&jpt_key(A)).unwrap().unwrap();
    assert_eq!(lower_bound(&a), Some(Val::Float(3.0)));
    assert_eq!(o.stack.overrides().len(), 1);
    assert_eq!(o.stack.read(&jpt_key(B)).unwrap().unwrap(), jpt(B, false));
}

fn file_of(d: &Dirs, spec: &str) -> ProjectFile {
    ProjectFile {
        project: MapSpec::parse(spec.as_bytes()).unwrap(),
        origin: Origin::Local,
        path: d.root.join("pkg"),
        package: "pkg".into(),
        item: 0,
        packages: Vec::new(),
    }
}

#[test]
fn a_missing_tree_base_import_or_edit_file_is_a_named_error() {
    let d = dirs("missing", false);
    let mut f = file_of(&d, &spec_text("p", "kits = [\"k\"]\n"));
    let no_tree = Roots {
        tree: d.root.join("nowhere"),
        ..d.roots.clone()
    };
    assert!(matches!(
        open_stack(&no_tree, &f, false),
        Err(ProjectError::NoTree(_))
    ));
    f.project.base = "other".into();
    let e = open_stack(&d.roots, &f, false).err().unwrap().to_string();
    assert!(e.contains("has no map other"), "{e}");
    f.project.base = BASE.into();
    f.project.imports = vec!["community".into()];
    let e = open_stack(&d.roots, &f, false).err().unwrap().to_string();
    assert!(e.contains("import layer community is not at"), "{e}");
    f.project.imports.clear();
    f.project.edits = vec!["gone".into()];
    let e = open_stack(&d.roots, &f, false).err().unwrap().to_string();
    assert!(e.contains("edit file gone is not at"), "{e}");
}

#[test]
fn the_tag_items_a_map_takes_are_the_authored_layer_generated_once() {
    let d = dirs("authored", false);
    let dir = d.roots.data.join("packages/p");
    let item = tag_item("tags/c.jpt!.yaml", C, "p");
    package(
        &dir,
        "p",
        Some(&spec_text("p", "kits = [\"k\"]\n")),
        &item,
        &[("tags/c.jpt!.yaml", &like_a(7.0))],
    );
    // An item for another map is not this map's.
    let other = d.roots.data.join("packages/q");
    let q_item = tag_item("tags/d.jpt!.yaml", "invented\\dmg\\d", "elsewhere");
    package(
        &other,
        "q",
        None,
        &q_item,
        &[("tags/d.jpt!.yaml", &like_a(1.0))],
    );
    write(&d.roots.workspace().join("edits/p.yaml"), &set_a(3.0));
    let f = load(&d.roots, "p").unwrap();

    let mut o = open_stack(&d.roots, &f, true).unwrap();
    let kinds: Vec<&str> = o.layers.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, vec!["edits", "authored", "tree"]);
    let (fp, keys) = o.authored.clone().unwrap();
    assert_eq!(keys, vec![jpt_key(C)]);
    assert_eq!(o.layers[1].name, fp);
    let c = o.stack.read(&jpt_key(C)).unwrap().unwrap();
    assert_eq!(lower_bound(&c), Some(Val::Float(7.0)));
    assert!(
        o.stack
            .read(&jpt_key("invented\\dmg\\d"))
            .unwrap()
            .is_none()
    );
    let kept = d.roots.workspace().join(GENERATED_DIR).join(&fp);
    assert!(kept.join(COMPLETE_FILE).is_file());

    // The same sources make the same layer; held in memory, nothing is written.
    let again = open_stack(&d.roots, &f, false).unwrap();
    assert_eq!(again.authored.unwrap().0, fp);
    assert!(again.layers[1].path.is_empty());
    assert_eq!(
        wanted_layers(&d.roots).unwrap(),
        BTreeSet::from([fp.clone()])
    );

    // A changed source is another layer.
    write(&dir.join("tags/c.jpt!.yaml"), &like_a(8.0));
    let f = load(&d.roots, "p").unwrap();
    let changed = open_stack(&d.roots, &f, false).unwrap();
    assert_ne!(changed.authored.unwrap().0, fp);
}

#[test]
fn an_authored_tag_the_tree_holds_or_two_items_add_is_a_named_error() {
    let d = dirs("authored-bad", false);
    let dir = d.roots.data.join("packages/p");
    let items = tag_item("tags/b.jpt!.yaml", B, "p");
    package(
        &dir,
        "p",
        Some(&spec_text("p", "kits = [\"k\"]\n")),
        &items,
        &[("tags/b.jpt!.yaml", &like_a(1.0))],
    );
    let f = load(&d.roots, "p").unwrap();
    let e = open_stack(&d.roots, &f, false).err().unwrap().to_string();
    assert!(e.contains("which a lower layer holds"), "{e}");
    assert!(e.contains("package p item 1"), "{e}");

    let items = format!(
        "{}{}",
        tag_item("tags/c.jpt!.yaml", C, "p"),
        tag_item("tags/c2.jpt!.yaml", C, "p")
    );
    package(
        &dir,
        "p",
        Some(&spec_text("p", "kits = [\"k\"]\n")),
        &items,
        &[
            ("tags/c.jpt!.yaml", &like_a(1.0)),
            ("tags/c2.jpt!.yaml", &like_a(2.0)),
        ],
    );
    let f = load(&d.roots, "p").unwrap();
    let e = open_stack(&d.roots, &f, false).err().unwrap().to_string();
    assert!(e.contains("both add"), "{e}");
    assert!(wanted_layers(&d.roots).is_err());
}

#[test]
fn onto_adds_the_changed_tags_the_base_lacks_and_what_they_reach() {
    let d = dirs("onto-add", false);
    let new_d = TagKey::new(FourCC(*b"ugh!"), "invented\\new\\d");
    let new_e = TagKey::new(FourCC(*b"ugh!"), "invented\\new\\e");
    let ours: BTreeMap<TagKey, LooseTag> = [
        plain(new_d.clone(), &[new_e.clone(), jpt_key(A)]),
        plain(new_e.clone(), &[]),
    ]
    .into_iter()
    .map(|t| (t.key.clone(), t))
    .collect();
    let tree = TagRepo::for_map(&d.roots.tree, BASE).unwrap();
    let mut stack =
        Edited::new(Layered::new().with(InMemory(ours)).with(tree), Vec::new()).unwrap();
    let base: BTreeSet<TagKey> = [jpt_key(A), jpt_key(B)].into_iter().collect();
    let changed: BTreeSet<TagKey> = [new_d.clone(), jpt_key(B)].into_iter().collect();
    let add = to_add(&mut stack, &base, &changed).unwrap();
    assert_eq!(add, vec![new_d.clone(), new_e]);

    // A reached tag no layer holds is named.
    let gone = TagKey::new(FourCC(*b"ugh!"), "invented\\new\\gone");
    let ours: BTreeMap<TagKey, LooseTag> = [plain(new_d.clone(), &[gone])]
        .into_iter()
        .map(|t| (t.key.clone(), t))
        .collect();
    let mut stack = Edited::new(Layered::new().with(InMemory(ours)), Vec::new()).unwrap();
    let e = to_add(&mut stack, &base, &[new_d].into_iter().collect())
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("invented\\new\\gone"), "{e}");
}

#[test]
fn a_fresh_build_refuses_a_lossy_tag_by_name() {
    let d = dirs("lossy", true);
    local_project(&d, "p", "kits = [\"adds\"]\n");
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[&Adds]))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("a fresh build refuses"), "{e}");
    assert!(e.contains(&format!("jpt!:{A}")), "{e}");
    assert!(e.contains("a field left empty"), "{e}");
    assert_eq!(lossy(&[jpt(A, true), jpt(B, false)]).len(), 1);
}

#[test]
fn an_edit_problem_stops_the_build() {
    let d = dirs("problem", false);
    local_project(&d, "p", "kits = [\"adds\"]\n");
    write(
        &d.roots.workspace().join("edits/p.yaml"),
        "- tag: 'jpt!:invented\\dmg\\nowhere'\n  set:\n    - {path: damage_lower_bound, value: 1.0}\n",
    );
    let f = load(&d.roots, "p").unwrap();
    match build(&d.roots, &f, &tools(&[&Adds])) {
        Err(ProjectError::Edits(p)) => {
            assert_eq!((p[0].file.as_str(), p[0].line), ("p", 1));
        }
        other => panic!("{:?}", other.map(|b| b.receipt.key)),
    }
}

#[test]
fn a_map_with_ground_does_not_build_from_the_tree() {
    let d = dirs("ground", false);
    local_project(&d, "p", "kits = [\"adds\"]\nground = \"ground.yaml\"\n");
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[&Adds]))
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("ground file (ground.yaml)"), "{e}");
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
    let dir = d.roots.data.join("packages/p");
    package(
        &dir,
        "p",
        Some(&format!(
            "name = \"p\"\nmode = \"onto\"\nbase = \"{BASE}\"\n"
        )),
        "",
        &[],
    );
    let f = load(&d.roots, "p").unwrap();
    let e = build(&d.roots, &f, &tools(&[])).err().unwrap().to_string();
    assert!(e.contains("holds no skeleton of tiny"), "{e}");
}

struct NoDisc;

impl BaseMaps for NoDisc {
    fn read(&self, _: &str) -> Result<Vec<u8>, String> {
        Err("no disc".into())
    }
}

#[test]
fn a_tree_build_is_the_map_package_prepare_keeps_for_a_map_naming_edits() {
    let d = dirs("written", false);
    local_project(&d, "p", "kits = [\"adds\"]\nedits = [\"tuning\"]\n");
    let f = load(&d.roots, "p").unwrap();
    // Not built from the tree yet: prepare names the tag workspace.
    let maps = d.roots.maps();
    let held = |dir: &str, p: &str, i: usize, k: &str| held_in(&maps, dir, p, i, k);
    let before = build_maps_held(&f.packages, &NoDisc, &NoFresh, &held);
    assert!(
        before.failed[0]
            .why
            .contains("builds with the tag workspace")
    );

    let receipt = Receipt {
        key: "k".into(),
        project: "p".into(),
        package: f.package.clone(),
        item: f.item,
        mode: Mode::Fresh,
        base: BASE.into(),
        layers: Vec::new(),
        authored: Vec::new(),
        edit_files: Vec::new(),
        edited: Vec::new(),
        overrides: Vec::new(),
        kits: Vec::new(),
        closure: 0,
        rebuilt: Vec::new(),
        added: Vec::new(),
        taken: Vec::new(),
        file_size: 4,
        memory_size: 0,
        memory_max: 0,
    };
    let built = Built {
        project: f.project.clone(),
        bytes: vec![1, 2, 3, 4],
        receipt,
    };
    let wrote = write_build(&d.roots, &built).unwrap();
    assert_eq!(wrote.file, "p.map");
    assert!(!wrote.unchanged);
    assert!(write_build(&d.roots, &built).unwrap().unchanged);
    assert!(held_in(&maps, "p", "p", 0, TREE_KEY).is_some());
    let after = build_maps_held(&f.packages, &NoDisc, &NoFresh, &held);
    assert!(after.failed.is_empty(), "{:?}", after.failed);
    assert_eq!(after.held.len(), 1);
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
