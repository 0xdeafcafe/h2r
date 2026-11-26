//! Invented map, tag and players. No game, socket or clock: messages are
//! delivered by hand, in orders a seeded generator picks.
use std::collections::BTreeMap;

use forge_doc::{
    BuilderVersions, Edit, Endpoint, EntryOp, FieldWrite, ForgeDoc, Limits, MapRef, Op, OpId,
    Placement, Proposal, Refusal, Sha256, TagKey, Value, WorldBounds,
};

use super::*;
use crate::things::frame_writes;

const BLOCK: BlockKind = BlockKind::Crates;
const HOST: Seat = Seat(0);
const ALPHA: Seat = Seat(1);
const BRAVO: Seat = Seat(2);

fn start() -> ForgeDoc {
    ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions {
            forge_build: 1,
            pathfind: 1,
            bots: 1,
        },
        base: MapRef {
            map: "testroom".into(),
            sha256: Sha256::of(b"an invented room"),
        },
        sources: vec![],
        title: "Room".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
        waves: None,
    }
}

fn limits() -> Limits {
    Limits {
        base_counts: BTreeMap::from([(BLOCK, 7)]),
        world: Some(WorldBounds {
            min: [-20.; 3],
            max: [20.; 3],
        }),
        ..Default::default()
    }
}

fn frame(x: f32) -> Frame {
    Frame {
        position: [x, 1., 2.],
        rotation: [0., 0., 0.],
    }
}

fn add(id: OpId, x: f32) -> Op {
    let mut fields = vec![FieldWrite {
        field: "palette index".into(),
        value: Value::Tag(Some(TagKey {
            group: "bloc".into(),
            path: r"fixture\cube".into(),
        })),
    }];
    fields.extend(frame_writes(BLOCK, frame(x)).unwrap());
    Op {
        id,
        edit: Edit::Placement(Placement::Add {
            block: BLOCK,
            like: None,
            fields,
        }),
    }
}

fn position(x: f32) -> FieldWrite {
    frame_writes(BLOCK, frame(x))
        .unwrap()
        .into_iter()
        .find(|w| w.field == "position")
        .unwrap()
}

fn move_to(id: OpId, target: Endpoint, x: f32) -> Op {
    Op {
        id,
        edit: Edit::Entry(EntryOp::Set {
            target,
            write: position(x),
        }),
    }
}

fn remove(id: OpId, target: Endpoint) -> Op {
    Op {
        id,
        edit: Edit::Entry(EntryOp::Remove { target }),
    }
}

fn undo(id: OpId) -> Op {
    Op {
        id,
        edit: Edit::Undo,
    }
}

fn propose(peer: &Peer, nonce: u32, op: Op) -> Up {
    Up::Propose {
        nonce,
        proposal: Proposal {
            author: "anyone".into(),
            base_seq: peer.state().head().seq,
            op,
            was: None,
        },
    }
}

/// A host at tick 0 with the given seats joined, each with its peer, and
/// the stream so far delivered to every peer.
struct Net<A: Admit> {
    host: Host<A>,
    peers: BTreeMap<Seat, Peer>,
    now: u32,
}

impl<A: Admit> Net<A> {
    fn new(admit: A, seats: &[(Seat, &str, Role)]) -> Self {
        let host = Host::new(start(), limits(), admit, 0, APPLY_DELAY).unwrap();
        let mut net = Self {
            host,
            peers: BTreeMap::new(),
            now: 0,
        };
        for &(seat, name, role) in seats {
            net.join(seat, name, role);
        }
        net
    }

    fn join(&mut self, seat: Seat, name: &str, role: Role) {
        let (welcome, item) = self.host.join(seat, name, role).unwrap();
        let welcome = Welcome::from_bytes(&welcome.to_bytes().unwrap()).unwrap();
        self.send(vec![item]);
        self.peers
            .insert(seat, Peer::join(welcome, limits()).unwrap());
    }

    fn send(&mut self, items: Vec<Stamped>) {
        for item in items {
            for peer in self.peers.values_mut() {
                peer.receive(item.clone()).unwrap();
            }
        }
    }

    fn up(&mut self, seat: Seat, up: Up) -> Option<Reply> {
        let out = self.host.receive(seat, up);
        self.send(out.items);
        out.reply
    }

