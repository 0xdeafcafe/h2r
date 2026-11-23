//! The node over a fake transport and a fake game: invented map, tag and
//! players, no clock, socket or engine. Messages cross in-memory pipes that
//! may hand bytes over a few at a time.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use forge_doc::{
    BuilderVersions, Edit, Endpoint, EntryOp, FieldWrite, ForgeDoc, Limits, MapRef, Op, OpId,
    Placement, Refusal, Sha256, TagKey, Value, WorldBounds,
};

use super::film::{self, Replay};
use super::*;
use crate::things::frame_writes;
use crate::{PaletteItem, Session, SessionAdmit, World};

const BLOCK: BlockKind = BlockKind::Crates;

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

fn tag() -> TagKey {
    TagKey {
        group: "bloc".into(),
        path: r"fixture\cube".into(),
    }
}

fn session(budget: usize) -> Session {
    Session::new(
        start(),
        limits(),
        vec![PaletteItem {
            block: BLOCK,
            tag: tag(),
        }],
        budget,
    )
    .unwrap()
}

fn at(x: f32) -> Frame {
    Frame {
        position: [x, 1., 2.],
        rotation: [0., 0., 0.],
    }
}

/// An add by hand, for what a session's own checks would stop.
fn add(id: u32, x: f32) -> Op {
    let mut fields = vec![FieldWrite {
        field: "palette index".into(),
        value: Value::Tag(Some(tag())),
    }];
    fields.extend(frame_writes(BLOCK, at(x)).unwrap());
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block: BLOCK,
            like: None,
            fields,
        }),
    }
}

#[derive(Default)]
struct FakeWorld {
    next: u32,
    objects: BTreeMap<u32, Frame>,
}

impl World for FakeWorld {
    fn place(&mut self, _: &PaletteItem, f: Frame) -> Result<u32, String> {
        self.next += 1;
        self.objects.insert(self.next, f);
        Ok(self.next)
    }
    fn respawn(&mut self, item: &PaletteItem, f: Frame) -> Result<u32, String> {
        self.place(item, f)
    }
    fn move_object(&mut self, h: u32, f: Frame) -> Result<(), String> {
        *self.objects.get_mut(&h).ok_or("stale handle")? = f;
        Ok(())
    }
    fn delete(&mut self, h: u32) -> Result<(), String> {
        self.objects.remove(&h).map(|_| ()).ok_or("stale handle".into())
    }
}

#[derive(Default)]
struct Pipe {
    bytes: Vec<u8>,
    beat: Option<Beat>,
}

/// One end of a reliable ordered pipe pair. With a `chop`, every other
/// `recv` holds back the last `chop` bytes, so messages arrive in pieces;
/// `cap` bounds the bytes queued.
#[derive(Clone)]
struct FakeLane {
    out: Rc<RefCell<Pipe>>,
    inn: Rc<RefCell<Pipe>>,
    chop: usize,
    cap: usize,
    reads: u32,
}

fn pair(chop: usize) -> (FakeLane, FakeLane) {
    let (a, b) = (Rc::default(), Rc::default());
    let lane = |out: &Rc<RefCell<Pipe>>, inn: &Rc<RefCell<Pipe>>| FakeLane {
        out: out.clone(),
        inn: inn.clone(),
        chop,
        cap: usize::MAX,
        reads: 0,
    };
    (lane(&a, &b), lane(&b, &a))
}

impl Lane for FakeLane {
    fn send(&mut self, bytes: &[u8]) -> bool {
        let mut pipe = self.out.borrow_mut();
        if pipe.bytes.len() + bytes.len() > self.cap {
            return false;
        }
        pipe.bytes.extend_from_slice(bytes);
        true
    }
    fn recv(&mut self) -> Vec<u8> {
        let mut pipe = self.inn.borrow_mut();
        let held_back = if self.reads % 2 == 0 && self.chop != usize::MAX {
            self.chop
        } else {
            0
        };
        self.reads += 1;
        let n = pipe.bytes.len().saturating_sub(held_back);
        pipe.bytes.drain(..n).collect()
    }
    fn set_beat(&mut self, beat: Option<Beat>) {
        self.out.borrow_mut().beat = beat;
    }
    fn beat(&self) -> Option<Beat> {
        self.inn.borrow().beat
    }
}

