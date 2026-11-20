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
            difficulty: 1,
            taunts: true,
            glitches: 0,
        },
        "off by default"
    );
    on(&s, 2, 3);
    assert_eq!(
        Want::read(&s),
        Want {
            count: 2,
            difficulty: 3,
            taunts: true,
            glitches: 0,
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
fn the_driver_seats_that_many_phantoms() {
    let ph = Phantoms::default();
    let d = Driver::new(ph.clone());
    let log = d.apply(Want {
        count: 5,
        difficulty: MIXED,
        taunts: true,
        glitches: 0,
    });
    let l = ph.list();
    let tags: Vec<&str> = l.iter().map(|p| p.identity.gamertag.as_str()).collect();
    assert_eq!(tags, ["Clanker 1", "Clanker 2", "Clanker 3", "Clanker 4", "Clanker 5"]);
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
        taunts: true,
        glitches: 0,
    });
    assert_eq!(ph.list().len(), 3, "the last added go first");
    d.apply(Want {
        count: 0,
        difficulty: MIXED,
        taunts: true,
        glitches: 0,
    });
    assert!(ph.list().is_empty());
}

#[test]
fn the_roster_follows_the_store_as_the_count_changes() {
    let s = store();
    let ph = Phantoms::default();
    let _driver = drive(&s, Driver::new(ph.clone()));
    for count in [1, 3, 7, 2] {
        on(&s, count, 1);
        assert_eq!(ph.list().len(), count as usize);
    }
    s.set(ENABLED.key, Value::Bool(false)).unwrap();
    assert!(ph.list().is_empty());
}

#[test]
fn a_difficulty_change_reaches_the_bots_already_there() {
    let d = Driver::new(Phantoms::default());
    d.apply(Want {
        count: 2,
        difficulty: 0,
        taunts: true,
        glitches: 0,
    });
    let skills = |d: &Driver| -> Vec<u8> {
        d.own
            .lock()
            .unwrap()
            .iter()
            .map(|b| b.skill.load(Ordering::Relaxed))
            .collect()
    };
    assert_eq!(skills(&d), vec![0, 0]);
    d.apply(Want {
        count: 2,
        difficulty: MIXED,
        taunts: true,
        glitches: 0,
    });
    assert_eq!(skills(&d), vec![1, 2], "Normal then Heroic in turn");
}

#[test]
fn taunts_off_reaches_the_bots_already_there() {
    let s = store();
    let ph = Phantoms::default();
    let d = drive(&s, Driver::new(ph.clone()));
    on(&s, 1, 1);
    assert!(d.taunts.load(Ordering::Relaxed), "on by default");
    s.set(TAUNTS.key, Value::Bool(false)).unwrap();
    assert!(!d.taunts.load(Ordering::Relaxed));
    assert_eq!(ph.list().len(), 1, "the same bot");
}

#[test]
fn glitches_are_off_by_default_and_reach_the_bots_already_there() {
    let s = store();
    assert_eq!(Want::read(&s).glitches, 0, "off by default");
    let ph = Phantoms::default();
    let d = drive(&s, Driver::new(ph.clone()));
    on(&s, 1, 1);
    assert_eq!(d.glitches.load(Ordering::Relaxed), 0);
    // Lots, then Some: the bot it has takes each at its next frame.
    s.set(GLITCHES.key, Value::Int(2)).unwrap();
    assert_eq!(d.glitches.load(Ordering::Relaxed), 2);
    assert_eq!(
        bots::glitches::Glitches::from_index(d.glitches.load(Ordering::Relaxed)),
        bots::glitches::Glitches::Lots
    );
    s.set(GLITCHES.key, Value::Int(1)).unwrap();
    assert_eq!(Want::read(&s).glitches, 1);
    assert_eq!(d.glitches.load(Ordering::Relaxed), 1);
    assert_eq!(ph.list().len(), 1, "the same bot");
    // The page has the row, last, with Off, Some and Lots.
    assert_eq!(DEFS.last().map(|d| d.key), Some("clankers.glitches"));
    assert_eq!(ROWS.len(), DEFS.len());
}

#[test]
fn the_driver_follows_the_store() {
    let s = store();
    let ph = Phantoms::default();
    let d = drive(&s, Driver::new(ph.clone()));
    assert!(ph.list().is_empty(), "off by default");
    assert!(!d.on());
    on(&s, 3, 1);
    assert!(d.on());
    assert_eq!(ph.list().len(), 3);
    s.set(COUNT.key, Value::Int(1)).unwrap();
    assert_eq!(ph.list().len(), 1);
    s.set(ENABLED.key, Value::Bool(false)).unwrap();
    assert!(!d.on());
    assert!(ph.list().is_empty());
}

