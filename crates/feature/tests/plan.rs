//! `Registry::build` is pure: order, conflicts, missing points and
//! switches, with no runtime and no guest.

use engine::points::{self, Point, Sig};
use feature::{Base, BuildError, Conflict, Flow, Kind, Registry, Remap, Switches, from_fn};

fn noop_before(b: &mut feature::Builder, p: &'static Point) {
    b.raw().before(p, "b", |_| Ok(Flow::Continue));
}

#[test]
fn two_tick_observers_share_one_chain() {
    // Lockstep's hash and Live telemetry's sample both on the game tick:
    // before the registry, the second refused (or displaced) the first.
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("lockstep", |b| {
        b.frame().on_tick("hash", |_| {});
        Ok(())
    }))
    .unwrap();
    r.add(&from_fn("live-telemetry", |b| {
        b.frame().on_tick("sample", |_| {});
        Ok(())
    }))
    .unwrap();
    let plan = r.build(&Base).unwrap();
    let chain = &plan.chains[&points::GAME_TICK.base];
    let ids: Vec<_> = chain.observe_after.iter().map(|h| h.feature).collect();
    assert_eq!(ids, ["live-telemetry", "lockstep"], "observers by id");
    assert!(chain.shadow.is_some(), "something runs after the tick");
    assert_eq!(
        plan.text(),
        "feature lockstep: on\nfeature live-telemetry: on\n\
         0x00137fe0 GAME_TICK: original -> observe_after [live-telemetry:sample, lockstep:hash]\n"
    );
}

#[test]
fn a_string_hook_and_the_rust_lookup_share_the_locale_point() {
    let mut r = Registry::new(Switches::defaults().with("impl.engine", true));
    r.add(&from_fn("layouts", |b| {
        noop_before(b, &points::LOCALE_FIND);
        Ok(())
    }))
    .unwrap();
    r.add(&feature::leaves::EngineLeaves).unwrap();
    let plan = r.build(&Base).unwrap();
    let chain = &plan.chains[&points::LOCALE_FIND.base];
    assert_eq!(chain.before[0].feature, "layouts");
    assert_eq!(chain.implement.as_ref().unwrap().feature, "engine-leaves");
    assert!(chain.shadow.is_none(), "nothing after: no shadow");
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("LOCALE_FIND") && w.contains("hot"))
    );
    assert_eq!(plan.chains.len(), engine::abi::OVERRIDES.len());
}

fn leaves(switches: &str) -> Vec<&'static str> {
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    r.add(&feature::leaves::EngineLeaves).unwrap();
    let plan = r.build(&Base).unwrap();
    plan.chains
        .values()
        .map(|c| c.implement.as_ref().unwrap().label)
        .collect()
}

#[test]
fn only_the_locale_leaves_are_on_by_default() {
    assert_eq!(
        leaves(""),
        ["locale_string_id_find", "locale_string_id_exists"]
    );
    assert!(leaves("impl.engine.locale=off").is_empty());
    assert!(leaves("impl.engine=off").is_empty());
    assert_eq!(leaves("impl.engine=on").len(), engine::abi::OVERRIDES.len());
    let one = leaves("impl.engine.locale=off,impl.engine.tags.tag_get_checked=on");
    assert_eq!(one, ["tag_get_checked"]);
    assert_eq!(
        leaves("impl.engine.datums=on").len(),
        18 + 2,
        "datums and the default locale"
    );
}

fn strings(b: &mut feature::Builder) -> Result<(), BuildError> {
    b.locale().source("ours", |id| (id == 7).then_some(0x1000));
    Ok(())
}

/// With the Rust lookup on (the default) our strings need no hook: the
/// implementation serves them. Off, one bridge hook does.
#[test]
fn string_sources_are_served_by_the_rust_lookup_or_one_bridge_hook() {
    for (switches, native) in [("", true), ("impl.engine.locale=off", false)] {
        let mut r = Registry::new(Switches::parse(switches).unwrap());
        r.add(&from_fn("ui-strings", strings)).unwrap();
        r.add(&feature::leaves::EngineLeaves).unwrap();
        let plan = r.build(&Base).unwrap();
        let chain = &plan.chains[&points::LOCALE_FIND.base];
        if native {
            assert!(chain.before.is_empty());
            assert_eq!(chain.implement.as_ref().unwrap().feature, "engine-leaves");
            assert_eq!(
                plan.locale,
                ["locale source ui-strings:ours (served by engine-leaves)"]
            );
        } else {
            assert_eq!(chain.before.len(), 1);
            assert_eq!(
                (chain.before[0].feature, chain.before[0].label),
                ("locale", "sources")
            );
            assert!(chain.implement.is_none());
            assert_eq!(
                plan.locale,
                ["locale source ui-strings:ours (served by the locale bridge)"]
            );
        }
    }
}

