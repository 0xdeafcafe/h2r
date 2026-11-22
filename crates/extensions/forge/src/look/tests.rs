//! Invented data only: no tag, key or hash here comes from a disc.
use std::collections::BTreeMap;

use serde_json::json;

use super::census::{self, Census, Described, Disc, Entry, MapInfo};
use super::saved::{self, Saved};
use super::*;

fn look() -> MonitorLook {
    MonitorLook {
        version: VERSION,
        model: "widget".into(),
        variant: Some("blue".into()),
        primary: 3,
        secondary: 17,
        tint: 2,
        light: 1,
        light_colour: 5,
    }
}

fn entry(key: &str, variants: &[&str], colours: bool) -> Entry {
    Entry {
        key: key.into(),
        name: key.into(),
        source: "map_a".into(),
        sha256: "a".repeat(64),
        variants: variants.iter().map(|v| (*v).into()).collect(),
        change_colours: colours,
        light: false,
        nodes: 4,
        triangles: 100,
    }
}

#[test]
fn a_good_look_validates_and_the_default_is_the_seats_marker() {
    look().validate().unwrap();
    let d = MonitorLook::default_for(7);
    d.validate().unwrap();
    assert!(d.is_marker());
    assert_eq!((d.primary, d.secondary, d.light_colour), (7, 7, 7));
    assert_eq!((d.tint, d.light, d.variant), (0, 0, None));
}