// --- Server rosters (stage 6c) ---

use live_proto::Xuid;
use live_proto::bots::{
    Colors, Emblem, Personality, Roster, RosterBot, RosterRequest, SERVER_XUID_MARK,
};

fn roster(session: &str, n: u64, skill: f32) -> Roster {
    Roster {
        match_id: session.into(),
        bots: (1..=n)
            .map(|i| RosterBot {
                xuid: Xuid(SERVER_XUID_MARK | (0x100 * n + i)),
                gamertag: live_proto::Gamertag::new(&format!("Rusty Lugnut {i}")).unwrap(),
                persistent: false,
                emblem: Emblem {
                    foreground: 12,
                    background: 3,
                    colors: [1, 7, i as u8],
                },
                colors: Colors {
                    primary: 4,
                    secondary: 9,
                },
                rank: None,
                team: (i == 2).then_some(1),
                skill,
                personality: Personality {
                    seed: 1000 + i,
                    taunt: 0.1,
                    ..Personality::default()
                },
            })
            .collect(),
    }
}

#[test]
fn a_roster_becomes_phantom_identities_and_brains() {
    let seats = roster_seats(&roster("0123456789abcdef", 2, 0.67));
    assert_eq!(seats.len(), 2);
    let a = &seats[0];
    assert_eq!(a.identity.xuid, SERVER_XUID_MARK | 0x201);
    assert_eq!(a.identity.gamertag, "Rusty Lugnut 1");
    assert_eq!(a.team, None, "null: the host balances");
    assert_eq!(seats[1].team, Some(1), "the roster's team when it has one");
    assert_eq!(a.skill, Skill::Heroic);
    assert_eq!((a.seed, a.taunt), (1001, 0.1));
}

#[test]
fn a_rosters_skill_is_the_nearest_named_difficulty() {
    use live_proto::bots::{EASY, HEROIC, LEGENDARY, NORMAL};
    assert_eq!(
        [EASY, NORMAL, HEROIC, LEGENDARY].map(skill_of),
        Skill::ALL,
        "the four named skills"
    );
    assert_eq!(skill_of(0.1), Skill::Easy);
    assert_eq!(skill_of(0.45), Skill::Normal);
    assert_eq!(skill_of(0.62), Skill::Heroic);
    assert_eq!(skill_of(0.9), Skill::Legendary);
    assert_eq!(
        [0, 1, 2, 3].map(request_skill),
        [Some(EASY), Some(NORMAL), Some(HEROIC), Some(LEGENDARY)]
    );
    assert_eq!(request_skill(MIXED), None, "Mixed: the server picks");
}

type Done = Box<dyn FnOnce(Result<Roster, String>) + Send>;

#[derive(Default)]
struct FakeLive {
    session: Mutex<Option<String>>,
    playlist: Mutex<Option<String>>,
    asked: Mutex<Vec<(RosterRequest, Option<Done>)>>,
    players: Mutex<Vec<Vec<Xuid>>>,
    /// The session each `players` call named.
    player_sessions: Mutex<Vec<Option<String>>>,
    clanker: Mutex<Option<(String, live_proto::hopper::clankers::ClankerHopper)>>,
}

impl Rosters for FakeLive {
    fn session(&self) -> Option<String> {
        self.session.lock().unwrap().clone()
    }
    fn playlist(&self) -> Option<String> {
        self.playlist.lock().unwrap().clone()
    }
    fn ask(&self, request: RosterRequest, done: Done) -> Result<(), String> {
        self.asked.lock().unwrap().push((request, Some(done)));
        Ok(())
    }
    fn players(&self, session: Option<String>, bots: Vec<Xuid>) {
        self.player_sessions.lock().unwrap().push(session);
        self.players.lock().unwrap().push(bots);
    }
    fn clanker_match(&self) -> Option<(String, live_proto::hopper::clankers::ClankerHopper)> {
        self.clanker.lock().unwrap().clone()
    }
}

impl FakeLive {
    fn requests(&self) -> Vec<RosterRequest> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.0.clone())
            .collect()
    }
    /// Answers the `i`th request.
    fn answer(&self, i: usize, r: Result<Roster, String>) {
        let done = self.asked.lock().unwrap()[i].1.take().unwrap();
        done(r);
    }
}

