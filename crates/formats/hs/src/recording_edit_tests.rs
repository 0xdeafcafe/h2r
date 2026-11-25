//! The encoder, the edits and the block's patches, over invented streams.
use super::*;
use crate::recording::{Game, decode_block, decode_stream, prologue_len};

const H2: Game = Game::Halo2;
const BASE: u32 = 0x9000_0000;
const AT: usize = 0x200;

/// A prologue whose every byte is different, so a lost byte shows.
fn prologue() -> Vec<u8> {
    (0..prologue_len(H2, 1))
        .map(|i| (i as u8).wrapping_mul(7).wrapping_add(1))
        .collect()
}

/// throttle after 4 ticks (delay form 2), `aiming_speed` 1 tick later
/// (form 1), the end 2 ticks after that (form 2).
fn stream() -> Vec<u8> {
    let mut s = prologue();
    s.extend_from_slice(&[(6 << 2) | 2, 4]);
    s.extend_from_slice(&0.5f32.to_le_bytes());
    s.extend_from_slice(&[0; 4]);
    s.extend_from_slice(&[(3 << 2) | 1, 5]);
    s.extend_from_slice(&[(1 << 2) | 2, 2]);
    s
}

fn parse(bytes: &[u8]) -> Stream {
    Stream::parse(H2, 1, bytes).unwrap()
}

#[test]
fn a_stream_encodes_to_the_bytes_it_was_read_from() {
    assert_eq!(parse(&stream()).encode(), stream());
    // Delay forms a canonical writer would not choose, and bytes after the
    // end, come back as they were.
    let mut s = prologue();
    s.extend_from_slice(&[(2 << 2) | 2, 1, 0x05]);
    s.extend_from_slice(&[(3 << 2) | 3, 5, 0, 0x01]);
    s.extend_from_slice(&[(0 << 2) | 3, 0x2c, 0x01]);
    s.extend_from_slice(&[(1 << 2) | 1, 0xaa, 0xbb]);
    let p = parse(&s);
    assert_eq!(p.trailing, [0xaa, 0xbb]);
    assert_eq!(p.ticks(), 1 + 5 + 300 + 1);
    assert_eq!(p.encode(), s);
    // A delay too big for its form is written in the form that holds it.
    let mut p = parse(&stream());
    p.events[1].head_len = 1;
    p.events[1].delay = 7;
    let bytes = p.encode();
    assert_eq!(decode_stream(H2, 1, &bytes).unwrap().events[1].delay, 7);
}

#[test]
fn removing_and_inserting_keep_every_other_event_at_its_tick() {
    let ticks = |s: &Stream| {
        let bytes = s.encode();
        decode_stream(H2, 1, &bytes)
            .unwrap()
            .events
            .iter()
            .map(|e| (e.name, e.tick))
            .collect::<Vec<_>>()
    };
    let mut s = parse(&stream());
    assert_eq!(
        ticks(&s),
        [("throttle", 4), ("aiming_speed", 5), ("end", 7)]
    );
    s.remove(0).unwrap();
    assert_eq!(ticks(&s), [("aiming_speed", 5), ("end", 7)]);
    assert!(s.remove(1).unwrap_err().contains("end event"));
    assert!(s.remove(9).unwrap_err().contains("no event 9"));

    let mut s = parse(&stream());
    s.insert(2, 2, 1, &[9]).unwrap();
    assert_eq!(
        ticks(&s),
        [
            ("throttle", 4),
            ("aiming_speed", 5),
            ("animation_state", 6),
            ("end", 7)
        ]
    );
    // Before the second event there is one tick, not three.
    assert!(
        s.insert(1, 2, 3, &[9])
            .unwrap_err()
            .contains("past the 1 ticks")
    );
    assert!(
        s.insert(1, 1, 0, &[])
            .unwrap_err()
            .contains("one end event")
    );
    assert!(
        s.insert(1, 2, 0, &[1, 2])
            .unwrap_err()
            .contains("takes 1 payload")
    );
    assert!(s.insert(1, 40, 0, &[]).unwrap_err().contains("no handler"));
    assert!(
        s.insert(9, 2, 0, &[1])
            .unwrap_err()
            .contains("before the end event")
    );
    s.check().unwrap();
}

#[test]
fn a_moved_event_leaves_the_rest_in_place_and_the_end_sets_the_length() {
    let mut s = parse(&stream());
    s.set_delay(0, 2).unwrap();
    assert_eq!(
        s.events.iter().map(|e| e.delay).collect::<Vec<_>>(),
        [2, 3, 2]
    );
    assert_eq!(s.ticks(), 7);
    // The second event has one tick to give.
    let mut s = parse(&stream());
    assert!(
        s.set_delay(0, 6)
            .unwrap_err()
            .contains("earlier than it can")
    );
    assert_eq!(s, parse(&stream()), "a refused edit changes nothing");
    s.set_delay(2, 9).unwrap();
    assert_eq!(s.ticks(), 14);
}

