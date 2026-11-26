use std::collections::BTreeSet;

use ::firefight::director::{Kill, PlayerSeen, SQUAD_SLOTS, ZoneSeen};
use ::firefight::{
    Caps, Command, Count, Def, Difficulty, DifficultyMultipliers, Drop, FORMAT, Observed, Outcome,
    Round, Scoring, ScriptDirector, Settings, Skull, SquadSpec, Wave,
};

use super::*;

/// A match: every element as the blob says.
const M: Scope = Scope::Match;

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
        teams: 1,
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
            player_kill: 0,
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

/// [`small_def`] for two teams.
fn two_team_def(shared: bool, rounds: usize) -> Def {
    ::firefight::builtin::with_teams(small_def(shared, rounds), 2)
}

fn director(def: Def) -> ScriptDirector {
    ScriptDirector::new(
        def,
        Settings {
            seed: 7,
            difficulty: Difficulty::Normal,
            skulls: BTreeSet::new(),
        },
    )
    .unwrap()
}

/// The players alive, each on game team `slot % 2` (one team ignores it),
/// and `kills` Elites killed this tick.
fn seen(players: &[u8], alive: [u16; SQUAD_SLOTS], kills: Vec<Kill>) -> Observed {
    Observed {
        players: players
            .iter()
            .map(|&slot| PlayerSeen {
                slot,
                alive: true,
                team: slot % 2,
            })
            .collect(),
        actors_alive_by_squad: alive,
        zones: vec![
            ZoneSeen {
                seen: false,
                distance: 100,
                team_distance: [100, 100],
            };
            3
        ],
        kills,
        player_kills: Vec::new(),
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
    /// Every note the client's views sent, in order ([`Feed`]).
    notes: Vec<String>,
}

/// Plays the director against a scripted game in which every placed squad
/// lives ten ticks and is killed by the first player. At each tick the
/// client's mode and view must equal the host's, from the encoded bytes
/// alone.
fn play(mut d: ScriptDirector, players: &[u8], local: u8) -> Played {
    let mut alive = [0u16; SQUAD_SLOTS];
    let mut placed_at = [0u32; SQUAD_SLOTS];
    let mut out = Played {
        stages: BTreeSet::new(),
        banners: BTreeSet::new(),
        boss_spawn: false,
        ticks: 0,
        finished: None,
        notes: Vec::new(),
    };
    let mut feed = Feed::default();
    for tick in 0..4_000u32 {
        let mut kills = Vec::new();
        for slot in 0..SQUAD_SLOTS {
            if alive[slot] > 0 && tick >= placed_at[slot] + 10 {
                for _ in 0..alive[slot] {
                    kills.push(Kill {
                        class: "elite".into(),
                        killer: Some(players[0]),
                        assist: None,
                    });
                }
                alive[slot] = 0;
            }
        }
        for command in d.step(tick, &seen(players, alive, kills)) {
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
        let (host_view, client_view) = (view(&host, local, M), view(&client, local, M));
        assert_eq!(client_view, host_view, "tick {tick}");
        out.notes.extend(feed.tick(Some(Ok(view_of(&client, local, M))), M).1);
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

    // Two teams, a player each: the first scores every kill, so its team
    // wins at the end; each viewer reads its own team's pool.
    for local in [0, 1] {
        let played = play(director(two_team_def(true, 2)), &[0, 1], local);
        assert_eq!(played.finished, Some(Outcome::TeamWon { team: 0 }));
        assert!(played.stages.contains(&(Stage::Team0Won as u8)));
        assert!(played.banners.contains("RED TEAM WINS"), "{:?}", played.banners);
        assert!(played.boss_spawn);
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
    d.step(0, &seen(&[0, 3], [0; SQUAD_SLOTS], Vec::new()));
    let mode = Mode::of(&d, 2, None).unwrap();
    assert_eq!(mode.lives, Lives::Each(vec![(0, 6), (3, 6)]));
    let blob = mode.encode();
    let mine = view(&Mode::decode(&blob).unwrap(), 3, M);
    assert_eq!(mine.get(LIVES_COUNT).unwrap().text, "6");
    let absent = view(&Mode::decode(&blob).unwrap(), 5, M);
    assert!(!absent.get(LIVES_COUNT).unwrap().shown, "no count, no pill");
}

#[test]
fn more_players_than_the_blob_holds_are_refused_by_name() {
    let mut d = director(small_def(false, 1));
    let slots: Vec<u8> = (0..9).collect();
    d.step(0, &seen(&slots, [0; SQUAD_SLOTS], Vec::new()));
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
        skulls: 0,
        lives: Lives::Shared(4),
        teams: None,
    }
}

/// [`sample_mode`] for two teams: slots 1 and 3 on the second.
fn two_team_mode() -> Mode {
    Mode {
        lives: Lives::Teams([6, 2]),
        teams: Some(Teams {
            scores: [450, 1_100],
            second: 0b1010,
        }),
        ..sample_mode()
    }
}

#[test]
fn mode_blobs_round_trip_within_the_channel_limit() {
    let shared = sample_mode();
    assert_eq!(shared.encode().len(), 17);
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
    assert_eq!(bytes.len(), 40);
    assert!(bytes.len() <= MAX_BLOB);
    assert_eq!(Mode::decode(&bytes), Ok(each.clone()));
    // Two teams: their points and members, then the team pools or each
    // player's own lives.
    let teams = two_team_mode();
    assert_eq!(teams.encode().len(), 29);
    assert_eq!(Mode::decode(&teams.encode()), Ok(teams.clone()));
    let teams_each = Mode {
        lives: each.lives.clone(),
        ..teams
    };
    let bytes = teams_each.encode();
    assert_eq!(bytes.len(), 50);
    assert!(bytes.len() <= MAX_BLOB);
    assert_eq!(Mode::decode(&bytes), Ok(teams_each));
    for stage in [Stage::Team0Won, Stage::Team1Won, Stage::Draw] {
        let over = Mode {
            stage,
            ..two_team_mode()
        };
        assert_eq!(Mode::decode(&over.encode()), Ok(over));
    }
}

#[test]
fn a_blob_it_does_not_understand_is_refused() {
    let good = sample_mode().encode();
    let mut v = good.clone();
    v[0] = 1;
    assert_eq!(Mode::decode(&v), Err(Error::Version(1)));
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
    assert_eq!(field(1, 11), Err(Error::Field("stage")));
    // A two-team stage in a blob of one team.
    for stage in [Stage::Team0Won, Stage::Team1Won, Stage::Draw] {
        assert_eq!(field(1, stage as u8), Err(Error::Field("stage")));
    }
    assert_eq!(field(2, 3), Err(Error::Field("round")));
    assert_eq!(field(3, 13), Err(Error::Field("round")));
    assert_eq!(field(4, 5), Err(Error::Field("wave")));
    assert_eq!(field(5, 17), Err(Error::Field("wave")));
    assert_eq!(field(6, 2), Err(Error::Field("flags")));
    assert_eq!(field(TEAM_COUNT, 0), Err(Error::Field("teams")));
    assert_eq!(field(TEAM_COUNT, 3), Err(Error::Field("teams")));
    // A skull Firefight never turns on (Envy, index 0; a bit past 16).
    assert_eq!(field(9, 1), Err(Error::Field("skulls")));
    assert_eq!(field(12, 0x80), Err(Error::Field("skulls")));
    // Two teams named with no room for their part.
    assert!(matches!(field(TEAM_COUNT, 2), Err(Error::Length { .. })));
    assert_eq!(field(HEAD, 3), Err(Error::Field("lives")));
    // Team pools in a blob of one team, and one pool in a blob of two.
    assert_eq!(field(HEAD, LIVES_TEAMS), Err(Error::Field("lives")));
    let mut teams = two_team_mode().encode();
    teams[HEAD + TEAMS] = LIVES_SHARED;
    assert!(Mode::decode(&teams).is_err());
    // Per-player lives: a count past the room, and slots out of order.
    let mut each = Mode {
        lives: Lives::Each(vec![(1, 1), (2, 2)]),
        ..sample_mode()
    }
    .encode();
    each[HEAD + 1] = 9;
    assert_eq!(Mode::decode(&each), Err(Error::Field("lives")));
    each[HEAD + 1] = 2;
    each[HEAD + 2] = 5;
    assert_eq!(Mode::decode(&each), Err(Error::Field("lives")));
}

/// With two teams each viewer reads its own team's pool, and the end
/// names the winner or a draw. The points are the game's own score
/// display's, not the kit HUD's.
#[test]
fn two_teams_show_each_viewer_its_teams_lives() {
    let m = two_team_mode();
    let red = view(&m, 0, M);
    let blue = view(&m, 3, M);
    assert_eq!(red.get(LIVES_COUNT).unwrap().text, "6");
    assert_eq!(blue.get(LIVES_COUNT).unwrap().text, "2");
    for (stage, banner) in [
        (Stage::Team0Won, "RED TEAM WINS"),
        (Stage::Team1Won, "BLUE TEAM WINS"),
        (Stage::Draw, "DRAW"),
    ] {
        let over = view(
            &Mode {
                stage,
                ..two_team_mode()
            },
            3,
            M,
        );
        assert_eq!(over.get(BANNER).unwrap().text, banner);
        assert!(!over.get(LIVES_COUNT).unwrap().shown, "{banner}");
    }
    assert_eq!(Stage::team_won(0), Stage::Team0Won);
    assert_eq!(Stage::team_won(1), Stage::Team1Won);
}

/// The director's two teams reach the blob: the team pools, the points
/// and the second team's slots.
#[test]
fn the_directors_two_teams_reach_the_blob() {
    let mut d = director(two_team_def(true, 1));
    d.step(0, &seen(&[0, 1, 3], [0; SQUAD_SLOTS], Vec::new()));
    let mode = Mode::of(&d, 3, None).unwrap();
    assert_eq!(mode.lives, Lives::Teams([6, 6]));
    assert_eq!(
        mode.teams,
        Some(Teams {
            scores: [0, 0],
            second: 0b1010
        })
    );
    let draw = Mode::of(&d, 3, Some(Outcome::Draw)).unwrap();
    assert_eq!(draw.stage, Stage::Draw);
    let won = Mode::of(&d, 3, Some(Outcome::TeamWon { team: 1 })).unwrap();
    assert_eq!(won.stage, Stage::Team1Won);
}

/// The words, stage by stage: the banner, the lives and the enemies left.
#[test]
fn each_stage_has_its_words() {
    let text = |m: &Mode| -> Vec<String> {
        view(m, 0, M)
            .0
            .iter()
            .filter(|(id, _)| [BANNER, LIVES_COUNT, ENEMIES_COUNT].contains(id))
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
        ["ff.banner=-", "ff.lives.count=4", "ff.enemies.count=9"]
    );
    let boss = Mode {
        stage: Stage::Spawning,
        wave: 4,
        boss: true,
        ..sample_mode()
    };
    assert_eq!(text(&boss)[0], "ff.banner=BOSS WAVE");
    let won = Mode {
        stage: Stage::Won,
        left: 0,
        ..sample_mode()
    };
    assert_eq!(
        text(&won),
        ["ff.banner=VICTORY", "ff.lives.count=-", "ff.enemies.count=-"]
    );
    let lost = Mode {
        stage: Stage::Lost,
        ..won.clone()
    };
    assert_eq!(text(&lost)[0], "ff.banner=GAME OVER");
    let between = Mode {
        stage: Stage::Intermission,
        left: 0,
        ..sample_mode()
    };
    assert_eq!(text(&between)[0], "ff.banner=WAVE COMPLETE");
    assert_eq!(text(&between)[2], "ff.enemies.count=-", "none while no wave is on");
}

#[test]
fn the_last_life_and_none_flash() {
    let style = |n: u16| {
        let m = Mode {
            lives: Lives::Shared(n),
            ..sample_mode()
        };
        view(&m, 0, M).get(LIVES_COUNT).unwrap().style
    };
    assert_eq!(style(2), Style::Normal);
    let warn = Style::Flash(hud_kit_core::model::Pulse::TimeWarning);
    assert_eq!(style(1), warn);
    assert_eq!(style(0), warn);
}

/// Every blob gives the kit HUD's one layout: a value for each of its
/// elements and nothing else, in every stage, lives kind and viewer.
#[test]
fn every_blob_gives_the_kits_one_layout() {
    let hud = hud_kit_core::views::firefight::hud();
    let mut modes = vec![sample_mode(), two_team_mode()];
    for stage in Stage::ALL {
        let base = if stage.two_teams_only() {
            two_team_mode()
        } else {
            sample_mode()
        };
        modes.push(Mode { stage, ..base });
    }
    modes.push(Mode {
        lives: Lives::Each(vec![(0, 3), (2, 1)]),
        ..sample_mode()
    });
    for m in &modes {
        for local in [0, 1, 3, 7] {
            let v = view(m, local, M);
            assert_eq!(v.layout_problems(&hud), Vec::<String>::new(), "{m:?} {local}");
            assert!(!v.get(NOTE).unwrap().shown, "a note is sent, not published");
        }
    }
}

/// The feed: no match hides the HUD; a match's first view sends no note,
/// a new round and a lost life one each; an unreadable blob hides it and
/// forgets the last view.
#[test]
fn the_feed_publishes_the_view_and_sends_the_notes_a_change_makes() {
    let hidden = hud_kit_core::views::firefight::hidden();
    let mut f = Feed::default();
    assert_eq!(f.tick(None, M), (hidden.clone(), Vec::new()));
    let a = view_of(&sample_mode(), 0, M);
    let (values, notes) = f.tick(Some(Ok(a.clone())), M);
    assert_eq!(values, view(&sample_mode(), 0, M));
    assert!(notes.is_empty(), "the first view");
    let next = Mode {
        stage: Stage::Spawning,
        round: 2,
        wave: 0,
        lives: Lives::Shared(3),
        ..sample_mode()
    };
    let (_, notes) = f.tick(Some(Ok(view_of(&next, 0, M))), M);
    assert_eq!(notes, ["Round 3 of 3", "A life lost"]);
    let (values, notes) = f.tick(Some(Err(Error::Version(1))), M);
    assert_eq!(values, hidden);
    assert!(notes.is_empty());
    assert_eq!(f.error.as_deref(), Some(Error::Version(1).to_string().as_str()));
    // After it, a view is the first again.
    let (_, notes) = f.tick(Some(Ok(view_of(&next, 0, M))), M);
    assert!(notes.is_empty());
    assert_eq!(f.error, None);
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

/// The skulls on are in the blob, one bit a session skull index, and only
/// Firefight's; every Firefight skull is in [`FIREFIGHT_SKULLS`].
#[test]
fn the_skulls_on_ride_in_the_blob() {
    // Exhaustive: a new Firefight skull fails to compile here until it is
    // listed.
    let listed = |s: Skull| match s {
        Skull::Assassins
        | Skull::Thunderstorm
        | Skull::Famine
        | Skull::Blind
        | Skull::Ghost
        | Skull::Catch
        | Skull::Mythic
        | Skull::Angry
        | Skull::LowGravity => FIREFIGHT_SKULLS.contains(&s),
    };
    assert!(FIREFIGHT_SKULLS.iter().all(|s| listed(*s)));
    assert_eq!(SKULL_BITS.count_ones(), 9);
    assert_eq!(skull_mask(&[Skull::Mythic, Skull::LowGravity]), 1 << 12 | 1 << 16);
    let m = Mode {
        skulls: skull_mask(&FIREFIGHT_SKULLS),
        ..sample_mode()
    };
    assert_eq!(m.skulls, SKULL_BITS);
    assert_eq!(Mode::decode(&m.encode()), Ok(m.clone()));
    assert_eq!(view_of(&m, 0, M).skulls, SKULL_BITS);
    // The director's set reaches the blob.
    let mut d = director(small_def(true, 2));
    d.step(0, &seen(&[0], [0; SQUAD_SLOTS], Vec::new()));
    let mode = Mode::of(&d, 1, None).unwrap();
    assert_eq!(mode.skulls, skull_mask(&d.state().skulls));
    assert_eq!(skull_name(12), Some("Mythic"));
    assert_eq!(skull_name(200), None);
}

/// A round's skull switched on is a note a client sends from the blob
/// alone, with the round's: the second round of the test set turns Blind
/// on.
#[test]
fn a_rounds_skull_is_a_note_from_the_blob() {
    let played = play(director(small_def(true, 2)), &[0], 0);
    assert_eq!(played.finished, Some(Outcome::Won));
    let at = |t: &str| played.notes.iter().position(|n| n == t);
    let (round, skull) = (at("Round 2 of 2"), at("Blind skull on"));
    assert!(round.is_some() && skull.is_some(), "{:?}", played.notes);
    assert!(round < skull, "{:?}", played.notes);
}

/// The end of a won match sends the rounds cleared once, from the blob
/// alone, after the round's skull note.
#[test]
fn a_won_match_ends_with_the_rounds_cleared() {
    let played = play(director(small_def(true, 2)), &[0], 0);
    assert_eq!(played.finished, Some(Outcome::Won));
    assert_eq!(played.notes.last().map(String::as_str), Some("Cleared 2/2"));
    assert_eq!(
        played.notes.iter().filter(|n| n.starts_with("Cleared")).count(),
        1,
        "{:?}",
        played.notes
    );
}

/// Split screen: each local user's feed views the blob at its own roster
/// slot, so each sees its own lives and gets its own notes; a user with no
/// player is hidden.
#[test]
fn each_local_user_sees_its_own_lives() {
    let m = Mode {
        lives: Lives::Each(vec![(0, 3), (2, 1)]),
        ..sample_mode()
    };
    let blob = m.encode();
    let current = |slot: u8, scope| Some(Mode::decode(&blob).map(|m| view_of(&m, slot, scope)));
    let mut feeds: Feeds = Default::default();
    let slots = [Some(0), Some(2), None, None];
    let all = tick_all(&mut feeds, &slots, &[M; USERS], current);
    let lives = |i: usize| {
        let v = all[i].0.get(LIVES_COUNT).unwrap();
        v.shown.then(|| v.text.clone())
    };
    assert_eq!(lives(0).as_deref(), Some("3"));
    assert_eq!(lives(1).as_deref(), Some("1"));
    assert_eq!(all[2].0, hud_kit_core::views::firefight::hidden());
    // User 1 loses its last life: the note is its alone.
    let hit = Mode {
        lives: Lives::Each(vec![(0, 3), (2, 0)]),
        ..sample_mode()
    }
    .encode();
    let current = |slot: u8, scope| Some(Mode::decode(&hit).map(|m| view_of(&m, slot, scope)));
    let all = tick_all(&mut feeds, &slots, &[M; USERS], current);
    assert!(all[0].1.is_empty());
    assert_eq!(all[1].1, ["A life lost"]);
}

/// A wave test seen from a biped: the banner, the notes and the enemies
/// pill as in a match, and no lives pill, whatever the blob's lives say
/// (a test charges none).
#[test]
fn a_test_shows_no_lives_pill() {
    use hud_kit_core::views::firefight::{ENEMIES_PILL, LIVES_END, LIVES_PILL};
    let m = Mode {
        stage: Stage::Spawning,
        ..sample_mode()
    };
    let in_match = view(&m, 0, Scope::Match);
    assert!(in_match.get(LIVES_PILL).unwrap().shown, "a match shows its lives");
    let biped = view(&m, 0, Scope::Biped);
    for id in [LIVES_PILL, LIVES_END, LIVES_COUNT] {
        assert!(!biped.get(id).unwrap().shown, "{id}: a test charges no lives");
    }
    assert_eq!(biped.get(BANNER).unwrap().text, "WAVE 3");
    assert!(biped.get(ENEMIES_PILL).unwrap().shown);
    assert_eq!(biped.get(ENEMIES_COUNT).unwrap().text, "9");
    assert_eq!(view_of(&m, 0, Scope::Biped).lives, None);
    // Lives lost send no note in a test.
    let mut f = Feed::default();
    f.tick(Some(Ok(view_of(&m, 0, Scope::Biped))), Scope::Biped);
    let hit = Mode {
        lives: Lives::Shared(1),
        ..m.clone()
    };
    let (_, notes) = f.tick(Some(Ok(view_of(&hit, 0, Scope::Biped))), Scope::Biped);
    assert!(notes.is_empty(), "{notes:?}");
    // Every scope is the one layout.
    let hud = hud_kit_core::views::firefight::hud();
    for scope in [Scope::Match, Scope::Biped, Scope::Monitor] {
        assert_eq!(view(&m, 0, scope).layout_problems(&hud), Vec::<String>::new(), "{scope:?}");
    }
}

/// A wave test seen from the Forge monitor: the banner shows and the
/// Firefight HUD's pills do not; the monitor HUD's own pill counts the
/// enemies, and with no wave on (or no test) it shows nothing.
#[test]
fn a_monitor_sees_the_banner_and_the_enemies_pill() {
    use hud_kit_core::views::firefight::{ENEMIES_END, ENEMIES_PILL, LIVES_PILL};
    use hud_kit_core::views::forge as monitor;
    let m = Mode {
        stage: Stage::Spawning,
        ..sample_mode()
    };
    let seen = view(&m, 0, Scope::Monitor);
    assert_eq!(seen.get(BANNER).unwrap().text, "WAVE 3");
    assert!(seen.get(BANNER).unwrap().shown);
    for id in [LIVES_PILL, LIVES_COUNT, ENEMIES_PILL, ENEMIES_END, ENEMIES_COUNT] {
        assert!(!seen.get(id).unwrap().shown, "{id}: the monitor's own HUD has the count");
    }
    // The monitor HUD's pill takes the count while the wave is on.
    assert_eq!(enemies_left(&m), Some(9));
    let v = monitor::View {
        monitor: true,
        enemies: enemies_left(&m),
        ..monitor::View::default()
    };
    let drawn = monitor::values(&v, &monitor::Buttons::sample());
    assert_eq!(drawn.get(monitor::ENEMIES_COUNT).unwrap().text, "9");
    assert!(drawn.get(monitor::ENEMIES_PILL).unwrap().shown);
    let between = Mode {
        stage: Stage::Intermission,
        ..sample_mode()
    };
    assert_eq!(enemies_left(&between), None);
    // A scope follows the test and the monitor.
    assert_eq!(Scope::of(false, true), Scope::Match);
    assert_eq!(Scope::of(true, false), Scope::Biped);
    assert_eq!(Scope::of(true, true), Scope::Monitor);
}
