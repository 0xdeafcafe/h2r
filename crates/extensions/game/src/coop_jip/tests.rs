use super::*;
use guest_layout::entry::Regs;
use guest_layout::entry::testing::run;
use guest_layout::memory::GuestMemory;
use guest_layout::testing::FakeGuest;

const SESSION: u32 = 0x0010_0000;
const REQUEST: u32 = 0x0020_0000;

const ON: Stages = Stages {
    lift: true,
    log: true,
};

/// A co-op campaign this console hosts, in state `state`.
fn hosted(state: u32, players: u32) -> Session {
    Session {
        addr: SESSION,
        state,
        peers: players,
        players,
        local_peer: 0,
        host_peer: 0,
        difficulty: 1,
        ..Session::default()
    }
}

#[test]
fn the_stages_are_on_unless_turned_off() {
    assert_eq!(Stages::parse(|_| None), ON);
    let off = |k: &str| (k == "H2_COOP_JIP_LIFT").then(|| "0".to_string());
    assert_eq!(
        Stages::parse(off),
        Stages {
            lift: false,
            log: true
        }
    );
    let off = |k: &str| (k == "H2_COOP_JIP_LOG").then(|| " Off ".to_string());
    assert!(!Stages::parse(off).log);
    let other = |_: &str| Some("1".to_string());
    assert_eq!(Stages::parse(other), ON);
}

#[test]
fn only_the_host_of_a_running_co_op_campaign_reads_as_the_lobby() {
    for state in RUNNING {
        assert_eq!(
            lift(&hosted(state, 2), true, ON, 3),
            Lift::AsLobby { real: state }
        );
    }
    let retail = |s: &Session, on, stages, limit| match lift(s, on, stages, limit) {
        Lift::Retail(why) => why,
        l => panic!("{l:?}"),
    };
    // In the lobby retail already takes it.
    assert_eq!(retail(&hosted(LOBBY, 2), true, ON, 3), "not a running game");
    assert_eq!(retail(&hosted(4, 2), true, ON, 3), "not a running game");
    assert_eq!(retail(&hosted(9, 2), true, ON, 3), "not a running game");
    assert_eq!(
        retail(&hosted(7, 2), false, ON, 3),
        "coop.join_in_progress is off"
    );
    let no_lift = Stages {
        lift: false,
        log: true,
    };
    assert_eq!(
        retail(&hosted(7, 2), true, no_lift, 3),
        "H2_COOP_JIP_LIFT is off"
    );
    let mut mp = hosted(7, 2);
    mp.difficulty = crate::coop_probe::NO_DIFFICULTY;
    assert_eq!(retail(&mp, true, ON, 3), "not a campaign session");
    let mut guest = hosted(7, 2);
    guest.local_peer = 1;
    assert_eq!(retail(&guest, true, ON, 3), "not the host");
    assert_eq!(retail(&hosted(7, 3), true, ON, 3), "the game is full");
}

/// Runs the lifted check on a session in state `real`; the stand-in for
/// retail's check records the state it saw and answers `code`.
fn check(real: u32, code: u32) -> (guest_layout::entry::testing::Run, u32) {
    let mut g = FakeGuest::new();
    g.put_u32(SESSION + layout::STATE, real);
    let seen = Arc::new(Mutex::new(0));
    let s = Arc::clone(&seen);
    let reentry = Arc::new(AtomicBool::new(false));
    let r2 = Arc::clone(&reentry);
    let f = move |cx: Cx| -> EntryFuture {
        Box::pin(check_as_lobby(cx, SESSION, REQUEST, real, Arc::clone(&r2)))
    };
    let r = run(&f, g, Regs::default(), move |c, m| {
        assert_eq!(c.target, points::SESSION_JOIN_CHECK.base);
        assert!(
            reentry.load(Ordering::SeqCst),
            "the hook lets its own call through"
        );
        *s.lock().unwrap() = m.u32(SESSION + layout::STATE).unwrap();
        Regs {
            eax: code,
            ..Regs::default()
        }
    });
    let seen = *seen.lock().unwrap();
    (r, seen)
}

#[test]
fn the_check_sees_the_lobby_and_the_state_goes_back() {
    let (r, seen) = check(7, 0);
    assert_eq!(seen, LOBBY);
    assert_eq!(r.result, Ok(Ret::new(0, 0)));
    assert_eq!(r.guest.u32(SESSION + layout::STATE).unwrap(), 7);
    assert_eq!(
        r.calls,
        [GuestCall::new(points::SESSION_JOIN_CHECK.base)
            .eax(SESSION)
            .edi(REQUEST)]
    );
    // Retail's other refusals stand.
    let (r, _) = check(8, 4);
    assert_eq!(r.result, Ok(Ret::new(4, 0)));
    assert_eq!(r.guest.u32(SESSION + layout::STATE).unwrap(), 8);
    assert_eq!(
        lift_line(8, 4),
        "coop-jip: host: a console asked to join the running game; still refused (4: no room for the console or its players)"
    );
    assert!(lift_line(7, 0).starts_with("coop-jip: host: a console joins the running game"));
}

