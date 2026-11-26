//! The lobby's pick over a fake library and a temporary store
//! (docs/game/forge-map-variants.md, slice V2).
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

use forge_doc::{BlockKind, BuilderVersions, Edit, MapRef, Op, OpId, Placement};

use super::*;


struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "h2-lobby-variant-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
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

/// A library of one machine: the installed map's hash, the packages built
/// (their receipts), and what the next build answers.
#[derive(Default)]
struct Fake {
    installed: Option<Sha256>,
    receipts: Vec<(MapId, PathBuf)>,
    builds: Arc<AtomicUsize>,
    fail: Option<Refusal>,
}

impl Maps for Fake {
    fn installed(&self, map: &str) -> Option<Sha256> {
        (map == "fixture").then_some(self.installed).flatten()
    }
    fn cached(&self, id: &MapId) -> Option<PathBuf> {
        self.receipts
            .iter()
            .find(|(r, _)| r == id)
            .map(|(_, dir)| dir.clone())
    }
    fn build(&mut self, doc: &ForgeDoc) -> Result<PathBuf, Refusal> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        if let Some(r) = self.fail.clone() {
            return Err(r);
        }
        let id = MapId::forge(doc.base.sha256, doc.id().unwrap());
        let dir = PathBuf::from(format!("maps/forge-{}", &id.to_string()[..16]));
        self.receipts.push((id, dir.clone()));
        Ok(dir)
    }
}

fn library(fake: Fake) -> SharedMaps {
    Arc::new(Mutex::new(Box::new(fake)))
}

fn store(title: &str) -> (Dir, String) {
    let dir = Dir::new();
    let file = variants::save(&dir.0, &doc(title), false).unwrap();
    (dir, file)
}

fn installed() -> Fake {
    Fake {
        installed: Some(hash()),
        ..Fake::default()
    }
}

