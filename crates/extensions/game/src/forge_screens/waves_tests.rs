//! The Waves screens on an invented set (`sample_facts`): the pages'
//! presses, what each value sends, the pickers' values, the questions and
//! the pages built again after an edit. No game, no disc.
use super::*;

use crate::ui_kit::input::Input;
use crate::ui_kit::page::{ListEffect, Page as _, named, run};
use crate::forge_screens::pages::SheetPage;
use crate::forge_screens::{Facts as ScreenFacts, publish};

/// The session's facts with the sample set, as a Forge session publishes
/// them, so a page's row opens the page above it.
fn publish_sample() -> Facts {
    let f = sample_facts();
    publish(ScreenFacts {
        active: true,
        waves: Some(f.clone()),
        ..ScreenFacts::default()
    });
    f
}

fn squad_at(round: u8, wave: u8, squad: u8) -> At {
    At { round, wave, squad }
}

/// Each Waves page's presses from a named state: the effects, then the
/// focused row. No row draws arrows: a sideways step does nothing.
#[test]
fn page_tables() {
    use Input::{A, B, Down, Left, Right};
    use ListEffect::{Action, Close, Focus, None as Nothing, Open};
    publish_sample();
    let picker = crate::ui_kit::picker::NAME;
    struct Case {
        kind: Kind,
        from: &'static str,
        inputs: &'static [Input],
        effects: Vec<ListEffect>,
        after: &'static str,
    }
    for case in [
        // Start from opens the picker over the built-in sets.
        Case {
            kind: Kind::Waves,
            from: "open",
            inputs: &[Down, A],
            effects: vec![Focus(1), Open(picker)],
            after: "YARD | focus 1 Start from...",
        },
        // Rounds... opens the Rounds list.
        Case {
            kind: Kind::Waves,
            from: "open",
            inputs: &[Down, Down, Down, Down, Down, Down, Down, A],
            effects: vec![
                Focus(1),
                Focus(2),
                Focus(3),
                Focus(4),
                Focus(5),
                Focus(6),
                Focus(7),
                Open(Kind::Rounds.screen()),
            ],
            after: "YARD | focus 7 Rounds... = 2",
        },
        // Reset waves asks first.
        Case {
            kind: Kind::Waves,
            from: "last",
            inputs: &[A, B],
            effects: vec![Open(Kind::Question.screen()), Close],
            after: "YARD | focus 10 Reset waves",
        },
        Case {
            kind: Kind::Rounds,
            from: "open",
            inputs: &[A],
            effects: vec![Open(Kind::Round.screen())],
            after: "ROUNDS | focus 0 Round 1",
        },
        Case {
            kind: Kind::Rounds,
            from: "add",
            inputs: &[A],
            effects: vec![Action],
            after: "ROUNDS | focus 2 Add round",
        },
        Case {
            kind: Kind::Round,
            from: "open",
            inputs: &[Left, Right, A, Down, A],
            effects: vec![Nothing, Nothing, Open(picker), Focus(1), Open(Kind::Wave.screen())],
            after: "ROUND 1 | focus 1 Wave 1 = 4 to 8 enemies",
        },
        Case {
            kind: Kind::Wave,
            from: "open",
            inputs: &[A, Down, A],
            effects: vec![Open(picker), Focus(1), Open(Kind::Squads.screen())],
            after: "WAVE 1 | focus 1 Squads... = 2",
        },
        // DROP ZONE: Add squad here... opens the picker of waves.
        Case {
            kind: Kind::Wave,
            from: "drop",
            inputs: &[Down, A],
            effects: vec![Focus(1), Open(picker)],
            after: "DROP ZONE 1 | focus 1 Add squad here...",
        },
        Case {
            kind: Kind::Squads,
            from: "open",
            inputs: &[A],
            effects: vec![Open(Kind::Squad.screen())],
            after: "SQUADS | focus 0 Squad 1",
        },
        Case {
            kind: Kind::Squad,
            from: "open",
            inputs: &[Left, Right, A, Down, A, B],
            effects: vec![Nothing, Nothing, Open(picker), Focus(1), Open(picker), Close],
            after: "SQUAD 1 | focus 1 For each player = 1",
        },
        Case {
            kind: Kind::WaveProblems,
            from: "open",
            inputs: &[A],
            effects: vec![Open(Kind::Squad.screen())],
            after: "PROBLEMS | focus 0 Round 1, wave 1, squad 2",
        },
        Case {
            kind: Kind::EnemyTypes,
            from: "open",
            inputs: &[Down, A],
            effects: vec![Focus(1), Open(picker)],
            after: "ENEMY TYPES | focus 1 Grunt = On",
        },
    ] {
        let page = SheetPage::new(case.kind);
        let mut state = named(&page, case.from);
        assert_eq!(
            run(&page, &mut state, case.inputs.iter().copied()),
            case.effects,
            "{:?} from {}",
            case.kind,
            case.from
        );
        assert_eq!(page.summary(&state), case.after, "{:?}", case.kind);
    }
    publish(ScreenFacts::default());
}

