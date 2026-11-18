use super::*;

fn store() -> Arc<Store> {
    Arc::new(Store::defaults(&DEFS))
}

fn on(store: &Store, count: i64, difficulty: i64) {
    store.set(COUNT.key, Value::Int(count)).unwrap();
    store.set(DIFFICULTY.key, Value::Int(difficulty)).unwrap();
    store.set(ENABLED.key, Value::Bool(true)).unwrap();
}

#[test]
fn the_settings_are_kept_with_every_other_setting() {
    for d in DEFS {
        assert!(
            crate::setting_defs::ALL.iter().any(|a| a.key == d.key),
            "{} is not in setting_defs::ALL",
            d.key
        );
        assert!(d.key.starts_with("clankers."), "{}", d.key);
    }
    let s = store();
    assert_eq!(
        Want::read(&s),
        Want {
            count: 0,
            difficulty: 1
        },
        "off by default"
    );
    on(&s, 2, 3);
    assert_eq!(
        Want::read(&s),
        Want {
            count: 2,
            difficulty: 3
        }
    );
}

#[test]
fn mixed_is_normal_heroic_and_easy_by_port() {
    assert_eq!(skill(0, 1), Skill::Easy);
    assert_eq!(skill(3, 2), Skill::Legendary);
    assert_eq!(
        [1, 2, 3].map(|p| skill(MIXED, p)),
        [Skill::Normal, Skill::Heroic, Skill::Easy]
    );
}

#[test]
fn the_driver_keeps_that_many_bots_on_the_ports_no_one_else_asked_for() {
    let local = Local::default();
    // An H2_BOTS bot on port 1.
    let other = local
        .add(1, "Clanker 1", Box::new(bots::Brain::default()))
        .unwrap();
    let d = Driver::new(local.clone());
    let log = d.apply(Want {
        count: 5,
        difficulty: 2,
    });
    assert_eq!(d.ports(), vec![2, 3]);
    assert!(log.last().unwrap().contains("no free port"), "{log:?}");
    assert_eq!(local.list().len(), 3);

    d.apply(Want {
        count: 1,
        difficulty: 2,
    });
    assert_eq!(d.ports(), vec![2], "the last added goes first");
    d.apply(Want {
        count: 0,
        difficulty: 2,
    });
    assert_eq!(d.ports(), Vec::<u8>::new());
    let left: Vec<BotHandle> = local.list().iter().map(|b| b.handle).collect();
    assert_eq!(left, vec![other], "the driver takes off only its own");
}

#[test]
fn past_the_free_ports_the_driver_adds_phantoms() {
    let local = Local::default();
    let ph = bots::phantom::Phantoms::default();
    let d = Driver::with_phantoms(local.clone(), ph.clone());
    let log = d.apply(Want {
        count: 5,
        difficulty: MIXED,
    });
    assert_eq!(d.ports(), vec![1, 2, 3]);
    let l = ph.list();
    let tags: Vec<&str> = l.iter().map(|p| p.identity.gamertag.as_str()).collect();
    assert_eq!(tags, ["Clanker 4", "Clanker 5"]);
    assert!(
        l.iter()
            .all(|p| live_proto::bots::is_local_xuid(live_proto::Xuid(p.identity.xuid))),
        "host-made xuids, never a server account's"
    );
    assert!(
        log.iter().any(|l| l.contains("Clanker 4 (phantom) on")),
        "{log:?}"
    );
    d.apply(Want {
        count: 3,
        difficulty: MIXED,
    });
    assert!(
        ph.list().is_empty(),
        "the phantoms go first, last added first"
    );
    assert_eq!(d.ports(), vec![1, 2, 3]);
}

#[test]
fn a_difficulty_change_reaches_the_bots_already_there() {
    let d = Driver::new(Local::default());
    d.apply(Want {
        count: 2,
        difficulty: 0,
    });
    let skills = |d: &Driver| -> Vec<u8> {
        d.own
            .lock()
            .unwrap()
            .iter()
            .map(|b| b.1.load(Ordering::Relaxed))
            .collect()
    };
    assert_eq!(skills(&d), vec![0, 0]);
    d.apply(Want {
        count: 2,
        difficulty: MIXED,
    });
    assert_eq!(skills(&d), vec![1, 2], "Normal on port 1, Heroic on 2");
    assert_eq!(d.ports(), vec![1, 2], "the same bots");
}

#[test]
fn the_driver_follows_the_store() {
    let s = store();
    let local = Local::default();
    let d = drive(&s, Driver::new(local.clone()));
    assert!(local.is_empty(), "off by default");
    on(&s, 3, 1);
    assert_eq!(d.ports(), vec![1, 2, 3]);
    s.set(COUNT.key, Value::Int(1)).unwrap();
    assert_eq!(d.ports(), vec![1]);
    s.set(ENABLED.key, Value::Bool(false)).unwrap();
    assert!(local.is_empty());
}

#[test]
fn the_page_has_a_row_per_setting_and_its_steps_are_saved() {
    let s = store();
    let p = page(Arc::clone(&s));
    assert_eq!(p.count(), DEFS.len());
    assert!(p.slots() <= IDS.len, "the family holds every text");
    let line = p.sideways(0, true).unwrap();
    assert!(line.contains("clankers.enabled -> On"), "{line}");
    assert_eq!(s.get(ENABLED.key), Some(Value::Bool(true)));
    p.sideways(2, false).unwrap();
    assert_eq!(
        s.get(DIFFICULTY.key),
        Some(Value::Int(0)),
        "Normal, left: Easy"
    );
    p.sideways(1, false).unwrap();
    assert_eq!(
        s.get(COUNT.key),
        Some(Value::Int(15)),
        "1, left: wraps to 15"
    );
}

#[test]
fn only_a_custom_games_list_gets_the_row_once() {
    assert!(wants_row(&[0, 1, 2]));
    assert!(wants_row(&[0, 1, 10]), "beside the Halo 1 Options row");
    assert!(!wants_row(&[1, 2]), "no Change Map: not a custom game");
    assert!(!wants_row(&[0, 11]), "already there");
}

#[test]
fn the_row_goes_after_the_halo1_row_on_the_plan() {
    let mut r = feature::Registry::new(feature::Switches::defaults());
    r.add(&h1row::SetupRow {
        data_root: std::env::temp_dir(),
    })
    .unwrap();
    let page = Arc::new(page(store()));
    r.add(&KitTexts(Arc::clone(&page))).unwrap();
    r.add(&page.class()).unwrap();
    r.add(&SetupRow { page }).unwrap();
    let text = r.build(&feature::Base).unwrap().text();
    for point in ["SQUAD_SETTINGS_ROW_TEXT", "SQUAD_SETTINGS_ROW_CHOSEN"] {
        let line = text.lines().find(|l| l.contains(point)).unwrap();
        let (h1, ours) = (
            line.find("h1-options-row:").unwrap(),
            line.find("clankers-row:").unwrap(),
        );
        assert!(h1 < ours, "{line}");
    }
}
