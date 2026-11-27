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
    Fly(Launch),
    Drop(Boarding),
    Hover(u32, bool),
    Unload(u32),
    Park(u32),
}

struct FakeGame {
    kit: Option<Kit>,
    ai: Option<AiGlobals>,
    players: Vec<PlayerView>,
    actors: [u16; SQUAD_SLOTS],
    kills: Vec<KillSeen>,
    player_kills: Vec<PlayerKill>,
    calls: Vec<Call>,
    ships: [Option<Ship>; 2],
    paths: Vec<Option<ShipPath>>,
    pods: Vec<Option<u32>>,
    flood_pods: usize,
    falls: Vec<Option<PodFall>>,
    /// The engine places no actor: every placement is refused.
    refuse_places: bool,
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
            ships: [None, None],
            paths: Vec::new(),
            pods: Vec::new(),
            flood_pods: 0,
            falls: Vec::new(),
            refuse_places: false,
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
        let refused = self.refuse_places && matches!(op, SquadOp::Place(_));
        self.calls.push(Call::Squad(op));
        anyhow::ensure!(!refused, "the engine placed no actor for slot 0: class row 0 at zone 7");
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
    fn fly(&mut self, launch: Launch) -> anyhow::Result<()> {
        self.calls.push(Call::Fly(launch));
        Ok(())
    }
    fn drop_squad(&mut self, boarding: Boarding) -> anyhow::Result<()> {
        self.calls.push(Call::Drop(boarding));
        Ok(())
    }
    fn ship(&self, kind: ShipKind) -> Option<Ship> {
        self.ships[kind.index()]
    }
    fn paths(&self) -> Vec<Option<ShipPath>> {
        self.paths.clone()
    }
    fn hover(&mut self, ship: u32, on: bool) -> anyhow::Result<()> {
        self.calls.push(Call::Hover(ship, on));
        Ok(())
    }
    fn unload(&mut self, ship: u32) -> anyhow::Result<()> {
        self.calls.push(Call::Unload(ship));
        Ok(())
    }
    fn squad_pods(&self) -> Vec<Option<u32>> {
        self.pods.clone()
    }
    fn flood_pod_count(&self) -> usize {
        self.flood_pods
    }
    fn falls(&self) -> Vec<Option<PodFall>> {
        self.falls.clone()
    }
    fn park_pod(&mut self, pod: u32) -> anyhow::Result<()> {
        self.calls.push(Call::Park(pod));
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
        pod_classes: BTreeSet::new(),
        flood_classes: BTreeSet::new(),
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

/// Holdout: a one-second warmup, a pool of 12, Ghost in round one, 100 points
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
    for tick in 0..=150 {
        if tick == 31 {
            // The squad the entry placed this tick is in the game.
            assert!(host.pre_tick(tick, game).is_empty());
            game.actors[0] = 1;
            host.post_tick(tick, &*game).unwrap();
            continue;
        }
        if tick == 80 {
            game.actors[0] = 0;
            game.kills = vec![KillSeen {
                character: ELITE,
                killer: Some(0),
                assist: None,
            }];
        }
        if tick == 130 {
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
        [1, 31, 31, 31, 31, 81, 131, 131]
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
        sets: 0,
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
    // A finite set is one set: cleared when it ends won.
    assert_eq!((won.sets_cleared, lost.sets_cleared, draw.sets_cleared), (1, 0, 1));
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
    let seen = observe(&kit(), &BTreeMap::new(), 2, &SquadPods::new(0, 0), &game);
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
    let seen = observe(&kit(), &BTreeMap::new(), 2, &SquadPods::new(0, 0), &game);
    // All four units' centre is x 35: 5 world units from the zone at 40.
    assert_eq!(seen.zones[0].distance, 500);
    assert_eq!(seen.zones[0].team_distance, [3_000, 2_000]);
    assert_eq!(zone, [40.0, 0.0, 0.0]);
    // A team with no unit is as far as can be.
    game.players.retain(|p| p.team == 0);
    let seen = observe(&kit(), &BTreeMap::new(), 2, &SquadPods::new(0, 0), &game);
    assert_eq!(seen.zones[0].team_distance, [3_000, u32::MAX]);
    // One team: every player is team 0's.
    game.players.push(PlayerView {
        position: Some([60.0, 0.0, 0.0]),
        ..player(2, 1)
    });
    let seen = observe(&kit(), &BTreeMap::new(), 1, &SquadPods::new(0, 0), &game);
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

/// Holdout's first wave to tick 400 on a game where `shows` actors are in
/// slot 0 from the tick after the squad was placed (31).
fn first_wave(shows: u16) -> (Host, FakeGame) {
    let mut game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    for tick in 0..=400 {
        if tick == 31 {
            assert!(host.pre_tick(tick, &mut game).is_empty());
            game.actors[0] = shows;
            host.post_tick(tick, &game).unwrap();
            continue;
        }
        run(&mut host, &mut game, tick);
    }
    (host, game)
}

/// A wave whose actors the game shows stays fought past the director's
/// slot timeout (3 s after the placement); the same wave with none seen
/// is cleared at it, which is the loop a local game made.
#[test]
fn a_wave_whose_actors_are_seen_is_not_cleared_at_the_slot_timeout() {
    use ::firefight::director::Phase;
    let (host, _) = first_wave(1);
    assert_eq!(host.director().unwrap().state().phase, Phase::Fighting);
    assert_eq!(host.placements_unseen(), 0);
    let (host, _) = first_wave(0);
    assert_ne!(host.director().unwrap().state().phase, Phase::Fighting);
}

#[test]
fn a_placement_nobody_sees_is_reported_once() {
    let (mut host, _) = first_wave(0);
    assert_eq!(host.placements_unseen(), 1);
    let rows = host.take_unseen_errors();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, None);
    assert_eq!(
        rows[0].error.to_string(),
        "slot 0: elite placed at tick 31, none seen"
    );
    assert!(host.take_unseen_errors().is_empty());
}

/// Holdout's opening wave on a game whose engine places no actor: the
/// placement is refused at once (`sync`), or by the runtime after the
/// tick's calls (`Host::refused_entry`). The errors of each tick are
/// returned with the host.
fn refused_wave(sync: bool) -> (Host, Vec<String>) {
    let mut game = FakeGame::new();
    game.refuse_places = sync;
    let mut host = start(Role::Host, &game);
    let mut errors = Vec::new();
    for tick in 0..=900 {
        let before = host.applied();
        errors.extend(host.pre_tick(tick, &mut game).into_iter().map(|e| e.to_string()));
        if !sync {
            for entry in host.log().since(before).to_vec() {
                if entry.seq <= host.applied() && matches!(entry.op, Command::PlaceSquad { .. }) {
                    host.refused_entry(entry.seq, &"the engine placed no actor for slot 0: class row 0 at zone 7");
                }
            }
        }
        host.post_tick(tick, &game).unwrap();
        errors.extend(host.take_stop_error().map(|e| e.to_string()));
    }
    (host, errors)
}

/// A wave whose every placement the engine refused has nothing to clear:
/// the director is stopped before its slot timeout counts the wave as
/// cleared, the stop names the refusal once, and no next wave is logged.
/// Before, the director cleared the wave 3 s later and went on to the next
/// ("wave complete" on a loop).
#[test]
fn a_wave_whose_every_placement_is_refused_is_stopped_not_cleared() {
    for sync in [true, false] {
        let (mut host, errors) = refused_wave(sync);
        let why = host.stopped().expect("the director is stopped");
        assert!(
            why.starts_with("round 1 wave 1 is refused, not cleared: the engine placed none of its "),
            "sync {sync}: {why}"
        );
        assert!(
            why.ends_with(
                "(the engine placed no actor for slot 0: class row 0 at zone 7); the director is stopped"
            ),
            "sync {sync}: {why}"
        );
        let stops: Vec<&String> = errors.iter().filter(|e| e.contains("is refused, not cleared")).collect();
        assert_eq!(stops.len(), 1, "named once: {errors:?}");
        assert!(stops[0].starts_with("at tick "), "{}", stops[0]);
        let waves: Vec<u8> = host
            .log()
            .since(0)
            .iter()
            .filter_map(|e| match e.op {
                Command::Wave { index } => Some(index),
                _ => None,
            })
            .collect();
        assert_eq!(waves, [0], "no wave after the refused one: sync {sync}");
        assert_eq!(host.take_ended(), None, "the match is not counted as won or lost");
    }
}

/// One placement the engine takes is enough: the wave is fought as before.
#[test]
fn a_wave_with_one_placement_taken_is_not_stopped() {
    let mut game = FakeGame::new();
    let mut host = start(Role::Host, &game);
    for tick in 0..=900 {
        run(&mut host, &mut game, tick);
        game.actors[0] = 1;
    }
    assert_eq!(host.stopped(), None);
}

// ---- Endless: sets and the bonus round ----

/// Endless (three-round sets, one after another) with a 60-tick bonus round
/// of one Elite at a time.
fn endless() -> Def {
    use ::firefight::{Bonus, Count, Drop, Side, SquadSpec};
    let mut def = builtin::definitions()
        .into_iter()
        .find(|def| def.set.endless)
        .expect("Endless is a built-in");
    // The test kit has one class; Endless's Marine allies are for the kit.
    for round in &mut def.rounds {
        round.allies.clear();
    }
    def.bonus = Some(Bonus {
        ticks: 60,
        squad: SquadSpec {
            class: "elite".into(),
            count: Count {
                per_player: 1,
                min: 1,
                max: 1,
            },
            drop: Drop::Edge,
            weapon_tier: 0,
            side: Side::Enemy,
            arrival: ::firefight::Arrival::Auto,
        },
        lives_by_points: vec![(100, 1)],
    });
    def
}

/// A match over [`FakeGame`] where every squad dies three ticks after it is
/// placed, and a respawn brings a dead player back.
struct Played {
    tick: u32,
    placed: BTreeMap<u8, (u32, u16)>,
    seen: usize,
}

impl Played {
    fn new() -> Self {
        Self {
            tick: 0,
            placed: BTreeMap::new(),
            seen: 0,
        }
    }

    fn step(&mut self, host: &mut Host, game: &mut FakeGame) {
        self.tick += 1;
        assert!(self.tick < 60_000, "the match never got there");
        let errors = host.pre_tick(self.tick, game);
        assert!(errors.is_empty(), "{errors:?}");
        let mut back = Vec::new();
        for call in &game.calls[self.seen..] {
            match call {
                Call::Squad(SquadOp::Place(p)) => {
                    self.placed.insert(p.slot, (self.tick, p.count));
                }
                Call::Squad(SquadOp::Erase { slot }) => {
                    self.placed.remove(slot);
                }
                Call::Respawn(player) => back.push(*player),
                _ => {}
            }
        }
        self.seen = game.calls.len();
        for player in game.players.iter_mut().filter(|p| back.contains(&p.slot)) {
            player.alive = true;
        }
        game.actors = [0; SQUAD_SLOTS];
        for (&slot, &(at, count)) in &self.placed {
            if self.tick - at < 3 {
                game.actors[usize::from(slot)] = count;
            }
        }
        host.post_tick(self.tick, &*game).unwrap();
    }

    fn until(&mut self, host: &mut Host, game: &mut FakeGame, done: impl Fn(&Host) -> bool) {
        while !done(host) {
            self.step(host, game);
        }
    }
}

fn ops(host: &Host) -> Vec<Command> {
    host.log().since(0).iter().map(|e| e.op.clone()).collect()
}

/// The skulls on after `calls`, in call order.
fn skulls_after(calls: &[Call]) -> BTreeSet<Skull> {
    let mut on = BTreeSet::new();
    for call in calls {
        if let Call::Skull(skull, set) = call {
            if *set {
                on.insert(*skull);
            } else {
                on.remove(skull);
            }
        }
    }
    on
}

#[test]
fn a_scripted_endless_match_makes_the_expected_engine_calls() {
    let mut game = FakeGame::new();
    let mut host = Host::start(Role::Host, endless(), settings(), &game).unwrap();
    let mut played = Played::new();
    // Through the first set, its bonus round and the first round of the
    // second set.
    played.until(&mut host, &mut game, |host| {
        let ops = ops(host);
        let set = ops.iter().position(|c| *c == Command::Set { index: 1 });
        set.is_some_and(|at| ops[at..].contains(&Command::Round { index: 0 }))
    });
    // Let its entries apply.
    for _ in 0..5 {
        played.step(&mut host, &mut game);
    }
    let ops = ops(&host);
    let at = |wanted: &Command| ops.iter().position(|c| c == wanted).unwrap();
    // The set, then its bonus round (on, then off), then the next set.
    let on = ops
        .iter()
        .position(|c| matches!(c, Command::Bonus { on: true, .. }))
        .expect("the bonus round starts");
    let off = ops
        .iter()
        .position(|c| matches!(c, Command::Bonus { on: false, .. }))
        .expect("the bonus round ends");
    assert!(at(&Command::Round { index: 2 }) < on && on < off);
    assert!(off < at(&Command::Set { index: 1 }));
    assert!(at(&Command::Set { index: 1 }) < ops.iter().rposition(|c| *c == Command::Round { index: 0 }).unwrap());
    // The game never ends, and no engine call was refused.
    assert!(!game.calls.iter().any(|c| matches!(c, Call::End(_))));
    // The second set's first round: the set's own skull and the round's,
    // none of the first set's round skulls.
    let on = skulls_after(&game.calls);
    assert!(on.contains(&Skull::Thunderstorm), "{on:?}");
    assert!(on.contains(&Skull::Ghost), "{on:?}");
    assert!(!on.contains(&Skull::Catch) && !on.contains(&Skull::BlackEye), "{on:?}");
    host.log().verify(host.head()).unwrap();
}

#[test]
fn no_respawn_is_issued_in_the_bonus_round() {
    let mut game = FakeGame::new();
    let mut host = Host::start(Role::Host, endless(), settings(), &game).unwrap();
    let mut played = Played::new();
    played.until(&mut host, &mut game, |host| host.bonus);
    // The only player dies in the bonus round: it ends the round, not the
    // game, and nothing is respawned or charged until it is over.
    game.players[0].alive = false;
    game.players[0].position = None;
    let (from, mut inside) = (game.calls.len(), game.calls.len());
    while host.bonus {
        inside = game.calls.len();
        played.step(&mut host, &mut game);
    }
    let during = &game.calls[from..inside];
    assert!(
        !during
            .iter()
            .any(|c| matches!(c, Call::Respawn(_) | Call::Lives(..) | Call::TeamLives(..))),
        "{during:?}"
    );
    assert!(!game.calls.iter().any(|c| matches!(c, Call::End(_))));
    // Then it grants and respawns: the player is back.
    for _ in 0..30 {
        played.step(&mut host, &mut game);
    }
    assert!(game.calls[inside..].contains(&Call::Respawn(0)));
    assert!(game.players[0].alive);
}

/// The host refuses a respawn the bonus round is on for: an operator's too.
#[test]
fn a_respawn_entry_in_the_bonus_round_is_refused_by_name() {
    let mut game = FakeGame::new();
    let mut host = Host::start(Role::Host, endless(), settings(), &game).unwrap();
    let mut played = Played::new();
    played.until(&mut host, &mut game, |host| host.bonus);
    host.append(played.tick + 1, Command::Respawn { player: 0 }).unwrap();
    let errors = host.pre_tick(played.tick + 1, &mut game);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].error.to_string().contains("bonus round"), "{errors:?}");
    assert!(!game.calls.contains(&Call::Respawn(0)));
}

/// A game lost in the second set's second round has played one set and
/// four rounds: the count runs across sets.
#[test]
fn an_endless_end_counts_sets_and_rounds() {
    let mut game = FakeGame::new();
    let mut host = Host::start(Role::Host, endless(), settings(), &game).unwrap();
    let mut played = Played::new();
    played.until(&mut host, &mut game, |host| {
        host.state().is_some_and(|s| s.set == 1 && s.round == 1)
    });
    let lost = [Command::Finish {
        outcome: Outcome::Lost,
    }];
    let ended = result(
        played.tick,
        "Endless".into(),
        progress(host.director().unwrap()),
        &lost,
    )
    .unwrap();
    assert_eq!((ended.sets_cleared, ended.rounds_cleared), (1, 4));
    // Lost in the first set's bonus round: that set is played through.
    let mut game = FakeGame::new();
    let mut host = Host::start(Role::Host, endless(), settings(), &game).unwrap();
    let mut played = Played::new();
    played.until(&mut host, &mut game, |host| {
        host.state()
            .is_some_and(|s| matches!(s.phase, Phase::Bonus { .. }))
    });
    let ended = result(
        played.tick,
        "Endless".into(),
        progress(host.director().unwrap()),
        &lost,
    )
    .unwrap();
    assert_eq!((ended.sets_cleared, ended.rounds_cleared), (1, 3));
}

const SHIP: u32 = 0x8123_0004;
const PELICAN: u32 = 0x8123_0005;

/// A game whose kit has the ship, at 4 seats, and a path for its zone.
fn ship_game() -> FakeGame {
    let mut game = FakeGame::new();
    game.ships[0] = Some(Ship { object: SHIP, capacity: 4 });
    game.ships[1] = Some(Ship { object: PELICAN, capacity: 3 });
    game.paths = vec![Some(ship_path())];
    game
}

fn ship_path() -> ShipPath {
    ShipPath {
        run_in: [-200.0, 0.0, 60.0],
        hovers: [Some([40.0, 0.0, 6.0]), Some([40.0, 0.0, 4.0])],
        run_out: [300.0, 0.0, 60.0],
    }
}

fn drop_command(count: u16) -> Command {
    Command::DropSquad {
        slot: 3,
        class: "elite".into(),
        count,
        zone: 0,
        weapon_tier: 2,
        arrival: Arrival::Auto,
        ship: ShipKind::Phantom,
    }
}

fn placement(count: u16) -> Placement {
    Placement {
        slot: 3,
        character: ELITE,
        count,
        zone: 0,
        team: KIT_TEAM,
        weapon_tier: 2,
    }
}

#[test]
fn a_drop_squad_flies_the_ship_in_and_places_the_squad_into_it() {
    let mut game = ship_game();
    let mut host = start(Role::Replica, &game);
    feed(&mut host, [(1, drop_command(6))]);
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(
        game.calls,
        [
            Call::Fly(Launch {
                ship: SHIP,
                path: vec![ship_path().run_in, ship_path().hovers[0].unwrap()],
                ticks: FLY_TICKS,
            }),
            Call::Drop(Boarding {
                placement: placement(6),
                ship: SHIP,
                capacity: 4,
            }),
        ]
    );
}

#[test]
fn a_pelican_drop_flies_the_pelican_at_its_own_hover_height() {
    let mut game = ship_game();
    let mut host = start(Role::Replica, &game);
    let Command::DropSquad { slot, class, count, zone, weapon_tier, .. } = drop_command(2) else {
        unreachable!()
    };
    let command = Command::DropSquad { slot, class, count, zone, weapon_tier, arrival: Arrival::Auto, ship: ShipKind::Pelican };
    feed(&mut host, [(1, command)]);
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(
        game.calls,
        [
            Call::Fly(Launch {
                ship: PELICAN,
                path: vec![ship_path().run_in, [40.0, 0.0, 4.0]],
                ticks: FLY_TICKS,
            }),
            Call::Drop(Boarding { placement: placement(2), ship: PELICAN, capacity: 3 }),
        ]
    );
    // A path with no Pelican hover key sends the squad on foot.
    let mut narrow = ship_game();
    narrow.paths = vec![Some(ShipPath { hovers: [Some([40.0, 0.0, 6.0]), None], ..ship_path() })];
    let mut host = start(Role::Replica, &narrow);
    let Command::DropSquad { slot, class, count, zone, weapon_tier, .. } = drop_command(2) else {
        unreachable!()
    };
    feed(&mut host, [(1, Command::DropSquad { slot, class, count, zone, weapon_tier, arrival: Arrival::Auto, ship: ShipKind::Pelican })]);
    assert!(host.pre_tick(1, &mut narrow).is_empty());
    assert_eq!(narrow.calls, [Call::Squad(SquadOp::Place(placement(2)))]);
}

#[test]
fn a_drop_squad_without_a_ship_or_a_path_places_on_foot() {
    let mut no_ship = ship_game();
    no_ship.ships[0] = None;
    let mut no_path = ship_game();
    no_path.paths = vec![None];
    for mut game in [no_ship, no_path] {
        let mut host = start(Role::Replica, &game);
        feed(&mut host, [(1, drop_command(6))]);
        assert!(host.pre_tick(1, &mut game).is_empty());
        assert_eq!(game.calls, [Call::Squad(SquadOp::Place(placement(6)))]);
    }
}

#[test]
fn the_ships_hold_unload_and_flight_out_are_the_directors_entries() {
    let mut game = ship_game();
    let mut host = start(Role::Replica, &game);
    feed(
        &mut host,
        [
            (1, Command::ShipHover { ship: ShipKind::Phantom, on: true }),
            (2, Command::ShipUnload { ship: ShipKind::Phantom }),
            (3, Command::ShipLeave { ship: ShipKind::Phantom, zone: 0 }),
            (4, Command::ShipHover { ship: ShipKind::Phantom, on: false }),
        ],
    );
    for tick in 1..=4 {
        assert!(host.pre_tick(tick, &mut game).is_empty());
    }
    assert_eq!(
        game.calls,
        [
            Call::Hover(SHIP, true),
            Call::Unload(SHIP),
            Call::Fly(Launch {
                ship: SHIP,
                path: vec![ship_path().hovers[0].unwrap(), ship_path().run_out],
                ticks: FLY_TICKS,
            }),
            Call::Hover(SHIP, false),
        ]
    );
    // A ship that is not in the world is a refused entry, not a panic.
    let mut gone = FakeGame::new();
    let mut host = start(Role::Replica, &gone);
    feed(&mut host, [(1, Command::ShipUnload { ship: ShipKind::Phantom })]);
    let errors = host.pre_tick(1, &mut gone);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].error.to_string().contains("not in the world"));
}

