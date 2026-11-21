use super::*;

fn actor(entity: u32) -> Actor {
    Actor {
        entity,
        character: 0xabba,
        squad: 0x8765,
        team: 200,
        posture: 234,
    }
}

fn packet(records: Vec<Record>) -> Packet {
    Packet {
        tick: 0x8765_4321,
        records,
    }
}

fn all_records() -> Packet {
    let a = actor(0xbeef_1020);
    let b = actor(0xfeed_1020);
    packet(vec![
        Record::Key {
            actors: vec![a, b],
            state: vec![9, 2],
        },
        Record::Actor(ActorChange::Remove(a.entity)),
        Record::Actor(ActorChange::Upsert(b)),
        Record::Posture {
            entity: b.entity,
            posture: 203,
        },
        Record::Control {
            entity: b.entity,
            control: Control::quantize(0.25, -0.75, true, PI / 4.0),
        },
        Record::Line {
            entity: b.entity,
            vocalization: 0xffef,
            tick: 0x8765_431f,
        },
        Record::State(vec![3, 4]),
        Record::Digest {
            log_head: std::array::from_fn(|i| i as u8),
            state_hash: 0x1234_5678_9abc_def0,
            actor_count: 1,
        },
    ])
}

#[test]
fn every_record_round_trips() {
    let packet = all_records();
    let bytes = encode(&packet).unwrap();
    assert_eq!(bytes.len(), packet.encoded_len().unwrap());
    assert_eq!(decode(&bytes).unwrap(), packet);
    for end in 0..bytes.len() {
        assert_eq!(decode(&bytes[..end]), Err(WireError::Truncated));
    }
}

#[test]
fn version_one_line_has_fixed_bytes() {
    let packet = packet(vec![Record::Line {
        entity: 0x0102_0304,
        vocalization: 0x98fe,
        tick: 0x8765_4320,
    }]);
    let bytes = [
        1, 0x21, 0x43, 0x65, 0x87, 1, 4, 4, 3, 2, 1, 0xfe, 0x98, 0x20, 0x43, 0x65, 0x87,
    ];
    assert_eq!(encode(&packet).unwrap().as_slice(), &bytes);
    assert_eq!(decode(&bytes).unwrap(), packet);
}

#[test]
fn packed_controls_preserve_zero_endpoints_and_the_half_step_error_bound() {
    let neutral = Control::quantize(f32::NAN, 0.0, false, f32::NAN);
    assert_eq!(
        (
            neutral.forward(),
            neutral.side(),
            neutral.facing_delta_radians()
        ),
        (0.0, 0.0, 0.0)
    );
    let clamped = Control::quantize(f32::INFINITY, f32::NEG_INFINITY, true, 100.0);
    assert_eq!(
        (
            clamped.forward(),
            clamped.side(),
            clamped.facing_delta_radians()
        ),
        (1.0, -1.0, PI)
    );
    assert!(clamped.crouch());
    for i in -512..=512 {
        let x = i as f32 / 512.0;
        let angle = x * PI;
        let c = Control::quantize(x, -x, i % 2 == 0, angle);
        assert!((c.forward() - x).abs() <= 1.0 / 30.0 + f32::EPSILON);
        assert!((c.side() + x).abs() <= 1.0 / 30.0 + f32::EPSILON);
        assert!((c.facing_delta_radians() - angle).abs() <= PI / 30.0 + 4.0 * f32::EPSILON);
        assert_eq!(c.crouch(), i % 2 == 0);
        assert_eq!(Control::from_bits(c.0).unwrap(), c);
    }
}

#[test]
fn both_directions_enforce_exact_limits() {
    let mut p = packet(vec![Record::State(vec![9; 63]); 15]);
    p.records.push(Record::State(vec![4; 17]));
    let mut bytes = encode(&p).unwrap();
    assert_eq!(bytes.len(), MAX_PACKET_BYTES);
    assert_eq!(decode(&bytes).unwrap(), p);
    let Record::State(last) = p.records.last_mut().unwrap() else {
        unreachable!()
    };
    last.push(1);
    assert_eq!(encode(&p), Err(WireError::PacketTooLarge));
    bytes.push(1);
    assert_eq!(decode(&bytes), Err(WireError::PacketTooLarge));

    let mut p = packet(vec![Record::State(Vec::new()); MAX_RECORDS]);
    let mut bytes = encode(&p).unwrap();
    assert_eq!(decode(&bytes).unwrap(), p);
    p.records.push(Record::State(Vec::new()));
    assert_eq!(encode(&p), Err(WireError::TooManyRecords));
    bytes[HEADER_LEN - 1] += 1;
    assert_eq!(decode(&bytes), Err(WireError::TooManyRecords));

    let actors: Vec<_> = (0..MAX_ACTORS as u32).map(actor).collect();
    let mut p = packet(vec![Record::Key {
        actors,
        state: vec![1; MAX_STATE_BYTES],
    }]);
    let mut bytes = encode(&p).unwrap();
    assert_eq!(decode(&bytes).unwrap(), p);
    let Record::Key { actors, state } = &mut p.records[0] else {
        unreachable!()
    };
    actors.push(actor(900));
    state.clear();
    assert_eq!(encode(&p), Err(WireError::TooManyActors));
    bytes[HEADER_LEN + 1] += 1;
    assert_eq!(decode(&bytes), Err(WireError::TooManyActors));
}