/// A machine: its node, its session and its world, and what it applied.
struct Machine<A: Admit> {
    node: Node<A, FakeLane>,
    session: Session,
    world: FakeWorld,
    name: &'static str,
    applied: Vec<Stamped>,
    /// The tick each applied item was applied at.
    applied_at: Vec<u32>,
}

impl<A: Admit> Machine<A> {
    fn step(&mut self, tick: u32) -> Result<Step, NodeError> {
        let out = self.session.take_outgoing();
        if !out.is_empty() {
            self.node.submit(out)?;
        }
        let step = self.node.tick(tick)?;
        if let Some(state) = &step.joined {
            let seat = self.node.seat().unwrap();
            self.session = self.session.clone().share(seat, self.name);
            self.session.sync_to(&mut self.world, state).unwrap();
        }
        for item in &step.items {
            self.session.apply_item(&mut self.world, item).unwrap();
            self.applied.push(item.clone());
            self.applied_at.push(tick);
        }
        Ok(step)
    }

    fn doc(&self) -> &ForgeDoc {
        self.session.document()
    }

    fn objects(&self) -> Vec<(OpId, Frame)> {
        let mut objects: Vec<_> = self
            .session
            .objects()
            .iter()
            .map(|o| (o.id, o.frame))
            .collect();
        objects.sort_by_key(|(id, _)| id.0);
        objects
    }
}

type Guest = Machine<DocOnly>;
type Hosted = Machine<SessionAdmit>;

fn hosted(budget: usize) -> Hosted {
    let session = session(budget).share(HOST_SEAT, "Host");
    let node = Node::host(
        session.document().clone(),
        limits(),
        session.admit(),
        "Host",
        0,
        APPLY_DELAY,
    )
    .unwrap();
    Machine {
        node,
        session,
        world: FakeWorld::default(),
        name: "Host",
        applied: vec![],
        applied_at: vec![],
    }
}

/// A guest on its own lane, accepted by `host` as `seat`.
fn joined(
    host: &mut Hosted,
    seat: Seat,
    name: &'static str,
    role: Role,
    chop: usize,
) -> (Guest, FakeLane) {
    let (to_host, to_guest) = pair(chop);
    host.node.accept(seat, to_guest, role).unwrap();
    let node = Node::guest(limits(), name, CanDraw::default(), to_host.clone()).unwrap();
    (
        Machine {
            node,
            session: session(10),
            world: FakeWorld::default(),
            name,
            applied: vec![],
            applied_at: vec![],
        },
        to_host,
    )
}

/// Run every machine through ticks `from..=to`, the host first.
fn run(host: &mut Hosted, guests: &mut [&mut Guest], from: u32, to: u32) {
    for tick in from..=to {
        host.step(tick).unwrap();
        for guest in guests.iter_mut() {
            guest.step(tick).unwrap();
        }
    }
}

fn settled(node: &mut Node<impl Admit, FakeLane>) -> Vec<Settled> {
    node.take_settled()
}

/// Every machine holds the host's document, head, log and objects, and so
/// does one fold of the whole log.
fn assert_converged(host: &Hosted, guests: &[&Guest]) {
    let state = host.node.state().unwrap();
    let (whole, head) =
        forge_doc::fold_from(state.start(), state.start_head(), state.log(), &limits()).unwrap();
    assert_eq!(&whole, state.doc());
    assert_eq!(head, state.head());
    assert_eq!(host.doc(), state.doc());
    for guest in guests {
        let theirs = guest.node.state().unwrap();
        assert_eq!(theirs.doc(), state.doc(), "{}", guest.name);
        assert_eq!(theirs.head(), state.head(), "{}", guest.name);
        assert_eq!(guest.doc(), state.doc(), "{}", guest.name);
        assert_eq!(guest.session.head(), state.head(), "{}", guest.name);
        assert_eq!(guest.objects(), host.objects(), "{}", guest.name);
        // A late joiner starts from the document, not from the items before it.
        assert!(host.applied.ends_with(&guest.applied), "{}", guest.name);
        assert_eq!(theirs.members(), state.members(), "{}", guest.name);
        assert_eq!(theirs.holds(), state.holds(), "{}", guest.name);
    }
}

