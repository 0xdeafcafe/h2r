use super::*;
use guest_layout::entry::{EntryFuture, Regs, testing};
use guest_layout::testing::FakeGuest;

#[test]
fn unsupported_and_offline_runs_add_no_join_hooks_or_dialog_allocations() {
    let enabled = Hle::with_address(
        live_hle::LiveConfig {
            enabled: true,
            server_url: Some("https://live.example".into()),
            ..Default::default()
        },
        "127.0.0.1:3074".parse().unwrap(),
    )
    .unwrap();
    for (live, unsupported) in [
        (Some(enabled), true),
        (None, false),
        (Some(signed_out()), false),
    ] {
        let mut features = hooks::Registry::new(hooks::Switches::defaults());
        install(&mut features, live, unsupported, None).unwrap();
        let plan = features.build(&hooks::Base).unwrap();
        assert!(plan.features.is_empty());
        assert!(plan.chains.is_empty() && plan.entries.is_empty() && plan.allocs.is_empty());
    }
}

#[test]
fn only_an_explicit_launch_skips_confirmation_at_an_empty_menu() {
    let squad = Squad { addr: SQUAD, incarnation: 9 };
    let lobby = Place::Lobby(squad);
    let game = Place::Game(Some(squad));
    assert_eq!(
        join_action(Place::Menu, Source::External, None),
        ReadyAction::Ask
    );
    assert_eq!(
        join_action(Place::Menu, Source::CommandLine, None),
        ReadyAction::Join
    );
    for source in [Source::CommandLine, Source::External] {
        for place in [lobby, game] {
            assert_eq!(join_action(place, source, None), ReadyAction::Ask);
            assert_eq!(join_action(place, source, Some(place)), ReadyAction::Leave);
        }
        assert_eq!(join_action(game, source, Some(lobby)), ReadyAction::Ask);
        assert_eq!(
            join_action(Place::Lobby(Squad { incarnation: 10, ..squad }), source, Some(lobby)),
            ReadyAction::Ask
        );
    }
}

fn job(source: Source) -> Job {
    Job::new(Request {
        id: 0,
        link: "h2recomp://join?server=https%3A%2F%2Flive.example&session=8000000000000001"
            .parse()
            .unwrap(),
        source,
    })
}

fn signed_out() -> Arc<Hle> {
    Hle::new(live_hle::LiveConfig {
        server_url: Some("https://live.example".into()),
        ..Default::default()
    })
    .unwrap()
}

const SQUAD: u32 = 0x3000;
fn memory(mode: u32, squad_state: u32) -> FakeGuest {
    let mut m = FakeGuest::new();
    m.put_u32(level_boot::GAME_GLOBALS, 0x1000)
        .put_u8(0x1000 + 0x1120, 1)
        .put_u32(0x1000 + 8, mode)
        .put_u8(points::NETWORK_READY.base, 1)
        .put_u32(points::SQUAD_SESSION.base, SQUAD)
        .map(SQUAD, coop_probe::layout::STATE + 4)
        .put_u32(SQUAD + coop_probe::layout::STATE, squad_state)
        .put_u32(SQUAD + bots::phantom::session::at::INCARNATION, 9);
    m
}

#[test]
fn confirming_a_lobby_departure_waits_for_teardown_then_resumes_sign_in() {
    use crate::list_class::Rows;
    let entry = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let (hle, dialog, outcome) = (signed_out(), Dialog::default(), Outcome::default());
            let tick =
                async |job: &mut Job| advance(&cx, 0, &hle, &dialog, job, &outcome).await.unwrap();
            let mut job = job(Source::External);
            let lobby = Place::Lobby(Squad { addr: SQUAD, incarnation: 9 });
            assert_eq!(cx.read(place), Some(lobby));
            job.step = Step::Confirm {
                place: lobby,
                for_sign_in: true,
            };
            dialog.prepare(true, "Join this game?");
            dialog.press(1);
            assert!(!tick(&mut job).await);
            assert!(matches!(job.step, Step::Confirm { .. }));
            dialog.closed();
            assert!(!tick(&mut job).await);
            assert_eq!(job.confirmed, Some(lobby));
            assert!(!tick(&mut job).await);
            assert!(matches!(job.step, Step::Returning));
            assert!(!tick(&mut job).await);
            assert!(matches!(job.step, Step::Returning));
            cx.write(|m| m.set_u32(SQUAD + coop_probe::layout::STATE, 0))?;
            assert!(!tick(&mut job).await);
            assert_eq!(job.confirmed, Some(Place::Menu));
            assert!(!tick(&mut job).await);
            assert!(matches!(job.step, Step::SignIn { opened: false }));
            Ok(Ret::new(0, 0))
        })
    };
    let run = testing::run(
        &entry,
        memory(level_boot::MODE_MAINMENU, 5),
        Regs::default(),
        |_, _| Regs::default(),
    );
    assert!(run.result.is_ok());
    assert_eq!(
        run.calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        [
            crate::host_lobby::LEAVE_SESSIONS,
            level_boot::REQUEST_MAINMENU
        ]
    );
}

