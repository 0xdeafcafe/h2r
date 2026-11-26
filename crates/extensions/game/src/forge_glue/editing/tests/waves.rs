//! The Waves screens' choices in a session: each is one `set_waves`, a
//! refusal changes nothing and says the validator's sentence, Start from
//! asks over an edited set, and Reset waves returns to the set as loaded
//! (docs/game/forge-firefight-ui.md, "The Waves screens").
use super::*;
use crate::forge_screens::waves::{At, Edit, WavesAsk};

const FIRST: At = At {
    round: 0,
    wave: 0,
    squad: 0,
};

#[test]
fn each_row_is_one_set_waves() {
    let mut e = editor();
    assert!(e.session.waves().is_none(), "Holdout, the default");
    e.choose(1, Choice::Waves(Edit::StartingLives(9))).unwrap();
    let set = e.session.waves().expect("the session's own set").clone();
    assert_eq!(set.lives.pool_start, 9);
    assert_eq!(set.name, "Holdout", "the default, changed in one place");
    let mut holdout = forge_doc::waves::default_set();
    holdout.lives.pool_start = 9;
    assert_eq!(set, holdout);
    e.choose(2, Choice::Number(sheet::NumberField::WavesName, " Yard ".into()))
        .unwrap();
    assert_eq!(e.session.waves().unwrap().name, "Yard");
    // The pages read the published set.
    screens::publish(Facts {
        active: true,
        waves: Some(e.wave_facts()),
        ..Facts::default()
    });
    screens::waves::open(screens::waves::Page::Waves).unwrap();
    assert_eq!(screens::screen(Kind::Waves).sheet().title, "YARD");
    screens::publish(Facts::default());
}

#[test]
fn a_wave_over_the_enemy_limit_is_refused_with_its_sentence() {
    let mut e = editor();
    // Holdout allows 12 live enemies; thirteen Elites with one player.
    let why = e
        .choose(1, Choice::Waves(Edit::AtLeast(FIRST, 13)))
        .unwrap_err();
    assert!(
        why.contains("round 1, wave 1 needs 13 actors for 1 players (cap 12)"),
        "{why}"
    );
    assert!(e.session.waves().is_none(), "nothing changed");
    // The pages are built again at the next publish.
    assert!(screens::waves::take_stale());
}

#[test]
fn start_from_asks_before_replacing() {
    let mut e = editor();
    // A set that is not edited is replaced at once.
    e.choose(1, Choice::Waves(Edit::StartFrom(1))).unwrap();
    assert_eq!(e.session.waves().unwrap().name, "Attrition");
    assert_eq!(screens::requested(), None);
    // An edited one asks first, over the page.
    e.choose(2, Choice::Waves(Edit::StartFrom(2))).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Question));
    let question = screens::screen(Kind::Question).sheet();
    assert!(question.body.contains("Last Stand"), "{}", question.body);
    assert_eq!(e.session.waves().unwrap().name, "Attrition", "not yet");
    e.choose(
        3,
        Choice::Answer {
            ask: Ask::Waves(WavesAsk::StartFrom(2)),
            yes: false,
        },
    )
    .unwrap();
    assert_eq!(e.session.waves().unwrap().name, "Attrition");
    e.choose(
        4,
        Choice::Answer {
            ask: Ask::Waves(WavesAsk::StartFrom(2)),
            yes: true,
        },
    )
    .unwrap();
    assert_eq!(e.session.waves().unwrap().name, "Last Stand");
}

#[test]
fn reset_returns_to_the_loaded_set() {
    let mut e = editor();
    e.choose(1, Choice::Waves(Edit::BonusLives(7))).unwrap();
    assert!(e.session.waves_edited());
    e.choose(
        2,
        Choice::Answer {
            ask: Ask::Waves(WavesAsk::Reset),
            yes: true,
        },
    )
    .unwrap();
    assert_eq!(e.session.waves(), None, "the session loaded none");
    assert!(!e.session.waves_edited());
}

#[test]
fn remove_round_and_clear_wave_are_asked_then_made() {
    let mut e = editor();
    // Holdout's first round's third wave has two squads.
    e.choose(1, Choice::Waves(Edit::BonusLives(5))).unwrap();
    let squads = |e: &Editor| e.session.waves().unwrap().rounds[0].waves[2].squads.len();
    assert_eq!(squads(&e), 2);
    e.choose(
        2,
        Choice::Answer {
            ask: Ask::Waves(WavesAsk::ClearWave(0, 2)),
            yes: true,
        },
    )
    .unwrap();
    assert_eq!(squads(&e), 1);
    let rounds = e.session.waves().unwrap().rounds.len();
    e.choose(
        3,
        Choice::Answer {
            ask: Ask::Waves(WavesAsk::RemoveRound(0)),
            yes: true,
        },
    )
    .unwrap();
    assert_eq!(e.session.waves().unwrap().rounds.len(), rounds - 1);
}
