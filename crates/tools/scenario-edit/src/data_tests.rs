//! The scenario's data references that change length
//! ([`crate::scenario::data`]), on an invented cache: every byte below is
//! made up here. No layout value, name or data comes from a game file.
use crate::edit::{Command, History};
use crate::fixture::*;
use crate::save::{self, MapSource};
use crate::scenario::Scenario;
use crate::scenario::data::{DATA_OUTSIDE, DATA_OWNED, DATA_RUNS_PAST};
use crate::session::DATA_GROWTH_REFUSAL;
use map::Map;
use std::io::Cursor;

/// The invented data reference's pair and its source bytes, as payload
/// offsets.
const PAIR_AT: usize = 0xf00;
const DATA_AT: usize = 0xf10;
const SOURCE: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

fn pair() -> u32 {
    META + (SCNR + PAIR_AT) as u32
}
fn data_address() -> u32 {
    META + (SCNR + DATA_AT) as u32
}

/// The fixture (metadata ending the file) with an invented 8-byte data
/// reference in the scenario's payload, outside every placement block.
fn with_data() -> Vec<u8> {
    let mut b = fixture_at_end();
    put(&mut b, sfile(PAIR_AT), SOURCE.len() as u32);
    put(&mut b, sfile(PAIR_AT) + 4, data_address());
    b[sfile(DATA_AT)..sfile(DATA_AT) + SOURCE.len()].copy_from_slice(&SOURCE);
    b
}

fn open(bytes: Vec<u8>) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(bytes)).unwrap()
}
fn read(bytes: Vec<u8>) -> Scenario {
    Scenario::read(&mut open(bytes)).unwrap()
}
fn write(src: &[u8], s: &Scenario) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    save::write_patched(&mut open(src.to_vec()), s, &mut out).unwrap();
    out.into_inner()
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
/// The `len` bytes at virtual `address` of a saved map.
fn read_at(bytes: &[u8], address: u32, len: u32) -> Vec<u8> {
    let mut m = open(bytes.to_vec());
    let at = m.file_offset(address, len).unwrap();
    m.read_file_range(at, len.into()).unwrap()
}

#[test]
fn a_shorter_payload_saves_in_place_and_reads_back() {
    let src = with_data();
    let mut s = read(src.clone());
    assert_eq!(s.data(pair()).as_deref(), Some(&SOURCE[..]));
    assert_eq!(s.data_capacity(pair()), 8);
    assert!(!s.data_grows(pair(), 3));

    s.resize_data(pair(), vec![9, 9, 9]).unwrap();
    assert!(s.is_modified());
    let c = s.changes();
    assert!(c.tag_grown.is_empty());
    assert!(c.patches.contains(&(data_address(), vec![9, 9, 9])));
    assert!(c.patches.contains(&(pair(), 3u32.to_le_bytes().to_vec())));

    let out = write(&src, &s);
    assert_eq!(out.len(), src.len(), "nothing appended");
    assert_eq!(word(&out, sfile(PAIR_AT)), 3);
    assert_eq!(word(&out, sfile(PAIR_AT) + 4), data_address());
    // The tail of the source stays as dead bytes.
    assert_eq!(
        &out[sfile(DATA_AT)..sfile(DATA_AT) + 8],
        [9, 9, 9, 4, 5, 6, 7, 8]
    );
    open(out.clone()).validate_checksum().unwrap();
    let back = read(out);
    assert_eq!(back.data(pair()).as_deref(), Some(&[9u8, 9, 9][..]));
    assert!(!back.is_modified());
}

#[test]
fn a_longer_payload_is_appended_and_the_old_bytes_are_unreferenced() {
    let src = with_data();
    let mut s = read(src.clone());
    let longer: Vec<u8> = (100..124).collect();
    assert!(s.data_grows(pair(), longer.len() as u32));
    s.resize_data(pair(), longer.clone()).unwrap();
    let c = s.changes();
    assert_eq!(c.tag_grown.len(), 1);
    assert_eq!(
        (c.tag_grown[0].root, c.tag_grown[0].count),
        (pair(), longer.len() as u32)
    );
    assert!(!c.patches.iter().any(|(a, _)| *a == pair()));

    let out = write(&src, &s);
    assert!(out.len() > src.len(), "the copy is appended");
    assert_eq!(word(&out, sfile(PAIR_AT)), longer.len() as u32);
    let at = word(&out, sfile(PAIR_AT) + 4);
    assert_ne!(at, data_address());
    assert!(at >= META + 0x2000, "past the source metadata: {at:#x}");
    assert_eq!(read_at(&out, at, longer.len() as u32), longer);
    // The source bytes are left where they were, and nothing points there.
    assert_eq!(&out[sfile(DATA_AT)..sfile(DATA_AT) + 8], SOURCE);
    open(out.clone()).validate_checksum().unwrap();

    // Reopened, the data lies outside the payload: it has no room in
    // place, so even a shorter edit is a new copy.
    let back = read(out);
    assert_eq!(back.data(pair()), None);
    assert_eq!(back.data_capacity(pair()), 0);
    assert!(back.data_grows(pair(), 1));
}

