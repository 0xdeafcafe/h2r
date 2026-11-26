use rules::caps::id;
use rules::vm::{Arg, Call, Game, Input, Placed, ScriptError, Vm};

const DIRECTOR: &str = include_str!("../../../../content/modes/rules/firefight.rules");

// A synthetic wave set with one authored drop and one scheduled skull.
const WAVES: &str = r#"
teams 1
const team_count = 1
const shared_lives = 1
const start_pool = 2
const second_pool = 0
const bonus_lives = 0
const player_kill_points = 0
const assist_share = 500
const largest_roster = 1
const waves_per_round = 5
const endless = 0
const set_scale = 0
const set_skull_count = 0
const zero_holds = 0
const bonus_on = 0
const bonus_ticks = 0
const set_rounds = 1
table squads: (kind: Class, per_player: int, minimum: int, maximum: int, preferred: int, tier: int, zone_role: int) = [
    ("fixture", 0, 1, 1, 1, 2, 0),
]
table waves: (first: int, count: int, gap: int) = [
    (0, 1, 1), (0, 1, 1), (0, 1, 1), (0, 1, 1), (0, 1, 1),
]
table round_data: (s0: int, s1: int, s2: int, s3: int, s4: int, s5: int, s6: int, s7: int, s8: int) = [
    (0, -1, -1, -1, -1, -1, -1, -1, -1),
]
table actor_caps: (actors: int) = [(1)]
table class_points: (kind: Class, value: int) = [("fixture", 7)]
table difficulties: (multiplier: int) = [(1000), (1500), (1000), (1000)]
table set_skulls: (which: int) = [(-1)]
table bonus_skulls: (s0: int, s1: int, s2: int, s3: int, s4: int, s5: int, s6: int, s7: int, s8: int) = [
    (-1, -1, -1, -1, -1, -1, -1, -1, -1),
]
table bonus_rows: (need0: int, extra0: int, need1: int, extra1: int, need2: int, extra2: int, need3: int, extra3: int, need4: int, extra4: int, need5: int, extra5: int) = [
    (-1, 0, -1, 0, -1, 0, -1, 0, -1, 0, -1, 0),
]
table skull_data: (id: int, bit: int, multiplier: int, scheduled: int) = [
    (0, 1, 1500, 1), (1, 2, 1000, 0), (2, 4, 1000, 0), (3, 8, 1000, 0), (4, 16, 1000, 0),
    (5, 32, 1000, 0), (6, 64, 1000, 0), (7, 128, 1000, 0), (8, 256, 1000, 0),
]
"#;

#[derive(Default)]
struct World {
    observed_squad: i32,
    log: Vec<Call>,
}

impl Game for World {
    fn players(&self) -> Vec<u8> {
        vec![0]
    }

    fn teams(&self) -> Vec<u8> {
        vec![0]
    }

    fn labelled(&self, label: &str) -> Vec<Placed> {
        let (base, len) = match label {
            "ff_slots" => (256, 16),
            "ff_drop" => (0, 2),
            _ => return Vec::new(),
        };
        (0..len)
            .map(|placement| Placed { placement, object: base + placement })
            .collect()
    }

    fn variant(&self, _: &str) -> i32 {
        0
    }

    fn read(&mut self, call: &Call) -> i32 {
        match (call.entry.id, call.args.as_slice()) {
            (id::ALIVE, _) => 1,
            (id::ZONE_HAS_ROLE, _) => 1,
            (id::SQUAD_ALIVE, [Arg::Squad(0)]) => self.observed_squad,
            _ => 0,
        }
    }

    fn act(&mut self, call: &Call) {
        self.log.push(call.clone());
    }

    fn script_error(&mut self, error: &ScriptError) {
        panic!("Firefight script stopped: {error:?}");
    }
}

#[test]
fn director_places_the_wave_and_scores_with_the_scheduled_skull() {
    let (ir, report) = rules::ship_files("firefight.rules", DIRECTOR, &|name| {
        (name == "firefight-waves.rules").then(|| WAVES.to_owned())
    })
    .unwrap();
    assert!(report.fits());
    let options = ir.default_options();
    let mut vm = Vm::new(ir, options, 1).unwrap();
    let mut world = World::default();

    vm.begin_tick(0);
    vm.fire(&mut world, Input::Init);
    vm.fire(&mut world, Input::Join(0));
    vm.update(&mut world);
    vm.flush_queue(&mut world);
    assert_eq!(world.log[0].entry.id, id::FF_LIVES);
    assert_eq!(world.log[0].args, [Arg::Player(None), Arg::Int(2)]);

    vm.begin_tick(1);
    vm.update(&mut world);
    vm.flush_queue(&mut world);
    let actions: Vec<_> = world.log.iter().map(|call| call.entry.id).collect();
    assert_eq!(actions, [id::FF_LIVES, id::FF_ROUND, id::FF_SKULL, id::FF_WAVE, id::PLACE_SQUAD_AT]);
    assert_eq!(world.log[4].args, [
        Arg::Squad(0),
        Arg::Class(Some("fixture".into())),
        Arg::Int(1),
        Arg::Int(1),
        Arg::Int(2),
    ]);

    vm.begin_tick(2);
    world.observed_squad = 1;
    vm.update(&mut world);
    vm.begin_tick(3);
    world.observed_squad = 0;
    vm.fire(&mut world, Input::ActorKill { killer: Some(0), class: "fixture".into() });
    vm.update(&mut world);
    vm.flush_queue(&mut world);
    assert_eq!(vm.global("wave"), Some(1));
    assert_eq!(vm.global("score_0"), Some(15));
    assert_eq!(world.log[5].entry.id, id::FF_SCORE);
    assert_eq!(world.log[5].args, [Arg::Player(Some(0)), Arg::Int(15), Arg::Bool(false)]);
}
