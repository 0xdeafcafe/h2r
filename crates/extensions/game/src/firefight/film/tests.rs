//! A Firefight film record's payload, on invented entries.

use super::*;
use host_log::{LogHead, Sha256};

fn entry(op: Command) -> Entry<Command> {
    let head = LogHead::genesis(Sha256::of(b"an invented match"));
    Entry {
        seq: head.next_seq().unwrap(),
        tick: 9,
        author: None,
        op,
        prev_hash: head.hash,
    }
}

#[test]
fn an_entry_is_its_canonical_json_and_reads_back() {
    for op in [
        Command::ModeState {
            blob: vec![3, 0, 1, 255],
        },
        Command::Respawn { player: 2 },
    ] {
        let e = entry(op);
        let bytes = payload(&e).unwrap();
        assert_eq!(bytes, e.canonical_bytes().unwrap());
        assert_eq!(parse(&bytes).unwrap(), e);
    }
}

#[test]
fn a_record_that_is_not_an_entry_is_refused() {
    assert!(parse(b"").is_err());
    assert!(parse(b"{\"seq\":1}").is_err());
}

#[test]
fn a_full_queue_keeps_the_latest() {
    let mut waiting: Vec<Vec<u8>> = (0..MAX_WAITING).map(|i| i.to_le_bytes().to_vec()).collect();
    keep(&mut waiting, b"new".to_vec());
    assert_eq!(waiting.len(), MAX_WAITING);
    assert_eq!(waiting[0], 1usize.to_le_bytes());
    assert_eq!(waiting.last().unwrap(), b"new");
}