fn rostered_driver(session: Option<&str>) -> (Arc<Store>, Arc<FakeLive>, Phantoms, Arc<Driver>) {
    let s = store();
    let live = Arc::new(FakeLive::default());
    *live.session.lock().unwrap() = session.map(str::to_owned);
    let ph = Phantoms::default();
    let d = drive(&s, Driver::new(ph.clone()).with_rosters(live.clone()));
    (s, live, ph, d)
}

#[test]
fn with_live_every_phantom_is_a_server_bot_account() {
    const SESSION: &str = "0123456789abcdef";
    let (s, live, ph, d) = rostered_driver(Some(SESSION));
    on(&s, 5, 2);
    assert_eq!(
        live.requests(),
        vec![RosterRequest {
            session_id: SESSION.into(),
            playlist: None,
            count: 5,
            skill: Some(live_proto::bots::HEROIC),
            teams: false,
            carry_from: None,
        }],
        "every bot at the page's difficulty"
    );
    // Until the answer, host-made phantoms; the game record names none.
    assert!(
        ph.list()
            .iter()
            .all(|p| live_proto::bots::is_local_xuid(Xuid(p.identity.xuid)))
    );
    let before = d.bots_in_force().expect("the bots, as bots");
    assert_eq!(before.session_id, None, "no roster yet: no session named");
    assert_eq!(before.machines.len(), 5);
    assert!(
        before.machines.iter().all(|(_, x)| x.0 == 0),
        "no xuid without a roster"
    );

    live.answer(0, Ok(roster(SESSION, 5, 0.67)));
    let x = |n: u64, i: u64| SERVER_XUID_MARK | (0x100 * n + i);
    let l = ph.list();
    let tags: Vec<&str> = l.iter().map(|p| p.identity.gamertag.as_str()).collect();
    assert_eq!(
        tags,
        ["Rusty Lugnut 1", "Rusty Lugnut 2", "Rusty Lugnut 3", "Rusty Lugnut 4", "Rusty Lugnut 5"]
    );
    let bots = d.bots_in_force().expect("the roster in force");
    assert_eq!(bots.session_id.as_deref(), Some(SESSION));
    assert_eq!(
        bots.machines,
        l.iter()
            .map(|p| (p.machine, Xuid(p.identity.xuid)))
            .collect::<Vec<_>>(),
        "each rostered phantom by its machine id"
    );
    assert!(
        bots.machines
            .iter()
            .all(|(_, x)| live_proto::bots::is_server_xuid(*x))
    );

    // The page's difficulty never changes a roster bot's skill; it asks
    // for a new roster, which replaces the old.
    s.set(DIFFICULTY.key, Value::Int(3)).unwrap();
    assert_eq!(live.requests().len(), 2);
    assert_eq!(live.requests()[1].skill, Some(live_proto::bots::LEGENDARY));
    assert!(
        ph.list()
            .iter()
            .all(|p| live_proto::bots::is_local_xuid(Xuid(p.identity.xuid))),
        "no roster while a new one is asked for"
    );
    s.set(COUNT.key, Value::Int(6)).unwrap();
    assert_eq!(live.requests().len(), 3, "a count change asks again");
    assert_eq!(live.requests()[2].count, 6);
    live.answer(1, Ok(roster(SESSION, 5, 1.0)));
    assert_eq!(ph.list().len(), 6, "an old answer is dropped");
    assert!(
        ph.list()
            .iter()
            .all(|p| live_proto::bots::is_local_xuid(Xuid(p.identity.xuid)))
    );
    live.answer(2, Ok(roster(SESSION, 6, 1.0)));
    let l = ph.list();
    assert_eq!(l.len(), 6);
    assert!(
        l.iter()
            .all(|p| p.identity.xuid >> 8 & 0xff == 6 && p.identity.gamertag.starts_with("Rusty")),
        "the new roster's bots: {l:?}"
    );
    assert_eq!(d.bots_in_force().unwrap().machines.len(), 6);

    // Fewer: the last added go.
    s.set(COUNT.key, Value::Int(2)).unwrap();
    assert_eq!(live.requests().len(), 4);
    assert_eq!(live.requests()[3].count, 2);
    live.answer(3, Ok(roster(SESSION, 2, 1.0)));
    let bots = d.bots_in_force().unwrap();
    assert_eq!(bots.machines.len(), 2);
}

