//! The host runtime over a fake game that records every engine call.

use super::*;
use ::firefight::{Difficulty, ScoreReason, builtin};

/// The kit's invented character slot for its Elite class.
const ELITE: u16 = 0x21;
const KIT_TEAM: u8 = 3;

#[derive(Clone, Debug, PartialEq)]
enum Call {
    AiOn,
    Squad(SquadOp),
    Score(u8, u32),
    Lives(Option<u8>, u16),
    TeamLives(u8, u16),
    Respawn(u8),
    Skull(Skull, bool),
    End(Outcome),
}

struct FakeGame {
    kit: Option<Kit>,
    ai: Option<AiGlobals>,
    players: Vec<PlayerView>,
    actors: [u16; SQUAD_SLOTS],
    kills: Vec<KillSeen>,
    player_kills: Vec<PlayerKill>,
    calls: Vec<Call>,
}

impl FakeGame {
    fn new() -> Self {
        Self {
            kit: Some(kit()),
            ai: Some(AiGlobals {
                on: true,
                initialised: true,
            }),
            players: vec![player(0, 0)],
            actors: [0; SQUAD_SLOTS],
            kills: Vec::new(),
            player_kills: Vec::new(),
            calls: Vec::new(),
        }
    }

    /// Two players a team on a kit with both teams' bases.
    fn two_teams() -> Self {
        let mut game = FakeGame::new();
        game.kit = Some(two_base_kit());
        game.players = vec![player(0, 0), player(1, 1)];
        game
    }
}

impl SquadEntry for FakeGame {
    fn squad(&mut self, op: SquadOp) -> anyhow::Result<()> {
        self.calls.push(Call::Squad(op));
        Ok(())
    }
}

impl Game for FakeGame {
    fn kit(&self) -> Option<Kit> {
        self.kit.clone()
    }
    fn ai(&self) -> Option<AiGlobals> {
        self.ai
    }
    fn switch_ai_on(&mut self) -> anyhow::Result<()> {
        self.calls.push(Call::AiOn);
        if let Some(ai) = &mut self.ai {
            ai.on = true;
        }
        Ok(())
    }
    fn players(&self) -> Vec<PlayerView> {
        self.players.clone()
    }
    fn actors_by_squad(&self) -> [u16; SQUAD_SLOTS] {
        self.actors
    }
    fn sees(&self, _: [f32; 3]) -> bool {
        false
    }
    fn kills(&self) -> Vec<KillSeen> {
        self.kills.clone()
    }
    fn player_kills(&self) -> Vec<PlayerKill> {
        self.player_kills.clone()
    }
    fn credit_score(&mut self, player: u8, points: u32) -> anyhow::Result<()> {
        self.calls.push(Call::Score(player, points));
        Ok(())
    }
    fn set_lives(&mut self, player: Option<u8>, pool: u16) -> anyhow::Result<()> {
        self.calls.push(Call::Lives(player, pool));
        Ok(())
    }
    fn set_team_lives(&mut self, team: u8, pool: u16) -> anyhow::Result<()> {
        self.calls.push(Call::TeamLives(team, pool));
        Ok(())
    }
    fn respawn(&mut self, player: u8) -> anyhow::Result<()> {
        self.calls.push(Call::Respawn(player));
        Ok(())
    }
    fn skull(&mut self, skull: Skull, on: bool) -> anyhow::Result<()> {
        self.calls.push(Call::Skull(skull, on));
        Ok(())
    }
    fn end_game(&mut self, outcome: Outcome) -> anyhow::Result<()> {
        self.calls.push(Call::End(outcome));
        Ok(())
    }
}

/// One zone, so the director's zone choice draws nothing, and the home
/// base.
fn kit() -> Kit {
    Kit {
        zones: vec![[40.0, 0.0, 0.0]],
        classes: [("elite".to_string(), ELITE)].into(),
        team: KIT_TEAM,
        bases: vec![[0.0, 0.0, 0.0]],
    }
}

/// [`kit`] with a second base, for two teams, at the far end.
fn two_base_kit() -> Kit {
    Kit {
        bases: vec![[0.0, 0.0, 0.0], [80.0, 0.0, 0.0]],
        ..kit()
    }
}

/// Crossfire: two teams, 50 points a player kill.
fn crossfire() -> Def {
    builtin::definitions()[3].clone()
}

fn player(slot: u8, team: u8) -> PlayerView {
    PlayerView {
        slot,
        alive: true,
        team,
        position: Some([0.0; 3]),
    }
}

/// Holdout: 600 warmup ticks, a pool of 12, Ghost in round one, 100 points
/// an Elite and a quarter for an assist.
fn holdout() -> Def {
    builtin::definitions()[0].clone()
}

fn settings() -> Settings {
    Settings {
        seed: 7,
        difficulty: Difficulty::Normal,
        skulls: BTreeSet::new(),
    }
}

fn start(role: Role, game: &FakeGame) -> Host {
    Host::start(role, holdout(), settings(), game).unwrap()
}

/// The next entry after `head`.
fn entry(head: LogHead, tick: u32, op: Command) -> Entry<Command> {
    Entry {
        seq: head.next_seq().unwrap(),
        tick,
        author: None,
        op,
        prev_hash: head.hash,
    }
}

fn feed(host: &mut Host, ops: impl IntoIterator<Item = (u32, Command)>) {
    for (tick, op) in ops {
        let next = entry(host.head(), tick, op);
        host.receive(next).unwrap();
    }
}

