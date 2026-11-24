//! Two invented peers over an in-memory LAN, and one over loopback UDP.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::rc::Rc;

use actor_net::{Actor, Budget, Packet, Record};
use ed25519_dalek::SigningKey;

use crate::{Client, Datagrams, Event, Host, Reason};

type Queues = Rc<RefCell<BTreeMap<SocketAddr, VecDeque<(Vec<u8>, SocketAddr)>>>>;

/// An invented LAN: a socket per address, delivery in order, nothing lost
/// unless a test drops it.
#[derive(Clone)]
struct Lan {
    queues: Queues,
    me: SocketAddr,
}

impl Lan {
    fn pair() -> (Self, Self, SocketAddr, SocketAddr) {
        let queues: Queues = Rc::default();
        let host = addr(1);
        let client = addr(2);
        (
            Lan { queues: queues.clone(), me: host },
            Lan { queues, me: client },
            host,
            client,
        )
    }
}

fn addr(n: u8) -> SocketAddr {
    SocketAddr::from(([10, 0, 0, n], 3077))
}

impl Datagrams for Lan {
    fn send_to(&self, bytes: &[u8], to: SocketAddr) -> io::Result<()> {
        self.queues
            .borrow_mut()
            .entry(to)
            .or_default()
            .push_back((bytes.to_vec(), self.me));
        Ok(())
    }

    fn recv_from(&self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        let Some((bytes, from)) = self
            .queues
            .borrow_mut()
            .get_mut(&self.me)
            .and_then(VecDeque::pop_front)
        else {
            return Ok(None);
        };
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(Some((n, from)))
    }
}

fn counter_entropy() -> impl FnMut() -> [u8; 32] + Send + 'static {
    let mut n = 0u8;
    move || {
        n += 1;
        [n.wrapping_mul(37).wrapping_add(11); 32]
    }
}

const SESSION: [u8; 16] = *b"invented-session";

fn session_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

struct Rig {
    host: Host<Lan>,
    client: Client<Lan>,
    lan: Lan,
    client_addr: SocketAddr,
    host_addr: SocketAddr,
}

fn rig_with(client_key: &SigningKey) -> Rig {
    let (h, c, host_addr, client_addr) = Lan::pair();
    let lan = h.clone();
    Rig {
        host: Host::new(h, SESSION, session_key(), counter_entropy()),
        client: Client::new(
            c,
            host_addr,
            client_key.verifying_key(),
            SESSION,
            [0x42; 32],
        ),
        lan,
        client_addr,
        host_addr,
    }
}

fn rig() -> Rig {
    rig_with(&session_key())
}

fn establish(r: &mut Rig) {
    r.client.hello().unwrap();
    r.host.poll().unwrap();
    assert_eq!(r.client.poll(0).unwrap(), vec![Event::Established]);
    assert!(r.client.established());
}

fn key_packet(tick: u32) -> Packet {
    Packet {
        tick,
        records: vec![Record::Key {
            actors: vec![Actor {
                entity: 5,
                character: 1,
                squad: 2,
                team: 3,
                posture: 4,
            }],
            state: vec![9, 9],
        }],
    }
}

fn cue(tick: u32, sequence: u8) -> Record {
    Record::Cue {
        tick,
        sequence,
        kind: 2,
        bytes: vec![1, 2, 3],
    }
}

#[test]
fn a_hello_gets_a_channel_and_the_host_asks_for_a_key() {
    let mut r = rig();
    establish(&mut r);
    assert_eq!(r.host.peers(), vec![(1, true)]);
}

#[test]
fn a_key_and_a_cue_reach_the_guest_and_a_key_ends_the_ask() {
    let mut r = rig();
    establish(&mut r);
    let key = key_packet(10);
    assert!(r.host.send(1, &key).unwrap());
    assert_eq!(r.host.peers(), vec![(1, false)]);
    let cue_packet = Packet {
        tick: 11,
        records: vec![cue(11, 0)],
    };
    assert!(r.host.send(1, &cue_packet).unwrap());
    assert_eq!(
        r.client.poll(0).unwrap(),
        vec![Event::Packet(key), Event::Packet(cue_packet)]
    );
}

