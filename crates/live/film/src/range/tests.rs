use super::*;
use crate::Writer;
use std::io::{self, Cursor};
use std::ops::Range;

fn film() -> Vec<u8> {
    let mut h = crate::tests::header(7);
    h.version = 3;
    let mut w = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    w.chunk(*b"PRIV", &"PRIVATE-GUEST-DISK").unwrap();
    w.chunk(
        blam::DESC,
        &blam::MatchHeader {
            ticks_per_second: Some(60),
            ..Default::default()
        },
    )
    .unwrap();
    w.chronological_scene(1, 6, true, &[1, 6]).unwrap();
    w.chunk_bytes(KEYF, &[6, 0, 0, 0]).unwrap();
    w.chronological_scene(7, 8, false, &[7, 2]).unwrap();
    w.chronological_scene(9, 11, true, &[9, 3]).unwrap();
    w.chunk(
        blam::SUMM,
        &blam::Summary {
            level: "ascension".into(),
            duration_ticks: 11,
            ..Default::default()
        },
    )
    .unwrap();
    h.meta.length_ticks = 11;
    w.chunk(HEAD, &h).unwrap();
    w.finish().unwrap().into_inner()
}

/// Mimic an HTTP server which will refuse requests for the host's private ranges.
struct Allowed<'a> {
    data: Cursor<&'a [u8]>,
    allowed: &'a [Range<u64>],
}
impl<'a> Allowed<'a> {
    fn new(bytes: &'a [u8], allowed: &'a [Range<u64>]) -> Self {
        Self {
            data: Cursor::new(bytes),
            allowed,
        }
    }
}
impl Read for Allowed<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let at = self.data.position();
        let range = self
            .allowed
            .iter()
            .find(|r| r.contains(&at))
            .ok_or_else(|| io::Error::other("read outside requested film ranges"))?;
        let limit = (range.end - at).min(out.len() as u64) as usize;
        self.data.read(&mut out[..limit])
    }
}
impl Seek for Allowed<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.data.seek(pos)
    }
}

#[test]
fn independent_summary_directory_and_time_ranges() {
    let bytes = film();
    let len = bytes.len() as u64;
    let f = footer(&mut Cursor::new(&bytes), len).unwrap();
    let front_len = Prefix::decode(&bytes[..PREFIX_BYTES]).unwrap().json_len;
    let metadata = 0..PREFIX_BYTES as u64 + u64::from(front_len);
    let front = read_front(&mut Allowed::new(&bytes, std::slice::from_ref(&metadata))).unwrap();
    assert_eq!(front.summary.level, "ascension");
    assert_eq!(front.image_stamp, "image A");
    assert_eq!(front.ticks_per_second, Some(60));
    let json = std::str::from_utf8(&bytes[PREFIX_BYTES..metadata.end as usize]).unwrap();
    assert!(!json.contains("xnet_addr") && !json.contains("127.0.0.1"));
    let directory_range = f.cidx_offset..f.indx_offset;
    let index = directory(
        &mut Allowed::new(&bytes, std::slice::from_ref(&directory_range)),
        &f,
    )
    .unwrap();
    let rows = index.covering(7, 9);
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].first, rows[0].scene, rows[1].scene), (7, 1, 9));
    assert!(index.covering(12, 20).is_empty());
    assert_eq!(index.nearest_keyframe(8).unwrap().tick, 6);
    assert!(index.nearest_keyframe(5).is_none());
    let c = &rows[0];
    let allowed = [
        metadata,
        directory_range,
        len - FOOTER_BYTES as u64..len,
        c.offset..c.offset + 8 + u64::from(c.size),
    ];
    let mut r = RangeReader::new(Allowed::new(&bytes, &allowed)).unwrap();
    assert_eq!(r.chunk(1).unwrap(), [7, 2]);
    assert!(r.chunk(0).is_err());
    let local = Reader::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(local.ranges.unwrap(), index);
    assert_eq!(local.front.unwrap(), front);
    assert_eq!(
        listing(&mut Cursor::new(&bytes))
            .unwrap()
            .unwrap()
            .0
            .meta
            .length_ticks,
        11
    );
}