/// The order features are added in does not matter, and no sources means
/// no bridge.
#[test]
fn the_bridge_is_decided_after_every_feature_declared() {
    let mut r = Registry::new(Switches::parse("impl.engine=off").unwrap());
    r.add(&feature::leaves::EngineLeaves).unwrap();
    r.add(&from_fn("ui-strings", strings)).unwrap();
    assert_eq!(r.build(&Base).unwrap().chains.len(), 1);
    let mut r = Registry::new(Switches::parse("impl.engine=off").unwrap());
    r.add(&feature::leaves::EngineLeaves).unwrap();
    assert!(r.build(&Base).unwrap().chains.is_empty());
}

#[test]
fn two_implementations_are_refused_naming_both() {
    let mut r = Registry::new(Switches::defaults().with("impl.engine", true));
    r.add(&feature::leaves::EngineLeaves).unwrap();
    r.add(&from_fn("my-locale", |b| {
        b.raw()
            .implement(&points::LOCALE_FIND, "mine", |_, _, _| unreachable!());
        Ok(())
    }))
    .unwrap();
    let Err(c) = r.build(&Base) else {
        panic!("built")
    };
    assert_eq!(
        c,
        [Conflict::TwoImplementations {
            point: "LOCALE_FIND",
            addr: 0x001a_02d0,
            a: "engine-leaves",
            b: "my-locale"
        }]
    );
    assert!(c[0].to_string().contains("engine-leaves and my-locale"));
}

#[test]
fn unordered_mutating_hooks_are_refused_and_an_order_resolves_them() {
    let build = |order_b: bool| {
        let mut r = Registry::new(Switches::defaults());
        r.add(&from_fn("a", |b| {
            noop_before(b, &points::GAME_TICK);
            Ok(())
        }))
        .unwrap();
        r.add(&from_fn("b", move |b| {
            let h = b
                .raw()
                .before(&points::GAME_TICK, "b", |_| Ok(Flow::Continue));
            if order_b {
                h.after_feature("a");
            }
            Ok(())
        }))
        .unwrap();
        r.build(&Base)
    };
    let Err(c) = build(false) else {
        panic!("built")
    };
    assert!(matches!(
        c[..],
        [Conflict::Unordered {
            kind: Kind::Before,
            a: "a",
            b: "b",
            ..
        }]
    ));
    let plan = build(true).unwrap();
    let ids: Vec<_> = plan.chains[&points::GAME_TICK.base]
        .before
        .iter()
        .map(|h| h.feature)
        .collect();
    assert_eq!(ids, ["a", "b"]);
}

#[test]
fn order_classes_and_after_feature_sort_the_chain() {
    let mut r = Registry::new(Switches::defaults());
    for (id, first, after) in [
        ("z", true, None),
        ("m", false, Some("y")),
        ("y", false, None),
    ] {
        r.add(&from_fn(id, move |b| {
            let h = b.raw().observe_after(&points::GAME_TICK, "o", |_| {});
            let h = if first { h.first() } else { h };
            if let Some(a) = after {
                h.after_feature(a);
            }
            Ok(())
        }))
        .unwrap();
    }
    let plan = r.build(&Base).unwrap();
    let ids: Vec<_> = plan.chains[&points::GAME_TICK.base]
        .observe_after
        .iter()
        .map(|h| h.feature)
        .collect();
    assert_eq!(ids, ["z", "y", "m"]);
}

#[test]
fn a_cycle_is_refused() {
    let mut r = Registry::new(Switches::defaults());
    for (id, after) in [("a", "b"), ("b", "a")] {
        r.add(&from_fn(id, move |b| {
            b.raw()
                .observe_after(&points::GAME_TICK, "o", |_| {})
                .after_feature(after);
            Ok(())
        }))
        .unwrap();
    }
    let Err(c) = r.build(&Base) else {
        panic!("built")
    };
    assert!(matches!(&c[..], [Conflict::Cycle { features, .. }] if features.len() == 2));
}