    /// Run every machine through `ticks` more ticks.
    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.now += 1;
            let through = self.host.tick(self.now).unwrap();
            self.send(through.into_iter().collect());
            for peer in self.peers.values_mut() {
                peer.due(self.now).unwrap();
            }
        }
    }

    fn peer(&self, seat: Seat) -> &Peer {
        &self.peers[&seat]
    }
}

fn accepted(reply: Option<Reply>) -> u64 {
    match reply {
        Some(Reply::Accepted { n, .. }) => n,
        other => panic!("expected an acceptance, got {other:?}"),
    }
}

fn refused(reply: Option<Reply>) -> NetRefusal {
    match reply {
        Some(Reply::Refused { refusal, .. }) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn editors() -> Net<DocOnly> {
    Net::new(
        DocOnly,
        &[
            (HOST, "Host", Role::Editor),
            (ALPHA, "Alpha", Role::Editor),
            (BRAVO, "Bravo", Role::Editor),
        ],
    )
}

/// Every machine and a fold of the whole log agree with the host.
fn assert_converged<A: Admit>(host: &Host<A>, peers: &[&Peer]) {
    let state = host.state();
    let (whole, head) =
        forge_doc::fold_from(state.start(), state.start_head(), state.log(), &limits()).unwrap();
    assert_eq!(head, state.head());
    assert_eq!(&whole, state.doc());
    for peer in peers {
        assert_eq!(peer.state().head(), state.head(), "{}", peer.seat());
        assert_eq!(peer.state().doc(), state.doc(), "{}", peer.seat());
        assert_eq!(peer.state().holds(), state.holds(), "{}", peer.seat());
        assert_eq!(peer.state().members(), state.members(), "{}", peer.seat());
        assert_eq!(peer.state().n(), state.n(), "{}", peer.seat());
        host.check(peer.seat(), peer.beat()).unwrap();
    }
}

#[test]
fn lanes_never_meet() {
    assert_eq!(next_in_lane(Seat(0), 0), Some(OpId(1)));
    assert_eq!(next_in_lane(Seat(2), 0), Some(OpId(3)));
    assert_eq!(next_in_lane(Seat(2), 3), Some(OpId(19)));
    assert_eq!(next_in_lane(Seat(2), 18), Some(OpId(19)));
    assert_eq!(next_in_lane(Seat(15), 16), Some(OpId(32)));
    assert_eq!(next_in_lane(Seat(1), u32::MAX - 1), None);
    for seat in 0..SEATS {
        for above in 0..100 {
            let id = next_in_lane(Seat(seat), above).unwrap();
            assert!(id.0 > above && id.0 <= above + u32::from(SEATS));
            assert!(lane(Seat(seat), id, 0));
            assert!((0..SEATS).filter(|s| lane(Seat(*s), id, 0)).count() == 1);
        }
    }
    // The starting document's IDs belong to no lane.
    assert!(lane(Seat(3), OpId(2), 5));
}

#[test]
fn an_edit_reaches_every_machine_at_its_tick() {
    let mut net = editors();
    net.run(1);
    let id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 1, add(id, 3.))));
    let entry = &net.host.state().log()[0];
    assert_eq!(entry.author, "Alpha");
    assert_eq!(entry.tick, 1 + APPLY_DELAY);
    // Nothing is due before the apply tick.
    for _ in 0..APPLY_DELAY - 1 {
        net.run(1);
        assert_eq!(net.peer(BRAVO).state().head().seq, 0);
    }
    net.run(1);
    for peer in net.peers.values() {
        assert_eq!(peer.state().head().seq, 1);
    }
    assert_converged(&net.host, &net.peers.values().collect::<Vec<_>>());
}

#[test]
fn a_peer_waits_for_the_watermark() {
    let mut host = Host::new(start(), limits(), DocOnly, 0, APPLY_DELAY).unwrap();
    let (welcome, _) = host.join(ALPHA, "Alpha", Role::Editor).unwrap();
    let mut peer = Peer::join(welcome, limits()).unwrap();
    assert_eq!(
        peer.due(1),
        Err(PeerError::Wait {
            tick: 1,
            through: 0
        })
    );
    let through = host.tick(1).unwrap().unwrap();
    assert_eq!(through.tick, APPLY_DELAY);
    peer.receive(through).unwrap();
    assert!(peer.due(APPLY_DELAY).is_ok());
    assert!(matches!(
        peer.due(APPLY_DELAY + 1),
        Err(PeerError::Wait { .. })
    ));
    assert!(matches!(peer.due(1), Err(PeerError::Rewind { .. })));
}

