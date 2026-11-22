//! The design page's example scripts compile, verify and run on the fake
//! game; refusals, budget, generator and grants behave as specified.

use rules::fake::{FakeGame, FakePlayer};
use rules::verify::{Fault, unhostable};
use rules::policy::Context;
use rules::vm::{Arg, ErrorKind, Proposal, ProposalRefusal, TICK_BUDGET};
use rules::{Fired, Input, Ir, Refusal, RulesExt, Vm, build, builtin, compile, verify};

const INFECTION: &str = r#"
rules "Infection" 1
carrier slayer
teams 2
requires player.team, player.weapons

option first_zombies: int 1..4 = 1

player var infected: bool = false

fn infect(p: player) {
    p.infected = true
    set_team(p, team 1)
    set_loadout(p, "objects/weapons/melee/energy_blade/energy_blade", none)
}

on round_start {
    for each players p { p.infected = false; set_team(p, team 0) }
    for each pick(players, first_zombies) p { infect(p) }
}

on kill(victim, killer) {
    if killer != none and killer.infected and not victim.infected {
        infect(victim)
        score(killer, 1)
    }
}

on tick {
    if count(players where not infected) == 0 { end_round(team 1) }
    else if round_time_left() == 0 { end_round(team 0) }
}
"#;

const HOLDOUT: &str = r#"
rules "Holdout" 1
carrier slayer
teams 1
requires ai.squads

label ff_drop: objects

table waves: (class: Class, per_player: int, min: int, max: int, gap: int) = [
    ("grunt", 2, 2, 6, 3),
    ("elite", 1, 1, 3, 5),
]
table points: (class: Class, score: int) = [("grunt", 1), ("elite", 3)]
global var wave: int = 0
global var lives: int = 2
global var next: timer = 2

on timer(next) {
    let w = waves[wave]
    let s = squad(wave % 16)
    place_squad(s, w.class, clamp(count(players) * w.per_player, w.min, w.max), ff_drop)
    hunt(s)
}
on squad_dead(s) {
    wave = wave + 1
    if wave == len(waves) { end_round(team 0) } else { next = waves[wave - 1].gap }
}
on actor_kill(killer, class) { if killer != none { score(killer, points[class].score) } }
on death(p) {
    if lives > 0 { lives = lives - 1; respawn(p) }
    else if count(players where alive) == 0 { end_round(none) }
}
"#;

const VIP: &str = r#"
rules "VIP" 1
carrier slayer
requires forge.move, forge.place, forge.delete
palette "vip" = ["objects/blocks/wall", "objects/blocks/ramp"]
enum Role { none, vip }
player var role: Role = Role.none

fn crown(p: player) {
    p.role = Role.vip
    grant(p, forge.place, budget 2, palette "vip")
    grant(p, forge.move, budget 40)
}
on join(p) { crown(p) }
on spawn(p) { if p.team == team 1 { grant(p, forge.delete, budget 3, palette "vip") } }
on press(p, o) { if granted(p, forge.place) { revoke(p, forge.move) } }
"#;

fn player(p: u8) -> Arg {
    Arg::Player(Some(p))
}

fn teams_of(g: &mut FakeGame, of: &[(u8, u8)]) {
    for &(slot, team) in of {
        g.players.get_mut(&slot).unwrap().team = team;
    }
}

// ---- Slayer ----