#[test]
fn the_budget_feeds_the_carrier_and_the_cue_repeats_in_three_packets() {
    let mut r = rig();
    establish(&mut r);
    let mut budget = Budget::default();
    let mut seen = Vec::new();
    for tick in 100..104u32 {
        let mut offers = Vec::new();
        if r.host.peers()[0].1 {
            offers.push(key_packet(tick).records.remove(0));
        }
        if tick == 101 {
            offers.push(cue(tick, 0));
        }
        if let Some(packet) = budget.schedule(tick, &offers).unwrap() {
            assert!(r.host.send(1, &packet).unwrap());
        }
        for event in r.client.poll(0).unwrap() {
            let Event::Packet(p) = event else { panic!("{event:?}") };
            seen.push(p);
        }
    }
    let copies = seen
        .iter()
        .flat_map(|p| &p.records)
        .filter(|r| matches!(r, Record::Cue { tick: 101, sequence: 0, .. }))
        .count();
    assert_eq!(copies, 3);
    assert!(matches!(seen[0].records[0], Record::Key { .. }));
}

#[test]
fn a_forged_tag_a_replay_and_a_stale_tick_are_dropped_and_counted() {
    let mut r = rig();
    establish(&mut r);
    r.host.send(1, &key_packet(20)).unwrap();
    // The datagram on the wire, to replay and to alter.
    let wire = r.lan.queues.borrow()[&r.client_addr][0].0.clone();
    r.client.poll(0).unwrap();

    let host = r.host_addr;
    let inject = |r: &Rig, bytes: Vec<u8>, from: SocketAddr| {
        r.lan
            .queues
            .borrow_mut()
            .entry(r.client_addr)
            .or_default()
            .push_back((bytes, from));
    };
    // Replay: same tick again.
    inject(&r, wire.clone(), host);
    // A flipped payload bit.
    let mut flipped = wire.clone();
    flipped[6] ^= 1;
    inject(&r, flipped, host);
    // A flipped tag bit.
    let mut tag = wire.clone();
    *tag.last_mut().unwrap() ^= 1;
    inject(&r, tag, host);
    // Another LAN peer, not the host, with a good copy.
    inject(&r, wire, addr(9));
    // Short and unknown datagrams.
    inject(&r, vec![3, 1], host);
    inject(&r, vec![0xee; 50], host);
    let events = r.client.poll(0).unwrap();
    assert_eq!(
        events,
        vec![
            Event::Dropped(Reason::Stale),
            Event::Dropped(Reason::BadTag),
            Event::Dropped(Reason::BadTag),
            Event::Dropped(Reason::NoChannel),
            Event::Dropped(Reason::Malformed),
            Event::Dropped(Reason::Malformed),
        ]
    );
    assert_eq!(r.client.drops(Reason::BadTag), 2);
    // The channel still works.
    r.host.send(1, &Packet { tick: 21, records: vec![cue(21, 0)] }).unwrap();
    assert!(matches!(r.client.poll(0).unwrap()[0], Event::Packet(_)));
}

#[test]
fn an_older_tick_is_not_sent_and_the_tick_wraps() {
    let mut r = rig();
    establish(&mut r);
    assert!(r.host.send(1, &key_packet(u32::MAX - 1)).unwrap());
    assert!(!r.host.send(1, &key_packet(u32::MAX - 1)).unwrap());
    assert!(!r.host.send(1, &key_packet(u32::MAX - 5)).unwrap());
    assert!(r.host.send(1, &key_packet(3)).unwrap());
    assert_eq!(r.client.poll(0).unwrap().len(), 2);
    assert!(!r.host.send(9, &key_packet(4)).unwrap());
}

#[test]
fn an_answer_from_a_key_the_client_does_not_hold_is_refused() {
    // The LAN peer answers with its own session key.
    let other = SigningKey::from_bytes(&[8u8; 32]);
    let mut r = rig_with(&other);
    r.client.hello().unwrap();
    r.host.poll().unwrap();
    assert_eq!(
        r.client.poll(0).unwrap(),
        vec![Event::Dropped(Reason::BadAnswer)]
    );
    assert!(!r.client.established());
    r.host.send(1, &key_packet(1)).unwrap();
    assert_eq!(
        r.client.poll(0).unwrap(),
        vec![Event::Dropped(Reason::NoChannel)]
    );
}

#[test]
fn an_old_answer_does_not_replace_the_channel_of_a_new_hello() {
    let mut r = rig();
    r.client.hello().unwrap();
    r.host.poll().unwrap();
    let old = r.lan.queues.borrow()[&r.client_addr][0].0.clone();
    r.client.poll(0).unwrap();
    r.client.hello().unwrap();
    r.host.poll().unwrap();
    r.client.poll(0).unwrap();
    r.lan
        .queues
        .borrow_mut()
        .entry(r.client_addr)
        .or_default()
        .push_back((old, r.host_addr));
    assert_eq!(
        r.client.poll(0).unwrap(),
        vec![Event::Dropped(Reason::BadAnswer)]
    );
    // A new hello asked for a new key.
    assert_eq!(r.host.peers(), vec![(1, true)]);
    r.host.send(1, &key_packet(5)).unwrap();
    assert!(matches!(r.client.poll(0).unwrap()[0], Event::Packet(_)));
}

