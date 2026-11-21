use firefight::{Count, Def, Drop, FORMAT, Limits, Refusal, Skull, SquadSpec, builtin, validate};

fn elite_kit() -> Limits {
    Limits::new(["elite"])
}

#[test]
fn golden_hash_for_the_builtin_defs() {
    let expected = [
        (
            "Holdout",
            "f5f343c25a973698a88bc7d862870afd0eb1aa1b87dfd2a4f4a6ffda02ffaca7",
        ),
        (
            "Attrition",
            "80ce34231906cb5b988fe8ad1e68d2d0826e18b5d32d027206b7ded0485eb2a7",
        ),
        (
            "Last Stand",
            "807b010dc2121ee96c814d1ac3cac84b47fe8f5192c68e26e771a510878798ca",
        ),
    ];
    let limits = elite_kit();
    for (def, (name, hash)) in builtin::definitions().into_iter().zip(expected) {
        assert_eq!(def.name, name);
        validate(&def, &limits).unwrap();
        assert_eq!(def.id().unwrap().to_string(), hash, "{name}");
        let json = def.canonical_json().unwrap();
        assert_eq!(Def::from_bytes(json.as_bytes(), &limits).unwrap(), def);
    }
}

#[test]
fn a_wave_over_the_cap_is_refused() {
    let [mut def, _, _] = builtin::definitions();
    def.rounds.truncate(1);
    def.caps.live_actors_by_players = vec![(4, 12), (8, 9), (16, 16)];
    def.rounds[0].waves[0].squads[0].count = Count {
        per_player: 2,
        min: 1,
        max: 10,
    };
    // Fits at one and sixteen players, but crosses the middle tier at five.
    assert_eq!(
        validate(&def, &elite_kit()),
        Err(Refusal::WaveOverCap {
            round: 1,
            wave: 1,
            players: 5,
            actors: 10,
            cap: 9
        }),
    );

    // Two individually valid squads must still fit together.
    def.caps.live_actors_by_players = vec![(4, 32), (8, 24), (16, 16)];
    let second = def.rounds[0].waves[0].squads[0].clone();
    def.rounds[0].waves[0].squads.push(second);
    assert_eq!(
        validate(&def, &elite_kit()),
        Err(Refusal::WaveOverCap {
            round: 1,
            wave: 1,
            players: 9,
            actors: 20,
            cap: 16
        }),
    );
    def.rounds[0].waves[0]
        .squads
        .iter_mut()
        .for_each(|squad| squad.count.max = 8);
    validate(&def, &elite_kit()).unwrap();
}

#[test]
fn an_unknown_class_is_refused() {
    let [mut def, _, _] = builtin::definitions();
    def.rounds[0].waves[0].squads[0].class = "fixture".into();
    def.scoring.by_class.insert("fixture".into(), 500);
    let canonical = def.canonical_json().unwrap();
    assert_eq!(
        Def::from_bytes(canonical.as_bytes(), &elite_kit()),
        Err(Refusal::UnknownClass("fixture".into())),
    );
    let matching = Limits::new(["elite", "fixture"]);
    assert_eq!(
        Def::from_bytes(canonical.as_bytes(), &matching).unwrap(),
        def
    );
}