#[test]
fn the_hosted_session_is_told_of_the_bot_accounts_in_the_game() {
    const SESSION: &str = "0123456789abcdef";
    let (s, live, ph, d) = rostered_driver(Some(SESSION));
    on(&s, 5, 2);
    d.report_players();
    assert!(
        live.players.lock().unwrap().is_empty(),
        "no roster in force: nothing said"
    );
    assert_eq!(d.session_bots(), vec![]);
    live.answer(0, Ok(roster(SESSION, 5, 0.67)));
    let x = |i: u64| Xuid(SERVER_XUID_MARK | (0x100 * 5 + i));
    // The phantoms' players are not made yet: none of them are the
    // session's to be told of.
    assert_eq!(d.session_bots(), vec![]);
    d.report_players();
    assert!(live.players.lock().unwrap().is_empty());
    // Once the game made their players, every joined phantom, and no
    // other (a removed one's player stays, but it is no bot of ours).
    let own = d.own.lock().unwrap();
    let all: Vec<PhantomHandle> = ph.list().iter().map(|p| p.handle).collect();
    assert_eq!(
        session_bots_of(&own, |h| all.contains(&h)),
        vec![x(1), x(2), x(3), x(4), x(5)]
    );
    assert_eq!(
        session_bots_of(&own, |h| h == all[1]),
        vec![x(1), x(3), x(4), x(5)]
    );
    drop(own);
    // Clankers off: nothing joined, so still nothing to say.
    s.set(ENABLED.key, Value::Bool(false)).unwrap();
    d.report_players();
    d.report_players();
    assert!(live.players.lock().unwrap().is_empty());
    assert!(live.player_sessions.lock().unwrap().is_empty());
}

#[test]
fn a_failed_roster_leaves_host_made_bots_that_no_record_names() {
    let (s, live, ph, d) = rostered_driver(Some("0123456789abcdef"));
    on(&s, 4, 1);
    live.answer(0, Err("refused".into()));
    assert_eq!(ph.list().len(), 4);
    assert_eq!(ph.list()[0].identity.gamertag, "Clanker 1");
    let bots = d.bots_in_force().expect("still bots");
    assert_eq!(bots.session_id, None);
    assert_eq!(
        bots.machines,
        vec![(ph.list()[0].machine, Xuid(0))],
        "a bot with no xuid"
    );
    // A roster for another session is not taken.
    s.set(COUNT.key, Value::Int(5)).unwrap();
    live.answer(1, Ok(roster("fedcba9876543210", 2, 0.33)));
    assert!(
        ph.list()
            .iter()
            .all(|p| p.identity.gamertag.starts_with("Clanker"))
    );
}

/// The game record knows a phantom by its machine id (live-telemetry),
/// with the prefix the phantoms are made with.
#[test]
fn the_record_knows_a_phantom_by_the_same_machine_prefix() {
    assert_eq!(
        bots::phantom::MACHINE_PREFIX,
        live_proto::bots::PHANTOM_MACHINE_PREFIX
    );
    let m = bots::phantom::machine_id(PhantomHandle(9));
    assert!(live_proto::bots::is_phantom_machine(m));
    assert!(!live_proto::bots::is_phantom_machine([
        0x00, 0x50, 0xf2, 0, 0, 9
    ]));
}

