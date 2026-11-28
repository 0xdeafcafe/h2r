//! PK4 (docs/editor/studio-redesign.md): the `pogo` package, `content/pogo`,
//! on `content_kit::fixture`'s invented multiplayer map. The package builds the
//! tags `content_kit::kit::install_all` builds (what `inject::add_pogo` is
//! made of), group by group; the `model`, `sound` and `data` builders refuse
//! what they cannot build by name. No disc bytes.
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use content::package::MapScope;
use map::build::TagSet;
use map::{FourCC, Map};
use content_kit::Style;
use content_kit::fixture::{fixture, fixture_class, kitted};
use content_kit::kit::{self, Facts};
use vfs::Rewrite as _;

use super::*;
use crate::tests::{base, pkg, target};

fn files_under(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files_under(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .expect("under the package")
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, std::fs::read(&p).unwrap());
        }
    }
}

/// The files of `content/pogo`, by path in the package.
fn pogo_files() -> BTreeMap<String, Vec<u8>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/pogo");
    let mut out = BTreeMap::new();
    files_under(&root, &root, &mut out);
    out
}

/// The package our game bakes.
fn pogo() -> Loaded {
    Loaded::from_files("content/pogo", &pogo_files()).expect("content/pogo reads")
}

fn court() -> Target<'static> {
    Target {
        name: "court",
        scope: MapScope::Multiplayer,
    }
}

/// The fixture with the kit added as `inject::add_pogo_with` adds it, less
/// the version string it interns: the same calls, one look after another.
fn kit_reference() -> Vec<u8> {
    let original = fixture(0);
    let mut map = Map::from_reader(Cursor::new(original.as_slice())).unwrap();
    let facts = Facts::read_with_class(&mut map, fixture_class()).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    kit::install_all(&mut set, &facts).unwrap();
    set.check_tag_memory = false;
    set.build().unwrap().0
}

fn decomposed(bytes: &[u8]) -> TagSet {
    let mut m = Map::from_reader(Cursor::new(bytes)).unwrap();
    TagSet::from_map(&mut m).unwrap()
}

/// Every tag of `set` as (group, name).
fn names(set: &TagSet) -> Vec<(FourCC, String)> {
    set.slots
        .iter()
        .filter_map(|s| Some((s.tag.as_ref()?.group, s.name.clone()?)))
        .collect()
}

#[test]
fn the_package_reads_and_checks_clean() {
    let p = pogo();
    let manifest = &p.package.manifest;
    assert_eq!(manifest.package.id, "pogo");
    assert_eq!(manifest.package.features, ["pogo"]);
    let count = |k| manifest.items.iter().filter(|i| i.kind == k).count();
    use content::package::Kind;
    assert_eq!(
        (
            count(Kind::Model),
            count(Kind::Tag),
            count(Kind::Sound),
            count(Kind::Data)
        ),
        (3, 3, 3, 4)
    );
    assert_eq!(check_package(&p), vec![]);
    assert_eq!(check_set(&[p]), vec![]);
}

#[test]
fn on_the_fixture_the_package_builds_the_tags_the_kit_builds() {
    let original = fixture(0);
    let (built, report) = build_map(&original, &[pogo()], &court()).expect("the package builds");
    let want = kit_reference();

    // The same tags, by group and name, in the same order.
    let (got_set, want_set) = (decomposed(&built), decomposed(&want));
    assert_eq!(names(&got_set), names(&want_set));
    // Each tag's content (references by name) is the kit's.
    let mut differ = Vec::new();
    for (group, name) in names(&want_set) {
        let (a, b) = (
            got_set
                .find(group, &name)
                .and_then(|id| tag_hash(&got_set, id)),
            want_set
                .find(group, &name)
                .and_then(|id| tag_hash(&want_set, id)),
        );
        if a != b {
            differ.push(format!("{group}:{name}"));
        }
    }
    assert!(
        differ.is_empty(),
        "tags that differ from the kit's: {differ:?}"
    );
    // The whole file, too: the datums, the layout, the strings.
    assert!(built == want, "the package's map differs from the kit's");

    // Seventeen tags, five groups each look, and the shared two.
    assert_eq!(report.added.len(), 2 + 5 * 3);
    for style in Style::ALL {
        let path = style.tag_path();
        for group in [b"bitm", b"shad", b"mode", b"hlmt", b"weap"] {
            assert!(
                got_set.find(FourCC(*group), &path).is_some(),
                "{path}.{}",
                FourCC(*group)
            );
        }
    }
    // Three models, three weapons, four data files, three clips.
    assert_eq!(report.packages, vec![("pogo".to_owned(), 13)]);
}

#[test]
fn the_weapon_borrows_the_first_person_view_of_the_maps_own_pistol() {
    let (built, _) = build_map(&fixture(0), &[pogo()], &court()).unwrap();
    let set = decomposed(&built);
    // The fixture's rifle has one element, its pistol two (one a player
    // character): the stick has the pistol's.
    let want = content_kit::fixture::FIXTURE_FIRST_PERSON.len();
    let mut map = Map::from_reader(Cursor::new(built.as_slice())).unwrap();
    for style in Style::ALL {
        let id = set.find(FourCC(*b"weap"), &style.tag_path()).unwrap();
        let held = kit::first_person_of(&mut map, id).unwrap();
        assert_eq!(held.len(), want, "{}", style.id());
    }
}