#[test]
fn reordered_delivery_gives_the_same_stream() {
    let mut host = Host::new(start(), limits(), DocOnly, 0, APPLY_DELAY).unwrap();
    let (welcome, join) = host.join(ALPHA, "Alpha", Role::Editor).unwrap();
    let mut late = Peer::join(welcome.clone(), limits()).unwrap();
    let mut items = vec![join];
    items.extend(host.tick(1).unwrap());
    let mut next = 1;
    for x in 0..5 {
        let id = next_in_lane(ALPHA, next).unwrap();
        next = id.0;
        let up = Up::Propose {
            nonce: x,
            proposal: Proposal {
                author: "Alpha".into(),
                base_seq: host.state().head().seq,
                op: add(id, x as f32),
                was: None,
            },
        };
        items.extend(host.receive(ALPHA, up).items);
    }
    items.extend(host.tick(40).unwrap());
    let mut peer = Peer::join(welcome, limits()).unwrap();
    for item in &items {
        peer.receive(item.clone()).unwrap();
    }
    for item in items.iter().rev() {
        late.receive(item.clone()).unwrap();
        // A copy is ignored.
        late.receive(item.clone()).unwrap();
    }
    peer.due(40).unwrap();
    late.due(40).unwrap();
    assert_eq!(peer.state(), late.state());
    assert_eq!(peer.state().head().seq, 5);
    assert_converged(&host, &[&peer, &late]);
}

#[test]
fn an_item_for_a_tick_already_run_ends_the_session() {
    let mut host = Host::new(start(), limits(), DocOnly, 0, APPLY_DELAY).unwrap();
    let (welcome, join) = host.join(ALPHA, "Alpha", Role::Editor).unwrap();
    let mut peer = Peer::join(welcome, limits()).unwrap();
    peer.receive(join).unwrap();
    peer.receive(host.tick(10).unwrap().unwrap()).unwrap();
    peer.due(10).unwrap();
    let forged = Stamped {
        n: peer.state().n() + 1,
        tick: 9,
        item: Item::Release { seat: ALPHA },
    };
    assert!(matches!(peer.receive(forged), Err(PeerError::Late { .. })));
}

/// A small seeded generator, so a failing order can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, self.below(i + 1));
        }
    }
}

/// What a machine proposes from what it has applied: an add, a move or a
/// removal of any active added thing, or an undo of its own last edit.
fn edit_from(peer: &mut Peer, rng: &mut Rng) -> Op {
    let doc = peer.state().doc();
    let added: Vec<OpId> = doc
        .ops
        .iter()
        .filter(|op| matches!(op.edit, Edit::Placement(Placement::Add { .. })))
        .map(|op| op.id)
        .collect();
    let mine = doc
        .ops
        .last()
        .filter(|op| lane(peer.seat(), op.id, 0))
        .map(|op| op.id);
    let x = (rng.below(30) as f32) - 15.;
    match rng.below(10) {
        0 if mine.is_some() => undo(mine.unwrap()),
        1 | 2 if !added.is_empty() => {
            let target = Endpoint::Added(added[rng.below(added.len())]);
            remove(peer.op_ids(1).unwrap()[0], target)
        }
        3..=5 if !added.is_empty() => {
            let target = Endpoint::Added(added[rng.below(added.len())]);
            move_to(peer.op_ids(1).unwrap()[0], target, x)
        }
        _ => add(peer.op_ids(1).unwrap()[0], x),
    }
}

