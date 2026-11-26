//! Factions: the teams classes are placed on, and a definition whose
//! classes the kit cut.

use firefight::{Def, Faction, Limits, Refusal, builtin, teams, validate};

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
