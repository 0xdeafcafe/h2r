use super::*;

fn learned_line() -> Entry {
    Entry {
        state: State::Learned,
        at: [-10.62, 3.88, -3.19],
        knobs: Knobs {
            how: Some(How::Runoff),
            from: Some([-8.33, 2.54, -0.76]),
            aim: Some([-10.54, 4.09]),
            yaw: Some(0.0),
            back: Some(1.2),
            run: Some(1.0),
            gain: Some(1.2),
            letgo: Some(0.03),
            ..Knobs::default()
        },
        tries: 64,
        hits: 19,
        n: 12,
        ok: 8,
        score: 1.214,
        best: 20.9,
        deaths: 1,
        why: None,
        stage: Stage::Compass,
        axis: 2,
        shrink: 1,
        moved: false,
        scattered: false,
    }
}

fn dead_line() -> Entry {
    Entry {
        state: State::Dead,
        at: [5.87, 6.36, 1.06],
        knobs: Knobs {
            how: Some(How::Stand),
            aim: Some([5.91, 6.36]),
            drift: Some(0.4),
            still: Some(4),
            ..Knobs::default()
        },
        tries: 120,
        hits: 0,
        n: 6,
        ok: 0,
        score: 0.412,
        best: 0.0,
        deaths: 0,
        why: Some("budget".into()),
        stage: Stage::Compass,
        axis: 1,
        shrink: 2,
        moved: false,
        scattered: true,
    }
}

const LEARNED_TEXT: &str = "bounce learned -10.620 3.880 -3.190 how=runoff fx=-8.330 fy=2.540 fz=-0.760 ax=-10.540 ay=4.090 yaw=0.0 back=1.20 run=1.00 gain=1.20 letgo=0.030 tries=64 hits=19 n=12 ok=8 score=1.214 best=20.90 deaths=1 stage=compass axis=2 shrink=1 moved=0 scattered=0";
const DEAD_TEXT: &str = "bounce dead 5.870 6.360 1.060 how=stand ax=5.910 ay=6.360 drift=0.40 still=4 tries=120 hits=0 n=6 ok=0 score=0.412 why=budget stage=compass axis=1 shrink=2 moved=0 scattered=1";

#[test]
fn text_round_trips() {
    // The spec's own example lines, byte for byte, and sorted by x.
    let text = to_text(&[dead_line(), learned_line()]);
    assert_eq!(text, format!("{}\n{LEARNED_TEXT}\n{DEAD_TEXT}\n", header()));
    let back = parse(&text).unwrap();
    assert_eq!(back, vec![learned_line(), dead_line()]);
    assert_eq!(to_text(&back), text, "byte-stable");
    // Any input order gives the same bytes.
    assert_eq!(to_text(&[learned_line(), dead_line()]), text);
    // Canonical entries read back equal.
    let mut odd = learned_line();
    odd.knobs.aim = Some([-10.54321, 4.0999]);
    odd.score = 1.2137;
    let c = odd.canonical();
    assert_eq!(parse(&to_text(&[c.clone()])).unwrap(), vec![c.clone()]);
    assert_eq!(c.knobs.aim, Some([-10.543, 4.100]));
    // No negative zero in the bytes.
    let mut z = learned_line();
    z.knobs.yaw = Some(-0.01);
    assert!(line(&z).contains(" yaw=0.0 "), "{}", line(&z));
    // Comments and blanks are skipped.
    let commented = format!("{}\n\n# a note\n{LEARNED_TEXT}\n", header());
    assert_eq!(parse(&commented).unwrap(), vec![learned_line()]);
    assert_eq!(parse("").unwrap(), Vec::<Entry>::new());
}