/// The run 20261119-020015: as the game started, Halo 2 deleted the
/// lobby's session and advertised another, and the driver asked for a new
/// roster: host-made bots in the gap, then new names for every bot,
/// mid-game. Now the bots keep who they are across the handoff, and the
/// server is asked to carry their roster to the new session.
#[test]
fn the_bots_keep_who_they_are_when_the_game_moves_to_a_new_session() {
    const LOBBY: &str = "8894d40b4db49acb";
    const GAME: &str = "83d9415d14bf4ddc";
    const NEXT: &str = "8000000000000001";
    let (s, live, ph, d) = rostered_driver(Some(LOBBY));
    on(&s, 5, 2);
    live.answer(0, Ok(roster(LOBBY, 5, 0.67)));
    let who = |d: &Driver| -> Vec<(Seat, u64)> {
        d.own
            .lock()
            .unwrap()
            .iter()
            .map(|o| (o.seat, o.xuid))
            .collect()
    };
    let before = who(&d);
    let phantoms = ph.list();
    assert_eq!(phantoms.len(), 5);
    assert!(before.iter().all(|(_, x)| *x != 0));

    // The lobby's session deleted, the next not advertised yet: nothing
    // changes, and a page change in the gap keeps the roster.
    *live.session.lock().unwrap() = None;
    assert!(d.refresh().is_empty());
    let log = d.apply(Want::read(&s));
    assert!(log.is_empty(), "{log:?}");
    assert_eq!(who(&d), before);
    assert_eq!(ph.list(), phantoms);

    // The game's session advertised: the server is asked to carry the
    // lobby's bots there, and they stay as they are meanwhile.
    *live.session.lock().unwrap() = Some(GAME.into());
    let log = d.refresh();
    assert_eq!(
        log,
        [format!(
            "the game moved from session {LOBBY} to {GAME}: asking Live to carry its bots there"
        )]
    );
    let asked = &live.requests()[1];
    assert_eq!(asked.session_id, GAME);
    assert_eq!(asked.carry_from.as_deref(), Some(LOBBY));
    assert_eq!(asked.count, 5);
    assert_eq!(who(&d), before, "no host-made bots in the gap");
    assert_eq!(ph.list(), phantoms, "the phantoms stay joined");
    assert!(d.refresh().is_empty(), "asked once");
    // Until the server answers, the roster is not the new session's.
    assert_eq!(d.bots_in_force().unwrap().session_id, None);
    assert!(d.session_bots().is_empty());

    // The carried roster: the same bots, now the game session's.
    live.answer(1, Ok(roster(GAME, 5, 0.67)));
    assert_eq!(who(&d), before);
    assert_eq!(ph.list(), phantoms);
    let bots = d.bots_in_force().unwrap();
    assert_eq!(bots.session_id.as_deref(), Some(GAME));
    assert_eq!(
        d.session_bots().len(),
        0,
        "no phantom's player is made yet"
    );

    // A carry the server refuses: a new roster is asked for.
    *live.session.lock().unwrap() = Some(NEXT.into());
    d.refresh();
    assert_eq!(live.requests()[2].carry_from.as_deref(), Some(GAME));
    live.answer(2, Err("status 400".into()));
    assert_eq!(live.requests().len(), 4);
    assert_eq!(live.requests()[3].session_id, NEXT);
    assert_eq!(live.requests()[3].carry_from, None);

    // A count change is a new roster, not a carry.
    live.answer(3, Ok(roster(NEXT, 5, 0.67)));
    s.set(COUNT.key, Value::Int(4)).unwrap();
    assert_eq!(live.requests()[4].carry_from, None);
}

#[test]
fn a_session_made_after_the_page_was_set_gets_its_roster() {
    let (s, live, ph, d) = rostered_driver(None);
    on(&s, 4, 0);
    assert!(live.requests().is_empty(), "no session: host-made, no ask");
    assert_eq!(ph.list().len(), 4);
    // The h2 bots test run of 2026-11-18 never signed the game in to Live
    // and said nothing about a roster: the log now says why none.
    let log = d.apply(Want::read(&s));
    assert!(
        log.iter().any(|l| l
            == "no roster yet: this console hosts no Xbox Live session (host-made bots until it does)"),
        "{log:?}"
    );
    assert!(live.requests().is_empty());
    assert!(d.refresh().is_empty());
    *live.session.lock().unwrap() = Some("00000000000000aa".into());
    d.refresh();
    assert_eq!(live.requests().len(), 1);
    assert_eq!(live.requests()[0].session_id, "00000000000000aa");
    assert_eq!(live.requests()[0].count, 4, "every bot a phantom");
    assert!(d.refresh().is_empty(), "asked once");
    live.answer(0, Ok(roster("00000000000000aa", 4, 0.0)));
    assert_eq!(ph.list()[0].identity.gamertag, "Rusty Lugnut 1");
    // Without Live, nothing to refresh.
    let plain = Driver::new(Phantoms::default());
    assert!(plain.refresh().is_empty());
    assert_eq!(plain.bots_in_force(), None);
}

