//! PK1 (docs/editor/studio-redesign.md): golden ids on invented hashes,
//! refusals by name, display fields outside the id, a stable order.
use std::collections::BTreeMap;

use super::*;
use crate::Sha256;

fn h(n: u8) -> Sha256 {
    Sha256([n; 32])
}

const POGO: &str = r#"
[package]
format = 1
id = "pogo"
name = "Pogo Stick"
description = "The stick a Superbounce player carries."
image = "preview.png"
requires = ["core"]
after = ["superbounce"]
features = ["pogo"]

[credit]
authors = ["Somebody"]
notes = "Original shapes."

[[item]]
kind = "model"
source = "models/unsc.yaml"
tag = 'objects\weapons\pogo'
maps = "multiplayer"

[[item]]
kind = "sound"
source = "sounds/spring.wav"
maps = ["midship", "Lockout"]
"#;

fn pogo_lock() -> Lock {
    Lock::new(BTreeMap::from([
        ("models/unsc.yaml".to_owned(), h(0x11)),
        ("sounds/spring.wav".to_owned(), h(0x22)),
    ]))
}

fn pogo() -> Manifest {
    Manifest::from_toml(POGO).unwrap()
}

#[test]
fn golden_canonical_json_and_id() {
    let canonical = PackageId::canonical(&pogo(), &pogo_lock()).unwrap();
    assert_eq!(
        canonical,
        r#"{"after":["superbounce"],"features":["pogo"],"format":1,"id":"pogo","item":[{"kind":"model","maps":"multiplayer","source":"1111111111111111111111111111111111111111111111111111111111111111","tag":"objects\\weapons\\pogo"},{"kind":"sound","maps":["lockout","midship"],"source":"2222222222222222222222222222222222222222222222222222222222222222"}],"requires":["core"]}"#
    );
    let id = PackageId::of(&pogo(), &pogo_lock()).unwrap();
    assert_eq!(
        id.to_string(),
        "9070724e2daabb093371d1f8d4d09808c1932538031911c7858d4428a8345f89"
    );
    assert_eq!(id.to_string().parse::<PackageId>().unwrap(), id);
}

#[test]
fn display_fields_and_credit_do_not_move_the_id() {
    let base = PackageId::of(&pogo(), &pogo_lock()).unwrap();
    let mut m = pogo();
    m.package.name = "Another name".into();
    m.package.description = None;
    m.package.image = Some("other.png".into());
    m.credit.authors = vec!["Someone else".into()];
    m.credit.release = Some("Original release".into());
    m.credit.notes = None;
    assert_eq!(PackageId::of(&m, &pogo_lock()).unwrap(), base);
}

#[test]
fn what_the_package_says_moves_the_id() {
    let base = PackageId::of(&pogo(), &pogo_lock()).unwrap();
    let id = |f: &dyn Fn(&mut Manifest)| {
        let mut m = pogo();
        f(&mut m);
        PackageId::of(&m, &pogo_lock()).unwrap()
    };
    assert_ne!(id(&|m| m.package.id = "pogo2".into()), base);
    assert_ne!(id(&|m| m.package.requires.clear()), base);
    assert_ne!(id(&|m| m.package.features.push("more".into())), base);
    assert_ne!(
        id(&|m| m.items[0].tag = Some(r"objects\other".into())),
        base
    );
    assert_ne!(id(&|m| m.items.swap(0, 1)), base);
    // A changed file moves it through the lock.
    let mut lock = pogo_lock();
    lock.files.insert("models/unsc.yaml".into(), h(0x33));
    assert_ne!(PackageId::of(&pogo(), &lock).unwrap(), base);
}

#[test]
fn sets_in_the_manifest_do_not_depend_on_their_order() {
    let base = PackageId::of(&pogo(), &pogo_lock()).unwrap();
    let mut m = pogo();
    m.package.after = vec!["zebra".into(), "alpha".into()];
    let a = PackageId::of(&m, &pogo_lock()).unwrap();
    m.package.after = vec!["alpha".into(), "zebra".into()];
    assert_eq!(PackageId::of(&m, &pogo_lock()).unwrap(), a);
    assert_ne!(a, base);
}