#[test]
fn slayer_scores_kills_suicides_and_betrayals() {
    let (ir, report) = build(builtin::SLAYER).unwrap();
    assert!(report.fits());
    let mut g = FakeGame::with_players(4);
    teams_of(&mut g, &[(2, 1), (3, 1)]);
    g.variant.insert("team_play".into(), 1);
    let defaults = ir.default_options();
    let mut vm = Vm::new(ir.clone(), defaults, 7).unwrap();
    vm.begin_tick(1);
    // A betrayal, a kill, a suicide by the world and by the player.
    for input in [
        Input::Kill { victim: 1, killer: Some(0) },
        Input::Kill { victim: 2, killer: Some(0) },
        Input::Kill { victim: 3, killer: None },
        Input::Kill { victim: 3, killer: Some(3) },
    ] {
        assert_eq!(vm.fire(&mut g, input), Fired::Ran);
    }
    assert_eq!(
        g.calls_to("score"),
        vec![
            &vec![player(0), Arg::Int(-1)],
            &vec![player(0), Arg::Int(1)],
            &vec![player(3), Arg::Int(-1)],
            &vec![player(3), Arg::Int(-1)],
        ]
    );
    assert_eq!(g.players[&0].score, 0);

    // Kill points 3 (raw 2 in bits 0..3), suicides free (bit 3 clear).
    let mut g = FakeGame::with_players(4);
    let mut vm = Vm::new(ir, 2, 7).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::Kill { victim: 2, killer: Some(0) });
    vm.fire(&mut g, Input::Kill { victim: 1, killer: None });
    // No team play: a same-team kill scores.
    vm.fire(&mut g, Input::Kill { victim: 3, killer: Some(1) });
    assert_eq!(
        g.calls_to("score"),
        vec![&vec![player(0), Arg::Int(3)], &vec![player(1), Arg::Int(3)]]
    );
    assert_eq!(vm.fire(&mut g, Input::Spawn(0)), Fired::NoHandler);
}

// ---- Infection ----

fn infection(players: u8, options: u32, seed: u64) -> (Vm, FakeGame) {
    let (ir, _) = build(INFECTION).unwrap();
    let mut g = FakeGame::with_players(players);
    g.round_time_left = 100;
    let mut vm = Vm::new(ir, options, seed).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(&mut g, Input::Init), Fired::NoHandler);
    assert_eq!(vm.fire(&mut g, Input::RoundStart), Fired::Ran);
    (vm, g)
}

fn zombies(vm: &Vm, players: u8) -> Vec<u8> {
    (0..players)
        .filter(|&p| vm.player_var(p, "infected") == Some(1))
        .collect()
}

#[test]
fn infection_switches_teams_and_ends_when_all_are_infected() {
    let (ir, _) = build(INFECTION).unwrap();
    // set_team has no traced entry, so a host refuses the script by name.
    assert_eq!(unhostable(&ir).map(|e| e.name), Some("set_team"));

    let (mut vm, mut g) = infection(4, 0, 99);
    let set_team = g.calls_to("set_team");
    assert_eq!(set_team.len(), 5);
    for (slot, call) in set_team.iter().take(4).enumerate() {
        assert_eq!(**call, vec![player(slot as u8), Arg::Team(Some(0))]);
    }
    let z = zombies(&vm, 4);
    assert_eq!(z.len(), 1);
    assert_eq!(*set_team[4], vec![player(z[0]), Arg::Team(Some(1))]);
    assert_eq!(
        g.calls_to("set_loadout"),
        vec![&vec![
            player(z[0]),
            Arg::Tag(Some("objects/weapons/melee/energy_blade/energy_blade".into())),
            Arg::Tag(None),
        ]]
    );
    assert_eq!(g.players[&z[0]].team, 1);

    // Humans remain: the tick ends nothing.
    vm.begin_tick(2);
    vm.update(&mut g);
    assert!(g.calls_to("end_round").is_empty());

    // The zombie kills every human; each kill infects and scores.
    let humans: Vec<u8> = (0..4).filter(|p| *p != z[0]).collect();
    for &h in &humans {
        vm.fire(&mut g, Input::Kill { victim: h, killer: Some(z[0]) });
    }
    // A human killing a zombie does nothing.
    vm.fire(&mut g, Input::Kill { victim: z[0], killer: Some(humans[0]) });
    assert_eq!(g.calls_to("score").len(), 3);
    assert_eq!(g.players[&z[0]].score, 3);
    assert_eq!(zombies(&vm, 4).len(), 4);

    vm.begin_tick(3);
    vm.update(&mut g);
    assert_eq!(g.calls_to("end_round"), vec![&vec![Arg::Team(Some(1))]]);
}

#[test]
fn infection_time_out_goes_to_the_humans() {
    let (mut vm, mut g) = infection(3, 0, 1);
    g.round_time_left = 0;
    vm.begin_tick(2);
    vm.update(&mut g);
    assert_eq!(g.calls_to("end_round"), vec![&vec![Arg::Team(Some(0))]]);
}

