//! `forge_doc::games`: sets of engines and the slots that name them. All
//! inputs are invented.
use forge_doc::games::{ENGINES, GameSet, GamesError, decode, encode, flag_games};
use map::scenario_layout::game_types::first_match;

const KOTH: i32 = 4;
const CTF: i32 = 1;
const SLAYER: i32 = 2;
const ODDBALL: i32 = 3;
const JUGGERNAUT: i32 = 7;
const TERRITORIES: i32 = 8;
const ASSAULT: i32 = 9;

#[test]
fn sets_and_slots_both_ways() {
    let table: [(&[i32], [i16; 4]); 7] = [
        (
            &[KOTH, CTF, SLAYER, ODDBALL, JUGGERNAUT, TERRITORIES, ASSAULT],
            [12; 4],
        ),
        (&[KOTH, SLAYER, ODDBALL, JUGGERNAUT, TERRITORIES], [13; 4]),
        (&[SLAYER], [2; 4]),
        (&[KOTH], [0; 4]),
        (&[CTF, ASSAULT], [1; 4]),
        (&[SLAYER, ODDBALL], [2, 3, 2, 2]),
        (&[KOTH, SLAYER, JUGGERNAUT, TERRITORIES], [0, 2, 7, 8]),
    ];
    for (engines, slots) in table {
        let set = GameSet::of(engines);
        assert_eq!(encode(set), Ok(slots), "{set}");
        assert_eq!(decode(&slots), Some(set), "{slots:?}");
    }
}

#[test]
fn every_set_that_can_be_named_reads_back_and_matches_as_the_engine_does() {
    let mut named = 0;
    for bits in 1u16..(1 << 7) {
        let engines: Vec<i32> = ENGINES
            .iter()
            .enumerate()
            .filter(|(n, _)| bits >> n & 1 == 1)
            .map(|(_, (i, _))| *i)
            .collect();
        let set = GameSet::of(&engines);
        let Ok(slots) = encode(set) else { continue };
        named += 1;
        assert_eq!(decode(&slots), Some(set), "{set}");
        // The matcher agrees with the set for every engine.
        for (index, _) in ENGINES {
            let code = if index == KOTH { 0 } else { index };
            assert_eq!(
                first_match(&slots, Some(code)).is_some(),
                set.has(index),
                "{set} engine {index}"
            );
        }
    }
    assert!(named > 40, "{named} sets");
}

#[test]
fn what_cannot_be_named_is_refused_by_its_sentence() {
    // Five single engines need five slots.
    let five = GameSet::of(&[KOTH, SLAYER, ODDBALL, JUGGERNAUT, TERRITORIES]);
    // That set is the group 13's; take one that is not a group.
    assert_eq!(encode(five), Ok([13; 4]));
    let five = GameSet::of(&[KOTH, SLAYER, ODDBALL, JUGGERNAUT, CTF, ASSAULT]);
    assert_eq!(encode(five), Err(GamesError::TooMany));
    assert_eq!(
        GamesError::TooMany.to_string(),
        "An item can name four game types at most."
    );
    // Capture the Flag and Assault match each other's slot.
    assert_eq!(encode(GameSet::of(&[CTF])), Err(GamesError::FlagPair));
    assert_eq!(
        encode(GameSet::of(&[ASSAULT, SLAYER])),
        Err(GamesError::FlagPair)
    );
    assert_eq!(encode(GameSet::NONE), Err(GamesError::Empty));
}

#[test]
fn a_slot_the_table_does_not_know_is_not_editable() {
    for bad in [4, 5, 6, 10, 11, 15, -1, 100] {
        assert_eq!(decode(&[2, bad, 2, 2]), None, "{bad}");
    }
    assert_eq!(decode(&[]), Some(GameSet::NONE));
}

#[test]
fn a_flags_game_type_is_what_it_is() {
    assert_eq!(flag_games(0), Some(GameSet::of(&[CTF])));
    assert_eq!(flag_games(3), Some(GameSet::of(&[ASSAULT])));
    assert_eq!(flag_games(4), Some(GameSet::of(&[ODDBALL])));
    assert_eq!(flag_games(10), Some(GameSet::of(&[TERRITORIES])));
    assert_eq!(flag_games(18), Some(GameSet::of(&[KOTH])));
    assert_eq!(flag_games(7), None, "a teleporter is in every game type");
    assert_eq!(flag_games(-1), None);
}

#[test]
fn a_set_reads_as_its_names() {
    assert_eq!(GameSet::ALL.to_string(), "All game types");
    assert_eq!(
        GameSet::of(&[SLAYER, ODDBALL]).to_string(),
        "Slayer, Oddball"
    );
    assert_eq!(
        GameSet::of(&[SLAYER, 5, 99]),
        GameSet::of(&[SLAYER]),
        "an unknown index is left out"
    );
}

#[test]
fn a_set_is_written_as_its_engine_indices_in_ascending_order() {
    let set = GameSet::of(&[ASSAULT, KOTH, CTF]);
    assert_eq!(serde_json::to_string(&set).unwrap(), "[1,4,9]");
    assert_eq!(serde_json::from_str::<GameSet>("[1,4,9]").unwrap(), set);
    assert_eq!(
        serde_json::to_string(&GameSet::ALL).unwrap(),
        "[1,2,3,4,7,8,9]"
    );
    assert_eq!(serde_json::to_string(&GameSet::NONE).unwrap(), "[]");
    // One spelling a set: out of order, repeated, or no engine's index.
    for bad in ["[4,1]", "[2,2]", "[5]", "[0]", "[-1]", "[1,2,3,4,7,8,9,10]"] {
        assert!(
            serde_json::from_str::<GameSet>(bad).is_err(),
            "{bad} was read"
        );
    }
}