#[test]
fn a_source_without_a_lock_hash_is_named() {
    let mut lock = pogo_lock();
    lock.files.remove("sounds/spring.wav");
    assert_eq!(lock.missing(&pogo()), Some("sounds/spring.wav"));
    assert_eq!(
        PackageId::of(&pogo(), &lock).unwrap_err(),
        IdError::MissingHash("sounds/spring.wav".into())
    );
}

#[test]
fn lock_and_manifest_round_trip_through_toml() {
    let lock = pogo_lock();
    assert_eq!(Lock::from_toml(&lock.to_toml()).unwrap(), lock);
    let m = pogo();
    assert_eq!(Manifest::from_toml(&m.to_toml()).unwrap(), m);
    assert!(Lock::from_toml("format = 1\nsurprise = 2\n").is_err());
    assert_eq!(Lock::from_toml("format = 9\n"), Err(IdError::Format(9)));
}

fn toml_err(text: &str) -> String {
    match Manifest::from_toml(text).unwrap_err() {
        ManifestError::Toml(t) => t,
        other => panic!("not a TOML error: {other}"),
    }
}

#[test]
fn unknown_keys_are_refused_by_name() {
    let bad = |from: &str, to: &str| POGO.replacen(from, to, 1);
    assert!(toml_err(&bad("format = 1", "format = 1\ncolour = 3")).contains("colour"));
    assert!(toml_err(&bad("[credit]", "[credit]\nlicence = \"x\"")).contains("licence"));
    assert!(toml_err(&bad("kind = \"model\"", "kind = \"model\"\nwere = \"y\"")).contains("were"));
    assert!(toml_err(&format!("{POGO}\n[wat]\nx = 1\n")).contains("wat"));
    assert!(toml_err(&bad("kind = \"model\"", "kind = \"spaceship\"")).contains("spaceship"));
    assert!(toml_err(&bad("maps = \"multiplayer\"", "maps = \"nowhere\"")).contains("nowhere"));
    let set = SetFile::from_toml("order = []\nflavour = 1\n").unwrap_err();
    assert!(matches!(set, SetError::Toml(t) if t.contains("flavour")));
}

#[test]
fn validation_refuses_by_name() {
    let with = |f: &dyn Fn(&mut Manifest)| {
        let mut m = pogo();
        f(&mut m);
        m.validate().unwrap_err()
    };
    assert_eq!(with(&|m| m.package.format = 2), ManifestError::Format(2));
    assert_eq!(
        with(&|m| m.package.id = "Pogo".into()),
        ManifestError::Id("Pogo".into())
    );
    assert_eq!(
        with(&|m| m.package.name = " ".into()),
        ManifestError::EmptyName
    );
    assert_eq!(
        with(&|m| m.package.requires = vec!["pogo".into()]),
        ManifestError::SelfReference("pogo".into())
    );
    assert_eq!(
        with(&|m| m.package.after = vec!["core".into()]),
        ManifestError::BothRequiresAndAfter {
            package: "pogo".into(),
            name: "core".into()
        }
    );
    assert!(matches!(
        with(&|m| m.package.requires = vec!["Bad Name".into()]),
        ManifestError::BadReference {
            field: "requires",
            ..
        }
    ));
    assert!(matches!(
        with(&|m| m.package.after = vec!["a".into(), "a".into()]),
        ManifestError::DuplicateReference { field: "after", .. }
    ));
    assert_eq!(
        with(&|m| m.items[0].source = Some("../out.yaml".into())),
        ManifestError::Escapes("../out.yaml".into())
    );
    assert_eq!(
        with(&|m| m.items[1].source = None),
        ManifestError::NoSource {
            index: 1,
            kind: Kind::Sound
        }
    );
    assert!(matches!(
        with(&|m| m.items[1].maps = Maps::Names(vec![])),
        ManifestError::BadMaps { index: 1, .. }
    ));
}

#[test]
fn a_package_never_holds_disc_bytes() {
    for (file, what) in [
        ("maps/lockout.map", "a cache map"),
        ("bundle.H2TAG", "a .h2tag bundle"),
        ("disc/default.xbe", "disc files"),
    ] {
        let mut m = pogo();
        m.items[0].source = Some(file.into());
        assert_eq!(
            m.validate().unwrap_err(),
            ManifestError::Forbidden {
                path: file.into(),
                what
            }
        );
    }
}