#[test]
fn pick_draws_the_same_players_for_the_same_seed() {
    // Four first zombies (raw 3) out of sixteen.
    let a = zombies(&infection(16, 3, 1234).0, 16);
    let b = zombies(&infection(16, 3, 1234).0, 16);
    assert_eq!(a.len(), 4);
    assert_eq!(a, b);
    let differs = (0..8u64).any(|s| zombies(&infection(16, 3, s).0, 16) != a);
    assert!(differs, "the seed changes the draw");
}

#[test]
fn state_round_trips_and_resumes_the_same_run() {
    let (vm, _) = infection(6, 1, 5);
    let bytes = vm.ext().encode();
    let ext = RulesExt::decode(&bytes).unwrap();
    assert_eq!(&ext, vm.ext());
    let mut resumed = Vm::resume(vm.ir().clone(), 1, ext).unwrap();
    let mut original = vm;
    let mut g1 = FakeGame::with_players(6);
    let mut g2 = FakeGame::with_players(6);
    for vm_g in [(&mut original, &mut g1), (&mut resumed, &mut g2)] {
        let (vm, g) = vm_g;
        vm.begin_tick(2);
        vm.fire(g, Input::RoundStart);
    }
    assert_eq!(g1.calls, g2.calls);
    assert_eq!(original.ext().encode(), resumed.ext().encode());
}

// ---- Holdout ----

#[test]
fn holdout_runs_waves_from_its_tables() {
    let (ir, report) = build(HOLDOUT).unwrap();
    assert!(report.fits());
    assert_eq!(unhostable(&ir), None);
    let mut g = FakeGame::with_players(3);
    let mut vm = Vm::new(ir, 0, 3).unwrap();

    // The timer counts 2, 1, then fires: squad calls are queued.
    for tick in 1..=2 {
        vm.begin_tick(tick);
        vm.update(&mut g);
    }
    assert_eq!(vm.ext().queue.len(), 2);
    assert!(g.calls.is_empty());
    vm.flush_queue(&mut g);
    assert_eq!(
        g.calls,
        vec![
            (
                "place_squad",
                vec![
                    Arg::Squad(0),
                    Arg::Class(Some("grunt".into())),
                    Arg::Int(6),
                    Arg::Label("ff_drop".into()),
                ]
            ),
            ("hunt", vec![Arg::Squad(0)]),
        ]
    );

    // Scores by class; a class the table lacks reads the zero row.
    vm.fire(&mut g, Input::ActorKill { killer: Some(1), class: "elite".into() });
    vm.fire(&mut g, Input::ActorKill { killer: Some(0), class: "brute".into() });
    assert_eq!(
        g.calls_to("score"),
        vec![&vec![player(1), Arg::Int(3)], &vec![player(0), Arg::Int(0)]]
    );

    // The squad dies: the next wave waits out the first wave's gap.
    vm.fire(&mut g, Input::SquadDead(0));
    assert_eq!(vm.global("wave"), Some(1));
    assert_eq!(vm.global("next"), Some(3));
    g.calls.clear();
    for tick in 3..=5 {
        vm.begin_tick(tick);
        vm.update(&mut g);
    }
    vm.flush_queue(&mut g);
    assert_eq!(
        g.calls_to("place_squad"),
        vec![&vec![
            Arg::Squad(1),
            Arg::Class(Some("elite".into())),
            Arg::Int(3),
            Arg::Label("ff_drop".into()),
        ]]
    );

    // Two lives, then the last death with nobody alive ends the round.
    vm.fire(&mut g, Input::Death(2));
    vm.fire(&mut g, Input::Death(2));
    assert_eq!(g.calls_to("respawn").len(), 2);
    assert_eq!(vm.global("lives"), Some(0));
    for p in g.players.values_mut() {
        p.alive = false;
    }
    vm.fire(&mut g, Input::Death(1));
    assert_eq!(g.calls_to("end_round"), vec![&vec![Arg::Team(None)]]);

    // Clearing the last wave wins.
    vm.fire(&mut g, Input::SquadDead(1));
    assert_eq!(g.calls_to("end_round").len(), 2);
    assert_eq!(*g.calls_to("end_round")[1], vec![Arg::Team(Some(0))]);
}

