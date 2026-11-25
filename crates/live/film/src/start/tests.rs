use super::*;
use crate::blam::{MapIdentity, Mode, Seat, SeatKind};

pub(crate) fn sample() -> GameStart {
    GameStart {
        stamp: Stamp {
            xbe_sha256: "a".repeat(64),
            simulation_abi: "h2-sim/2".into(),
            contract_sha256: Some("b".repeat(64)),
            hooks_sha256: Some("c".repeat(64)),
            build: "image stamp".into(),
        },
        game: Game {
            started_ms: 1_700_000_000_000,
            first_update: 91,
            first_game_tick: 3,
            descriptor: MatchHeader {
                match_start_tick: Some(91),
                ticks_per_second: Some(30),
                map: MapIdentity {
                    name: "ascension".into(),
                    origin: "retail".into(),
                    sha256: "d".repeat(64),
                    ..Default::default()
                },
                game_type: "Slayer".into(),
                mode: Mode::Custom,
                roster: vec![Seat {
                    seat: 0,
                    name: "Chief".into(),
                    xuid: None,
                    team: 0,
                    kind: SeatKind::Local,
                    bot_params: None,
                }],
                ..Default::default()
            },
        },
        options: (0..OPTIONS_BYTES).map(|i| (i * 7) as u8).collect(),
        random: [0x1234_5678, 0x9abc_def0],
        start_hash: 0x0123_4567_89ab_cdef,
        settings: Settings {
            values: BTreeMap::from([("aim.assist".into(), 1), ("coop.max".into(), 4)]),
            switches: "-film-autorecord".into(),
        },
        variant: Some(b"script bytes".to_vec()),
        map_variant: Some(vec![9; 3000]),
    }
}

#[test]
fn a_game_start_round_trips_with_and_without_its_optional_sections() {
    let full = sample();
    assert_eq!(GameStart::decode(&full.encode().unwrap()).unwrap(), full);
    let bare = GameStart { variant: None, map_variant: None, ..sample() };
    let decoded = GameStart::decode(&bare.encode().unwrap()).unwrap();
    assert_eq!(decoded, bare);
    assert!(decoded.variant.is_none() && decoded.map_variant.is_none());
}

#[test]
fn every_truncation_of_a_game_start_is_refused() {
    for start in [sample(), GameStart { variant: None, map_variant: None, ..sample() }] {
        let bytes = start.to_bytes().unwrap();
        for len in 0..bytes.len() {
            assert!(GameStart::from_bytes(&bytes[..len]).is_err(), "{len} of {}", bytes.len());
        }
        let frame = start.encode().unwrap();
        for len in 0..frame.len() {
            assert!(GameStart::decode(&frame[..len]).is_err(), "frame {len}");
        }
    }
}

#[test]
fn made_up_and_damaged_game_starts_are_refused_without_a_panic() {
    let bytes = sample().to_bytes().unwrap();
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..300 {
        let len = (next() % 6000) as usize;
        let junk: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        assert!(GameStart::from_bytes(&junk).is_err());
        assert!(GameStart::decode(&junk).is_err());
    }
    // Damage to the framing is refused; damage inside a section's bytes may
    // still be a game start, but never a panic.
    for _ in 0..300 {
        let mut bad = bytes.clone();
        for _ in 0..3 {
            let at = (next() % bad.len() as u64) as usize;
            bad[at] = next() as u8;
        }
        let _ = GameStart::from_bytes(&bad);
    }
    for at in [0, 1, 2, 3, 4, 5, 6, 7, 8] {
        let mut bad = bytes.clone();
        bad[at] ^= 0xff;
        assert!(GameStart::from_bytes(&bad).is_err(), "framing byte {at}");
    }
}

/// Sections by hand, as a hostile writer would put them.
fn sections(list: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = (list.len() as u32).to_le_bytes().to_vec();
    for (tag, bytes) in list {
        out.extend_from_slice(tag);
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

fn valid_sections() -> Vec<([u8; 4], Vec<u8>)> {
    let s = sample();
    vec![
        (STMP, serde_json::to_vec(&s.stamp).unwrap()),
        (GAME, serde_json::to_vec(&s.game).unwrap()),
        (OPTS, s.options.clone()),
        (RAND, vec![0; 8]),
        (HASH, vec![0; 8]),
        (SETS, serde_json::to_vec(&s.settings).unwrap()),
    ]
}

#[test]
fn a_missing_repeated_unknown_or_overlong_section_is_refused() {
    assert!(GameStart::from_bytes(&sections(&valid_sections())).is_ok());
    for skip in 0..6 {
        let mut list = valid_sections();
        list.remove(skip);
        let why = GameStart::from_bytes(&sections(&list)).unwrap_err().to_string();
        assert!(why.contains("lacks section"), "{why}");
    }
    let mut list = valid_sections();
    list.push((HASH, vec![0; 8]));
    let why = GameStart::from_bytes(&sections(&list)).unwrap_err().to_string();
    assert!(why.contains("repeats section"), "{why}");
    let mut list = valid_sections();
    list.push((*b"JUNK", vec![1]));
    let why = GameStart::from_bytes(&sections(&list)).unwrap_err().to_string();
    assert!(why.contains("unknown section"), "{why}");
    // A length past the frame.
    let mut bytes = sections(&valid_sections());
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    let why = GameStart::from_bytes(&bytes).unwrap_err().to_string();
    assert!(why.contains("runs past its frame"), "{why}");
    // Bytes after the last section.
    let mut bytes = sections(&valid_sections());
    bytes.push(0);
    assert!(GameStart::from_bytes(&bytes).is_err());
    // A count that promises more sections than there are.
    let mut bytes = sections(&valid_sections());
    bytes[..4].copy_from_slice(&7u32.to_le_bytes());
    assert!(GameStart::from_bytes(&bytes).is_err());
    bytes[..4].copy_from_slice(&1000u32.to_le_bytes());
    assert!(GameStart::from_bytes(&bytes).is_err());
}

#[test]
fn an_options_block_of_another_size_is_refused() {
    for len in [0, 1, OPTIONS_BYTES - 1, OPTIONS_BYTES + 1, 2 * OPTIONS_BYTES] {
        let start = GameStart { options: vec![0; len], ..sample() };
        assert!(start.to_bytes().is_err(), "writing {len}");
        let mut list = valid_sections();
        list[2].1 = vec![0; len];
        let why = GameStart::from_bytes(&sections(&list)).unwrap_err().to_string();
        assert!(why.contains("not 4376"), "{why}");
    }
    // The random words and the hash have a size too.
    for (at, len) in [(3, 7), (3, 9), (4, 4), (4, 16)] {
        let mut list = valid_sections();
        list[at].1 = vec![0; len];
        assert!(GameStart::from_bytes(&sections(&list)).is_err(), "section {at} of {len}");
    }
}
