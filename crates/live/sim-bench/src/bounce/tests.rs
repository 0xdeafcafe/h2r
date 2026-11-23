use super::*;

const A: [f32; 3] = [-10.62, 3.88, -3.19];
const B: [f32; 3] = [4.0, 1.0, 2.0];

fn who() -> Vec<Who> {
    vec![
        Who {
            index: 1,
            seat: Some(0),
            name: "Clanker 0".into(),
            arm: Some("base".into()),
        },
        Who {
            index: 2,
            seat: Some(1),
            name: "Clanker 1".into(),
            arm: Some("base".into()),
        },
    ]
}

/// A bounce off the floor at `floor`: the touchdown, the chain's award,
/// then up to `floor + rise` and back down.
fn launch(t: &mut BounceTally, player: u8, tick: &mut u32, floor: f32, rise: f32, chain: u32) {
    t.height(player, *tick, floor + 0.3);
    *tick += 1;
    t.height(player, *tick, floor);
    t.event(player, *tick, Event::Chain { chain });
    for k in 1..=10 {
        *tick += 1;
        t.height(player, *tick, floor + rise * k as f32 / 10.0);
    }
    for k in 1..=3 {
        *tick += 1;
        t.height(player, *tick, floor + rise - 0.1 * k as f32);
    }
}

#[test]
fn tries_primes_bounces_and_reasons_by_bot_and_by_site() {
    let mut t = BounceTally::new();
    let mut tick = 1000;
    // Bot 1 at A: primes itself, bounces (a chain of two), then misses.
    t.event(1, tick, Event::Started { site: A });
    t.event(
        1,
        tick,
        Event::Primed {
            site: A,
            read: true,
            own: true,
        },
    );
    launch(&mut t, 1, &mut tick, A[2], 2.0, 1);
    launch(&mut t, 1, &mut tick, A[2], 3.0, 2);
    t.event(
        1,
        tick,
        Event::Tried {
            site: A,
            bounced: true,
            why: None,
        },
    );
    t.event(1, tick, Event::Started { site: A });
    t.event(
        1,
        tick,
        Event::Primed {
            site: A,
            read: false,
            own: false,
        },
    );
    t.event(
        1,
        tick,
        Event::Tried {
            site: A,
            bounced: false,
            why: Some("missed".into()),
        },
    );
    // Bot 2 at B: the prime fails; then a Bounced.
    t.event(2, tick, Event::Started { site: B });
    t.event(
        2,
        tick,
        Event::Tried {
            site: B,
            bounced: false,
            why: Some("prime_failed".into()),
        },
    );
    t.event(2, tick, Event::Bounced { chain: 0 });
    let r = t.report(Some(1000), &who());
    assert_eq!(r.format, FORMAT);
    let one = &r.players[0].stats;
    assert_eq!(
        (one.attempts, one.primes, one.primes_read),
        (2, 1, 1),
        "a carried prime is not one"
    );
    assert_eq!(
        (one.practice_bounces, one.bounced_tries, one.chain_bounces),
        (1, 1, 2)
    );
    assert_eq!(one.bounce_rate, 0.5);
    assert_eq!(one.best_chain, 2);
    assert_eq!(one.chains.get("2"), Some(&1), "{:?}", one.chains);
    let l = one.launch.unwrap();
    assert_eq!(l.n, 2);
    assert!(
        (l.best - 3.0).abs() < 1e-4 && (l.mean - 2.5).abs() < 1e-4,
        "{l:?}"
    );
    assert_eq!(one.why.get("missed"), Some(&1));
    assert_eq!(r.players[0].first_bounce_ticks, Some(1));
    let two = &r.players[1].stats;
    assert_eq!(
        (two.attempts, two.bounced_deaths, two.bounce_rate),
        (1, 1, 0.0)
    );
    assert_eq!(two.why.get("prime_failed"), Some(&1));
    assert_eq!(r.players[1].first_bounce_ticks, None);
    // By site.
    assert_eq!(r.sites.len(), 2);
    let a = r.sites.iter().find(|s| s.site == A).unwrap();
    assert_eq!(
        (
            a.stats.attempts,
            a.stats.bounced_tries,
            a.stats.chain_bounces
        ),
        (2, 1, 2)
    );
    assert!(a.stats.launch.is_some());
    let b = r.sites.iter().find(|s| s.site == B).unwrap();
    assert_eq!(
        (b.stats.attempts, b.stats.why.get("prime_failed")),
        (1, Some(&1))
    );
    // The totals are the bots'.
    assert_eq!(r.totals.attempts, 3);
    assert_eq!(r.totals.chain_bounces, 2);
    assert!((r.totals.bounce_rate - 1.0 / 3.0).abs() < 1e-9);
    assert_eq!(r.totals.bounced_deaths, 1);
}