#[test]
fn kinds_know_who_needs_them() {
    let m = pogo();
    assert!(m.has_sim_item());
    let mut m = m;
    m.items.retain(|i| i.kind == Kind::Sound);
    assert!(!m.has_sim_item());
    assert_eq!(Kind::Playlist.scope(), ItemScope::Live);
    assert_eq!(Kind::Kit.scope(), ItemScope::PerFeature);
    assert_eq!(Kind::TagDelta.scope(), ItemScope::Sim);
}

// -------------------------------------------------------------- mods (AS6)

/// Invented asset hashes: never a store's.
const PISTOL: &str = "abababababababababababababababababababababababababababababababab";
const RIFLE: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const OTHER: &str = "efefefefefefefefefefefefefefefefefefefefefefefefefefefefefefefef";

/// A mod: an authored override of one weapon, a field patch and a delta of
/// another.
const MOD: &str = r#"
[package]
format = 1
id = "balanced"
name = "Balanced"

[[item]]
kind = "tag"
source = "tags/pistol.jpt!.yaml"
tag = 'objects\weapons\pistol'
replaces = "PISTOL"

[[item]]
kind = "edits"
source = "edits.yaml"

[[item]]
kind = "tag-delta"
source = "deltas/rifle.weap.bps"
tag = 'objects\weapons\rifle'
was = "RIFLE"
maps = "multiplayer"
"#;

fn invention() -> Manifest {
    Manifest::from_toml(&MOD.replace("PISTOL", PISTOL).replace("RIFLE", RIFLE)).unwrap()
}

fn invention_lock() -> Lock {
    Lock::new(BTreeMap::from([
        ("tags/pistol.jpt!.yaml".to_owned(), h(0x71)),
        ("edits.yaml".to_owned(), h(0x72)),
        ("deltas/rifle.weap.bps".to_owned(), h(0x73)),
    ]))
}

#[test]
fn a_mod_names_its_assets_by_key_and_hash() {
    let m = invention();
    let [tag, edits, delta] = (&m.items[0], &m.items[1], &m.items[2]);
    assert_eq!(tag.replaces.as_deref(), Some(PISTOL));
    assert_eq!((edits.replaces, edits.was), (None, None));
    assert_eq!(
        (delta.kind, delta.was.as_deref()),
        (Kind::TagDelta, Some(RIFLE))
    );
    assert_eq!(delta.maps, Maps::Scope(MapScope::Multiplayer));
    assert_eq!(Manifest::from_toml(&m.to_toml()).unwrap(), m);
    assert!(m.has_sim_item());
    // An item without them says neither.
    let text = pogo().to_toml();
    assert!(
        !text.contains("replaces") && !text.contains("\nwas = "),
        "{text}"
    );
    // A package left out of one level, and why.
    let refusal = Refusal {
        package: "balanced".into(),
        why: "it was made on another asset".into(),
    };
    assert_eq!(
        refusal.to_string(),
        "package \"balanced\" is off for this level: it was made on another asset"
    );
}

#[test]
fn replaces_and_was_move_the_id_as_the_hash_they_name() {
    let base = PackageId::of(&invention(), &invention_lock()).unwrap();
    let id = |f: &dyn Fn(&mut Item)| {
        let mut m = invention();
        for item in &mut m.items {
            f(item);
        }
        PackageId::of(&m, &invention_lock()).unwrap()
    };
    // The case a hash is written in does not move it.
    let upper = id(&|i| {
        for t in [&mut i.replaces, &mut i.was].into_iter().flatten() {
            *t = t.to_ascii_uppercase();
        }
    });
    assert_eq!(upper, base);
    // Another asset does.
    let other_replaces = id(&|i| i.replaces = i.replaces.map(|_| OTHER.into()));
    let other_was = id(&|i| i.was = i.was.map(|_| OTHER.into()));
    assert_ne!(other_replaces, base);
    assert_ne!(other_was, base);
    assert_ne!(other_replaces, other_was);
    // So does leaving them out.
    let text = invention()
        .to_toml()
        .replace(&format!("replaces = \"{PISTOL}\"\n"), "")
        .replace(&format!("was = \"{RIFLE}\"\n"), "");
    let bare = Manifest::from_toml(&text).unwrap();
    assert!(
        bare.items
            .iter()
            .all(|i| i.replaces.is_none() && i.was.is_none())
    );
    assert_ne!(PackageId::of(&bare, &invention_lock()).unwrap(), base);
}