#[test]
fn a_map_of_another_class_gets_nothing_and_a_map_that_holds_the_kit_is_left_alone() {
    let campaign = Target {
        name: "court",
        scope: MapScope::Campaign,
    };
    let (_, report) = build_map(&fixture(0), &[pogo()], &campaign).unwrap();
    assert!(report.added.is_empty() && report.packages.is_empty());

    let mut rw = PackageRewrite::new(vec![pogo()]);
    assert!(rw.wants(r"maps\court.map"));
    assert_eq!(rw.rewrite(r"maps\court.map", &kitted().bytes), None);
    assert_eq!(rw.served().lock().unwrap()["court"].held, ["pogo"]);
}

#[test]
fn the_rewrite_serves_the_stick_and_says_which_weapons_it_holds() {
    let seen: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::default();
    let log = Arc::clone(&seen);
    let mut rw = PackageRewrite::new(vec![pogo()]).observed(move |stem, map| {
        log.lock()
            .unwrap()
            .push((stem.to_owned(), map.read_all().unwrap()))
    });
    let served = rw
        .rewrite(r"maps\court.map", &fixture(0))
        .expect("the stick is added");
    let (placed, _) =
        content_kit::served::placed_in(std::io::Cursor::new(&served)).expect("all three weapons");
    assert_eq!(placed, kitted().placed);
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "court");
        assert!(seen[0].1 == served, "the observer reads the served map");
    }
    assert_eq!(content_kit::served::placed_in(std::io::Cursor::new(fixture(0))), None);
}

#[test]
fn a_map_with_no_shader_template_or_no_weapon_of_the_class_stops_by_name() {
    // A map of two tags has no shader template for the model to draw with.
    let bytes = base().build().unwrap().0;
    let err = build_map(&bytes, &[pogo()], &target()).unwrap_err();
    assert!(err.contains("what the map lends a model"), "{err}");

    // A weapon with nothing to borrow from.
    let p = pkg(
        "w",
        &[],
        &[("tag", "x.weap.yaml", Some("invented\\w"), "all")],
        &[(
            "x.weap.yaml",
            b"borrow:\n  - {from: weap, take: [first_person]}\n",
        )],
    );
    let err = build(&mut base(), &[p], &target(), None).unwrap_err();
    assert!(err.why.contains("no tag of the map matches"), "{err}");
}

#[test]
fn the_shared_tags_go_in_the_folder_above_the_look() {
    assert_eq!(
        crate::model::shared_path(r"objects\weapons\pogo\unsc\pogo_unsc").unwrap(),
        r"objects\weapons\pogo\pogo"
    );
    assert!(crate::model::shared_path(r"unsc\pogo_unsc").is_err());
}

#[test]
fn the_model_builder_names_what_it_lacks() {
    let mut files = pogo_files();
    files.remove("models/unsc.png");
    // A package of the one model, its atlas left out of the files.
    let p = pkg(
        "m",
        &[],
        &[(
            "model",
            "models/unsc.yaml",
            Some(r"a\b\unsc\pogo_unsc"),
            "multiplayer",
        )],
        &[("models/unsc.yaml", files["models/unsc.yaml"].as_slice())],
    );
    let found = check_package(&p);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].what.contains("models/unsc.png"), "{found:?}");

    // No tag path.
    let bare = pkg(
        "m",
        &[],
        &[("model", "models/unsc.yaml", None, "multiplayer")],
        &[("models/unsc.yaml", files["models/unsc.yaml"].as_slice())],
    );
    assert!(check_package(&bare)[0].what.contains("`tag`"));
}

/// A 16-bit PCM WAV of `frames` frames of silence.
fn wav(channels: u16, rate: u32, frames: u32, format: u16) -> Vec<u8> {
    let data = frames * u32::from(channels) * 2;
    let mut b = b"RIFF".to_vec();
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(format.to_le_bytes());
    b.extend(channels.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((rate * u32::from(channels) * 2).to_le_bytes());
    b.extend((channels * 2).to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    b.extend(vec![0u8; data as usize]);
    b
}

#[test]
fn the_sound_builder_checks_the_clip_and_leaves_the_map_alone() {
    let ok = wav(1, 22050, 100, 1);
    assert_eq!(
        crate::sound::read_wav(&ok),
        Ok(crate::sound::Wav {
            channels: 1,
            rate: 22050,
            frames: 100
        })
    );
    assert!(crate::sound::read_wav(&wav(1, 22050, 100, 3)).is_err());
    assert!(crate::sound::read_wav(&wav(1, 22050, 0, 1)).is_err());
    assert!(crate::sound::read_wav(b"RIFFnope").is_err());

    let good = pkg(
        "s",
        &[],
        &[("sound", "a.wav", None, "all")],
        &[("a.wav", ok.as_slice())],
    );
    assert_eq!(check_package(&good), vec![]);
    let mut set = base();
    let before = names(&set);
    let report = build(&mut set, &[good], &target(), None).unwrap();
    assert_eq!(names(&set), before);
    assert!(report.added.is_empty());

    // An item with a `tag` is written as a `snd!` (import_tests).
    let tagged = pkg(
        "s",
        &[],
        &[("sound", "a.wav", Some("invented\\snd\\a"), "all")],
        &[("a.wav", ok.as_slice())],
    );
    assert_eq!(check_package(&tagged), vec![]);
    let bad = pkg(
        "s",
        &[],
        &[("sound", "a.wav", None, "all")],
        &[("a.wav", b"not a wav")],
    );
    assert!(check_package(&bad)[0].what.contains("RIFF"));
}