#[test]
fn a_bad_line_is_refused() {
    let h = header();
    let bad = |l: &str| parse(&format!("{h}\n# x\n{l}\n")).unwrap_err();
    for (l, want) in [
        (
            LEARNED_TEXT.replace("run=1.00", "rum=1.00"),
            "unknown key \"rum\"",
        ),
        (LEARNED_TEXT.replace("learned", "maybe"), "state \"maybe\""),
        (LEARNED_TEXT.replace("how=runoff", "how=hop"), "how \"hop\""),
        (
            LEARNED_TEXT.replace("compass", "polish"),
            "stage \"polish\"",
        ),
        (
            LEARNED_TEXT.replace("run=1.00", "run=1.50"),
            "out of its range",
        ),
        (
            LEARNED_TEXT.replace("letgo=0.030", "letgo=-1"),
            "out of its range",
        ),
        (DEAD_TEXT.replace("still=4", "still=31"), "out of its range"),
        (
            LEARNED_TEXT.replace("score=1.214", "score=9"),
            "out of its range",
        ),
        (LEARNED_TEXT.replace(" fy=2.540", ""), "go together"),
        (LEARNED_TEXT.replace(" ay=4.090", ""), "go together"),
        (
            LEARNED_TEXT.replace("run=1.00", "run=1.00 run=0.90"),
            "twice",
        ),
        (
            DEAD_TEXT.replace("still=4", "still=4 run=1.00"),
            "not a knob of how=stand",
        ),
        (
            LEARNED_TEXT.replace("letgo=0.030", "letgo=0.030 early=0.05"),
            "not a knob of how=runoff",
        ),
        (LEARNED_TEXT.replace("hits=19", "hits=65"), "over tries"),
        (LEARNED_TEXT.replace("ok=8", "ok=13"), "over n"),
        (
            LEARNED_TEXT.replace("deaths=1", "deaths=1 why=budget"),
            "why= on a learned line",
        ),
        (DEAD_TEXT.replace("why=budget", "why=No-Path"), "why="),
        (LEARNED_TEXT.replace("moved=0", "moved=2"), "0 or 1"),
        (LEARNED_TEXT.replace("axis=2", "axis=9"), "not under"),
        (LEARNED_TEXT.replace("shrink=1", "shrink=4"), "over 3"),
        (LEARNED_TEXT.replace("tries=64", "tries=-1"), "count"),
        (LEARNED_TEXT.replace("tries=64", "tries"), "key=value"),
        (LEARNED_TEXT.replace("-3.190", "nan"), "not a number"),
        ("bounce learned 1 2".to_string(), "no z"),
        ("crouch learned 1 2 3".to_string(), "want bounce"),
    ] {
        let err = bad(&l);
        assert!(err.starts_with("line 3:"), "{l}: {err}");
        assert!(err.contains(want), "{l}: {err} (want {want:?})");
    }
    // Another version, or no header.
    let err = parse(&format!("# superbounce learned v2\n{LEARNED_TEXT}\n")).unwrap_err();
    assert!(err.contains("version 2"), "{err}");
    assert!(
        parse(&format!("{LEARNED_TEXT}\n"))
            .unwrap_err()
            .contains("not a learned file")
    );
}

#[test]
fn an_untrained_spot_plays_the_route_s_own_knobs() {
    let stand = [1.0, 2.0, 3.0];
    assert!(knobs_for(&[], stand).is_default());
    let mut d = dead_line();
    d.at = stand;
    assert!(
        knobs_for(&[d.clone()], stand).is_default(),
        "a dead line plays nothing"
    );
    d.state = State::Blocked;
    assert!(knobs_for(&[d.clone()], stand).is_default());
    d.state = State::Partial;
    d.why = None;
    assert_eq!(knobs_for(&[d.clone()], stand).drift, Some(0.4));
    // Another site's line says nothing here.
    assert!(knobs_for(&[d], [9.0, 2.0, 3.0]).is_default());
    // `or` fills only what is unset.
    let k = Knobs {
        run: Some(0.5),
        ..Knobs::default()
    };
    let base = Knobs {
        run: Some(1.0),
        back: Some(0.6),
        ..Knobs::default()
    };
    assert_eq!(k.or(base).run, Some(0.5));
    assert_eq!(k.or(base).back, Some(0.6));
}