#[test]
fn the_director_is_told_of_the_ship_and_the_zones_it_can_fly_to() {
    let mut game = ship_game();
    game.kit = Some(Kit {
        zones: vec![[40.0, 0.0, 0.0], [90.0, 0.0, 0.0]],
        ..kit()
    });
    game.paths = vec![Some(ship_path()), None];
    let kit = game.kit().unwrap();
    let seen = observe(&kit, &BTreeMap::new(), 1, &SquadPods::new(0, 0), &game);
    assert_eq!(
        seen.ship,
        ShipSeen::phantom(true, vec![true, false]).with(ShipKind::Pelican, true, vec![true, false])
    );
    let seen = observe(&kit, &BTreeMap::new(), 1, &SquadPods::new(0, 0), &FakeGame::new());
    assert_eq!(
        seen.ship,
        ShipSeen::phantom(false, vec![false, false]).with(ShipKind::Pelican, false, vec![false, false])
    );
}

#[test]
fn an_ally_squad_takes_the_home_team_and_the_hold_order() {
    // Crossfire's players are on engine teams 0 and 1: an ally of Firefight
    // team 1 joins engine team 1 and holds base 1; team 0's holds base 0.
    let mut game = FakeGame::two_teams();
    let mut host = Host::start(Role::Replica, crossfire(), settings(), &game).unwrap();
    feed(
        &mut host,
        [
            (1, place(5, 2, 0, 0)),
            (1, Command::Ally { slot: 5, team: 1 }),
            (1, Command::Ally { slot: 6, team: 0 }),
            // No third base: refused.
            (2, Command::Ally { slot: 7, team: 2 }),
        ],
    );
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(
        game.calls[1..],
        [
            Call::Squad(SquadOp::Ally { slot: 5, team: 1, base: 1 }),
            Call::Squad(SquadOp::Ally { slot: 6, team: 0, base: 0 }),
        ]
    );
    let errors = host.pre_tick(2, &mut game);
    assert_eq!(errors.len(), 1);
    // Nobody on the team to serve: refused, not placed on a guess.
    game.players.retain(|p| p.team == 0);
    feed(&mut host, [(3, Command::Ally { slot: 5, team: 1 })]);
    assert_eq!(host.pre_tick(3, &mut game).len(), 1);
    assert_eq!(game.calls.len(), 3);
}

