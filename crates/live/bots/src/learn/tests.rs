use super::*;

const AT: Vec3 = [3.0, 0.0, 0.0];

fn base() -> Knobs {
    Knobs {
        // The flight model's choice: a running jump at a slow stick.
        how: Some(How::Jump),
        from: Some([0.0, 0.0, 2.0]),
        aim: Some([3.0, 0.0]),
        yaw: Some(0.0),
        back: Some(0.6),
        run: Some(0.6),
        early: Some(0.05),
        gain: Some(2.5),
        letgo: Some(0.03),
        drift: Some(0.5),
        still: Some(4),
    }
}

fn setup(budget: u32) -> Setup {
    Setup {
        at: AT,
        spot: spot_id("test", AT),
        seed: 1,
        budget,
        hows: vec![How::Jump, How::Runoff],
        base: base(),
        run_off: 0.5,
        need: 5.0,
    }
}

/// A fake landing: a run-off at a full stick comes down fast enough, 0.15
/// short of its aim; a slower stick lands slower and shorter; a running
/// jump is always too slow. It bounces within 0.05 of a hidden groove.
struct Model {
    groove: [f32; 2],
    /// The landing's spread, each axis, world units.
    noise: f32,
    /// Extra shortfall.
    short: f32,
    tries: u32,
    seed: u64,
}

impl Model {
    fn new(groove: [f32; 2], noise: f32, seed: u64) -> Model {
        Model {
            groove,
            noise,
            short: 0.0,
            tries: 0,
            seed,
        }
    }

    fn fly(&mut self, k: Knobs) -> Try {
        let i = self.tries;
        self.tries += 1;
        let k = k.or(base());
        let run = k.run.unwrap();
        let aim = k.aim.unwrap();
        let (speed_in, bias) = match k.how.unwrap() {
            How::Runoff => (3.0 + 2.4 * run, -0.15 - 0.8 * (1.0 - run)),
            How::Jump => (3.0 + 1.0 * run, -0.27),
            How::Stand => (3.0, 0.0),
        };
        let u = |axis: u32| (draw(self.seed, 7, i, axis) * 2.0 - 1.0) * self.noise;
        let at = [
            aim[0] + bias - self.short + u(0),
            aim[1] + k.yaw.unwrap() * 0.005 + u(1),
        ];
        let g = (at[0] - self.groove[0]).hypot(at[1] - self.groove[1]);
        let bounced = g < 0.05 && speed_in >= 5.0;
        let off = [AT[0] - at[0], AT[1] - at[1]];
        Try {
            void: None,
            down: Some(Down {
                miss: off[0].hypot(off[1]),
                off,
                speed_in,
                vz_out: if bounced { 15.0 + u(2) } else { 0.5 },
            }),
            died: false,
        }
    }
}

/// Flies one round: each slot the search asks, then the fold.
fn round(s: &mut Search, m: &mut Model, asked: &mut Vec<Knobs>) -> Folded {
    let mut done = Vec::new();
    while let Some(k) = s.next(&done) {
        asked.push(k);
        done.push(m.fly(k));
        assert!(done.len() < 100, "a round never ends");
    }
    s.fold(&done)
}

fn run_out(s: &mut Search, m: &mut Model) -> Vec<Folded> {
    let mut out = Vec::new();
    let mut asked = Vec::new();
    while s.state() == State::Open {
        out.push(round(s, m, &mut asked));
        assert!(out.len() < 500, "the search never stops");
    }
    out
}