#[test]
fn every_bound_has_a_refusing_case() {
    let cases: Vec<(&str, Box<dyn Fn(&mut MonitorLook)>)> = vec![
        (
            "model",
            Box::new(|l: &mut MonitorLook| l.model = String::new()),
        ),
        (
            "model",
            Box::new(|l: &mut MonitorLook| l.model = "x".repeat(KEY_MAX + 1)),
        ),
        (
            "model",
            Box::new(|l: &mut MonitorLook| l.model = "Upper".into()),
        ),
        (
            "model",
            Box::new(|l: &mut MonitorLook| l.model = "a-b".into()),
        ),
        (
            "variant",
            Box::new(|l: &mut MonitorLook| l.variant = Some(String::new())),
        ),
        (
            "variant",
            Box::new(|l: &mut MonitorLook| l.variant = Some("x".repeat(VARIANT_MAX + 1))),
        ),
        (
            "variant",
            Box::new(|l: &mut MonitorLook| l.variant = Some("A b".into())),
        ),
        (
            "primary",
            Box::new(|l: &mut MonitorLook| l.primary = PALETTE),
        ),
        (
            "secondary",
            Box::new(|l: &mut MonitorLook| l.secondary = PALETTE),
        ),
        (
            "tint",
            Box::new(|l: &mut MonitorLook| l.tint = TINT_MAX + 1),
        ),
        (
            "light",
            Box::new(|l: &mut MonitorLook| l.light = LIGHT_MAX + 1),
        ),
        (
            "light_colour",
            Box::new(|l: &mut MonitorLook| l.light_colour = PALETTE),
        ),
    ];
    for (field, change) in cases {
        let mut l = look();
        change(&mut l);
        match l.validate() {
            Err(Refusal::Invalid { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{field}: {other:?}"),
        }
    }
    // The last of each range is accepted.
    let mut l = look();
    (l.primary, l.secondary, l.light_colour) = (PALETTE - 1, PALETTE - 1, PALETTE - 1);
    (l.tint, l.light) = (TINT_MAX, LIGHT_MAX);
    l.validate().unwrap();
    let mut old = look();
    old.version = 0;
    assert_eq!(
        old.validate(),
        Err(Refusal::Version {
            theirs: 0,
            ours: VERSION
        })
    );
}

#[test]
fn the_marker_has_no_variant_tint_or_light() {
    let cases: [(&str, fn(&mut MonitorLook)); 3] = [
        ("variant", |l: &mut MonitorLook| {
            l.variant = Some("x".into())
        }),
        ("tint", |l: &mut MonitorLook| l.tint = 1),
        ("light", |l: &mut MonitorLook| l.light = 1),
    ];
    for (field, change) in cases {
        let mut l = MonitorLook::default_for(0);
        change(&mut l);
        assert!(
            matches!(l.validate(), Err(Refusal::Invalid { field: f, .. }) if f == field),
            "{field}"
        );
    }
}

#[test]
fn canonical_bytes_match_their_golden_vectors() {
    assert_eq!(
        MonitorLook::default_for(3).canonical().unwrap(),
        r#"{"light":0,"light_colour":3,"model":"marker","primary":3,"secondary":3,"tint":0,"version":1}"#
    );
    assert_eq!(
        look().canonical().unwrap(),
        r#"{"light":1,"light_colour":5,"model":"widget","primary":3,"secondary":17,"tint":2,"variant":"blue","version":1}"#
    );
}

#[test]
fn canonical_bytes_round_trip_and_nothing_else_is_canonical() {
    let l = look();
    let bytes = l.canonical().unwrap();
    assert_eq!(MonitorLook::from_canonical(bytes.as_bytes()), Ok(l.clone()));
    // Same value, different key order or spacing: not the canonical bytes.
    let spaced = bytes.replace(',', ", ");
    assert!(MonitorLook::from_canonical(spaced.as_bytes()).is_err());
    assert!(MonitorLook::from_canonical(b"not json").is_err());
    // An unknown key, a missing field and an out-of-range value are named.
    let mut v = l.to_value();
    v["extra"] = json!(1);
    assert!(matches!(
        MonitorLook::from_value(&v),
        Err(Refusal::Invalid { field, .. }) if field == "extra"
    ));
    let mut v = l.to_value();
    v.as_object_mut().unwrap().remove("tint");
    assert!(matches!(
        MonitorLook::from_value(&v),
        Err(Refusal::Invalid { field, .. }) if field == "tint"
    ));
    let mut v = l.to_value();
    v["primary"] = json!(300);
    assert!(matches!(
        MonitorLook::from_value(&v),
        Err(Refusal::Invalid { field, .. }) if field == "primary"
    ));
    let mut v = l.to_value();
    v["version"] = json!(2);
    assert_eq!(
        MonitorLook::from_value(&v),
        Err(Refusal::Version {
            theirs: 2,
            ours: VERSION
        })
    );
}

#[test]
fn a_patch_reads_named_fields_and_refuses_the_rest() {
    let p = Patch::from_value(&json!({"model": "widget", "variant": null, "tint": 3})).unwrap();
    assert_eq!(p.model.as_deref(), Some("widget"));
    assert_eq!(p.variant, Some(None));
    assert_eq!(p.fields(), ["model", "variant", "tint"]);
    assert!(Patch::from_value(&json!({})).unwrap().is_empty());
    for bad in [
        json!({"tint": 4}),
        json!({"light": -1}),
        json!({"primary": "red"}),
        json!({"model": 3}),
        json!({"version": 1}),
        json!({"nope": 1}),
        json!([]),
    ] {
        assert!(Patch::from_value(&bad).is_err(), "{bad}");
    }
}

#[test]
fn each_refusal_has_its_code_and_fields() {
    let all = [
        Refusal::invalid("tint", "must be small"),
        Refusal::SourceMissing {
            machine: "Ann".into(),
            map: "map_a".into(),
        },
        Refusal::Renderer {
            machine: "Bob".into(),
        },
        Refusal::locked("primary", "Team colours"),
        Refusal::Version { theirs: 2, ours: 1 },
        Refusal::NoSession,
    ];
    let codes: Vec<_> = all.iter().map(Refusal::code).collect();
    assert_eq!(codes, CODES);
    for r in &all {
        let v = r.to_value();
        assert_eq!(v["code"], r.code());
        assert!(!v["message"].as_str().unwrap().is_empty());
    }
    assert_eq!(all[1].to_value()["machine"], "Ann");
    assert_eq!(all[1].to_value()["map"], "map_a");
    assert_eq!(all[2].to_value()["machine"], "Bob");
    assert_eq!(all[3].to_value()["field"], "primary");
}

#[test]
fn switching_to_the_marker_clears_what_it_cannot_draw() {
    let l = look();
    let m = l
        .with(&Patch {
            model: Some(MARKER.into()),
            ..Patch::default()
        })
        .unwrap();
    assert_eq!((m.variant, m.tint, m.light), (None, 0, 0));
    assert_eq!((m.primary, m.secondary), (3, 17));
}

#[test]
fn apply_checks_the_catalogue_and_the_locks() {
    let catalogue = [
        entry("widget", &["blue", "red"], true),
        entry("plain", &["only"], false),
    ];
    let marker = MonitorLook::default_for(2);
    let widget = apply(
        &marker,
        &Patch {
            model: Some("widget".into()),
            variant: Some(Some("red".into())),
            tint: Some(1),
            ..Patch::default()
        },
        &catalogue,
    )
    .unwrap();
    assert_eq!(widget.variant.as_deref(), Some("red"));
    // A model the catalogue lacks, and a variant the model lacks.
    assert!(matches!(
        apply(&marker, &Patch { model: Some("ghost".into()), ..Patch::default() }, &catalogue),
        Err(Refusal::Invalid { field, .. }) if field == "model"
    ));
    assert!(matches!(
        apply(&widget, &Patch { variant: Some(Some("green".into())), ..Patch::default() }, &catalogue),
        Err(Refusal::Invalid { field, .. }) if field == "variant"
    ));
    // A model with no colour areas locks the colour rows and says why.
    let plain = apply(
        &marker,
        &Patch {
            model: Some("plain".into()),
            ..Patch::default()
        },
        &catalogue,
    )
    .unwrap();
    assert!(matches!(
        apply(&plain, &Patch { primary: Some(9), ..Patch::default() }, &catalogue),
        Err(Refusal::Locked { field, reason }) if field == "primary" && reason.contains("colour areas")
    ));
    // Naming a locked field at its present value is not a change.
    apply(
        &plain,
        &Patch {
            primary: Some(plain.primary),
            ..Patch::default()
        },
        &catalogue,
    )
    .unwrap();
    // The marker locks the variant, tint and light rows.
    assert!(matches!(
        apply(&marker, &Patch { tint: Some(1), ..Patch::default() }, &catalogue),
        Err(Refusal::Invalid { field, .. }) if field == "tint"
    ));
    assert_eq!(locks(MARKER, &catalogue).len(), 3);
    assert_eq!(locks("plain", &catalogue).len(), 3);
    assert!(locks("widget", &catalogue).is_empty());
}

// ---- the profile ----

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("h2-look-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join(settings::SETTINGS_FILE)
}

fn defs() -> Vec<&'static settings::Def> {
    saved::DEFS.iter().collect()
}

fn store(path: &std::path::Path) -> (settings::Store, Vec<settings::Problem>) {
    settings::Store::load(&defs(), Some(path), &|_| None, &[])
}

#[test]
fn the_defs_are_valid_presentation_settings_with_the_eight_keys() {
    let keys: Vec<_> = saved::DEFS.iter().map(|d| d.key).collect();
    assert_eq!(
        keys,
        [
            saved::PRIMARY,
            saved::SECONDARY,
            saved::TINT,
            saved::LIGHT,
            saved::LIGHT_COLOUR
        ]
    );
    for d in &saved::DEFS {
        assert_eq!(d.check(d.default), Ok(d.default), "{}", d.key);
        assert_eq!(d.scope, settings::Scope::View, "{}", d.key);
        assert!(d.env.is_none() && d.flag.is_none() && d.menu.is_none());
        assert!(d.key.starts_with("forge.monitor_"));
    }
    assert_eq!(saved::TEXT_KEYS.len() + saved::DEFS.len(), 8);
}

#[test]
fn a_profile_with_nothing_loads_the_seats_marker() {
    let path = tmp("empty");
    let (s, _) = store(&path);
    let loaded = saved::load(&s, "Chief", 4);
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    assert_eq!(loaded.saved, Saved::marker(4));
}

#[test]
fn a_look_round_trips_through_the_store_and_the_file() {
    let path = tmp("round-trip");
    let (s, _) = store(&path);
    let saved_look = Saved {
        look: look(),
        source: Some("b".repeat(64)),
    };
    assert_eq!(saved::save(&s, "Chief", &saved_look), Ok(None));
    assert_eq!(saved::load(&s, "Chief", 0).saved, saved_look);
    // A fresh store reads the same from the file, per profile.
    let (again, problems) = store(&path);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(saved::load(&again, "Chief", 0).saved, saved_look);
    assert_eq!(saved::load(&again, "Other", 6).saved, Saved::marker(6));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[profiles.Chief.forge]"), "{text}");
    assert!(text.contains("monitor_model = \"widget\""), "{text}");
    assert!(text.contains("monitor_tint = \"medium\""), "{text}");
    assert!(text.contains("monitor_light = \"dim\""), "{text}");
    assert!(text.contains("monitor_primary = 3"), "{text}");
}

#[test]
fn saving_the_marker_clears_the_variant_and_source() {
    let path = tmp("marker");
    let (s, _) = store(&path);
    let disc = Saved {
        look: look(),
        source: Some("b".repeat(64)),
    };
    saved::save(&s, "Chief", &disc).unwrap();
    let marker = Saved::marker(1);
    saved::save(&s, "Chief", &marker).unwrap();
    assert_eq!(saved::load(&s, "Chief", 1).saved, marker);
}

#[test]
fn a_bad_value_is_reported_by_key_and_left_in_the_file() {
    let path = tmp("bad");
    let file = "[profiles.Chief.forge]\nmonitor_primary = 99\nmonitor_model = \"Bad Key\"\nmonitor_source = \"nothex\"\nmonitor_tint = 2\n";
    std::fs::write(&path, file).unwrap();
    let (s, mut problems) = store(&path);
    let loaded = saved::load(&s, "Chief", 5);
    problems.extend(loaded.problems.iter().map(|p| settings::Problem {
        key: None,
        message: p.clone(),
    }));
    let text: String = problems
        .iter()
        .map(|p| p.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("monitor_primary"), "{text}");
    assert!(text.contains("monitor_model"), "{text}");
    assert!(text.contains("monitor_source"), "{text}");
    // Nothing valid was saved, so the seat's marker is used.
    assert_eq!(loaded.saved, Saved::marker(5));
    assert!(text.contains("monitor_tint"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        file,
        "loading writes nothing"
    );
}

#[test]
fn an_unknown_saved_model_is_kept_in_the_file_and_the_session_gets_the_marker() {
    let path = tmp("unknown");
    let (s, _) = store(&path);
    let want = Saved {
        look: look(),
        source: Some("b".repeat(64)),
    };
    saved::save(&s, "Chief", &want).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let loaded = saved::load(&s, "Chief", 0).saved;
    let catalogue = [entry("other", &["blue"], true)];
    let r = saved::resolve(&loaded, &catalogue);
    assert!(r.look.is_marker());
    assert_eq!((r.look.primary, r.look.secondary), (3, 17));
    assert_eq!(
        (r.look.tint, r.look.light, r.look.variant.clone()),
        (0, 0, None)
    );
    assert_eq!(
        r.notice.as_deref(),
        Some("Widget needs a campaign map this install does not have")
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "nothing rewritten"
    );
    assert_eq!(saved::load(&s, "Chief", 0).saved, want, "still saved");
    // The same key from a different source map is not the saved model.
    let mut other_map = entry("widget", &["blue"], true);
    other_map.sha256 = "c".repeat(64);
    assert!(saved::resolve(&loaded, &[other_map]).notice.is_some());
    // From the right map it stands.
    let mut right = entry("widget", &["blue"], true);
    right.sha256 = "b".repeat(64);
    let r = saved::resolve(&loaded, &[right.clone()]);
    assert_eq!((r.look.clone(), r.notice), (loaded.look.clone(), None));
    // A variant the model no longer has is said, not dropped.
    right.variants = vec!["red".into()];
    assert!(
        saved::resolve(&loaded, &[right])
            .notice
            .unwrap()
            .contains("blue")
    );
}

// ---- the census ----

#[derive(Default)]
struct Fake {
    maps: Vec<(MapInfo, Vec<&'static str>)>,
    bad: Vec<&'static str>,
}

impl Disc for Fake {
    fn campaign_maps(&self) -> Result<Vec<MapInfo>, String> {
        Ok(self.maps.iter().map(|(m, _)| m.clone()).collect())
    }
    fn bipeds(&self, map: &MapInfo) -> Result<Vec<String>, String> {
        let (_, tags) = self.maps.iter().find(|(m, _)| m == map).ok_or("no map")?;
        Ok(tags.iter().map(|t| (*t).to_owned()).collect())
    }
    fn describe(&self, map: &MapInfo, tag: &str) -> Result<Described, String> {
        if self.bad.contains(&tag) {
            return Err("unreadable".into());
        }
        Ok(Described {
            variants: vec![format!("{}_a", map.name), "BAD NAME".into()],
            change_colours: tag.contains("coloured"),
            light: false,
            nodes: tag.len() as u32,
            triangles: 10,
        })
    }
}

fn map(name: &str, hash: char) -> MapInfo {
    MapInfo {
        name: name.into(),
        sha256: hash.to_string().repeat(64),
    }
}

fn fake() -> Fake {
    Fake {
        maps: vec![
            (
                map("m1", 'a'),
                vec![
                    "objects\\things\\gizmo\\gizmo",
                    "objects\\things\\gizmo\\gizmo_mp",
                    "objects\\things\\coloured\\coloured",
                    "loose",
                ],
            ),
            (
                map("m2", 'b'),
                vec![
                    "objects\\things\\gizmo\\gizmo",
                    "objects\\things\\gizmo\\gizmo",
                    "objects\\things\\gizmo\\gizmo_mp",
                    "objects\\things\\gizmo\\gizmo_mp",
                    "objects\\things\\marker\\marker",
                ],
            ),
        ],
        bad: vec![],
    }
}

#[test]
fn the_census_lists_one_entry_a_key_by_the_tie_rules() {
    let c = census::run(&fake()).unwrap();
    let keys: Vec<_> = c.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["coloured", "gizmo"]);
    let gizmo = c.entry("gizmo").unwrap();
    // The tag named like its directory wins over the one on as many maps.
    assert_eq!(gizmo.name, "gizmo");
    // Its source is the first map that holds it.
    assert_eq!(
        (gizmo.source.as_str(), gizmo.sha256.as_str()),
        ("m1", "a".repeat(64).as_str())
    );
    assert!(c.entry("coloured").unwrap().change_colours);
    assert!(!gizmo.change_colours);
    // The reserved key, a tag with no directory, and a bad variant name are
    // each said, not dropped.
    let said: Vec<_> = c.skipped.iter().map(|s| s.what.as_str()).collect();
    assert!(said.iter().any(|w| w.contains("marker")), "{said:?}");
    assert!(said.contains(&"loose"), "{said:?}");
    assert!(said.iter().any(|w| w.contains("BAD NAME")), "{said:?}");
    assert_eq!(gizmo.variants, ["m1_a"]);
}

#[test]
fn the_tie_rules_are_the_art_extractors() {
    let tags: BTreeMap<String, usize> = [
        ("a\\x\\foo\\zzz_long_name", 5),
        ("a\\x\\foo\\foo_b", 5),
        ("a\\x\\foo\\foo_a", 5),
        ("a\\x\\bar\\bar_mp", 1),
        ("a\\x\\bar\\bar_mpx", 3),
        ("a\\x\\baz\\baz", 1),
        ("a\\x\\baz\\baz_most", 9),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let chosen = census::choose_bipeds(&tags);
    // Neither is named like its directory: the most maps, then the
    // shortest, then the first alphabetically.
    assert_eq!(chosen["foo"], "a\\x\\foo\\foo_a");
    assert_eq!(chosen["bar"], "a\\x\\bar\\bar_mpx");
    // Named like its directory beats a tag on more maps.
    assert_eq!(chosen["baz"], "a\\x\\baz\\baz");
    assert_eq!(
        census::species_of("a\\x\\Grunt\\grunt").as_deref(),
        Some("grunt")
    );
    assert_eq!(census::species_of("solo"), None);
    assert_eq!(census::species_of("a\\Bad Name\\x"), None);
}

#[test]
fn an_entrys_identity_does_not_move_with_tag_order() {
    let a = census::run(&fake()).unwrap();
    let mut reordered = fake();
    for (_, tags) in &mut reordered.maps {
        tags.reverse();
    }
    reordered.maps[0].1.rotate_left(2);
    let b = census::run(&reordered).unwrap();
    let id = |c: &Census| -> Vec<(String, String)> {
        c.entries
            .iter()
            .map(|e| (e.key.clone(), e.sha256.clone()))
            .collect()
    };
    assert_eq!(id(&a), id(&b));
    assert_eq!(a.entries, b.entries);
}

#[test]
fn an_unreadable_model_is_skipped_with_its_reason() {
    let mut f = fake();
    f.bad = vec!["objects\\things\\coloured\\coloured"];
    let c = census::run(&f).unwrap();
    assert!(c.entry("coloured").is_none());
    assert!(c.skipped.iter().any(|s| s.reason.contains("unreadable")));
    assert!(c.entry("gizmo").is_some());
}

#[test]
fn a_weighted_region_picks_the_highest_probability_the_first_on_a_tie() {
    assert_eq!(census::pick_permutation(&[]), None);
    assert_eq!(census::pick_permutation(&[0.0]), Some(0));
    assert_eq!(census::pick_permutation(&[1.0, 3.0, 2.0]), Some(1));
    assert_eq!(census::pick_permutation(&[2.0, 5.0, 5.0]), Some(1));
    assert_eq!(census::pick_permutation(&[f32::NAN, 0.5]), Some(1));
    assert_eq!(census::pick_permutation(&[f32::NAN, f32::NAN]), Some(0));
}

#[test]
fn the_cache_key_is_the_set_of_map_hashes_and_the_census_round_trips() {
    let maps = vec![map("m1", 'a'), map("m2", 'b')];
    let key = census::cache_key(&maps);
    assert_eq!(key.len(), 64);
    let swapped = vec![map("m2", 'b'), map("m1", 'a')];
    assert_eq!(census::cache_key(&swapped), key, "order does not matter");
    assert_ne!(census::cache_key(&[map("m1", 'a'), map("m2", 'c')]), key);
    assert_ne!(census::cache_key(&[map("m1", 'a')]), key);

    let c = census::run(&fake()).unwrap();
    let dir = tmp("cache").parent().unwrap().to_path_buf();
    assert!(census::load_cached(&dir, &key).is_none());
    assert_eq!(census::load_latest(&dir), Ok(None), "no census has run");
    census::store_cached(&dir, &key, &c).unwrap();
    assert_eq!(census::load_cached(&dir, &key), Some(c.clone()));
    assert_eq!(census::load_latest(&dir), Ok(Some(c)));
    std::fs::write(dir.join(census::LATEST), "not json").unwrap();
    assert!(census::load_latest(&dir).is_err(), "said, not hidden");
    // Another version's file is read as absent.
    std::fs::write(
        census::cache_path(&dir, "old"),
        r#"{"version":0,"entries":[],"skipped":[]}"#,
    )
    .unwrap();
    assert!(census::load_cached(&dir, "old").is_none());
    std::fs::write(census::cache_path(&dir, "junk"), "not json").unwrap();
    assert!(census::load_cached(&dir, "junk").is_none());
}

// ---- the seat's look through the agent API's rules ----

use super::monitors::{Looks, Who, drawn_as};

fn who() -> Who {
    Who {
        profile: "Chief".into(),
        colour: 4,
    }
}

fn looks(bodies: bool) -> Looks {
    let catalogue = vec![
        entry("widget", &["blue", "red"], true),
        entry("plain", &["only"], false),
    ];
    Looks::new(catalogue, None).with_bodies(bodies)
}

#[test]
fn with_no_session_the_report_is_the_saved_look_and_the_catalogue() {
    let l = looks(false);
    let v = l.report(None, &who(), None).unwrap();
    assert_eq!(v["session"], false);
    assert_eq!(v["look"], MonitorLook::default_for(4).to_value());
    assert_eq!(v["saved"]["look"], v["look"]);
    assert_eq!(v["drawn_as"]["as"], "marker");
    assert_eq!(v["catalogue"].as_array().unwrap().len(), 2);
    assert_eq!(v["catalogue"][0]["key"], "widget");
    assert_eq!(v["catalogue"][0]["available"], false);
    assert_eq!(
        v["catalogue"][0]["refusal"]["code"],
        "monitor_look_renderer"
    );
    assert_eq!(v["locks"]["tint"], "The marker has no tint");
    assert!(
        l.report(None, &who(), Some(1)).is_err(),
        "no seat 1 offline"
    );
    assert!(l.report(None, &who(), Some(0)).is_ok());
    // An empty catalogue says why.
    let empty = Looks::new(vec![], Some("run h2 forge looks".into()));
    assert_eq!(
        empty.report(None, &who(), None).unwrap()["catalogue_note"],
        "run h2 forge looks"
    );
}

#[test]
fn a_set_needs_a_session_and_each_refusal_leaves_the_look() {
    let mut l = looks(true);
    assert_eq!(
        l.set(None, &who(), &json!({"tint": 1})),
        Err(Refusal::NoSession)
    );
    assert!(l.open(None, &who()).is_none());
    let before = l.session().cloned().unwrap();
    assert_eq!(before, MonitorLook::default_for(4));
    let refused = [
        (json!({}), "monitor_look_invalid"),
        (json!({"nope": 1}), "monitor_look_invalid"),
        (json!({"tint": 9}), "monitor_look_invalid"),
        (json!({"tint": 1}), "monitor_look_invalid"),
        (json!({"model": "ghost"}), "monitor_look_invalid"),
        (
            json!({"model": "widget", "variant": "green"}),
            "monitor_look_invalid",
        ),
        (json!({"reset": true, "tint": 1}), "monitor_look_invalid"),
        (json!({"reset": 1}), "monitor_look_invalid"),
        (json!({"save": true, "tint": 1}), "monitor_look_invalid"),
        (json!(7), "monitor_look_invalid"),
    ];
    for (params, code) in refused {
        let r = l.set(None, &who(), &params).unwrap_err();
        assert_eq!(r.code(), code, "{params}");
        assert_eq!(l.session(), Some(&before), "{params}");
    }
    // A model with no colour areas locks its colour rows.
    l.set(None, &who(), &json!({"model": "plain"})).unwrap();
    let locked = l.set(None, &who(), &json!({"primary": 9})).unwrap_err();
    assert_eq!(locked.code(), "monitor_look_locked");
}

#[test]
fn a_set_and_a_reset_change_the_session_look() {
    let mut l = looks(true);
    l.open(None, &who());
    let v = l
        .set(
            None,
            &who(),
            &json!({"model": "widget", "variant": "red", "primary": 9, "tint": 2, "light": 1}),
        )
        .unwrap();
    assert_eq!(v["changed"], true);
    assert_eq!(v["saved"], false);
    assert_eq!(v["look"]["model"], "widget");
    assert_eq!(v["look"]["variant"], "red");
    assert_eq!(v["look"]["secondary"], 4, "the seat's colour stays");
    assert_eq!(v["drawn_as"]["as"], "model");
    assert_eq!(l.report(None, &who(), None).unwrap()["look"], v["look"]);
    let again = l.set(None, &who(), &json!({"primary": 9})).unwrap();
    assert_eq!(again["changed"], false);
    let reset = l.set(None, &who(), &json!({"reset": true})).unwrap();
    assert_eq!(reset["look"], MonitorLook::default_for(4).to_value());
    assert_eq!(reset["changed"], true);
    l.close();
    assert_eq!(l.session(), None);
    assert_eq!(
        l.set(None, &who(), &json!({"reset": true})),
        Err(Refusal::NoSession)
    );
}

#[test]
fn save_writes_the_profile_only_when_asked() {
    let path = tmp("set-save");
    let (s, _) = store(&path);
    let mut l = looks(true);
    l.open(Some(&s), &who());
    l.set(Some(&s), &who(), &json!({"model": "widget", "tint": 1}))
        .unwrap();
    assert!(!path.exists(), "a set alone writes nothing");
    let v = l
        .set(Some(&s), &who(), &json!({"save": true, "light": 2}))
        .unwrap();
    assert_eq!(v["saved"], true);
    let saved_look = saved::load(&s, "Chief", 4).saved;
    assert_eq!(saved_look.look.model, "widget");
    assert_eq!(saved_look.look.light, 2);
    assert_eq!(saved_look.source.as_deref(), Some("a".repeat(64).as_str()));
    // With no store a save is refused, not skipped.
    let mut bare = looks(true);
    bare.open(None, &who());
    assert!(
        bare.set(None, &who(), &json!({"save": true, "tint": 0, "light": 0}))
            .is_err()
    );
}

#[test]
fn opening_a_session_uses_the_saved_look_and_says_when_it_cannot() {
    let path = tmp("open");
    let (s, _) = store(&path);
    let want = Saved {
        look: look(),
        source: Some("b".repeat(64)),
    };
    saved::save(&s, "Chief", &want).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let mut l = looks(true);
    let notice = l.open(Some(&s), &who()).unwrap();
    assert!(notice.contains("Widget needs a campaign map"), "{notice}");
    assert!(l.session().unwrap().is_marker());
    let v = l.report(Some(&s), &who(), None).unwrap();
    assert_eq!(v["notice"], notice);
    assert_eq!(v["saved"]["look"]["model"], "widget", "still saved");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn the_monitors_report_the_local_seat_and_what_others_see() {
    let mut l = looks(false);
    assert_eq!(l.monitors(&who(), false, None), json!({"monitors": []}));
    l.open(None, &who());
    l.set(None, &who(), &json!({"model": "widget"})).unwrap();
    let v = l.monitors(&who(), true, Some(([1.0, 2.0, 3.0], 0.5, -0.25)));
    let m = &v["monitors"][0];
    assert_eq!(
        (m["seat"].clone(), m["name"].clone()),
        (json!(0), json!("Chief"))
    );
    assert_eq!(m["monitor"], true);
    assert_eq!(m["pose"]["position"], json!([1.0, 2.0, 3.0]));
    assert_eq!(m["drawn_as"]["as"], "hidden");
    assert!(
        m["drawn_as"]["reason"]
            .as_str()
            .unwrap()
            .contains("own view")
    );
    assert_eq!(m["peers_see"]["as"], "marker");
    assert!(
        m["peers_see"]["reason"]
            .as_str()
            .unwrap()
            .contains("does not draw")
    );
    assert_eq!(m["look"]["model"], "widget");
    assert_eq!(m["seen_ticks_ago"], 0);
    let out = l.monitors(&who(), false, None);
    assert!(out["monitors"][0]["pose"].is_null());
    assert_eq!(out["monitors"][0]["peers_see"]["as"], "hidden");
}

#[test]
fn drawn_as_names_the_reason_for_anything_but_the_asked_look() {
    let catalogue = [entry("widget", &["blue"], true)];
    let marker = MonitorLook::default_for(1);
    assert_eq!(drawn_as(&marker, &catalogue, false).kind, "marker");
    assert_eq!(drawn_as(&marker, &catalogue, false).reason, None);
    let widget = MonitorLook {
        model: "widget".into(),
        variant: None,
        ..marker.clone()
    };
    assert_eq!(drawn_as(&widget, &catalogue, true).kind, "model");
    assert_eq!(drawn_as(&widget, &catalogue, true).reason, None);
    assert!(drawn_as(&widget, &catalogue, false).reason.is_some());
    assert!(drawn_as(&widget, &[], true).reason.is_some());
}