#[test]
fn corrupt_public_chunk_is_localised_and_truncated_footer_is_rejected() {
    let mut bytes = film();
    let mut r = RangeReader::new(Cursor::new(&bytes)).unwrap();
    let c = r.directory.chunks[1].clone();
    assert_eq!(r.chunk(1).unwrap(), [7, 2]);
    bytes[(c.offset + 7 + u64::from(c.size)) as usize] ^= 1; // zstd checksum
    let mut r = RangeReader::new(Cursor::new(&bytes)).unwrap();
    assert!(r.chunk(1).is_err());
    assert_eq!(r.chunk(0).unwrap(), [1, 6]);
    let len = bytes.len();
    for n in [
        PREFIX_BYTES - 1,
        DATA_OFFSET as usize,
        len - FOOTER_BYTES,
        len - 1,
    ] {
        assert!(RangeReader::new(Cursor::new(&bytes[..n])).is_err());
    }
    bytes[8..12].copy_from_slice(&4u32.to_le_bytes());
    assert!(Reader::new(Cursor::new(&bytes)).is_err());
}

#[test]
fn reject_invalid_scene_links_clock_and_overlapping_ranges() {
    let bytes = film();
    let f = footer(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    let index = directory(&mut Cursor::new(&bytes), &f).unwrap();
    for (field, value) in [
        ("first", 1),
        ("last", u64::MAX),
        ("offset", f.cidx_offset),
        ("size", 0),
        ("raw_size", u64::from(u32::MAX)),
        ("scene", 2),
    ] {
        let mut rows = serde_json::to_value(&index).unwrap();
        rows["chunks"][1][field] = value.into();
        let invalid: Directory = serde_json::from_value(rows).unwrap();
        assert!(invalid.validate(&f).is_err(), "{field}");
    }
    let mut h = crate::tests::header(1);
    h.version = 3;
    let mut w = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    assert!(w.chronological_scene(1, 1, false, &[1, 1]).is_err());
    assert!(w.chronological(1, 1, &[0x81, 0, 1]).is_err());
    w.chronological(1, 1, &[1, 1]).unwrap();
    assert!(w.chronological(1, 1, &[1, 1]).is_err());
    assert!(
        w.chronological_scene(2100, 2100, false, &[0xb4, 0x10, 1])
            .is_err()
    );
    for tick in 2..=MAX_SCENE_CHUNKS as u8 {
        let t = u64::from(tick);
        w.chronological_scene(t, t, false, &[tick, 1]).unwrap();
    }
    assert!(w.chronological_scene(17, 17, false, &[17, 1]).is_err());
    w.chronological_scene(17, 17, true, &[17, 1]).unwrap();
    assert!(w.finish().is_err()); // incomplete front summary
}

#[test]
fn guest_clock_index_handles_pause_revert_and_missing_observations() {
    let mut header = crate::tests::header(1);
    header.version = 3;
    let mut w = Writer::new(Cursor::new(Vec::new()), &header).unwrap();
    let mut first = blam::stream::ChunkWriter::new(1, 5).unwrap();
    for (tick, clock) in [(1, None), (2, Some(90)), (4, Some(91)), (5, None)] {
        let mut bytes = Vec::new();
        if let Some(clock) = clock {
            blam::stream::write_varint(clock, &mut bytes);
        }
        first.record(tick, blam::stream::CLOCK, &bytes).unwrap();
    }
    w.chronological(1, 5, &first.finish()).unwrap();
    let mut second = blam::stream::ChunkWriter::new(6, 4).unwrap();
    second.record(6, blam::stream::CLOCK, &[90]).unwrap();
    w.chronological(6, 9, &second.finish()).unwrap();
    w.chunk(blam::SUMM, &blam::Summary::default()).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let index = RangeReader::new(Cursor::new(bytes)).unwrap().directory;
    assert_eq!(
        index.chunks[0].clock,
        [(1, Some(90)), (3, Some(91)), (4, None)]
    );
    assert_eq!(index.chunks[0].guest_tick(1), None);
    assert_eq!(index.chunks[0].guest_tick(3), Some(91));
    assert_eq!(index.chunks[0].guest_tick(4), Some(91));
    assert_eq!(index.chunks[0].guest_tick(5), None);
    assert_eq!(index.film_tick_for_guest(90), Some(2));
    assert_eq!(index.film_tick_for_guest(91), Some(3));
    assert_eq!(index.film_tick_for_guest(92), Some(8));
    assert_eq!(index.film_tick_for_guest(94), None);
    assert_eq!(index.film_range_for_guest(90, 92, 1, 90), Some([2, 8]));
    assert_eq!(index.film_range_for_guest(90, 94, 6, 90), Some([6, 10]));
    assert_eq!(index.film_range_for_guest(94, 96, 6, 90), None);
    assert_eq!(index.film_range_for_guest(94, 95, 1, 90), Some([5, 6])); // no clock at 5
}

#[test]
fn range_directory_and_writer_reject_a_missing_first_or_interior_tick() {
    let bytes = film();
    let footer = footer(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    let valid = directory(&mut Cursor::new(&bytes), &footer).unwrap();
    for row in [0, 1] {
        let mut index = valid.clone();
        index.chunks[row].first += 1;
        if row == 0 {
            index.chunks[0].scene = 2;
            index.chunks[1].scene = 2;
        }
        assert!(index.validate(&footer).is_err());
    }
    let mut h = crate::tests::header(3);
    h.version = 3;
    let mut w = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    assert!(w.chronological(2, 2, &[2, 1]).is_err());
    w.chronological(1, 1, &[1, 1]).unwrap();
    assert!(w.chronological(3, 3, &[3, 1]).is_err());
    w.chronological(2, 3, &[2, 2]).unwrap();
}

/// A v3 film with the roster and results of `names`, sealed.
fn film_of(names: &[String], summary_written_first: bool) -> Vec<u8> {
    let mut h = crate::tests::header(8);
    h.version = 3;
    let mut w = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    w.chronological_scene(1, 2, true, &[1, 2]).unwrap();
    let summary = blam::Summary {
        level: "zanzibar".into(),
        players: names
            .iter()
            .enumerate()
            .map(|(i, name)| live_proto::film::PlayerSummary {
                seat: i as u8,
                name: name.clone(),
                medals: (0..40).map(|m| (format!("medal {m}"), 1)).collect(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let desc = blam::MatchHeader {
        roster: names
            .iter()
            .enumerate()
            .map(|(i, name)| blam::Seat { seat: i as u8, name: name.clone(), ..Default::default() })
            .collect(),
        ..Default::default()
    };
    // A game described late has its descriptor after the results.
    if summary_written_first {
        w.chunk(blam::SUMM, &summary).unwrap();
        w.chunk(blam::DESC, &desc).unwrap();
    } else {
        w.chunk(blam::DESC, &desc).unwrap();
        w.chunk(blam::SUMM, &summary).unwrap();
    }
    w.chunk(HEAD, &h).unwrap();
    w.finish().unwrap().into_inner()
}

#[test]
fn a_film_described_late_has_a_front_summary() {
    let bytes = film_of(&["one".into(), "two".into()], true);
    let front = read_front(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(front.summary.level, "zanzibar");
    assert_eq!(front.roster.len(), 2);
}

#[test]
fn an_unsealed_film_has_a_front_naming_its_game() {
    let mut h = crate::tests::header(9);
    h.version = 3;
    let mut out = Cursor::new(Vec::new());
    {
        let mut w = Writer::new(&mut out, &h).unwrap();
        // Not described, and the recording stops here: never finished.
        w.chronological_scene(1, 2, true, &[1, 2]).unwrap();
    }
    let front = read_front(&mut out).unwrap();
    assert_eq!(front.id, "9");
    assert!(front.roster.is_empty());
    // The descriptor, once it arrives, is in the front before any seal.
    let mut out = Cursor::new(Vec::new());
    {
        let mut w = Writer::new(&mut out, &h).unwrap();
        let seat = blam::Seat { seat: 3, name: "someone".into(), ..Default::default() };
        w.chunk(blam::DESC, &blam::MatchHeader { roster: vec![seat], ..Default::default() }).unwrap();
        w.chronological_scene(1, 2, true, &[1, 2]).unwrap();
    }
    let front = read_front(&mut out).unwrap();
    assert_eq!(front.roster[0].name, "someone");
}

#[test]
fn a_big_roster_stays_under_the_front_limit() {
    let names: Vec<String> = (0..blam::MAX_SEATS).map(|i| format!("{i}{}", "x".repeat(6_000))).collect();
    let bytes = film_of(&names, false);
    let len = Prefix::decode(&bytes[..PREFIX_BYTES]).unwrap().json_len as usize;
    assert!(len <= FRONT_CAPACITY);
    let front = read_front(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(front.roster.len(), blam::MAX_SEATS);
    assert_eq!(front.summary.players.len(), blam::MAX_SEATS);
    assert_eq!(front.summary.level, "zanzibar");
}

#[test]
fn the_prefix_names_why_a_front_is_missing_or_too_big() {
    let mut bytes = film_of(&["one".into()], false)[..PREFIX_BYTES].to_vec();
    bytes[16..20].copy_from_slice(&0u32.to_le_bytes());
    let missing = Prefix::decode(&bytes).unwrap_err();
    assert!(missing.contains("no front summary") && missing.contains("0 of"), "{missing}");
    bytes[16..20].copy_from_slice(&(FRONT_CAPACITY as u32 + 1).to_le_bytes());
    let big = Prefix::decode(&bytes).unwrap_err();
    assert!(big.contains(&format!("{} bytes", FRONT_CAPACITY + 1)), "{big}");
}

#[test]
fn a_refreshed_front_names_the_game_so_far_and_the_seal_replaces_it() {
    let mut h = crate::tests::header(10);
    h.version = 3;
    let mut out = Cursor::new(Vec::new());
    let seat = blam::Seat { seat: 1, name: "one".into(), ..Default::default() };
    let desc = blam::MatchHeader { roster: vec![seat], ..Default::default() };
    let mut summary = blam::Summary { level: "zanzibar".into(), duration_ticks: 30, ..Default::default() };
    {
        let mut w = Writer::new(&mut out, &h).unwrap();
        w.chronological_scene(1, 2, true, &[1, 2]).unwrap();
        w.refresh_front(&desc, &summary).unwrap();
        // The next refresh replaces the last, and writing goes on after it.
        summary.duration_ticks = 60;
        w.refresh_front(&desc, &summary).unwrap();
        w.chronological_scene(3, 4, false, &[3, 2]).unwrap();
        // Never finished: a crash.
    }
    let front = read_front(&mut out).unwrap();
    assert_eq!((front.summary.level.as_str(), front.summary.duration_ticks), ("zanzibar", 60));
    assert_eq!(front.roster[0].name, "one");

    // The seal writes the final summary over it, and refresh stays callable
    // for a film that is not v3 (it has no front).
    let mut w = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    w.refresh_front(&desc, &summary).unwrap();
    w.chronological_scene(1, 2, true, &[1, 2]).unwrap();
    summary.duration_ticks = 99;
    w.chunk(blam::DESC, &desc).unwrap();
    w.chunk(blam::SUMM, &summary).unwrap();
    w.chunk(HEAD, &h).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    assert_eq!(read_front(&mut Cursor::new(&bytes)).unwrap().summary.duration_ticks, 99);
    let mut old = crate::tests::header(11);
    old.version = 2;
    Writer::new(Cursor::new(Vec::new()), &old).unwrap().refresh_front(&desc, &summary).unwrap();
}
