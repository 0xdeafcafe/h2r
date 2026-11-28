use super::*;
use guest_layout::datum::layout as dl;
use guest_layout::points;
use guest_layout::testing::FakeGuest;

const PLAYERS_HEADER: u32 = 0x0061_0000;
const PLAYERS_DATA: u32 = 0x0061_1000;

fn idle() -> Box<dyn PadSource> {
    Box::new(|_: &crate::pad_source::Frame<'_>| Default::default())
}

fn who(xuid: u64, tag: &str) -> Identity {
    Identity {
        xuid,
        gamertag: tag.into(),
    }
}

/// Player records with these machine ids; `None` is a free slot, and the
/// flag marks a player leaving.
fn players(ids: &[Option<([u8; 6], bool)>]) -> FakeGuest {
    let mut g = FakeGuest::new();
    let n = ids.len() as u32;
    g.put_u32(points::PLAYERS.base, PLAYERS_HEADER);
    g.map(PLAYERS_HEADER, 0x48)
        .put_u32(PLAYERS_HEADER + dl::ELEMENT_SIZE, 0x21c)
        .put_u32(PLAYERS_HEADER + dl::HIGH_WATER, n)
        .put_u32(PLAYERS_HEADER + dl::DATA, PLAYERS_DATA)
        .map(PLAYERS_DATA, 0x21c * n.max(1));
    for (i, slot) in (0u32..).zip(ids) {
        let Some((id, leaving)) = slot else { continue };
        let p = PLAYERS_DATA + i * 0x21c;
        g.put_u16(p, 0xe000 + i as u16)
            .put_u16(p + 2, if *leaving { 2 } else { 0 });
        for (k, b) in (0u32..).zip(id) {
            g.put_u8(p + crate::guest::PLAYER_MACHINE + k, *b);
        }
    }
    g
}

#[test]
fn machine_ids_are_ours_and_round_trip() {
    for h in [0, 1, 255, 256, 0x0102_0304, u32::MAX] {
        let id = machine_id(PhantomHandle(h));
        assert_eq!(id[..2], MACHINE_PREFIX);
        assert_eq!(handle_of(id), Some(PhantomHandle(h)));
    }
    // A real adapter address (unicast, the Xbox's OUI) is not ours.
    assert_eq!(handle_of([0x00, 0x50, 0xf2, 1, 2, 3]), None);
    assert_ne!(machine_id(PhantomHandle(1)), machine_id(PhantomHandle(2)));
}

#[test]
fn add_puts_a_phantom_on_the_books_pending() {
    let ph = Phantoms::default();
    let h = ph
        .add(who(0x9001, "Rusty Lugnut"), Some(1), idle())
        .unwrap();
    let l = ph.list();
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].handle, h);
    assert_eq!(l[0].identity.gamertag, "Rusty Lugnut");
    assert_eq!(l[0].team, Some(1));
    assert_eq!(l[0].machine, machine_id(h));
    assert_eq!(l[0].state, State::Pending);
    assert_eq!(l[0].player, None);
    assert_eq!(ph.player(h), None);
}

#[test]
fn add_refuses_bad_identities_duplicates_and_a_full_house() {
    let ph = Phantoms::default();
    assert_eq!(
        ph.add(who(0, "Zero"), None, idle()),
        Err(Error::BadIdentity)
    );
    assert_eq!(ph.add(who(5, "  "), None, idle()), Err(Error::BadIdentity));
    ph.add(who(5, "Five"), None, idle()).unwrap();
    assert_eq!(
        ph.add(who(5, "Again"), None, idle()),
        Err(Error::Duplicate(5))
    );
    for x in 6..6 + MAX_PHANTOMS as u64 - 1 {
        ph.add(who(x, "Bot"), None, idle()).unwrap();
    }
    assert_eq!(ph.add(who(99, "One more"), None, idle()), Err(Error::Full));
}

#[test]
fn handles_are_not_reused_after_remove() {
    let ph = Phantoms::default();
    let a = ph.add(who(1, "A"), None, idle()).unwrap();
    ph.remove(a).unwrap();
    assert_eq!(ph.remove(a), Err(Error::NoSuchPhantom(a)));
    let b = ph.add(who(1, "A"), None, idle()).unwrap();
    assert_ne!(a, b, "a new phantom never takes an old machine id");
    assert!(ph.list().iter().all(|p| p.handle == b));
}

#[test]
fn refresh_finds_players_by_machine_id_not_by_order() {
    let ph = Phantoms::default();
    let a = ph.add(who(1, "A"), None, idle()).unwrap();
    let b = ph.add(who(2, "B"), None, idle()).unwrap();
    // B's player was made first, and a human sits between them.
    let g = players(&[
        Some(([0, 0x50, 0xf2, 0, 0, 1], false)),
        Some((machine_id(b), false)),
        None,
        Some((machine_id(a), false)),
    ]);
    ph.refresh(&g).unwrap();
    assert_eq!(ph.player(a), Some(3));
    assert_eq!(ph.player(b), Some(1));
    assert!(ph.list().iter().all(|p| p.state == State::Joined));
}

#[test]
fn a_leaving_or_gone_player_sends_the_phantom_back_to_pending() {
    let ph = Phantoms::default();
    let a = ph.add(who(1, "A"), None, idle()).unwrap();
    ph.refresh(&players(&[Some((machine_id(a), false))]))
        .unwrap();
    assert_eq!(ph.player(a), Some(0));
    ph.refresh(&players(&[Some((machine_id(a), true))]))
        .unwrap();
    assert_eq!(ph.player(a), None);
    assert_eq!(ph.list()[0].state, State::Pending);
    ph.refresh(&players(&[None])).unwrap();
    assert_eq!(ph.list()[0].state, State::Pending);
}

#[test]
fn failed_stays_failed_with_its_reason() {
    let ph = Phantoms::default();
    let a = ph.add(who(1, "A"), None, idle()).unwrap();
    ph.fail(a, "not hosting a session");
    ph.refresh(&players(&[Some((machine_id(a), false))]))
        .unwrap();
    let l = ph.list();
    assert_eq!(l[0].state, State::Failed("not hosting a session".into()));
    assert_eq!(l[0].player, None);
}

#[test]
fn no_players_array_means_no_players() {
    let g = FakeGuest::new();
    let mut g = g;
    g.map(points::PLAYERS.base & !0xfff, 0x1000);
    assert_eq!(players_by_machine(&g).unwrap(), vec![]);
}

#[test]
fn the_free_functions_use_the_global_books() {
    let h = add(who(0x0123_4567_89ab, "Global"), None, idle()).unwrap();
    assert!(list().iter().any(|p| p.handle == h));
    remove(h).unwrap();
    assert_eq!(
        remove(PhantomHandle(12345)),
        Err(Error::NoSuchPhantom(PhantomHandle(12345)))
    );
    assert_eq!(player(PhantomHandle(12345)), None);
}