#[test]
fn a_drills_word_read_at_the_start_is_armed_not_a_prime() {
    let mut t = BounceTally::new();
    for read in [true, true, false] {
        t.event(1, 10, Event::Started { site: A });
        t.event(
            1,
            10,
            Event::Primed {
                site: A,
                read,
                own: false,
            },
        );
        t.event(
            1,
            20,
            Event::Tried {
                site: A,
                bounced: false,
                why: Some("missed".into()),
            },
        );
    }
    let r = t.report(Some(0), &who());
    let s = &r.players[0].stats;
    assert_eq!((s.attempts, s.primes, s.primes_read, s.armed), (3, 0, 0, 2));
    assert_eq!(r.sites[0].stats.armed, 2);
    assert_eq!(r.totals.armed, 2);
}

#[test]
fn a_chain_bounce_outside_a_try_counts_for_the_bot_not_a_site() {
    let mut t = BounceTally::new();
    let mut tick = 5;
    launch(&mut t, 1, &mut tick, 0.0, 1.5, 1);
    let r = t.report(Some(0), &who());
    assert_eq!(r.players[0].stats.chain_bounces, 1);
    assert_eq!(r.players[0].stats.attempts, 0);
    assert!(r.sites.is_empty());
    assert_eq!(
        r.players[0].stats.chains.get("1"),
        Some(&1),
        "an open chain counts at the end"
    );
}

#[test]
fn the_block_round_trips_through_json_with_a_stable_shape() {
    let mut t = BounceTally::new();
    let mut tick = 0;
    t.event(1, tick, Event::Started { site: A });
    launch(&mut t, 1, &mut tick, A[2], 2.0, 1);
    t.event(
        1,
        tick,
        Event::Tried {
            site: A,
            bounced: true,
            why: None,
        },
    );
    let r = t.report(Some(0), &who());
    let text = serde_json::to_string(&r).unwrap();
    let back: BounceReport = serde_json::from_str(&text).unwrap();
    assert_eq!(back, r);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    for key in ["format", "totals", "players", "sites"] {
        assert!(v.get(key).is_some(), "{key}");
    }
    for key in [
        "attempts",
        "primes",
        "primes_read",
        "practice_bounces",
        "bounced_tries",
        "bounce_rate",
        "chain_bounces",
        "best_chain",
        "bounced_deaths",
        "launch",
    ] {
        assert!(v["totals"].get(key).is_some(), "totals.{key}");
    }
    assert_eq!(v["players"][0]["name"], "Clanker 0");
    assert_eq!(v["players"][0]["first_bounce_ticks"], 1);
    // An older reader's empty block reads too.
    let empty: BounceReport = serde_json::from_str(
        r#"{"format":"h2-sim-bounce/1","totals":{"attempts":0,"primes":0,"primes_read":0,
        "practice_bounces":0,"bounced_tries":0,"bounce_rate":0.0,"chain_bounces":0,
        "best_chain":0,"bounced_deaths":0},"players":[],"sites":[]}"#,
    )
    .unwrap();
    assert_eq!(empty.totals, Stats::default());
}

#[test]
fn pooling_adds_counts_and_weighs_launches_by_count() {
    let a = Stats {
        attempts: 4,
        bounced_tries: 1,
        launch: Some(Launch {
            n: 1,
            best: 2.0,
            mean: 2.0,
        }),
        why: [("missed".to_string(), 3)].into(),
        ..Stats::default()
    };
    let b = Stats {
        attempts: 6,
        bounced_tries: 4,
        launch: Some(Launch {
            n: 3,
            best: 4.0,
            mean: 3.0,
        }),
        why: [("missed".to_string(), 1), ("too_slow".to_string(), 1)].into(),
        ..Stats::default()
    };
    let p = pool([&a, &b]);
    assert_eq!((p.attempts, p.bounced_tries), (10, 5));
    assert_eq!(p.bounce_rate, 0.5);
    let l = p.launch.unwrap();
    assert_eq!((l.n, l.best), (4, 4.0));
    assert!((l.mean - 2.75).abs() < 1e-9);
    assert_eq!(p.why.get("missed"), Some(&4));
    assert_eq!(pool(std::iter::empty()), Stats::default());
}