#[test]
fn search_finds_the_groove_on_a_fake_landing() {
    let groove = [3.10, 0.02];
    for seed in 1..6 {
        let mut s = Search::new(setup(200), None);
        let mut m = Model::new(groove, 0.01, seed);
        let rounds = run_out(&mut s, &mut m);
        let e = s.entry();
        assert_eq!(e.state, State::Learned, "seed {seed}: {e:?}\n{rounds:#?}");
        assert!(e.tries <= 200);
        assert!(e.n >= LEARNED_N && e.rate() >= LEARNED_RATE);
        let k = e.knobs;
        assert_eq!(
            k.how,
            Some(How::Runoff),
            "the arms beat the flight model's jump"
        );
        assert_eq!(k.run, Some(1.0));
        // Where the centre lands, without the noise.
        let aim = k.aim.unwrap();
        let down = [aim[0] - 0.15, aim[1]];
        let g = (down[0] - groove[0]).hypot(down[1] - groove[1]);
        assert!(g < 0.05, "seed {seed}: lands {g} from the groove: {k:?}");
        // The line reads back as it is held.
        assert_eq!(
            learned::parse(&learned::to_text(std::slice::from_ref(&e))).unwrap(),
            vec![e.clone()]
        );
    }
}

#[test]
fn shift_closes_a_short_landing_before_any_bounce() {
    // A groove nowhere near: nothing bounces.
    let mut s = Search::new(setup(1000), None);
    let mut m = Model::new([50.0, 50.0], 0.0, 1);
    m.short = 0.05;
    let mut asked = Vec::new();
    let arms = round(&mut s, &mut m, &mut asked);
    assert_eq!((arms.stage, s.entry().stage), (Stage::Arms, Stage::Shift));
    let start_aim = s.centre().aim.unwrap();
    let mut last = arms.after;
    let mut shifts = 0;
    while s.entry().stage == Stage::Shift {
        let f = round(&mut s, &mut m, &mut asked);
        assert_eq!(f.stage, Stage::Shift);
        assert!(
            f.after > last,
            "the score rises round by round: {last} -> {}",
            f.after
        );
        assert!(
            f.change.as_deref().is_some_and(|c| c.starts_with("aim +")),
            "{f:?}"
        );
        last = f.after;
        shifts += 1;
        assert!(shifts <= SHIFT_ROUNDS);
    }
    assert_eq!(shifts, SHIFT_ROUNDS);
    let aim = s.centre().aim.unwrap();
    // 0.2 short (0.15 and 0.05): the aim moves on, most of the way.
    assert!(aim[0] - start_aim[0] > 0.15, "{start_aim:?} -> {aim:?}");
    assert!((aim[1] - start_aim[1]).abs() < 1e-3);
    assert_eq!((s.entry().stage, s.entry().shrink), (Stage::Compass, 0));
}

#[test]
fn a_site_that_cannot_bounce_ends_dead() {
    let mut s = Search::new(setup(5000), None);
    let mut m = Model::new([50.0, 50.0], 0.01, 3);
    run_out(&mut s, &mut m);
    let e = s.entry();
    assert_eq!(
        (e.state, e.why.as_deref()),
        (State::Dead, Some("no_bounce")),
        "{e:?}"
    );
    assert!(e.scattered && e.hits == 0 && e.tries < 5000);
    // The same with a small budget: dead by it.
    let mut s = Search::new(setup(30), None);
    run_out(&mut s, &mut Model::new([50.0, 50.0], 0.01, 3));
    let e = s.entry();
    assert_eq!((e.state, e.why.as_deref()), (State::Dead, Some("budget")));
    assert!(e.tries >= 30);
    // A bounce now and then, then the budget: partial, and a higher budget
    // opens it again.
    let mut s = Search::new(setup(12), None);
    let mut m = Model::new([2.85, 0.0], 0.0, 1);
    run_out(&mut s, &mut m);
    let e = s.entry().clone();
    assert_eq!(e.state, State::Partial, "{e:?}");
    assert_eq!(Search::new(setup(12), Some(&e)).state(), State::Partial);
    assert_eq!(Search::new(setup(100), Some(&e)).state(), State::Open);
}