/// Three editing machines (the host's own and two others) propose against
/// whatever they have applied. The host takes the proposals in a shuffled
/// order and each machine gets the stream in its own shuffled order and
/// at its own pace. All end at the host's head and document.
#[test]
fn two_peers_and_a_host_converge_under_reordered_proposals() {
    let mut rebased_total = 0;
    for seed in 1..=24u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut net = editors();
        let seats = [HOST, ALPHA, BRAVO];
        let mut to_host: Vec<(Seat, Up)> = vec![];
        let mut inbox: BTreeMap<Seat, Vec<Stamped>> = BTreeMap::new();
        let mut ran: BTreeMap<Seat, u32> = seats.iter().map(|s| (*s, 0)).collect();
        let mut nonce = 0;
        let mut accepted_n = 0;
        let mut rebased = 0;
        let deliver = |items: Vec<Stamped>, inbox: &mut BTreeMap<Seat, Vec<Stamped>>| {
            for seat in seats {
                inbox.entry(seat).or_default().extend(items.iter().cloned());
            }
        };

        for now in 1..=120u32 {
            // Each machine may propose from what it has applied.
            for seat in seats {
                if rng.below(3) == 0 {
                    nonce += 1;
                    let peer = net.peers.get_mut(&seat).unwrap();
                    let op = edit_from(peer, &mut rng);
                    to_host.push((seat, propose(peer, nonce, op)));
                }
            }
            // The host takes a shuffled part of what has reached it.
            rng.shuffle(&mut to_host);
            let take = rng.below(to_host.len() + 1);
            for (seat, up) in to_host.drain(..take).collect::<Vec<_>>() {
                let base = match &up {
                    Up::Propose { proposal, .. } => proposal.base_seq,
                    _ => unreachable!(),
                };
                let head = net.host.state().head().seq;
                let out = net.host.receive(seat, up);
                if matches!(out.reply, Some(Reply::Accepted { .. })) {
                    accepted_n += 1;
                    rebased += usize::from(base < head);
                }
                deliver(out.items, &mut inbox);
            }
            deliver(
                net.host.tick(now).unwrap().into_iter().collect(),
                &mut inbox,
            );
            // Each machine gets a shuffled part of its stream and runs as
            // far as the watermark lets it.
            for seat in seats {
                let items = inbox.get_mut(&seat).unwrap();
                rng.shuffle(items);
                let take = rng.below(items.len() + 1);
                let peer = net.peers.get_mut(&seat).unwrap();
                for item in items.drain(..take).collect::<Vec<_>>() {
                    peer.receive(item).unwrap();
                }
                while ran[&seat] < now {
                    match peer.due(ran[&seat] + 1) {
                        Ok(_) => *ran.get_mut(&seat).unwrap() += 1,
                        Err(PeerError::Wait { .. }) => break,
                        Err(e) => panic!("seed {seed}, {seat}: {e}"),
                    }
                }
            }
        }

        // Quiet: the rest arrives and every machine runs on.
        for (seat, up) in std::mem::take(&mut to_host) {
            let out = net.host.receive(seat, up);
            deliver(out.items, &mut inbox);
        }
        let end = 120 + 2 * APPLY_DELAY;
        deliver(
            net.host.tick(end).unwrap().into_iter().collect(),
            &mut inbox,
        );
        for seat in seats {
            let peer = net.peers.get_mut(&seat).unwrap();
            for item in inbox.remove(&seat).unwrap_or_default() {
                peer.receive(item).unwrap();
            }
            peer.due(end).unwrap();
        }
        assert!(accepted_n > 20, "seed {seed}: only {accepted_n} accepted");
        rebased_total += rebased;
        let peers: Vec<&Peer> = net.peers.values().collect();
        assert_converged(&net.host, &peers);
        // Each author's entries are in its lane.
        for entry in net.host.state().log() {
            let seat = net
                .host
                .state()
                .members()
                .iter()
                .find(|(_, m)| m.name == entry.author)
                .map(|(s, _)| *s)
                .unwrap();
            assert!(entry.op.edit == Edit::Undo || lane(seat, entry.op.id, 0));
        }
    }
    // Proposals made at an older head were taken onto the newer one.
    assert!(rebased_total > 0);
}

