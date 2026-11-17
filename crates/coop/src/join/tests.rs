use super::*;

const SESSION: u32 = 0x5e55;
const NONCE: u32 = 7;
const IN_LEVEL: HostGame = HostGame {
    in_level: true,
    free_slot: Some(2),
};

/// Bytes that compress about as badly as a real snapshot might.
fn state(len: usize) -> Vec<u8> {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn plan() -> Plan {
    Plan {
        tick: 4321,
        slot: 2,
        level: "03a_oldmombasa".into(),
        bsp: 1,
        len: 5000,
        chunks: 5,
        sum: 0x1234_5678_9abc_def0,
    }
}

fn round_trip(msg: Msg) {
    let d = Datagram {
        session: SESSION,
        nonce: NONCE,
        msg,
    };
    let bytes = d.encode();
    assert!(Datagram::is_join(&bytes));
    assert_eq!(Datagram::decode(&bytes), Ok(d));
}

#[test]
fn every_message_round_trips() {
    round_trip(Msg::Request);
    round_trip(Msg::Accept(plan()));
    round_trip(Msg::Refuse(Refusal::Full));
    round_trip(Msg::Chunk {
        index: 9,
        data: state(CHUNK),
    });
    round_trip(Msg::Chunk {
        index: 0,
        data: vec![],
    });
    round_trip(Msg::Ack { next: 3 });
    round_trip(Msg::Ready { tick: 99 });
    round_trip(Msg::Abort(Abort::Restore));
}

#[test]
fn bad_datagrams_are_refused() {
    let good = Datagram {
        session: SESSION,
        nonce: NONCE,
        msg: Msg::Accept(plan()),
    }
    .encode();
    for n in 0..good.len() {
        assert_eq!(
            Datagram::decode(&good[..n]),
            Err(WireError::Malformed),
            "{n}"
        );
    }
    let mut long = good.clone();
    long.push(0);
    assert_eq!(Datagram::decode(&long), Err(WireError::Malformed));
    let mut old = good.clone();
    old[4] = 9;
    assert_eq!(Datagram::decode(&old), Err(WireError::Version(9)));
    // A pad datagram is not a join one.
    assert!(!Datagram::is_join(b"h2co\x01"));
    let mut kind = good;
    kind[9] = 0x40;
    assert_eq!(Datagram::decode(&kind), Err(WireError::Malformed));
}

#[test]
fn a_chunk_fits_the_relay_and_a_window_its_queue() {
    let d = Datagram {
        session: SESSION,
        nonce: NONCE,
        msg: Msg::Chunk {
            index: u32::MAX,
            data: state(CHUNK + 50),
        },
    };
    let bytes = d.encode();
    assert_eq!(bytes.len(), MAX_CHUNK_DATAGRAM);
    // The relay's cap (live-server MAX_RELAY_MESSAGE) and queue.
    const { assert!(MAX_CHUNK_DATAGRAM <= 1200) };
    const { assert!(WINDOW < 256) };
    // The whole game state block, uncompressed, is about 4100 chunks.
    assert_eq!(chunks_for(0x3f_e000), 4088);
}

/// Both ends and the messages between them. `drop(n)` loses the n-th
/// message either way.
struct Pair {
    host: Host,
    joiner: Joiner,
    now: u64,
    sent: usize,
    to_host: Vec<Msg>,
    to_joiner: Vec<Msg>,
    host_did: Vec<HostOut>,
    joiner_did: Vec<JoinOut>,
}

impl Pair {
    fn new(enabled: bool) -> Pair {
        let (joiner, out) = Joiner::start(0, NONCE);
        let mut p = Pair {
            host: Host::new(enabled),
            joiner,
            now: 0,
            sent: 0,
            to_host: vec![],
            to_joiner: vec![],
            host_did: vec![],
            joiner_did: vec![],
        };
        p.joiner_says(out, &|_, _| false);
        p
    }

    fn joiner_says(&mut self, out: Vec<JoinOut>, drop: &dyn Fn(usize, &Msg) -> bool) {
        for o in out {
            if let JoinOut::Send(m) = &o {
                self.sent += 1;
                if !drop(self.sent, m) {
                    // Through the wire, so the encoding is exercised too.
                    self.to_host.push(wire(m));
                }
            }
            self.joiner_did.push(o);
        }
    }

    fn host_says(&mut self, out: Vec<HostOut>, drop: &dyn Fn(usize, &Msg) -> bool) {
        for o in out {
            if let HostOut::Send(m) = &o {
                self.sent += 1;
                if !drop(self.sent, m) {
                    self.to_joiner.push(wire(m));
                }
            }
            self.host_did.push(o);
        }
    }

    /// Runs `ms` milliseconds in 10 ms steps. The host snapshots `state`
    /// when asked, the newcomer restores with `restore_ok`.
    fn run(
        &mut self,
        ms: u64,
        game: HostGame,
        snap: &[u8],
        restore_ok: bool,
        drop: &dyn Fn(usize, &Msg) -> bool,
    ) {
        let end = self.now + ms;
        while self.now < end {
            self.now += 10;
            for m in std::mem::take(&mut self.to_host) {
                let out = self.host.on_msg(self.now, NONCE, m, game);
                let snapshot = out
                    .iter()
                    .any(|o| matches!(o, HostOut::TakeSnapshot { .. }));
                self.host_says(out, drop);
                if snapshot {
                    let out =
                        self.host
                            .snapshot(self.now, 4321, "03a_oldmombasa", 1, snap.to_vec());
                    self.host_says(out, drop);
                }
            }
            for m in std::mem::take(&mut self.to_joiner) {
                let out = self.joiner.on_msg(self.now, NONCE, m);
                let restore = out.iter().any(|o| matches!(o, JoinOut::Restore { .. }));
                self.joiner_says(out, drop);
                if restore {
                    let out = self.joiner.restored(self.now, restore_ok);
                    self.joiner_says(out, drop);
                }
            }
            let out = self.host.poll(self.now);
            self.host_says(out, drop);
            let out = self.joiner.poll(self.now);
            self.joiner_says(out, drop);
        }
    }

    fn host_did(&self, f: impl Fn(&HostOut) -> bool) -> usize {
        self.host_did.iter().filter(|o| f(o)).count()
    }
}

fn wire(m: &Msg) -> Msg {
    let d = Datagram {
        session: SESSION,
        nonce: NONCE,
        msg: m.clone(),
    };
    Datagram::decode(&d.encode()).unwrap().msg
}

fn restored(p: &Pair) -> Option<Vec<u8>> {
    p.joiner_did.iter().find_map(|o| match o {
        JoinOut::Restore { state, .. } => Some(state.clone()),
        _ => None,
    })
}

#[test]
fn a_join_holds_the_host_sends_the_state_and_admits_on_the_next_tick() {
    let snap = state(300_000);
    let mut p = Pair::new(true);
    p.run(2_000, IN_LEVEL, &snap, true, &|_, _| false);
    assert_eq!(restored(&p), Some(snap));
    // The host held, snapshotted before the player existed, and admitted
    // it on the tick after the snapshot.
    let held: Vec<_> = p
        .host_did
        .iter()
        .filter(|o| !matches!(o, HostOut::Send(_)))
        .cloned()
        .collect();
    assert_eq!(
        held,
        vec![
            HostOut::Hold,
            HostOut::TakeSnapshot { slot: 2 },
            HostOut::Admit {
                slot: 2,
                tick: 4322
            },
            HostOut::Resume,
        ]
    );
    assert!(!p.host.holding());
    assert!(p.joiner_did.contains(&JoinOut::Joined {
        slot: 2,
        tick: 4322
    }));
    // On a link with no loss the transfer is one pass: one accept, each
    // chunk once.
    assert_eq!(
        p.host_did(|o| matches!(o, HostOut::Send(Msg::Accept(_)))),
        1
    );
    assert_eq!(
        p.host_did(|o| matches!(o, HostOut::Send(Msg::Chunk { .. }))),
        chunks_for(300_000) as usize
    );
}

#[test]
fn a_lossy_link_still_gets_the_whole_state_across() {
    let snap = state(200_000);
    let mut p = Pair::new(true);
    // Every fifth message either way is lost, the request included.
    p.run(10_000, IN_LEVEL, &snap, true, &|n, _| n % 5 == 0);
    assert_eq!(restored(&p), Some(snap));
    assert_eq!(p.host_did(|o| matches!(o, HostOut::Admit { .. })), 1);
    assert_eq!(p.host_did(|o| *o == HostOut::Resume), 1);
    assert!(!p.host.holding());
}

#[test]
fn a_lost_ready_is_repeated_and_admits_once() {
    let snap = state(5_000);
    let mut p = Pair::new(true);
    // Lose the newcomer's first ready.
    let lost = std::cell::Cell::new(false);
    let lose_first_ready = |_, m: &Msg| matches!(m, Msg::Ready { .. }) && !lost.replace(true);
    p.run(1_000, IN_LEVEL, &snap, true, &lose_first_ready);
    assert!(lost.get());
    assert_eq!(p.host_did(|o| matches!(o, HostOut::Admit { .. })), 1);
    // The newcomer keeps saying ready until the stream starts; the host
    // does not admit twice.
    p.run(1_000, IN_LEVEL, &snap, true, &|_, _| false);
    assert_eq!(p.host_did(|o| matches!(o, HostOut::Admit { .. })), 1);
    p.joiner.streaming();
    assert!(p.joiner.done());
    assert_eq!(p.joiner.poll(p.now + 1_000), vec![]);
}

#[test]
fn the_host_refuses_when_it_cannot_take_a_player() {
    let refusal = |enabled, game| {
        let mut h = Host::new(enabled);
        let out = h.on_msg(0, NONCE, Msg::Request, game);
        assert!(!h.holding());
        out
    };
    let say = |r| vec![HostOut::Send(Msg::Refuse(r))];
    assert_eq!(refusal(false, IN_LEVEL), say(Refusal::Off));
    let lobby = HostGame {
        in_level: false,
        free_slot: Some(1),
    };
    assert_eq!(refusal(true, lobby), say(Refusal::NotInGame));
    let full = HostGame {
        in_level: true,
        free_slot: None,
    };
    assert_eq!(refusal(true, full), say(Refusal::Full));

    let mut h = Host::new(true);
    h.on_msg(0, NONCE, Msg::Request, IN_LEVEL);
    // A repeat of the same request is the same join.
    assert_eq!(h.on_msg(5, NONCE, Msg::Request, IN_LEVEL), vec![]);
    // Someone else waits for the next one.
    assert_eq!(
        h.on_msg(5, NONCE + 1, Msg::Request, IN_LEVEL),
        say(Refusal::Busy)
    );
    // And their acks and aborts are not this join's.
    assert_eq!(
        h.on_msg(6, NONCE + 1, Msg::Abort(Abort::Cancelled), IN_LEVEL),
        vec![]
    );
    assert!(h.holding());
}

#[test]
fn a_refused_newcomer_goes_back_to_the_lobby() {
    let (mut j, _) = Joiner::start(0, NONCE);
    assert_eq!(
        j.on_msg(10, NONCE, Msg::Refuse(Refusal::Full)),
        vec![JoinOut::BackToLobby(JoinFail::Refused(Refusal::Full))]
    );
    assert!(j.done());
    assert_eq!(
        JoinFail::Refused(Refusal::Full).says(),
        "refused: the game is full"
    );
}

#[test]
fn a_silent_host_sends_the_newcomer_back() {
    let (mut j, _) = Joiner::start(0, NONCE);
    assert_eq!(j.poll(RESEND_MS), vec![JoinOut::Send(Msg::Request)]);
    assert_eq!(j.poll(RESEND_MS + 1), vec![]);
    assert_eq!(
        j.poll(ASK_MS + 1),
        vec![JoinOut::BackToLobby(JoinFail::NoAnswer)]
    );
}

#[test]
fn a_newcomer_that_goes_quiet_releases_the_host() {
    let mut h = Host::new(true);
    h.on_msg(0, NONCE, Msg::Request, IN_LEVEL);
    h.snapshot(10, 100, "01b_spacestation", 0, state(50_000));
    h.on_msg(20, NONCE, Msg::Ack { next: 0 }, IN_LEVEL);
    assert!(h.holding());
    // Resends while within the stall limit.
    assert!(!h.poll(20 + RESEND_MS).is_empty());
    let out = h.poll(10 + STALL_MS + 1);
    assert_eq!(
        out,
        vec![
            HostOut::Send(Msg::Abort(Abort::Timeout)),
            HostOut::Failed {
                why: Abort::Timeout
            },
            HostOut::Resume,
        ]
    );
    assert!(!h.holding());
    // Nothing was admitted, so nothing needs undoing.
}

#[test]
fn the_hold_has_a_ceiling_even_with_progress() {
    let mut h = Host::new(true);
    h.on_msg(0, NONCE, Msg::Request, IN_LEVEL);
    h.snapshot(0, 100, "01b_spacestation", 0, state(CHUNK * 200));
    let mut t = 0;
    for next in 1..200 {
        t += 1_000;
        if t > HOLD_MS {
            break;
        }
        h.on_msg(t, NONCE, Msg::Ack { next }, IN_LEVEL);
        assert!(h.holding(), "{t}");
    }
    let out = h.poll(HOLD_MS + 1);
    assert!(out.contains(&HostOut::Failed {
        why: Abort::Timeout
    }));
    assert!(!h.holding());
}

#[test]
fn a_host_that_cannot_snapshot_resumes() {
    let mut h = Host::new(true);
    h.on_msg(0, NONCE, Msg::Request, IN_LEVEL);
    assert_eq!(
        h.cancel(),
        vec![
            HostOut::Send(Msg::Abort(Abort::Cancelled)),
            HostOut::Failed {
                why: Abort::Cancelled
            },
            HostOut::Resume,
        ]
    );
    // And a snapshot that never comes is a timeout.
    let mut h = Host::new(true);
    h.on_msg(0, NONCE, Msg::Request, IN_LEVEL);
    assert_eq!(h.poll(STALL_MS), vec![]);
    assert!(h.poll(STALL_MS + 1).contains(&HostOut::Resume));
}

#[test]
fn a_failed_restore_tells_the_host_which_resumes_without_the_player() {
    let snap = state(20_000);
    let mut p = Pair::new(true);
    p.run(1_000, IN_LEVEL, &snap, false, &|_, _| false);
    assert!(
        p.joiner_did
            .contains(&JoinOut::BackToLobby(JoinFail::Aborted(Abort::Restore)))
    );
    assert_eq!(p.host_did(|o| matches!(o, HostOut::Admit { .. })), 0);
    assert!(p.host_did.contains(&HostOut::Failed {
        why: Abort::Restore
    }));
    assert_eq!(p.host_did(|o| *o == HostOut::Resume), 1);
    assert!(!p.host.holding());
}

#[test]
fn damaged_state_is_caught_by_the_checksum() {
    let snap = state(3 * CHUNK);
    let mut good = Plan::new(5, 1, "a", 0, &snap);
    good.sum ^= 1;
    let (mut j, _) = Joiner::start(0, NONCE);
    j.on_msg(1, NONCE, Msg::Accept(good));
    let mut out = vec![];
    for i in 0..3 {
        out = j.on_msg(
            2,
            NONCE,
            Msg::Chunk {
                index: i,
                data: snap[i as usize * CHUNK..(i as usize + 1) * CHUNK].to_vec(),
            },
        );
    }
    assert_eq!(
        out,
        vec![
            JoinOut::Send(Msg::Abort(Abort::Checksum)),
            JoinOut::BackToLobby(JoinFail::Aborted(Abort::Checksum)),
        ]
    );
}

#[test]
fn chunks_out_of_order_or_the_wrong_size_are_handled() {
    let snap = state(2 * CHUNK + 10);
    let plan = Plan::new(5, 1, "a", 0, &snap);
    let (mut j, _) = Joiner::start(0, NONCE);
    j.on_msg(1, NONCE, Msg::Accept(plan));
    let piece = |i: usize| snap[i * CHUNK..((i + 1) * CHUNK).min(snap.len())].to_vec();
    let chunk = |i: u32, data| Msg::Chunk { index: i, data };
    // The last one first: still waiting for chunk 0.
    assert_eq!(
        j.on_msg(2, NONCE, chunk(2, piece(2))),
        vec![JoinOut::Send(Msg::Ack { next: 0 })]
    );
    // A short chunk in the middle is ignored.
    assert_eq!(
        j.on_msg(3, NONCE, chunk(1, vec![0; 5])),
        vec![JoinOut::Send(Msg::Ack { next: 0 })]
    );
    assert_eq!(
        j.on_msg(4, NONCE, chunk(0, piece(0))),
        vec![JoinOut::Send(Msg::Ack { next: 1 })]
    );
    let out = j.on_msg(5, NONCE, chunk(1, piece(1)));
    assert_eq!(out[0], JoinOut::Send(Msg::Ack { next: 3 }));
    assert!(matches!(&out[1], JoinOut::Restore { state, .. } if *state == snap));
    // Another join's messages are not this one's.
    assert_eq!(j.on_msg(6, NONCE + 1, Msg::Abort(Abort::Timeout)), vec![]);
}

#[test]
fn an_insane_plan_is_not_taken() {
    let mut p = plan();
    p.chunks = 4;
    let (mut j, _) = Joiner::start(0, NONCE);
    assert!(
        j.on_msg(1, NONCE, Msg::Accept(p))
            .contains(&JoinOut::BackToLobby(JoinFail::Aborted(Abort::Checksum)))
    );
    // Nor is a snapshot too big to send.
    let mut h = Host::new(true);
    h.on_msg(0, NONCE, Msg::Request, IN_LEVEL);
    let out = h.snapshot(1, 1, "a", 0, vec![0; MAX_STATE as usize + 1]);
    assert!(out.contains(&HostOut::Resume));
}

#[test]
fn the_log_lines() {
    assert_eq!(
        joined_line(2, 4322),
        "coop-jip: joined in progress at tick 4322 as player 3"
    );
    assert_eq!(
        admitted_line(2, 4322, 300_000, 840),
        "coop-jip: admitted player 3 at tick 4322 (293 KiB sent, held 840 ms)"
    );
}