#[test]
fn merge_keeps_the_better_line_whole() {
    let a = learned_line();
    let at = a.at;
    let mut open = Entry {
        state: State::Open,
        at,
        tries: 200,
        hits: 30,
        n: 4,
        ok: 2,
        score: 1.1,
        ..Entry::default()
    };
    // Learned over not, however many tries.
    assert_eq!(rank(&a, &open), Ordering::Greater);
    // Some hits over none.
    let none = Entry {
        hits: 0,
        tries: 500,
        ..open.clone()
    };
    assert_eq!(rank(&open, &none), Ordering::Greater);
    // Between two learned, the higher rate.
    let mut better = a.clone();
    better.ok = 10;
    assert_eq!(rank(&better, &a), Ordering::Greater);
    // More tries, then the higher score, then the text.
    let mut more = open.clone();
    more.tries += 1;
    assert_eq!(rank(&more, &open), Ordering::Greater);
    let mut higher = open.clone();
    higher.score += 0.1;
    assert_eq!(rank(&higher, &open), Ordering::Greater);
    let mut blocked = open.clone();
    blocked.state = State::Blocked;
    blocked.why = Some("no_path".into());
    assert_eq!(
        rank(&blocked, &open),
        Ordering::Greater,
        "sorts first as text"
    );
    // A search carried on from a line beats it.
    let mut on = open.clone();
    on.tries += 6;
    on.score -= 0.3;
    assert_eq!(rank(&on, &open), Ordering::Greater);

    // Total, commutative, idempotent, the same in any order.
    open.at = [1.0, 0.0, 0.0];
    let mut far = dead_line();
    far.at = [1.0, 0.0, 0.0];
    let lines = [
        a.clone(),
        better.clone(),
        open.clone(),
        far.clone(),
        more.clone(),
        dead_line(),
    ];
    for x in &lines {
        for y in &lines {
            assert_eq!(rank(x, y), rank(y, x).reverse());
            for z in &lines {
                if rank(x, y) == Ordering::Greater && rank(y, z) == Ordering::Greater {
                    assert_eq!(rank(x, z), Ordering::Greater);
                }
            }
        }
    }
    let m = merge(&[a.clone(), open.clone()], &[better.clone(), far.clone()]);
    assert_eq!(
        m,
        merge(&[better.clone(), far.clone()], &[a.clone(), open.clone()])
    );
    assert_eq!(
        m,
        merge(
            &merge(&[far.clone()], &[a.clone()]),
            &[open.clone(), better.clone()]
        )
    );
    assert_eq!(merge(&m, &m), m, "merging twice adds nothing");
    assert_eq!(merge(&m, &[]), m);
    assert_eq!(m.len(), 2);
    assert_eq!(m[0], better, "sorted by x");
    assert_eq!(m[1], open, "the open line beats the dead one by its hits");
    assert_eq!(to_text(&m), to_text(&merge(&[far, better], &[open, a])));
}

#[test]
fn replace_puts_a_site_s_line_in_place() {
    let mut on = learned_line();
    on.tries += 1;
    let r = replace(&[learned_line(), dead_line()], on.clone());
    assert_eq!(r, vec![on, dead_line()]);
}

#[test]
fn spot_ids_are_fixed_and_differ() {
    let a = spot_id("ascension", [-10.62, 3.88, -3.19]);
    assert_eq!(
        a,
        spot_id("ascension", [-10.6201, 3.8799, -3.19]),
        "as printed"
    );
    assert_ne!(a, spot_id("ascension", [-10.63, 3.88, -3.19]));
    assert_ne!(a, spot_id("lockout", [-10.62, 3.88, -3.19]));
    // FNV-1a 64 of the empty-map string, a fixed value.
    assert_eq!(spot_id("", [0.0, 0.0, 0.0]), spot_id("", [-0.0, 0.0, 0.0]));
}