fn both_modes(f: impl Fn(usize)) {
    // Whole messages, and a few bytes at a time.
    for chop in [usize::MAX, 7] {
        f(chop);
    }
}

#[test]
fn edits_made_on_every_machine_end_at_one_document() {
    both_modes(|chop| {
        let mut host = hosted(10);
        let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, chop);
        let (mut bravo, _) = joined(&mut host, Seat(2), "Bravo", Role::Editor, chop);
        run(&mut host, &mut [&mut alpha, &mut bravo], 1, 12);
        assert!(alpha.node.state().is_some() && bravo.node.state().is_some());

        // Each seat places something in the same tick, from the same head.
        let ids = [
            host.session.place(&mut host.world, 12, 0, at(1.)).unwrap(),
            alpha.session.place(&mut alpha.world, 12, 0, at(2.)).unwrap(),
            bravo.session.place(&mut bravo.world, 12, 0, at(3.)).unwrap(),
        ];
        // Nothing is applied where it is made.
        assert!(host.session.objects().is_empty());
        run(&mut host, &mut [&mut alpha, &mut bravo], 13, 30);

        assert_eq!(host.session.objects().len(), 3);
        assert_converged(&host, &[&alpha, &bravo]);
        // A seat's new ops are in its lane: `seat + 1`, then 16 on.
        for (id, seat) in ids.iter().zip(0u32..) {
            assert_eq!((id.0 - 1) % 16, seat, "{id:?}");
        }
        // And every one of them reached the log, by its own author.
        let authors: Vec<_> = host
            .node
            .state()
            .unwrap()
            .log()
            .iter()
            .map(|e| e.author.as_str())
            .collect();
        for name in ["Host", "Alpha", "Bravo"] {
            assert!(authors.contains(&name), "{authors:?}");
        }
    });
}

#[test]
fn a_proposal_settles_with_its_entry_and_a_refusal_with_its_code() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    let (mut watcher, _) = joined(&mut host, Seat(2), "Watcher", Role::Player, usize::MAX);
    run(&mut host, &mut [&mut alpha, &mut watcher], 1, 12);

    // An editor's edit comes back as its entry, once it is applied.
    let nonce = alpha.node.propose(add(2, 1.), None).unwrap();
    assert_eq!(alpha.node.in_flight(), 1);
    run(&mut host, &mut [&mut alpha, &mut watcher], 13, 24);
    let done = settled(&mut alpha.node);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].nonce, nonce);
    let entry = done[0].result.clone().unwrap().unwrap();
    assert_eq!((entry.seq, entry.author.as_str()), (1, "Alpha"));
    assert_eq!(alpha.node.in_flight(), 0);

    // A player's is refused by role, and nothing changes.
    let nonce = watcher.node.propose(add(3, 2.), None).unwrap();
    run(&mut host, &mut [&mut alpha, &mut watcher], 25, 36);
    let done = settled(&mut watcher.node);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].nonce, nonce);
    let why = done[0].result.clone().unwrap_err();
    assert!(why.starts_with("forge_role: "), "{why}");
    assert_eq!(host.node.state().unwrap().log().len(), 1);

    // An op outside the seat's lane is refused by name.
    alpha.node.propose(add(3, 3.), None).unwrap();
    run(&mut host, &mut [&mut alpha, &mut watcher], 37, 48);
    let why = settled(&mut alpha.node)[0].result.clone().unwrap_err();
    assert!(why.starts_with("forge_lane: "), "{why}");
    assert_converged(&host, &[&alpha, &watcher]);
}