#[test]
fn a_pick_builds_once_and_the_next_reuses_the_receipt() {
    clear();
    let (dir, file) = store("Castle");
    let fake = installed();
    let builds = fake.builds.clone();
    let maps = library(fake);
    let worker = pick(&dir.0, "fixture", Some(7), &file, &maps).expect("a build starts");
    worker.join().unwrap();
    let id = crate::lobby_start::map_variant().expect("the lobby holds the id");
    let (made, built) = package().expect("built");
    assert_eq!(made, id);
    assert!(built.to_string_lossy().starts_with("maps/forge-"));
    assert_eq!(hold(), None);
    // The same pick again, and after a map change and back, build nothing.
    assert!(pick(&dir.0, "fixture", Some(7), &file, &maps).is_none());
    assert_eq!(package(), Some((id, built.clone())));
    sync(Some(8));
    assert!(pick(&dir.0, "fixture", Some(7), &file, &maps).is_none());
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[test]
fn a_build_holds_the_start_until_it_is_done() {
    clear();
    let (dir, file) = store("Castle");
    let (release, wait) = std::sync::mpsc::channel::<()>();
    struct Slow(std::sync::mpsc::Receiver<()>);
    impl Maps for Slow {
        fn installed(&self, _: &str) -> Option<Sha256> {
            Some(hash())
        }
        fn cached(&self, _: &MapId) -> Option<PathBuf> {
            None
        }
        fn build(&mut self, _: &ForgeDoc) -> Result<PathBuf, Refusal> {
            self.0.recv().unwrap();
            Ok("built".into())
        }
    }
    let maps: SharedMaps = Arc::new(Mutex::new(Box::new(Slow(wait))));
    let worker = pick(&dir.0, "fixture", None, &file, &maps).unwrap();
    assert_eq!(hold().as_deref(), Some("Building Castle\u{2026}"));
    assert_eq!(package(), None);
    release.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(hold(), None);
}

#[test]
fn each_refusal_gives_its_sentence_and_holds_the_start() {
    let (dir, file) = store("Castle");
    // The base map of another hash, and no such map installed.
    let cases = [
        (
            Fake {
                installed: Some(Sha256::of(b"another version")),
                ..Fake::default()
            },
            "variant_base_hash",
            "Castle was made for another version of fixture.",
        ),
        (
            Fake::default(),
            "variant_base_missing",
            "Castle needs fixture, which is not installed.",
        ),
    ];
    for (fake, code, sentence) in cases {
        clear();
        assert!(pick(&dir.0, "fixture", None, &file, &library(fake)).is_none());
        let r = refusal().expect(code);
        assert_eq!((r.code, r.sentence.as_str()), (code, sentence));
        assert_eq!(hold().as_deref(), Some(sentence));
        assert_eq!(package(), None);
    }
    // A file that is not there is `variant_missing`.
    clear();
    pick(&dir.0, "fixture", None, "gone", &library(installed()));
    assert_eq!(refusal().unwrap().code, "variant_missing");
    // A build that the author's map id disagrees with is `variant_expect`.
    clear();
    let expect = Refusal::new(
        "variant_expect",
        "Castle did not build to the map its author saved.",
    );
    let worker = pick(
        &dir.0,
        "fixture",
        None,
        &file,
        &library(Fake {
            fail: Some(expect.clone()),
            ..installed()
        }),
    )
    .unwrap();
    worker.join().unwrap();
    assert_eq!(refusal(), Some(expect));
    assert!(hold().unwrap().ends_with("saved."));
    assert_eq!(package(), None);
}

#[test]
fn a_map_change_or_leaving_clears_the_pick() {
    clear();
    let (dir, file) = store("Castle");
    let maps = library(installed());
    pick(&dir.0, "fixture", Some(7), &file, &maps).unwrap().join().unwrap();
    sync(Some(7));
    assert!(picked().is_some(), "the same map keeps it");
    sync(Some(9));
    assert_eq!(picked(), None);
    assert_eq!(crate::lobby_start::map_variant(), None);
    pick(&dir.0, "fixture", Some(9), &file, &maps);
    clear();
    assert_eq!((picked(), hold(), package()), (None, None, None));
}

#[test]
fn a_pick_with_no_builder_is_refused_by_name() {
    clear();
    pick_installed(Path::new("none"), "fixture", None, "Castle");
    let r = refusal().unwrap();
    assert_eq!(r.code, "variant_build");
    assert_eq!(hold().as_deref(), Some("Castle cannot be built in this game."));
}

#[test]
fn a_members_refusal_names_whose_it_is() {
    let inner = Refusal::new("variant_base_missing", "Castle needs fixture, which is not installed.");
    let r = Refusal::member("Gamer", &inner);
    assert_eq!(r.code, "variant_member");
    assert_eq!(r.sentence, "Gamer: Castle needs fixture, which is not installed.");
}

/// The empty draft of `fixture` the kit start builds, in its session form.
fn kit_draft() -> ForgeDoc {
    let mut draft = doc("fixture");
    draft.ops.clear();
    draft.author = "Builder".into();
    draft.format = draft.required_format();
    forge_doc::session_form(&draft)
}

#[test]
fn a_forge_start_builds_the_new_map_with_the_kit_and_resolves_to_its_package() {
    clear();
    let fake = installed();
    let builds = fake.builds.clone();
    let maps = library(fake);
    let worker = pick_kit("fixture", Some(7), "Builder", &maps).expect("a build starts");
    worker.join().unwrap();

    let draft = kit_draft();
    assert!(draft.recipes.contains(&forge_doc::kit()));
    let id = MapId::forge(hash(), draft.id().unwrap());
    assert_eq!(kit_start(), Some(id));
    assert_eq!(crate::lobby_start::map_variant(), Some(id));
    let (made, dir) = package().expect("built");
    assert_eq!(made, id);
    assert!(dir.to_string_lossy().starts_with("maps/forge-"));
    assert_eq!(hold(), None);

    // The same start again builds nothing.
    assert!(pick_kit("fixture", Some(7), "Builder", &maps).is_none());
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    // A variant of the store is not a kit start.
    clear();
    assert_eq!(kit_start(), None);
}

#[test]
fn a_forge_start_on_a_map_this_machine_lacks_is_refused_by_name() {
    clear();
    assert!(pick_kit("fixture", None, "Builder", &library(Fake::default())).is_none());
    assert_eq!(refusal().unwrap().code, "variant_base_missing");
    assert!(hold().unwrap().contains("fixture is not installed"));
    assert_eq!(kit_start(), None);
    assert_eq!(package(), None);
}

#[test]
fn the_lobbys_mode_keeps_the_kit_start_or_drops_it() {
    clear();
    let maps = library(installed());
    pick_kit("fixture", Some(7), "Builder", &maps)
        .unwrap()
        .join()
        .unwrap();
    let held = kit_start();
    assert!(held.is_some());
    let built = package().expect("the kit package is ready");
    // Forge mode on the same map keeps it, and asks nothing of the author.
    follow_mode(true, Some(7), Some("fixture"), || panic!("no new pick"));
    assert_eq!(kit_start(), held);
    // An installed offer opens in its own cache slot, with no disc kit.
    follow_mode(true, Some(8), None, || panic!("offer keeps its own slot"));
    assert_eq!(package(), None);
    assert_eq!(picked(), None);
    assert!(
        pick_kit("fixture", Some(7), "Builder", &maps).is_none(),
        "returning to the disc map reuses the cached kit"
    );
    assert_eq!(kit_start(), held);
    assert_eq!(package(), Some(built));
    // Leaving Forge mode drops it.
    follow_mode(false, Some(7), Some("fixture"), || panic!("no new pick"));
    assert_eq!(kit_start(), None);
    assert_eq!(picked(), None);
    // A game with no builder has no kit start: nothing is picked.
    follow_mode(true, Some(7), Some("fixture"), || "Builder".into());
    assert_eq!(picked(), None);
}

fn ground_store(title: &str) -> (Dir, String) {
    let mut d = doc(title);
    d.recipes.push(
        forge_doc::GroundRecipe {
            kit_version: 3,
            home: 0,
            probe: false,
        }
        .to_recipe(),
    );
    d.format = d.required_format();
    let dir = Dir::new();
    let file = variants::save(&dir.0, &d, false).unwrap();
    (dir, file)
}

#[test]
fn a_pick_keeps_whether_its_document_has_ground() {
    clear();
    let (dir, file) = ground_store("Basin");
    let maps = library(installed());
    pick(&dir.0, "fixture", Some(7), &file, &maps).unwrap().join().unwrap();
    assert!(picked().unwrap().ground);
    assert!(ground_on(Some(7)));
    assert!(!ground_on(Some(8)), "another lobby map");
    let (plain, file) = store("Castle");
    pick(&plain.0, "fixture", Some(7), &file, &maps).unwrap().join().unwrap();
    assert!(!picked().unwrap().ground);
    assert!(!ground_on(Some(7)));
}

#[test]
fn clearing_the_pick_refuses_the_start_again() {
    use crate::lobby_start::KitMaps;
    use crate::map_select::{Catalogue, Map};
    clear();
    let cat = Catalogue::new([Map {
        id: 7,
        name: "Fixture".into(),
        sort: 0,
        path: r"d:\maps\fixture".into(),
    }]);
    let (dir, file) = ground_store("Basin");
    let maps = library(installed());
    pick(&dir.0, "fixture", Some(7), &file, &maps).unwrap().join().unwrap();
    let lobby = || KitMaps::for_pick(Some(&cat), Some(7), None, ground_on(Some(7))).lobby;
    assert!(lobby(), "the pick makes the map a kit map");
    clear();
    assert!(!lobby(), "no pick, no ground");
    pick(&dir.0, "fixture", Some(7), &file, &maps);
    assert!(lobby());
    sync(Some(8));
    assert!(!lobby(), "a change of map drops the pick");
}