#[test]
fn a_second_hold_is_refused() {
    let mut net = editors();
    net.run(1);
    let id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 1, add(id, 0.))));
    net.run(APPLY_DELAY);
    let thing = Endpoint::Added(id);

    accepted(net.up(
        ALPHA,
        Up::Hold {
            nonce: 2,
            target: thing,
        },
    ));
    assert_eq!(
        refused(net.up(
            BRAVO,
            Up::Hold {
                nonce: 3,
                target: thing
            }
        )),
        NetRefusal::Held { by: "Alpha".into() }
    );
    // Nor can Bravo edit it, by identity or by its block's index.
    let bravo_id = net.peers.get_mut(&BRAVO).unwrap().op_ids(1).unwrap()[0];
    assert_eq!(
        refused(net.up(
            BRAVO,
            propose(net.peer(BRAVO), 4, move_to(bravo_id, thing, 5.))
        )),
        NetRefusal::Held { by: "Alpha".into() }
    );
    let by_index = Op {
        id: bravo_id,
        edit: Edit::Placement(Placement::Set {
            block: BLOCK,
            index: 7,
            write: position(5.),
        }),
    };
    assert_eq!(
        refused(net.up(BRAVO, propose(net.peer(BRAVO), 5, by_index))),
        NetRefusal::Held { by: "Alpha".into() }
    );

    // Alpha carries it; a second carry for the same apply tick is dropped.
    let carry = |x| Up::Carry {
        target: thing,
        frame: Carry::new(frame(x)).unwrap(),
    };
    let first = net.host.receive(ALPHA, carry(1.)).items;
    assert_eq!(first.len(), 1);
    net.send(first);
    assert!(net.host.receive(ALPHA, carry(2.)).items.is_empty());
    // Out of bounds is refused.
    assert_eq!(
        net.host
            .carry(ALPHA, thing, Carry::new(frame(99.)).unwrap())
            .map(|_| ()),
        Err(NetRefusal::Doc(Refusal::Invalid {
            what: "position".into(),
            reason: "outside the map's world bounds".into(),
        }))
    );
    // Bravo cannot carry what it does not hold.
    assert_eq!(
        net.host.carry(BRAVO, thing, Carry::new(frame(1.)).unwrap()),
        Err(NetRefusal::NotHolding)
    );

    // The drop is Alpha's edit, then the release.
    let drop_id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(
        ALPHA,
        propose(net.peer(ALPHA), 6, move_to(drop_id, thing, 1.)),
    ));
    net.up(ALPHA, Up::Release);
    net.run(APPLY_DELAY + 1);
    assert!(net.host.may_rebuild());
    accepted(net.up(
        BRAVO,
        Up::Hold {
            nonce: 7,
            target: thing,
        },
    ));
    assert!(!net.host.may_rebuild());
    net.run(APPLY_DELAY + 1);
    assert_eq!(net.peer(HOST).state().holds()[&BRAVO].target, thing);
    assert_converged(&net.host, &net.peers.values().collect::<Vec<_>>());
}

#[test]
fn roles_limit_who_edits() {
    let mut net = Net::new(
        DocOnly,
        &[
            (HOST, "Host", Role::Editor),
            (ALPHA, "Alpha", Role::Editor),
            (BRAVO, "Bravo", Role::Spectator),
        ],
    );
    net.run(1);
    let id = net.peers.get_mut(&BRAVO).unwrap().op_ids(1).unwrap()[0];
    assert_eq!(
        refused(net.up(BRAVO, propose(net.peer(BRAVO), 1, add(id, 0.)))),
        NetRefusal::Role {
            role: Role::Spectator
        }
    );
    let id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 2, add(id, 0.))));
    net.run(APPLY_DELAY);
    let thing = Endpoint::Added(id);
    assert_eq!(
        refused(net.up(
            BRAVO,
            Up::Hold {
                nonce: 3,
                target: thing
            }
        )),
        NetRefusal::Role {
            role: Role::Spectator
        }
    );
    accepted(net.up(
        ALPHA,
        Up::Hold {
            nonce: 4,
            target: thing,
        },
    ));
    // Made a player, Alpha lets go and can no longer edit.
    let item = net.host.set_role(ALPHA, Role::Player).unwrap();
    net.send(vec![item]);
    net.run(APPLY_DELAY + 1);
    assert!(net.peer(BRAVO).state().holds().is_empty());
    assert_eq!(net.peer(BRAVO).state().members()[&ALPHA].role, Role::Player);
    let id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    assert_eq!(
        refused(net.up(ALPHA, propose(net.peer(ALPHA), 5, add(id, 1.)))),
        NetRefusal::Role { role: Role::Player }
    );
    // A seat that never joined, a taken seat and a taken name.
    assert_eq!(
        refused(net.up(
            Seat(9),
            Up::Hold {
                nonce: 6,
                target: thing
            }
        )),
        NetRefusal::NotJoined
    );
    assert_eq!(
        net.host.join(ALPHA, "Other", Role::Editor).map(|_| ()),
        Err(NetRefusal::Seat)
    );
    assert_eq!(
        net.host.join(Seat(5), "alpha", Role::Editor).map(|_| ()),
        Err(NetRefusal::Name)
    );
    assert_eq!(
        net.host.join(Seat(SEATS), "Zulu", Role::Editor).map(|_| ()),
        Err(NetRefusal::Seat)
    );
    // Leaving ends the seat's hold.
    let item = net.host.set_role(ALPHA, Role::Editor).unwrap();
    net.send(vec![item]);
    accepted(net.up(
        ALPHA,
        Up::Hold {
            nonce: 7,
            target: thing,
        },
    ));
    let item = net.host.leave(ALPHA).unwrap();
    net.send(vec![item]);
    net.run(APPLY_DELAY + 1);
    assert!(net.host.state().holds().is_empty());
    assert!(!net.peer(HOST).state().members().contains_key(&ALPHA));
    let peers: Vec<&Peer> = net.peers.values().collect();
    assert_converged(&net.host, &peers);
}