/// Every page of the sample, every round, wave and squad.
fn every_page(f: &Facts) -> Vec<Sheet> {
    let mut pages = vec![waves_sheet(f), rounds_sheet(f)];
    pages.extend(problems_sheet(f));
    for (r, round) in f.set.rounds.iter().enumerate() {
        let r = r as u8;
        pages.extend(round_sheet(f, r));
        for (w, wave) in round.waves.iter().enumerate() {
            let w = w as u8;
            pages.extend(wave_sheet(f, r, w));
            pages.extend(squads_sheet(f, r, w));
            for s in 0..wave.squads.len() {
                pages.extend(squad_sheet(f, squad_at(r, w, s as u8)));
            }
        }
    }
    pages
}

/// Each value of each page sends one choice, a Waves edit: the whole set
/// changed in one place. Applied, the set is admitted as a document's is,
/// or refused by name; the value the set holds gives the set back.
#[test]
fn each_row_is_one_set_waves() {
    let f = sample_facts();
    let mut edits = 0;
    for page in every_page(&f) {
        for row in &page.rows {
            let choices: Vec<&Choice> = match &row.act {
                Act::Pick(p) => {
                    assert_eq!(p.values.len(), p.choices.len(), "{}: {}", page.title, row.label);
                    p.choices.iter().collect()
                }
                Act::Choose(c) => vec![c],
                _ => continue,
            };
            for choice in choices {
                let edit = match choice {
                    Choice::Waves(edit) => edit,
                    Choice::TestWaves | Choice::StopWaves => continue,
                    other => panic!("{}: {}: {other:?} is no Waves edit", page.title, row.label),
                };
                edits += 1;
                match apply(&f.set, edit, &f.presets) {
                    Ok(def) => assert!(
                        forge_doc::waves::check(&def).is_ok(),
                        "{}: {}: {edit:?}",
                        page.title,
                        row.label
                    ),
                    Err(why) => assert!(!why.is_empty(), "{edit:?}"),
                }
            }
            // The value shown is the set's: choosing it changes nothing.
            if let Act::Pick(p) = &row.act
                && row.value.is_some()
                && p.values.get(p.current) == row.value.as_ref()
                && let Some(Choice::Waves(edit)) = p.choices.get(p.current)
                && !matches!(edit, Edit::StartFrom(_))
            {
                assert_eq!(
                    apply(&f.set, edit, &f.presets).as_ref(),
                    Ok(&f.set),
                    "{}: {}",
                    page.title,
                    row.label
                );
            }
        }
    }
    assert!(edits > 100, "{edits} edits");
}

/// A squad that takes a wave over the limit is refused with the
/// validator's sentence.
#[test]
fn a_wave_over_the_enemy_limit_is_refused_with_its_sentence() {
    let f = sample_facts();
    let at = squad_at(0, 0, 0);
    // Sixteen Elites and the two Grunts with one player, over the 16.
    let why = apply(&f.set, &Edit::AtLeast(at, 16), &f.presets).unwrap_err();
    assert!(
        why.contains("round 1, wave 1 needs 18 actors for 1 players (cap 16)"),
        "{why}"
    );
    // Raising At least past At most takes At most with it; lowering At
    // most takes At least down.
    let squad = |d: &Def| d.rounds[0].waves[0].squads[0].count;
    let up = apply(&f.set, &Edit::AtLeast(at, 6), &f.presets).unwrap();
    assert_eq!((squad(&up).min, squad(&up).max), (6, 6));
    let down = apply(&f.set, &Edit::AtMost(at, 1), &f.presets).unwrap();
    assert_eq!((squad(&down).min, squad(&down).max), (1, 1));
}

