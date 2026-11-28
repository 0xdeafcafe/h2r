use super::*;
use guest_layout::{FlatRam, MemMut};
use sim_players::{PlayerRecord, PlayersState};

fn record(name: &str, team: i8, unit: u32, local: i16, grace: i16) -> PlayerRecord {
    let mut r = PlayerRecord::default();
    // The record starts at datum +2.
    let put = |r: &mut PlayerRecord, at: usize, v: &[u8]| r.bytes[at - 2..at - 2 + v.len()].copy_from_slice(v);
    put(&mut r, Player::MACHINE.offset as usize, &[1, 2, 3, 4, 5, 6]);
    put(&mut r, Player::LOCAL_USER.offset as usize, &local.to_le_bytes());
    put(&mut r, Player::UNIT.offset as usize, &unit.to_le_bytes());
    let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    put(&mut r, Player::NAME.offset as usize, &units);
    put(&mut r, Player::TEAM.offset as usize, &[team as u8]);
    put(&mut r, Player::SPAWN_GRACE.offset as usize, &grace.to_le_bytes());
    r
}

fn state() -> PlayersState {
    let mut s = PlayersState::default();
    s.alloc(record("Alex", 1, 0xe26f_0003, 0, 30)).unwrap();
    let gone = s.alloc(record("Gone", 0, u32::MAX, -1, 0)).unwrap();
    s.alloc(record("Pat", 2, u32::MAX, -1, 5)).unwrap();
    s.delete(gone).unwrap();
    // The globals' local-user map: user 0 is player 0, the rest unmapped.
    s.globals.addr = 0x0050_0000;
    s.globals.bytes[0xc..0x1c].copy_from_slice(&[0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255]);
    s.first_person.addr = 0x0050_1000;
    s.first_person.bytes[4..8].copy_from_slice(&0xe26f_0003u32.to_le_bytes());
    s.first_person.bytes[8..12].copy_from_slice(&5u32.to_le_bytes());
    s
}

#[test]
fn the_players_list_in_slot_order_without_free_slots() {
    let s = state();
    let listed = players(&PlayersMem::new(&s)).unwrap();
    assert_eq!(listed.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Alex", "Pat"]);
    assert_eq!(listed[0].unit, Some(ObjectId(0xe26f_0003)));
    assert_eq!((listed[0].team, listed[0].local_user, listed[0].spawn_grace), (1, Some(0), 30));
    assert_eq!((listed[1].unit, listed[1].local_user, listed[1].machine), (None, None, [1, 2, 3, 4, 5, 6]));
    assert_eq!(listed[1].id, PlayerId(0xec72_0002));
}

#[test]
fn a_leaving_player_is_not_listed() {
    let mut s = state();
    let h = s.players.handle_at(0).unwrap();
    s.players.get_mut(h).unwrap().bytes[(Player::FLAGS.offset - 2) as usize] = 1 << 1;
    let names: Vec<_> = players(&PlayersMem::new(&s)).unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Pat"]);
}

#[test]
fn no_players_global_lists_nobody() {
    assert!(players(&FlatRam::new(0x004e_0000, 0x10_0000)).unwrap().is_empty());
}

#[test]
fn local_users_and_first_person_read_the_blocks() {
    let s = state();
    let mem = PlayersMem::new(&s);
    assert_eq!(local_users(&mem).unwrap(), [Some(0), None, None, None]);
    let fp = first_person(&mem, 0).unwrap().unwrap();
    assert_eq!((fp.unit, fp.key), (Some(ObjectId(0xe26f_0003)), 5));
    assert_eq!(first_person(&mem, 1).unwrap().unwrap().unit, Some(ObjectId(0)));
    assert!(first_person(&mem, 4).unwrap().is_none());
}

/// The animation view reads the manager `Object::ANIMATION_MANAGER` names:
/// the graph at `+0x68` and three channels of `0x20` bytes.
#[test]
fn anim_reads_the_manager_channels() {
    let (base, body) = (0x0060_0000u32, 0x0060_0100u32);
    let mut ram = FlatRam::new(base, 0x1000);
    let w = |ram: &mut FlatRam, at: u32, v: &[u8]| ram.write_bytes(at, v).unwrap();
    w(&mut ram, body + Object::ANIMATION_MANAGER.offset, &0x200i16.to_le_bytes());
    let manager = body + 0x200;
    w(&mut ram, manager + 0x68, &0xe000_0007u32.to_le_bytes());
    w(&mut ram, manager, &0xe000_0007u32.to_le_bytes());
    w(&mut ram, manager + 4, &(-1i16).to_le_bytes());
    w(&mut ram, manager + 6, &12i16.to_le_bytes());
    w(&mut ram, manager + 0x1c, &3.5f32.to_le_bytes());
    // Channels 2 and 3 are unset: graph -1.
    w(&mut ram, manager + 0x20, &u32::MAX.to_le_bytes());
    w(&mut ram, manager + 0x40, &u32::MAX.to_le_bytes());
    let a = anim(&ram, Object::at(body)).unwrap().unwrap();
    assert_eq!(a.graph, Some(0xe000_0007));
    assert!(a.channels[0].active());
    assert_eq!((a.channels[0].index, a.channels[0].frame), (12, 3.5));
    assert!(!a.channels[1].active() && !a.channels[2].active());
}

#[test]
fn an_object_without_a_manager_has_no_animation_state() {
    let (base, body) = (0x0060_0000u32, 0x0060_0100u32);
    let mut ram = FlatRam::new(base, 0x1000);
    ram.write_bytes(body + Object::ANIMATION_MANAGER.offset, &(-1i16).to_le_bytes()).unwrap();
    assert!(anim(&ram, Object::at(body)).unwrap().is_none());
}