#[test]
fn replaces_and_was_are_refused_by_kind_and_shape() {
    let err = |f: &dyn Fn(&mut Manifest)| {
        let mut m = invention();
        f(&mut m);
        m.validate().unwrap_err()
    };
    // `replaces` on a kind that makes no asset of its own.
    for kind in [Kind::Edits, Kind::Kit, Kind::TagDelta] {
        assert_eq!(
            err(&|m| m.items[0].kind = kind),
            ManifestError::Replaces { index: 0, kind },
            "{kind:?}"
        );
    }
    // `was` on anything but a delta.
    for kind in [Kind::Tag, Kind::Edits, Kind::Model] {
        assert_eq!(
            err(&|m| m.items[2].kind = kind),
            ManifestError::Was { index: 2, kind },
            "{kind:?}"
        );
    }
    // Not an asset hash: refused, naming the item and the field.
    let changes: [(&str, usize, &str, &dyn Fn(&mut Manifest)); 2] = [
        ("replaces", 0, "not a hash", &|m| {
            m.items[0].replaces = Some("not a hash".into())
        }),
        ("was", 2, "cd", &|m| m.items[2].was = Some("cd".into())),
    ];
    for (field, index, text, f) in changes {
        assert_eq!(
            err(f),
            ManifestError::Hash {
                index,
                field,
                text: text.into(),
            }
        );
    }
}

fn pkg(id: &str, requires: &[&str], after: &[&str]) -> Package {
    let list = |v: &[&str]| format!("{:?}", v);
    let text = format!(
        r#"
[package]
format = 1
id = "{id}"
name = "{id}"
requires = {}
after = {}

[[item]]
kind = "edits"
source = "e.yaml"
"#,
        list(requires),
        list(after)
    );
    let lock = Lock::new(BTreeMap::from([("e.yaml".to_owned(), h(0x44))]));
    Package::new(Manifest::from_toml(&text).unwrap(), lock).unwrap()
}

fn names(set: &Set) -> Vec<&str> {
    set.loaded.iter().map(Package::name).collect()
}