#[test]
fn an_empty_payload_patches_only_the_size() {
    let src = with_data();
    let mut s = read(src.clone());
    s.resize_data(pair(), Vec::new()).unwrap();
    let c = s.changes();
    assert_eq!(c.patches, vec![(pair(), 0u32.to_le_bytes().to_vec())]);
    let out = write(&src, &s);
    assert_eq!(word(&out, sfile(PAIR_AT)), 0);
}

#[test]
fn undo_restores_the_pair_and_the_bytes() {
    let src = with_data();
    let mut s = read(src.clone());
    let before = s.changes();
    let mut h = History::default();
    h.apply(&mut s, Command::resize_data(pair(), vec![7; 3], "strings"))
        .unwrap();
    h.apply(&mut s, Command::resize_data(pair(), vec![6; 40], "strings"))
        .unwrap();
    assert_eq!(s.changes().tag_grown.len(), 1);
    assert_eq!(h.undo(&mut s).unwrap().as_deref(), Some("resize strings"));
    assert_eq!(s.data(pair()), Some(vec![7; 3]));
    assert!(s.changes().tag_grown.is_empty());
    h.undo(&mut s).unwrap();
    assert_eq!(s.data(pair()).as_deref(), Some(&SOURCE[..]));
    assert_eq!(s.changes(), before);
    assert!(!s.is_modified());
    assert_eq!(write(&src, &s), src, "an undone resize writes nothing");
    h.redo(&mut s).unwrap();
    assert_eq!(s.data(pair()), Some(vec![7; 3]));
}

#[test]
fn the_source_bytes_again_are_no_change() {
    let mut s = read(with_data());
    s.resize_data(pair(), vec![0; 2]).unwrap();
    s.resize_data(pair(), SOURCE.to_vec()).unwrap();
    assert!(!s.is_modified());
    assert!(s.changes().is_empty());
}

#[test]
fn refused_data_references_are_named() {
    let s = read(with_data());
    // In another tag (the invented weapon), not the scenario's payload.
    assert_eq!(s.data_pair_refusal(META + 0x1100), Some(DATA_OUTSIDE));
    // In a placement block's elements, and a block's root pair.
    let weapons = META + (SCNR + WEAPONS_AT) as u32;
    assert_eq!(s.data_pair_refusal(weapons), Some(DATA_OWNED));
    let root = META + SCNR as u32 + crate::layout::BLOCKS[0].offset;
    assert_eq!(s.data_pair_refusal(root), Some(DATA_OWNED));
    assert_eq!(s.data_pair_refusal(pair()), None);

    // Data that starts in the payload and runs past its end.
    let mut b = with_data();
    put(&mut b, sfile(PAIR_AT), 0x200);
    let s = read(b);
    assert_eq!(s.data_pair_refusal(pair()), Some(DATA_RUNS_PAST));
    let mut s = s;
    assert!(s.resize_data(pair(), vec![1]).is_err());

    // Data whose bytes are a placement block's.
    let mut b = with_data();
    put(&mut b, sfile(PAIR_AT) + 4, weapons);
    let s = read(b);
    assert_eq!(s.data_pair_refusal(pair()), Some(DATA_OWNED));
}

#[test]
fn the_session_refuses_growth_while_it_is_off_and_undoes_a_resize() {
    let dir = std::env::temp_dir().join(format!("scenario-edit-data-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.map");
    std::fs::write(&path, with_data()).unwrap();
    let mut ed = crate::Session::new();
    ed.open(MapSource::file(&path)).unwrap();

    assert_eq!(ed.data_refusal(pair(), 9), Some(DATA_GROWTH_REFUSAL));
    assert!(ed.resize_data(pair(), vec![0; 9], "strings").is_err());
    assert!(!ed.is_dirty());
    assert_eq!(ed.data_refusal(pair(), 8), None);

    ed.resize_data(pair(), vec![5; 4], "strings").unwrap();
    assert!(ed.is_dirty());
    ed.set_allow_growth(true);
    assert_eq!(ed.data_refusal(pair(), 9), None);
    ed.resize_data(pair(), vec![5; 30], "strings").unwrap();
    assert_eq!(ed.scenario().unwrap().changes().tag_grown.len(), 1);
    // The same bytes again are no step.
    ed.resize_data(pair(), vec![5; 30], "strings").unwrap();
    assert!(ed.undo());
    assert_eq!(ed.scenario().unwrap().data(pair()), Some(vec![5; 4]));
    assert!(ed.undo());
    assert!(!ed.is_dirty());
    assert!(!ed.can_undo());
    std::fs::remove_dir_all(&dir).unwrap();
}
