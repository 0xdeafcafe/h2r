use super::*;

fn recvfrom(n: u8) -> Call {
    Call {
        entry: 0x003cd22b,
        args: vec![0x100, 0x8000, 1264, 0, 0x9000, 0x9010],
        answer: Answer::Return(Returned {
            value: 3,
            error: None,
            zeroed: Vec::new(),
            writes: vec![(0x8000, vec![n, n.wrapping_add(1), n.wrapping_add(2)]), (0x9000, vec![2, 0, 0x0c, 0x02]), (0x9010, vec![16, 0, 0, 0])],
            alloc: None,
        }),
    }
}

fn would_block() -> Call {
    Call {
        entry: 0x003cd22b,
        args: vec![0x100, 0x8000, 1264, 0, 0, 0],
        answer: Answer::Return(Returned { value: u32::MAX, error: Some(10035), ..Returned::default() }),
    }
}

fn sample() -> Tape {
    let qos = Call {
        entry: 0x003cd156,
        args: (0..12).collect(),
        answer: Answer::Return(Returned {
            value: 0,
            error: None,
            zeroed: vec![(0xa000_0000, 0x10_0000, 3)],
            writes: vec![(0xa000_1000, vec![1; 32]), (11, 0xa000_1000u32.to_le_bytes().to_vec())],
            alloc: Some((32, 0xa000_1000)),
        }),
    };
    let stop = Call { entry: 0x003cd103, args: vec![1, 2, 3, 4], answer: Answer::Stop("unimplemented Live API XNetTsAddrToInAddr".into()) };
    Tape {
        calls: vec![recvfrom(7), would_block(), qos, recvfrom(9), stop],
        pads: vec![PadRead { handle: 4, status: 0, packet: [3; PACKET] }, PadRead { handle: 4, status: 1167, packet: [0; PACKET] }],
    }
}

#[test]
fn a_tape_reads_back_as_written_call_for_call() {
    let tape = sample();
    let mut back = Tape::default();
    for block in tape.blocks().unwrap() {
        back.push_block(&block).unwrap();
    }
    assert_eq!(back, tape);
}

#[test]
fn a_long_tape_splits_into_ordered_blocks() {
    let tape = Tape { calls: (0..2 * BLOCK_ENTRIES).map(|i| recvfrom(i as u8)).collect(), pads: Vec::new() };
    let blocks = tape.blocks().unwrap();
    assert_eq!(blocks.len(), 2);
    let mut back = Tape::default();
    for block in &blocks {
        back.push_block(block).unwrap();
    }
    assert_eq!(back.calls.len(), tape.calls.len());
    // A block out of order is refused, not spliced in.
    let mut wrong = Tape::default();
    assert!(wrong.push_block(&blocks[1]).unwrap_err().to_string().contains("starts at call 4096"));
}

#[test]
fn a_newer_tape_version_is_named() {
    let mut block = sample().blocks().unwrap().remove(0);
    block[0] = VERSION + 1;
    let error = decode_block(&block).unwrap_err().to_string();
    assert!(error.contains(&format!("version {}", VERSION + 1)), "{error}");
}

#[test]
fn a_cut_block_is_refused() {
    let block = sample().blocks().unwrap().remove(0);
    assert!(decode_block(&block[..block.len() - 1]).is_err());
    let mut long = block.clone();
    long.push(0);
    assert!(decode_block(&long).unwrap_err().to_string().contains("after its last entry"));
}

#[test]
fn roles_name_the_clients() {
    assert!(Role::SystemLinkClient.is_client() && Role::LiveClient.is_client());
    assert!(!Role::Host.is_client() && !Role::Local.is_client());
    assert_eq!(serde_json::to_string(&Role::LiveClient).unwrap(), "\"live-client\"");
    assert_eq!(Role::default(), Role::Local);
}