// ---- Budget ----

const SPIN: &str = r#"
rules "Spin" 1
carrier slayer
global var n: int = 0
on tick {
    for each players a { for each players b { for each players c { for each players d { n = n + 1 } } } }
}
"#;

fn spin() -> (Vm, FakeGame) {
    let (ir, report) = build(SPIN).unwrap();
    assert!(!report.fits(), "the estimate names the overflow");
    let mut g = FakeGame::with_players(16);
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    vm.begin_tick(10);
    assert_eq!(vm.update(&mut g), Fired::Stopped);
    (vm, g)
}

#[test]
fn a_budget_overflow_is_a_script_error_at_the_same_op_every_run() {
    let (vm, g) = spin();
    let error = g.script_error.expect("the game is told");
    assert_eq!(error.kind, ErrorKind::Budget);
    assert_eq!(error.tick, 10);
    assert_eq!(vm.ext().ops, TICK_BUDGET);
    assert_eq!(vm.ext().stopped, Some(error));

    let (vm2, g2) = spin();
    assert_eq!(g2.script_error, Some(error));
    assert_eq!(vm2.global("n"), vm.global("n"));
    assert_eq!(vm2.ext().encode(), vm.ext().encode());
}

#[test]
fn a_stopped_vm_runs_nothing() {
    let (mut vm, mut g) = spin();
    vm.begin_tick(11);
    assert_eq!(vm.update(&mut g), Fired::Stopped);
    assert_eq!(vm.fire(&mut g, Input::RoundStart), Fired::Stopped);
    assert_eq!(vm.ext().ops, 0);
}

// ---- Grants ----

#[test]
fn grants_follow_the_declaration_palette_and_budget() {
    let (ir, _) = build(VIP).unwrap();
    let mut g = FakeGame::with_players(3);
    teams_of(&mut g, &[(1, 1)]);
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    vm.begin_tick(1);
    vm.fire(&mut g, Input::Join(0));
    assert_eq!(vm.player_var(0, "role"), Some(1));

    let wall = Proposal {
        player: 0,
        cap: "forge.place",
        tag: "objects/blocks/wall",
        match_made: false,
        protected: false,
    };
    assert_eq!(vm.check_proposal(Context::Custom, &wall), Ok(()));
    assert_eq!(vm.check_proposal(Context::Ranked, &wall), Err(ProposalRefusal::Ranked));
    assert_eq!(vm.check_proposal(Context::Theater, &wall), Err(ProposalRefusal::Theater));
    assert_eq!(vm.check_proposal(Context::Hopper { deny: &[] }, &wall), Ok(()));
    let tank = Proposal {
        tag: "objects/vehicles/tank",
        ..wall
    };
    assert_eq!(
        vm.check_proposal(Context::Custom, &tank),
        Err(ProposalRefusal::OutsidePalette("objects/vehicles/tank".into()))
    );
    assert_eq!(
        vm.check_proposal(Context::Custom, &Proposal { protected: true, ..wall }),
        Err(ProposalRefusal::Protected)
    );
    assert_eq!(
        vm.check_proposal(Context::Custom, &Proposal { cap: "forge.scale", ..wall }),
        Err(ProposalRefusal::Undeclared("forge.scale".into()))
    );
    assert_eq!(
        vm.check_proposal(Context::Custom, &Proposal { player: 2, ..wall }),
        Err(ProposalRefusal::NoGrant("forge.place".into()))
    );

    // Moving with no palette reaches only what the match made.
    let mv = Proposal {
        cap: "forge.move",
        tag: "objects/scenery/rock",
        ..wall
    };
    assert_eq!(vm.check_proposal(Context::Custom, &mv), Err(ProposalRefusal::NotMatchMade));
    assert_eq!(vm.check_proposal(Context::Custom, &Proposal { match_made: true, ..mv }), Ok(()));

    // Each accepted edit costs a use; one past the budget is refused.
    assert_eq!(vm.spend(0, "forge.place"), Ok(1));
    assert_eq!(vm.spend(0, "forge.place"), Ok(0));
    assert_eq!(
        vm.check_proposal(Context::Custom, &wall),
        Err(ProposalRefusal::NoUses("forge.place".into()))
    );
    assert_eq!(vm.spend(0, "forge.place"), Err(ProposalRefusal::NoUses("forge.place".into())));

    // granted() sees the spent grant: no revoke for player 0.
    vm.fire(&mut g, Input::Press(0, 7));
    assert!(vm.ext().granted(0, vm.cap_index("forge.move").unwrap()));
    // A fresh VIP presses while placing is live: the move grant is revoked.
    vm.fire(&mut g, Input::Join(2));
    vm.fire(&mut g, Input::Press(2, 7));
    assert_eq!(
        vm.check_proposal(Context::Custom, &Proposal { player: 2, match_made: true, ..mv }),
        Err(ProposalRefusal::NoGrant("forge.move".into()))
    );

    // A spawn on team 1 grants delete; quit and the round's end clear.
    vm.fire(&mut g, Input::Spawn(1));
    let del = vm.cap_index("forge.delete").unwrap();
    assert!(vm.ext().granted(1, del));
    vm.fire(&mut g, Input::Quit(1));
    assert!(!vm.ext().granted(1, del));
    vm.fire(&mut g, Input::RoundEnd);
    assert!(vm.ext().grants.is_empty());
}