const POD: u32 = 0x777;

fn pod_game() -> FakeGame {
    let mut game = FakeGame::new();
    game.pods = vec![Some(POD)];
    game.falls = vec![Some(PodFall { top: [40.0, 0.0, 40.0], ground: [40.0, 0.0, 0.0] })];
    game
}

fn pod_command(zone: u8) -> Command {
    Command::DropSquad {
        slot: 3,
        class: "elite".into(),
        count: 2,
        zone,
        weapon_tier: 2,
        arrival: Arrival::Pod,
        ship: ShipKind::Phantom,
    }
}

#[test]
fn a_pod_squad_falls_and_is_placed_on_arrival() {
    let mut game = pod_game();
    let mut host = start(Role::Host, &game);
    host.append(1, pod_command(0)).unwrap();
    assert!(host.pre_tick(1, &mut game).is_empty());
    // The pod flies the fall; nothing is placed yet.
    assert_eq!(
        game.calls,
        [Call::Fly(Launch {
            ship: POD,
            path: vec![[40.0, 0.0, 40.0], [40.0, 0.0, 0.0]],
            ticks: FALL_TICKS,
        })]
    );
    let placed = |host: &Host| {
        host.log()
            .since(0)
            .iter()
            .filter(|e| matches!(&e.op, Command::PlaceSquad { slot: 3, .. }))
            .map(|e| e.tick)
            .collect::<Vec<_>>()
    };
    host.post_tick(1, &game).unwrap();
    assert!(placed(&host).is_empty());
    host.post_tick(1 + FALL_TICKS - 1, &game).unwrap();
    assert_eq!(placed(&host), [1 + FALL_TICKS]);
    // A film's replica, fed the log, makes the same calls.
    let mut replica_game = pod_game();
    let mut replica = start(Role::Replica, &replica_game);
    for e in host.log().since(0) {
        replica.receive(e.clone()).unwrap();
    }
    for tick in 1..=1 + FALL_TICKS {
        assert!(replica.pre_tick(tick, &mut replica_game).is_empty());
    }
    let squads: Vec<_> = replica_game
        .calls
        .iter()
        .filter(|c| matches!(c, Call::Fly(_) | Call::Squad(SquadOp::Place(Placement { slot: 3, .. }))))
        .collect();
    assert_eq!(squads.len(), 2);
    assert!(matches!(squads[1], Call::Squad(SquadOp::Place(p)) if p.slot == 3 && p.count == 2));
}