#[test]
fn an_edit_by_index_from_an_older_head_is_stale() {
    let mut net = editors();
    net.run(1);
    let a = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 1, add(a, 0.))));
    net.run(APPLY_DELAY);
    // Bravo proposes at head 1 by index and by identity; Alpha's add lands
    // first.
    let ids = net.peers.get_mut(&BRAVO).unwrap().op_ids(2).unwrap();
    let (by_id, by_index) = (ids[0], ids[1]);
    let at_one = propose(net.peer(BRAVO), 2, move_to(by_id, Endpoint::Added(a), 4.));
    let index_at_one = propose(
        net.peer(BRAVO),
        3,
        Op {
            id: by_index,
            edit: Edit::Placement(Placement::Set {
                block: BLOCK,
                index: 7,
                write: position(4.),
            }),
        },
    );
    let a2 = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 4, add(a2, 1.))));
    assert_eq!(
        refused(net.up(BRAVO, index_at_one)),
        NetRefusal::Stale {
            proposed: 1,
            head: 2
        }
    );
    accepted(net.up(BRAVO, at_one));
    // A position the host has not reached, and an ID in another lane.
    let mut ahead = propose(net.peer(BRAVO), 5, add(OpId(99), 0.));
    if let Up::Propose { proposal, .. } = &mut ahead {
        proposal.base_seq = 50;
    }
    assert_eq!(
        refused(net.up(BRAVO, ahead)),
        NetRefusal::Ahead {
            proposed: 50,
            head: 3
        }
    );
    let alpha_lane = next_in_lane(ALPHA, 1000).unwrap();
    assert_eq!(
        refused(net.up(BRAVO, propose(net.peer(BRAVO), 6, add(alpha_lane, 0.)))),
        NetRefusal::Lane { id: alpha_lane }
    );
    net.run(APPLY_DELAY + 1);
    assert_converged(&net.host, &net.peers.values().collect::<Vec<_>>());
}

#[test]
fn undo_follows_the_whole_log_rules() {
    let mut net = editors();
    net.run(1);
    let a = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 1, add(a, 0.))));
    net.run(APPLY_DELAY);
    // Bravo cannot undo Alpha's edit.
    assert_eq!(
        refused(net.up(BRAVO, propose(net.peer(BRAVO), 2, undo(a)))),
        NetRefusal::Doc(Refusal::UndoAuthor(a))
    );
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 3, undo(a))));
    net.run(APPLY_DELAY);
    // An undone op's ID is not free for another op, even to its author:
    // a fold of the whole log would refuse it.
    assert_eq!(
        refused(net.up(ALPHA, propose(net.peer(ALPHA), 4, add(a, 9.)))),
        NetRefusal::Doc(Refusal::DuplicateOp(a))
    );
    // The same op again is a redo.
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 5, add(a, 0.))));
    net.run(APPLY_DELAY);
    assert_converged(&net.host, &net.peers.values().collect::<Vec<_>>());
}

/// The admitter of a game that keeps each added thing's position.
struct Positions;