// ---- Refusals ----

fn compile_error(src: &str) -> String {
    compile(src).unwrap_err().to_string()
}

#[test]
fn a_call_or_grant_outside_requires_is_refused() {
    let no_team = INFECTION.replace("requires player.team, player.weapons", "requires player.weapons");
    assert!(compile_error(&no_team).contains("set_team needs capability player.team"));

    let no_place = VIP.replace("requires forge.move, forge.place, forge.delete", "requires forge.move, forge.delete");
    assert!(compile_error(&no_place).contains("capability forge.place is not in requires"));

    let not_grantable = r#"
rules "X" 1
carrier slayer
requires hud.message
on join(p) { grant(p, hud.message, budget 1) }
"#;
    assert!(compile_error(not_grantable).contains("hud.message cannot be granted"));

    let over = VIP.replace("budget 40", "budget 256");
    assert!(compile_error(&over).contains("at most 255 uses"));

    // The verifier refuses the same blobs edited past the compiler.
    let (mut ir, _) = build(INFECTION).unwrap();
    ir.header.requires.retain(|c| c != "player.team");
    assert!(matches!(
        verify(&ir),
        Err(Refusal::Op {
            fault: Fault::Undeclared { cap: "player.team", .. },
            ..
        })
    ));
    let (mut ir, _) = build(VIP).unwrap();
    ir.header.requires[1] = "hud.message".into();
    assert!(matches!(
        verify(&ir),
        Err(Refusal::Op {
            fault: Fault::NotGrantable(_),
            ..
        })
    ));
}

#[test]
fn a_blob_naming_an_unknown_capability_is_refused_by_that_name() {
    let (mut ir, _) = build(builtin::SLAYER).unwrap();
    ir.header.requires.push("net.socket".into());
    let bytes = ir.encode();
    assert_eq!(
        rules::load(&bytes).unwrap_err(),
        Refusal::UnknownCapability("net.socket".into())
    );
    assert!(compile_error("rules \"X\" 1\ncarrier slayer\nrequires net.socket\n").contains("unknown capability net.socket"));
}

#[test]
fn compile_errors_carry_line_and_column() {
    let e = compile("rules \"X\" 1\ncarrier slayer\non tick {\n    score(nobody, 1)\n}\n").unwrap_err();
    assert_eq!((e.line, e.col), (4, 11));
    assert!(e.message.contains("unknown name nobody"));

    let e = compile("rules \"X\" 1\ncarrier slayer\nfn a() { a() }\non tick { a() }\n").unwrap_err();
    assert!(e.message.contains("recursion"));

    let e = compile("rules \"X\" 1\ncarrier slayer\non tick { let x = 1 + true }\n").unwrap_err();
    assert!(e.message.contains("expected bool, found int"), "{e}");

    let e = compile("rules \"X\" 1\non tick { }\n").unwrap_err();
    assert!(e.message.contains("carrier"));

    let e = compile("rules \"X\" 1\ncarrier slayer\nplayer var t: timer\n").unwrap_err();
    assert!(e.message.contains("timer is a global"));

    let e = compile("rules \"X\" 1\ncarrier slayer\noption a: int 0..65535 = 0\noption b: int 0..65535 = 0\noption c: bool = false\n")
        .unwrap_err();
    assert!(e.message.contains("33 bits"), "{e}");
}