#[test]
fn the_host_reads_only_a_hello_and_answers_sixteen_machines_at_most() {
    let mut r = rig();
    let push = |r: &Rig, bytes: Vec<u8>, from: SocketAddr| {
        r.lan
            .queues
            .borrow_mut()
            .entry(r.host_addr)
            .or_default()
            .push_back((bytes, from));
    };
    // A client's data datagram, an empty one and a short hello.
    push(&r, vec![3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], addr(2));
    push(&r, vec![], addr(2));
    push(&r, vec![1, 1, 0], addr(2));
    r.host.poll().unwrap();
    assert_eq!(r.host.drops()[&Reason::NotAHello], 1);
    assert_eq!(r.host.drops()[&Reason::Malformed], 2);
    assert!(r.host.peers().is_empty());

    for n in 0..20u8 {
        let mut hello = vec![1, 1];
        hello.extend(crate::crypto::public_key(&[n + 1; 32]));
        hello.extend(1u64.to_le_bytes());
        push(&r, hello, addr(100 + n));
    }
    r.host.poll().unwrap();
    assert_eq!(r.host.peers().len(), crate::MAX_PEERS);
    assert_eq!(r.host.drops()[&Reason::HostFull], 5);
}

#[test]
fn a_low_order_client_key_is_refused() {
    let mut r = rig();
    let mut hello = vec![1, 1];
    hello.extend([0u8; 32]);
    hello.extend(1u64.to_le_bytes());
    r.lan
        .queues
        .borrow_mut()
        .entry(r.host_addr)
        .or_default()
        .push_back((hello, addr(2)));
    r.host.poll().unwrap();
    assert_eq!(r.host.drops()[&Reason::WeakKey], 1);
    assert!(r.host.peers().is_empty());
}

#[test]
fn thirty_drops_in_a_second_close_the_channel_and_a_slow_trickle_does_not() {
    let mut r = rig();
    establish(&mut r);
    let junk = |r: &Rig, n: usize| {
        for _ in 0..n {
            r.lan
                .queues
                .borrow_mut()
                .entry(r.client_addr)
                .or_default()
                .push_back((vec![0xee], r.host_addr));
        }
    };
    // 29 in a window, then one more after the window slid past the first.
    junk(&r, 29);
    r.client.poll(0).unwrap();
    junk(&r, 1);
    r.client.poll(1_000).unwrap();
    assert!(r.client.established());
    junk(&r, 30);
    let events = r.client.poll(1_100).unwrap();
    assert_eq!(events.last(), Some(&Event::Closed));
    assert!(r.client.closed() && !r.client.established());
    // Nothing more is read or sent.
    r.client.hello().unwrap();
    r.host.send(1, &key_packet(1)).unwrap();
    assert!(r.client.poll(1_200).unwrap().is_empty());
}

#[test]
fn two_sockets_on_loopback_exchange_a_key_and_a_cue() {
    let h = UdpSocket::bind("127.0.0.1:0").unwrap();
    let c = UdpSocket::bind("127.0.0.1:0").unwrap();
    h.set_nonblocking(true).unwrap();
    c.set_nonblocking(true).unwrap();
    let host_addr = h.local_addr().unwrap();
    let mut host = Host::new(h, SESSION, session_key(), counter_entropy());
    let mut client = Client::new(c, host_addr, session_key().verifying_key(), SESSION, [5; 32]);

    let wait = |f: &mut dyn FnMut() -> bool| {
        for _ in 0..200 {
            if f() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no datagram on loopback");
    };
    client.hello().unwrap();
    wait(&mut || {
        host.poll().unwrap();
        !host.peers().is_empty()
    });
    let mut events = Vec::new();
    wait(&mut || {
        events.extend(client.poll(0).unwrap());
        client.established()
    });
    let key = key_packet(1);
    let cue_packet = Packet {
        tick: 2,
        records: vec![cue(2, 0)],
    };
    host.send(1, &key).unwrap();
    host.send(1, &cue_packet).unwrap();
    wait(&mut || {
        events.extend(client.poll(0).unwrap());
        events.len() >= 3
    });
    assert_eq!(
        events,
        vec![
            Event::Established,
            Event::Packet(key),
            Event::Packet(cue_packet)
        ]
    );
}