#[test]
fn no_free_pod_places_the_squad_on_foot() {
    let mut game = pod_game();
    game.pods = vec![None];
    let mut host = start(Role::Replica, &game);
    feed(&mut host, [(1, pod_command(0))]);
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(game.calls, [Call::Squad(SquadOp::Place(placement(2)))]);
}

#[test]
fn a_zone_without_a_fall_places_the_squad_on_foot() {
    let mut game = pod_game();
    game.falls = vec![None];
    let mut host = start(Role::Replica, &game);
    feed(&mut host, [(1, pod_command(0))]);
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(game.calls, [Call::Squad(SquadOp::Place(placement(2)))]);
}

#[test]
fn a_wreck_is_reparked_after_its_ticks() {
    use super::super::pods::WRECK_TICKS;
    let mut game = pod_game();
    let mut host = start(Role::Replica, &game);
    feed(&mut host, [(1, pod_command(0)), (1 + FALL_TICKS, place(3, 2, 0, 2))]);
    for tick in 1..=1 + FALL_TICKS + WRECK_TICKS {
        host.pre_tick(tick, &mut game);
        if tick < 1 + FALL_TICKS + WRECK_TICKS {
            assert!(!game.calls.contains(&Call::Park(POD)), "tick {tick}");
        }
    }
    assert_eq!(game.calls.iter().filter(|c| **c == Call::Park(POD)).count(), 1);
}

