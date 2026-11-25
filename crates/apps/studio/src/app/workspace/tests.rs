//! The workspace's state over invented maps: the start page, extraction
//! into a fresh data root, the background plan of a later start, the
//! sizes and prune. Every map is built by `map::build::TagSet` with
//! zeroed payloads sized for the checked-in schema, and every raw is
//! named by a tag file built here: no byte comes from a game file.
use std::path::PathBuf;

use map::FourCC;
use map::build::{Block, TagPayload, TagSet};
use tags::repo::format;
use tags::repo::{Body, LooseTag, Node, Origin, RawRes, Region, TagKey, Tree};

use super::*;

/// A temporary directory, removed on drop.
struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-ws-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An invented map of four tags (a globals, a scenario, a weapon and a
/// collection), its payloads zero-filled to the checked-in schema's root
/// sizes. The tag paths are fixed; `name` goes into the header, so two
/// maps can differ in identity (their file hash) with the same tags.
pub(crate) fn invented_map(name: &str) -> Vec<u8> {
    let (matg, scnr, weap, itmc) = (
        FourCC(*b"matg"),
        FourCC(*b"scnr"),
        FourCC(*b"weap"),
        FourCC(*b"itmc"),
    );
    let mut set = TagSet::new(map::CacheType::Multiplayer, name, "test");
    for g in [matg, scnr, weap, itmc] {
        set.add_group(g, None, None);
    }
    set.globals = set
        .add_tag(matg, "globals", TagPayload::Built(Block::zeroed(0x284)))
        .unwrap();
    set.scenario = set
        .add_tag(
            scnr,
            "levels\\test\\example",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    set.add_tag(
        weap,
        "objects\\test\\example",
        TagPayload::Built(Block::zeroed(0x31c)),
    )
    .unwrap();
    set.add_tag(
        itmc,
        "objects\\test\\example_kit",
        TagPayload::Built(Block::zeroed(0xc)),
    )
    .unwrap();
    set.build().unwrap().0
}

/// An invented disc: a folder of one map.
fn invented_disc(t: &Temp) -> PathBuf {
    let disc = t.path("disc");
    std::fs::create_dir_all(&disc).unwrap();
    std::fs::write(disc.join("example.map"), invented_map("example")).unwrap();
    disc
}

fn fresh_app(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    app
}

fn extract(app: &mut App, disc: &Path) -> Extracting {
    app.workspace_extract(Some(disc.to_path_buf()), None, false, None)
        .unwrap_or_else(|e| panic!("extract: {e}"))
}

#[test]
fn the_start_page_shows_until_the_tree_is_extracted() {
    let t = Temp::new("start");
    let mut app = fresh_app(&t);
    assert!(app.workspace_start_page());
    extract(&mut app, &invented_disc(&t));
    app.wait_workspace();
    assert!(!app.workspace_start_page());
    // A source open works without a tree: the page does not nag.
    let t2 = Temp::new("start-source");
    let mut with_source = fresh_app(&t2);
    std::fs::write(t2.path("loose.map"), scenario_edit::fixture::fixture()).unwrap();
    with_source.dispatch(crate::app::Action::OpenSource(t2.path("loose.map")));
    assert!(!with_source.workspace_start_page());
}

#[test]
fn extraction_records_the_tree_and_an_unchanged_disc_is_a_no_op() {
    let t = Temp::new("extract");
    let mut app = fresh_app(&t);
    let disc = invented_disc(&t);
    let started = extract(&mut app, &disc);
    assert_eq!(started.maps, ["example"]);
    assert_eq!(started.root, app.tags_root());
    app.wait_workspace();
    let status = app.workspace_status();
    assert_eq!(status["has_tree"], json!(true));
    assert_eq!(status["tree"]["maps"], json!(1));
    assert_eq!(status["tree"]["tags"], json!(4));
    assert_eq!(status["tree"]["refused"], json!(0));
    assert_eq!(status["tree"]["lossy"], json!(0));
    assert_eq!(status["last"]["maps"], json!(1));
    assert_eq!(status["last"]["tags_new"], json!(4));
    assert_eq!(status["stale"]["maps"], json!([]));
    assert_eq!(status["extracting"], serde_json::Value::Null);
    assert!(status["sizes"]["tree"].as_u64().unwrap() > 0);
    // The same disc again: the plan is empty and nothing is extracted.
    extract(&mut app, &disc);
    app.wait_workspace();
    let again = app.workspace_status();
    assert_eq!(again["last"]["maps"], json!(0));
    assert_eq!(again["last"]["tags_new"], json!(0));
}

#[test]
fn a_changed_disc_file_is_extracted_again_by_the_background_plan() {
    let t = Temp::new("update");
    let mut app = fresh_app(&t);
    let disc = invented_disc(&t);
    extract(&mut app, &disc);
    app.wait_workspace();
    // The disc's map file moves (its identity changes; its tags are the
    // same).
    std::fs::write(disc.join("example.map"), invented_map("example changed")).unwrap();
    // A later start runs the plan against the recorded disc.
    app.start_workspace();
    app.wait_workspace();
    let status = app.workspace_status();
    assert_eq!(status["last"]["maps"], json!(1));
    // The whole map is taken again, its four tags the same as before.
    assert_eq!(status["last"]["tags_new"], json!(0), "{status}");
    assert_eq!(status["last"]["tags_same"], json!(4), "{status}");
    assert_eq!(status["last"]["variants"], json!(0), "{status}");
    // A start with no tree does nothing (the start page shows).
    let t2 = Temp::new("update-bare");
    let mut bare = fresh_app(&t2);
    bare.start_workspace();
    assert!(bare.workspace.job.is_none());
}

#[test]
fn a_root_inside_a_git_checkout_is_refused_and_not_recorded() {
    let t = Temp::new("checkout");
    let mut app = fresh_app(&t);
    let disc = invented_disc(&t);
    std::fs::create_dir_all(t.path("checkout/.git")).unwrap();
    let root = t.path("checkout/tags");
    match app.workspace_extract(Some(disc), None, false, Some(root.clone())) {
        Err(ExtractRefusal::Checkout { root: r, repo }) => {
            assert_eq!(r, root);
            assert!(repo.ends_with("checkout"));
        }
        other => panic!("not refused: {other:?}"),
    }
    assert_eq!(app.tags_root(), t.path("data").join("tags"));
    assert!(app.workspace.job.is_none());
}

#[test]
fn an_unknown_map_name_and_a_missing_disc_are_refused() {
    let t = Temp::new("params");
    let mut app = fresh_app(&t);
    let disc = invented_disc(&t);
    match app.workspace_extract(Some(disc), Some(vec!["nope".into()]), false, None) {
        Err(ExtractRefusal::Params(m)) => assert!(m.contains("nope.map")),
        other => panic!("not refused: {other:?}"),
    }
    let missing = t.path("nowhere");
    match app.workspace_extract(Some(missing), None, false, None) {
        Err(ExtractRefusal::Params(m)) => assert!(m.contains("nowhere")),
        other => panic!("not refused: {other:?}"),
    }
    assert!(app.workspace.job.is_none());
}

#[test]
fn sizes_walk_the_tree_the_layers_the_caches_and_the_packages() {
    let t = Temp::new("sizes");
    let mut app = fresh_app(&t);
    extract(&mut app, &invented_disc(&t));
    app.wait_workspace();
    let write = |dir: &str, bytes: usize| {
        let d = t.path("data").join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("f"), vec![0u8; bytes]).unwrap();
    };
    write("workspace/generated/f1", 100);
    write("workspace/imports/m1", 200);
    write("workspace/edits/e1", 50);
    write("workspace/build/p", 300);
    write("maps/pkg", 400);
    let status = app.workspace_status();
    let sizes = &status["sizes"];
    let tree = sizes["tree"].as_u64().unwrap();
    assert!(tree > 0);
    assert_eq!(
        sizes["layers"],
        json!({"generated": {"f1": 100}, "imports": {"m1": 200}, "edits": {"e1": 50}})
    );
    assert_eq!(sizes["projects"], json!({"p": 300}));
    assert_eq!(sizes["outputs"], json!({"pkg": 400}));
    assert_eq!(sizes["total"].as_u64().unwrap(), tree + 1050);
}

#[test]
fn prune_removes_unnamed_raws_unused_caches_and_unasked_for_layers() {
    let t = Temp::new("prune");
    let mut app = fresh_app(&t);
    let root = app.tags_root();
    TagRepo::create(&root).unwrap();
    // A tag file that names a raw: the resource stays.
    let raw = RawRes {
        origin: Origin::Local,
        len: 4,
        hash: [7; 32],
    };
    let named = raw.hex();
    let tag = LooseTag {
        key: TagKey::new(FourCC(*b"bitm"), "a\\named"),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![Node {
                bytes: vec![0; 4],
                count: 1,
                size: 4,
                align: 4,
                fields: Vec::new(),
            }],
            raws: vec![raw],
            losses: Vec::new(),
        }),
    };
    std::fs::create_dir_all(root.join("tags/a")).unwrap();
    std::fs::write(root.join("tags/a/named.bitm"), format::encode(&tag)).unwrap();
    std::fs::create_dir_all(root.join(format!("raw/{}", &named[..2]))).unwrap();
    std::fs::write(
        root.join(format!("raw/{}/{}", &named[..2], named)),
        [1, 2, 3, 4],
    )
    .unwrap();
    // A raw nothing names, and a manifest that names another.
    let orphan = tags::repo::hex(&[9; 32]);
    std::fs::create_dir_all(root.join(format!("raw/{}", &orphan[..2]))).unwrap();
    std::fs::write(root.join(format!("raw/{}/{}", &orphan[..2], orphan)), [5]).unwrap();
    let low = tags::repo::hex(&[3; 32]);
    std::fs::create_dir_all(root.join(format!("raw/{}", &low[..2]))).unwrap();
    std::fs::write(root.join(format!("raw/{}/{}", &low[..2], low)), [6, 7]).unwrap();
    let manifest = tags::repo::store::Manifest {
        map: "example".into(),
        low_detail: Some(low.clone()),
        ..Default::default()
    };
    std::fs::write(root.join("maps/example.txt"), manifest.to_text()).unwrap();
    // A build cache unused for ten days, one used now, and a generated
    // layer (no project asks for any).
    let old = t.path("data/workspace/build/old");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("cache"), [0; 10]).unwrap();
    let stamp = std::fs::File::options()
        .write(true)
        .open(old.join("cache"))
        .unwrap();
    stamp
        .set_modified(SystemTime::now() - Duration::from_secs(10 * 24 * 60 * 60))
        .unwrap();
    drop(stamp);
    let fresh = t.path("data/workspace/build/fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    std::fs::write(fresh.join("cache"), [0; 10]).unwrap();
    let layer = t.path("data/workspace/generated/f1");
    std::fs::create_dir_all(&layer).unwrap();
    std::fs::write(layer.join("tag"), [0; 20]).unwrap();

    // A dry run reports and removes nothing.
    let report = app.workspace_prune(true).unwrap();
    assert_eq!(report["dry_run"], json!(true));
    assert_eq!(report["raw"], json!([orphan]));
    assert_eq!(report["generated_removed"], json!(["f1"]));
    let days = report["caches_removed"][0]["unused_days"].as_u64().unwrap();
    assert!((9..=10).contains(&days), "{days}");
    assert_eq!(report["outputs_removed"], json!([]));
    assert_eq!(report["bytes"], json!(31));
    assert!(
        root.join(format!("raw/{}/{}", &orphan[..2], orphan))
            .exists()
    );
    assert!(layer.exists());

    // The real run removes them and keeps what is named or in use.
    let report = app.workspace_prune(false).unwrap();
    assert_eq!(report["raw_removed"], json!(1));
    assert!(
        !root
            .join(format!("raw/{}/{}", &orphan[..2], orphan))
            .exists()
    );
    assert!(root.join(format!("raw/{}/{}", &named[..2], named)).exists());
    assert!(root.join(format!("raw/{}/{}", &low[..2], low)).exists());
    assert!(!layer.exists());
    assert!(!old.exists());
    assert!(fresh.exists());
    // The named raw stays across another prune.
    assert_eq!(app.workspace_prune(false).unwrap()["raw_removed"], json!(0));
    // With a project file, generated layers are kept: which ones it asks
    // for is the project's (W2's projects).
    std::fs::create_dir_all(t.path("data/workspace/projects")).unwrap();
    std::fs::write(
        t.path("data/workspace/projects/stage.toml"),
        "name = \"stage\"\n",
    )
    .unwrap();
    let again = t.path("data/workspace/generated/f2");
    std::fs::create_dir_all(&again).unwrap();
    std::fs::write(again.join("tag"), [0; 5]).unwrap();
    let report = app.workspace_prune(false).unwrap();
    assert_eq!(report["generated_removed"], json!([]));
    assert_eq!(report["generated_kept"], json!(["f2"]));
    assert!(again.exists());
}

#[test]
fn the_settings_round_trip_through_the_ini() {
    let ws = Workspace {
        root: Some(PathBuf::from("/builds/tags")),
        disc: Some(PathBuf::from("/disc/halo2.iso")),
        ..Default::default()
    };
    let ini = ws.ini();
    let back = Workspace::from_ini(&ini);
    assert_eq!(back.root, ws.root);
    assert_eq!(back.disc, ws.disc);
    assert_eq!(back.ini(), ini);
    // Defaults where the section has none.
    assert_eq!(Workspace::from_ini("[Window][Tags]\nPos=0,0\n").root, None);
}

#[test]
fn the_status_reports_the_extraction_while_it_runs() {
    let t = Temp::new("status");
    let mut app = fresh_app(&t);
    let disc = invented_disc(&t);
    extract(&mut app, &disc);
    // Before it finishes: the extraction is in progress, with the disc
    // it reads.
    let status = app.workspace_status();
    let running = &status["extracting"];
    assert_eq!(running["disc"], json!(disc.display().to_string()));
    app.wait_workspace();
    assert_eq!(
        app.workspace_status()["extracting"],
        serde_json::Value::Null
    );
}