fn place(slot: u8, count: u16, zone: u8, weapon_tier: u8) -> Command {
    Command::PlaceSquad {
        slot,
        class: "elite".into(),
        count,
        zone,
        weapon_tier,
    }
}

fn score(player: u8, points: u32) -> Command {
    Command::Score {
        player,
        points,
        reason: ScoreReason::Kill,
    }
}

fn run(host: &mut Host, game: &mut FakeGame, tick: u32) {
    assert!(host.pre_tick(tick, game).is_empty());
    host.post_tick(tick, &*game).unwrap();
}

#[test]
fn place_calls_the_squad_entry_with_class_and_count() {
    let mut game = FakeGame::new();
    let mut host = start(Role::Replica, &game);
    feed(&mut host, [(1, place(3, 4, 0, 2))]);
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(
        game.calls,
        [Call::Squad(SquadOp::Place(Placement {
            slot: 3,
            character: ELITE,
            count: 4,
            zone: 0,
            team: KIT_TEAM,
            weapon_tier: 2,
        }))]
    );
}

/// [`kit`] with more classes, each on its own palette row.
fn mixed_kit() -> Kit {
    Kit {
        classes: [
            ("elite".to_string(), ELITE),
            ("grunt".to_string(), ELITE + 1),
            ("brute".to_string(), ELITE + 2),
        ]
        .into(),
        ..kit()
    }
}

fn place_class(slot: u8, class: &str) -> Command {
    Command::PlaceSquad {
        slot,
        class: class.into(),
        count: 2,
        zone: 0,
        weapon_tier: 0,
    }
}