#[test]
fn the_page_has_a_row_per_setting_and_its_steps_are_saved() {
    let s = store();
    let p = page(Arc::clone(&s));
    assert_eq!(p.count(), DEFS.len());
    assert!(p.slots() <= IDS.len, "the family holds every text");
    // The Game Setup row says "Clanker Options..."; the page is "Clankers".
    let row = p.row_label_id();
    assert_ne!(row, p.title());
    assert!(IDS.owns(row) && IDS.owns(p.title()));
    assert_eq!(ROW_LABEL, "Clanker Options...");
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

/// A matchmade session's bots are its playlist's (docs/game/bots.md §9.4,
/// §9.5): the roster asks with the playlist, so the server fills it with
/// the persistent bot accounts nearest the humans' level.
#[test]
fn a_matchmade_session_asks_for_its_playlists_bots() {
    let (s, live, _ph, _d) = rostered_driver(Some("0123456789abcdef"));
    *live.playlist.lock().unwrap() = Some("3".into());
    on(&s, 2, 1);
    let r = live.requests();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].playlist.as_deref(), Some("3"));
    assert_eq!(r[0].count, 2);
}

// ---- Clankermatch (docs/game/bots.md §8.4) ----

/// A Clankermatch hosted by this console for `mark`, with Clankers off on
/// the page: the playlist decides.
fn clankermatch(
    session: &'static str,
    mark: live_proto::hopper::clankers::ClankerHopper,
) -> (Arc<FakeLive>, Phantoms, Arc<Driver>) {
    let (_s, live, ph, d) = rostered_driver(Some(session));
    *live.playlist.lock().unwrap() = Some(mark.hopper_id.to_string());
    *live.clanker.lock().unwrap() = Some((session.into(), mark));
    (live, ph, d)
}

fn hopper(hopper_id: u32, players: u32, teams: u32) -> live_proto::hopper::clankers::ClankerHopper {
    live_proto::hopper::clankers::ClankerHopper {
        hopper_id,
        playlist_id: hopper_id + 1,
        players,
        teams,
        ranks_as: None,
    }
}

/// The fill marks its phantoms the match's alone while it runs, so they
/// never join the party's lobby, and clears it once they are gone.
#[test]
fn a_clankermatchs_phantoms_are_the_matchs_alone() {
    use std::time::{Duration, Instant};
    const SESSION: &str = "00000000000000c5";
    let (live, ph, d) = clankermatch(SESSION, hopper(8, 8, 0));
    assert!(!ph.match_only());
    let t0 = Instant::now();
    d.tick_match(t0);
    assert!(ph.match_only(), "set before the first is seated");
    live.answer(0, Ok(roster(SESSION, 7, 0.5)));
    d.tick_match(t0 + Duration::from_secs(60));
    assert_eq!(ph.list().len(), 7);
    assert!(ph.match_only());
    *live.clanker.lock().unwrap() = None;
    *live.session.lock().unwrap() = None;
    d.tick_match(t0 + Duration::from_secs(61));
    d.tick_match(t0 + Duration::from_secs(61) + FILL_GONE + Duration::from_secs(2));
    assert!(ph.list().is_empty());
    assert!(!ph.match_only(), "cleared once they are gone");
}