#[test]
fn cancelling_or_rejecting_an_external_request_never_interrupts_the_match() {
    let entry = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let (hle, dialog, outcome) = (signed_out(), Dialog::default(), Outcome::default());
            let mut cancelled = job(Source::External);
            cancelled.step = Step::Confirm {
                place: Place::Game(None),
                for_sign_in: true,
            };
            dialog.prepare(true, "Join this game?");
            dialog.closed(); // B dismisses the question.
            assert!(
                advance(&cx, 0, &hle, &dialog, &mut cancelled, &outcome)
                    .await
                    .unwrap()
            );
            let mut wrong_server = job(Source::External);
            wrong_server.request.link =
                "h2recomp://join?server=https%3A%2F%2Fother.example&session=8000000000000001"
                    .parse()
                    .unwrap();
            let err = advance(&cx, 0, &hle, &dialog, &mut wrong_server, &outcome)
                .await
                .unwrap_err();
            wrong_server.fail(err);
            assert!(
                advance(&cx, 0, &hle, &dialog, &mut wrong_server, &outcome)
                    .await
                    .unwrap()
            );
            Ok(Ret::new(0, 0))
        })
    };
    let run = testing::run(
        &entry,
        memory(level_boot::MODE_MULTIPLAYER, 0),
        Regs::default(),
        |_, _| panic!("unconfirmed request called the game"),
    );
    assert!(run.result.is_ok());
    assert!(run.calls.is_empty());
}

#[test]
fn the_join_target_keeps_the_inviter_session_and_guest_acceptance_time() {
    let mut invite = [0; 156];
    let id = 17_u64.to_le_bytes();
    let session = 0x9342_u64.to_be_bytes();
    let time = 1234_u64.to_le_bytes();
    invite[..8].copy_from_slice(&id);
    invite[12..16].copy_from_slice(b"Echo");
    invite[28..32].copy_from_slice(&0x0800_0011u32.to_le_bytes());
    invite[40..48].copy_from_slice(&session);
    invite[100..108].copy_from_slice(&time);
    let target = selected(&invite);
    assert_eq!(target[..4], 2_u32.to_le_bytes());
    assert_eq!(target[4..12], id);
    assert_eq!(&target[16..20], b"Echo");
    assert_eq!(target[32..36], 0x11u32.to_le_bytes());
    assert_eq!(target[36..44], time);
    assert_eq!(target[44..52], session);
}

#[test]
fn cold_launch_finds_the_listed_main_menu_and_enters_its_live_action() {
    let mut m = FakeGuest::new();
    let channel = ui_watch::channel(5, ui_watch::SHARED).unwrap();
    let (root, scratch) = (0x4100, 0x5100);
    m.map(channel, 0x40).map(root, 0x800).map(scratch, 0x100);
    // A root that the channel lists without putting it in its active slot.
    m.put_u32(channel + 0x34, root)
        .put_u32(root + 0x70, ui_watch::MAIN_MENU);
    let entry = move |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let screen = cx.read(main_menu).expect("listed root menu");
            sign_in(&cx, screen, scratch).await?;
            Ok(Ret::new(0, 0))
        })
    };
    let result = testing::run(&entry, m, Regs::default(), |f, m| {
        assert_eq!(f.target, OPEN_LIVE);
        assert_eq!(f.args, [root, scratch]);
        let context = m.u32(scratch).unwrap();
        assert_eq!(m.u32(context + 4).unwrap(), 0);
        Regs::default()
    });
    assert!(result.result.is_ok());
    assert_eq!(result.calls.len(), 1);
}

#[test]
fn the_menu_selects_this_target_and_joins_with_the_resolved_controller() {
    let mut m = FakeGuest::new();
    let scratch = 0x7100;
    m.map(scratch, 0x400);
    m.put(scratch + 40, &[1; 8]);
    let entry = move |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            dispatch_record(&cx, scratch, 2, &Outcome::default())
                .await
                .map_err(EntryError::IsolatedCall)?;
            Ok(Ret::new(0, 0))
        })
    };
    let run = testing::run(&entry, m, Regs::default(), |f, guest| {
        match f.target {
            SELECT_TARGET => {
                assert_eq!(f.args, [scratch + SELECTION, 1]);
                let mut id = [0; 8];
                guest.read(f.args[0] + 44, &mut id).unwrap();
                assert_eq!(id, [1; 8]);
            }
            JOIN_SELECTED => assert_eq!(f.args, [2, 0, 0, 0]),
            _ => unreachable!(),
        }
        Regs::default()
    });
    assert!(run.result.is_ok());
    assert_eq!(
        run.calls.iter().map(|f| f.target).collect::<Vec<_>>(),
        [SELECT_TARGET, JOIN_SELECTED]
    );
}

/// Both run before UI_UPDATE; with no order between them the plan refused
/// and every boot with Live enabled stopped at the feature plan.
#[test]
fn the_join_link_and_lab_consent_plan_together_at_ui_update() {
    let live = Hle::new(live_hle::LiveConfig {
        enabled: true,
        server_url: Some("https://live.example".into()),
        ..Default::default()
    })
    .unwrap();
    let mut features = hooks::Registry::new(hooks::Switches::defaults());
    install(&mut features, Some(live), false, None).unwrap();
    crate::lab_takeover::install(&mut features, &agent::Agent::new(), true).unwrap();
    let plan = features.build(&hooks::Base).unwrap().text();
    assert!(
        plan.contains("process a Live join link") && plan.contains("open or expire a lab request"),
        "{plan}"
    );
}