#[test]
fn payloads_and_the_initial_state_are_set_by_name_within_their_widths() {
    let mut s = parse(&stream());
    assert!(s.set_payload(1, &[1, 2]).unwrap_err().contains("takes 1"));
    s.set_payload(1, &[0xf0]).unwrap();
    s.set_initial("aiming_speed", &Value::Signed(-3)).unwrap();
    s.set_initial("throttle", &Value::Reals(vec![1.0, -2.0]))
        .unwrap();
    s.set_initial("animation_state", &Value::Unsigned(0x0600_0086))
        .unwrap();
    s.set_trailer(&[3; 12]).unwrap();
    let d = decode_stream(H2, 1, &s.encode()).unwrap();
    let get = |n: &str| {
        d.initial
            .fields
            .iter()
            .find(|f| f.0 == n)
            .unwrap()
            .1
            .clone()
    };
    assert_eq!(get("aiming_speed"), Value::Signed(-3));
    assert_eq!(get("throttle"), Value::Reals(vec![1.0, -2.0]));
    assert_eq!(get("animation_state"), Value::Unsigned(0x0600_0086));
    assert_eq!(d.initial.trailer, [3; 12]);
    assert_eq!(d.events[1].payload, [0xf0]);
    // The padding the typed read drops is untouched.
    assert_eq!(s.prologue[14..16], prologue()[14..16]);

    assert!(
        s.set_initial("aiming_speed", &Value::Signed(40_000))
            .unwrap_err()
            .contains("does not fit")
    );
    assert!(
        s.set_initial("throttle", &Value::Reals(vec![1.0]))
            .unwrap_err()
            .contains("2 finite")
    );
    assert!(
        s.set_initial("throttle", &Value::Reals(vec![f32::NAN, 0.0]))
            .is_err()
    );
    assert!(
        s.set_initial("pad", &Value::Unsigned(0))
            .unwrap_err()
            .contains("no initial field")
    );
    assert!(s.set_trailer(&[0; 3]).unwrap_err().contains("12 bytes"));
    s.check().unwrap();
}

#[test]
fn payloads_are_built_from_the_names_the_read_gives() {
    let u = Value::Unsigned;
    let s = Value::Signed;
    assert_eq!(payload_from(2, &[("value", u(7))]).unwrap(), [7]);
    assert_eq!(payload_from(3, &[("value", s(-1))]).unwrap(), [0xff]);
    assert_eq!(
        payload_from(4, &[("value", u(0x0102_0304))]).unwrap(),
        [4, 3, 2, 1]
    );
    assert_eq!(payload_from(5, &[("value", s(-2))]).unwrap(), [0xfe, 0xff]);
    assert_eq!(
        payload_from(6, &[("value", Value::Reals(vec![1.0, 0.0]))]).unwrap(),
        [0, 0, 0x80, 0x3f, 0, 0, 0, 0]
    );
    assert_eq!(
        payload_from(9, &[("channel", u(2)), ("a", s(-1)), ("b", s(2))]).unwrap(),
        [0xff, 2]
    );
    assert_eq!(
        payload_from(16, &[("a", s(1)), ("b", s(-1))]).unwrap(),
        [1, 0, 0xff, 0xff]
    );
    assert!(
        payload_from(9, &[("a", s(200)), ("b", s(0))])
            .unwrap_err()
            .contains("does not fit")
    );
    assert!(
        payload_from(9, &[("a", s(1))])
            .unwrap_err()
            .contains("needs \"b\"")
    );
    assert!(payload_from(0, &[]).is_err());
    // Every length matches the handler table's.
    for kind in 2..24u8 {
        let fields = [
            ("value", Value::Reals(vec![0.0, 0.0])),
            ("a", s(0)),
            ("b", s(0)),
        ];
        let fields: Vec<_> = match kind {
            6 => vec![fields[0].clone()],
            7..=22 => fields[1..].to_vec(),
            4 | 23 | 5 | 2 | 3 => vec![("value", u(0))],
            _ => unreachable!(),
        };
        assert_eq!(
            payload_from(kind, &fields).unwrap().len(),
            payload_len(H2, kind).unwrap(),
            "type {kind}"
        );
    }
}

fn rows() -> Vec<Recorded> {
    let mut b = prologue();
    b.extend_from_slice(&[(1 << 2) | 1]);
    let row = |name: &str, ticks, stream| Recorded {
        name: name.into(),
        version: 4,
        control_version: 1,
        ticks,
        stream,
    };
    vec![
        row("a", 7, stream()),
        row("b", 1, b.clone()),
        row("c", 1, b),
    ]
}

fn block() -> (Vec<u8>, Block) {
    let mut p = vec![0u8; 0x400];
    crate::fixture::write_recordings(&mut p, BASE, AT, &rows());
    let b = decode_block(&p, BASE).unwrap();
    (p, b)
}

