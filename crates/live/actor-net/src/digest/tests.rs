use super::*;
use crate::wire::{Actor, ActorChange};
use crate::{Packet, Replica, decode, encode};

fn actor(entity: u32) -> Actor {
    Actor {
        entity,
        character: 3,
        squad: 4,
        team: 2,
        posture: 0xff,
    }
}

fn host(entities: &[u32], state: &[u8]) -> ActorSet {
    let mut set = ActorSet::default();
    for &entity in entities {
        set.apply(&Record::Actor(ActorChange::Upsert(actor(entity))))
            .unwrap();
    }
    set.apply(&Record::State(state.to_vec())).unwrap();
    set
}

fn parts(record: &Record) -> (u64, u16) {
    let Record::Digest {
        state_hash,
        actor_count,
        ..
    } = record
    else {
        panic!("not a digest: {record:?}")
    };
    (*state_hash, *actor_count)
}

#[test]
fn the_state_hash_is_fnv_1a_64() {
    // Published FNV-1a 64 vectors.
    assert_eq!(state_hash(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(state_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(state_hash(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn a_digest_carries_the_log_head_state_hash_count_and_show_hash_and_round_trips() {
    let set = host(&[0x0100_0001, 0x0200_0002], &[1, 2, 3]);
    let digest = set.digest([9; 32], state_hash(&[4, 5]));
    assert_eq!(
        digest,
        Record::Digest {
            log_head: [9; 32],
            state_hash: state_hash(&[1, 2, 3]),
            actor_count: 2,
            show_hash: state_hash(&[4, 5]),
        }
    );
    let packet = Packet {
        tick: 30,
        records: vec![digest.clone()],
    };
    assert_eq!(decode(&encode(&packet).unwrap()).unwrap().records, [digest]);
}

#[test]
fn a_replica_equal_to_the_host_agrees() {
    let set = host(&[0x0100_0001], &[7]);
    let mut replica = Replica::default();
    replica
        .apply(&Packet {
            tick: 1,
            records: vec![set.key()],
        })
        .unwrap();
    let (hash, count) = parts(&set.digest([0; 32], 0));
    let mut check = Check::default();
    assert_eq!(check.observe(replica.set(), hash, count), Verdict::Agrees);
    assert_eq!(check.misses(), 0);
}

#[test]
fn a_wrong_state_or_count_asks_for_a_key() {
    let set = host(&[0x0100_0001, 0x0200_0002], &[7]);
    let (hash, count) = parts(&set.digest([0; 32], 0));
    let mut check = Check::default();
    let lost_actor = host(&[0x0100_0001], &[7]);
    assert_eq!(check.observe(&lost_actor, hash, count), Verdict::RequestKey);
    let mut check = Check::default();
    let lost_state = host(&[0x0100_0001, 0x0200_0002], &[6]);
    assert_eq!(check.observe(&lost_state, hash, count), Verdict::RequestKey);
}

#[test]
fn three_mismatches_in_a_row_with_keys_between_diverge() {
    let set = host(&[0x0100_0001], &[1]);
    let (hash, count) = parts(&set.digest([0; 32], 0));
    let stale = ActorSet::default();
    let mut check = Check::default();
    assert_eq!(check.observe(&stale, hash, count), Verdict::RequestKey);
    check.key_applied();
    assert_eq!(check.observe(&stale, hash, count), Verdict::RequestKey);
    assert_eq!(check.misses(), 2);
    check.key_applied();
    assert_eq!(check.observe(&stale, hash, count), Verdict::Diverged);
    assert_eq!(check.misses(), DIVERGED_AFTER);
}

#[test]
fn an_agreement_ends_the_run() {
    let set = host(&[0x0100_0001], &[1]);
    let (hash, count) = parts(&set.digest([0; 32], 0));
    let stale = ActorSet::default();
    let mut check = Check::default();
    check.observe(&stale, hash, count);
    check.key_applied();
    check.observe(&stale, hash, count);
    assert_eq!(check.observe(&set, hash, count), Verdict::Agrees);
    assert_eq!(check.misses(), 0);
    assert_eq!(check.observe(&stale, hash, count), Verdict::RequestKey);
    assert_eq!(check.misses(), 1);
}

#[test]
fn mismatches_while_the_key_is_on_its_way_ask_again_and_never_diverge() {
    let set = host(&[0x0100_0001], &[1]);
    let (hash, count) = parts(&set.digest([0; 32], 0));
    let stale = ActorSet::default();
    let mut check = Check::default();
    for _ in 0..10 {
        assert_eq!(check.observe(&stale, hash, count), Verdict::RequestKey);
        assert_eq!(check.misses(), 1);
    }
}
