use crate::manifest::{check, manifest_for, scenario_path};

#[test]
fn halo1_manifest_lists_every_level_with_its_scenario_path() {
    for level in [
        "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
    ] {
        let text = manifest_for(None, level).unwrap();
        check(&text, level).unwrap();
    }
    assert_eq!(scenario_path("A30"), "scenarios\\solo\\a30\\a30");
}

#[test]
fn a_manifest_without_the_level_or_with_another_path_is_refused() {
    let text = content::campaign_list::HALO1;
    assert!(check(text, "zz9").is_err());
    let moved = text.replacen(
        "map = \"a30\"",
        "map = \"a30\"\nscenario = \"scenarios\\\\solo\\\\x\\\\a30\"",
        1,
    );
    assert!(check(&moved, "a30").is_err());
    // A broken existing manifest falls back to Halo 1's.
    assert_eq!(manifest_for(Some("junk"), "a30").unwrap(), text);
}

#[test]
fn on_sector_finds_a_point_over_a_square() {
    use tags::pathfinding::{Link, PathfindingData, Sector};
    let mut pf = PathfindingData {
        sectors: Vec::new(),
        links: Vec::new(),
        refs: Vec::new(),
        bsp2d_nodes: Vec::new(),
        vertices: vec![
            [0.0, 0.0, 1.0],
            [2.0, 0.0, 1.0],
            [2.0, 2.0, 1.0],
            [0.0, 2.0, 1.0],
        ],
        object_refs: Vec::new(),
        hints: Vec::new(),
        instance_refs: Vec::new(),
        checksum: 0,
        user_hints: Vec::new(),
    };
    for i in 0..4u16 {
        pf.links.push(Link {
            vertex1: i,
            vertex2: (i + 1) % 4,
            flags: 0,
            hint: u16::MAX,
            forward_link: (i + 1) % 4,
            reverse_link: i,
            left_sector: 0,
            right_sector: u16::MAX,
        });
    }
    pf.sectors.push(Sector {
        flags: 0,
        hint: u16::MAX,
        first_link: 0,
    });
    assert!(crate::on_sector(&pf, [1.0, 1.0, 1.1], 0.5, 0.25));
    assert!(
        !crate::on_sector(&pf, [1.0, 1.0, 3.0], 0.5, 0.25),
        "too far above"
    );
    assert!(
        !crate::on_sector(&pf, [3.0, 1.0, 1.0], 0.5, 0.25),
        "outside"
    );
}

#[test]
fn hooks_take_retail_encodings() {
    use crate::scenario::{Targets, hook_value};
    use crate::scripts::compile::Hook;
    let hook = |ty: &str, token: &str| Hook {
        slot: 0,
        h2_type: 0,
        type_name: ty.into(),
        h1_type: String::new(),
        token: token.into(),
    };
    let names = vec!["lz_cship".to_string(), "hog".to_string()];
    let volumes = ["first", "second"];
    let squads = ["enc/a".to_string()];
    let groups = ["enc".to_string()];
    let t = Targets {
        names: &names,
        placed: &[true, true],
        volumes: &volumes,
        parked: 10,
        squads: &squads,
        groups: &groups,
    };
    assert_eq!(
        hook_value(&hook("object_name", "hog"), &t),
        Some(0xffff_0001)
    );
    assert_eq!(hook_value(&hook("unit", "lz_cship"), &t), Some(0xffff_0000));
    assert_eq!(hook_value(&hook("object", "none"), &t), Some(u32::MAX));
    assert_eq!(hook_value(&hook("trigger_volume", "second"), &t), Some(11));
    assert_eq!(
        hook_value(&hook("short", "unit_exit_vehicle:0"), &t),
        Some(0xffff_0000)
    );
    assert_eq!(
        hook_value(&hook("short", "unit_exit_vehicle:1"), &t),
        Some(0xffff_0001)
    );
    assert_eq!(hook_value(&hook("team", "covenant"), &t), Some(0xffff_0003));
    assert_eq!(
        hook_value(&hook("game_difficulty", "impossible"), &t),
        Some(0xffff_0003)
    );
    assert_eq!(hook_value(&hook("ai", "enc/a"), &t), Some(0));
    assert_eq!(hook_value(&hook("ai", "enc"), &t), Some(0x4000_0000));
    assert_eq!(hook_value(&hook("ai", "other"), &t), None);
    assert_eq!(hook_value(&hook("cutscene_flag", "x"), &t), None);
}
