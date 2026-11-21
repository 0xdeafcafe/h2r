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
            calls: Vec::new(),
        }
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
    fn credit_score(&mut self, player: u8, points: u32) -> anyhow::Result<()> {
        self.calls.push(Call::Score(player, points));
        Ok(())
    }
    fn set_lives(&mut self, player: Option<u8>, pool: u16) -> anyhow::Result<()> {
        self.calls.push(Call::Lives(player, pool));
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

/// One zone, so the director's zone choice draws nothing.
fn kit() -> Kit {
    Kit {
        zones: vec![[40.0, 0.0, 0.0]],
        classes: [("elite".to_string(), ELITE)].into(),
        team: KIT_TEAM,
    }
}

fn player(slot: u8, team: u8) -> PlayerView {
    PlayerView {
        slot,
        alive: true,
        team,
        position: Some([0.0; 3]),
    }
}

/// Holdout: 600 warmup ticks, a pool of 12, Blind in round one, 100 points
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
        Call::Skull(Skull::Blind, true),
        Call::Squad(SquadOp::Place(Placement {
            slot: 0,
            character: ELITE,
            count: 1,
            zone: 0,
            team: KIT_TEAM,
            weapon_tier: 0,
        })),
        // 100 points, normal difficulty, Blind at 1,100 thousandths.
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