#[test]
fn all_void_rounds_end_blocked() {
    let mut s = Search::new(setup(100), None);
    let first = s.next_slot(&[]).unwrap();
    // A void try asks its slot again, twice at most.
    let v = Try::void("no_path");
    assert_eq!(s.next_slot(&[v]).unwrap(), first);
    assert_eq!(s.next_slot(&[v, v]).unwrap(), first);
    assert_eq!(s.next_slot(&[v, v, v]).unwrap().0, 1);
    for r in 0..VOID_ROUNDS {
        let mut done = Vec::new();
        while s.next(&done).is_some() {
            done.push(if done.len() % 5 == 4 {
                Try::void("not_primed")
            } else {
                v
            });
        }
        let f = s.fold(&done);
        assert!(f.void);
        assert_eq!(f.tries, 0, "void tries cost no budget");
        let want = if r + 1 < VOID_ROUNDS {
            State::Open
        } else {
            State::Blocked
        };
        assert_eq!(s.state(), want);
    }
    let e = s.entry();
    assert_eq!(e.why.as_deref(), Some("no_path"), "the commonest reason");
    assert_eq!((e.stage, e.tries), (Stage::Arms, 0), "never flown");
    assert!(s.next(&[]).is_none());
    // A scored round between void ones resets the count.
    let mut s = Search::new(setup(100), None);
    let mut m = Model::new([50.0, 50.0], 0.0, 1);
    for _ in 0..2 {
        s.fold(&[]);
    }
    round(&mut s, &mut m, &mut Vec::new());
    for _ in 0..2 {
        s.fold(&[]);
    }
    assert_eq!(s.state(), State::Open);
    // A site flown before is never blocked: its bots are where their last
    // tries left them, not the site.
    for _ in 0..VOID_ROUNDS * 3 {
        s.fold(&[]);
    }
    assert_eq!(s.state(), State::Open);
    // And a blocked line with tries, from the file, is flown again.
    let mut e = s.entry().clone();
    assert!(e.tries > 0);
    e.state = State::Blocked;
    e.why = Some("no_path".into());
    assert_eq!(Search::new(setup(100), Some(&e)).state(), State::Open);
}

#[test]
fn the_same_results_give_the_same_search() {
    let go = || {
        let mut s = Search::new(setup(150), None);
        let mut m = Model::new([3.10, 0.02], 0.03, 9);
        let mut asked = Vec::new();
        while s.state() == State::Open {
            round(&mut s, &mut m, &mut asked);
        }
        (s.entry().clone(), asked)
    };
    let (a, ka) = go();
    let (b, kb) = go();
    assert_eq!(a, b);
    assert_eq!(ka, kb);
    assert_eq!(learned::to_text(&[a]), learned::to_text(&[b]));
    // The draw: fixed for fixed arguments, different across each.
    let d = draw(11, 22, 33, 4);
    assert_eq!(d, draw(11, 22, 33, 4));
    assert!((0.0..1.0).contains(&d));
    for other in [
        draw(12, 22, 33, 4),
        draw(11, 23, 33, 4),
        draw(11, 22, 34, 4),
        draw(11, 22, 33, 5),
    ] {
        assert_ne!(d, other);
    }
    // Spread over the range.
    let mean: f32 = (0..1000).map(|i| draw(1, 2, i, 3)).sum::<f32>() / 1000.0;
    assert!((mean - 0.5).abs() < 0.05, "{mean}");
}

#[test]
fn a_search_resumes_from_its_line() {
    let groove = [3.10, 0.02];
    let mut a = Search::new(setup(200), None);
    let mut b = Search::new(setup(200), None);
    let mut ma = Model::new(groove, 0.03, 4);
    let mut mb = Model::new(groove, 0.03, 4);
    let (mut ka, mut kb) = (Vec::new(), Vec::new());
    let mut rounds = 0;
    while a.state() == State::Open {
        round(&mut a, &mut ma, &mut ka);
        round(&mut b, &mut mb, &mut kb);
        // B is written and read back after every round.
        let text = learned::to_text(&[b.entry().clone()]);
        let back = learned::parse(&text).unwrap();
        b = Search::new(setup(200), back.first());
        assert_eq!(a.entry(), b.entry(), "round {rounds}");
        assert_eq!(ka, kb, "round {rounds}");
        rounds += 1;
    }
    assert!(rounds > 4);
    assert_eq!(b.state(), a.state());
}