/// Apply `c` to `payload` the way a save does (growth appended at `at`).
fn apply(payload: &[u8], c: &BlockChanges, grow_at: usize) -> Vec<u8> {
    let mut p = payload.to_vec();
    p.resize(0x800, 0);
    let mut at = grow_at;
    for (a, b) in &c.patches {
        let o = (a - BASE) as usize;
        p[o..o + b.len()].copy_from_slice(b);
    }
    for g in &c.grown {
        p[at..at + g.bytes.len()].copy_from_slice(&g.bytes);
        let o = (g.root - BASE) as usize;
        p[o..o + 4].copy_from_slice(&(g.bytes.len() as u32).to_le_bytes());
        p[o + 4..o + 8].copy_from_slice(&(BASE + at as u32).to_le_bytes());
        at += g.bytes.len().next_multiple_of(4);
    }
    p
}

#[test]
fn the_block_reads_with_its_elements_and_nothing_edited_changes_nothing() {
    let (_, b) = block();
    assert_eq!((b.root, b.address), (BASE + 0x110, BASE + AT as u32));
    assert_eq!(b.elements.len(), 3);
    assert_eq!(changes(&b, &entries(&b)).unwrap(), BlockChanges::default());
    assert_eq!(entries(&b)[0].name(), "a");
    assert_eq!(entries(&b)[0].recorded(), b.rows[0]);
}

#[test]
fn a_shorter_stream_is_written_in_place_and_reads_back_clean() {
    let (p, b) = block();
    let mut e = entries(&b);
    let mut s = parse(&e[0].stream);
    s.remove(0).unwrap();
    s.set_initial("aiming_speed", &Value::Signed(9)).unwrap();
    e[0].stream = s.encode();
    e[0].ticks = s.ticks() as i16;
    assert!(e[0].stream.len() < e[0].original.len());
    let c = changes(&b, &e).unwrap();
    assert!(c.grown.is_empty());
    let out = apply(&p, &c, 0x600);
    let after = decode_block(&out, BASE).unwrap();
    assert_eq!(after.rows[0].stream, e[0].stream);
    assert_eq!(after.rows[0].ticks, 7);
    assert_eq!(after.rows[1], b.rows[1]);
    assert_eq!(after.rows[2], b.rows[2]);
    decode_stream(H2, 1, &after.rows[0].stream).unwrap();
    // Only the first recording's element and stream changed.
    let changed: Vec<_> = p
        .iter()
        .zip(&out)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|x| x.0)
        .collect();
    assert!(changed.iter().all(|&i| i < 0x2f4), "{changed:x?}");
}

#[test]
fn a_longer_stream_is_grown_at_its_slots_pair() {
    let (p, b) = block();
    let mut e = entries(&b);
    let mut s = parse(&e[1].stream);
    s.insert(0, 2, 0, &[3]).unwrap();
    s.insert(0, 2, 0, &[4]).unwrap();
    s.set_delay(2, 5).unwrap();
    e[1].stream = s.encode();
    e[1].ticks = s.ticks() as i16;
    let c = changes(&b, &e).unwrap();
    assert_eq!(
        c.grown,
        [Grown {
            root: BASE + AT as u32 + ELEMENT as u32 + PAIR as u32,
            bytes: e[1].stream.clone()
        }]
    );
    let out = apply(&p, &c, 0x600);
    let after = decode_block(&out, BASE).unwrap();
    assert_eq!(after.rows[1].stream, e[1].stream);
    assert_eq!(after.rows[1].ticks, 5);
    assert_eq!(after.rows[0], b.rows[0]);
    assert_eq!(after.rows[2], b.rows[2]);
}

#[test]
fn removing_a_recording_moves_the_later_ones_down_and_lowers_the_count() {
    let (p, b) = block();
    let mut e = entries(&b);
    e.remove(0);
    let c = changes(&b, &e).unwrap();
    assert!(
        c.patches
            .contains(&(BASE + 0x110, 2u32.to_le_bytes().to_vec()))
    );
    let out = apply(&p, &c, 0x600);
    let after = decode_block(&out, BASE).unwrap();
    assert_eq!(after.rows, [b.rows[1].clone(), b.rows[2].clone()]);
    // A removed middle one with a grown stream behind it.
    let mut e = entries(&b);
    let mut s = parse(&e[2].stream);
    s.insert(0, 2, 0, &[3]).unwrap();
    e[2].stream = s.encode();
    e.remove(1);
    let c = changes(&b, &e).unwrap();
    let out = apply(&p, &c, 0x600);
    let after = decode_block(&out, BASE).unwrap();
    assert_eq!(after.rows[0], b.rows[0]);
    assert_eq!(after.rows[1].stream, e[1].stream);
    assert_eq!(after.rows[1].name, "c");
    assert_eq!(after.rows.len(), 2);
}

#[test]
fn a_recording_cannot_be_added_past_the_blocks_room() {
    let (_, b) = block();
    let mut e = entries(&b);
    e.push(e[0].clone());
    assert!(changes(&b, &e).unwrap_err().contains("room for 3"));
}