impl Admit for Positions {
    fn holds(&self, doc: &ForgeDoc, op: &Op) -> Result<Option<Sha256>, String> {
        let Edit::Entry(EntryOp::Set {
            target: Endpoint::Added(id),
            write,
        }) = &op.edit
        else {
            return Ok(None);
        };
        let mut value = None;
        for seen in &doc.ops {
            match &seen.edit {
                Edit::Placement(Placement::Add { fields, .. }) if seen.id == *id => {
                    value = fields.iter().find(|f| f.field == write.field).cloned();
                }
                Edit::Entry(EntryOp::Set {
                    target: Endpoint::Added(t),
                    write: w,
                }) if t == id && w.field == write.field => value = Some(w.clone()),
                _ => {}
            }
        }
        value
            .map(|w| w.value.digest().map_err(|e| e.to_string()))
            .transpose()
    }

    fn admit(&self, doc: &ForgeDoc) -> Result<(), String> {
        let adds = doc
            .ops
            .iter()
            .filter(|op| matches!(op.edit, Edit::Placement(Placement::Add { .. })))
            .count();
        if adds > 2 {
            Err("The map's Forge budget is spent.".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn was_is_checked_against_the_host_after_a_rebase() {
    let mut net = Net::new(
        Positions,
        &[
            (HOST, "Host", Role::Editor),
            (ALPHA, "Alpha", Role::Editor),
            (BRAVO, "Bravo", Role::Editor),
        ],
    );
    net.run(1);
    let a = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 1, add(a, 0.))));
    net.run(APPLY_DELAY);
    let thing = Endpoint::Added(a);
    let saw = position(0.).value.digest().unwrap();
    let with_was = |peer: &Peer, nonce, id, x| Up::Propose {
        nonce,
        proposal: Proposal {
            author: "anyone".into(),
            base_seq: peer.state().head().seq,
            op: move_to(id, thing, x),
            was: Some(saw),
        },
    };
    let b = net.peers.get_mut(&BRAVO).unwrap().op_ids(1).unwrap()[0];
    let bravo = with_was(net.peer(BRAVO), 2, b, 5.);
    let a2 = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, with_was(net.peer(ALPHA), 3, a2, 3.)));
    // Bravo saw the old position: the host now holds Alpha's.
    assert_eq!(
        refused(net.up(BRAVO, bravo)),
        NetRefusal::Doc(Refusal::Mismatch)
    );
    // The map's own check refuses past its budget.
    let ids = net.peers.get_mut(&BRAVO).unwrap().op_ids(2).unwrap();
    accepted(net.up(BRAVO, propose(net.peer(BRAVO), 4, add(ids[0], 1.))));
    assert_eq!(
        refused(net.up(BRAVO, propose(net.peer(BRAVO), 5, add(ids[1], 2.)))),
        NetRefusal::Map("The map's Forge budget is spent.".into())
    );
    net.run(APPLY_DELAY + 1);
    assert_converged(&net.host, &net.peers.values().collect::<Vec<_>>());
}

#[test]
fn a_late_joiners_document_equals_the_hosts() {
    let mut net = Net::new(
        DocOnly,
        &[(HOST, "Host", Role::Editor), (ALPHA, "Alpha", Role::Editor)],
    );
    net.run(1);
    let mut last = None;
    for x in 0..6 {
        let id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
        accepted(net.up(ALPHA, propose(net.peer(ALPHA), x, add(id, x as f32))));
        if x == 3 {
            let undo_last = undo(id);
            accepted(net.up(ALPHA, propose(net.peer(ALPHA), 99, undo_last)));
        }
        last = Some(id);
        net.run(2);
    }
    accepted(net.up(
        ALPHA,
        Up::Hold {
            nonce: 50,
            target: Endpoint::Added(last.unwrap()),
        },
    ));
    // Bravo joins with entries still in flight: some applied, some not.
    let pending_before = net.host.state().n() - net.host.applied().n();
    assert!(pending_before > 0);
    net.join(BRAVO, "Bravo", Role::Editor);
    assert!(net.peer(BRAVO).state().head().seq < net.host.state().head().seq);
    net.run(APPLY_DELAY + 1);
    assert_eq!(net.peer(BRAVO).state().doc(), net.host.state().doc());
    assert_eq!(
        net.peer(BRAVO).state().holds()[&ALPHA].target,
        Endpoint::Added(last.unwrap())
    );
    // Bravo edits at once, in its own lane.
    let id = net.peers.get_mut(&BRAVO).unwrap().op_ids(1).unwrap()[0];
    assert!(lane(BRAVO, id, 0));
    accepted(net.up(BRAVO, propose(net.peer(BRAVO), 60, add(id, 7.))));
    net.run(APPLY_DELAY + 1);
    assert_converged(&net.host, &net.peers.values().collect::<Vec<_>>());
}

