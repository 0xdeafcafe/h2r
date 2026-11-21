use std::collections::BTreeSet;

use ::firefight::director::{PlayerSeen, SQUAD_SLOTS, ZoneSeen};
use ::firefight::{
    Caps, Command, Count, Def, Difficulty, DifficultyMultipliers, Director, Drop, FORMAT, Observed,
    Outcome, Round, Scoring, Settings, Skull, SquadSpec, Wave,
};

use super::*;

/// `rounds` rounds of five waves, each of three one-actor squads, two ticks
/// apart: a match the director finishes in a few hundred ticks.
fn small_def(shared: bool, rounds: usize) -> Def {
    let waves = || {
        (0..5)
            .map(|wave| Wave {
                squads: (0..3)
                    .map(|_| SquadSpec {
                        class: "elite".into(),
                        count: Count {
                            per_player: 1,
                            min: 1,
                            max: 1,
                        },
                        drop: Drop::Edge,
                        weapon_tier: 0,
                    })
                    .collect(),
                boss: wave == 4,
                gap_ticks: 2,
            })
            .collect::<Vec<_>>()
    };
    Def {
        format: FORMAT,
        name: "Hud".into(),
        // Each skull is switched on by one round only.
        rounds: [Skull::Famine, Skull::Blind, Skull::Ghost, Skull::Catch, Skull::Mythic]
            .into_iter()
            .take(rounds)
            .map(|skull| Round {
                waves: waves(),
                skulls_on: vec![skull],
            })
            .collect(),
        lives: ::firefight::Lives {
            pool_start: 6,
            per_round_bonus: 2,
            shared,
        },
        caps: Caps {
            live_actors_by_players: vec![(4, 8)],
        },
        scoring: Scoring {
            by_class: [("elite".into(), 100)].into(),
            difficulty_multiplier: DifficultyMultipliers {
                easy: 1_000,
                normal: 1_000,
                heroic: 1_000,
                legendary: 1_000,
            },
            skull_multipliers: [(Skull::Famine, 1_100)].into(),
            assist: 250,
        },
    }
}

fn director(def: Def) -> Director {
    Director::new(
        def,
        Settings {
            seed: 7,
            difficulty: Difficulty::Normal,
            skulls: BTreeSet::new(),
        },
    )
    .unwrap()
}

fn seen(players: &[u8], alive: [u16; SQUAD_SLOTS]) -> Observed {
    Observed {
        players: players
            .iter()
            .map(|&slot| PlayerSeen { slot, alive: true })
            .collect(),
        actors_alive_by_squad: alive,
        zones: vec![
            ZoneSeen {
                seen: false,
                distance: 100
            };
            3
        ],
        kills: Vec::new(),
    }
}

/// What a match shows, tick by tick: the host's mode, and a client's, which
/// has only the host's bytes.
struct Played {
    stages: BTreeSet<u8>,
    banners: BTreeSet<String>,
    boss_spawn: bool,
    ticks: u32,
    finished: Option<Outcome>,
}