const FLOOD: u16 = 0x22;
const FPOD: u32 = 0x778;

/// An insertion pod and a Flood pod, and a Flood class beside the Elite.
fn two_pod_game(flood_pod: Option<u32>) -> FakeGame {
    let mut game = pod_game();
    game.pods = vec![Some(POD), flood_pod];
    game.flood_pods = 1;
    let kit = game.kit.as_mut().unwrap();
    kit.classes.insert("flood".to_string(), FLOOD);
    kit.pod_classes = ["flood".to_string()].into();
    kit.flood_classes = ["flood".to_string()].into();
    game
}

fn flood_drop(slot: u8) -> Command {
    Command::DropSquad {
        slot,
        class: "flood".into(),
        count: 2,
        zone: 0,
        weapon_tier: 2,
        arrival: Arrival::Pod,
        ship: ShipKind::Phantom,
    }
}

fn flown(game: &FakeGame) -> Vec<u32> {
    game.calls
        .iter()
        .filter_map(|c| if let Call::Fly(l) = c { Some(l.ship) } else { None })
        .collect()
}

#[test]
fn a_flood_squad_falls_in_a_flood_pod_and_the_others_in_the_insertion_pod() {
    let mut game = two_pod_game(Some(FPOD));
    let mut host = start(Role::Replica, &game);
    feed(&mut host, [(1, flood_drop(1)), (1, pod_command(0))]);
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert_eq!(flown(&game), [FPOD, POD]);
}

#[test]
fn a_flood_squad_never_takes_the_insertion_pod_and_goes_on_foot_without_a_flood_pod() {
    let mut game = two_pod_game(None);
    let mut host = start(Role::Replica, &game);
    feed(&mut host, [(1, flood_drop(1))]);
    assert!(host.pre_tick(1, &mut game).is_empty());
    assert!(flown(&game).is_empty());
    assert!(matches!(game.calls[..], [Call::Squad(SquadOp::Place(p))] if p.slot == 1));
    assert_eq!(host.squad_pod_counts().fallbacks.get(&Fallback::NoFreePod), Some(&1));
}

#[test]
fn the_director_sees_each_faction_pool_apart() {
    let game = two_pod_game(Some(FPOD));
    let host = start(Role::Host, &game);
    let seen = observe(host.kit(), &BTreeMap::new(), 1, &SquadPods::new(1, 1), &game);
    assert_eq!((seen.pods.free, seen.pods.free_flood), (1, 1));
    assert!(seen.pods.flood_classes.contains("flood"));
}
