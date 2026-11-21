use super::*;
use crate::{Hle, LiveConfig, tunnel_client::ActorInbox};
use actor_net::{Actor, ActorSet, Budget, KEY_PERIOD_TICKS, Replica, budget::BYTES_PER_SECOND};
use coop::link::{Conditions, FakeClock, Link, SimControl, SimLink, sim_pair};

fn register(peers: &mut Peers, actor_channel: bool) -> (u32, Receiver<Frame>) {
    peers
        .register(
            Instant::now(),
            Arc::new(Poller::new().unwrap()),
            actor_channel,
        )
        .unwrap()
}

fn actor_set(posture: u8, state: u8) -> ActorSet {
    let mut set = ActorSet::default();
    set.apply(&Record::Key {
        actors: vec![Actor {
            entity: 4,
            character: 7,
            squad: 2,
            team: 1,
            posture,
        }],
        state: vec![state],
    })
    .unwrap();
    set
}

fn packet(tick: u32, record: Record) -> Packet {
    Packet {
        tick,
        records: vec![record],
    }
}

struct ActorLink {
    clock: FakeClock,
    host: SimLink,
    client: SimLink,
    control: SimControl,
    inbox: ActorInbox,
}

impl ActorLink {
    fn new() -> Self {
        let clock = FakeClock::default();
        let (host, client, control) = sim_pair(&clock, Conditions::default(), 5);
        Self {
            clock,
            host,
            client,
            control,
            inbox: ActorInbox::default(),
        }
    }

    fn send(&mut self, peers: &mut Peers, id: u32, out: &Receiver<Frame>, packet: &Packet) {
        assert!(peers.send_actor(id, packet).unwrap());
        self.host.send(&encode(&out.try_recv().unwrap()));
    }

    fn receive(&mut self) -> Vec<Packet> {
        while let Some(bytes) = self.client.recv() {
            self.inbox.receive(&decode(&bytes).unwrap());
        }
        self.inbox.drain()
    }
}

#[test]
fn lost_actor_frames_converge_on_the_next_key() {
    let mut link = ActorLink::new();
    let mut peers = Peers::default();
    let (id, out) = register(&mut peers, true);
    let mut replica = Replica::default();
    let initial = actor_set(1, 1);
    let current = actor_set(2, 7);

    link.send(&mut peers, id, &out, &packet(1, initial.key()));
    assert!(replica.apply(&link.receive().pop().unwrap()).unwrap());

    link.control.set_conditions(Conditions {
        loss: 1.0,
        ..Default::default()
    });
    link.send(&mut peers, id, &out, &packet(2, Record::State(vec![7])));
    assert!(link.receive().is_empty());
    assert_eq!(link.control.dropped(), 1);
    assert_eq!(replica.set(), &initial);

    link.control.set_conditions(Conditions::default());
    link.send(
        &mut peers,
        id,
        &out,
        &packet(1 + KEY_PERIOD_TICKS, current.key()),
    );
    assert!(replica.apply(&link.receive().pop().unwrap()).unwrap());
    assert_eq!(replica.set(), &current);
}

#[test]
fn reordered_actor_frames_cannot_rewind_a_newer_key() {
    let mut link = ActorLink::new();
    let mut peers = Peers::default();
    let (id, out) = register(&mut peers, true);
    let current = actor_set(2, 7);
    let mut replica = Replica::default();

    link.control.set_conditions(Conditions {
        latency: 10,
        reorder: 1.0,
        ..Default::default()
    });
    link.send(&mut peers, id, &out, &packet(1, actor_set(1, 1).key()));
    assert!(link.receive().is_empty());

    link.control.set_conditions(Conditions::default());
    link.send(
        &mut peers,
        id,
        &out,
        &packet(1 + KEY_PERIOD_TICKS, current.key()),
    );
    assert!(replica.apply(&link.receive().pop().unwrap()).unwrap());

    link.clock.advance(20);
    let late = link.receive().pop().unwrap();
    assert_eq!(late.tick, 1);
    assert!(!replica.apply(&late).unwrap());
    assert_eq!(replica.set(), &current);
}

#[test]
fn a_tunnel_down_retires_the_peer_and_rejoin_gets_a_key() {
    let mut link = ActorLink::new();
    let mut peers = Peers::default();
    let (old, old_out) = register(&mut peers, true);
    let key = packet(1, actor_set(2, 7).key());

    link.control.set_down(true);
    link.send(&mut peers, old, &old_out, &key);
    assert_eq!(link.control.dropped(), 1);
    assert!(link.receive().is_empty());
    peers.remove(old);
    assert!(peers.actor_peers(2).is_empty());
    assert!(!peers.send_actor(old, &key).unwrap());

    let (new, new_out) = register(&mut peers, true);
    assert_ne!(old, new);
    assert_eq!(peers.actor_peers(2), vec![(new, true)]);
    link.control.set_down(false);
    let key = Packet { tick: 2, ..key };
    link.send(&mut peers, new, &new_out, &key);
    assert_eq!(link.receive(), vec![key]);
}