#[test]
fn a_clankermatch_lobby_fills_phantoms_on_balanced_teams() {
    use std::time::{Duration, Instant};
    const SESSION: &str = "00000000000000c1";
    // A 4 v 4 clanker hopper hosted by this console (descriptor 9).
    let (live, ph, d) = clankermatch(SESSION, hopper(9, 8, 2));
    let t0 = Instant::now();
    let log = d.tick_match(t0);
    assert!(log.iter().any(|l| l.contains("7 bots join")), "{log:?}");
    // The run says the bots join the lobby as peers, in the match only.
    assert!(
        log.iter().any(|l| l.contains("7 phantoms join the lobby as peers")),
        "{log:?}"
    );
    assert!(d.on(), "the nav mesh is wanted");
    // One roster for the whole lobby, for the playlist, its skill the
    // server's (from the human's level), with teams.
    assert_eq!(
        live.requests(),
        vec![RosterRequest {
            session_id: SESSION.into(),
            playlist: Some("9".into()),
            count: 7,
            skill: None,
            teams: true,
            carry_from: None,
        }]
    );
    // Nobody seated before the roster: no host-made bot for it to replace.
    d.tick_match(t0 + Duration::from_secs(60));
    assert!(d.seats().is_empty() && ph.list().is_empty());
    let (live, ph, d) = clankermatch(SESSION, hopper(9, 8, 2));
    d.tick_match(t0);
    live.answer(0, Ok(roster(SESSION, 7, 0.5)));
    assert!(d.seats().is_empty(), "the first bot's time has not come");
    let schedule =
        crate::clankermatch::schedule(7, u64::from_str_radix(SESSION, 16).unwrap() ^ 0xc1a4);
    for (i, at) in schedule.iter().enumerate() {
        if i > 0 {
            d.tick_match(t0 + *at - Duration::from_millis(1));
            assert_eq!(d.seats().len(), i, "bot {i} not before its time");
        }
        d.tick_match(t0 + *at);
        assert_eq!(d.seats().len(), i + 1, "bot {i} at {at:?}");
        assert_eq!(ph.list().len(), i + 1, "bot {i}");
    }
    assert_eq!(live.requests().len(), 1, "asked once, not per bot");
    // The phantoms are the roster's seven, in order.
    let x = |i: u64| SERVER_XUID_MARK | (0x100 * 7 + i);
    let phantoms: Vec<u64> = ph.list().iter().map(|p| p.identity.xuid).collect();
    assert_eq!(phantoms, (1..=7).map(x).collect::<Vec<_>>());
    // Around the human on red (0): three join it, four go blue, 4 v 4.
    let teams: Vec<Option<u8>> = ph.list().iter().map(|p| p.team).collect();
    assert_eq!(
        teams.iter().filter(|t| **t == Some(0)).count(),
        3,
        "{teams:?}"
    );
    assert_eq!(
        teams.iter().filter(|t| **t == Some(1)).count(),
        4,
        "{teams:?}"
    );
    // The game hands the lobby to a new session: no session for a few
    // seconds keeps the bots.
    *live.clanker.lock().unwrap() = None;
    *live.session.lock().unwrap() = None;
    let end = t0 + *schedule.last().unwrap();
    d.tick_match(end + Duration::from_secs(1));
    assert_eq!(d.seats().len(), 7, "a handoff's gap keeps them");
    // Left for good: they go.
    d.tick_match(end + FILL_GONE + Duration::from_secs(2));
    assert!(ph.list().is_empty());
    assert!(d.seats().is_empty());
    assert!(!d.on());
}

#[test]
fn a_clankermatch_balances_around_the_hosts_real_team() {
    use std::time::Instant;
    const SESSION: &str = "00000000000000c3";
    let (live, ph, d) = clankermatch(SESSION, hopper(9, 8, 2));
    // The host's player is on blue (team 1), as the game says.
    d.set_host_team(Some(1));
    let t0 = Instant::now();
    let log = d.tick_match(t0);
    assert!(
        log.iter().any(|l| l.contains("the host on team 1")),
        "{log:?}"
    );
    live.answer(0, Ok(roster(SESSION, 7, 0.5)));
    // A change of team after the first bot is in moves nobody.
    let schedule =
        crate::clankermatch::schedule(7, u64::from_str_radix(SESSION, 16).unwrap() ^ 0xc1a4);
    d.tick_match(t0 + schedule[0]);
    assert_eq!(d.seats().len(), 1);
    d.set_host_team(Some(0));
    d.tick_match(t0 + *schedule.last().unwrap());
    assert_eq!(d.seats().len(), 7);
    // The human on blue: three phantoms with it, four on red, 4 v 4.
    let teams: Vec<Option<u8>> = ph.list().iter().map(|p| p.team).collect();
    assert_eq!(
        teams.iter().filter(|t| **t == Some(1)).count(),
        3,
        "{teams:?}"
    );
    assert_eq!(
        teams.iter().filter(|t| **t == Some(0)).count(),
        4,
        "{teams:?}"
    );
}

#[test]
fn a_clankermatch_of_two_v_two_is_three_phantoms() {
    use std::time::{Duration, Instant};
    const SESSION: &str = "00000000000000c4";
    let (live, ph, d) = clankermatch(SESSION, hopper(10, 4, 2));
    let t0 = Instant::now();
    d.tick_match(t0);
    live.answer(0, Ok(roster(SESSION, 3, 0.5)));
    d.tick_match(t0 + Duration::from_secs(60));
    // One phantom with the human (red, 0), two on blue.
    let teams: Vec<Option<u8>> = ph.list().iter().map(|p| p.team).collect();
    assert_eq!(
        teams.iter().filter(|t| **t == Some(0)).count(),
        1,
        "{teams:?}"
    );
    assert_eq!(
        teams.iter().filter(|t| **t == Some(1)).count(),
        2,
        "{teams:?}"
    );
}