#[test]
fn the_hosts_own_proposals_take_the_same_road() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha], 1, 12);
    let nonce = host.node.propose(add(1, 1.), None).unwrap();
    run(&mut host, &mut [&mut alpha], 13, 24);
    let done = settled(&mut host.node);
    assert_eq!(done[0].nonce, nonce);
    assert!(done[0].result.clone().unwrap().is_some());
    assert_converged(&host, &[&alpha]);
    // Its own lane is seat 0's.
    assert_eq!(host.session.lane_ids(2).unwrap(), [OpId(17), OpId(33)]);
    assert_eq!(alpha.session.lane_ids(1).unwrap(), [OpId(2)]);
}

#[test]
fn the_games_checks_run_on_the_host_over_the_sessions_map() {
    // A budget of one: the second add leaves a document the map refuses.
    let mut host = hosted(1);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha], 1, 12);
    alpha.node.propose(add(2, 1.), None).unwrap();
    run(&mut host, &mut [&mut alpha], 13, 24);
    assert!(settled(&mut alpha.node)[0].result.is_ok());
    alpha.node.propose(add(18, 2.), None).unwrap();
    run(&mut host, &mut [&mut alpha], 25, 36);
    let why = settled(&mut alpha.node)[0].result.clone().unwrap_err();
    assert!(why.starts_with("forge_map: "), "{why}");
    assert_eq!(host.session.objects().len(), 1);
    assert_converged(&host, &[&alpha]);

    // A session's own tool refuses it before the host is asked.
    let refused = host.session.place(&mut host.world, 36, 0, at(3.));
    assert!(refused.is_err());
    assert!(host.session.take_outgoing().is_empty());
}

#[test]
fn a_held_thing_is_carried_on_every_machine_and_dropped_as_one_edit() {
    both_modes(|chop| {
        let mut host = hosted(10);
        let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, chop);
        let (mut bravo, _) = joined(&mut host, Seat(2), "Bravo", Role::Editor, chop);
        run(&mut host, &mut [&mut alpha, &mut bravo], 1, 12);
        let id = host.session.place(&mut host.world, 12, 0, at(1.)).unwrap();
        run(&mut host, &mut [&mut alpha, &mut bravo], 13, 24);
        let target = Endpoint::Added(id);

        alpha.node.hold(target).unwrap();
        run(&mut host, &mut [&mut alpha, &mut bravo], 25, 32);
        assert!(settled(&mut alpha.node)[0].result.is_ok());
        assert_eq!(host.node.state().unwrap().holds().len(), 1);

        // A second hold, and an edit of the held thing, are refused.
        bravo.node.hold(target).unwrap();
        bravo.node.propose(
            Op {
                id: OpId(3),
                edit: Edit::Entry(EntryOp::Remove { target }),
            },
            None,
        )
        .unwrap();
        run(&mut host, &mut [&mut alpha, &mut bravo], 33, 44);
        let refusals: Vec<_> = settled(&mut bravo.node)
            .into_iter()
            .map(|s| s.result.unwrap_err())
            .collect();
        assert_eq!(refusals.len(), 2);
        assert!(refusals.iter().all(|r| r.starts_with("forge_held: Alpha")), "{refusals:?}");

        // The carry moves the thing in every world, and no document.
        alpha.node.carry(target, at(5.)).unwrap();
        run(&mut host, &mut [&mut alpha, &mut bravo], 45, 56);
        let log_before = host.node.state().unwrap().log().len();
        for machine in [&host.objects(), &alpha.objects(), &bravo.objects()] {
            assert_eq!(machine, &vec![(id, at(5.))]);
        }
        assert_eq!(host.node.state().unwrap().log().len(), log_before);

        // The drop is the holder's edit, then a release.
        alpha
            .session
            .move_thing(&mut alpha.world, 56, target, at(6.))
            .unwrap();
        alpha.node.release().unwrap();
        run(&mut host, &mut [&mut alpha, &mut bravo], 57, 70);
        assert!(host.node.state().unwrap().holds().is_empty());
        assert_eq!(host.objects(), vec![(id, at(6.))]);
        assert_converged(&host, &[&alpha, &bravo]);
    });
}