/// A refused value goes back: the page is built again from the set.
#[test]
fn a_refused_value_goes_back_on_the_page() {
    let f = publish_sample();
    let at = squad_at(0, 0, 0);
    open(Page::Squad(at)).unwrap();
    let shown = screen(Kind::Squad);
    // The picker wrote the value it chose into the row.
    let mut sheet = shown.sheet();
    sheet.rows[1].value = Some("8".into());
    shown.show(sheet).unwrap();
    stale();
    publish(ScreenFacts {
        active: true,
        waves: Some(f),
        ..ScreenFacts::default()
    });
    assert_eq!(shown.sheet().rows[1].value.as_deref(), Some("1"));
    publish(ScreenFacts::default());
}

/// A page whose part is gone is dropped when the set changes.
#[test]
fn a_removed_round_drops_its_page() {
    let mut f = publish_sample();
    open(Page::Round(1)).unwrap();
    assert_eq!(shown(Kind::Round), Some(Page::Round(1)));
    f.set = apply(&f.set, &Edit::RemoveRound(1), &f.presets).unwrap();
    publish(ScreenFacts {
        active: true,
        waves: Some(f),
        ..ScreenFacts::default()
    });
    assert_eq!(shown(Kind::Round), None);
    publish(ScreenFacts::default());
}

#[test]
fn the_class_picker_offers_the_built_classes_only() {
    let f = sample_facts();
    let grunt = squad_sheet(&f, squad_at(0, 0, 1)).unwrap();
    let Act::Pick(p) = &grunt.rows[0].act else {
        panic!("Enemy opens the picker")
    };
    assert_eq!(p.values, ["Elite"], "the stage-one ground's classes");
    assert_eq!(
        p.choices,
        [Choice::Waves(Edit::Enemy(squad_at(0, 0, 1), "elite".into()))]
    );
    assert_eq!(grunt.rows[0].value.as_deref(), Some("Grunt"), "what it names");
    // A class the set has no points for takes its points as it is named.
    let mut def = f.set.clone();
    def.scoring.by_class.remove("grunt");
    def.rounds[0].waves[0].squads[1].class = "elite".into();
    let named = apply(
        &def,
        &Edit::Enemy(squad_at(0, 0, 1), "jackal".into()),
        &f.presets,
    )
    .unwrap();
    assert!(named.scoring.by_class.contains_key("jackal"));
}

#[test]
fn the_drop_picker_lists_any_drop_then_each_placed_drop() {
    let f = sample_facts();
    let at = squad_at(0, 0, 1);
    let grunt = squad_sheet(&f, at).unwrap();
    let drop = grunt.rows.iter().find(|r| r.label == "Drop").unwrap();
    let Act::Pick(p) = &drop.act else {
        panic!("Drop opens the picker")
    };
    assert_eq!(p.values, ["Any drop", "Drop zone 0", "Drop zone 1"]);
    assert_eq!(
        p.choices,
        [
            Choice::Waves(Edit::Drop(at, Drop::Edge)),
            Choice::Waves(Edit::Drop(at, Drop::PreferZone(0))),
            Choice::Waves(Edit::Drop(at, Drop::PreferZone(1))),
        ]
    );
    assert_eq!(
        drop.value.as_deref(),
        Some("Drop zone 3"),
        "the zone it prefers, though none is placed"
    );
    let elite = squad_sheet(&f, squad_at(0, 0, 0)).unwrap();
    let drop = elite.rows.iter().find(|r| r.label == "Drop").unwrap();
    assert_eq!(drop.value.as_deref(), Some("Any drop"));
}