#[test]
fn rejects_invalid_version_tags_payloads_and_entity_names() {
    fn malformed(record: &[u8]) -> WireError {
        let mut bytes = vec![VERSION, 0, 0, 0, 0, 1];
        bytes.extend(record);
        decode(&bytes).unwrap_err()
    }

    assert_eq!(decode(&[2]), Err(WireError::UnsupportedVersion(2)));
    assert_eq!(
        decode(&[VERSION, 0, 0, 0, 0, 0, 9]),
        Err(WireError::TrailingBytes)
    );
    assert_eq!(malformed(&[255]), WireError::UnknownRecord(255));
    assert_eq!(malformed(&[ACTOR, 2]), WireError::InvalidActorChange(2));
    for field in [0, 5, 11] {
        let bits = (31u16 << field).to_le_bytes();
        assert_eq!(
            malformed(&[CONTROL, 1, 0, 0, 0, bits[0], bits[1]]),
            WireError::InvalidControl
        );
    }
    assert_eq!(malformed(&[STATE, 64]), WireError::StateTooLarge);
    assert_eq!(malformed(&[KEY, 0, 64]), WireError::StateTooLarge);
    assert_eq!(
        malformed(&[ACTOR, 0, 255, 255, 255, 255]),
        WireError::InvalidEntity
    );
    let invalid = packet(vec![Record::Posture {
        entity: u32::MAX,
        posture: 7,
    }]);
    assert_eq!(encode(&invalid), Err(WireError::InvalidEntity));
    let duplicate = packet(vec![Record::Key {
        actors: vec![actor(17); 2],
        state: vec![],
    }]);
    assert_eq!(encode(&duplicate), Err(WireError::DuplicateEntity));
    let mut body = vec![KEY, 2];
    for _ in 0..2 {
        put_actor(&mut body, &actor(17));
    }
    body.push(0);
    assert_eq!(malformed(&body), WireError::DuplicateEntity);
}

struct Seed(u32);

impl Seed {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

#[test]
fn decode_never_panics_on_fuzz() {
    let valid = encode(&all_records()).unwrap();
    let mut rng = Seed(0x9826_3beb);
    for i in 0..10_000 {
        let mut bytes = match i % 3 {
            0 => valid.clone(),
            1 => valid[..(rng.next() as usize % (valid.len() + 1))].to_vec(),
            _ => {
                let len = rng.next() as usize % (MAX_PACKET_BYTES + 17);
                let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                if let Some(version) = bytes.first_mut() {
                    *version = VERSION;
                }
                bytes
            }
        };
        if !bytes.is_empty() {
            let at = rng.next() as usize % bytes.len();
            bytes[at] ^= rng.next() as u8;
        }
        if let Ok(packet) = decode(&bytes) {
            assert!(packet.records.len() <= MAX_RECORDS);
            assert!(bytes.len() <= MAX_PACKET_BYTES);
            assert_eq!(encode(&packet).unwrap(), bytes);
        }
    }
}

#[test]
fn encoder_checks_generated_bounds_before_writing() {
    let mut rng = Seed(0xcd22_0611);
    for _ in 0..10_000 {
        let count = rng.next() as usize % (MAX_RECORDS + 3);
        let actor_count = rng.next() as usize % (MAX_ACTORS + 3);
        let state_len = rng.next() as usize % (MAX_STATE_BYTES + 3);
        let actors = (0..actor_count as u32).map(actor).collect();
        let mut records = vec![Record::State(vec![0; state_len]); count];
        records.push(Record::Key {
            actors,
            state: Vec::new(),
        });
        let packet = packet(records);
        let expected = HEADER_LEN + count * (2 + state_len) + 3 + actor_count * ACTOR_LEN;
        let fits = count < MAX_RECORDS
            && actor_count <= MAX_ACTORS
            && (count == 0 || state_len <= MAX_STATE_BYTES)
            && expected <= MAX_PACKET_BYTES;
        match encode(&packet) {
            Ok(bytes) => {
                assert!(fits);
                assert_eq!(bytes.len(), expected);
                assert_eq!(decode(&bytes).unwrap(), packet);
            }
            Err(_) => assert!(!fits),
        }
    }
}