#[test]
fn undo_is_the_authors_and_the_log_only_grows() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha], 1, 12);
    host.session.place(&mut host.world, 12, 0, at(1.)).unwrap();
    run(&mut host, &mut [&mut alpha], 13, 24);
    // Another seat's edit is not Alpha's to undo, and the session says so
    // before it asks the host.
    let err = alpha.session.undo(&mut alpha.world, 24).unwrap_err();
    assert!(err.contains("not yours"), "{err}");
    assert!(alpha.session.take_outgoing().is_empty());

    alpha.session.place(&mut alpha.world, 24, 0, at(2.)).unwrap();
    run(&mut host, &mut [&mut alpha], 25, 36);
    assert_eq!(host.session.objects().len(), 2);
    alpha.session.undo(&mut alpha.world, 36).unwrap();
    run(&mut host, &mut [&mut alpha], 37, 48);
    assert_eq!(host.session.objects().len(), 1);
    assert_eq!(host.node.state().unwrap().log().len(), 3);
    assert_converged(&host, &[&alpha]);
    // Redo is the same op proposed again.
    assert_eq!(
        alpha.session.redo(&mut alpha.world, 48).unwrap_err(),
        crate::SHARED_REDO
    );
}

#[test]
fn a_late_joiner_is_given_the_document_and_the_items_in_flight() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha], 1, 12);
    host.session.place(&mut host.world, 12, 0, at(1.)).unwrap();
    alpha.session.place(&mut alpha.world, 12, 0, at(2.)).unwrap();
    run(&mut host, &mut [&mut alpha], 13, 24);
    // An edit still on its way when the third machine asks.
    alpha.session.place(&mut alpha.world, 24, 0, at(3.)).unwrap();
    run(&mut host, &mut [&mut alpha], 25, 26);
    let (mut carol, _) = joined(&mut host, Seat(3), "Carol", Role::Editor, 5);
    run(&mut host, &mut [&mut alpha, &mut carol], 27, 50);
    assert_eq!(carol.session.objects().len(), 3);
    assert_converged(&host, &[&alpha, &carol]);
    assert_eq!(
        host.node.state().unwrap().members().keys().copied().collect::<Vec<_>>(),
        [Seat(0), Seat(1), Seat(3)]
    );
    // Her lane is her seat's.
    assert_eq!(carol.session.lane_ids(1).unwrap(), [OpId(20)]);
}

#[test]
fn a_seat_that_leaves_lets_go_of_what_it_held() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha], 1, 12);
    let id = host.session.place(&mut host.world, 12, 0, at(1.)).unwrap();
    run(&mut host, &mut [&mut alpha], 13, 24);
    alpha.node.hold(Endpoint::Added(id)).unwrap();
    run(&mut host, &mut [&mut alpha], 25, 32);
    assert_eq!(host.node.state().unwrap().holds().len(), 1);
    host.node.disconnect(Seat(1)).unwrap();
    run(&mut host, &mut [], 33, 44);
    assert!(host.node.state().unwrap().holds().is_empty());
    assert!(!host.node.state().unwrap().members().contains_key(&Seat(1)));
    // Its seat can be taken again.
    let (_again, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
}

#[test]
fn a_head_that_differs_ends_that_machines_session_and_only_that_ones() {
    let mut host = hosted(10);
    let (mut alpha, tap) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    let (mut bravo, _) = joined(&mut host, Seat(2), "Bravo", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha, &mut bravo], 1, 12);
    host.session.place(&mut host.world, 12, 0, at(1.)).unwrap();
    run(&mut host, &mut [&mut alpha, &mut bravo], 13, 24);
    assert!(host.node.report().members.len() == 3);

    // Alpha's keep-alive says another head at the position they share.
    let mut wrong = alpha.node.peer().unwrap().beat();
    wrong.head.hash = Sha256([9; 32]);
    tap.out.borrow_mut().beat = Some(wrong);
    host.step(25).unwrap();
    bravo.step(25).unwrap();
    let ended = alpha.step(25).unwrap_err();
    match ended {
        NodeError::Refused(NetRefusal::HeadDiffers { seat, seq }) => {
            assert_eq!((seat, seq), (Seat(1), wrong.head.seq));
        }
        other => panic!("{other:?}"),
    }
    // The ended machine stays ended; the others go on without its seat.
    assert!(matches!(alpha.step(26), Err(NodeError::Ended(_))));
    assert!(alpha.node.report().ended.is_some());
    run(&mut host, &mut [&mut bravo], 26, 40);
    assert!(!host.node.state().unwrap().members().contains_key(&Seat(1)));
    assert_converged(&host, &[&bravo]);
}