// ---- Bounce events ----

const BOUNCES: &str = r#"
rules "Bounces" 1
carrier slayer
player var last_chain: int = 0
player var monsters: int = 0
player var falls: int = 0
object var hits: int = 0
on bounce(p, spot, chain, monster) {
    p.last_chain = chain
    spot.hits = spot.hits + 1
    if monster { p.monsters = p.monsters + 1 }
}
on bounced(p) { p.falls = p.falls + 1 }
"#;

/// `on bounce` binds the player, the strip, the chain's count and the
/// Monster flag in that order; `on bounced` binds the player.
#[test]
fn on_bounce_hears_its_chain_and_monster_and_on_bounced_its_player() {
    let (ir, report) = build(BOUNCES).unwrap();
    assert!(report.fits());
    verify(&ir).unwrap();
    let mut g = FakeGame::with_players(2);
    let mut vm = Vm::new(ir, 0, 1).unwrap();
    vm.begin_tick(1);
    let bounce = |chain, monster| Input::Bounce {
        player: 1,
        spot: 77,
        chain,
        monster,
    };
    assert_eq!(vm.fire(&mut g, bounce(1, false)), Fired::Ran);
    assert_eq!(vm.player_var(1, "last_chain"), Some(1));
    assert_eq!(vm.fire(&mut g, bounce(2, true)), Fired::Ran);
    assert_eq!(vm.player_var(1, "last_chain"), Some(2));
    assert_eq!(vm.player_var(1, "monsters"), Some(1));
    assert_eq!(vm.player_var(1, "falls"), Some(0));
    assert_eq!(vm.fire(&mut g, Input::Bounced(1)), Fired::Ran);
    assert_eq!(vm.player_var(1, "falls"), Some(1));
    assert_eq!(vm.player_var(0, "falls"), Some(0));
    // A script without `on bounced` does not hear one.
    let (ir, _) = build(builtin::SLAYER).unwrap();
    let mut vm = Vm::new(ir, 0, 1).unwrap();
    vm.begin_tick(1);
    assert_eq!(vm.fire(&mut g, Input::Bounced(1)), Fired::NoHandler);
}

/// A handler may leave out trailing arguments (`on bounce(p, spot)`); one
/// that names more than its event gives is refused with what the event
/// gives.
#[test]
fn a_bounce_handler_with_too_many_parameters_is_refused() {
    compile("rules \"X\" 1\ncarrier slayer\non bounce(p, spot) { score(p, 1) }\n").unwrap();
    let e = compile_error("rules \"X\" 1\ncarrier slayer\non bounce(p, spot, chain, monster, extra) { }\n");
    assert!(
        e.contains("on bounce gives 4 arguments (player, zone, int, bool); this handler names 5."),
        "{e}"
    );
    let e = compile_error("rules \"X\" 1\ncarrier slayer\non bounced(p, spot) { }\n");
    assert!(e.contains("on bounced gives 1 argument (player); this handler names 2."), "{e}");
    // The verifier refuses a blob whose bounce handler lacks the count.
    let (mut ir, _) = build(BOUNCES).unwrap();
    let hd = ir
        .handlers
        .iter_mut()
        .find(|h| h.event == rules::ir::Event::Bounce)
        .unwrap();
    hd.regs[2] = rules::ty::Ty::Object;
    assert!(verify(&ir).is_err());
}

// ---- Codec and identity ----