fn file(order: &[&str], off: &[&str]) -> SetFile {
    SetFile {
        order: order.iter().map(|s| s.to_string()).collect(),
        off: off.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn requires_and_after_come_first() {
    let set = Set::new(
        vec![
            pkg("c", &["b"], &[]),
            pkg("b", &[], &["a"]),
            pkg("a", &[], &[]),
        ],
        &SetFile::default(),
    )
    .unwrap();
    assert_eq!(names(&set), ["a", "b", "c"]);
    assert!(set.problems.is_empty());
}

#[test]
fn after_an_absent_package_is_ignored() {
    let set = Set::new(vec![pkg("a", &[], &["ghost"])], &SetFile::default()).unwrap();
    assert_eq!(names(&set), ["a"]);
}

#[test]
fn ties_go_to_the_users_order_then_the_id() {
    let pkgs = || vec![pkg("a", &[], &[]), pkg("b", &[], &[]), pkg("c", &[], &[])];
    let by_id = Set::new(pkgs(), &SetFile::default()).unwrap();
    assert_eq!(names(&by_id), ["a", "b", "c"]);
    let user = Set::new(pkgs(), &file(&["c", "a"], &[])).unwrap();
    assert_eq!(names(&user), ["c", "a", "b"]);
    // The user's order never beats a dependency.
    let dep = Set::new(
        vec![
            pkg("a", &["c"], &[]),
            pkg("c", &[], &[]),
            pkg("b", &[], &[]),
        ],
        &file(&["a", "b", "c"], &[]),
    )
    .unwrap();
    assert_eq!(names(&dep), ["b", "c", "a"]);
}

#[test]
fn the_order_does_not_depend_on_how_packages_arrive() {
    let mut pkgs = vec![
        pkg("d", &["a"], &[]),
        pkg("a", &[], &[]),
        pkg("b", &[], &["d"]),
        pkg("c", &["a", "b"], &[]),
        pkg("e", &[], &[]),
    ];
    let f = file(&["e"], &[]);
    let first = Set::new(pkgs.clone(), &f).unwrap();
    for _ in 0..pkgs.len() {
        pkgs.rotate_left(1);
        assert_eq!(Set::new(pkgs.clone(), &f).unwrap(), first);
        let mut reversed = pkgs.clone();
        reversed.reverse();
        assert_eq!(Set::new(reversed, &f).unwrap(), first);
    }
    assert_eq!(names(&first), ["e", "a", "d", "b", "c"]);
}

#[test]
fn a_cycle_is_refused_naming_its_members() {
    let err = Set::new(
        vec![
            pkg("a", &["b"], &[]),
            pkg("b", &[], &["c"]),
            pkg("c", &["a"], &[]),
            pkg("d", &["a"], &[]),
            pkg("free", &[], &[]),
        ],
        &SetFile::default(),
    )
    .unwrap_err();
    assert_eq!(
        err,
        SetError::Cycle(vec!["a".into(), "b".into(), "c".into()])
    );
    assert!(err.to_string().contains("a, b, c"));
}

#[test]
fn a_missing_requires_is_named_and_the_package_is_left_out() {
    let set = Set::new(
        vec![
            pkg("a", &["ghost"], &[]),
            pkg("b", &["a"], &[]),
            pkg("c", &[], &[]),
        ],
        &SetFile::default(),
    )
    .unwrap();
    assert_eq!(names(&set), ["c"]);
    assert_eq!(
        set.problems,
        [
            Problem::Missing {
                package: "a".into(),
                requires: "ghost".into()
            },
            Problem::Disabled {
                package: "b".into(),
                requires: "a".into()
            },
        ]
    );
    assert!(set.problems[0].to_string().contains("\"ghost\""));
}

#[test]
fn switching_a_package_off_takes_its_dependents() {
    let set = Set::new(
        vec![
            pkg("a", &[], &[]),
            pkg("b", &["a"], &[]),
            pkg("c", &[], &["a"]),
        ],
        &file(&[], &["a", "never-installed"]),
    )
    .unwrap();
    assert_eq!(names(&set), ["c"]);
    assert_eq!(
        set.problems,
        [Problem::Off {
            package: "b".into(),
            requires: "a".into()
        }]
    );
}

#[test]
fn two_packages_with_one_id_are_refused() {
    let err = Set::new(
        vec![pkg("a", &[], &[]), pkg("a", &["b"], &[])],
        &SetFile::default(),
    )
    .unwrap_err();
    assert_eq!(err, SetError::DuplicateId("a".into()));
}

#[test]
fn the_content_set_is_the_sim_packages_in_order() {
    let mut client = pkg("a", &[], &[]);
    client.manifest.items[0].kind = Kind::Bitmap;
    let sim = pkg("b", &["a"], &[]);
    let set = Set::new(vec![sim.clone(), client], &SetFile::default()).unwrap();
    assert_eq!(set.content_set(), [sim.id]);
    let hash = set.content_hash();
    let again = Set::new(vec![sim.clone()], &SetFile::default());
    // b requires a, which is absent here: nothing loads and the hash differs.
    assert_ne!(again.unwrap().content_hash(), hash);
}

#[test]
fn set_file_round_trips() {
    let f = file(&["b", "a"], &["x"]);
    assert_eq!(SetFile::from_toml(&f.to_toml()).unwrap(), f);
    assert_eq!(SetFile::from_toml("").unwrap(), SetFile::default());
}

#[test]
fn core_and_ui_are_always_on() {
    for name in ALWAYS_ON {
        let text = format!("off = [\"{name}\"]\n");
        assert_eq!(
            SetFile::from_toml(&text).unwrap_err(),
            SetError::AlwaysOn(name.into())
        );
        let err = Set::new(vec![pkg(name, &[], &[])], &file(&[], &[name])).unwrap_err();
        assert_eq!(err, SetError::AlwaysOn(name.into()));
        assert!(err.to_string().contains(&format!("{name:?}")), "{err}");
    }
    // Nothing they could lack can disable them.
    let err = Set::new(vec![pkg("core", &["a"], &[])], &SetFile::default()).unwrap_err();
    assert_eq!(
        err,
        SetError::AlwaysOnRequires {
            package: "core".into(),
            requires: "a".into()
        }
    );
    let set = Set::new(
        vec![pkg("a", &["core"], &[]), pkg("core", &[], &[])],
        &file(&[], &["a"]),
    )
    .unwrap();
    assert_eq!(names(&set), ["core"]);
}