#[test]
fn a_before_on_an_opaque_point_is_refused() {
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("x", |b| {
        noop_before(b, &points::PLAYER_KILLED);
        Ok(())
    }))
    .unwrap();
    assert_eq!(
        r.build(&Base).err().unwrap(),
        [Conflict::BeforeOnOpaque {
            point: "PLAYER_KILLED",
            feature: "x"
        }]
    );
}

static ALIAS: Point = Point::new(
    "TICK_AGAIN",
    0x0013_7fe0,
    Sig::Opaque,
    "docs/feature-engine.md",
);

#[test]
fn one_address_with_two_names_is_refused() {
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("x", |b| {
        b.frame().on_tick("t", |_| {});
        b.raw().observe_after(&ALIAS, "t", |_| {});
        Ok(())
    }))
    .unwrap();
    assert!(matches!(
        r.build(&Base).err().unwrap()[..],
        [Conflict::TwoNames {
            a: "GAME_TICK",
            b: "TICK_AGAIN",
            ..
        }]
    ));
}

#[test]
fn duplicate_ids_are_refused() {
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("x", |_| Ok(()))).unwrap();
    r.add(&from_fn("x", |_| Ok(()))).unwrap();
    assert_eq!(
        r.build(&Base).err().unwrap(),
        [Conflict::DuplicateFeature("x")]
    );
}

#[test]
fn switched_off_and_failed_features_declare_nothing() {
    let mut r = Registry::new(Switches::parse("-quiet").unwrap());
    assert_eq!(
        r.add(&from_fn("quiet", |b| {
            b.frame().on_tick("t", |_| {});
            Ok(())
        })),
        Ok(false)
    );
    assert!(
        r.add(&from_fn("broken", |b| {
            b.frame().on_tick("t", |_| {});
            Err(BuildError("no config".into()))
        }))
        .is_err()
    );
    let plan = r.build(&Base).unwrap();
    assert!(plan.chains.is_empty());
    assert!(!plan.is_on("quiet") && !plan.is_on("broken"));
    assert!(
        plan.text()
            .contains("feature broken: off (build: no config)")
    );
}

#[test]
fn a_missing_point_turns_its_feature_off_or_drops_an_optional_hook() {
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("telemetry", |b| {
        b.game().on_start("start", |_| {});
        b.raw().observe_before(&points::WEAPON_FIRE, "fire", |_| {});
        Ok(())
    }))
    .unwrap();
    r.add(&from_fn("needs-tick", |b| {
        b.frame().on_tick("t", |_| {});
        b.game().on_start("start", |_| {});
        Ok(())
    }))
    .unwrap();
    // This image has activation only.
    let plan = r
        .build(&Remap::new(&[(&points::GAME_ACTIVATE, 0x40_2000)]))
        .unwrap();
    assert!(plan.is_on("telemetry"));
    assert!(!plan.is_on("needs-tick"));
    let ids: Vec<_> = plan.chains[&0x40_2000]
        .observe_after
        .iter()
        .map(|h| h.feature)
        .collect();
    assert_eq!(ids, ["telemetry"]);
    assert!(
        plan.notes
            .iter()
            .any(|n| n.contains("fire dropped: WEAPON_FIRE"))
    );
    assert!(
        plan.notes
            .iter()
            .any(|n| n.contains("needs-tick: off: GAME_TICK"))
    );
}

#[test]
fn guest_alloc_hands_out_arena_pages() {
    let mut r = Registry::new(Switches::defaults());
    let got = std::sync::Mutex::new(Vec::new());
    r.add(&from_fn("a", |b| {
        got.lock()
            .unwrap()
            .push(b.guest_alloc(0x10, runtime::guest::Permissions::RW)?);
        got.lock()
            .unwrap()
            .push(b.guest_alloc(0x2000, runtime::guest::Permissions::RW)?);
        Ok(())
    }))
    .unwrap();
    let plan = r.build(&Base).unwrap();
    let got = got.into_inner().unwrap();
    assert_eq!(
        got,
        [
            feature::arena::ARENA_START,
            feature::arena::ARENA_START + 0x1000
        ]
    );
    assert_eq!(plan.allocs.len(), 2);
}
