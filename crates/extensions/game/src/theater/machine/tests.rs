use super::*;
use film::tape::{Answer, Call, Returned};

pub(crate) fn sample_tape() -> Tape {
    Tape {
        calls: vec![Call {
            entry: 0x003cd22b,
            args: vec![0x100, 0x8000, 64, 0, 0x9000, 0x9010],
            answer: Answer::Return(Returned { value: 3, writes: vec![(0x8000, b"one".to_vec())], ..Returned::default() }),
        }],
        pads: vec![PadRead { handle: 4, status: 0, packet: [9; film::tape::PACKET] }],
    }
}

#[test]
fn pad_reads_survive_the_tape() {
    let tape = sample_tape();
    let log = input_log(&tape.pads);
    assert_eq!(log.reads.len(), 1);
    assert_eq!(pad_reads(&log), tape.pads);
}

#[test]
fn a_fixed_machine_hands_back_its_tape_and_role() {
    let tape = sample_tape();
    let machine = Machine::fixed(Role::LiveClient, &tape);
    assert_eq!(machine.role(), Role::LiveClient);
    assert_eq!(machine.tape().unwrap(), tape);
}
