//! Recipes over the invented fixture map (`crate::fixture`): parsing and
//! the checks, the anchor frame, floor drops, reuse and growth, and the
//! package a build saves.

use super::*;
use crate::fixture::fixture;
use crate::geometry::Tri;

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("scenario-recipe-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session with the fixture open as `fixture`.
fn session(d: &Path) -> Session {
    let src = d.join("fixture.map");
    std::fs::write(&src, fixture()).unwrap();
    let mut s = Session::new();
    s.build_scenes = false;
    s.set_data_root(d.join("data"));
    s.open_named(MapSource::file(&src), "fixture").unwrap();
    s
}

fn recipe(extra: &str) -> Recipe {
    Recipe::from_toml(&format!(
        "name = \"t\"\ntitle = \"T\"\npurpose = \"p\"\nbase = \"fixture\"\nfidelity = \"full\"\n{extra}"
    ))
    .unwrap()
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

#[test]
fn every_built_in_recipe_parses_and_keeps_the_rules() {
    let all = built_in().unwrap();
    assert_eq!(all.len(), BUILT_IN.len());
    for (r, (name, _)) in all.iter().zip(BUILT_IN) {
        assert_eq!(r.name, *name, "a built-in's file name is its name");
        assert_eq!(r.package_dir(), format!("test-{name}"));
        assert!(!r.place.is_empty(), "{name} places something");
        if r.fidelity == Fidelity::Approximate {
            assert!(r.note.is_some(), "{name} says what it leaves out");
        }
        // A recipe carries offsets and names, never a block of numbers
        // that could be copied geometry.
        let text = BUILT_IN.iter().find(|(n, _)| n == name).unwrap().1;
        assert!(text.len() < 4096, "{name} is a short recipe");
    }
    assert_eq!(built_in_named("spawn-grid").unwrap().name, "spawn-grid");
    let e = built_in_named("nope").unwrap_err().to_string();
    assert!(e.contains("spawn-grid"), "{e}");
}

#[test]
fn mistakes_are_refused_before_any_map_is_read() {
    let base = "title = \"T\"\npurpose = \"p\"\nbase = \"fixture\"\n";
    for (bad, says) in [
        ("name = \"Bad Name\"\nfidelity = \"full\"", "name"),
        ("name = \"t\"\nfidelity = \"approximate\"", "note"),
        ("name = \"t\"\nfidelity = \"full\"\nvariant = \"ctf\"", "variant"),
        ("name = \"t\"\nfidelity = \"full\"\ncolour = 1", "colour"),
        ("name = \"t\"\nfidelity = \"full\"\nclear = [\"tanks\"]", "tanks"),
        ("name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"tanks\"", "tanks"),
        (
            "name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"starting_locations\"\ntag = \"x\"",
            "no type",
        ),
        (
            "name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"weapons\"\nteam = 1",
            "no team",
        ),
        (
            "name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"starting_locations\"\ngrid = { rows = 2, cols = 2, spacing = 1 }\neach = {}",
            "not both",
        ),
        (
            "name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"netgame_equipment\"\neach = {}",
            "each.group",
        ),
        (
            "name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"netgame_equipment\"\neach = { group = \"scnr\" }",
            "scnr",
        ),
        (
            "name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"weapons\"\nfields = { position = [1, 2, 3] }",
            "at and facing",
        ),
        (
            "name = \"t\"\nfidelity = \"full\"\n[[place]]\nblock = \"starting_locations\"\ngrid = { rows = 17, cols = 17, spacing = 1 }",
            "grid",
        ),
        (
            "name = \"t\"\nfidelity = \"full\"\n[anchor]\nfrom = \"weapons\"\nat = [0, 0, 0]",
            "not both",
        ),
    ] {
        let e = Recipe::from_toml(&format!("{base}{bad}")).unwrap_err().to_string();
        assert!(e.contains(says), "{bad}: {e}");
    }
    // The same rules for the agent API's JSON form.
    let e = Recipe::from_json(serde_json::json!({
        "name": "t", "title": "T", "purpose": "p", "base": "fixture",
        "fidelity": "full", "place": [{"block": "weapons", "grid": {"rows": 0, "cols": 1, "spacing": 1}}]
    }))
    .unwrap_err()
    .to_string();
    assert!(e.contains("place 0"), "{e}");
}

#[test]
fn a_recipe_ports_to_another_base_unless_anchored_at_a_point() {
    let r = recipe("[[place]]\nblock = \"starting_locations\"");
    let moved = r.on_base("Burial_Mounds").unwrap();
    assert_eq!(moved.base, "burial_mounds");
    assert_eq!(moved.name, "t-burial-mounds");
    assert_eq!(r.on_base("FIXTURE").unwrap(), r);
    let pinned = recipe("[anchor]\nat = [1, 2, 3]\n[[place]]\nblock = \"starting_locations\"");
    assert!(pinned.on_base("lockout").is_err());
}

#[test]
fn the_anchor_frame_turns_forward_and_left_by_its_yaw() {
    let f = Frame {
        pos: [10.0, 20.0, 1.0],
        yaw: std::f32::consts::FRAC_PI_2,
    };
    // Facing +y: forward is +y, left is -x.
    assert!(close(f.world([2.0, 0.0, 0.0]), [10.0, 22.0, 1.0]));
    assert!(close(f.world([0.0, 3.0, 0.5]), [7.0, 20.0, 1.5]));
}

#[test]
fn a_placement_drops_onto_the_highest_floor_below_its_reach() {
    let tri = |z: f32| Tri {
        p: [[-5.0, -5.0, z], [5.0, -5.0, z], [0.0, 5.0, z]],
        n: [0.0, 0.0, 1.0],
    };
    let wall = Tri {
        p: [[0.0, -5.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, 9.0]],
        n: [1.0, 0.0, 0.0],
    };
    let tris = [tri(0.0), tri(1.5), tri(5.0), wall];
    // 1.5 is in reach from 0.2 (up to 2.2); 5.0 is not.
    assert_eq!(floor_z(&tris, 0.0, 0.0, 0.2), Some(1.5));
    assert_eq!(floor_z(&tris, 0.0, 0.0, 4.0), Some(5.0));
    assert_eq!(floor_z(&tris, 0.0, 0.0, -3.0), None);
    // Outside every triangle.
    assert_eq!(floor_z(&tris, 50.0, 0.0, 0.0), None);
    // A downward-wound floor counts too.
    let flipped = Tri {
        p: [[-5.0, -5.0, 0.0], [0.0, 5.0, 0.0], [5.0, -5.0, 0.0]],
        n: [0.0, 0.0, -1.0],
    };
    assert_eq!(floor_z(&[flipped], 1.0, 0.0, 0.0), Some(0.0));
}

#[test]
fn a_spawn_grid_reuses_the_bases_entries_from_the_anchor() {
    let d = dir("grid");
    let mut s = session(&d);
    let r = recipe(
        "clear = \"all\"\n[[place]]\nblock = \"starting_locations\"\nat = [2, 0, 0]\ngrid = { rows = 1, cols = 2, spacing = 4 }\nteams = 2",
    );
    let b = build(&mut s, &r).unwrap();
    // The fixture's first starting location: (0, -1, -2), facing 0.5.
    let a = b.anchor.unwrap();
    assert!(close(a.pos, [0.0, -1.0, -2.0]));
    assert!((a.yaw - 0.5).abs() < 1e-6);
    assert_eq!(b.counts.get("starting_locations"), Some(&2));
    assert_eq!(b.counts.len(), 1, "everything else cleared: {:?}", b.counts);
    assert_eq!(b.placed.len(), 2);
    let sc = s.scenario().unwrap();
    for (i, p) in b.placed.iter().enumerate() {
        assert_eq!(p.index, i);
        let want = a.world([2.0, if i == 0 { -2.0 } else { 2.0 }, 0.0]);
        assert!(close(p.pos, want), "{i}: {:?} {want:?}", p.pos);
        // No BSP in the fixture: the point stays.
        assert!(!p.on_floor);
        assert_eq!(p.team, Some(i as i64));
        assert_eq!(
            sc.get(BlockKind::StartingLocations, i, "position").unwrap(),
            Value::Vec3(p.pos)
        );
        assert_eq!(
            sc.get(BlockKind::StartingLocations, i, "team").unwrap(),
            Value::Int(i as i64)
        );
        assert_eq!(
            sc.get(BlockKind::StartingLocations, i, "facing").unwrap(),
            Value::Float(0.5)
        );
    }
    // Two spawns: no single-spawn check.
    assert!(b.expect.is_empty());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn growth_past_the_base_is_refused_unless_the_recipe_allows_it() {
    let d = dir("growth");
    let grid = "clear = [\"starting_locations\"]\n[[place]]\nblock = \"starting_locations\"\ngrid = { rows = 1, cols = 3, spacing = 1 }";
    let mut s = session(&d);
    let e = build(&mut s, &recipe(grid)).unwrap_err().to_string();
    assert!(e.contains("starting_locations: adding"), "{e}");
    let mut s = session(&d);
    let b = build(&mut s, &recipe(&format!("allow_growth = true\n{grid}"))).unwrap();
    assert_eq!(b.counts.get("starting_locations"), Some(&3));
    assert_eq!(b.placed[2].index, 2);
    // Blocks not cleared keep their entries.
    assert_eq!(b.counts.get("weapons"), Some(&2));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn each_places_one_item_per_matching_tag_of_the_map() {
    let d = dir("each");
    let mut s = session(&d);
    let r = recipe(
        "clear = \"all\"\n[[place]]\nblock = \"starting_locations\"\n[[place]]\nblock = \"netgame_equipment\"\nat = [3, 0, 0]\nstep = [0, 2, 0]\neach = { group = \"weap\" }",
    );
    let b = build(&mut s, &r).unwrap();
    let sc = s.scenario().unwrap();
    assert_eq!(b.counts.get("netgame_equipment"), Some(&1));
    let item = b.placed.iter().find(|p| p.block == "netgame_equipment").unwrap();
    assert_eq!(item.tag.as_deref(), Some("objects\\test\\blaster"));
    match sc.get(BlockKind::NetgameEquipment, 0, "item").unwrap() {
        Value::Tag(t) => {
            assert_eq!(t.group.0, *b"weap");
            assert_eq!(t.id, crate::fixture::id(2));
        }
        v => panic!("{v:?}"),
    }
    // One spawn: its point is a check for sim.expect.
    let spawn = b.placed.iter().find(|p| p.block == "starting_locations").unwrap();
    assert_eq!(b.expect.len(), 1);
    assert_eq!(b.expect[0]["pointer"], "/player/0/pos");
    assert_eq!(b.expect[0]["want"]["to"], serde_json::json!(spawn.pos));
    // A filter nothing passes names the placement.
    let mut s = session(&d);
    let none = recipe(
        "[[place]]\nblock = \"netgame_equipment\"\neach = { group = \"weap\", exclude = [\"blaster\"] }",
    );
    let e = build(&mut s, &none).unwrap_err().to_string();
    assert!(e.contains("place 0"), "{e}");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_tag_picks_a_palette_entry_and_fields_are_set_by_name() {
    let d = dir("tag");
    let mut s = session(&d);
    let r = recipe(
        "clear = [\"weapons\"]\n[[place]]\nblock = \"weapons\"\ntag = \"BLASTER\"\nat = [1, 0, 0]\nfacing = 90\nfields = { scale = 2 }\n[[set]]\nblock = \"netgame_flags\"\nindex = 0\nfields = { team = 3 }",
    );
    let b = build(&mut s, &r).unwrap();
    let sc = s.scenario().unwrap();
    assert_eq!(b.counts.get("weapons"), Some(&1));
    assert_eq!(sc.get(BlockKind::Weapons, 0, "palette index").unwrap(), Value::Int(0));
    assert_eq!(sc.get(BlockKind::Weapons, 0, "scale").unwrap(), Value::Float(2.0));
    let yaw = 0.5 + std::f32::consts::FRAC_PI_2;
    match sc.get(BlockKind::Weapons, 0, "rotation").unwrap() {
        Value::Vec3(v) => assert!((v[0] - yaw).abs() < 1e-5 && v[1] == 0.0 && v[2] == 0.0),
        v => panic!("{v:?}"),
    }
    assert_eq!(sc.get(BlockKind::NetgameFlags, 0, "team").unwrap(), Value::Int(3));
    assert_eq!(b.done[0], "set netgame_flags 0");
    assert!(
        b.done.iter().any(|l| l == "weapons: 1 placed (1 reused, 0 added), 1 removed"),
        "{:?}",
        b.done
    );
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_built_package_resolves_to_the_rig() {
    let d = dir("package");
    let src = d.join("fixture.map");
    let bytes = fixture();
    std::fs::write(&src, &bytes).unwrap();
    let data = d.join("data");
    let r = recipe("clear = \"all\"\n[[place]]\nblock = \"starting_locations\"\nat = [1, 0, 0]");
    let (b, saved) = build_package(MapSource::file(&src), &r, &data).unwrap();
    assert!(saved.dir.ends_with(Path::new("maps").join("test-t")), "{:?}", saved.dir);
    assert_eq!(saved.manifest.base.map, "fixture");
    assert_eq!(saved.manifest.package.name, "Test: T");
    assert!(!saved.manifest.base.derived);
    // The package served over the base is the rig.
    let mut lib = content::Library::open(&data.join("maps"));
    let served = lib
        .resolve("fixture", &mut &bytes[..])
        .unwrap()
        .unwrap()
        .read_all(&mut &bytes[..])
        .unwrap();
    let mut m = map::Map::from_reader(std::io::Cursor::new(served)).unwrap();
    let sc = Scenario::read(&mut m).unwrap();
    assert_eq!(sc.block(BlockKind::StartingLocations).len(), 1);
    assert_eq!(sc.block(BlockKind::Weapons).len(), 0);
    assert_eq!(
        sc.get(BlockKind::StartingLocations, 0, "position").unwrap(),
        Value::Vec3(b.placed[0].pos)
    );
    // A second build into the same directory is refused; the caller
    // removes the old one first.
    assert!(build_package(MapSource::file(&src), &r, &data).is_err());
    std::fs::remove_dir_all(&d).unwrap();
}