#[test]
fn the_match_report_carries_the_block_only_in_a_superbounce_match() {
    use crate::metrics::{BotInfo, End, Finish, Tally};
    let bots = [BotInfo {
        seat: 0,
        player: Some(1),
        name: "Clanker 0".into(),
        arm: None,
    }];
    let finish = || Finish {
        bots: &bots,
        ..Finish::bare(End::TickLimit)
    };
    let mut plain = Tally::new();
    plain.bounce_event(1, 0, Event::Started { site: A });
    assert_eq!(plain.report(finish()).bounce, None);
    let mut t = Tally::new();
    t.track_bounces();
    // A start (or a snapshot load's restart) keeps it on.
    t.start(100);
    t.bounce_event(1, 101, Event::Started { site: A });
    t.bounce_event(
        1,
        130,
        Event::Tried {
            site: A,
            bounced: false,
            why: Some("too_slow".into()),
        },
    );
    let r = t.report(finish());
    let b = r.bounce.clone().expect("a bounce block");
    assert_eq!(b.players.len(), 1);
    assert_eq!(b.players[0].stats.attempts, 1);
    assert_eq!(b.totals.why.get("too_slow"), Some(&1));
    let text = serde_json::to_string(&r).unwrap();
    let back: crate::metrics::Report = serde_json::from_str(&text).unwrap();
    assert_eq!(back.bounce, Some(b));
}

#[test]
fn habits_count_variety_fumbles_chains_monsters_and_yields() {
    let mut t = BounceTally::new();
    let mut tick = 100;
    // Bot 1 at A: a fumbled try at a Monster site with two links planned,
    // which the chain still filed a bounce in (a Monster), one link made.
    t.event(1, tick, Event::Started { site: A });
    t.event(
        1,
        tick,
        Event::Intent {
            site: A,
            monster: true,
            links: 2,
            fumble: Some("off_line".into()),
            again: false,
        },
    );
    launch(&mut t, 1, &mut tick, -3.19, 4.0, 1);
    t.event(1, tick, Event::Monster);
    t.event(
        1,
        tick,
        Event::Chained {
            site: A,
            tried: 2,
            made: 1,
        },
    );
    t.event(
        1,
        tick,
        Event::Tried {
            site: A,
            bounced: true,
            why: None,
        },
    );
    // Again at A (a retry), missed; then it lets players go first at B
    // twice (the second time it gives B up), steps aside, and tries B.
    tick += 10;
    t.event(1, tick, Event::Started { site: A });
    t.event(
        1,
        tick,
        Event::Intent {
            site: A,
            monster: true,
            links: 0,
            fumble: None,
            again: true,
        },
    );
    t.event(
        1,
        tick,
        Event::Tried {
            site: A,
            bounced: false,
            why: Some("missed".into()),
        },
    );
    t.event(
        1,
        tick,
        Event::Yielded {
            site: B,
            waited: 60,
            gave_up: false,
        },
    );
    t.event(
        1,
        tick,
        Event::Yielded {
            site: B,
            waited: 300,
            gave_up: true,
        },
    );
    t.event(1, tick, Event::SteppedAside);
    t.event(1, tick, Event::Started { site: B });
    t.event(
        1,
        tick,
        Event::Tried {
            site: B,
            bounced: false,
            why: Some("too_slow".into()),
        },
    );
    let r = t.report(Some(0), &who());
    let h = &r.players[0].stats.habits;
    assert_eq!((h.repeats, h.distinct_sites), (1, 2));
    assert_eq!((h.fumbles, h.fumble_bounces, h.retries), (1, 1, 1));
    assert_eq!((h.monster_tries, h.monster_bounces), (2, 1));
    assert_eq!(
        (h.chain_links_planned, h.chain_links_tried, h.chain_links_made),
        (2, 2, 1)
    );
    assert_eq!(
        (h.yields, h.yields_given_up, h.yield_ticks, h.stepped_aside),
        (2, 1, 360, 1)
    );
    // The totals add the bots' habits.
    assert_eq!(r.totals.habits, *h);
    // By site: A had the fumble and the Monster, B the waits.
    let at = |p: [f32; 3]| r.sites.iter().find(|s| s.site == p).unwrap().stats.habits.clone();
    assert_eq!((at(A).fumbles, at(A).monster_bounces, at(A).yields), (1, 1, 0));
    assert_eq!((at(B).yields, at(B).yields_given_up), (2, 1));
    // A block from before the habits reads with none.
    assert_eq!(Stats::default().habits, Habits::default());
}