#[test]
fn a_clankermatch_refused_a_roster_seats_host_made_bots() {
    use std::time::{Duration, Instant};
    const SESSION: &str = "00000000000000c2";
    let (live, ph, d) = clankermatch(SESSION, hopper(8, 8, 0));
    let t0 = Instant::now();
    d.tick_match(t0);
    d.tick_match(t0 + Duration::from_secs(60));
    assert!(d.seats().is_empty(), "waiting for the roster");
    live.answer(0, Err("refused".into()));
    // Free for all of 8: seven host-made phantoms.
    let l = ph.list();
    assert_eq!(l.len(), 7);
    assert!(
        l.iter()
            .all(|p| live_proto::bots::is_local_xuid(Xuid(p.identity.xuid)) && p.team.is_none())
    );
    let names: Vec<&str> = l.iter().map(|p| p.identity.gamertag.as_str()).collect();
    assert_eq!(
        names,
        [
            "Clanker 1",
            "Clanker 2",
            "Clanker 3",
            "Clanker 4",
            "Clanker 5",
            "Clanker 6",
            "Clanker 7"
        ]
    );
}

/// A on a row opens the value picker on it (the On/Off row too, as
/// retail's two-value settings do), and the Game Setup row has its own
/// description in the page's family.
#[test]
fn a_opens_the_value_picker_and_the_row_has_a_description() {
    use crate::ui::Chosen;
    use crate::ui_kit::picker;
    let s = store();
    let p = page(Arc::clone(&s));
    assert_eq!(p.press(2), Chosen::Open(picker::NAME));
    let pick = picker::current().expect("a pick");
    assert_eq!(pick.title, "DIFFICULTY");
    assert_eq!(pick.values.len(), 5, "the four skills and Mixed");
    assert_eq!(
        pick.opener,
        crate::ui_kit::view::Opener::GAME_SETUP_CLANKERS
    );
    assert_eq!(p.press(0), Chosen::Open(picker::NAME));
    assert_eq!(picker::current().unwrap().values, ["Off", "On"]);
    assert_eq!(p.press(7), Chosen::Open(picker::NAME));
    let pick = picker::current().expect("a pick");
    assert_eq!(pick.title, "GLITCHES");
    assert_eq!(pick.values, ["Off", "Some", "Lots"]);
    assert_eq!(
        s.get(ENABLED.key),
        Some(Value::Bool(false)),
        "opening changes nothing"
    );
    let help = p.row_help_id().expect("a description");
    assert!(IDS.owns(help));
    assert_ne!(help, p.row_label_id());
    assert!(p.slots() <= IDS.len);
    assert!(ROW_HELP.len() < 120, "a few lines under Game Setup's list");
}

/// The page has Quick Options' header: its line in the help line whatever
/// the focus (not the focused row's description, which the value picker
/// shows), a Spartan in the game type picture, both in the spec the
/// preview draws.
#[test]
fn the_page_has_quick_options_header() {
    use crate::list_class::Rows;
    use crate::ui_kit::dialog::{QUICK_HELP, QUICK_PICTURE, icon, quick_icon};
    use crate::ui_kit::spec::Describe;
    let p = page(store());
    let header = Rows::description(&p, 0).expect("the help line");
    assert_eq!(
        Rows::description(&p, 5),
        Some(header),
        "the same whatever the focus"
    );
    assert!(IDS.owns(header));
    assert_ne!(Some(header), p.row_help_id());
    assert_eq!(Rows::description(&p, 99), None, "no such row");
    assert_eq!(Rows::picture_image(&p), Some(icon::SPARTAN));
    assert!(p.slots() <= IDS.len);
    let s = p.spec();
    assert_eq!(s.help.as_deref(), Some(HEADER_HELP));
    assert_eq!(s.description_text().as_deref(), Some(HEADER_HELP));
    assert_eq!(s.art, [quick_icon(icon::SPARTAN)]);
    assert_eq!(s.art[0].at, QUICK_PICTURE.at);
    // The rows keep their own descriptions, for the picker.
    assert!(s.rows.iter().all(|r| r.description.is_some()));
    // One line on the kit's own measure in Quick Options' help line.
    let room = i32::from(QUICK_HELP.right) - i32::from(QUICK_HELP.left);
    assert!(crate::ui_kit::spec::fallback::width(HEADER_HELP) <= room);
}

#[test]
fn a_rostered_phantom_uses_the_server_seed_and_taunt() {
    let driver = Driver::new(Phantoms::default());
    let (source, _) = driver.clanker(Skill::Heroic, 99, 0.4);
    assert_eq!(source.brain_view(), Some(bots::Brain::new(Skill::Heroic, 99).view()));
    assert_eq!(source.taunt, 0.4);
}