#[test]
fn start_from_asks_before_replacing() {
    let mut f = sample_facts();
    assert!(f.edited);
    let Start::Ask(question) = start_from(&f, 1).unwrap() else {
        panic!("an edited set is asked about")
    };
    assert_eq!(question.kind, Kind::Question);
    assert!(question.body.contains("Attrition"), "{}", question.body);
    assert_eq!(question.rows[question.focus].label, "No");
    let Act::Choose(Choice::Answer { ask: Ask::Waves(ask), yes: true }) = question.rows[0].act
    else {
        panic!("Yes answers the question")
    };
    let edit = answered(ask).unwrap();
    assert_eq!(edit, Edit::StartFrom(1));
    assert_eq!(apply(&f.set, &edit, &f.presets).unwrap().name, "Attrition");

    f.edited = false;
    assert_eq!(start_from(&f, 1), Ok(Start::Apply(Edit::StartFrom(1))));
    assert_eq!(start_from(&f, 99), Err(GONE.into()));
    assert_eq!(answered(WavesAsk::Reset), None, "Reset sets the set as loaded");
}

#[test]
fn the_wave_line_counts_enemies_at_one_and_four_players() {
    let f = sample_facts();
    let round = &f.set.rounds[0];
    // Two squads of one for each player, two to four: 4 with one player,
    // 8 with four.
    assert_eq!(wave_line(&round.waves[0]), "4 to 8 enemies");
    assert_eq!(wave_line(&round.waves[4]), "2 to 4 enemies");
    let mut even = round.waves[4].clone();
    even.squads[0].count = ::firefight::Count {
        per_player: 1,
        min: 3,
        max: 3,
    };
    assert_eq!(wave_line(&even), "3 enemies");
    even.squads[0].count.max = 1;
    even.squads[0].count.min = 1;
    assert_eq!(wave_line(&even), "1 enemy");
    assert_eq!(
        wave_help(&f.set, &round.waves[0]),
        "4, 4 and 8 of 16, 16 and 16 enemies with one, two and four players."
    );
    let round_page = round_sheet(&f, 0).unwrap();
    assert_eq!(round_page.rows[5].label, "Boss wave");
    assert_eq!(round_page.rows[5].value.as_deref(), Some("2 to 4 enemies"));
}

#[test]
fn the_enemy_limit_scales_the_tiers_for_fewer_players() {
    let f = sample_facts();
    assert_eq!(enemy_limit(&f.set), 16);
    let def = apply(&f.set, &Edit::EnemyLimit(32), &f.presets).unwrap();
    // Each tier of 16 doubled, within the host table's budget.
    assert_eq!(
        def.caps.live_actors_by_players,
        [(4, 32), (8, 24), (16, 16)]
    );
    assert_eq!(enemy_limit(&def), 32);
}

#[test]
fn the_skull_picker_offers_the_skulls_no_other_round_uses() {
    let f = sample_facts();
    let second = round_sheet(&f, 1).unwrap();
    let Act::Pick(p) = &second.rows[0].act else {
        panic!("Skull opens the picker")
    };
    assert_eq!(p.values[0], "None");
    assert!(!p.values.iter().any(|v| v == "Catch"), "round 1 has it");
    assert_eq!(p.values.len(), 9);
    let first = round_sheet(&f, 0).unwrap();
    assert_eq!(first.rows[0].value.as_deref(), Some("Catch"));
}

#[test]
fn the_waves_rows_follow_the_set() {
    let mut f = sample_facts();
    let labels = |s: &Sheet| s.rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
    assert_eq!(
        labels(&waves_sheet(&f)),
        [
            "Name",
            "Start from...",
            "Teams",
            "Starting lives",
            "Bonus lives a round",
            "Shared lives",
            "Enemy limit",
            "Rounds...",
            "Problems",
            "Test waves",
            "Reset waves",
        ]
    );
    assert_eq!(waves_sheet(&f).title, "YARD");
    f.problems.clear();
    f.edited = false;
    f.testing = true;
    let rows = labels(&waves_sheet(&f));
    assert!(!rows.contains(&"Problems".to_string()), "absent at 0");
    assert!(!rows.contains(&"Reset waves".to_string()), "nothing to reset");
    assert_eq!(rows.last().map(String::as_str), Some("Stop waves"));
    // The only round has no Copy from or Remove round; the only squad no
    // Remove squad.
    let one = apply(&f.set, &Edit::RemoveRound(1), &f.presets).unwrap();
    f.set = one;
    assert!(!labels(&round_sheet(&f, 0).unwrap()).contains(&"Remove round".to_string()));
    let boss = squad_sheet(&f, squad_at(0, 4, 0)).unwrap();
    assert!(!labels(&boss).contains(&"Remove squad".to_string()));
}