#[test]
fn the_host_says_when_it_sends_the_state_and_when_the_player_arrives() {
    let mut t = Track::default();
    // A player-set change with no join under way is the game's own.
    assert_eq!(
        t.applied(0, true, 10, 0b11, Some(0b111)),
        Vec::<String>::new()
    );
    assert_eq!(
        t.sending(1_000, 0),
        "coop-jip: host: sending the game state to a joining console (attempt 1)"
    );
    assert!(t.sent(true).contains("queued"));
    // Updates without a change: nothing.
    assert!(t.applied(1_100, true, 11, 0b11, None).is_empty());
    assert_eq!(
        t.applied(1_250, true, 12, 0b11, Some(0b111)),
        ["coop-jip: admitted player 3 at tick 12 (250 ms after the state went out)"]
    );
    // Once.
    assert!(t.applied(1_300, true, 13, 0b111, Some(0b111)).is_empty());
    // A failed queue forgets the send.
    t.sending(2_000, 1);
    assert!(t.sent(false).contains("without the joiner"));
    assert!(t.applied(2_100, true, 20, 0b11, Some(0b111)).is_empty());
}

#[test]
fn the_newcomer_says_it_joined_on_its_first_update_after_the_restore() {
    let mut t = Track::default();
    assert_eq!(
        t.join_message(500),
        "coop-jip: guest: join message; the host's game is at update 500"
    );
    assert_eq!(
        t.restored(true, 200 * 1024 + 1),
        "coop-jip: guest: game state restored (201 KiB compressed)"
    );
    // The first update carries the newcomer in: three players.
    let lines = t.applied(0, false, 501, 0b011, Some(0b111));
    assert_eq!(
        lines,
        ["coop-jip: joined in progress at tick 501 as player 3"]
    );
    // The verdict's wording (coop::join::joined_line).
    assert!(lines[0].starts_with("coop-jip: joined in progress at tick "));
    assert!(t.applied(0, false, 502, 0b111, None).is_empty());
    // Not a host: never an admission.
    assert!(t.applied(0, false, 503, 0b111, Some(0b1111)).is_empty());
}

#[test]
fn a_state_that_does_not_restore_sends_the_newcomer_back() {
    let mut t = Track::default();
    t.join_message(500);
    assert_eq!(
        t.restored(false, 9),
        "coop-jip: back to the lobby: the game state did not restore"
    );
    assert!(t.applied(0, false, 501, 0b11, None).is_empty());
}

#[test]
fn the_feature_is_on_the_plan_with_the_co_op_hooks() {
    let rt = runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    );
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let arm = crate::coop_probe::install_live(&rt, &mut r, std::path::Path::new("/nonexistent"))
        .unwrap()
        .unwrap();
    assert!(!arm.active(), "off until a co-op session arms it");
    let text = r.build(&hooks::Base).unwrap().text();
    assert!(text.contains(FEATURE), "{text}");
    for p in HOOKED {
        assert!(text.contains(p.name), "{} missing:\n{text}", p.name);
    }
    // H2_FEATURES=-coop-jip leaves the rest of co-op on.
    let mut r = hooks::Registry::new(hooks::Switches::parse("-coop-jip").unwrap());
    assert!(
        crate::coop_probe::install_live(&rt, &mut r, std::path::Path::new("/x"))
            .unwrap()
            .is_some()
    );
    let text = r.build(&hooks::Base).unwrap().text();
    assert!(!text.contains("join in progress:"), "{text}");
    assert!(text.contains("coop-players"), "{text}");
}

#[test]
fn without_the_lift_or_the_lines_their_hooks_are_not_built() {
    let rt = runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    );
    let _ = &rt;
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let off = Stages {
        lift: false,
        log: false,
    };
    r.add(&CoopJip::new(Arm::session(), off)).unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    assert!(!text.contains("join in progress:"), "{text}");
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let lift_only = Stages {
        lift: true,
        log: false,
    };
    r.add(&CoopJip::new(Arm::session(), lift_only)).unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    assert!(
        text.contains("the join check while the game runs"),
        "{text}"
    );
    assert!(!text.contains("host sends the state"), "{text}");
}

/// docs/runtime/snapshots.md: join in progress blocks a snapshot while a
/// co-op session has it armed, whatever stages are built.
#[test]
fn join_in_progress_blocks_snapshots_only_while_armed() {
    let off = Stages {
        lift: false,
        log: false,
    };
    for stages in [ON, off] {
        let arm = Arm::session();
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&CoopJip::new(Arc::clone(&arm), stages)).unwrap();
        let plan = r.build(&hooks::Base).unwrap();
        assert_eq!(plan.snapshot_blockers(), Vec::new());
        arm.arm(None);
        assert_eq!(
            plan.snapshot_blockers(),
            vec![(
                FEATURE,
                "a co-op session has armed the co-op hooks".to_string()
            )]
        );
        arm.disarm();
        assert_eq!(plan.snapshot_blockers(), Vec::new());
    }
}