#[test]
fn a_lane_that_will_not_take_bytes_ends_its_seat() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha], 1, 12);
    // The queue to the guest is full: the host cannot tell it anything.
    let (to_host, mut to_guest) = pair(usize::MAX);
    to_guest.cap = 0;
    let mut stuck = Node::<DocOnly, _>::guest(limits(), "Stuck", to_host).unwrap();
    host.node.accept(Seat(2), to_guest, Role::Editor).unwrap();
    for tick in 13..=20 {
        host.step(tick).unwrap();
        let _ = stuck.tick(tick);
        alpha.step(tick).unwrap();
    }
    run(&mut host, &mut [&mut alpha], 21, 34);
    assert!(!host.node.state().unwrap().members().contains_key(&Seat(2)));
    assert_converged(&host, &[&alpha]);
}

#[test]
fn a_taken_seat_or_name_is_refused_and_the_refused_machine_hears_why() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    run(&mut host, &mut [&mut alpha], 1, 12);
    let (to_host, to_guest) = pair(usize::MAX);
    let (_, spare) = pair(usize::MAX);
    assert_eq!(
        host.node.accept(Seat(1), spare, Role::Editor),
        Err(NodeError::Refused(NetRefusal::Seat))
    );
    host.node.accept(Seat(2), to_guest, Role::Editor).unwrap();
    let mut twin = Node::<DocOnly, _>::guest(limits(), "alpha", to_host).unwrap();
    host.step(13).unwrap();
    alpha.step(13).unwrap();
    let err = twin.tick(13).unwrap_err();
    assert_eq!(err, NodeError::Refused(NetRefusal::Name));
    assert_converged(&host, &[&alpha]);
}

#[test]
fn a_recorded_session_replays_through_a_peer_to_the_same_state() {
    let mut host = hosted(10);
    let (mut alpha, _) = joined(&mut host, Seat(1), "Alpha", Role::Editor, usize::MAX);
    let welcome = film::welcome_payload(&host.node.take_welcome().unwrap()).unwrap();
    run(&mut host, &mut [&mut alpha], 1, 12);
    let id = host.session.place(&mut host.world, 12, 0, at(1.)).unwrap();
    alpha.session.place(&mut alpha.world, 12, 0, at(2.)).unwrap();
    run(&mut host, &mut [&mut alpha], 13, 24);
    alpha.node.hold(Endpoint::Added(id)).unwrap();
    run(&mut host, &mut [&mut alpha], 25, 28);
    alpha.node.carry(Endpoint::Added(id), at(4.)).unwrap();
    run(&mut host, &mut [&mut alpha], 29, 34);
    alpha
        .session
        .move_thing(&mut alpha.world, 34, Endpoint::Added(id), at(4.))
        .unwrap();
    alpha.node.release().unwrap();
    run(&mut host, &mut [&mut alpha], 35, 50);

    // The film: the welcome, then each item at the tick it was applied.
    let records: Vec<(u32, Vec<u8>)> = host
        .applied
        .iter()
        .zip(&host.applied_at)
        .filter_map(|(item, tick)| film::item_payload(item).unwrap().map(|p| (*tick, p)))
        .collect();
    assert_eq!(records.len(), host.applied.len(), "no watermark is applied");
    let mut replay = Replay::new(limits());
    replay.feed(&welcome).unwrap();
    let mut replayed = vec![];
    for tick in 1..=50 {
        for (_, payload) in records.iter().filter(|(at, _)| *at == tick) {
            replay.feed(payload).unwrap();
        }
        replayed.extend(replay.due(tick).unwrap());
    }
    assert_eq!(replayed, host.applied);
    let (theirs, ours) = (replay.peer().unwrap().state(), host.node.state().unwrap());
    assert_eq!(theirs.doc(), ours.doc());
    assert_eq!(theirs.head(), ours.head());
    assert_eq!(theirs.log(), ours.log());
    assert_eq!(theirs.members(), ours.members());
    assert_eq!(theirs.holds(), ours.holds());

    // The same items drive a fresh session to the live one's world.
    let mut world = FakeWorld::default();
    let mut shared = session(10);
    shared
        .sync_to(&mut world, replay.peer().unwrap().state())
        .unwrap();
    assert_eq!(shared.document(), host.doc());

    // Records out of order, or of no kind, are refused by name.
    let mut other = Replay::new(limits());
    assert!(matches!(other.feed(&records[0].1), Err(film::FilmError::Order(_))));
    other.feed(&welcome).unwrap();
    assert!(matches!(other.feed(&welcome), Err(film::FilmError::Order(_))));
    assert!(matches!(other.feed(&[9, 1]), Err(film::FilmError::Record(_))));
    assert!(matches!(other.feed(&[]), Err(film::FilmError::Record(_))));
}

