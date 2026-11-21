use host_log::{Entry, Error, Log, LogHead, Sha256};
use serde_json::json;

fn genesis() -> LogHead {
    LogHead::genesis(Sha256::of(b"synthetic session"))
}

fn entry<Op>(head: LogHead, tick: u32, op: Op) -> Entry<Op> {
    Entry {
        seq: head.next_seq().unwrap(),
        tick,
        author: None,
        op,
        prev_hash: head.hash,
    }
}

fn three_entries() -> Log<&'static str> {
    let mut log = Log::new(genesis()).unwrap();
    for (tick, op) in [(15, "begin"), (15, "spawn"), (31, "finish")] {
        log.push(entry(log.head(), tick, op)).unwrap();
    }
    log
}

#[test]
fn chain_breaks_on_a_changed_entry() {
    let host = three_entries();
    let entries = host.since(0);
    let mut altered = Log::new(genesis()).unwrap();
    let mut first = entries[0].clone();
    first.op = "skip";
    altered.push(first).unwrap();
    assert!(matches!(
        altered.push(entries[1].clone()),
        Err(Error::Predecessor { seq: 2 })
    ));

    let mut received = Log::new(genesis()).unwrap();
    for entry in &entries[..2] {
        received.push(entry.clone()).unwrap();
    }
    assert!(matches!(
        received.verify(host.head()),
        Err(Error::HeadMismatch { actual, .. }) if actual.seq == 2
    ));

    let mut last = entries[2].clone();
    last.op = "restart";
    let actual = received.push(last).unwrap();
    assert_eq!(actual.seq, host.head().seq);
    assert_eq!(actual.tick, host.head().tick);
    assert!(matches!(
        received.verify(host.head()),
        Err(Error::HeadMismatch { .. })
    ));
}

#[test]
fn since_returns_the_tail() {
    let host = three_entries();
    let entries = host.since(0);
    assert_eq!(host.since(1), &entries[1..]);
    assert_eq!(host.since(2), &entries[2..]);
    assert!(host.since(3).is_empty());
    assert!(host.since(u32::MAX).is_empty());

    let checkpoint = LogHead {
        seq: entries[0].seq,
        tick: entries[0].tick,
        hash: entries[0].hash().unwrap(),
    };
    let mut peer = Log::new(checkpoint).unwrap();
    assert_eq!(peer.head(), checkpoint);
    for entry in host.since(checkpoint.seq) {
        peer.push(entry.clone()).unwrap();
    }
    assert_eq!(peer.since(0), host.since(1));
    assert_eq!(peer.since(1), host.since(1));
    assert_eq!(peer.since(2), host.since(2));
    assert!(peer.since(3).is_empty());
    peer.verify(host.head()).unwrap();
}

#[test]
fn golden_head_for_three_entries() {
    let log = three_entries();
    assert_eq!(
        log.since(0)[0].canonical_bytes().unwrap().as_slice(),
        br#"{"op":"begin","prev_hash":"2eb8c484286db2f5c9d1a0c5603c78c0e602f2200cf33e074f342eb65291dbdf","seq":1,"tick":15}"#
    );
    let expected = LogHead {
        seq: 3,
        tick: 31,
        hash: "79a5d4202b0d2a212b64a5f2cfda6b3070b87939cb4448f4fbfb30f995790a4a"
            .parse()
            .unwrap(),
    };
    assert_eq!(log.head(), expected);
    log.verify(expected).unwrap();
}

#[test]
fn out_of_order_seq_is_refused() {
    let mut log = Log::new(genesis()).unwrap();
    let mut gap = entry(genesis(), 1, "begin");
    gap.seq = 2;
    assert!(matches!(
        log.push(gap),
        Err(Error::Sequence {
            expected: 1,
            actual: 2
        })
    ));
    assert_eq!(log.head(), genesis());
    assert!(log.since(0).is_empty());

    let first = entry(genesis(), 1, "begin");
    let head = log.push(first.clone()).unwrap();
    assert!(matches!(
        log.push(first),
        Err(Error::Sequence {
            expected: 2,
            actual: 1
        })
    ));
    assert_eq!(log.head(), head);
    assert_eq!(log.since(0).len(), 1);
}

#[test]
fn backwards_tick_leaves_the_log_intact() {
    let mut log = three_entries();
    let head = log.head();
    assert!(matches!(
        log.push(entry(head, 30, "restart")),
        Err(Error::TickBackwards {
            seq: 4,
            previous: 31,
            actual: 30
        })
    ));
    assert_eq!(log.head(), head);
    assert_eq!(log.since(0).len(), 3);
    log.verify(head).unwrap();
}

#[test]
fn rejects_a_bad_genesis_tick_and_sequence_overflow() {
    assert!(matches!(
        Log::<&str>::new(LogHead {
            tick: 1,
            ..genesis()
        }),
        Err(Error::InitialTick(1))
    ));
    let checkpoint = LogHead {
        seq: u32::MAX,
        tick: 31,
        hash: Sha256::of(b"synthetic checkpoint"),
    };
    let mut log = Log::new(checkpoint).unwrap();
    assert!(matches!(
        log.push(Entry {
            seq: 0,
            tick: 32,
            author: None,
            op: "restart",
            prev_hash: checkpoint.hash,
        }),
        Err(Error::SequenceExhausted)
    ));
    assert_eq!(log.head(), checkpoint);
}

#[test]
fn noninteger_operations_are_refused_without_advancing() {
    let mut log = Log::new(genesis()).unwrap();
    assert!(matches!(
        log.push(entry(genesis(), 0, json!({"fraction": 0.5}))),
        Err(Error::Canonical(_))
    ));
    assert_eq!(log.head(), genesis());
    assert!(log.since(0).is_empty());
}

#[test]
fn forge_f1_authored_entry_keeps_its_wire_bytes_and_hash() {
    // The synthetic host_entry_golden_vector from forge-doc.
    let wire = br#"{"author":"Editor","op":{"edit":{"placement":{"set":{"block":"weapons","index":0,"write":{"field":"scale","value":{"float":"80000000"}}}}},"id":1},"prev_hash":"77806693f7f1cea053e7a5fe7a3cd3fa28222cb01b8c282b6018a4abbb909001","seq":1,"tick":90}"#;
    let entry: Entry<serde_json::Value> = serde_json::from_slice(wire).unwrap();
    assert_eq!(entry.author.as_deref(), Some("Editor"));
    assert_eq!(entry.canonical_bytes().unwrap().as_slice(), wire);
    let mut log = Log::new(LogHead::genesis(entry.prev_hash)).unwrap();
    let expected = LogHead {
        seq: 1,
        tick: 90,
        hash: "030c1227e8f49097f024c4d6fcceb4e3363d3cfa4ca1340f4c9003c341e20f0f"
            .parse()
            .unwrap(),
    };
    assert_eq!(log.push(entry).unwrap(), expected);
    log.verify(expected).unwrap();
}