/// Plays the director against a scripted game in which every placed squad
/// lives ten ticks. At each tick the client's mode and view must equal the
/// host's, from the encoded bytes alone.
fn play(mut d: Director, players: &[u8], local: u8) -> Played {
    let mut alive = [0u16; SQUAD_SLOTS];
    let mut placed_at = [0u32; SQUAD_SLOTS];
    let mut out = Played {
        stages: BTreeSet::new(),
        banners: BTreeSet::new(),
        boss_spawn: false,
        ticks: 0,
        finished: None,
    };
    for tick in 0..4_000u32 {
        for slot in 0..SQUAD_SLOTS {
            if alive[slot] > 0 && tick >= placed_at[slot] + 10 {
                alive[slot] = 0;
            }
        }
        for command in d.step(tick, &seen(players, alive)) {
            match command {
                Command::PlaceSquad { slot, count, .. } => {
                    alive[usize::from(slot)] = count;
                    placed_at[usize::from(slot)] = tick;
                }
                Command::Finish { outcome } => out.finished = Some(outcome),
                _ => {}
            }
        }
        let host = Mode::of(&d, players.len() as u8, out.finished).unwrap();
        let blob = host.encode();
        assert!(blob.len() <= MAX_BLOB, "{} bytes", blob.len());
        let client = Mode::decode(&blob).unwrap();
        assert_eq!(client, host, "tick {tick}");
        let (host_view, client_view) = (view(&host, local), view(&client, local));
        assert_eq!(client_view, host_view, "tick {tick}");
        out.stages.insert(host.stage as u8);
        out.boss_spawn |= host.stage == Stage::Spawning && host.boss;
        if let Some(b) = host_view.get(BANNER).filter(|b| b.shown) {
            out.banners.insert(b.text.clone());
        }
        out.ticks = tick;
        if out.finished.is_some() {
            break;
        }
    }
    out
}

/// A client given only the encoded mode draws what the host draws, at every
/// tick of a whole match: [`view`] and [`Mode::decode`] take bytes and a
/// roster slot and nothing else.
#[test]
fn the_hud_reads_only_the_mode_blob() {
    let played = play(director(small_def(true, 2)), &[0], 0);
    assert_eq!(
        played.finished,
        Some(Outcome::Won),
        "{} ticks",
        played.ticks
    );
    for stage in [
        Stage::Warmup,
        Stage::Spawning,
        Stage::Fighting,
        Stage::Intermission,
        Stage::Won,
    ] {
        assert!(played.stages.contains(&(stage as u8)), "{stage:?}");
    }
    assert!(played.boss_spawn, "a boss wave spawned");
    for banner in [
        "GET READY",
        "WAVE 1",
        "BOSS WAVE",
        "WAVE COMPLETE",
        "ROUND COMPLETE",
        "VICTORY",
    ] {
        assert!(
            played.banners.contains(banner),
            "{banner}: {:?}",
            played.banners
        );
    }
    // The signature says it: bytes and a slot in, a view out.
    let _: fn(&[u8]) -> Result<Mode, Error> = Mode::decode;
    let _: fn(&Mode, u8) -> Values = view;
}

/// With lives not shared each player's own count is in the blob, and each
/// client reads its own.
#[test]
fn each_player_reads_their_own_lives_from_the_blob() {
    let played = play(director(small_def(false, 1)), &[0, 3], 3);
    assert_eq!(played.finished, Some(Outcome::Won));
    let mut d = director(small_def(false, 1));
    d.step(0, &seen(&[0, 3], [0; SQUAD_SLOTS]));
    let mode = Mode::of(&d, 2, None).unwrap();
    assert_eq!(mode.lives, Lives::Each(vec![(0, 6), (3, 6)]));
    let blob = mode.encode();
    let mine = view(&Mode::decode(&blob).unwrap(), 3);
    assert_eq!(mine.get(LIVES).unwrap().text, "LIVES 6");
    let absent = view(&Mode::decode(&blob).unwrap(), 5);
    assert!(!absent.get(LIVES).unwrap().shown, "no count, no line");
}

#[test]
fn more_players_than_the_blob_holds_are_refused_by_name() {
    let mut d = director(small_def(false, 1));
    let slots: Vec<u8> = (0..9).collect();
    d.step(0, &seen(&slots, [0; SQUAD_SLOTS]));
    assert_eq!(Mode::of(&d, 9, None), Err(Error::TooManyPlayers(9)));
    assert!(Error::TooManyPlayers(9).to_string().contains("9 players"));
}

fn sample_mode() -> Mode {
    Mode {
        stage: Stage::Fighting,
        round: 1,
        rounds: 3,
        wave: 2,
        waves: 5,
        boss: false,
        left: 9,
        lives: Lives::Shared(4),
    }
}

