use super::*;

fn obj(id: Guid, body: &[u8]) -> Vec<u8> {
    let mut b = id.to_vec();
    b.extend((body.len() as u64 + 24).to_le_bytes());
    b.extend(body);
    b
}
fn packet(number: u8, offset: u32, size: u32, payload: &[u8]) -> Vec<u8> {
    let mut b = vec![0x82, 0, 0, 0x08, 0x5d, 0];
    b.extend(100u32.to_le_bytes());
    b.extend(20u16.to_le_bytes());
    b.extend([0x81, number]);
    b.extend(offset.to_le_bytes());
    b.push(8);
    b.extend(size.to_le_bytes());
    b.extend(100u32.to_le_bytes());
    b.extend(payload);
    b[5] = (64 - b.len()) as u8;
    b.resize(64, 0);
    b
}
fn file(packets: &[Vec<u8>]) -> Vec<u8> {
    let mut props = vec![0; 80];
    props[32..40].copy_from_slice(&(packets.len() as u64).to_le_bytes());
    props[64..68].copy_from_slice(&2u32.to_le_bytes());
    props[68..72].copy_from_slice(&64u32.to_le_bytes());
    props[72..76].copy_from_slice(&64u32.to_le_bytes());
    let mut stream = AUDIO.to_vec();
    stream.extend(NO_ERROR_CORRECTION);
    stream.extend(0u64.to_le_bytes());
    stream.extend(18u32.to_le_bytes());
    stream.extend(0u32.to_le_bytes());
    stream.extend(1u16.to_le_bytes());
    stream.extend(0u32.to_le_bytes());
    stream.extend(1u16.to_le_bytes());
    stream.extend(2u16.to_le_bytes());
    stream.extend(44100u32.to_le_bytes());
    stream.extend(176400u32.to_le_bytes());
    stream.extend(4u16.to_le_bytes());
    stream.extend(16u16.to_le_bytes());
    stream.extend(0u16.to_le_bytes());
    let mut h = vec![3, 0, 0, 0, 1, 2];
    h.extend(obj(FILE_PROPERTIES, &props));
    h.extend(obj(HEADER_EXTENSION, &[0; 22]));
    h.extend(obj(STREAM_PROPERTIES, &stream));
    let mut b = obj(HEADER, &h);
    let mut d = vec![0; 16];
    d.extend((packets.len() as u64).to_le_bytes());
    d.extend([1, 1]);
    for p in packets {
        d.extend(p);
    }
    b.extend(obj(DATA, &d));
    let len = b.len() as u64;
    b[70..78].copy_from_slice(&len.to_le_bytes());
    b
}
#[test]
fn synthetic_asf_reassembles_fragments() {
    let b = file(&[
        packet(255, 0, 6, &[1, 2]),
        packet(255, 2, 6, &[3, 4, 5, 6]),
        packet(0, 0, 2, &[7, 8]),
    ]);
    let a = Asf::parse(&b).unwrap();
    assert_eq!(a.streams[0].audio.as_ref().unwrap().sample_rate, 44100);
    a.streams[0].check_audio_spread().unwrap();
    let mut r = Reassembler::new(1, 16).unwrap();
    let mut objects = Vec::new();
    for p in a.packets() {
        for p in p.unwrap().payloads {
            if let Some(o) = r.push(&p).unwrap() {
                objects.push(o);
            }
        }
    }
    r.finish().unwrap();
    assert_eq!(objects.len(), 2);
    assert_eq!(objects[0].data, [1, 2, 3, 4, 5, 6]);
    assert_eq!(objects[1].number, 0);
}
#[test]
fn all_file_truncations_fail() {
    let b = file(&[packet(0, 0, 4, &[1, 2, 3, 4])]);
    for end in 0..b.len() {
        assert!(Asf::parse(&b[..end]).is_err(), "end {end}");
    }
}
#[test]
fn malicious_sizes_and_counts_fail() {
    let original = file(&[packet(0, 0, 4, &[1, 2, 3, 4])]);
    for (offset, value) in [(16, u64::MAX), (16, 23), (46, u64::MAX)] {
        let mut b = original.clone();
        b[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        assert!(Asf::parse(&b).is_err());
    }
    let mut b = original;
    b[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Asf::parse(&b).is_err());
}
#[test]
fn invalid_packet_padding_and_replicated_data_fail() {
    let b = packet(0, 0, 4, &[1, 2, 3, 4]);
    let mut bad = b.clone();
    bad[5] = 255;
    assert!(Packet::parse(&bad).is_err());
    let mut bad = b.clone();
    bad[18] = 7;
    assert!(Packet::parse(&bad).is_err());
    let mut bad = b;
    bad[19..23].copy_from_slice(&3u32.to_le_bytes());
    assert!(Packet::parse(&bad).is_err());
}
#[test]
fn compressed_multi_payloads_expand_with_time_and_number_wrap() {
    // Multiple payloads, word-sized explicit packet/padding lengths, byte sequence.
    let mut b = vec![0x53, 0x5d, 64, 0, 42, 0, 0];
    b.extend(100u32.to_le_bytes());
    b.extend(30u16.to_le_bytes());
    b.push(0x81); // one outer payload, word length
    b.extend([0x81, 255]);
    b.extend(100u32.to_le_bytes());
    b.extend([1, 10]); // compressed, timestamp delta 10
    b.extend(7u16.to_le_bytes());
    b.extend([2, 1, 2, 3, 3, 4, 5]);
    let pad = 64 - b.len();
    b[5..7].copy_from_slice(&(pad as u16).to_le_bytes());
    b.resize(64, 0);
    let p = Packet::parse(&b).unwrap();
    assert_eq!(p.payloads.len(), 2);
    assert_eq!(p.payloads[0].data, [1, 2]);
    assert_eq!(p.payloads[1].data, [3, 4, 5]);
    assert_eq!(p.payloads[1].object_number, 0);
    assert_eq!(p.payloads[1].presentation_time_ms, Some(110));
}
#[test]
fn reassembly_rejects_gaps_overlaps_oversizes_and_truncation() {
    let b = packet(1, 0, 8, &[1, 2]);
    let p = Packet::parse(&b).unwrap().payloads[0];
    for offset in [1, 3] {
        let mut r = Reassembler::new(1, 8).unwrap();
        r.push(&p).unwrap();
        assert!(r.push(&Payload { offset, ..p }).is_err());
        assert!(r.finish().is_err());
    }
    let mut r = Reassembler::new(1, 8).unwrap();
    r.push(&p).unwrap();
    assert!(r.finish().is_err());
    let mut r = Reassembler::new(1, 7).unwrap();
    assert!(r.push(&p).is_err());
    let mut r = Reassembler::new(1, 8).unwrap();
    assert!(
        r.push(&Payload {
            object_size: None,
            ..p
        })
        .is_err()
    );
}
#[test]
fn mutation_smoke_no_panics() {
    let b = file(&[packet(0, 0, 4, &[1, 2, 3, 4])]);
    for i in 0..b.len() {
        for v in [0, 1, 0x7f, 0x80, 0xff] {
            let mut b = b.clone();
            b[i] = v;
            if let Ok(a) = Asf::parse(&b) {
                for p in a.packets() {
                    let _ = p;
                }
            }
        }
    }
    let b = packet(0, 0, 4, &[1, 2, 3, 4]);
    for i in 0..b.len() {
        for v in 0..=255 {
            let mut b = b.clone();
            b[i] = v;
            let _ = Packet::parse(&b);
        }
    }
}
