use super::*;

fn call(index: u64, op: &str, args: &[u32], result: Result<u32, u32>, outputs: &[&[u8]]) -> Call {
    Call {
        index,
        op: op.into(),
        args: args.to_vec(),
        thread: 3,
        result,
        outputs: outputs.iter().map(|o| o.to_vec()).collect(),
    }
}

fn round_trip(calls: &[Call]) -> usize {
    let bytes = encode_batch(calls).unwrap();
    assert_eq!(bytes[0], MAGIC);
    assert_eq!(decode_batch(&bytes).unwrap(), calls);
    bytes.len()
}

#[test]
fn every_shape_of_call_round_trips() {
    round_trip(&[
        call(0, "Socket", &[], Ok(0), &[]),
        call(
            1,
            "Recv",
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
            Err(10040),
            &[b"abcd"],
        ),
        call(
            2,
            "RecvFrom",
            &[u32::MAX, 0, 1],
            Ok(u32::MAX),
            &[b"", &[0; 16], &[1, 2, 3, 4]],
        ),
        call(7, "XInputGetState", &[0x1234], Ok(0), &[&[9; 22]]),
        // The same handle and slot: a repeat, a delta, a change of length, a delta again.
        call(8, "XInputGetState", &[0x1234], Ok(0), &[&[9; 22]]),
        call(
            9,
            "XInputGetState",
            &[0x1234],
            Ok(0),
            &[&[
                9, 9, 9, 9, 8, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 7,
            ]],
        ),
        call(10, "XInputGetState", &[0x1234], Ok(0), &[&[1; 3]]),
        call(11, "XInputGetState", &[0x1234], Ok(0), &[&[1, 2, 1]]),
        // Another handle keeps its own last packet.
        call(12, "XInputGetState", &[0x9999], Ok(0), &[&[9; 22]]),
        // Calls out of order and a call index past 32 bits.
        call(
            5,
            "Select",
            &[0, 1, 0, 0, 0],
            Ok(1),
            &[&[1, 0, 0, 0, 3, 0, 0, 0]],
        ),
        call(u64::MAX, "Select", &[0], Err(u32::MAX), &[]),
        call(0, "Random", &[16], Ok(0), &[&[0xff; 16]]),
    ]);
}

#[test]
fn a_pad_read_is_the_bytes_that_differ_from_the_handles_last_packet() {
    let mut packet = [0u8; 22];
    let mut calls = Vec::new();
    for i in 0..128u64 {
        // One stick byte moves a little every few reads.
        if i % 5 == 0 {
            packet[6] = packet[6].wrapping_add(3);
        }
        calls.push(call(i, "XInputGetState", &[0x1234], Ok(0), &[&packet]));
    }
    let size = round_trip(&calls);
    // First packet whole, then a repeat (one byte) or one changed byte (a
    // few) a read.
    assert!(size < 22 + 128 * 12, "{size}");
    assert!(size < 128 * 12, "{size}");
}

#[test]
fn the_binary_stream_is_a_fraction_of_the_json_one_on_a_synthetic_tape() {
    // A minute-ish of an offline game's interleaved stream: one pad read a
    // tick, a socket poll every third tick, an empty select every tenth.
    let mut calls = Vec::new();
    let mut packet = [0u8; 22];
    let mut index = 0;
    for tick in 0..120u64 {
        packet[2] = (tick / 7) as u8;
        packet[6] = (tick * 3) as u8;
        calls.push(call(index, "XInputGetState", &[0x40], Ok(0), &[&packet]));
        index += 1;
        if tick % 3 == 0 {
            calls.push(call(
                index,
                "RecvFrom",
                &[0x100, 0, 1500, 0, 0, 0],
                Err(10035),
                &[],
            ));
            index += 1;
        }
        if tick % 10 == 0 {
            calls.push(call(
                index,
                "Select",
                &[0, 0, 0, 0, 0],
                Ok(0),
                &[&[0, 0, 0, 0]],
            ));
            index += 1;
        }
    }
    calls.truncate(128);
    let binary = round_trip(&calls);
    let json = serde_json::to_vec(&calls).unwrap().len();
    assert!(binary * 6 < json, "binary {binary} against json {json}");
}

#[test]
fn a_json_batch_is_refused_as_a_breaking_change() {
    let json = serde_json::to_vec(&[call(0, "Socket", &[], Ok(0), &[])]).unwrap();
    let error = decode_batch(&json).unwrap_err().to_string();
    assert!(error.contains("JSON encoding"), "{error}");
    assert!(decode_batch(&[]).is_err());
    assert!(decode_batch(&[0x01, 0x00]).is_err());
}

#[test]
fn a_batch_the_encoder_would_not_write_is_refused() {
    assert!(encode_batch(&[]).is_err());
    let long = "x".repeat(65);
    assert!(encode_batch(&[call(0, &long, &[], Ok(0), &[])]).is_err());
    round_trip(&[call(0, "Op", &[0; 65], Ok(0), &[b"x"; 129])]);
}

#[test]
fn a_truncated_or_fuzzed_batch_is_refused_and_never_reads_out_of_bounds() {
    let calls: Vec<Call> = (0..40u64)
        .map(|i| {
            call(
                i,
                if i % 2 == 0 {
                    "XInputGetState"
                } else {
                    "RecvFrom"
                },
                &[0x40, i as u32],
                if i % 7 == 0 { Err(10035) } else { Ok(i as u32) },
                &[&[(i % 3) as u8; 22], &[0xab; 5]],
            )
        })
        .collect();
    let bytes = encode_batch(&calls).unwrap();
    for len in 0..bytes.len() {
        assert!(decode_batch(&bytes[..len]).is_err(), "prefix {len}");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(decode_batch(&extra).is_err());
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..4000 {
        let mut copy = bytes.clone();
        for _ in 0..=(next() % 4) {
            let at = (next() % copy.len() as u64) as usize;
            copy[at] = next() as u8;
        }
        // Either way, no panic and no allocation sized by a lie: lengths
        // are checked against the bytes left.
        let _ = decode_batch(&copy);
    }
}

#[test]
fn a_declared_length_past_the_bytes_left_is_refused_without_allocating_it() {
    let mut bytes = vec![MAGIC];
    put(&mut bytes, 1);
    put(&mut bytes, 2);
    bytes.extend_from_slice(b"Op");
    put(&mut bytes, 1);
    for field in [0, 0, 0, 0, 0, 1] {
        put(&mut bytes, field);
    }
    bytes.push(RAW);
    put(&mut bytes, 16 << 20);
    assert!(decode_batch(&bytes).is_err());
    let at = bytes.len() - 5;
    bytes.truncate(at);
    bytes.push(RAW);
    put(&mut bytes, u64::MAX);
    assert!(decode_batch(&bytes).is_err());
}

#[test]
fn a_delta_or_repeat_with_no_earlier_output_is_refused() {
    for kind in [REPEAT, DELTA] {
        let mut bytes = vec![MAGIC];
        put(&mut bytes, 1);
        put(&mut bytes, 2);
        bytes.extend_from_slice(b"Op");
        put(&mut bytes, 1);
        for field in [0, 0, 0, 0, 0, 1] {
            put(&mut bytes, field);
        }
        bytes.push(kind);
        put(&mut bytes, 0);
        assert!(decode_batch(&bytes).is_err(), "kind {kind}");
    }
}