#[test]
fn a_client_without_the_channel_keeps_receiving_retail_datagrams() {
    let mut peers = Peers::default();
    let (legacy, out) = register(&mut peers, false);
    let (capable, _capable_out) = register(&mut peers, true);
    peers.request_actor_key(legacy);

    assert_eq!(peers.actor_peers(1), vec![(capable, true)]);
    assert!(
        !peers
            .send_actor(legacy, &packet(1, actor_set(1, 1).key()))
            .unwrap()
    );
    assert!(peers.send(legacy, 6, b"game datagram", Instant::now()));
    let frame = out.try_recv().unwrap();
    assert_eq!(frame.kind, Kind::Datagram);
    assert_eq!(frame.payload.as_slice(), b"game datagram");
    assert!(out.try_recv().is_err());
}

#[test]
fn frames_of_another_kind_are_ignored() {
    let mut inbox = ActorInbox::default();
    let packet = packet(1, actor_set(2, 7).key());
    let payload = actor_net::encode(&packet).unwrap();
    for kind in [
        Kind::Datagram,
        Kind::Hello,
        Kind::Bye,
        Kind::ActorKeyRequest,
    ] {
        inbox.receive(&control(kind, 2, payload.clone()));
    }
    let mut wrong_port = control(Kind::Actor, 2, payload.clone());
    wrong_port.port = 6;
    inbox.receive(&wrong_port);
    inbox.receive(&control(Kind::Actor, 2, vec![0]));
    assert!(inbox.drain().is_empty());

    inbox.receive(&control(Kind::Actor, 2, payload));
    assert_eq!(inbox.drain(), vec![packet]);
    assert!(inbox.drain().is_empty());
}

#[test]
fn a_key_request_is_answered_once_a_second_at_most() {
    let mut peers = Peers::default();
    let (id, out) = register(&mut peers, true);
    let set = actor_set(2, 7);
    let join = u32::MAX - 3;

    assert_eq!(peers.actor_peers(join), vec![(id, true)]);
    assert!(peers.send_actor(id, &packet(join, set.key())).unwrap());
    for elapsed in 1..TICKS_PER_SECOND {
        peers.request_actor_key(id);
        assert_eq!(
            peers.actor_peers(join.wrapping_add(elapsed)),
            vec![(id, false)]
        );
    }
    let due = join.wrapping_add(TICKS_PER_SECOND);
    assert_eq!(peers.actor_peers(due), vec![(id, true)]);
    assert!(peers.send_actor(id, &packet(due, set.key())).unwrap());
    assert_eq!(out.try_iter().count(), 2);

    assert_eq!(peers.actor_peers(due + TICKS_PER_SECOND), vec![(id, false)]);
    peers.request_actor_key(id);
    assert_eq!(
        peers.actor_peers(due + TICKS_PER_SECOND - 1),
        vec![(id, false)]
    );
    assert_eq!(peers.actor_peers(due + TICKS_PER_SECOND), vec![(id, true)]);
}

#[test]
fn a_full_tunnel_outbox_keeps_the_join_key_pending() {
    let mut peers = Peers::default();
    let (id, out) = register(&mut peers, true);
    let now = Instant::now();
    let set = actor_set(2, 7);
    for _ in 0..OUTBOX_DEPTH {
        assert!(peers.send(id, 0, b"game", now));
    }
    assert!(!peers.send_actor(id, &packet(1, set.key())).unwrap());
    assert_eq!(peers.actor_peers(2), vec![(id, true)]);
    assert!(
        !peers
            .send_actor(id, &packet(2, Record::State(vec![1])))
            .unwrap()
    );
    assert_eq!(out.try_iter().count(), OUTBOX_DEPTH);
    assert!(peers.send_actor(id, &packet(3, set.key())).unwrap());
    assert_eq!(peers.actor_peers(4), vec![(id, false)]);
}

#[test]
fn tunnel_framing_is_outside_the_actor_budget() {
    let mut peers = Peers::default();
    let (id, out) = register(&mut peers, true);
    let mut budget = Budget::default();
    let mut actor_bytes = 0;
    let mut framed_bytes = 0;
    let candidates = [actor_set(2, 7).key(), Record::State(vec![3; 63])];
    let mut sent = 0;

    for tick in 0..TICKS_PER_SECOND {
        let offered = if tick == 0 {
            &candidates[..]
        } else {
            &candidates[1..]
        };
        let Some(packet) = budget.schedule(tick, offered).unwrap() else {
            continue;
        };
        assert!(peers.send_actor(id, &packet).unwrap());
        let frame = out.try_recv().unwrap();
        assert_eq!(frame.payload, actor_net::encode(&packet).unwrap());
        assert_eq!(frame.payload.len(), packet.encoded_len().unwrap());
        actor_bytes += frame.payload.len();
        framed_bytes += encode(&frame).len();
        sent += 1;
    }
    assert!(sent > 1);
    assert!(actor_bytes <= BYTES_PER_SECOND);
    assert!(framed_bytes > BYTES_PER_SECOND);
    assert_eq!(
        framed_bytes - actor_bytes,
        sent * live_proto::tunnel::HEADER_LEN
    );
}

#[test]
fn no_tunnel_means_no_channel() {
    let address = "127.0.0.1:31847".parse().unwrap();
    let hle = Hle::with_address(LiveConfig::default(), address).unwrap();
    assert!(hle.actor_tunnel_peers(1).is_empty());
    assert!(
        !hle.send_actor_tunnel_packet(2, &packet(1, actor_set(2, 7).key()))
            .unwrap()
    );
    assert!(hle.drain_actor_tunnel_packets().is_empty());
    assert!(!hle.request_actor_tunnel_key());
}