#[test]
fn a_welcome_with_a_changed_entry_is_refused() {
    let mut net = editors();
    net.run(1);
    let id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 1, add(id, 0.))));
    net.run(APPLY_DELAY + 1);
    let (mut welcome, _) = net.host.join(Seat(3), "Charlie", Role::Player).unwrap();
    welcome.entries[0].op = add(id, 5.);
    assert!(matches!(
        Peer::join(welcome, limits()),
        Err(PeerError::Invalid { .. })
    ));
}

#[test]
fn a_head_that_differs_is_named() {
    let mut net = editors();
    net.run(1);
    let id = net.peers.get_mut(&ALPHA).unwrap().op_ids(1).unwrap()[0];
    accepted(net.up(ALPHA, propose(net.peer(ALPHA), 1, add(id, 0.))));
    net.run(APPLY_DELAY + 1);
    let mut beat = net.peer(BRAVO).beat();
    net.host.check(BRAVO, beat).unwrap();
    beat.head.hash = Sha256::of(b"another log");
    assert_eq!(
        net.host.check(BRAVO, beat),
        Err(NetRefusal::HeadDiffers {
            seat: BRAVO,
            seq: 1
        })
    );
    beat.head.seq = 7;
    assert_eq!(
        net.host.check(BRAVO, beat),
        Err(NetRefusal::HeadDiffers {
            seat: BRAVO,
            seq: 7
        })
    );
}

/// A film keeps the session's welcome and every stream item as bytes;
/// playback is a peer fed the same items, which reaches the same head.
#[test]
fn a_recorded_stream_replays_to_the_same_head() {
    let mut host = Host::new(start(), limits(), DocOnly, 0, APPLY_DELAY).unwrap();
    let (welcome, join) = host.join(ALPHA, "Alpha", Role::Editor).unwrap();
    let film_start = welcome.to_bytes().unwrap();
    let mut film: Vec<Vec<u8>> = vec![join.to_bytes().unwrap()];
    let mut live = Peer::join(welcome, limits()).unwrap();
    live.receive(join).unwrap();
    let mut above = 0;
    for now in 1..=12 {
        let mut items: Vec<Stamped> = host.tick(now).unwrap().into_iter().collect();
        if now % 3 == 0 {
            let id = next_in_lane(ALPHA, above).unwrap();
            above = id.0;
            let up = Up::Propose {
                nonce: now,
                proposal: Proposal {
                    author: "Alpha".into(),
                    base_seq: host.state().head().seq,
                    op: add(id, now as f32),
                    was: None,
                },
            };
            items.extend(host.receive(ALPHA, up).items);
        }
        for item in items {
            film.push(item.to_bytes().unwrap());
            live.receive(item).unwrap();
        }
        live.due(now).unwrap();
    }
    let mut playback = Peer::join(Welcome::from_bytes(&film_start).unwrap(), limits()).unwrap();
    for bytes in &film {
        playback
            .receive(Stamped::from_bytes(bytes).unwrap())
            .unwrap();
    }
    playback.due(12).unwrap();
    assert_eq!(playback.state(), live.state());
    assert_eq!(playback.state().head().seq, 2);
}

#[test]
fn messages_are_bounded() {
    let big = vec![b' '; MAX_MESSAGE_BYTES + 1];
    assert!(matches!(Up::from_bytes(&big), Err(Refusal::Limit { .. })));
    assert!(matches!(
        Stamped::from_bytes(&big),
        Err(Refusal::Limit { .. })
    ));
    assert!(matches!(
        Up::from_bytes(b"{\"release\":1}"),
        Err(Refusal::Json(_))
    ));
    let up = Up::Release;
    let bytes = serde_json::to_vec(&up).unwrap();
    assert_eq!(Up::from_bytes(&bytes).unwrap(), up);
}
