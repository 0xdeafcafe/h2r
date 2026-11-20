use super::*;
use engine::entry::{EntryFuture, Regs, testing};
use engine::testing::FakeGuest;

#[test]
fn a_game_only_leaves_after_consent_and_a_successful_new_lookup() {
    assert_eq!(after_lookup(Place::Game, false), ReadyAction::Ask);
    assert_eq!(after_lookup(Place::Game, true), ReadyAction::Leave);
    // The same check after the menu loads sends the player straight in.
    assert_eq!(after_lookup(Place::Menu, false), ReadyAction::Join);
    assert_eq!(after_lookup(Place::Menu, true), ReadyAction::Join);
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
