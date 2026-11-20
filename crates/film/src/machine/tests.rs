use super::*;
use crate::{DATA, HEAD, NETR, Writer, blam, range};
use serde_json::{Value, json};
use std::io::Cursor;

fn film(key: bool) -> Vec<u8> {
    let mut h = crate::tests::header(1);
    h.version = 3;
    let mut writer = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    writer.chunk(DATA, &"private guest disk").unwrap();
    writer.chunk(NETR, &"boot service result").unwrap();
    writer.chronological(1, 1, &[1, 1]).unwrap();
    if key {
        writer.chunk_bytes(KEYF, &[1, 123]).unwrap();
    }
    writer.chunk(NETR, &"continuation").unwrap();
    writer.chronological(2, 2, &[2, 1]).unwrap();
    writer.chunk(blam::SUMM, &blam::Summary::default()).unwrap();
    writer.chunk(HEAD, &h).unwrap();
    writer.finish().unwrap().into_inner()
}

#[test]
fn private_bootstrap_and_continuation_share_a_section_even_without_a_snapshot() {
    for key in [true, false] {
        let bytes = film(key);
        let reader = Reader::new(Cursor::new(&bytes)).unwrap();
        let section = reader.machine_section();
        let runs: Vec<Vec<_>> = section
            .runs()
            .map(|rows| rows.iter().map(|r| r.tag).collect())
            .collect();
        assert_eq!(
            runs,
            vec![
                vec![HEAD, DATA, NETR],
                if key { vec![KEYF, NETR] } else { vec![NETR] },
                vec![HEAD],
            ]
        );
        assert_eq!(section.unique(DATA).unwrap().tag, DATA);
        assert!(section.unique(NETR).is_err());
        let dir = reader.ranges.as_ref().unwrap();
        assert!(section.keyframe_at(dir.chunks[0].offset).is_err());
        if key {
            assert_eq!(
                section.keyframe_at(dir.keyframes[0].offset).unwrap().tag,
                KEYF
            );
        } else {
            assert!(dir.keyframes.is_empty());
        }
        let raw = encode(&reader.entries).unwrap();
        let rows: Value = serde_json::from_slice(&raw).unwrap();
        let manifests: Vec<_> = rows
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["machine_span"].as_u64())
            .collect();
        assert_eq!(manifests, vec![3, if key { 2 } else { 1 }, 1]);
    }
}

fn decode_rows(rows: &Value) -> Result<Vec<Entry>> {
    let raw = serde_json::to_vec(rows)?;
    let mut de = serde_json::Deserializer::from_slice(&raw);
    Ok(de.deserialize_seq(Rows)?)
}

#[test]
fn private_index_proves_disjoint_runs_and_reads_earlier_flat_indexes() {
    let reader = Reader::new(Cursor::new(film(true))).unwrap();
    let rows: Value = serde_json::from_slice(&encode(&reader.entries).unwrap()).unwrap();
    assert_eq!(decode_rows(&rows).unwrap(), reader.entries);
    let flat = serde_json::to_value(&reader.entries).unwrap();
    assert_eq!(decode_rows(&flat).unwrap(), reader.entries);
    let tick = reader
        .entries
        .iter()
        .position(|e| e.tag == range::TICK)
        .unwrap();
    for (at, count) in [(0, 0), (0, 2), (0, 4), (1, 2), (tick, 1)] {
        let mut bad = rows.clone();
        bad[at]["machine_span"] = json!(count);
        assert!(decode_rows(&bad).is_err());
    }
    let mut mixed = rows.clone();
    mixed[0].as_object_mut().unwrap().remove("machine_span");
    assert!(decode_rows(&mixed).is_err());
}