#[test]
fn each_class_the_kit_has_places_on_its_own_palette_row() {
    let mut game = FakeGame::new();
    game.kit = Some(mixed_kit());
    let mut host = start(Role::Replica, &game);
    feed(
        &mut host,
        [
            (1, place_class(0, "brute")),
            (1, place_class(1, "elite")),
            (1, place_class(2, "grunt")),
        ],
    );
    assert!(host.pre_tick(1, &mut game).is_empty());
    let rows: Vec<_> = game
        .calls
        .iter()
        .map(|call| match call {
            Call::Squad(SquadOp::Place(p)) => (p.slot, p.character),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(rows, [(0, ELITE + 2), (1, ELITE), (2, ELITE + 1)]);
}

#[test]
fn a_wave_naming_a_class_the_map_lacks_is_refused_by_name() {
    // Holdout's first wave, with its first squad a Grunt.
    let mut def = holdout();
    let elite_points = def.scoring.by_class["elite"];
    def.scoring.by_class.insert("grunt".into(), elite_points);
    def.rounds[0].waves[0].squads[0].class = "grunt".into();

    let mut game = FakeGame::new();
    assert_eq!(
        Host::start(Role::Host, def.clone(), settings(), &game).err(),
        Some(Refusal::Def(::firefight::Refusal::UnknownClass(
            "grunt".into()
        )))
    );
    // The same definition on a map whose kit built a Grunt starts, and
    // a class the kit lacks stays refused when a wave names it.
    game.kit = Some(mixed_kit());
    assert!(Host::start(Role::Host, def.clone(), settings(), &game).is_ok());
    def.scoring.by_class.insert("hunter".into(), elite_points);
    def.rounds[0].waves[0].squads[0].class = "hunter".into();
    assert_eq!(
        Host::start(Role::Host, def, settings(), &game).err(),
        Some(Refusal::Def(::firefight::Refusal::UnknownClass(
            "hunter".into()
        )))
    );
}

#[test]
fn a_placement_the_kit_cannot_make_is_an_error_and_no_call() {
    let mut game = FakeGame::new();
    let mut host = start(Role::Replica, &game);
    let unknown = Command::PlaceSquad {
        slot: 0,
        class: "hunter".into(),
        count: 2,
        zone: 0,
        weapon_tier: 0,
    };
    feed(
        &mut host,
        [
            (1, unknown),
            (1, place(0, 2, 5, 0)),
            (1, place(16, 2, 0, 0)),
        ],
    );
    let errors = host.pre_tick(1, &mut game);
    assert_eq!(
        errors.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [Some(1), Some(2), Some(3)]
    );
    assert!(game.calls.is_empty());
    // Applied once: a refused entry is not retried.
    assert!(host.pre_tick(2, &mut game).is_empty());
}

#[test]
fn actors_join_a_team_no_player_holds() {
    let mut game = FakeGame::new();
    assert_eq!(start(Role::Host, &game).team(), KIT_TEAM);
    game.players = vec![player(0, 0), player(1, KIT_TEAM)];
    assert_eq!(start(Role::Host, &game).team(), 1);
    game.players = (0..TEAMS).map(|team| player(team, team)).collect();
    assert_eq!(
        Host::start(Role::Host, holdout(), settings(), &game).err(),
        Some(Refusal::NoFreeTeam)
    );
}

#[test]
fn a_client_applies_no_command() {
    let mut game = FakeGame::new();
    game.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    game.kills = vec![KillSeen {
        character: ELITE,
        killer: Some(0),
        assist: None,
    }];
    let mut host = start(Role::Client, &game);
    feed(
        &mut host,
        [
            (1, place(0, 2, 0, 0)),
            (1, score(0, 100)),
            (2, Command::Respawn { player: 0 }),
            (
                3,
                Command::Finish {
                    outcome: Outcome::Lost,
                },
            ),
        ],
    );
    for tick in 0..10 {
        run(&mut host, &mut game, tick);
    }
    assert!(game.calls.is_empty());
    // The client keeps the log for its digest and writes none of its own.
    assert_eq!(host.head().seq, 4);
    assert!(host.state().is_none());
}

#[test]
fn score_credits_the_killer() {
    let mut game = FakeGame::new();
    game.players = vec![player(0, 0), player(1, 0), player(2, 0)];
    game.kills = vec![KillSeen {
        character: ELITE,
        killer: Some(2),
        assist: Some(1),
    }];
    let mut host = start(Role::Host, &game);
    host.post_tick(0, &game).unwrap();
    game.kills.clear();
    assert!(host.pre_tick(0, &mut game).is_empty());
    assert!(game.calls.is_empty(), "entries wait for the next tick");
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(
        game.calls,
        [
            Call::Lives(None, 12),
            Call::Score(2, 100),
            Call::Score(1, 25)
        ]
    );
}

#[test]
fn a_kill_of_a_character_outside_the_kit_scores_nothing() {
    let mut game = FakeGame::new();
    game.kills = vec![KillSeen {
        character: ELITE + 1,
        killer: Some(0),
        assist: None,
    }];
    let mut host = start(Role::Host, &game);
    run(&mut host, &mut game, 0);
    run(&mut host, &mut game, 1);
    assert_eq!(game.calls, [Call::Lives(None, 12)]);
}

#[test]
fn entries_apply_at_their_tick_in_seq_order() {
    let mut game = FakeGame::new();
    let mut host = start(Role::Replica, &game);
    feed(
        &mut host,
        [
            (5, score(0, 1)),
            (5, score(1, 2)),
            (7, Command::Respawn { player: 0 }),
        ],
    );
    assert!(host.pre_tick(4, &mut game).is_empty());
    assert!(game.calls.is_empty());
    assert!(host.pre_tick(5, &mut game).is_empty());
    assert_eq!(game.calls, [Call::Score(0, 1), Call::Score(1, 2)]);
    assert!(host.pre_tick(6, &mut game).is_empty());
    assert_eq!(game.calls.len(), 2);
    assert!(host.pre_tick(7, &mut game).is_empty());
    assert!(host.pre_tick(9, &mut game).is_empty());
    assert_eq!(
        game.calls,
        [Call::Score(0, 1), Call::Score(1, 2), Call::Respawn(0)]
    );

    // The chain refuses an entry out of sequence.
    let head = host.head();
    let mut skipped = entry(head, 9, score(0, 3));
    skipped.seq += 1;
    assert!(host.receive(skipped).is_err());
    assert_eq!(host.head(), head);
}

#[test]
fn the_host_writes_its_own_log() {
    let game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    let next = entry(host.head(), 1, score(0, 1));
    assert!(host.receive(next).is_err());
}

#[test]
fn a_map_without_the_kit_refuses_the_mode() {
    let mut game = FakeGame::new();
    game.kit = None;
    for role in [Role::Host, Role::Replica, Role::Client] {
        assert_eq!(
            Host::start(role, holdout(), settings(), &game).err(),
            Some(Refusal::NoKit)
        );
    }

    let mut without_elites = kit();
    without_elites.classes = [("grunt".to_string(), ELITE)].into();
    game.kit = Some(without_elites);
    assert_eq!(
        Host::start(Role::Host, holdout(), settings(), &game).err(),
        Some(Refusal::Def(::firefight::Refusal::UnknownClass(
            "elite".into()
        )))
    );

    let mut no_zones = kit();
    no_zones.zones.clear();
    game.kit = Some(no_zones);
    assert_eq!(
        Host::start(Role::Host, holdout(), settings(), &game).err(),
        Some(Refusal::NoZones)
    );
    assert!(game.calls.is_empty());
}

#[test]
fn the_ai_switch_waits_for_the_globals_and_writes_only_when_off() {
    let mut game = FakeGame::new();
    game.ai = None;
    let mut host = start(Role::Host, &game);
    assert!(host.pre_tick(0, &mut game).is_empty());
    game.ai = Some(AiGlobals {
        on: false,
        initialised: false,
    });
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert!(game.calls.is_empty());
    game.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    for tick in 2..5 {
        assert!(host.pre_tick(tick, &mut game).is_empty());
    }
    assert_eq!(game.calls, [Call::AiOn]);

    // P2 answers "on": the check writes nothing.
    let mut on = FakeGame::new();
    let mut host = start(Role::Replica, &on);
    for tick in 0..3 {
        assert!(host.pre_tick(tick, &mut on).is_empty());
    }
    assert!(on.calls.is_empty());
}

/// One player against Holdout's first wave: warmup, the squad placed, the
/// Elite killed, then the player dies between waves and respawns from the
/// pool.
fn scripted_match(game: &mut FakeGame, host: &mut Host) {
    for tick in 0..=720 {
        if tick == 601 {
            // The squad the entry placed this tick is in the game.
            assert!(host.pre_tick(tick, game).is_empty());
            game.actors[0] = 1;
            host.post_tick(tick, &*game).unwrap();
            continue;
        }
        if tick == 650 {
            game.actors[0] = 0;
            game.kills = vec![KillSeen {
                character: ELITE,
                killer: Some(0),
                assist: None,
            }];
        }
        if tick == 700 {
            game.players[0].alive = false;
            game.players[0].position = None;
        }
        run(host, game, tick);
        game.kills.clear();
    }
}

fn expected_calls() -> Vec<Call> {
    vec![
        Call::AiOn,
        Call::Lives(None, 12),
        Call::Skull(Skull::Ghost, true),
        Call::Squad(SquadOp::Place(Placement {
            slot: 0,
            character: ELITE,
            count: 1,
            zone: 0,
            team: KIT_TEAM,
            weapon_tier: 0,
        })),
        // 100 points, normal difficulty, Ghost at 1,100 thousandths.
        Call::Score(0, 110),
        Call::Respawn(0),
        Call::Lives(None, 11),
    ]
}

#[test]
fn a_scripted_match_makes_the_expected_engine_calls_in_order() {
    let mut game = FakeGame::new();
    game.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    let mut host = start(Role::Host, &game);
    scripted_match(&mut game, &mut host);
    assert_eq!(game.calls, expected_calls());
    assert_eq!(
        host.log()
            .since(0)
            .iter()
            .map(|entry| entry.tick)
            .collect::<Vec<_>>(),
        [1, 601, 601, 601, 601, 651, 701, 701]
    );
    host.log().verify(host.head()).unwrap();
}

#[test]
fn a_replica_fed_the_hosts_log_makes_the_same_engine_calls() {
    let mut game = FakeGame::new();
    game.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    let mut host = start(Role::Host, &game);
    scripted_match(&mut game, &mut host);

    let mut replay = FakeGame::new();
    replay.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    let mut replica = start(Role::Replica, &replay);
    for entry in host.log().since(0) {
        replica.receive(entry.clone()).unwrap();
    }
    for tick in 0..=720 {
        assert!(replica.pre_tick(tick, &mut replay).is_empty());
    }
    assert_eq!(replay.calls, expected_calls());
    assert_eq!(replica.head(), host.head());
}

#[test]
fn an_operator_command_applies_at_its_tick_and_names_its_author() {
    let mut game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    host.set_held(true).unwrap();
    let e = host.append(5, Command::Respawn { player: 0 }).unwrap();
    assert_eq!((e.seq, e.tick), (1, 5));
    assert_eq!(e.author.as_deref(), Some(OPERATOR));
    // Never before the head's tick.
    let late = host.append(2, Command::EraseSquad { slot: 0 }).unwrap();
    assert_eq!((late.seq, late.tick), (2, 5));
    assert!(host.pre_tick(4, &mut game).is_empty());
    assert!(game.calls.is_empty());
    assert!(host.pre_tick(5, &mut game).is_empty());
    assert_eq!(
        game.calls,
        [Call::Respawn(0), Call::Squad(SquadOp::Erase { slot: 0 })]
    );
    assert_eq!(host.applied(), 2);
}

#[test]
fn a_held_director_issues_nothing_until_it_runs_again() {
    let mut game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    host.set_held(true).unwrap();
    for tick in 0..2000 {
        run(&mut host, &mut game, tick);
    }
    assert_eq!(host.head().seq, 0, "a held director appends nothing");
    assert!(!host.state().unwrap().started);
    host.set_held(false).unwrap();
    let mut appended = 0;
    for tick in 2000..2100 {
        assert!(host.pre_tick(tick, &mut game).is_empty());
        appended += host.post_tick(tick, &game).unwrap();
    }
    assert!(appended > 0, "the director runs again");
}

#[test]
fn only_the_host_takes_operator_commands_or_holds() {
    let game = FakeGame::new();
    for role in [Role::Replica, Role::Client] {
        let mut host = start(role, &game);
        assert!(host.append(1, Command::Finish { outcome: Outcome::Won }).is_err());
        assert!(host.set_held(true).is_err());
        assert_eq!(host.head().seq, 0);
    }
}

#[test]
fn credits_pair_each_kill_with_its_class_and_assist() {
    let mut game = FakeGame::new();
    game.players = vec![player(0, 0), player(1, 0), player(2, 0)];
    game.kills = vec![KillSeen {
        character: ELITE,
        killer: Some(2),
        assist: Some(1),
    }];
    let mut host = start(Role::Host, &game);
    host.post_tick(0, &game).unwrap();
    assert_eq!(
        host.take_credits(),
        [
            Credit {
                tick: 0,
                player: 2,
                points: 100,
                reason: ScoreReason::Kill,
                class: Some("elite".into())
            },
            Credit {
                tick: 0,
                player: 1,
                points: 25,
                reason: ScoreReason::Assist,
                class: None
            },
        ]
    );
    assert!(host.take_credits().is_empty(), "each credit is taken once");
    assert_eq!(host.take_ended(), None);
}

#[test]
fn a_score_with_no_kill_to_pair_keeps_its_points() {
    let kill = |class: &str, killer: Option<u8>| Kill {
        class: class.into(),
        killer,
        assist: None,
    };
    let seen = Observed {
        kills: vec![kill("elite", Some(0)), kill("elite", None), kill("hunter", Some(0))],
        ..Default::default()
    };
    // "hunter" scores nothing; an unclaimed kill scores nothing; a score the
    // kills do not explain is kept with no class.
    let commands = [score(0, 100), score(3, 5)];
    let got = credits(9, &seen, &commands, |class| class == "elite");
    assert_eq!(
        got.iter().map(|c| (c.player, c.points, c.class.as_deref())).collect::<Vec<_>>(),
        [(0, 100, Some("elite")), (3, 5, None)]
    );
}

/// Round 3 of 3 (index 2), four respawns, the teams at 300 and 150.
fn at_round_three(last_wave_cleared: bool) -> Progress {
    Progress {
        round: 2,
        rounds: 3,
        last_wave_cleared,
        respawns: 4,
        teams: 2,
        scores: [300, 150],
    }
}

#[test]
fn a_finish_makes_the_result_with_the_rounds_it_cleared() {
    let finish = |outcome| [Command::Respawn { player: 0 }, Command::Finish { outcome }];
    let won = result(5, "Holdout".into(), at_round_three(true), &finish(Outcome::Won)).unwrap();
    assert_eq!((won.outcome, won.rounds_cleared, won.lives_used), (Outcome::Won, 3, 4));
    let lost = result(5, "Holdout".into(), at_round_three(false), &finish(Outcome::Lost)).unwrap();
    assert_eq!((lost.outcome, lost.rounds_cleared, lost.set.as_str()), (Outcome::Lost, 2, "Holdout"));
    assert_eq!(result(5, "x".into(), at_round_three(false), &[score(0, 1)]), None);
    // Two teams: after the last wave every round was cleared; out of lives
    // before it, the rounds before the one they were in.
    let team_won = Outcome::TeamWon { team: 1 };
    let last = result(5, "Crossfire".into(), at_round_three(true), &finish(team_won)).unwrap();
    assert_eq!(
        (last.outcome, last.rounds_cleared, last.teams, last.scores),
        (team_won, 3, 2, [300, 150])
    );
    let early = result(5, "Crossfire".into(), at_round_three(false), &finish(team_won)).unwrap();
    assert_eq!(early.rounds_cleared, 2);
    let draw = result(5, "Crossfire".into(), at_round_three(true), &finish(Outcome::Draw)).unwrap();
    assert_eq!((draw.outcome, draw.rounds_cleared), (Outcome::Draw, 3));
}

/// A player kill's score is credited with no class: it was for no actor.
#[test]
fn a_player_kill_is_credited_with_no_class() {
    let seen = Observed {
        kills: vec![Kill {
            class: "elite".into(),
            killer: Some(0),
            assist: None,
        }],
        player_kills: vec![PlayerKill {
            killer: Some(0),
            victim: 1,
        }],
        ..Default::default()
    };
    let commands = [
        score(0, 100),
        Command::Score {
            player: 0,
            points: 50,
            reason: ScoreReason::PlayerKill,
        },
    ];
    let got = credits(9, &seen, &commands, |class| class == "elite");
    assert_eq!(
        got.iter()
            .map(|c| (c.reason, c.points, c.class.as_deref()))
            .collect::<Vec<_>>(),
        [
            (ScoreReason::Kill, 100, Some("elite")),
            (ScoreReason::PlayerKill, 50, None)
        ]
    );
}

#[test]
fn observed_carries_each_players_team_and_player_kills() {
    let mut game = FakeGame::two_teams();
    game.players.push(player(4, 2));
    game.player_kills = vec![
        PlayerKill {
            killer: Some(0),
            victim: 1,
        },
        PlayerKill {
            killer: None,
            victim: 4,
        },
    ];
    let seen = observe(&kit(), &BTreeMap::new(), 2, &game);
    assert_eq!(
        seen.players,
        [
            PlayerSeen {
                slot: 0,
                alive: true,
                team: 0
            },
            PlayerSeen {
                slot: 1,
                alive: true,
                team: 1
            },
            // The game team, as read: the director maps 2 to Firefight
            // team 1.
            PlayerSeen {
                slot: 4,
                alive: true,
                team: 2
            },
        ]
    );
    assert_eq!(seen.player_kills, game.player_kills);
}

#[test]
fn zone_team_distance_is_from_each_teams_centre() {
    let mut game = FakeGame::two_teams();
    // Team 0 at x 0 and 20 (centre 10); team 1 at x 60 with game teams 1
    // and 3; a player of team 1 between lives counts for neither.
    game.players = vec![
        PlayerView {
            position: Some([0.0, 0.0, 0.0]),
            ..player(0, 0)
        },
        PlayerView {
            position: Some([20.0, 0.0, 0.0]),
            ..player(1, 0)
        },
        PlayerView {
            position: Some([60.0, 0.0, 0.0]),
            ..player(2, 1)
        },
        PlayerView {
            position: Some([60.0, 0.0, 0.0]),
            ..player(3, 3)
        },
        PlayerView {
            alive: false,
            position: None,
            ..player(5, 1)
        },
    ];
    let zone = kit().zones[0];
    let seen = observe(&kit(), &BTreeMap::new(), 2, &game);
    // All four units' centre is x 35: 5 world units from the zone at 40.
    assert_eq!(seen.zones[0].distance, 500);
    assert_eq!(seen.zones[0].team_distance, [3_000, 2_000]);
    assert_eq!(zone, [40.0, 0.0, 0.0]);
    // A team with no unit is as far as can be.
    game.players.retain(|p| p.team == 0);
    let seen = observe(&kit(), &BTreeMap::new(), 2, &game);
    assert_eq!(seen.zones[0].team_distance, [3_000, u32::MAX]);
    // One team: every player is team 0's.
    game.players.push(PlayerView {
        position: Some([60.0, 0.0, 0.0]),
        ..player(2, 1)
    });
    let seen = observe(&kit(), &BTreeMap::new(), 1, &game);
    assert_eq!(seen.zones[0].team_distance, [seen.zones[0].distance, u32::MAX]);
}

#[test]
fn hunt_goes_to_the_targets_base() {
    let mut game = FakeGame::two_teams();
    let mut host = Host::start(Role::Replica, crossfire(), settings(), &game).unwrap();
    feed(
        &mut host,
        [
            (1, Command::Target { slot: 3, team: 1 }),
            (1, Command::Hunt { slot: 3 }),
            // A slot never sent anywhere goes to the home base.
            (1, Command::Hunt { slot: 4 }),
            (2, Command::Target { slot: 3, team: 0 }),
            (2, Command::Hunt { slot: 3 }),
            // No third base: refused, and the slot keeps its target.
            (3, Command::Target { slot: 3, team: 2 }),
        ],
    );
    assert!(host.pre_tick(2, &mut game).is_empty());
    assert_eq!(
        game.calls,
        [
            Call::Squad(SquadOp::Hunt { slot: 3, base: 1 }),
            Call::Squad(SquadOp::Hunt { slot: 4, base: 0 }),
            Call::Squad(SquadOp::Hunt { slot: 3, base: 0 }),
        ]
    );
    let errors = host.pre_tick(3, &mut game);
    assert_eq!(errors.iter().map(|e| e.seq).collect::<Vec<_>>(), [Some(6)]);
    assert_eq!(host.targets()[3], 0);
}

#[test]
fn two_teams_need_a_base_each() {
    let mut game = FakeGame::two_teams();
    game.kit = Some(kit());
    assert_eq!(
        Host::start(Role::Host, crossfire(), settings(), &game).err(),
        Some(Refusal::Bases { have: 1, need: 2 })
    );
    // One team needs none beyond the home base.
    assert!(Host::start(Role::Host, holdout(), settings(), &game).is_ok());
}

/// Two teams' pools reach the game and the HUD's blob, and a team's win
/// ends the game and is the blob's stage.
#[test]
fn team_lives_and_team_won_reach_the_blob() {
    use crate::firefight::hud;
    let mut game = FakeGame::two_teams();
    let mut host = Host::start(Role::Host, crossfire(), settings(), &game).unwrap();
    run(&mut host, &mut game, 0);
    run(&mut host, &mut game, 1);
    assert_eq!(game.calls, [Call::TeamLives(0, 10), Call::TeamLives(1, 10)]);
    let mode = hud::Mode::of(host.director().unwrap(), 2, None).unwrap();
    assert_eq!(mode.lives, hud::Lives::Teams([10, 10]));
    assert_eq!(
        mode.teams,
        Some(hud::Teams {
            scores: [0, 0],
            second: 1 << 1
        })
    );
    let blob = mode.encode();
    assert_eq!(hud::Mode::decode(&blob), Ok(mode));

    host.set_held(true).unwrap();
    let won = Outcome::TeamWon { team: 1 };
    host.append(2, Command::Finish { outcome: won }).unwrap();
    assert!(host.pre_tick(2, &mut game).is_empty());
    assert_eq!(game.calls.last(), Some(&Call::End(won)));
    let mode = hud::Mode::of(host.director().unwrap(), 2, Some(won)).unwrap();
    assert_eq!(mode.stage, hud::Stage::Team1Won);
    let view = hud::view(&hud::Mode::decode(&mode.encode()).unwrap(), 0, hud::Scope::Match);
    assert_eq!(view.get(hud::BANNER).unwrap().text, "BLUE TEAM WINS");
}

/// The engine keeps no team pool, so a `TeamLives` sets the director's:
/// an operator's empties team 1's pool, which its respawns then spend from
/// (the director holds them; firefight's `teams.rs` plays it), and the HUD
/// shows it. A count the match does not keep is refused by entry.
#[test]
fn an_operator_team_pool_is_the_one_the_director_spends() {
    use crate::firefight::hud;
    let mut game = FakeGame::two_teams();
    let mut host = Host::start(Role::Host, crossfire(), settings(), &game).unwrap();
    run(&mut host, &mut game, 0);
    run(&mut host, &mut game, 1);
    assert_eq!(host.director().unwrap().state().pools, [10, 10]);
    host.set_held(true).unwrap();
    let empty = host.append(2, Command::TeamLives { team: 1, pool: 0 }).unwrap();
    let own = host
        .append(
            2,
            Command::Lives {
                player: Some(0),
                pool: 3,
            },
        )
        .unwrap();
    let errors = host.pre_tick(2, &mut game);
    assert_eq!(
        errors.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [Some(own.seq)],
        "Crossfire's lives are each team's pool, not a player's own"
    );
    assert!(empty.seq < own.seq);
    assert_eq!(host.director().unwrap().state().pools, [10, 0]);
    assert_eq!(game.calls.last(), Some(&Call::TeamLives(1, 0)));
    let mode = hud::Mode::of(host.director().unwrap(), 2, None).unwrap();
    assert_eq!(mode.lives, hud::Lives::Teams([10, 0]));
    // A replica keeps no director: the entry is the game's alone.
    let mut replay = FakeGame::two_teams();
    let mut replica = Host::start(Role::Replica, crossfire(), settings(), &replay).unwrap();
    for entry in host.log().since(0) {
        replica.receive(entry.clone()).unwrap();
    }
    assert!(replica.pre_tick(2, &mut replay).is_empty());
    assert!(replay.calls.contains(&Call::TeamLives(1, 0)));
}

#[test]
fn a_draw_ends_the_game() {
    let mut game = FakeGame::two_teams();
    let mut host = Host::start(Role::Host, crossfire(), settings(), &game).unwrap();
    host.set_held(true).unwrap();
    host.append(1, Command::Finish {
        outcome: Outcome::Draw,
    })
    .unwrap();
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(game.calls, [Call::End(Outcome::Draw)]);
    // A replica fed the host's log ends its game the same way.
    let mut replay = FakeGame::two_teams();
    let mut replica = Host::start(Role::Replica, crossfire(), settings(), &replay).unwrap();
    for entry in host.log().since(0) {
        replica.receive(entry.clone()).unwrap();
    }
    assert!(replica.pre_tick(1, &mut replay).is_empty());
    assert_eq!(replay.calls, [Call::End(Outcome::Draw)]);
}

#[test]
fn the_actors_take_a_third_team_with_two_player_teams() {
    let mut game = FakeGame::two_teams();
    // The kit's team is the second player team: the actors move past both.
    let mut on_blue = two_base_kit();
    on_blue.team = 1;
    game.kit = Some(on_blue);
    let host = Host::start(Role::Host, crossfire(), settings(), &game).unwrap();
    assert_eq!(host.team(), 2);
    // Even with nobody on team 1 yet.
    game.players = vec![player(0, 0)];
    let host = Host::start(Role::Host, crossfire(), settings(), &game).unwrap();
    assert_eq!(host.team(), 2);
    // A player on 2 moves them to 3; the kit's own team when it is free.
    game.players = vec![player(0, 0), player(1, 1), player(2, 2)];
    let host = Host::start(Role::Host, crossfire(), settings(), &game).unwrap();
    assert_eq!(host.team(), 3);
    game.kit = Some(two_base_kit());
    let host = Host::start(Role::Host, crossfire(), settings(), &game).unwrap();
    assert_eq!(host.team(), KIT_TEAM);
    assert!(host.team() > 1, "never a player team");
    // One team keeps team 1 free for the actors.
    assert_eq!(actor_team(1, 0, &BTreeSet::new()), Ok(1));
    assert_eq!(actor_team(2, 0, &BTreeSet::new()), Ok(2));
}

/// One tick of [`scripted_match`]'s game, with what the runtime does
/// around the host: the entries the pre-tick applied, as a film records
/// them, and the step's mode blob into the log (`runtime::post_tick`).
fn recorded_tick(host: &mut Host, game: &mut FakeGame, tick: u32) -> Vec<Vec<u8>> {
    use crate::firefight::{film, hud};
    if tick == 650 {
        game.actors[0] = 0;
        game.kills = vec![KillSeen {
            character: ELITE,
            killer: Some(0),
            assist: None,
        }];
    }
    if tick == 700 {
        game.players[0].alive = false;
        game.players[0].position = None;
    }
    let before = host.applied();
    assert!(host.pre_tick(tick, game).is_empty());
    let applied = host.applied();
    let payloads = host
        .log()
        .since(before)
        .iter()
        .take_while(|entry| entry.seq <= applied)
        .map(|entry| film::payload(entry).unwrap())
        .collect();
    if tick == 601 {
        game.actors[0] = 1;
    }
    host.post_tick(tick, &*game).unwrap();
    if let Ok(mode) = hud::Mode::of(host.director().unwrap(), 1, None) {
        host.note_mode(tick, &mode.encode()).unwrap();
    }
    game.kills.clear();
    payloads
}

/// The film's time chunks of `recorded` (film tick = game tick + 1), read
/// back: each game tick's Firefight entry records, in record order.
fn through_a_film(recorded: &[Vec<Vec<u8>>]) -> Vec<Vec<Vec<u8>>> {
    use live_proto::film::stream;
    let mut back = vec![Vec::new(); recorded.len()];
    for (c, chunk) in recorded.chunks(stream::MAX_TICKS).enumerate() {
        let first = (c * stream::MAX_TICKS) as u64 + 1;
        let mut out = stream::ChunkWriter::new(first, chunk.len() as u16).unwrap();
        for (i, payloads) in chunk.iter().enumerate() {
            for p in payloads {
                out.record(first + i as u64, stream::FIREFIGHT, p).unwrap();
            }
        }
        let raw = out.finish();
        for r in stream::Chunk::parse(&raw).unwrap().records() {
            let r = r.unwrap();
            assert_eq!(r.tag, stream::FIREFIGHT);
            back[(r.tick - 1) as usize].push(r.payload.to_vec());
        }
    }
    back
}

/// AN14: the host log carries the mode state. A film's playback, fed each
/// tick's records before the tick as the theater hands them over, applies
/// the host's entries at their ticks: the same engine calls, the same log,
/// and at every tick the mode blob the host held.
#[test]
fn a_film_replays_entries_at_their_ticks() {
    use crate::firefight::film;
    let mut game = FakeGame::new();
    game.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    let mut host = start(Role::Host, &game);
    let mut recorded = Vec::new();
    let mut held = Vec::new();
    for tick in 0..=720 {
        recorded.push(recorded_tick(&mut host, &mut game, tick));
        held.push(host.mode_state().map(<[u8]>::to_vec));
    }
    assert_eq!(game.calls, expected_calls(), "the mode state makes no call");
    let blobs: BTreeSet<_> = held.iter().flatten().collect();
    assert!(blobs.len() > 1, "the blob changes over the match: {blobs:?}");
    assert!(
        host.log()
            .since(0)
            .iter()
            .any(|e| matches!(e.op, Command::ModeState { .. }))
    );

    let mut replay = FakeGame::new();
    replay.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    let mut replica = start(Role::Replica, &replay);
    for (tick, payloads) in (0..).zip(through_a_film(&recorded)) {
        for p in payloads {
            replica.receive(film::parse(&p).unwrap()).unwrap();
        }
        assert!(replica.pre_tick(tick, &mut replay).is_empty());
        assert_eq!(
            replica.mode_state().map(<[u8]>::to_vec),
            held[tick as usize],
            "tick {tick}"
        );
    }
    assert_eq!(replay.calls, expected_calls());
    assert_eq!(replica.applied(), host.applied());
    let applied = replica.log().since(0);
    assert_eq!(applied, &host.log().since(0)[..applied.len()]);
    replica.log().verify(replica.head()).unwrap();
}

/// A film's playback runs no producer: its director is none, so its steps
/// append nothing and its mode state is only what the log brought.
#[test]
fn playback_never_runs_a_producer() {
    let mut game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    for tick in 0..=10 {
        recorded_tick(&mut host, &mut game, tick);
    }
    let mut replay = FakeGame::new();
    let mut replica = start(Role::Replica, &replay);
    assert!(replica.director().is_none());
    for entry in host.log().since(0) {
        replica.receive(entry.clone()).unwrap();
    }
    let head = replica.head();
    for tick in 0..=10 {
        assert!(replica.pre_tick(tick, &mut replay).is_empty());
        assert_eq!(replica.post_tick(tick, &replay).unwrap(), 0);
        assert!(!replica.note_mode(tick, b"not the host's").unwrap());
    }
    assert_eq!(replica.head(), head, "the replica appended nothing");
    assert_eq!(replica.mode_state(), host.mode_state());
}

/// The host appends the mode blob only when it changes.
#[test]
fn the_mode_state_is_appended_when_it_changes() {
    let game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    assert!(host.note_mode(4, &[3, 1]).unwrap());
    assert!(!host.note_mode(5, &[3, 1]).unwrap());
    assert!(host.note_mode(6, &[3, 2]).unwrap());
    let ticks: Vec<_> = host.log().since(0).iter().map(|e| e.tick).collect();
    assert_eq!(ticks, [5, 7], "each for the next tick");
    assert_eq!(host.mode_state(), None, "until it applies");
    let mut game = game;
    assert!(host.pre_tick(7, &mut game).is_empty());
    assert_eq!(host.mode_state(), Some(&[3, 2][..]));
}

/// A test of Holdout over the scripted match makes the match's calls in
/// order, the squad's placement among them, less the lives and the
/// respawn: the director runs as in a match.
#[test]
fn a_test_makes_a_matchs_squad_calls_in_order() {
    let mut game = FakeGame::new();
    game.ai = Some(AiGlobals {
        on: false,
        initialised: true,
    });
    let mut test = Host::start_test(holdout(), settings(), &game).unwrap();
    assert!(test.test() && test.role() == Role::Host);
    assert!(!start(Role::Host, &game).test());
    scripted_match(&mut game, &mut test);
    let want: Vec<Call> = expected_calls()
        .into_iter()
        .filter(|c| !matches!(c, Call::Lives(..) | Call::TeamLives(..) | Call::Respawn(_)))
        .collect();
    assert!(want.iter().any(|c| matches!(c, Call::Squad(SquadOp::Place(_)))));
    assert_eq!(game.calls, want);
    // The director still wrote every entry a match's writes.
    assert_eq!(test.log().since(0).len(), 8);
}

/// In a test the lives and the respawns are not the game's: an operator's
/// `Lives` and `Respawn` make no call, where a match's make one each.
#[test]
fn lives_and_respawn_are_not_applied_in_a_test() {
    let ops = [
        Command::Lives {
            player: None,
            pool: 3,
        },
        Command::Respawn { player: 0 },
    ];
    let mut game = FakeGame::new();
    let mut test = Host::start_test(holdout(), settings(), &game).unwrap();
    for op in ops.clone() {
        test.append(1, op).unwrap();
    }
    assert!(test.pre_tick(1, &mut game).is_empty());
    assert!(game.calls.is_empty(), "{:?}", game.calls);
    assert_eq!(test.applied(), 2, "applied, as no-ops");

    let mut game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    for op in ops {
        host.append(1, op).unwrap();
    }
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(game.calls, [Call::Lives(None, 3), Call::Respawn(0)]);
}

/// A test's `Finish` ends the test, once, and not the game: no end call.
#[test]
fn finish_ends_the_test_and_not_the_game() {
    let mut game = FakeGame::new();
    let mut test = Host::start_test(holdout(), settings(), &game).unwrap();
    test.append(
        1,
        Command::Finish {
            outcome: Outcome::Won,
        },
    )
    .unwrap();
    assert!(test.pre_tick(1, &mut game).is_empty());
    assert!(game.calls.is_empty(), "{:?}", game.calls);
    assert_eq!(test.take_test_end(), Some(Outcome::Won));
    assert_eq!(test.take_test_end(), None, "once");
    // A match's Finish is the game's end.
    let mut game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    host.append(
        1,
        Command::Finish {
            outcome: Outcome::Won,
        },
    )
    .unwrap();
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(game.calls, [Call::End(Outcome::Won)]);
    assert_eq!(host.take_test_end(), None);
}
