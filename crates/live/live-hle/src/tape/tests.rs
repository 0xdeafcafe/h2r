use super::*;
use film::tape::Returned;

fn call(entry: u32, value: u32) -> Call {
    Call {
        entry,
        args: vec![1, 2],
        answer: Answer::Return(Returned { value, ..Returned::default() }),
    }
}

#[test]
fn a_recorder_keeps_calls_in_order_until_it_is_spoiled() {
    let tape = Tape::recorder();
    assert!(!tape.is_replay());
    tape.record(call(0x003cd211, 1));
    tape.record(call(0x003cd22b, 2));
    assert_eq!(tape.position(), 2);
    assert_eq!(tape.calls(), [call(0x003cd211, 1), call(0x003cd22b, 2)]);
    tape.spoil("recv wrote what a tape cannot hold".into());
    tape.record(call(0x003cd211, 3));
    assert!(tape.calls().is_empty());
    assert_eq!(tape.spoiled().as_deref(), Some("call 2: recv wrote what a tape cannot hold"));
}

#[test]
fn a_replayer_answers_by_index_and_names_what_it_cannot() {
    let tape = Tape::replayer(vec![call(0x003cd211, 7)]);
    assert!(tape.is_replay());
    assert_eq!(tape.answer(0x003cd211, &[1, 2]), Ok(call(0x003cd211, 7)));
    let error = tape.answer(0x003cd211, &[1, 2]).unwrap_err();
    assert_eq!(error, TapeError::NoAnswer { index: 1, name: "recv", recorded: 1 });
    assert_eq!(tape.failure(), Some(error));
}

#[test]
fn a_replayer_refuses_other_arguments_at_an_index() {
    let tape = Tape::replayer(vec![call(0x003cd211, 7)]);
    let error = tape.answer(0x003cd211, &[1, 3]).unwrap_err();
    assert!(matches!(error, TapeError::Diverged { index: 0, name: "recv", .. }), "{error}");
    // Once stopped it stays stopped, even for the recorded call.
    assert_eq!(tape.answer(0x003cd211, &[1, 2]).unwrap_err(), error);
}

#[test]
fn permissions_survive_the_tape() {
    use runtime::guest::Permissions as P;
    for p in [P::NONE, P::READ, P::RW, P::RX, P::RWX] {
        assert_eq!(permissions(permission_bits(p)), Some(p));
    }
    assert_eq!(permissions(2), None);
}