#[test]
fn score_orders_the_outcomes() {
    let down = |miss: f32, speed_in: f32, vz_out: f32| Try {
        void: None,
        down: Some(Down {
            miss,
            off: [miss, 0.0],
            speed_in,
            vz_out,
        }),
        died: false,
    };
    let need = 5.0;
    let bounce = score(&down(0.3, 5.4, 12.0), need);
    let near = score(&down(0.02, 5.4, 0.5), need);
    let far = score(&down(0.3, 5.4, 0.5), need);
    let slow = score(&down(0.02, 2.0, 0.5), need);
    let none = score(&Try::default(), need);
    let deadly_bounce = score(
        &Try {
            died: true,
            ..down(0.0, 5.4, 12.0)
        },
        need,
    );
    assert!(
        bounce > near && near > far && far > none,
        "{bounce} {near} {far} {none}"
    );
    assert!(near > slow, "fast over slow");
    assert!(bounce >= 1.0 && score(&down(0.0, 5.4, 40.0), need) == 1.5);
    assert!(deadly_bounce < far, "a deadly bounce under a clean miss");
    assert_eq!(none, 0.0);
    // An unknown need: the speed term is full.
    assert!((score(&down(0.0, 0.0, 0.5), 0.0) - 0.8).abs() < 1e-6);
    // A void try says nothing.
    assert!(!Try::void("map_changed").scored());
    assert!(!Try::void("map_changed").bounced());
}

#[test]
fn an_untrained_spot_gets_default_knobs() {
    let s = Search::new(setup(100), None);
    assert!(s.centre().is_default(), "no line: the route's own knobs");
    assert_eq!(
        (s.state(), s.entry().stage, s.entry().tries),
        (State::Open, Stage::Arms, 0)
    );
    // The first slot flies the route's own choice at a full stick.
    let k = s.next(&[]).unwrap();
    assert_eq!(k.how, Some(How::Jump));
    assert_eq!(k.run, Some(1.0));
    assert_eq!(k.aim, Some([3.0, 0.0]));
    assert!(learned::knobs_for(&[], AT).is_default());
}

#[test]
fn from_leap_reads_a_try() {
    use crate::bounce_route::Touchdown;
    let mut leap = Leap {
        tick: 0,
        takeoff: [0.0, 0.0, 2.0],
        speed: 2.0,
        vz: 0.0,
        word: None,
        stick_sum: [0.0; 2],
        stick_max: 0.0,
        ticks: 0,
        idle: 0,
        landing: Some(Touchdown {
            tick: 10,
            at: [2.85, 0.0, 0.0],
            speed_in: 5.4,
            vz_out: 0.5,
            miss: 0.1,
            word: None,
        }),
        end: Some("no_bounce"),
    };
    let t = Try::from_leap(Some(&leap), AT, false);
    let d = t.down.unwrap();
    assert!((d.miss - 0.15).abs() < 1e-4 && (d.off[0] - 0.15).abs() < 1e-4);
    assert!(t.scored() && !t.bounced());
    leap.end = Some("not_primed");
    assert_eq!(
        Try::from_leap(Some(&leap), AT, false).void,
        Some("not_primed")
    );
    let none = Try::from_leap(None, AT, true);
    assert!(none.scored() && none.down.is_none() && none.died);
}

fn site(at: Vec3) -> SiteIn {
    SiteIn {
        at,
        hows: vec![How::Runoff],
        base: Knobs {
            from: Some([at[0] - 3.0, at[1], at[2] + 2.0]),
            aim: Some([at[0], at[1]]),
            ..Knobs::default()
        },
        run_off: 0.5,
        need: 5.0,
    }
}