#[test]
fn compiled_scripts_round_trip_and_hash_stably() {
    for src in [builtin::SLAYER, INFECTION, HOLDOUT, VIP, SPIN] {
        let a = compile(src).unwrap();
        let b = compile(src).unwrap();
        assert_eq!(a.encode(), b.encode());
        assert_eq!(a.id(), b.id());
        let bytes = a.encode();
        assert_eq!(Ir::decode(&bytes).unwrap(), a);
        assert!(bytes.len() <= rules::ir::MAX_IR_BYTES);
    }
    // Changing a constant changes the identity.
    let other = compile(&HOLDOUT.replace("(\"elite\", 3)", "(\"elite\", 4)")).unwrap();
    assert_ne!(other.id(), compile(HOLDOUT).unwrap().id());
}

#[test]
fn division_by_zero_gives_zero_and_a_warning() {
    let src = r#"
rules "Div" 1
carrier slayer
global var zero: int = 0
global var out: int = 9
on tick { out = 7 / zero + 7 % zero }
"#;
    let (ir, _) = build(src).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::default();
    vm.begin_tick(1);
    vm.update(&mut g);
    assert_eq!(vm.global("out"), Some(0));
    assert_eq!(vm.take_warnings().len(), 2);
}

#[test]
fn zone_events_past_64_a_tick_carry_over() {
    let src = r#"
rules "Zones" 1
carrier slayer
global var n: int = 0
on enter(zone, o) { n = n + 1 }
"#;
    let (ir, _) = build(src).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::default();
    vm.begin_tick(1);
    let mut carried = 0;
    for i in 0..70 {
        if vm.fire(&mut g, Input::Enter { zone: 1, object: i }) == Fired::Carried {
            carried += 1;
        }
    }
    assert_eq!(carried, 6);
    assert_eq!(vm.global("n"), Some(64));
    vm.begin_tick(2);
    assert_eq!(vm.fire(&mut g, Input::Enter { zone: 1, object: 0 }), Fired::Ran);
}

#[test]
fn for_each_visits_players_by_slot_and_objects_by_placement() {
    let src = r#"
rules "Order" 1
carrier slayer
label flag: objects
global var n: int = 0
on tick {
    for each players p { score(p, 1) }
    for each flag o { if is_objective(o) { n = n + 1 } }
    if let p = first(players where alive) { score(p, 5) }
}
"#;
    let (ir, _) = build(src).unwrap();
    let mut vm = Vm::new(ir, 0, 0).unwrap();
    let mut g = FakeGame::with_players(0);
    for slot in [5u8, 2, 9] {
        g.players.insert(slot, FakePlayer { alive: slot != 2, ..FakePlayer::default() });
    }
    vm.begin_tick(1);
    vm.update(&mut g);
    let order: Vec<_> = g.calls_to("score").into_iter().cloned().collect();
    assert_eq!(
        order,
        vec![
            vec![player(2), Arg::Int(1)],
            vec![player(5), Arg::Int(1)],
            vec![player(9), Arg::Int(1)],
            vec![player(5), Arg::Int(5)],
        ]
    );
}

#[test]
fn a_list_line_files_the_script_under_a_named_row() {
    use rules::ir::List;
    let plain = compile("rules \"X\" 1\ncarrier slayer\n").unwrap();
    assert_eq!(plain.header.list, List::GameType);
    let listed = compile("rules \"X\" 1\ncarrier slayer\nlist tweakers\n").unwrap();
    assert_eq!(listed.header.list, List::Tweakers);
    assert_ne!(plain.id(), listed.id());
    assert!(
        compile_error("rules \"X\" 1\ncarrier slayer\nlist slayer\n")
            .contains("unknown list slayer: a list is tweakers")
    );
    assert!(
        compile_error("rules \"X\" 1\ncarrier slayer\nlist tweakers\nlist tweakers\n")
            .contains("the list is named twice")
    );
}

/// What one op costs, for choosing [`TICK_BUDGET`]: run with
/// `cargo test --release -p rules --test scripts op_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn op_cost() {
    let mut best = f64::MAX;
    for _ in 0..20 {
        let start = std::time::Instant::now();
        let (vm, _) = spin();
        let ns = start.elapsed().as_nanos() as f64 / f64::from(vm.ext().ops);
        best = best.min(ns);
    }
    println!("{best:.2} ns per op; a full {TICK_BUDGET}-op tick is {:.3} ms", best * f64::from(TICK_BUDGET) / 1e6);
}