#[test]
fn mode_blobs_round_trip_within_the_channel_limit() {
    let shared = sample_mode();
    assert_eq!(shared.encode().len(), 12);
    assert_eq!(Mode::decode(&shared.encode()), Ok(shared));
    let each = Mode {
        boss: true,
        lives: Lives::Each(
            (0..MAX_EACH as u8)
                .map(|s| (s, u16::from(s) * 100))
                .collect(),
        ),
        ..sample_mode()
    };
    let bytes = each.encode();
    assert_eq!(bytes.len(), 35);
    assert!(bytes.len() <= MAX_BLOB);
    assert_eq!(Mode::decode(&bytes), Ok(each));
}

#[test]
fn a_blob_it_does_not_understand_is_refused() {
    let good = sample_mode().encode();
    let mut v = good.clone();
    v[0] = 2;
    assert_eq!(Mode::decode(&v), Err(Error::Version(2)));
    assert!(matches!(
        Mode::decode(&good[..good.len() - 1]),
        Err(Error::Length { .. })
    ));
    let mut long = good.clone();
    long.push(0);
    assert!(matches!(Mode::decode(&long), Err(Error::Length { .. })));
    assert!(matches!(Mode::decode(&[]), Err(Error::Length { .. })));
    assert!(matches!(
        Mode::decode(&[0; MAX_BLOB + 1]),
        Err(Error::Length { .. })
    ));
    let field = |at: usize, value: u8| {
        let mut v = good.clone();
        v[at] = value;
        Mode::decode(&v)
    };
    assert_eq!(field(1, 8), Err(Error::Field("stage")));
    assert_eq!(field(2, 3), Err(Error::Field("round")));
    assert_eq!(field(3, 13), Err(Error::Field("round")));
    assert_eq!(field(4, 5), Err(Error::Field("wave")));
    assert_eq!(field(5, 17), Err(Error::Field("wave")));
    assert_eq!(field(6, 2), Err(Error::Field("flags")));
    assert_eq!(field(9, 2), Err(Error::Field("lives")));
    // Per-player lives: a count past the room, and slots out of order.
    let mut each = Mode {
        lives: Lives::Each(vec![(1, 1), (2, 2)]),
        ..sample_mode()
    }
    .encode();
    each[HEAD] = 9;
    assert_eq!(Mode::decode(&each), Err(Error::Field("lives")));
    each[HEAD] = 2;
    each[HEAD + 1] = 5;
    assert_eq!(Mode::decode(&each), Err(Error::Field("lives")));
}

/// The words, stage by stage, with the spacing the HUD copy keeps.
#[test]
fn each_stage_has_its_words() {
    let text = |m: &Mode| -> Vec<String> {
        view(m, 0)
            .0
            .iter()
            .map(|(id, v)| {
                if v.shown {
                    format!("{id}={}", v.text)
                } else {
                    format!("{id}=-")
                }
            })
            .collect()
    };
    assert_eq!(
        text(&sample_mode()),
        [
            "banner=-",
            "round=ROUND 2 OF 3",
            "wave=WAVE 3 OF 5",
            "enemies=ENEMIES 9",
            "lives=LIVES 4"
        ]
    );
    let boss = Mode {
        stage: Stage::Spawning,
        wave: 4,
        boss: true,
        ..sample_mode()
    };
    assert_eq!(text(&boss)[0], "banner=BOSS WAVE");
    assert_eq!(text(&boss)[2], "wave=BOSS WAVE");
    let won = Mode {
        stage: Stage::Won,
        left: 0,
        ..sample_mode()
    };
    assert_eq!(
        text(&won),
        [
            "banner=VICTORY",
            "round=-",
            "wave=-",
            "enemies=-",
            "lives=-"
        ]
    );
    let lost = Mode {
        stage: Stage::Lost,
        ..won.clone()
    };
    assert_eq!(text(&lost)[0], "banner=GAME OVER");
    let between = Mode {
        stage: Stage::Intermission,
        left: 0,
        ..sample_mode()
    };
    assert_eq!(text(&between)[0], "banner=WAVE COMPLETE");
    assert_eq!(text(&between)[3], "enemies=-", "none while no wave is on");
}