#[test]
fn the_beat_survives_the_keep_alive_bytes() {
    let beat = Beat {
        head: forge_doc::LogHead {
            seq: 9,
            tick: 77,
            hash: Sha256::of(b"head"),
        },
        n: u64::MAX - 3,
    };
    assert_eq!(Beat::from_bytes(&beat.to_bytes()), beat);
}

#[test]
fn messages_are_cut_back_out_of_the_byte_stream_whole_and_in_order() {
    let messages = [
        ToHost::Hello { name: "Alpha".into() },
        ToHost::Up(Up::Release),
        ToHost::Up(Up::Hold {
            nonce: 3,
            target: Endpoint::Added(OpId(5)),
        }),
    ];
    let bytes: Vec<u8> = messages.iter().flat_map(|m| frame(m).unwrap()).collect();
    let mut framer = Framer::default();
    let mut got = vec![];
    for piece in bytes.chunks(5) {
        framer.push(piece);
        while let Some(message) = framer.pop::<ToHost>().unwrap() {
            got.push(message);
        }
    }
    assert_eq!(got, messages);
    assert_eq!(framer.held(), 0);

    // A length over the cap, and a body that is not a message, end the lane.
    let mut framer = Framer::default();
    framer.push(&(MAX_FRAME_BYTES as u32 + 1).to_le_bytes());
    assert!(matches!(framer.pop::<ToHost>(), Err(Refusal::Limit { .. })));
    let mut framer = Framer::default();
    framer.push(&3u32.to_le_bytes());
    framer.push(b"{}x");
    assert!(matches!(framer.pop::<ToHost>(), Err(Refusal::Json(_))));
}

#[test]
fn every_refusal_has_a_code_that_names_it() {
    let refusals = [
        NetRefusal::NotJoined,
        NetRefusal::Role { role: Role::Player },
        NetRefusal::Held { by: "A".into() },
        NetRefusal::NotHolding,
        NetRefusal::Stale { proposed: 1, head: 2 },
        NetRefusal::Ahead { proposed: 3, head: 2 },
        NetRefusal::Lane { id: OpId(3) },
        NetRefusal::Doc(Refusal::Mismatch),
        NetRefusal::Doc(Refusal::Print {
            block: BLOCK,
            index: 0,
        }),
        NetRefusal::Doc(Refusal::DuplicateOp(OpId(1))),
        NetRefusal::Map("x".into()),
        NetRefusal::Seat,
        NetRefusal::Name,
        NetRefusal::HeadDiffers { seat: Seat(1), seq: 1 },
        NetRefusal::Clock { now: 1, was: 2 },
    ];
    let codes: std::collections::BTreeSet<_> = refusals.iter().map(NetRefusal::code).collect();
    // Every refusal, each of `Doc`'s three among them, has its own code.
    assert_eq!(codes.len(), refusals.len());
    for refusal in &refusals {
        assert!(refusal.code().starts_with("forge_"));
        assert!(refusal.named().starts_with(&format!("{}: ", refusal.code())));
    }
}