#[test]
fn shards_split_the_sites_and_the_pick_is_fewest_rounds_nearest_first() {
    let sites: Vec<SiteIn> = (0..40)
        .map(|i| site([i as f32 * 1.37, (i % 7) as f32, 0.0]))
        .collect();
    let train = |shard| Train {
        shard,
        shards: 4,
        budget: 60,
        seed: 1,
    };
    let boards: Vec<Board> = (0..4)
        .map(|s| Board::new("ascension", train(s), sites.clone(), &[]))
        .collect();
    let total: usize = boards.iter().map(|b| b.sites().len()).sum();
    assert_eq!(total, 40, "every site in exactly one shard");
    for s in &sites {
        let id = spot_id("ascension", s.at);
        let owners = boards
            .iter()
            .filter(|b| b.sites().iter().any(|h| h.search.setup().spot == id))
            .count();
        assert_eq!(owners, 1);
    }

    let mut b = Board::new(
        "lockout",
        Train {
            shard: 0,
            shards: 1,
            budget: 60,
            seed: 1,
        },
        vec![
            site([0.0, 0.0, 0.0]),
            site([10.0, 0.0, 0.0]),
            site([20.0, 0.0, 0.0]),
        ],
        &[],
    );
    // Nearest first; a held site is not given to another bot; a bot keeps
    // its own.
    assert_eq!(b.pick(1, [19.0, 0.0, 0.0]), Some(2));
    assert_eq!(b.pick(1, [0.0, 0.0, 0.0]), Some(2));
    assert_eq!(b.pick(2, [19.0, 0.0, 0.0]), Some(1));
    // A round flown: the site is let go, with one round more.
    let mut m = Model::new([50.0, 50.0], 0.0, 1);
    let mut done = Vec::new();
    while let Some(k) = b.next(2, &done) {
        done.push(m.fly(k));
    }
    let f = b.fold(2, &done).unwrap();
    assert!(
        b.describe(2, &f)
            .starts_with("site (20.00, 0.00, 0.00) round 1 arms")
    );
    assert_eq!(b.claimed(1), None);
    // Fewest rounds first: site 0, though site 2 is nearer.
    assert_eq!(b.pick(1, [19.0, 0.0, 0.0]), Some(0));
    b.release(2);
    assert_eq!(b.pick(3, [19.0, 0.0, 0.0]), Some(1));
    assert!(!b.done());
    // A board of closed sites is done.
    let closed = Entry {
        state: State::Learned,
        at: [0.0, 0.0, 0.0],
        n: 10,
        ok: 10,
        ..Entry::default()
    };
    let b = Board::new(
        "lockout",
        Train {
            shard: 0,
            shards: 1,
            budget: 60,
            seed: 1,
        },
        vec![site([0.0, 0.0, 0.0])],
        &[closed],
    );
    assert!(b.done());
    assert_eq!(b.clone().pick(1, [0.0; 3]), None);
}

#[test]
fn the_process_board_is_set_and_read() {
    set(None);
    assert!(!done());
    install(Board::new(
        "x",
        Train {
            shard: 0,
            shards: 1,
            budget: 1,
            seed: 0,
        },
        vec![],
        &[],
    ));
    assert!(with_board(|_| ()).is_none(), "no training run: no board");
    let t = Train {
        shard: 0,
        shards: 1,
        budget: 1,
        seed: 0,
    };
    set(Some(t));
    assert_eq!(current(), Some(t));
    assert!(!done(), "false until the board is built");
    install(Board::new("x", t, vec![], &[]));
    assert!(done(), "no open site");
    set(None);
    assert!(!done());
}

#[test]
fn reopen_opens_dead_and_blocked_lines() {
    let mut v = vec![
        Entry {
            state: State::Dead,
            why: Some("budget".into()),
            ..Entry::default()
        },
        Entry {
            state: State::Learned,
            ..Entry::default()
        },
    ];
    reopen(&mut v);
    assert_eq!((v[0].state, v[0].why.clone()), (State::Open, None));
    assert_eq!(v[1].state, State::Learned);
}