#[test]
fn canonical_form_is_stable_under_key_order() {
    let [mut def, _, _] = builtin::definitions();
    let canonical = def.canonical_json().unwrap();
    let reordered = serde_json::to_string_pretty(&def).unwrap();
    assert!(canonical.starts_with(r#"{"caps":"#));
    assert!(reordered.starts_with("{\n  \"format\":"));
    // The derived serializer orders skull keys by enum order. Canonical JSON
    // sorts these keys by their spelling as it does for every object.
    assert!(canonical.find("\"blind\":") < canonical.find("\"thunderstorm\":"));
    assert!(reordered.find("\"thunderstorm\":") < reordered.find("\"blind\":"));
    let decoded = Def::from_bytes(reordered.as_bytes(), &elite_kit()).unwrap();
    assert_eq!(decoded.canonical_json().unwrap(), canonical);
    assert_eq!(decoded.id().unwrap(), def.id().unwrap());

    let original_id = def.id().unwrap();
    def.rounds[0].waves.swap(0, 2);
    assert_ne!(def.id().unwrap(), original_id);
}

#[test]
fn count_scaling_uses_both_bounds() {
    let count = Count {
        per_player: 2,
        min: 3,
        max: 9,
    };
    assert_eq!(count.for_players(1), 3);
    assert_eq!(count.for_players(3), 6);
    assert_eq!(count.for_players(16), 9);
    assert_eq!(
        Count {
            per_player: u8::MAX,
            ..count
        }
        .for_players(16),
        9
    );
    assert_eq!(
        Count {
            per_player: 0,
            ..count
        }
        .for_players(16),
        3
    );
}

#[test]
fn cap_tiers_are_inclusive_and_may_cover_a_smaller_lobby() {
    let [mut def, _, _] = builtin::definitions();
    def.caps.live_actors_by_players.pop();
    assert_eq!(def.caps.for_players(0), None);
    assert_eq!(def.caps.for_players(1), Some(12));
    assert_eq!(def.caps.for_players(8), Some(12));
    assert_eq!(def.caps.for_players(9), None);
    validate(&def, &elite_kit()).unwrap();
}

fn refuse(doc: &Def, field: &str) {
    match validate(doc, &elite_kit()) {
        Err(Refusal::Invalid { field: found, .. } | Refusal::Limit { field: found, .. }) => {
            assert_eq!(found, field);
        }
        result => panic!("expected refusal for {field}, got {result:?}"),
    }
}

#[test]
fn invalid_headers_and_schedule_are_refused() {
    let [mut doc, _, _] = builtin::definitions();
    doc.format = FORMAT + 1;
    assert_eq!(
        validate(&doc, &elite_kit()),
        Err(Refusal::Format(FORMAT + 1))
    );
    assert_eq!(doc.id(), Err(Refusal::Format(FORMAT + 1)));

    let cases: [(fn(&mut Def), &str); 13] = [
        (|d| d.name = " ".into(), "name"),
        (|d| d.name.push('\0'), "name"),
        (|d| d.name = "é".repeat(65), "name bytes"),
        (|d| d.rounds.clear(), "rounds"),
        (|d| d.rounds.resize(13, d.rounds[0].clone()), "rounds"),
        (
            |d| {
                d.rounds[0].waves.pop();
            },
            "waves",
        ),
        (|d| d.rounds[0].waves[0].boss = true, "boss"),
        (|d| d.rounds[0].waves[4].boss = false, "boss"),
        (|d| d.rounds[0].skulls_on.push(Skull::Blind), "skulls_on"),
        (|d| d.rounds[1].skulls_on = vec![Skull::Blind], "skulls_on"),
        (
            |d| d.rounds[0].skulls_on = vec![Skull::Blind; 10],
            "skulls_on",
        ),
        (|d| d.lives.pool_start = 1_001, "lives"),
        (|d| d.lives.per_round_bonus = 1_001, "lives"),
    ];
    for (change, field) in cases {
        let [mut doc, _, _] = builtin::definitions();
        change(&mut doc);
        refuse(&doc, field);
    }
}

#[test]
fn cap_tiers_cannot_bypass_the_host_budget() {
    for tiers in [
        vec![],
        vec![(0, 12)],
        vec![(4, 12), (4, 12)],
        vec![(8, 12), (4, 12)],
        vec![(17, 12)],
        vec![(4, 0)],
        vec![(4, 33)],
        vec![(5, 25)],
        vec![(9, 17)],
        vec![(16, 17)],
        vec![(1, 1); 17],
    ] {
        let [mut doc, _, _] = builtin::definitions();
        doc.caps.live_actors_by_players = tiers;
        refuse(&doc, "actor cap tiers");
    }
}

#[test]
fn malformed_waves_are_refused() {
    let cases: [(fn(&mut Vec<SquadSpec>), &str); 10] = [
        (Vec::clear, "squads"),
        (|s| s.resize(17, s[0].clone()), "squads"),
        (|s| s[0].class = "../elite".into(), "class"),
        (|s| s[0].class = "a".repeat(33), "class bytes"),
        (|s| s[0].class = "fixture".into(), "class"),
        (|s| s[0].count.min = 0, "count"),
        (|s| s[0].count.min = s[0].count.max + 1, "count"),
        (|s| s[0].count.max = 33, "count"),
        (|s| s[0].drop = Drop::PreferZone(12), "drop"),
        (|s| s[0].weapon_tier = 4, "weapon_tier"),
    ];
    for (change, field) in cases {
        let [mut doc, _, _] = builtin::definitions();
        change(&mut doc.rounds[0].waves[0].squads);
        refuse(&doc, field);
    }

    let [mut doc, _, _] = builtin::definitions();
    doc.rounds[0].waves[0].gap_ticks = 9_001;
    refuse(&doc, "gap_ticks");
}

#[test]
fn scoring_bounds_are_enforced() {
    let cases: [(fn(&mut Def), &str); 7] = [
        (
            |d| {
                d.scoring.by_class.insert("ELITE".into(), 10);
            },
            "class",
        ),
        (
            |d| *d.scoring.by_class.get_mut("elite").unwrap() = 0,
            "by_class",
        ),
        (
            |d| *d.scoring.by_class.get_mut("elite").unwrap() = 1_001,
            "by_class",
        ),
        (
            |d| d.scoring.difficulty_multiplier.easy = 0,
            "scoring multiplier",
        ),
        (
            |d| d.scoring.difficulty_multiplier.legendary = 2_001,
            "scoring multiplier",
        ),
        (
            |d| {
                d.scoring.skull_multipliers.insert(Skull::Blind, 2_001);
            },
            "scoring multiplier",
        ),
        (|d| d.scoring.assist = 1_001, "assist"),
    ];
    for (change, field) in cases {
        let [mut doc, _, _] = builtin::definitions();
        change(&mut doc);
        refuse(&doc, field);
    }

    let [mut doc, _, _] = builtin::definitions();
    for i in 0..32 {
        doc.scoring.by_class.insert(format!("fixture_{i}"), 1);
    }
    refuse(&doc, "scoring classes");
    doc.scoring
        .by_class
        .retain(|class, _| class == "elite" || class == "fixture_0");
    assert_eq!(
        validate(&doc, &elite_kit()),
        Err(Refusal::UnknownClass("fixture_0".into()))
    );
}
