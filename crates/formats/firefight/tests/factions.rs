//! Factions: the teams classes are placed on, and a definition whose
//! classes the kit cut.

use firefight::{Count, Def, Drop, Faction, Limits, Refusal, Side, SquadSpec, builtin, teams, validate};

/// A built-in definition whose first squad is `class`, scored.
fn naming(class: &str) -> Def {
    let [mut def, ..] = builtin::definitions();
    def.rounds[0].waves[0].squads[0].class = class.into();
    def.scoring.by_class.insert(class.into(), 500);
    def
}

#[test]
fn each_faction_has_its_own_engine_team() {
    assert_eq!(Faction::Covenant.team(), 3);
    assert_eq!(Faction::Flood.team(), 4);
    assert_eq!(Faction::Human.team(), 2);
    let mut seen = Vec::new();
    for faction in Faction::ALL {
        assert_eq!(Faction::from_team(faction.team()), Some(faction));
        assert!(!seen.contains(&faction.team()));
        seen.push(faction.team());
    }
    // The player teams and the engine's default team are no faction.
    for team in [0, 1, 5, 255] {
        assert_eq!(Faction::from_team(team), None);
    }
    assert_eq!(teams::FLOOD, Faction::Flood.team());
}

#[test]
fn a_set_naming_a_cut_class_is_refused_by_name() {
    let def = naming("flood_carrier");
    let kit = Limits::new(["elite"]).with_cut(["flood_carrier"]);
    assert_eq!(
        validate(&def, &kit),
        Err(Refusal::CutClass("flood_carrier".into()))
    );
    assert!(
        Refusal::CutClass("flood_carrier".into())
            .to_string()
            .contains("flood_carrier")
    );
    // A class the kit never had is unknown, not cut, and one it has passes.
    assert_eq!(
        validate(&def, &Limits::new(["elite"])),
        Err(Refusal::UnknownClass("flood_carrier".into()))
    );
    assert_eq!(validate(&def, &Limits::new(["elite", "flood_carrier"])), Ok(()));
}

/// A built-in definition whose first round has `allies`.
fn allied(allies: Vec<SquadSpec>) -> Def {
    let [mut def, ..] = builtin::definitions();
    def.rounds[0].allies = allies;
    // Allies count toward the actor cap: room for the most the rules allow
    // (four), so these checks reach the allies' own rules.
    for (_, cap) in &mut def.caps.live_actors_by_players {
        *cap += 4;
    }
    def
}

fn ally(class: &str, count: u16) -> SquadSpec {
    SquadSpec {
        class: class.into(),
        count: Count { per_player: 0, min: count, max: count },
        drop: Drop::Edge,
        weapon_tier: 0,
        side: Side::Ally,
        arrival: ::firefight::Arrival::Auto,
    }
}

fn invalid(def: &Def, kit: &Limits, field: &'static str) {
    assert!(
        matches!(validate(def, kit), Err(Refusal::Invalid { field: found, .. }) if found == field),
        "{:?}",
        validate(def, kit)
    );
}

#[test]
fn allies_are_human_unscored_and_at_most_four() {
    let kit = Limits::new(["elite", "marine", "grunt"])
        .with_factions([("marine", Faction::Human), ("grunt", Faction::Covenant)]);
    assert_eq!(validate(&allied(vec![ally("marine", 4)]), &kit), Ok(()));
    // A kit that does not say a class's faction admits it.
    let silent = Limits::new(["elite", "marine"]);
    assert_eq!(validate(&allied(vec![ally("marine", 2)]), &silent), Ok(()));

    invalid(&allied(vec![ally("grunt", 2)]), &kit, "allies");
    invalid(&allied(vec![ally("marine", 3), ally("marine", 2)]), &kit, "allies");
    // An ally class is not scored and is no wave's enemy.
    invalid(&allied(vec![ally("elite", 2)]), &kit, "allies");
    let mut scored = allied(vec![ally("marine", 2)]);
    scored.scoring.by_class.insert("marine".into(), 50);
    invalid(&scored, &kit, "allies");
    // The sides are kept apart: an ally in a round's list, an enemy in a wave.
    let mut enemy = ally("marine", 2);
    enemy.side = Side::Enemy;
    invalid(&allied(vec![enemy.clone()]), &kit, "side");
    let mut waved = allied(vec![]);
    waved.rounds[0].waves[0].squads.push(ally("marine", 1));
    invalid(&waved, &kit, "side");
    // The kit must have it, and not have cut it.
    assert_eq!(
        validate(&allied(vec![ally("marine", 2)]), &Limits::new(["elite"])),
        Err(Refusal::UnknownClass("marine".into()))
    );
    assert_eq!(
        validate(&allied(vec![ally("marine", 2)]), &silent.clone().with_cut(["marine"])),
        Err(Refusal::CutClass("marine".into()))
    );
}

#[test]
fn two_teams_have_one_ally_squad_each() {
    let kit = Limits::new(["elite", "marine"]);
    let mut def = builtin::with_teams(allied(vec![ally("marine", 1); 3]), 2);
    invalid(&def, &kit, "allies");
    def.rounds[0].allies.truncate(2);
    assert_eq!(validate(&def, &kit), Ok(()));
}

#[test]
fn a_definition_without_allies_keeps_its_id() {
    // Neither field is written when it is the default.
    let json = builtin::definitions()[0].canonical_json().unwrap();
    assert!(!json.contains("allies") && !json.contains("side"));
}