#[test]
fn the_first_problem_is_the_notice() {
    let f = sample_facts();
    assert_eq!(
        first_problem(&f.set, &f.problems).as_deref(),
        Some("Round 1, wave 1, squad 2: the AI ground has no grunt.")
    );
    assert_eq!(first_problem(&f.set, &[]), None);
    let p = problems_sheet(&f).unwrap();
    assert_eq!(p.rows[0].description.as_deref(), Some("No Grunts on this map."));
    assert_eq!(p.rows[1].description.as_deref(), Some("Drop zone 3 is not placed."));
}

/// The Forge menu's Waves tab has Waves... while the session has waves to
/// edit, and Enemy types... while the recipe names classes.
#[test]
fn the_forge_menu_opens_the_waves() {
    use crate::forge_screens::menu::{self, Tab};
    let f = publish_sample();
    let facts = |waves| ScreenFacts {
        active: true,
        monitor: true,
        waves,
        ..ScreenFacts::default()
    };
    let tab = menu::sheet(Tab::Waves, &facts(Some(f)));
    let row = tab.rows.iter().find(|r| r.label == "Waves...").unwrap();
    assert_eq!(row.act, Act::Open(Kind::Waves));
    assert!(!tab.rows.iter().any(|r| r.label == "Enemy types..."));
    crate::forge_screens::prepare(Kind::Waves).unwrap();
    assert_eq!(screen(Kind::Waves).sheet().title, "YARD");
    assert_eq!(shown(Kind::Waves), Some(Page::Waves));
    publish(ScreenFacts {
        active: true,
        ..ScreenFacts::default()
    });
    let tab = menu::sheet(Tab::Waves, &facts(None));
    assert!(!tab.rows.iter().any(|r| r.label == "Waves..."));
    assert!(open(Page::Waves).is_err(), "no waves to edit");
    publish(ScreenFacts::default());
}

#[test]
fn drop_zone_lists_its_squads_and_add_squad_here_presets_the_drop() {
    let f = sample_facts();
    // The sample's Grunt squad prefers zone 3, which is not placed; two
    // drops are, and none of them has a squad.
    assert!(drop_sheet(&f, 2).is_none(), "an unplaced zone has no page");
    let page = drop_sheet(&f, 1).expect("zone 1 is placed");
    assert_eq!(page.title, "DROP ZONE 1");
    assert_eq!(page.rows[0].value.as_deref(), Some("None"));
    assert_eq!(page.rows[1].label, "Add squad here...");
    assert_eq!(page.rows[2].label, "Coordinates...");
    let Act::Pick(p) = &page.rows[1].act else {
        panic!("Add squad here... opens a picker")
    };
    assert_eq!(p.values.len(), 10, "two rounds of five waves");
    assert_eq!(p.values[0], "Round 1 Wave 1");
    assert_eq!(p.values[9], "Round 2 Wave 5");
    assert_eq!(
        p.choices[1],
        Choice::Waves(Edit::AddSquadAt {
            round: 0,
            wave: 1,
            zone: 1
        })
    );
    // Choosing the second wave adds a squad that prefers the zone.
    let Choice::Waves(edit) = &p.choices[1] else {
        panic!("a waves edit")
    };
    let next = apply(&f.set, edit, &f.presets).unwrap();
    let squads = &next.rounds[0].waves[1].squads;
    assert_eq!(squads.len(), f.set.rounds[0].waves[1].squads.len() + 1);
    assert_eq!(squads.last().unwrap().drop, Drop::PreferZone(1));
    // The page now lists it, and the Squads list opens its SQUAD page.
    let with = Facts { set: next, ..f };
    let page = drop_sheet(&with, 1).unwrap();
    assert_eq!(page.rows[0].value.as_deref(), Some("1"));
    assert_eq!(page.rows[0].act, Act::Waves(Page::DropSquads(1)));
    let list = drop_squads_sheet(&with, 1).unwrap();
    assert_eq!(list.rows[0].label, "Round 1 Wave 2: Elites");
    assert_eq!(
        list.rows[0].act,
        Act::Waves(Page::Squad(squad_at(0, 1, 1)))
    );
    assert_eq!(lands_here(0), "No squads land here");
    assert_eq!(lands_here(1), "1 squad lands here");
    assert_eq!(lands_here(2), "2 squads land here");
}