#[test]
fn the_last_life_and_none_are_flagged() {
    let style = |n: u16| {
        let m = Mode {
            lives: Lives::Shared(n),
            ..sample_mode()
        };
        view(&m, 0).get(LIVES).unwrap().style
    };
    assert_eq!(style(2), Style::Normal);
    assert_eq!(style(1), Style::Warn);
    assert_eq!(style(0), Style::Warn);
}

/// The kit's layout rules hold in every state and every screen mode.
#[test]
fn every_state_passes_the_layout_checks() {
    let all = states();
    let names: Vec<_> = all.iter().map(|(n, _)| n.as_str()).collect();
    for stage in [
        "warmup",
        "spawning",
        "boss",
        "fighting",
        "cleanup",
        "wave-complete",
        "round-complete",
        "won",
        "lost",
        "over",
        "last-life",
        "no-lives",
        "widest",
        "own-lives",
        "no-own-lives",
    ] {
        assert!(names.contains(&stage), "no state {stage}");
    }
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "state names are unique");
    assert_eq!(check(&all), vec![]);
    for (name, values) in &all {
        let ids: Vec<_> = values.0.iter().map(|(i, _)| *i).collect();
        assert_eq!(
            ids,
            LAYOUT.iter().map(|e| e.id).collect::<Vec<_>>(),
            "{name}: one layout"
        );
    }
}

/// Each rule fires on a state built to break it.
#[test]
fn each_layout_check_fires_on_a_state_built_to_break_it() {
    let good = view(&sample_mode(), 0);
    let with = |id: &'static str, text: &str| {
        let mut v = good.clone();
        let entry = v.0.iter_mut().find(|(i, _)| *i == id).unwrap();
        entry.1 = Value {
            shown: true,
            text: text.into(),
            style: Style::Normal,
        };
        vec![("broken".to_string(), v)]
    };
    assert_eq!(
        check(&with(LIVES, "LIVES 1234567")),
        [Finding::Wide {
            state: "broken".into(),
            id: LIVES,
            text: "LIVES 1234567".into()
        }]
    );
    for text in [" LIVES 4", "LIVES 4 ", "LIVES  4"] {
        assert!(
            matches!(&check(&with(LIVES, text))[..], [Finding::Spacing { .. }]),
            "{text:?}"
        );
    }
    assert!(matches!(
        &check(&with(LIVES, "Lives 4"))[..],
        [Finding::Glyph { ch: 'i', .. }]
    ));
    assert!(matches!(
        &check(&with(LIVES, ""))[..],
        [Finding::Empty { .. }]
    ));
    let mut fewer = good.clone();
    fewer.0.pop();
    assert_eq!(
        check(&[("short".to_string(), fewer)]),
        [Finding::Layout {
            state: "short".into()
        }]
    );
}

/// No two lines meet in any screen mode, and a hidden one has no
/// rectangle.
#[test]
fn the_layout_keeps_its_lines_apart_in_every_screen_mode() {
    for screen in Screen::ALL {
        let rects: Vec<_> = LAYOUT.iter().filter_map(|e| rect(e, screen)).collect();
        assert!(!rects.is_empty(), "{screen:?}");
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!overlaps(*a, *b), "{screen:?}: {a:?} {b:?}");
            }
        }
    }
    assert!(rect(&LAYOUT[0], Screen::Quarter).is_none());
    assert!(rect(&LAYOUT[4], Screen::Quarter).is_some());
}

#[test]
fn the_published_blob_is_the_latest_or_none() {
    let held = Slot::new();
    assert_eq!(held.get(), None);
    held.set(Some(sample_mode().encode()));
    assert_eq!(Mode::decode(&held.get().unwrap()), Ok(sample_mode()));
    held.set(None);
    assert_eq!(held.get(), None);
}
